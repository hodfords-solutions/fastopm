// Load client of the memory test (memtest.mjs). Runs inside the test's Docker network, next to
// the server, so neither Docker Desktop's port forwarding nor the host limits the request rate.
//
//   node lib/memload.mjs --target http://host:4873 --plan '<json>'
//
// plan: {rounds: [{name, requests: [{pkg, variant}]}], concurrency, readyTimeoutMs}
// Each round fires all its requests at once (at most `concurrency` in flight), reads every body
// to the end and discards it. Prints one JSON line per round on stdout:
//   {"round", "requests", "status": {code: n}, "errors": {code: n}, "bytes", "ms", "p50", "p99", "max"}
// and `{"ready": ms}` once `/-/ping` answers. Exit code 0 even when requests fail (the server
// being OOM-killed is a result, not a client error).

import http from 'node:http';

const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const target = new URL(opt('--target'));
const plan = JSON.parse(opt('--plan'));

const ACCEPT = {
  // what npm sends for `npm install`
  abbreviated: 'application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*',
  full: 'application/json',
};

const agent = new http.Agent({ keepAlive: true, maxSockets: plan.concurrency });

function get(path, headers) {
  return new Promise((resolve) => {
    const started = performance.now();
    const req = http.request(
      { host: target.hostname, port: target.port, path, headers, agent },
      (res) => {
        let bytes = 0;
        res.on('data', (chunk) => {
          bytes += chunk.length;
        });
        res.on('end', () =>
          resolve({ status: res.statusCode, bytes, ms: performance.now() - started }),
        );
        res.on('error', (e) =>
          resolve({ error: e.code ?? e.message, ms: performance.now() - started }),
        );
      },
    );
    req.on('error', (e) => resolve({ error: e.code ?? e.message, ms: performance.now() - started }));
    req.end();
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitReady(timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const r = await get('/-/ping', {});
    if (r.status === 200) return Date.now() - started;
    await sleep(100);
  }
  throw new Error('server not ready');
}

function percentile(sorted, p) {
  if (sorted.length === 0) return 0;
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))];
}

async function runRound(round) {
  const started = performance.now();
  const queue = round.requests.slice();
  const results = [];
  async function worker() {
    for (;;) {
      const next = queue.shift();
      if (!next) return;
      const path = `/${next.pkg.replace('/', '%2F')}`;
      const headers = { accept: ACCEPT[next.variant], 'accept-encoding': 'gzip,deflate' };
      results.push(await get(path, headers));
    }
  }
  await Promise.all(Array.from({ length: Math.min(plan.concurrency, queue.length) }, worker));
  const status = {};
  const errors = {};
  let bytes = 0;
  for (const r of results) {
    if (r.error) errors[r.error] = (errors[r.error] ?? 0) + 1;
    else status[r.status] = (status[r.status] ?? 0) + 1;
    bytes += r.bytes ?? 0;
  }
  const times = results.map((r) => r.ms).sort((a, b) => a - b);
  return {
    round: round.name,
    requests: results.length,
    status,
    errors,
    bytes,
    ms: Math.round(performance.now() - started),
    p50: Math.round(percentile(times, 50)),
    p99: Math.round(percentile(times, 99)),
    max: Math.round(times.at(-1) ?? 0),
  };
}

const ready = await waitReady(plan.readyTimeoutMs ?? 30_000);
console.log(JSON.stringify({ ready }));
for (const round of plan.rounds) {
  console.log(JSON.stringify(await runRound(round)));
  if (plan.pauseMs) await sleep(plan.pauseMs);
}
agent.destroy();
