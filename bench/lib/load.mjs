// Load generation: `oha` for throughput runs, a small keep-alive Node client for the cold-cache
// and start-up runs where every individual response matters.

import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import http from 'node:http';
import { join } from 'node:path';
import { promisify } from 'node:util';

import { BENCH_DIR, foreignSampler, procStats, rssSampler, sleep, waitQuiet } from './servers.mjs';

const execFileP = promisify(execFile);

export function ohaBin() {
  const local = join(BENCH_DIR, '.tools', 'bin', 'oha');
  return existsSync(local) ? local : 'oha';
}

/**
 * One oha run, time-boxed (`seconds`) or fixed-count (`count`). Returns the numbers we use,
 * latencies in ms. A time-boxed run cuts in-flight requests at the deadline (not counted);
 * the server may still be working on them afterwards, see `waitIdle`.
 */
export async function oha({ url, headers = {}, conns, seconds, count, noCompression = false }) {
  const args = count ? ['-n', String(count)] : ['-z', `${seconds}s`];
  // oha sends its own Accept-Encoding unless told not to
  if (noCompression) args.push('--disable-compression');
  args.push('-c', String(conns), '--no-tui', '--output-format', 'json');
  for (const [k, v] of Object.entries(headers)) args.push('-H', `${k}: ${v}`);
  args.push(url);
  const { stdout } = await execFileP(ohaBin(), args, { maxBuffer: 256 * 1024 * 1024 });
  const j = JSON.parse(stdout);
  const errors = { ...j.errorDistribution };
  delete errors['aborted due to deadline']; // in-flight requests cut by -z, not failures
  const statuses = j.statusCodeDistribution ?? {};
  const completed = Object.values(statuses).reduce((a, b) => a + b, 0);
  return {
    rps: completed / j.summary.total,
    completed,
    p50: (j.latencyPercentiles.p50 ?? NaN) * 1000,
    p99: (j.latencyPercentiles.p99 ?? NaN) * 1000,
    bytesPerSec: j.summary.sizePerSec ?? 0,
    sizePerRequest: j.summary.sizePerRequest ?? 0,
    seconds: j.summary.total,
    statuses,
    errors,
  };
}

const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

export function summarize(values) {
  const v = values.filter((x) => Number.isFinite(x));
  if (!v.length) return null;
  const med = median(v);
  const min = Math.min(...v);
  const max = Math.max(...v);
  return { median: med, min, max, spreadPct: med ? ((max - min) / med) * 100 : 0, n: v.length };
}

/**
 * Wait until the server uses less than 10% of a core over 300 ms, twice in a row (it may still
 * be serving requests a time-boxed oha run gave up on). Gives up after `maxMs`.
 */
export async function waitIdle(pid, maxMs = 300_000) {
  const t0 = performance.now();
  let quiet = 0;
  let prev = await procStats(pid);
  while (performance.now() - t0 < maxMs) {
    await sleep(300);
    const cur = await procStats(pid);
    quiet = cur.cpuSec - prev.cpuSec < 0.03 ? quiet + 1 : 0;
    prev = cur;
    if (quiet >= 2) return performance.now() - t0;
  }
  return performance.now() - t0;
}

/** Longest a fixed-count repetition may take, in seconds (bounds the slow Verdaccio cells). */
const MAX_COUNT_SECONDS = 120;

/**
 * Warm-up, then `reps` measured oha runs against `server`, sampling the server's CPU time and
 * RSS around each run (and the uplink's request counter when given).
 *
 * Mode: time-boxed (`-z seconds`) when the warm-up shows enough requests fit in the window
 * (at least max(200, 20 per connection)); otherwise fixed-count (`-n`), sized to about
 * `seconds` of work but at least 3 requests per connection, at most ~MAX_COUNT_SECONDS. A
 * time-boxed window over a server that answers a handful of requests per second would mostly
 * measure the requests it cut off.
 */
export async function measure(server, spec, { warmup, seconds, reps, uplink }) {
  const { conns } = spec;
  let est = null;
  if (warmup > 0) {
    const w = await oha({ ...spec, seconds: warmup });
    await waitIdle(server.pid);
    if (w.completed >= Math.max(20, conns * 2)) est = w.rps;
  }
  if (est === null) {
    // too slow to estimate from a short time-boxed run: a small fixed-count probe
    const p = await oha({ ...spec, count: Math.max(conns * 2, 8) });
    await waitIdle(server.pid);
    est = p.rps;
  }
  let mode = { seconds };
  if (est * seconds < Math.max(200, conns * 20)) {
    let count = Math.max(conns * 3, Math.ceil(est * seconds), 30);
    count = Math.min(count, Math.max(conns * 2, Math.ceil(est * MAX_COUNT_SECONDS)));
    mode = { count };
  }
  const runs = [];
  for (let i = 0; i < reps; i += 1) {
    const quiet = await waitQuiet();
    if (uplink) await uplink.reset();
    const before = await procStats(server.pid);
    const sampler = rssSampler(server.pid);
    const foreign = foreignSampler();
    const r = await oha({ ...spec, ...mode });
    const after = await procStats(server.pid);
    const { peakRssKiB } = await sampler.stop();
    const foreignCpuPct = await foreign.stop();
    const cpuSec = after.cpuSec - before.cpuSec;
    const run = {
      ...r,
      cpuSec,
      cpuPct: (cpuSec / r.seconds) * 100,
      reqPerCpuSec: cpuSec > 0 ? r.completed / cpuSec : null,
      peakRssKiB: Math.max(peakRssKiB, after.rssKiB),
      foreignCpuPct,
      quietWaitMs: quiet.waitedMs,
    };
    if (uplink) {
      const st = await uplink.stats();
      run.uplinkRequests = st.total;
      run.uplinkNotModified = st.notModified;
      run.uplinkPerRequest = r.completed ? st.total / r.completed : null;
    }
    runs.push(run);
    await waitIdle(server.pid);
  }
  const pick = (k) => summarize(runs.map((r) => r[k]));
  return {
    conns,
    mode: mode.count ? `n=${mode.count}` : `${seconds}s`,
    rps: pick('rps'),
    p50: pick('p50'),
    p99: pick('p99'),
    mbps: summarize(runs.map((r) => r.bytesPerSec / 1e6)),
    cpuPct: pick('cpuPct'),
    reqPerCpuSec: pick('reqPerCpuSec'),
    peakRssKiB: pick('peakRssKiB'),
    foreignCpuPct: pick('foreignCpuPct'),
    uplinkPerRequest: runs[0].uplinkPerRequest === undefined ? undefined : pick('uplinkPerRequest'),
    sizePerRequest: runs[0].sizePerRequest,
    statuses: mergeCounts(runs.map((r) => r.statuses)),
    errors: mergeCounts(runs.map((r) => r.errors)),
    runs: runs.map(({ rps, p50, p99, cpuPct, peakRssKiB, uplinkRequests, completed, seconds: s, foreignCpuPct, quietWaitMs }) => ({
      foreignCpuPct,
      quietWaitMs,
      rps,
      p50,
      p99,
      cpuPct,
      peakRssKiB,
      uplinkRequests,
      completed,
      seconds: s,
    })),
  };
}

function mergeCounts(list) {
  const out = {};
  for (const m of list) for (const [k, v] of Object.entries(m ?? {})) out[k] = (out[k] ?? 0) + v;
  return out;
}

// ---------------------------------------------------------------------------------------------
// Node client

const agent = new http.Agent({ keepAlive: true, maxSockets: Infinity });

/** GET `url`; resolves {status, ms, bytes, headers} once the whole body is read. */
export function get(url, headers = {}) {
  return new Promise((resolve, reject) => {
    const t0 = performance.now();
    const req = http.get(url, { agent, headers }, (res) => {
      let bytes = 0;
      res.on('data', (c) => {
        bytes += c.length;
      });
      res.on('end', () =>
        resolve({ status: res.statusCode, ms: performance.now() - t0, bytes, headers: res.headers }),
      );
      res.on('error', reject);
    });
    req.on('error', reject);
  });
}

/** GET `url` and return the body as text. */
export function getText(url, headers = {}) {
  return new Promise((resolve, reject) => {
    http
      .get(url, { agent, headers }, (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
        res.on('error', reject);
      })
      .on('error', reject);
  });
}

/** Open `n` keep-alive connections first so a burst measures requests, not TCP handshakes. */
export async function preconnect(base, n) {
  await Promise.all(Array.from({ length: n }, () => get(`${base}/-/ping`)));
}

export function percentile(xs, p) {
  const s = [...xs].sort((a, b) => a - b);
  if (!s.length) return null;
  const idx = Math.min(s.length - 1, Math.max(0, Math.ceil((p / 100) * s.length) - 1));
  return s[idx];
}
