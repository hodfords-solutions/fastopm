// Mock npm registry used as the uplink of both servers. Runs as its own process
// (`node lib/uplink.mjs`) so its CPU use does not share an event loop with the load generator.
// Prints `{"port": N}` on stdout once listening.
//
// Packages:
//   ms, express, typescript     real packuments from fixtures/*.json.gz (fetched once from npmjs),
//                               dist.tarball rewritten to this server
//   bench-tarball               synthetic: 1.0.0 -> 1 MiB tarball, 2.0.0 -> 50 MiB tarball
//   bench-cold-<anything>       clone of the express packument under that name (cold-cache runs)
//   bench-tgz-<anything>        one version 1.0.0 whose tarball is the 1 MiB blob (cold tarballs)
//
// Every packument has ETag = "md5(body)"; a matching If-None-Match gets 304. Tarballs are random
// (incompressible) bytes generated at start-up.
//
// Admin (never delayed):
//   GET  /__stats             {total, byPath: {path: count}, notModified}
//   POST /__reset             zero the counters
//   POST /__config?delay=N    delay every registry answer by N ms (default 0)

import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import http from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(here, '..', 'fixtures');
const PLACEHOLDER = '__BENCH_NAME__';
const ORIGIN_PLACEHOLDER = '__BENCH_ORIGIN__';

const md5 = (buf) => createHash('md5').update(buf).digest('hex');
const sha1 = (buf) => createHash('sha1').update(buf).digest('hex');
const sha512 = (buf) => `sha512-${createHash('sha512').update(buf).digest('base64')}`;

const MiB = 1024 * 1024;
const blobs = { small: randomBytes(1 * MiB), large: randomBytes(50 * MiB) };

let origin = '';
let delay = 0;
let counters = { total: 0, byPath: {}, notModified: 0 };

function loadFixture(name) {
  return JSON.parse(gunzipSync(readFileSync(join(FIXTURES, `${name}.json.gz`))).toString('utf8'));
}

/** Rewrite every dist.tarball to `<ORIGIN>/<name>/-/<file>`; name may be the placeholder. */
function retarget(pkg, name) {
  pkg.name = name;
  pkg._id = name;
  for (const [ver, v] of Object.entries(pkg.versions ?? {})) {
    v.name = name;
    v._id = `${name}@${ver}`;
    if (v.dist?.tarball) {
      const file = `${name.split('/').pop()}-${ver}.tgz`;
      v.dist.tarball = `${ORIGIN_PLACEHOLDER}/${name}/-/${file}`;
    }
  }
  return pkg;
}

function versionFor(name, ver, blob) {
  return {
    name,
    version: ver,
    description: 'synthetic package for fastopm benchmarks',
    main: 'index.js',
    license: 'MIT',
    _id: `${name}@${ver}`,
    dist: {
      shasum: sha1(blob),
      integrity: sha512(blob),
      tarball: `${ORIGIN_PLACEHOLDER}/${name}/-/${name}-${ver}.tgz`,
    },
  };
}

function syntheticPackument(name, versions) {
  const time = { created: '2024-01-01T00:00:00.000Z', modified: '2024-01-01T00:00:00.000Z' };
  const out = { _id: name, name, 'dist-tags': {}, versions: {}, time, readme: '# bench\n' };
  for (const [ver, blob] of versions) {
    out.versions[ver] = versionFor(name, ver, blob);
    out.time[ver] = '2024-01-01T00:00:00.000Z';
    out['dist-tags'].latest = ver;
  }
  return out;
}

// Templates are JSON strings with placeholders, so a per-name packument is two replaceAll calls.
const templates = {
  ms: JSON.stringify(retarget(loadFixture('ms'), 'ms')),
  express: JSON.stringify(retarget(loadFixture('express'), 'express')),
  typescript: JSON.stringify(retarget(loadFixture('typescript'), 'typescript')),
  'bench-tarball': JSON.stringify(
    syntheticPackument('bench-tarball', [
      ['1.0.0', blobs.small],
      ['2.0.0', blobs.large],
    ]),
  ),
  cold: JSON.stringify(retarget(loadFixture('express'), PLACEHOLDER)),
  tgz: JSON.stringify(syntheticPackument(PLACEHOLDER, [['1.0.0', blobs.small]])),
};

const bodyCache = new Map();
function packument(name) {
  let entry = bodyCache.get(name);
  if (entry) return entry;
  let text;
  if (Object.hasOwn(templates, name) && name !== 'cold' && name !== 'tgz') text = templates[name];
  else if (name.startsWith('bench-cold-')) text = templates.cold.replaceAll(PLACEHOLDER, name);
  else if (name.startsWith('bench-tgz-')) text = templates.tgz.replaceAll(PLACEHOLDER, name);
  else return null;
  const body = Buffer.from(text.replaceAll(ORIGIN_PLACEHOLDER, origin));
  entry = { body, etag: `"${md5(body)}"` };
  // the big fixtures are reused; cold packages are fetched once per name
  if (!name.startsWith('bench-cold-')) bodyCache.set(name, entry);
  return entry;
}

function tarball(name, file) {
  if (name === 'bench-tarball' && file === 'bench-tarball-2.0.0.tgz') return blobs.large;
  if (name === 'bench-tarball' && file === 'bench-tarball-1.0.0.tgz') return blobs.small;
  if (name.startsWith('bench-tgz-') && file === `${name}-1.0.0.tgz`) return blobs.small;
  // real fixtures: tiny stand-in (never measured)
  if (['ms', 'express', 'typescript'].includes(name) || name.startsWith('bench-cold-'))
    return Buffer.from(`stand-in tarball for ${name}/${file}\n`);
  return null;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function handle(req, res) {
  const url = new URL(req.url, 'http://x');
  if (url.pathname === '/__stats') {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify(counters));
  }
  if (url.pathname === '/__reset') {
    counters = { total: 0, byPath: {}, notModified: 0 };
    res.writeHead(200);
    return res.end('{}');
  }
  if (url.pathname === '/__config') {
    if (url.searchParams.has('delay')) delay = Number(url.searchParams.get('delay'));
    res.writeHead(200);
    return res.end(JSON.stringify({ delay }));
  }

  let path;
  try {
    path = decodeURIComponent(url.pathname);
  } catch {
    path = url.pathname;
  }
  counters.total += 1;
  counters.byPath[path] = (counters.byPath[path] ?? 0) + 1;
  if (delay > 0) await sleep(delay);

  const parts = path.slice(1).split('/');
  // scoped names are not used by the benchmark
  if (parts.length === 1) {
    const p = packument(parts[0]);
    if (!p) {
      res.writeHead(404, { 'content-type': 'application/json' });
      return res.end('{"error":"not found"}');
    }
    if (req.headers['if-none-match'] === p.etag) {
      counters.notModified += 1;
      res.writeHead(304, { etag: p.etag });
      return res.end();
    }
    res.writeHead(200, {
      'content-type': 'application/json',
      'content-length': p.body.length,
      etag: p.etag,
    });
    return res.end(p.body);
  }
  if (parts.length === 3 && parts[1] === '-') {
    const blob = tarball(parts[0], parts[2]);
    if (blob) {
      res.writeHead(200, {
        'content-type': 'application/octet-stream',
        'content-length': blob.length,
      });
      return res.end(blob);
    }
  }
  res.writeHead(404, { 'content-type': 'application/json' });
  res.end('{"error":"not found"}');
}

const server = http.createServer((req, res) => {
  handle(req, res).catch((e) => {
    res.writeHead(500);
    res.end(String(e));
  });
});
server.keepAliveTimeout = 60_000;
server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  const { port } = server.address();
  origin = `http://127.0.0.1:${port}`;
  process.stdout.write(`${JSON.stringify({ port })}\n`);
});
process.on('SIGTERM', () => server.close(() => process.exit(0)));
process.on('disconnect', () => process.exit(0));
