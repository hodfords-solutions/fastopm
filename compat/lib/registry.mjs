// Start / stop a registry process (Verdaccio or a candidate binary) with the
// shared CLI contract: `<bin> --config <path> --listen <addr>`.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { request } from './http.mjs';
import { sleep } from './util.mjs';

const COMPAT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** Absolute path of the verdaccio CLI entry installed as devDependency. */
export function verdaccioBin() {
  // `verdaccio` has an exports map without ./package.json, so locate it directly
  const pkgPath = path.join(COMPAT_DIR, 'node_modules', 'verdaccio', 'package.json');
  if (!fs.existsSync(pkgPath)) {
    throw new Error(`verdaccio is not installed: run \`npm install\` in ${COMPAT_DIR}`);
  }
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  const rel = typeof pkg.bin === 'string' ? pkg.bin : pkg.bin.verdaccio;
  return { file: path.resolve(path.dirname(pkgPath), rel), version: pkg.version };
}

/** How to launch one side: `{ command, args }` before `--config/--listen`. */
export function launcherFor(candidateBin) {
  if (!candidateBin) {
    const { file } = verdaccioBin();
    return { command: process.execPath, args: [file], label: 'verdaccio' };
  }
  return { command: path.resolve(candidateBin), args: [], label: path.basename(candidateBin) };
}

// Environment variables that change registry behaviour; children never inherit
// them from the shell running the harness (scenarios set them explicitly).
const SCRUBBED_ENV = [
  /^VERDACCIO_/,
  /^NPM_TOKEN$/,
  /^(HTTP|HTTPS|NO|ALL)_PROXY$/i,
  /^DEBUG$/,
  /^NODE_OPTIONS$/,
  /^RUST_LOG$/,
];

export function childEnv(extra = {}) {
  const env = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!SCRUBBED_ENV.some((re) => re.test(k))) env[k] = v;
  }
  return { ...env, ...extra };
}

const live = new Set();

/** Kill every child still running (used on exit / signals). */
export function killAll(signal = 'SIGKILL') {
  for (const child of live) {
    try {
      child.kill(signal);
    } catch {
      /* already gone */
    }
  }
}

export async function startRegistry({ launcher, configPath, port, env, logFile, startTimeoutMs = 30_000 }) {
  const out = fs.openSync(logFile, 'a');
  const args = [...launcher.args, '--config', configPath, '--listen', `127.0.0.1:${port}`];
  const child = spawn(launcher.command, args, {
    env,
    cwd: path.dirname(configPath),
    stdio: ['ignore', out, out],
  });
  fs.closeSync(out);
  live.add(child);
  let exited = null;
  child.once('exit', (code, signal) => {
    exited = { code, signal };
    live.delete(child);
  });

  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + startTimeoutMs;
  while (Date.now() < deadline) {
    if (exited) break;
    const res = await request(url, { path: '/-/ping' }, 2000);
    if (res.status === 200) {
      return {
        url,
        port,
        child,
        async stop() {
          if (exited) return;
          child.kill('SIGTERM');
          const t = Date.now();
          while (!exited && Date.now() - t < 5000) await sleep(50);
          if (!exited) child.kill('SIGKILL');
          while (!exited && Date.now() - t < 7000) await sleep(50);
        },
      };
    }
    await sleep(100);
  }
  if (!exited) child.kill('SIGKILL');
  const log = fs.existsSync(logFile) ? fs.readFileSync(logFile, 'utf8').split('\n').slice(-30).join('\n') : '';
  throw new Error(
    `${launcher.label} did not answer GET /-/ping on ${url}` +
      (exited ? ` (exited code=${exited.code} signal=${exited.signal})` : ` within ${startTimeoutMs}ms`) +
      `\ncommand: ${launcher.command} ${args.join(' ')}\n--- log tail ---\n${log}`
  );
}
