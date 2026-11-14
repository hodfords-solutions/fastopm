// Runs one scenario: two isolated sides (mock uplinks + registry + temp storage),
// identical steps on both, per-step and end-of-scenario comparison.

import fs from 'node:fs';
import path from 'node:path';

import { buildConfig } from './config.mjs';
import {
  compareResponses,
  compareStorage,
  compareUplinkRequests,
  makeNormaliser,
  recordEtag,
  snapshotStorage,
} from './compare.mjs';
import { request } from './http.mjs';
import { MockUplink } from './mock-uplink.mjs';
import { childEnv, startRegistry } from './registry.mjs';
import { ALLOWED_DIFFS } from './rules.mjs';
import { getFreePort, sleep } from './util.mjs';

/** Default Host header sent to both registries, so rewritten tarball URLs are identical. */
export const DEFAULT_HOST = 'localhost:4873';
/** Pause after each request so async storage writes and uplink calls land in the right step. */
const SETTLE_MS = 150;

const resolve = (v, side) => (typeof v === 'function' ? v(side) : v);

function describeRequest(req) {
  const hdrs = Object.entries(req.headers ?? {})
    .filter(([k]) => k.toLowerCase() !== 'host' || req.headers[k] !== DEFAULT_HOST)
    .map(([k, v]) => `${k}: ${typeof v === 'function' ? '<per side>' : v}`);
  return `${req.method ?? 'GET'} ${req.path}${hdrs.length ? `  [${hdrs.join('; ')}]` : ''}`;
}

/** Fills `side` in place, so a failure half-way still leaves everything started reachable for teardown. */
async function setUpSide(side, scenario, opts) {
  const { role, dir } = side;
  const storage = path.join(dir, 'storage');
  fs.mkdirSync(storage, { recursive: true });
  const uplinkDefs = scenario.uplinks ?? { upstream: {} };
  Object.assign(side, { storage, mocks: {}, last: null, responses: {}, etagBook: new Map() });
  for (const [name, def] of Object.entries(uplinkDefs)) {
    const mock = new MockUplink(name, def.packages ? { packages: def.packages } : {});
    await mock.start();
    side.mocks[name] = mock;
  }
  side.uplink = (name = Object.keys(side.mocks)[0]) => side.mocks[name];
  if (scenario.seed) await scenario.seed(storage, side);
  if (scenario.setup) await scenario.setup(side);

  const uplinks = {};
  for (const [name, def] of Object.entries(uplinkDefs)) {
    uplinks[name] = { origin: side.mocks[name].origin, config: def.config };
  }
  const { yaml } = buildConfig({
    storage,
    uplinks,
    packages: scenario.packages,
    extra: scenario.config,
    patch: role === 'candidate' ? opts.candidatePatch : undefined,
  });
  const configPath = path.join(dir, 'config.yaml');
  fs.writeFileSync(configPath, yaml);
  side.port = await getFreePort();
  side.server = await startRegistry({
    launcher: role === 'candidate' ? opts.candidate : opts.baseline,
    configPath,
    port: side.port,
    env: childEnv(scenario.env ?? {}),
    logFile: path.join(dir, 'server.log'),
  });
  const origins = { [`127.0.0.1:${side.port}`]: '{registry}' };
  for (const [name, mock] of Object.entries(side.mocks)) origins[`127.0.0.1:${mock.port}`] = `{uplink:${name}}`;
  side.norm = makeNormaliser(origins);
}

async function tearDownSide(side) {
  if (!side) return;
  await side.server?.stop().catch(() => {});
  await Promise.all(Object.values(side.mocks ?? {}).map((m) => m.stop().catch(() => {})));
}

function takeUplinkRequests(side) {
  const out = {};
  for (const [name, mock] of Object.entries(side.mocks)) out[name] = mock.takeRequests();
  return out;
}

/** Short one-line body for verbose output (textual bodies only). */
function preview(res) {
  const ct = String(res.headers?.['content-type'] ?? '');
  if (!res.body?.length || !/json|text/.test(ct) || res.headers?.['content-encoding']) return undefined;
  const oneLine = res.body.toString('utf8').replace(/\s+/g, ' ').trim();
  return oneLine.length > 160 ? `${oneLine.slice(0, 160)}…` : oneLine;
}

function summariseUplinks(reqs) {
  const parts = [];
  for (const [name, list] of Object.entries(reqs)) {
    for (const r of list) parts.push(`${name}: ${r.method} ${r.url} -> ${r.status}${r.headers['if-none-match'] ? ' (If-None-Match)' : ''}`);
  }
  return parts;
}

/**
 * @returns {Promise<object>} scenario result
 */
export async function runScenario(scenario, opts) {
  const started = Date.now();
  const allowed = ALLOWED_DIFFS[scenario.name];
  const result = { name: scenario.name, description: scenario.description, steps: [], storage: [], status: 'match' };
  const base = path.join(opts.tmpRoot, scenario.name.replace(/[^a-z0-9._-]+/gi, '_'));
  let sides = [];
  try {
    sides = ['baseline', 'candidate'].map((role) => ({ role, dir: path.join(base, role), mocks: {} }));
    const setups = await Promise.allSettled(
      sides.map((side) => {
        fs.mkdirSync(side.dir, { recursive: true });
        return setUpSide(side, scenario, opts);
      })
    );
    const failed = setups.find((x) => x.status === 'rejected');
    if (failed) throw failed.reason;
    const [B, C] = sides;

    let n = 0;
    for (const step of scenario.steps) {
      n++;
      if (step.do) await Promise.all(sides.map((s) => step.do(s)));
      if (step.wait) await sleep(step.wait);
      if (!step.request) continue;

      for (const s of sides) takeUplinkRequests(s); // drop anything not caused by this step
      const responses = await Promise.all(
        sides.map((s) => {
          const r = step.request;
          const headers = { host: DEFAULT_HOST };
          for (const [k, v] of Object.entries(r.headers ?? {})) {
            const val = resolve(v, s);
            if (k.toLowerCase() === 'host') delete headers.host;
            if (val !== undefined) headers[k] = val;
          }
          return request(s.server.url, {
            method: r.method ?? 'GET',
            path: resolve(r.path, s),
            headers,
            body: resolve(r.body, s),
          }, step.timeout);
        })
      );
      await sleep(step.settle ?? SETTLE_MS);
      const uplinkReqs = sides.map(takeUplinkRequests);
      responses.forEach((res, i) => (res.method = step.request.method ?? 'GET'));
      sides.forEach((s, i) => {
        s.last = responses[i];
        if (step.saveAs) s.responses[step.saveAs] = responses[i];
      });

      const diffs = [
        ...compareResponses(responses[0], responses[1], B, C),
        ...compareUplinkRequests(uplinkReqs[0], uplinkReqs[1], B.norm, C.norm),
      ];
      sides.forEach((s, i) => recordEtag(responses[i], s));
      const stepName = step.name ?? describeRequest(step.request);
      const stepAllowed = allowed && (!allowed.steps || allowed.steps.includes(stepName));
      result.steps.push({
        index: n,
        name: stepName,
        baseline: {
          status: responses[0].status,
          error: responses[0].error,
          body: preview(responses[0]),
          uplink: summariseUplinks(uplinkReqs[0]),
        },
        candidate: {
          status: responses[1].status,
          error: responses[1].error,
          body: preview(responses[1]),
          uplink: summariseUplinks(uplinkReqs[1]),
        },
        diffs,
        allowed: Boolean(stepAllowed && diffs.length),
      });
    }

    if (scenario.compareStorage !== false) {
      await sleep(SETTLE_MS);
      result.storage = compareStorage(snapshotStorage(B.storage), snapshotStorage(C.storage), B.norm, C.norm);
    }
  } catch (err) {
    result.status = 'error';
    result.error = err?.stack ?? String(err);
  } finally {
    await Promise.all(sides.map(tearDownSide));
    if (!opts.keep) fs.rmSync(base, { recursive: true, force: true });
    else result.dir = base;
  }

  if (result.status !== 'error') {
    const unexpected =
      result.steps.some((s) => s.diffs.length && !s.allowed) ||
      (result.storage.length > 0 && !(allowed && !allowed.steps));
    const anyDiff = result.steps.some((s) => s.diffs.length) || result.storage.length > 0;
    result.status = unexpected ? 'diff' : anyDiff ? 'allowed' : 'match';
    if (allowed) result.allowedReason = allowed.reason;
  }
  result.ms = Date.now() - started;
  return result;
}
