#!/usr/bin/env node
// End-to-end test with real clients against the real npmjs registry (needs Internet).
// Not part of the offline harness (`compat/run.mjs`). See e2e/README.md.
//
// For every client found on the machine (npm, pnpm, yarn classic or berry, bun) and every
// server (fastopm, verdaccio@7.1.0 from compat/node_modules), with Verdaccio's default
// config.yaml and an empty storage:
//
//   direct          install straight from registry.npmjs.org (reference time, gives the npmjs lockfile)
//   cold            through the server, empty storage, no lockfile        -> server lockfile
//   warm            server lockfile, empty client cache                    -> 0 tarballs from the uplink
//   npmjs-lock      lockfile generated against npmjs                       -> integrity equal to the server's
//   offline-lock    uplinks.npmjs.url -> closed port, server lockfile      -> 0 uplink tarballs, success
//   offline-nolock  same, no lockfile (resolution from cached metadata)
//   cross           the other server, offline, on a copy of this server's storage, with its lockfile
//
// Every round uses a fresh project directory and a fresh fake HOME (client caches and config
// live there), so the user's npm/pnpm/yarn/bun configuration is never read or written.

import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { childEnv, killAll, startRegistry, verdaccioBin } from '../lib/registry.mjs';
import { getFreePort } from '../lib/util.mjs';

const E2E_DIR = path.dirname(fileURLToPath(import.meta.url));
const COMPAT_DIR = path.resolve(E2E_DIR, '..');
const ROOT = path.resolve(COMPAT_DIR, '..');
const NPMJS = 'https://registry.npmjs.org/';
const DEFAULT_CONFIG = path.join(COMPAT_DIR, 'node_modules/@verdaccio/config/build/conf/default.yaml');
const INSTALL_TIMEOUT_MS = 20 * 60_000;
const FASTOPM_LOG_FILTER = 'warn,fastopm_server::access=info,fastopm_uplink=info,fastopm_store=info';

// ------------------------------------------------------------------ arguments

function parseArgs(argv) {
  const opts = {
    fastopmBin: path.join(ROOT, 'target/release/fastopm'),
    clients: null,
    servers: ['fastopm', 'verdaccio'],
    work: null,
    keep: false,
    json: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      return argv[++i];
    };
    if (a === '--fastopm-bin') opts.fastopmBin = path.resolve(next());
    else if (a === '--clients') opts.clients = next().split(',').filter(Boolean);
    else if (a === '--servers') opts.servers = next().split(',').filter(Boolean);
    else if (a === '--work') opts.work = path.resolve(next());
    else if (a === '--keep') opts.keep = true;
    else if (a === '--json') opts.json = path.resolve(next());
    else if (a === '-h' || a === '--help') {
      console.log(fs.readFileSync(path.join(E2E_DIR, 'README.md'), 'utf8'));
      process.exit(0);
    } else throw new Error(`unknown argument ${a}`);
  }
  return opts;
}

// ------------------------------------------------------------------ small helpers

const rm = (p) => fs.rmSync(p, { recursive: true, force: true });
const mkdir = (p) => fs.mkdirSync(p, { recursive: true });
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');
const fmtMs = (ms) => (ms == null ? '-' : `${(ms / 1000).toFixed(1)}s`);

function commandVersion(cmd, env) {
  const r = spawnSync(cmd, ['--version'], { encoding: 'utf8', env, timeout: 30_000, cwd: os.tmpdir() });
  if (r.error || r.status !== 0) return null;
  return r.stdout.trim().split('\n').pop().trim();
}

/** Run a command, output appended to `logFile`. Resolves `{ code, ms }`. */
function run(cmd, args, { cwd, env, logFile, timeoutMs = INSTALL_TIMEOUT_MS }) {
  return new Promise((resolve) => {
    fs.appendFileSync(logFile, `\n$ ${cmd} ${args.join(' ')}\n`);
    const out = fs.openSync(logFile, 'a');
    const started = performance.now();
    const child = spawn(cmd, args, { cwd, env, stdio: ['ignore', out, out] });
    fs.closeSync(out);
    const timer = setTimeout(() => child.kill('SIGKILL'), timeoutMs);
    child.on('error', (err) => {
      clearTimeout(timer);
      fs.appendFileSync(logFile, `spawn error: ${err.message}\n`);
      resolve({ code: -1, ms: performance.now() - started });
    });
    child.on('exit', (code, signal) => {
      clearTimeout(timer);
      resolve({ code: code ?? (signal ? 128 : -1), ms: performance.now() - started });
    });
  });
}

function tail(file, lines = 40) {
  if (!fs.existsSync(file)) return '';
  return stripAnsi(fs.readFileSync(file, 'utf8')).trimEnd().split('\n').slice(-lines).join('\n');
}

/** `name@version` -> integrity, from a lockfile. */
function splitSpec(spec) {
  const at = spec.lastIndexOf('@');
  return at > 0 ? [spec.slice(0, at), spec.slice(at + 1)] : [spec, ''];
}

// ------------------------------------------------------------------ clients

/** Client environment: fake HOME, no inherited client configuration. */
function clientEnv(home, extra) {
  const env = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (/^(npm_|yarn_|pnpm_|bun_|corepack_|node_options$|verdaccio_)/i.test(k)) continue;
    env[k] = v;
  }
  return {
    ...env,
    HOME: home,
    USERPROFILE: home,
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_CACHE_HOME: path.join(home, '.cache'),
    XDG_DATA_HOME: path.join(home, '.local/share'),
    XDG_STATE_HOME: path.join(home, '.local/state'),
    npm_config_userconfig: path.join(home, '.npmrc'),
    npm_config_globalconfig: path.join(home, '.npmrc-global'),
    npm_config_update_notifier: 'false',
    NO_UPDATE_NOTIFIER: '1',
    COREPACK_ENABLE_AUTO_PIN: '0',
    COREPACK_ENABLE_DOWNLOAD_PROMPT: '0',
    ...extra,
  };
}

function npmLock(text) {
  const out = new Map();
  for (const [key, entry] of Object.entries(JSON.parse(text).packages ?? {})) {
    if (!key || !entry.version || !entry.integrity || entry.link) continue;
    const name = entry.name ?? key.slice(key.lastIndexOf('node_modules/') + 'node_modules/'.length);
    out.set(`${name}@${entry.version}`, entry.integrity);
  }
  return out;
}

function pnpmLock(text) {
  const out = new Map();
  let inPackages = false;
  let current = null;
  for (const line of text.split('\n')) {
    if (/^\S/.test(line)) {
      inPackages = line.startsWith('packages:');
      current = null;
      continue;
    }
    if (!inPackages) continue;
    const key = line.match(/^ {2}(?:'([^']+)'|([^\s'][^:]*)):\s*$/);
    if (key) {
      current = (key[1] ?? key[2]).replace(/\(.*$/, '');
      continue;
    }
    const integrity = line.match(/resolution: \{.*integrity: ([^,}\s]+)/);
    if (current && integrity) out.set(current, integrity[1]);
  }
  return out;
}

function yarnClassicLock(text) {
  const out = new Map();
  for (const block of text.split(/\n(?=\S)/)) {
    const header = block.split('\n')[0];
    if (!header.endsWith(':') || header.startsWith('#')) continue;
    const first = header.slice(0, -1).split(',')[0].trim().replace(/^"|"$/g, '');
    const [name] = splitSpec(first);
    const version = block.match(/\n {2}version "([^"]+)"/)?.[1];
    const integrity = block.match(/\n {2}integrity "?([^\s"]+)"?/)?.[1];
    if (version && integrity) out.set(`${name}@${version}`, integrity);
  }
  return out;
}

function bunLock(text) {
  const out = new Map();
  for (const m of text.matchAll(/\["((?:@[^@"\s]+\/)?[^@"\s]+)@([^"\s]+)", "[^"]*", \{[^\n]*?\}, "(sha\d+-[^"]+)"\]/g)) {
    out.set(`${m[1]}@${m[2]}`, m[3]);
  }
  return out;
}

const LOCAL_ORIGIN = /http:\/\/127\.0\.0\.1:\d+\//g;

/** Client adapters. `ctx`: { registry, home, cache, store, project, frozen }. */
function detectClients(wanted) {
  const env = clientEnv(fs.mkdtempSync(path.join(os.tmpdir(), 'e2e-detect-')), {});
  const clients = [];
  const skipped = [];
  const want = (n) => !wanted || wanted.includes(n);

  const npmVersion = want('npm') && commandVersion('npm', env);
  if (npmVersion) {
    clients.push({
      name: 'npm',
      version: npmVersion,
      lockfile: 'package-lock.json',
      command: (ctx) => [
        'npm',
        [ctx.frozen ? 'ci' : 'install', '--ignore-scripts', '--no-audit', '--no-fund', '--loglevel=warn'],
      ],
      env: (ctx) => ({ npm_config_cache: ctx.cache, npm_config_registry: ctx.registry }),
      parseLock: npmLock,
      // npm itself replaces registry.npmjs.org with the configured registry (replace-registry-host)
      adaptLock: (text, registry) => text.replace(LOCAL_ORIGIN, registry),
    });
  } else if (want('npm')) skipped.push('npm: not found');

  const pnpmVersion = want('pnpm') && commandVersion('pnpm', env);
  if (pnpmVersion) {
    clients.push({
      name: 'pnpm',
      version: pnpmVersion,
      lockfile: 'pnpm-lock.yaml',
      command: (ctx) => [
        'pnpm',
        [
          'install',
          '--ignore-scripts',
          '--store-dir',
          ctx.store,
          '--registry',
          ctx.registry,
          '--reporter',
          'append-only',
          ctx.frozen ? '--frozen-lockfile' : '--no-frozen-lockfile',
        ],
      ],
      env: (ctx) => ({ npm_config_registry: ctx.registry }),
      parseLock: pnpmLock,
      adaptLock: (text) => text, // pnpm-lock.yaml has no registry URLs
    });
  } else if (want('pnpm')) skipped.push('pnpm: not found');

  const yarnVersion = want('yarn') && commandVersion('yarn', env);
  if (yarnVersion && Number(yarnVersion.split('.')[0]) < 2) {
    clients.push({
      name: 'yarn',
      label: `yarn classic ${yarnVersion}`,
      version: yarnVersion,
      lockfile: 'yarn.lock',
      command: (ctx) => [
        'yarn',
        [
          'install',
          '--ignore-scripts',
          '--non-interactive',
          '--no-progress',
          '--cache-folder',
          ctx.cache,
          '--registry',
          ctx.registry,
          '--network-timeout',
          '600000',
          ...(ctx.frozen ? ['--frozen-lockfile'] : []),
        ],
      ],
      env: () => ({}),
      parseLock: yarnClassicLock,
      // yarn 1 downloads from the `resolved` URL as written: point it at the server under test
      adaptLock: (text, registry) =>
        text.replace(/https?:\/\/(?:127\.0\.0\.1:\d+|registry\.npmjs\.org|registry\.yarnpkg\.com)\//g, registry),
    });
  } else if (yarnVersion) {
    clients.push({
      name: 'yarn',
      label: `yarn berry ${yarnVersion}`,
      version: yarnVersion,
      lockfile: 'yarn.lock',
      setup: (ctx) =>
        fs.writeFileSync(
          path.join(ctx.project, '.yarnrc.yml'),
          [
            `npmRegistryServer: "${ctx.registry.replace(/\/$/, '')}"`,
            'unsafeHttpWhitelist: ["127.0.0.1"]',
            'enableGlobalCache: false',
            `cacheFolder: "${ctx.cache}"`,
            'nodeLinker: node-modules',
            'enableTelemetry: false',
            'enableScripts: false',
            '',
          ].join('\n')
        ),
      command: (ctx) => ['yarn', ['install', ctx.frozen ? '--immutable' : '--no-immutable']],
      env: () => ({ YARN_ENABLE_IMMUTABLE_INSTALLS: 'false' }),
      parseLock: () => null, // berry checksums are yarn's own, not npm integrity
      adaptLock: (text) => text,
    });
  } else if (want('yarn')) skipped.push('yarn: not found');

  const bunVersion = want('bun') && commandVersion('bun', env);
  if (bunVersion) {
    clients.push({
      name: 'bun',
      version: bunVersion,
      lockfile: 'bun.lock',
      command: (ctx) => [
        'bun',
        ['install', '--ignore-scripts', '--registry', ctx.registry, ...(ctx.frozen ? ['--frozen-lockfile'] : [])],
      ],
      env: (ctx) => ({ BUN_INSTALL_CACHE_DIR: ctx.cache, BUN_CONFIG_REGISTRY: ctx.registry }),
      parseLock: bunLock,
      adaptLock: (text, registry) => text.replace(LOCAL_ORIGIN, registry),
    });
  } else if (want('bun')) skipped.push('bun: not found');

  return { clients, skipped };
}

/** Checks on the installed tree: scoped packages, binaries, a binary that runs. */
function checkTree(project) {
  const nm = path.join(project, 'node_modules');
  const problems = [];
  for (const rel of [
    '@babel/core/package.json',
    '@types/node/package.json',
    'express/package.json',
    'webpack/package.json',
    '.bin/tsc',
    '.bin/eslint',
    '.bin/webpack',
    '.bin/prettier',
    '.bin/semver',
    '.bin/rimraf',
  ]) {
    if (!fs.existsSync(path.join(nm, rel))) problems.push(`missing node_modules/${rel}`);
  }
  if (!problems.length) {
    const r = spawnSync(path.join(nm, '.bin/tsc'), ['--version'], { encoding: 'utf8', cwd: project, timeout: 60_000 });
    if (r.status !== 0 || !r.stdout.includes('5.7.2')) problems.push(`tsc --version: ${r.stdout}${r.stderr}`.trim());
  }
  return problems;
}

// ------------------------------------------------------------------ servers

function serverConfig({ offlinePort } = {}) {
  let text = fs.readFileSync(DEFAULT_CONFIG, 'utf8');
  if (offlinePort) {
    const before = text;
    text = text.replace(/url: https:\/\/registry\.npmjs\.org\/?/, `url: http://127.0.0.1:${offlinePort}/`);
    if (text === before) throw new Error('could not rewrite uplinks.npmjs.url in the default config');
  }
  return text;
}

function launcher(kind, fastopmBin) {
  if (kind === 'fastopm') return { command: fastopmBin, args: [], label: 'fastopm' };
  return { command: process.execPath, args: [verdaccioBin().file], label: 'verdaccio' };
}

async function startServer(kind, opts, dir, { offline, port }) {
  mkdir(dir);
  const configPath = path.join(dir, 'config.yaml');
  const offlinePort = offline ? await getFreePort() : null;
  fs.writeFileSync(configPath, serverConfig({ offlinePort }));
  const env = kind === 'fastopm' ? childEnv({ RUST_LOG: process.env.E2E_FASTOPM_RUST_LOG ?? FASTOPM_LOG_FILTER }) : childEnv();
  const logFile = path.join(dir, offline ? 'server-offline.log' : 'server.log');
  const server = await startRegistry({ launcher: launcher(kind, opts.fastopmBin), configPath, port, env, logFile });
  return { ...server, kind, logFile, registry: `http://127.0.0.1:${port}/`, storage: path.join(dir, 'storage') };
}

/** Requests the server made to its uplink, from the log written since `offset`. */
function uplinkActivity(server, offset) {
  const text = stripAnsi(fs.readFileSync(server.logFile, 'utf8').slice(offset));
  const lines = text.split('\n');
  if (server.kind === 'fastopm') {
    return {
      tarballs: lines.filter((l) => /fetching tarball|off-uplink tarball/.test(l)).length,
      metadata: lines.filter((l) => /uplink response/.test(l) && !/streaming/.test(l)).length,
    };
  }
  return {
    tarballs: lines.filter((l) => /fetching tarball from/.test(l)).length,
    metadata: lines.filter((l) => /req: 'GET https?:\/\//.test(l) && !/streaming/.test(l)).length,
  };
}

const logSize = (server) => fs.statSync(server.logFile).size;

// ------------------------------------------------------------------ rounds

async function install(client, { roundDir, registry, lockText, logFile }) {
  rm(roundDir);
  const home = path.join(roundDir, 'home');
  const project = path.join(roundDir, 'project');
  mkdir(home);
  mkdir(project);
  fs.writeFileSync(path.join(home, '.npmrc'), `registry=${registry}\n`);
  fs.writeFileSync(path.join(home, '.npmrc-global'), '');
  fs.copyFileSync(path.join(E2E_DIR, 'project/package.json'), path.join(project, 'package.json'));
  const ctx = {
    registry,
    home,
    project,
    cache: path.join(home, 'client-cache'),
    store: path.join(home, 'client-store'),
    frozen: lockText != null,
  };
  if (lockText != null) fs.writeFileSync(path.join(project, client.lockfile), client.adaptLock(lockText, registry));
  client.setup?.(ctx);
  const [cmd, args] = client.command(ctx);
  const { code, ms } = await run(cmd, args, { cwd: project, env: clientEnv(home, client.env(ctx)), logFile });
  const lockPath = path.join(project, client.lockfile);
  const lock = fs.existsSync(lockPath) ? fs.readFileSync(lockPath, 'utf8') : null;
  const problems = code === 0 ? checkTree(project) : [`${cmd} exited with code ${code}`];
  return { ok: problems.length === 0, ms, lock, problems };
}

function compareIntegrity(client, serverLock, npmjsLock) {
  const a = client.parseLock(serverLock);
  const b = client.parseLock(npmjsLock);
  if (!a || !b) return { comparable: false };
  let common = 0;
  const mismatches = [];
  for (const [spec, integrity] of a) {
    if (!b.has(spec)) continue;
    common++;
    if (b.get(spec) !== integrity) mismatches.push(`${spec}: server ${integrity} vs npmjs ${b.get(spec)}`);
  }
  return { comparable: true, packages: a.size, common, mismatches };
}

// ------------------------------------------------------------------ main

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.servers.includes('fastopm') && !fs.existsSync(opts.fastopmBin)) {
    throw new Error(`fastopm binary not found: ${opts.fastopmBin} (cargo build --release -p fastopm)`);
  }
  verdaccioBin();
  const work = opts.work ?? fs.mkdtempSync(path.join(os.tmpdir(), 'fastopm-e2e-'));
  mkdir(work);
  const { clients, skipped } = detectClients(opts.clients);
  console.log(`work dir: ${work}`);
  console.log(`servers : ${opts.servers.join(', ')} (verdaccio ${verdaccioBin().version})`);
  console.log(`clients : ${clients.map((c) => c.label ?? `${c.name} ${c.version}`).join(', ') || '(none)'}`);
  for (const s of skipped) console.log(`skipped : ${s}`);
  if (!clients.length) throw new Error('no client available');

  const results = [];
  const record = (r) => {
    results.push(r);
    const mark = r.ok ? '✔' : '✘';
    const up = r.uplink ? `  uplink: ${r.uplink.tarballs} tarballs, ${r.uplink.metadata} metadata` : '';
    console.log(`${mark} ${r.client.padEnd(5)} ${r.server.padEnd(9)} ${r.round.padEnd(15)} ${fmtMs(r.ms).padStart(7)}${up}`);
    for (const n of r.notes ?? []) console.log(`      ${n}`);
    for (const p of r.problems ?? []) console.log(`      ! ${p}`);
    if (!r.ok && r.logFile) console.log(tail(r.logFile).replace(/^/gm, '      | '));
  };

  for (const client of clients) {
    const cdir = path.join(work, client.name);
    const locks = path.join(cdir, 'locks');
    mkdir(locks);

    // direct: reference install from npmjs, gives the npmjs lockfile
    const directDir = path.join(cdir, 'direct');
    const directLog = path.join(cdir, 'direct.log');
    const direct = await install(client, { roundDir: directDir, registry: NPMJS, lockText: null, logFile: directLog });
    record({ client: client.name, server: 'npmjs', round: 'direct', ok: direct.ok, ms: direct.ms, problems: direct.problems, logFile: directLog });
    if (!opts.keep) rm(directDir);
    if (!direct.ok || !direct.lock) {
      console.log(`  cannot install ${client.name} from npmjs directly; skipping ${client.name}`);
      continue;
    }
    fs.writeFileSync(path.join(locks, `npmjs.${client.lockfile}`), direct.lock);

    const serverLocks = {};
    for (const kind of opts.servers) {
      const sdir = path.join(cdir, kind);
      const port = await getFreePort();
      const round = async (name, server, lockText, check) => {
        const logFile = path.join(sdir, `${name}.log`);
        const offset = logSize(server);
        const r = await install(client, { roundDir: path.join(sdir, 'rounds', name), registry: server.registry, lockText, logFile });
        await new Promise((res) => setTimeout(res, 300)); // let the access log flush
        const uplink = uplinkActivity(server, offset);
        const res = { client: client.name, server: kind, round: name, ok: r.ok, ms: r.ms, uplink, problems: [...r.problems], notes: [], logFile };
        check?.(res, r);
        res.ok = res.problems.length === 0;
        record(res);
        if (!opts.keep) rm(path.join(sdir, 'rounds', name));
        return r;
      };

      let server = await startServer(kind, opts, sdir, { offline: false, port });
      try {
        const cold = await round('cold', server, null);
        if (!cold.ok || !cold.lock) continue;
        serverLocks[kind] = cold.lock;
        fs.writeFileSync(path.join(locks, `${kind}.${client.lockfile}`), cold.lock);
        await round('warm', server, cold.lock, (res) => {
          if (res.uplink.tarballs) res.problems.push(`${res.uplink.tarballs} tarball(s) fetched from the uplink with a warm cache`);
        });
        await round('npmjs-lock', server, direct.lock, (res) => {
          const cmp = compareIntegrity(client, cold.lock, direct.lock);
          if (!cmp.comparable) res.notes.push('integrity comparison not applicable for this lockfile format');
          else {
            res.notes.push(`integrity: ${cmp.common} of ${cmp.packages} name@version in both lockfiles, ${cmp.mismatches.length} differ`);
            res.problems.push(...cmp.mismatches.slice(0, 10));
          }
        });
      } finally {
        await server.stop();
      }

      server = await startServer(kind, opts, sdir, { offline: true, port });
      try {
        const offlineCheck = (res) => {
          if (res.uplink.tarballs) res.problems.push(`${res.uplink.tarballs} tarball fetch(es) attempted while offline`);
        };
        await round('offline-lock', server, serverLocks[kind], offlineCheck);
        await round('offline-nolock', server, null, offlineCheck);
      } finally {
        await server.stop();
      }
    }

    // cross: each server reads the other one's storage (copied), offline, with its lockfile
    if (opts.servers.includes('fastopm') && opts.servers.includes('verdaccio')) {
      for (const [reader, writer] of [
        ['verdaccio', 'fastopm'],
        ['fastopm', 'verdaccio'],
      ]) {
        if (!serverLocks[writer]) continue;
        const sdir = path.join(cdir, `${reader}-on-${writer}-storage`);
        mkdir(sdir);
        fs.cpSync(path.join(cdir, writer, 'storage'), path.join(sdir, 'storage'), { recursive: true });
        const server = await startServer(reader, opts, sdir, { offline: true, port: await getFreePort() });
        try {
          const logFile = path.join(sdir, 'cross.log');
          const offset = logSize(server);
          const r = await install(client, {
            roundDir: path.join(sdir, 'round'),
            registry: server.registry,
            lockText: serverLocks[writer],
            logFile,
          });
          await new Promise((res) => setTimeout(res, 300));
          const uplink = uplinkActivity(server, offset);
          const problems = [...r.problems];
          if (uplink.tarballs) problems.push(`${uplink.tarballs} tarball fetch(es) attempted while offline`);
          record({
            client: client.name,
            server: reader,
            round: `cross(${writer})`,
            ok: problems.length === 0,
            ms: r.ms,
            uplink,
            problems,
            logFile,
          });
          if (!opts.keep) rm(path.join(sdir, 'round'));
        } finally {
          await server.stop();
        }
      }
    }
    if (!opts.keep) {
      for (const kind of opts.servers) rm(path.join(cdir, kind, 'storage'));
      for (const d of fs.readdirSync(cdir)) if (d.includes('-storage')) rm(path.join(cdir, d, 'storage'));
    }
  }

  // summary table: client x server, one column per round
  const rounds = ['direct', 'cold', 'warm', 'npmjs-lock', 'offline-lock', 'offline-nolock', 'cross(fastopm)', 'cross(verdaccio)'];
  const rows = [...new Set(results.map((r) => `${r.client}|${r.server}`))];
  console.log('\nSummary (✔/✘ and install time)\n');
  console.log(['client', 'server', ...rounds].map((h, i) => (i < 2 ? h.padEnd(9) : h.padStart(16))).join(' '));
  for (const row of rows) {
    const [client, server] = row.split('|');
    const cells = rounds.map((round) => {
      const r = results.find((x) => x.client === client && x.server === server && x.round === round);
      return (r ? `${r.ok ? '✔' : '✘'} ${fmtMs(r.ms)}` : '').padStart(16);
    });
    console.log([client.padEnd(9), server.padEnd(9), ...cells].join(' '));
  }
  const failed = results.filter((r) => !r.ok);
  console.log(`\n${results.length} rounds, ${failed.length} failed. Logs: ${work}`);
  if (opts.json) {
    fs.writeFileSync(
      opts.json,
      JSON.stringify({ clients: clients.map((c) => ({ name: c.name, version: c.version })), skipped, results }, null, 2)
    );
  }
  process.exitCode = failed.length ? 1 : 0;
}

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    killAll();
    process.exit(130);
  });
}

main().catch((err) => {
  killAll();
  console.error(err.stack ?? String(err));
  process.exit(2);
});
