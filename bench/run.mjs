#!/usr/bin/env node
// fastopm vs verdaccio@7.1.0 benchmark. See bench/README.md.
//
//   node bench/run.mjs                      full run (~45 min)
//   node bench/run.mjs --quick              smoke run (~4 min): short runs, 1 repetition
//   node bench/run.mjs --only s1,s3         some scenarios (s1 s1i s2 s3 s4 s5 s6 s7 checks)
//   node bench/run.mjs --servers fastopm    some servers (fastopm, verdaccio, fastopm-1t)
//   options: --warmup S --duration S --reps N --keep (keep work dir) --logs (server logs)
//            --out FILE (default bench/results/latest.json)

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import { join, relative } from 'node:path';
import { parseArgs } from 'node:util';

import { get, getText, measure, ohaBin, percentile, preconnect, summarize } from './lib/load.mjs';
import { renderMarkdown } from './lib/report.mjs';
import { buildTemplate, PKGS, seedStorage } from './lib/storage.mjs';
import {
  BENCH_DIR,
  fastopmBin,
  procStats,
  ROOT_DIR,
  sleep,
  startServer,
  startUplink,
  verdaccioBin,
} from './lib/servers.mjs';

const { values: argv } = parseArgs({
  options: {
    quick: { type: 'boolean', default: false },
    only: { type: 'string' },
    servers: { type: 'string', default: 'fastopm,verdaccio,fastopm-1t' },
    warmup: { type: 'string' },
    duration: { type: 'string' },
    reps: { type: 'string' },
    keep: { type: 'boolean', default: false },
    logs: { type: 'boolean', default: false },
    out: { type: 'string', default: join(BENCH_DIR, 'results', 'latest.json') },
  },
});

const QUICK = argv.quick;
const OPTS = {
  warmup: Number(argv.warmup ?? (QUICK ? 1 : 3)),
  seconds: Number(argv.duration ?? (QUICK ? 2 : 10)),
  reps: Number(argv.reps ?? (QUICK ? 1 : 3)),
};
const ALL = ['s1', 's1i', 's2', 's3', 's4', 's5', 's6', 's7', 'checks'];
const ONLY = new Set(argv.only ? argv.only.split(',').map((s) => s.trim()) : ALL);
const SERVERS = argv.servers.split(',').map((s) => s.trim());
const MAIN = SERVERS.filter((s) => s !== 'fastopm-1t'); // the 1-thread variant only runs s1

const CONNS_S1 = QUICK ? [16] : [1, 16, 128];
const CONNS_S2 = QUICK ? [16] : [1, 16];
const NPM_ACCEPT_ABBR = 'application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*';
const ACCEPT_ENCODING = 'gzip,deflate'; // what npm (minipass-fetch) sends
const H_ABBR = { accept: NPM_ACCEPT_ABBR, 'accept-encoding': ACCEPT_ENCODING };
const H_FULL = { accept: 'application/json', 'accept-encoding': ACCEPT_ENCODING };
const WARM_MAXAGE = '30m';
const HERD = QUICK ? 20 : 100;
const DISTINCT = QUICK ? 20 : 100;
const UPLINK_DELAY_MS = 50;

const RUN_ID = new Date().toISOString().replace(/[:.]/g, '-');
const WORK = join(BENCH_DIR, '.work', `run-${RUN_ID}`);
mkdirSync(WORK, { recursive: true });

const results = {
  meta: machineInfo(),
  options: { ...OPTS, quick: QUICK, only: [...ONLY], servers: SERVERS, herd: HERD, distinct: DISTINCT },
  cells: [],
  cold: [],
  revalidate: [],
  resources: {},
  checks: {},
  notes: [],
};

const log = (...a) => console.log(`[${new Date().toISOString().slice(11, 19)}]`, ...a);
const want = (s) => ONLY.has(s);

function machineInfo() {
  const sh = (cmd, args) => {
    try {
      return execFileSync(cmd, args, { encoding: 'utf8' }).trim();
    } catch {
      return null;
    }
  };
  return {
    date: new Date().toISOString(),
    cpu: sh('sysctl', ['-n', 'machdep.cpu.brand_string']) ?? os.cpus()[0]?.model,
    cores: os.cpus().length,
    perfCores: sh('sysctl', ['-n', 'hw.perflevel0.physicalcpu']),
    effCores: sh('sysctl', ['-n', 'hw.perflevel1.physicalcpu']),
    memGiB: os.totalmem() / 2 ** 30,
    os: `${sh('sw_vers', ['-productName'])} ${sh('sw_vers', ['-productVersion'])} (${os.release()})`,
    node: process.version,
    rustc: sh('/opt/homebrew/opt/rustup/bin/rustc', ['--version']) ?? sh('rustc', ['--version']),
    oha: sh(ohaBin(), ['--version']),
    verdaccio: JSON.parse(readFileSync(join(verdaccioBin(), '..', '..', 'package.json'), 'utf8'))
      .version,
    fastopmBinary: relative(ROOT_DIR, fastopmBin()),
    fastopmBuilt: statSync(fastopmBin()).mtime.toISOString(),
  };
}

// ---------------------------------------------------------------------------------------------
// storage

async function template_(uplink) {
  log('building storage template with verdaccio');
  return buildTemplate(uplink, join(WORK, 'template'), WARM_MAXAGE);
}

let serial = 0;
function serverDir(kind, phase) {
  serial += 1;
  return join(WORK, `${String(serial).padStart(2, '0')}-${kind}-${phase}`);
}

async function withServer(kind, phase, { template, maxage = WARM_MAXAGE, uplink }, fn) {
  const dir = serverDir(kind, phase);
  const storage = join(dir, 'storage');
  if (template) seedStorage(template, storage);
  else mkdirSync(storage, { recursive: true });
  const srv = await startServer(kind, { dir, storage, uplink: uplink.origin, maxage, logs: argv.logs });
  try {
    return await fn(srv);
  } finally {
    await srv.stop();
    if (!argv.keep) rmSync(dir, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------------------------------------
// scenarios

async function cell(srv, scenario, cellInfo, spec, uplink) {
  const label = `${scenario} ${srv.kind} ${JSON.stringify(cellInfo)}`;
  log(label);
  const m = await measure(srv, spec, { ...OPTS, uplink });
  if ((m.foreignCpuPct?.max ?? 0) > 150)
    results.notes.push(`${label}: other processes used up to ${m.foreignCpuPct.max.toFixed(0)}% CPU during a repetition`);
  const bad = Object.keys(m.statuses).filter((s) => !['200', '304'].includes(s));
  if (bad.length || Object.keys(m.errors).length)
    results.notes.push(`${label}: statuses ${JSON.stringify(m.statuses)} errors ${JSON.stringify(m.errors)}`);
  results.cells.push({ scenario, kind: srv.kind, ...cellInfo, ...m });
  log(
    `  [${m.mode}] rps ${m.rps.median.toFixed(0)} (±${m.rps.spreadPct.toFixed(1)}%) p50 ${m.p50.median.toFixed(2)}ms ` +
      `p99 ${m.p99.median.toFixed(2)}ms cpu ${m.cpuPct.median.toFixed(0)}% rss ${(m.peakRssKiB.median / 1024).toFixed(0)}MiB foreign ${m.foreignCpuPct?.max?.toFixed(0)}%`,
  );
  return m;
}

async function warmPhase(kind, ctx) {
  const scenarios = kind === 'fastopm-1t' ? ['s1'] : ['s1', 's1i', 's2', 's3', 's4'];
  if (!scenarios.some(want) && !(want('s7') && kind !== 'fastopm-1t')) return;
  await withServer(kind, 'warm', ctx, async (srv) => {
    await sleep(2000);
    const idle = await procStats(srv.pid);
    const res = (results.resources[kind] ??= {});
    res.idleRssAfterStartKiB = idle.rssKiB;
    await ctx.uplink.reset();

    // sanity: every package answers 200 from the seeded storage
    for (const p of PKGS) {
      const r = await get(`${srv.url}/${p}`, H_ABBR);
      if (r.status !== 200) throw new Error(`${kind}: GET /${p} -> ${r.status}`);
    }
    const base = srv.url;
    if (want('s1'))
      for (const pkg of PKGS)
        for (const conns of CONNS_S1)
          await cell(srv, 's1', { pkg, conns }, { url: `${base}/${pkg}`, headers: H_ABBR, conns });
    if (kind === 'fastopm-1t') return;
    if (want('s1i'))
      for (const pkg of PKGS)
        await cell(srv, 's1i', { pkg, conns: 16 }, { url: `${base}/${pkg}`, headers: { accept: NPM_ACCEPT_ABBR }, conns: 16 });
    if (want('s2'))
      for (const pkg of PKGS)
        for (const conns of CONNS_S2)
          await cell(srv, 's2', { pkg, conns }, { url: `${base}/${pkg}`, headers: H_FULL, conns });
    if (want('s3'))
      for (const pkg of PKGS) {
        const first = await get(`${base}/${pkg}`, H_ABBR);
        const etag = first.headers.etag;
        const check = await get(`${base}/${pkg}`, { ...H_ABBR, 'if-none-match': etag });
        if (check.status !== 304) results.notes.push(`s3 ${kind} ${pkg}: If-None-Match ${etag} -> ${check.status}`);
        await cell(srv, 's3', { pkg, conns: 16, etag }, {
          url: `${base}/${pkg}`,
          headers: { ...H_ABBR, 'if-none-match': etag },
          conns: 16,
        });
      }
    if (want('s4'))
      for (const [file, size, conns] of [
        ['bench-tarball-1.0.0.tgz', '1MiB', 1],
        ['bench-tarball-1.0.0.tgz', '1MiB', 16],
        ['bench-tarball-2.0.0.tgz', '50MiB', 1],
        ['bench-tarball-2.0.0.tgz', '50MiB', 8],
      ]) {
        if (QUICK && conns === 1) continue;
        await cell(srv, 's4', { pkg: size, conns }, {
          url: `${base}/bench-tarball/-/${file}`,
          headers: { 'accept-encoding': ACCEPT_ENCODING },
          conns,
        });
      }
    const st = await ctx.uplink.stats();
    res.uplinkRequestsDuringWarm = st.total;
    await sleep(2000);
    res.idleRssAfterLoadKiB = (await procStats(srv.pid)).rssKiB;
  });
}

async function burst(urls, headers) {
  const t0 = performance.now();
  const rs = await Promise.all(urls.map((u) => get(u, headers)));
  const wall = performance.now() - t0;
  const ms = rs.map((r) => r.ms);
  const statuses = {};
  for (const r of rs) statuses[r.status] = (statuses[r.status] ?? 0) + 1;
  return {
    wallMs: wall,
    p50: percentile(ms, 50),
    p99: percentile(ms, 99),
    max: Math.max(...ms),
    statuses,
    bytes: rs.map((r) => r.bytes),
  };
}

async function coldPhase(kind, ctx) {
  const { uplink } = ctx;
  await withServer(kind, 'cold', { ...ctx, template: null }, async (srv) => {
    await preconnect(srv.url, HERD);
    const out = { kind, herdMeta: [], herdTarball: [], sequential: null, parallel: null };
    // (a) N clients ask for the same uncached package at once
    await uplink.setDelay(UPLINK_DELAY_MS);
    for (let rep = 0; rep < OPTS.reps; rep += 1) {
      const name = `bench-cold-${kind}-herd-${rep}`;
      await uplink.reset();
      log(`s5 ${kind} herd metadata ${name} x${HERD}`);
      const b = await burst(Array(HERD).fill(`${srv.url}/${name}`), H_ABBR);
      const st = await uplink.stats();
      delete b.bytes;
      out.herdMeta.push({ ...b, uplinkRequests: st.byPath[`/${name}`] ?? 0 });
      log(`  wall ${b.wallMs.toFixed(0)}ms uplink ${st.byPath[`/${name}`]} statuses ${JSON.stringify(b.statuses)}`);
    }
    // (b) N clients ask for the same uncached tarball at once (manifest already cached)
    for (let rep = 0; rep < OPTS.reps; rep += 1) {
      const name = `bench-tgz-${kind}-${rep}`;
      const m = await get(`${srv.url}/${name}`, H_ABBR);
      if (m.status !== 200) throw new Error(`${name} -> ${m.status}`);
      await uplink.reset();
      const path = `/${name}/-/${name}-1.0.0.tgz`;
      log(`s5 ${kind} herd tarball ${path} x${HERD}`);
      const b = await burst(Array(HERD).fill(`${srv.url}${path}`), { 'accept-encoding': ACCEPT_ENCODING });
      const st = await uplink.stats();
      const complete = b.bytes.filter((n) => n === 1024 * 1024).length;
      delete b.bytes;
      out.herdTarball.push({ ...b, complete, uplinkRequests: st.byPath[path] ?? 0 });
      log(`  wall ${b.wallMs.toFixed(0)}ms uplink ${st.byPath[path]} complete ${complete}/${HERD}`);
    }
    // (c) distinct uncached packages one after another, uplink without delay
    await uplink.setDelay(0);
    await uplink.reset();
    log(`s5 ${kind} ${DISTINCT} distinct packages, sequential`);
    {
      const ms = [];
      const statuses = {};
      const t0 = performance.now();
      for (let i = 0; i < DISTINCT; i += 1) {
        const r = await get(`${srv.url}/bench-cold-${kind}-seq-${i}`, H_ABBR);
        ms.push(r.ms);
        statuses[r.status] = (statuses[r.status] ?? 0) + 1;
      }
      const wall = performance.now() - t0;
      const st = await uplink.stats();
      out.sequential = {
        n: DISTINCT,
        wallMs: wall,
        p50: percentile(ms, 50),
        p99: percentile(ms, 99),
        statuses,
        uplinkRequests: st.total,
      };
      log(`  wall ${wall.toFixed(0)}ms p50 ${out.sequential.p50.toFixed(1)}ms uplink ${st.total}`);
    }
    // (d) same with 16 clients in parallel, each on its own packages
    await uplink.reset();
    log(`s5 ${kind} ${DISTINCT} distinct packages, 16 in parallel`);
    {
      const ms = [];
      const statuses = {};
      let next = 0;
      const t0 = performance.now();
      await Promise.all(
        Array.from({ length: 16 }, async () => {
          for (;;) {
            const i = next++;
            if (i >= DISTINCT) return;
            const r = await get(`${srv.url}/bench-cold-${kind}-par-${i}`, H_ABBR);
            ms.push(r.ms);
            statuses[r.status] = (statuses[r.status] ?? 0) + 1;
          }
        }),
      );
      const wall = performance.now() - t0;
      const st = await uplink.stats();
      out.parallel = {
        n: DISTINCT,
        conns: 16,
        wallMs: wall,
        p50: percentile(ms, 50),
        p99: percentile(ms, 99),
        statuses,
        uplinkRequests: st.total,
      };
      log(`  wall ${wall.toFixed(0)}ms p50 ${out.parallel.p50.toFixed(1)}ms uplink ${st.total}`);
    }
    results.cold.push(out);
  });
}

async function revalidatePhase(kind, ctx) {
  const { uplink } = ctx;
  await withServer(kind, 'revalidate', { ...ctx, maxage: '1ms' }, async (srv) => {
    for (const pkg of ['ms', 'express']) {
      for (const [conns, delay] of QUICK ? [[32, 0]] : [[1, 0], [32, 0], [32, 20]]) {
        await uplink.setDelay(delay);
        const label = { pkg, conns, uplinkDelayMs: delay };
        log(`s6 ${kind} ${JSON.stringify(label)}`);
        const m = await measure(srv, { url: `${srv.url}/${pkg}`, headers: H_ABBR, conns }, { ...OPTS, uplink });
        const bad = Object.keys(m.statuses).filter((s) => s !== '200');
        if (bad.length || Object.keys(m.errors).length)
          results.notes.push(`s6 ${kind} ${JSON.stringify(label)}: ${JSON.stringify(m.statuses)} ${JSON.stringify(m.errors)}`);
        results.revalidate.push({ kind, ...label, ...m });
        log(
          `  rps ${m.rps.median.toFixed(0)} p50 ${m.p50.median.toFixed(2)}ms p99 ${m.p99.median.toFixed(2)}ms ` +
            `uplink/req ${m.uplinkPerRequest?.median?.toFixed(3)} cpu ${m.cpuPct.median.toFixed(0)}%`,
        );
      }
    }
    await uplink.setDelay(0);
  });
}

async function startupPhase(kind, ctx) {
  const reps = QUICK ? 2 : 5;
  const times = [];
  const rss = [];
  for (let i = 0; i < reps; i += 1) {
    await withServer(kind, 'startup', { ...ctx, template: null }, async (srv) => {
      times.push(srv.startupMs);
      await sleep(1000);
      rss.push((await procStats(srv.pid)).rssKiB);
    });
  }
  const res = (results.resources[kind] ??= {});
  res.startupMs = summarize(times);
  res.idleRssEmptyKiB = summarize(rss);
  log(`s7 ${kind} startup ${res.startupMs.median.toFixed(0)}ms idle rss ${(res.idleRssEmptyKiB.median / 1024).toFixed(1)}MiB`);
}

/**
 * Design checks (PLAN.md section 3), single client, `typescript` full manifest, no compression:
 *   hot       same request repeated: whatever caching exists is used
 *   newBase   a new Host each time: the rendered body cannot come from a cache keyed by base URL
 *   fileChanged  package.json modified on disk before each request: the manifest must be re-read
 *   notModified  If-None-Match matching: 304
 */
async function checksPhase(kind, ctx) {
  await withServer(kind, 'checks', ctx, async (srv) => {
    const url = `${srv.url}/typescript`;
    const H = { accept: 'application/json' };
    const N = QUICK ? 5 : 20;
    const timeMany = async (fn, n) => {
      const ms = [];
      for (let i = 0; i < n; i += 1) ms.push(await fn(i));
      return summarize(ms);
    };
    await get(url, H);
    const hot = await timeMany(async () => (await get(url, H)).ms, N);
    const newBase = await timeMany(async (i) => (await get(url, { ...H, host: `h${kind}${i}.bench.test:4873` })).ms, Math.min(N, 10));
    const file = join(srv.dir, 'storage', 'typescript', 'package.json');
    let seen = 0;
    const fileChanged = await timeMany(async (i) => {
      const text = readFileSync(file, 'utf8');
      const tag = `bench${i}x${Date.now()}`;
      writeFileSync(file, text.replace('"dist-tags":{', `"dist-tags":{"${tag}":"5.0.4",`));
      await sleep(20);
      const t0 = performance.now();
      const r = await getText(url, H);
      const ms = performance.now() - t0;
      if (r.includes(`"${tag}"`)) seen += 1;
      return ms;
    }, Math.min(N, 10));
    const first = await get(url, H);
    const notModified = await timeMany(async () => {
      const r = await get(url, { ...H, 'if-none-match': first.headers.etag });
      if (r.status !== 304) throw new Error(`${kind} 304 check -> ${r.status}`);
      return r.ms;
    }, N);
    results.checks[kind] = {
      hotMs: hot,
      newBaseMs: newBase,
      fileChangedMs: fileChanged,
      fileChangeSeen: `${seen}/${Math.min(N, 10)}`,
      notModifiedMs: notModified,
      bodyBytes: first.bytes,
    };
    log(
      `checks ${kind} hot ${hot.median.toFixed(2)}ms newBase ${newBase.median.toFixed(2)}ms ` +
        `fileChanged ${fileChanged.median.toFixed(2)}ms (seen ${seen}) 304 ${notModified.median.toFixed(2)}ms`,
    );
  });
}

function footprint() {
  const du = (p) => {
    try {
      return Number(execFileSync('du', ['-sk', p], { encoding: 'utf8' }).split(/\s+/)[0]) * 1024;
    } catch {
      return null;
    }
  };
  const nm = join(verdaccioBin(), '..', '..', '..');
  results.resources.footprint = {
    fastopmBinaryBytes: statSync(fastopmBin()).size,
    verdaccioNodeModulesBytes: du(nm),
    verdaccioNodeModulesPath: relative(ROOT_DIR, nm),
    nodeBinaryBytes: statSync(process.execPath).size,
  };
}

// ---------------------------------------------------------------------------------------------

async function main() {
  if (!existsSync(fastopmBin())) throw new Error(`build first: ${fastopmBin()} (see bench/README.md)`);
  log(`work dir ${relative(ROOT_DIR, WORK)}; options ${JSON.stringify(results.options)}`);
  const uplink = await startUplink();
  try {
    const template = await template_(uplink);
    const ctx = { uplink, template };
    for (const kind of SERVERS) await warmPhase(kind, ctx);
    if (want('checks')) for (const kind of MAIN) await checksPhase(kind, ctx);
    if (want('s5')) for (const kind of MAIN) await coldPhase(kind, ctx);
    if (want('s6')) for (const kind of MAIN) await revalidatePhase(kind, ctx);
    if (want('s7')) {
      for (const kind of MAIN) await startupPhase(kind, ctx);
      footprint();
    }
  } finally {
    await uplink.stop();
    if (!argv.keep) rmSync(WORK, { recursive: true, force: true });
  }
  mkdirSync(join(BENCH_DIR, 'results'), { recursive: true });
  writeFileSync(argv.out, `${JSON.stringify(results, null, 2)}\n`);
  const md = renderMarkdown(results);
  const mdPath = argv.out.replace(/\.json$/, '.md');
  writeFileSync(mdPath, md);
  log(`wrote ${relative(process.cwd(), argv.out)} and ${relative(process.cwd(), mdPath)}`);
}

main().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
