#!/usr/bin/env node
// Where fastopm spends its time under load: macOS `sample` (no root needed) on the fastopm process
// while oha drives one scenario. Writes the raw `sample` report and a summary
// (self time by function, inclusive time of fastopm's own functions) to bench/results/.
//
//   node bench/profile.mjs                 every case
//   node bench/profile.mjs abbr-ms tarball-50
//   options: --seconds N (sampling time, default 8)
//
// Cases:
//   abbr-ms        S1: abbreviated `ms`, gzip, c=128 (per-request overhead)
//   abbr-ts        S1: abbreviated `typescript`, gzip, c=16 (large body)
//   full-ts        S2: full `typescript`, gzip, c=16
//   not-modified   S3: `express` with matching If-None-Match, c=128
//   tarball-50     S4: 50 MiB tarball, c=8
//   revalidate     S6: `express`, maxage 1ms (every request revalidates), c=32
//   cold-distinct  S5: distinct uncached packages (express-sized), 16 clients

import { execFile, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs, promisify } from 'node:util';

import { get, oha } from './lib/load.mjs';
import { BENCH_DIR, sleep, startServer, startUplink } from './lib/servers.mjs';
import { buildTemplate, seedStorage } from './lib/storage.mjs';

const execFileP = promisify(execFile);
const { values: argv, positionals } = parseArgs({
  allowPositionals: true,
  options: { seconds: { type: 'string', default: '8' } },
});
const SECONDS = Number(argv.seconds);

const ABBR = {
  accept: 'application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*',
  'accept-encoding': 'gzip,deflate',
};
const FULL = { accept: 'application/json', 'accept-encoding': 'gzip,deflate' };

const CASES = {
  'abbr-ms': { path: '/ms', headers: ABBR, conns: 128 },
  'abbr-ts': { path: '/typescript', headers: ABBR, conns: 16 },
  'full-ts': { path: '/typescript', headers: FULL, conns: 16 },
  'not-modified': { path: '/express', headers: ABBR, conns: 128, etag: true },
  'tarball-50': { path: '/bench-tarball/-/bench-tarball-2.0.0.tgz', headers: {}, conns: 8 },
  revalidate: { path: '/express', headers: ABBR, conns: 32, maxage: '1ms' },
  'cold-distinct': { cold: true, conns: 16 },
};

const selected = positionals.length ? positionals : Object.keys(CASES);
const WORK = join(BENCH_DIR, '.work', `profile-${Date.now()}`);
const OUT = join(BENCH_DIR, 'results', 'profile');
mkdirSync(OUT, { recursive: true });

/** Rust v0 symbols are not demangled by `sample`; use rustfilt when available. */
function demangle(text) {
  const bin = join(BENCH_DIR, '.tools', 'bin', 'rustfilt');
  if (!existsSync(bin)) return text;
  const r = spawnSync(bin, [], { input: text, encoding: 'utf8', maxBuffer: 1 << 30 });
  return r.status === 0 ? r.stdout : text;
}

/** For leaf frames matching `leafRe`, sum samples by the nearest caller not matching `skipRe`. */
function callersOf(nodes, leafRe, skipRe) {
  const out = new Map();
  const stack = [];
  for (const n of nodes) {
    while (stack.length && stack[stack.length - 1].depth >= n.depth) stack.pop();
    if (leafRe.test(n.sym)) {
      const caller = [...stack].reverse().find((s) => !skipRe.test(s.sym));
      const key = caller ? caller.sym : '?';
      out.set(key, (out.get(key) ?? 0) + n.count);
    }
    stack.push(n);
  }
  return [...out].sort((a, b) => b[1] - a[1]);
}

/** Parse the call graph of a `sample` report into nodes {depth, count, symbol}. */
function parseCallGraph(text) {
  const start = text.indexOf('Call graph:');
  const end = text.indexOf('Total number in stack');
  const lines = text.slice(start, end > 0 ? end : undefined).split('\n').slice(1);
  const nodes = [];
  for (const line of lines) {
    const m = line.match(/^([\s+!:|]*)(\d+)\s+(.*)$/);
    if (!m) continue;
    const depth = m[1].length;
    const sym = m[3]
      .replace(/\s+\(in [^)]+\).*$/, '')
      .replace(/\s+\[0x[0-9a-f,]+\]$/, '')
      .replace(/\s+\+\s+\d+$/, '')
      .trim();
    nodes.push({ depth, count: Number(m[2]), sym });
  }
  return nodes;
}

/** Inclusive samples per symbol (a symbol counted once per stack even when recursive). */
function inclusive(nodes) {
  const totals = new Map();
  const stack = [];
  for (const n of nodes) {
    while (stack.length && stack[stack.length - 1].depth >= n.depth) stack.pop();
    if (!stack.some((s) => s.sym === n.sym)) totals.set(n.sym, (totals.get(n.sym) ?? 0) + n.count);
    stack.push(n);
  }
  return totals;
}

function selfTime(text) {
  const start = text.indexOf('Sort by top of stack');
  if (start < 0) return [];
  const out = [];
  for (const line of text.slice(start).split('\n').slice(1)) {
    const m = line.match(/^\s+(.+?)\s+(\d+)\s*$/);
    if (!m) {
      if (out.length && !line.trim()) break;
      continue;
    }
    out.push({ sym: m[1].replace(/\s+\(in ([^)]+)\)/, ' [$1]'), count: Number(m[2]) });
  }
  return out;
}

const short = (s) =>
  s
    .replace(/::h[0-9a-f]{16}$/, '')
    .replace(/<impl [^>]*>/g, '<impl>')
    .slice(0, 140);

const IDLE_RE = /__psynch_cvwait|kevent|__workq_kernreturn|semaphore_wait|mach_msg|__semwait/;
const LOCK_RE = /__psynch_mutexwait/;

function summarize(name, text, extra) {
  const nodes = parseCallGraph(text);
  const total = nodes.filter((n) => /^Thread_\d+/.test(n.sym)).reduce((a, n) => a + n.count, 0);
  const threads = nodes.filter((n) => /^Thread_\d+/.test(n.sym)).length;
  const incl = [...inclusive(nodes)].sort((a, b) => b[1] - a[1]);
  const top = selfTime(text);
  const sum = (re) => top.filter((b) => re.test(b.sym)).reduce((a, b) => a + b.count, 0);
  const idle = sum(IDLE_RE);
  const lock = sum(LOCK_RE);
  const onCpu = Math.max(1, top.reduce((a, b) => a + b.count, 0) - idle - lock);
  const active = onCpu + lock;
  const lines = [];
  lines.push(`## ${name}\n`);
  lines.push(`${extra}\n`);
  lines.push(
    `${threads} threads, ${total} thread-samples over ${SECONDS}s (1 ms interval): ` +
      `${onCpu} on CPU (≈${(onCpu / (SECONDS * 1000)).toFixed(1)} cores), ` +
      `${lock} blocked on a mutex (≈${(lock / (SECONDS * 1000)).toFixed(1)} threads), rest idle.\n`,
  );
  lines.push('Self time (top of stack, on-CPU only), top 25:\n');
  lines.push('| samples | % of on-CPU | function |\n|---:|---:|---|');
  for (const b of top.filter((x) => !IDLE_RE.test(x.sym) && !LOCK_RE.test(x.sym)).slice(0, 25))
    lines.push(`| ${b.count} | ${((b.count / onCpu) * 100).toFixed(1)} | \`${short(b.sym)}\` |`);
  lines.push('\nInclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:\n');
  lines.push('| samples | % of on-CPU+mutex | function |\n|---:|---:|---|');
  const interesting = incl.filter(
    ([s]) =>
      /fastopm|serde_json|md5|md_5|flate2|miniz|zlib|brotli|tokio::fs|spawn_blocking|axum::serve|tracing_core|Stdout|writev|write_all|recvfrom|stat\b|sendfile|memmove|indexmap|reqwest/i.test(s) &&
      !/^Thread_|start_wqthread|_pthread_start|thread_start|fastopm::main|block_on|pool::Spawner>::spawn_thread|__rust_begin_short_backtrace/.test(s),
  );
  for (const [s, c] of interesting.slice(0, 40))
    lines.push(`| ${c} | ${((c / active) * 100).toFixed(1)} | \`${short(s)}\` |`);
  lines.push('\nMutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:\n');
  lines.push('| samples | caller |\n|---:|---|');
  const skip = /mutex|Mutex|pthread|psynch|parking_lot|lock_api|MutexGuard|lock_contended|^lock$|::lock$|std::sync/;
  for (const [sym, c] of callersOf(nodes, LOCK_RE, skip).slice(0, 10)) lines.push(`| ${c} | \`${short(sym)}\` |`);
  return `${lines.join('\n')}\n`;
}

async function runCase(name, uplink, template) {
  const c = CASES[name];
  const dir = join(WORK, name);
  const storage = join(dir, 'storage');
  if (c.cold) mkdirSync(storage, { recursive: true });
  else seedStorage(template, storage);
  const srv = await startServer('fastopm', {
    dir,
    storage,
    uplink: uplink.origin,
    maxage: c.maxage ?? '30m',
  });
  try {
    const headers = { ...c.headers };
    let load;
    if (c.cold) {
      let i = 0;
      let stop = false;
      load = (async () => {
        await Promise.all(
          Array.from({ length: c.conns }, async () => {
            while (!stop) await get(`${srv.url}/bench-cold-profile-${i++}`, ABBR);
          }),
        );
      })();
      load.stop = () => {
        stop = true;
      };
    } else {
      if (c.etag) headers['if-none-match'] = (await get(`${srv.url}${c.path}`, headers)).headers.etag;
      await oha({ url: `${srv.url}${c.path}`, headers, conns: c.conns, seconds: 2 }); // warm-up
      load = oha({ url: `${srv.url}${c.path}`, headers, conns: c.conns, seconds: SECONDS + 3 });
    }
    await sleep(1500);
    const file = join(OUT, `${name}.sample.txt`);
    await execFileP('sample', [String(srv.pid), String(SECONDS), '1', '-mayDie', '-file', file]);
    let extra;
    if (c.cold) {
      load.stop();
      await load;
      extra = `Load: distinct uncached packages, ${c.conns} clients.`;
    } else {
      const r = await load;
      extra = `Load: \`GET ${c.path}\` c=${c.conns}, ${JSON.stringify(c.headers)}${c.etag ? ' + If-None-Match' : ''}${
        c.maxage ? `, maxage ${c.maxage}` : ''
      }: ${Math.round(r.rps)} req/s, statuses ${JSON.stringify(r.statuses)}.`;
    }
    const summary = summarize(name, demangle(readFileSync(file, 'utf8')), extra);
    writeFileSync(join(OUT, `${name}.md`), summary);
    console.log(summary);
  } finally {
    await srv.stop();
  }
}

async function main() {
  const uplink = await startUplink();
  try {
    const template = await buildTemplate(uplink, join(WORK, 'template'), '30m');
    for (const name of selected) {
      if (!CASES[name]) throw new Error(`unknown case ${name}; known: ${Object.keys(CASES).join(', ')}`);
      console.log(`\n=== ${name}`);
      await runCase(name, uplink, template);
    }
  } finally {
    await uplink.stop();
    rmSync(WORK, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
