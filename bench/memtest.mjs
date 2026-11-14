#!/usr/bin/env node
// Memory test: fastopm under a hard memory limit, cold cache, hundreds of parallel
// `GET /:package` for large real packuments (the `npm install` that got the pod OOM-killed).
//
//   node bench/memtest.mjs [--image fastopm:memtest] [--memory 512m,2g] [--cpus 6]
//                          [--rounds 3] [--clones 3] [--medium 60] [--small 200]
//                          [--concurrency 256] [--fastopm '<yaml of the fastopm: section>']
//                          [--label name] [--settle 15] [--env K=V ...] [--logs dir]
//
// For each memory limit, in a fresh Docker network:
//   - mock uplink (lib/uplink.mjs) in a `node` container, serving clones of the fixtures under
//     unique names `memtest-r<round>-<i>--<fixture>`, so every round is a cold cache;
//   - fastopm from `--image` with `--memory <limit> --memory-swap <limit> --cpus <cpus>`, the
//     image's own entrypoint, config below, storage inside the container;
//   - a sampler (`docker exec`) reading VmRSS/VmHWM of the fastopm process and the cgroup's
//     memory.current / memory.events every 200 ms;
//   - the load client (lib/memload.mjs) in a `node` container on the same network.
// Per round: all packages of the round requested at once, abbreviated (`Accept` as npm) and
// full, `--concurrency` in flight. Rounds `cold-N` use new names; the last round, `warm`,
// repeats the names of the last cold round. Then `--settle` seconds idle and one more sample.
//
// Records: OOM kill (Docker `State.OOMKilled`, exit code, memory.events oom_kill), peak RSS
// (VmHWM), RSS after the load stopped, per-round status counts and latency. Results:
// `bench/results/memtest-<label>.json` and a line per limit on stdout. Every container, the
// network and the temporary config directory are removed at the end (also on Ctrl-C).

import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const BENCH = dirname(fileURLToPath(import.meta.url));
const NODE_IMAGE = 'node:26.8';
const UPLINK_PORT = 4874;

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const image = opt('--image', 'fastopm:memtest');
const limits = opt('--memory', '512m,2g').split(',');
const cpus = opt('--cpus', '6');
const roundsCold = Number(opt('--rounds', '3'));
const clones = Number(opt('--clones', '3'));
const medium = Number(opt('--medium', '60'));
const small = Number(opt('--small', '200'));
const concurrency = Number(opt('--concurrency', '256'));
const fastopmSection = opt('--fastopm', '');
const label = opt('--label', image.replace(/[^a-z0-9.-]+/gi, '_'));
const settle = Number(opt('--settle', '15'));
// extra environment for the fastopm container, `--env K=V` (repeatable)
// save the full server log of each limit as <dir>/<label>-<limit>.log
const logDir = opt('--logs', '');
const extraEnv = args.flatMap((a, i) => (a === '--env' ? ['-e', args[i + 1]] : []));

// the large packuments of the incident, raw sizes 4–39 MB
const LARGE = [
  'typescript',
  'vite',
  'firebase__firestore',
  'types__node',
  'rollup',
  'webpack',
  'aws-sdk__client-s3',
  'zod',
];

function plan() {
  const rounds = [];
  const roundPackages = (r) => {
    const pkgs = [];
    for (const stem of LARGE)
      for (let i = 0; i < clones; i++) pkgs.push(`memtest-r${r}-${i}--${stem}`);
    for (let i = 0; i < medium; i++) pkgs.push(`memtest-r${r}-${i}--express`);
    for (let i = 0; i < small; i++) pkgs.push(`memtest-r${r}-${i}--ms`);
    return pkgs;
  };
  const requests = (pkgs) => {
    // interleave so large packages are not all at the start of the queue
    const out = [];
    const shuffled = pkgs
      .map((p, i) => [(i * 2654435761) % 4294967296, p])
      .sort((a, b) => a[0] - b[0])
      .map(([, p]) => p);
    for (const pkg of shuffled) {
      out.push({ pkg, variant: 'abbreviated' });
      out.push({ pkg, variant: 'full' });
    }
    return out;
  };
  let last = [];
  for (let r = 1; r <= roundsCold; r++) {
    last = roundPackages(r);
    rounds.push({ name: `cold-${r}`, requests: requests(last) });
  }
  rounds.push({ name: 'warm', requests: requests(last) });
  return { rounds, concurrency, readyTimeoutMs: 60_000, pauseMs: 2000 };
}

function configYaml(uplinkHost) {
  return `storage: /verdaccio/storage/data
uplinks:
  npmjs:
    url: http://${uplinkHost}:${UPLINK_PORT}/
    maxage: 30m
    timeout: 120s
packages:
  '@*/*':
    proxy: npmjs
  '**':
    proxy: npmjs
${fastopmSection ? `fastopm:\n${fastopmSection.replace(/^/gm, '  ')}\n` : ''}log: {type: stdout, format: pretty, level: http}
`;
}

const docker = (...a) => execFileSync('docker', a, { encoding: 'utf8' }).trim();
const dockerQuiet = (...a) => {
  try {
    return docker(...a);
  } catch {
    return '';
  }
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const spawnSyncText = (cmd, a) => {
  const r = spawnSync(cmd, a, { encoding: 'utf8', maxBuffer: 1 << 30 });
  return `${r.stdout ?? ''}${r.stderr ?? ''}`;
};

const cleanups = [];
function cleanup() {
  while (cleanups.length) {
    try {
      cleanups.pop()();
    } catch {
      // best effort
    }
  }
}
process.on('SIGINT', () => {
  cleanup();
  process.exit(130);
});

// one `sample` line every 200 ms: ms since epoch, VmRSS kB, VmHWM kB, memory.current, oom_kill
const SAMPLER = `
pid=
while [ -z "$pid" ]; do
  for p in /proc/[0-9]*; do
    [ "$(cat $p/comm 2>/dev/null)" = fastopm ] && pid=\${p#/proc/}
  done
  [ -z "$pid" ] && sleep 0.1
done
while [ -r /proc/$pid/status ]; do
  rss=$(sed -n 's/^VmRSS:[^0-9]*\\([0-9]*\\).*/\\1/p' /proc/$pid/status)
  hwm=$(sed -n 's/^VmHWM:[^0-9]*\\([0-9]*\\).*/\\1/p' /proc/$pid/status)
  cur=$(cat /sys/fs/cgroup/memory.current)
  oom=$(sed -n 's/^oom_kill \\([0-9]*\\)/\\1/p' /sys/fs/cgroup/memory.events)
  stat=$(awk '$1=="anon"||$1=="file"||$1=="file_dirty"||$1=="sock"||$1=="kernel"{printf "%s ", $2}' /sys/fs/cgroup/memory.stat)
  echo "sample $(date +%s%3N) $rss $hwm $cur $oom $stat"
  sleep 0.2
done
`;

function startSampler(container, samples) {
  const child = spawn('docker', ['exec', container, 'sh', '-c', SAMPLER], {
    stdio: ['ignore', 'pipe', 'ignore'],
  });
  let buf = '';
  child.stdout.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).split(' ');
      buf = buf.slice(i + 1);
      if (line[0] !== 'sample' || !line[2]) continue;
      samples.push({
        t: Number(line[1]),
        rssKiB: Number(line[2]),
        hwmKiB: Number(line[3]),
        cgroupBytes: Number(line[4]),
        oomKill: Number(line[5] || 0),
        // memory.stat in file order: anon, file, kernel, sock, file_dirty
        stat: line.slice(6, 11).map(Number),
      });
    }
  });
  return child;
}

function runClient(network, server, planJson, onLine) {
  return new Promise((resolve) => {
    const child = spawn(
      'docker',
      [
        'run', '--rm', '--network', network, '-v', `${BENCH}:/bench:ro`, NODE_IMAGE,
        'node', '/bench/lib/memload.mjs', '--target', `http://${server}:4873`, '--plan', planJson,
      ],
      { stdio: ['ignore', 'pipe', 'inherit'] },
    );
    let buf = '';
    child.stdout.on('data', (d) => {
      buf += d;
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        try {
          onLine(JSON.parse(line));
        } catch {
          process.stderr.write(`client: ${line}\n`);
        }
      }
    });
    child.on('exit', resolve);
  });
}

const MiB = (kib) => Math.round(kib / 1024);

function cgroupAtPeak(samples) {
  const top = samples.reduce((m, x) => (!m || x.cgroupBytes > m.cgroupBytes ? x : m), null);
  if (!top) return null;
  const keys = ['anon', 'file', 'kernel', 'sock', 'file_dirty'];
  return Object.fromEntries(keys.map((k, i) => [k, Math.round((top.stat[i] ?? 0) / 1048576)]));
}

async function runLimit(limit) {
  const tag = `fastopm-memtest-${process.pid}-${limit}`;
  const uplink = `${tag}-uplink`;
  const server = `${tag}-server`;
  docker('network', 'create', tag);
  cleanups.push(() => dockerQuiet('network', 'rm', tag));

  docker(
    'run', '-d', '--name', uplink, '--network', tag, '-v', `${BENCH}:/bench:ro`,
    '-e', `UPLINK_ORIGIN=http://${uplink}:${UPLINK_PORT}`,
    NODE_IMAGE, 'node', '/bench/lib/uplink.mjs', String(UPLINK_PORT), '0.0.0.0',
  );
  cleanups.push(() => dockerQuiet('rm', '-f', uplink));
  for (let i = 0; i < 100 && !dockerQuiet('logs', uplink).includes('"port"'); i++) await sleep(100);

  const conf = mkdtempSync(join(tmpdir(), 'fastopm-memtest-'));
  cleanups.push(() => rmSync(conf, { recursive: true, force: true }));
  writeFileSync(join(conf, 'config.yaml'), configYaml(uplink));
  // the container user (uid 10001) must read it
  execFileSync('chmod', ['0755', conf]);
  execFileSync('chmod', ['0644', join(conf, 'config.yaml')]);

  docker(
    'run', '-d', '--name', server, '--network', tag,
    '--memory', limit, '--memory-swap', limit, '--cpus', cpus,
    '-v', `${join(conf, 'config.yaml')}:/verdaccio/conf/config.yaml:ro`,
    ...extraEnv,
    image,
  );
  cleanups.push(() => dockerQuiet('rm', '-f', server));

  const samples = [];
  const sampler = startSampler(server, samples);
  cleanups.push(() => sampler.kill());

  const rounds = [];
  const started = Date.now();
  await runClient(tag, server, JSON.stringify(plan()), (line) => {
    if (line.round) {
      const sinceStart = Date.now() - started;
      const peak = samples.reduce((m, s) => Math.max(m, s.hwmKiB), 0);
      rounds.push({ ...line, peakRssMiB: MiB(peak), atMs: sinceStart });
      const errs = Object.entries(line.errors).map(([k, v]) => `${k}:${v}`).join(' ');
      console.log(
        `  [${limit}] ${line.round}: ${line.requests} req in ${line.ms} ms, status ${JSON.stringify(line.status)}${errs ? `, errors ${errs}` : ''}, p50 ${line.p50} ms, p99 ${line.p99} ms, peak RSS so far ${MiB(peak)} MiB`,
      );
    }
  });
  const loadEnded = Date.now();

  let state = JSON.parse(docker('inspect', '-f', '{{json .State}}', server));
  let settleRss = null;
  if (state.Running) {
    await sleep(settle * 1000);
    const last = samples.at(-1);
    settleRss = last ? MiB(last.rssKiB) : null;
    state = JSON.parse(docker('inspect', '-f', '{{json .State}}', server));
  }
  sampler.kill();
  // SIGTERM so the process can exit cleanly (and e.g. print allocator stats)
  if (state.Running) dockerQuiet('stop', '-t', '10', server);
  const logs = dockerQuiet('logs', '--tail', '5', server);
  if (logDir) {
    mkdirSync(logDir, { recursive: true });
    const full = spawnSyncText('docker', ['logs', server]);
    writeFileSync(join(logDir, `${label}-${limit}.log`), full);
  }
  const budgetLine = dockerQuiet('logs', server).split('\n').find((l) => l.includes('memory budgets')) ?? null;
  const last = samples.at(-1) ?? {};
  const peakHwm = samples.reduce((m, s) => Math.max(m, s.hwmKiB), 0);
  const peakCgroup = samples.reduce((m, s) => Math.max(m, s.cgroupBytes), 0);
  const result = {
    limit,
    image,
    oomKilled: state.OOMKilled || (last.oomKill ?? 0) > 0 || state.ExitCode === 137,
    running: state.Running,
    exitCode: state.ExitCode,
    oomKillEvents: last.oomKill ?? null,
    peakRssMiB: MiB(peakHwm),
    peakCgroupMiB: Math.round(peakCgroup / 1048576),
    rssAfterLoadMiB: settleRss,
    settleSeconds: settle,
    loadSeconds: Math.round((loadEnded - started) / 1000),
    rounds,
    extraEnv: extraEnv.filter((_, i) => i % 2 === 1),
    // [seconds, RSS MiB, cgroup MiB]
    timeline: samples
      .filter((_, i) => i % 5 === 0)
      .map((s) => [
        Math.round((s.t - started) / 100) / 10,
        MiB(s.rssKiB),
        Math.round(s.cgroupBytes / 1048576),
      ]),
    // cgroup memory.stat (MiB) at the highest cgroup usage seen
    cgroupAtPeak: cgroupAtPeak(samples),
    logTail: logs.split('\n').slice(-5),
    budgets: budgetLine,
  };
  cleanup();
  return result;
}

const results = [];
const p = plan();
console.log(
  `memtest ${label}: image ${image}, ${p.rounds.length} rounds × ${p.rounds[0].requests.length} requests, concurrency ${concurrency}, cpus ${cpus}`,
);
for (const limit of limits) {
  const r = await runLimit(limit);
  results.push(r);
  console.log(
    `${label} --memory ${limit}: ${r.oomKilled ? 'OOM-KILLED' : r.running ? 'survived' : `exited ${r.exitCode}`}, peak RSS ${r.peakRssMiB} MiB (cgroup ${r.peakCgroupMiB} MiB), RSS ${settle}s after load ${r.rssAfterLoadMiB ?? '-'} MiB`,
  );
}
mkdirSync(join(BENCH, 'results'), { recursive: true });
const out = join(BENCH, 'results', `memtest-${label}.json`);
writeFileSync(
  out,
  `${JSON.stringify({ label, image, date: new Date().toISOString(), options: { cpus, roundsCold, clones, medium, small, concurrency, fastopm: fastopmSection }, results }, null, 1)}\n`,
);
console.log(`wrote ${out}`);
