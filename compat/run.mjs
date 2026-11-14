#!/usr/bin/env node
// Differential compatibility harness: Verdaccio (baseline) vs a candidate registry.
// Usage: node run.mjs [--candidate-bin <path>] [options]   (see README.md or --help)

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { parseArgs } from 'node:util';

import { launcherFor, killAll, verdaccioBin } from './lib/registry.mjs';
import { printScenario, printSummary } from './lib/report.mjs';
import { runScenario } from './lib/runner.mjs';
import { mapLimit } from './lib/util.mjs';
import { SCENARIOS } from './scenarios/index.mjs';

const HELP = `Usage: node run.mjs [options]

  --candidate-bin <path>      registry binary to test (CLI: --config <path> --listen <addr>);
                              default: verdaccio itself (self-check of the harness)
  --candidate-config <json>   JSON deep-merged into the candidate's config.yaml only
                              (e.g. '{"url_prefix":"/x/"}' to check the harness sees a difference)
  --filter <text|/re/>        run only scenarios whose name matches (repeatable)
  --jobs <n>                  scenarios run in parallel (default 4)
  --verbose                   print every step with statuses and uplink calls
  --keep                      keep temp dirs (configs, storages, server logs)
  --json <file>               write the full result as JSON
  --list                      list scenarios and exit
  -h, --help

Exit code: 0 no unexpected difference, 1 differences outside rules.mjs ALLOWED_DIFFS, 2 harness error.`;

const { values } = parseArgs({
  options: {
    'candidate-bin': { type: 'string' },
    'candidate-config': { type: 'string' },
    filter: { type: 'string', multiple: true },
    jobs: { type: 'string', default: '4' },
    verbose: { type: 'boolean', default: false },
    keep: { type: 'boolean', default: false },
    json: { type: 'string' },
    list: { type: 'boolean', default: false },
    help: { type: 'boolean', short: 'h', default: false },
  },
});

if (values.help) {
  console.log(HELP);
  process.exit(0);
}

const matchers = (values.filter ?? []).map((f) =>
  f.startsWith('/') && f.endsWith('/') ? ((n) => new RegExp(f.slice(1, -1)).test(n)) : (n) => n.includes(f)
);
const selected = SCENARIOS.filter((s) => !matchers.length || matchers.some((m) => m(s.name)));

if (values.list) {
  for (const s of SCENARIOS) console.log(`${s.name}\n    ${s.description}`);
  process.exit(0);
}
if (!selected.length) {
  console.error('no scenario matches the filter');
  process.exit(2);
}

let candidatePatch;
if (values['candidate-config']) {
  try {
    candidatePatch = JSON.parse(values['candidate-config']);
  } catch (err) {
    console.error(`--candidate-config is not valid JSON: ${err.message}`);
    process.exit(2);
  }
}
if (values['candidate-bin'] && !fs.existsSync(values['candidate-bin'])) {
  console.error(`--candidate-bin not found: ${values['candidate-bin']}`);
  process.exit(2);
}

const baseline = launcherFor(undefined);
const candidate = launcherFor(values['candidate-bin']);
const { version } = verdaccioBin();
const baselineLabel = `verdaccio@${version}`;
const candidateLabel =
  (values['candidate-bin'] ? path.resolve(values['candidate-bin']) : `verdaccio@${version}`) +
  (candidatePatch ? ` + config ${JSON.stringify(candidatePatch)}` : '');

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'fastopm-compat-'));
let cleaned = false;
function cleanup() {
  if (cleaned) return;
  cleaned = true;
  killAll();
  if (!values.keep) fs.rmSync(tmpRoot, { recursive: true, force: true });
}
process.on('exit', cleanup);
for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(sig, () => {
    cleanup();
    process.exit(130);
  });
}

console.log(`baseline : ${baselineLabel}`);
console.log(`candidate: ${candidateLabel}`);
console.log(`scenarios: ${selected.length}${values.keep ? `  (temp: ${tmpRoot})` : ''}\n`);

const t0 = Date.now();
const opts = {
  baseline,
  candidate,
  candidatePatch,
  candidateIsBin: Boolean(values['candidate-bin']),
  tmpRoot,
  keep: values.keep,
};
const results = await mapLimit(selected, Math.max(1, Number(values.jobs) || 1), async (s) => {
  const r = await runScenario(s, opts);
  printScenario(r, { verbose: values.verbose });
  return r;
});

// stable order for the summary
results.sort((a, b) => selected.findIndex((s) => s.name === a.name) - selected.findIndex((s) => s.name === b.name));
printSummary(results, { baselineLabel, candidateLabel, totalMs: Date.now() - t0 });

if (values.json) {
  fs.writeFileSync(
    values.json,
    JSON.stringify({ baseline: baselineLabel, candidate: candidateLabel, results }, null, 2) + '\n'
  );
}

const hasError = results.some((r) => r.status === 'error');
const hasDiff = results.some((r) => r.status === 'diff');
process.exitCode = hasError ? 2 : hasDiff ? 1 : 0;

// Everything is torn down per scenario; if a handle still keeps the loop alive, say so and leave.
cleanup();
setTimeout(() => {
  console.error('warning: open handles after the run, forcing exit');
  process.exit(process.exitCode);
}, 5000).unref();
