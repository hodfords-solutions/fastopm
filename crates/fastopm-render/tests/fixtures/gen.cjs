// Regenerates tests/fixtures/expected/*.json from the Verdaccio code the compat harness runs
// (verdaccio@7.1.0 in compat/node_modules). Run: node crates/fastopm-render/tests/fixtures/gen.cjs
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { createRequire } = require('node:module');

const root = path.resolve(__dirname, '../../../..');
const req = createRequire(path.join(root, 'compat/package.json'));
const store = req('@verdaccio/store');
const tarball = req('@verdaccio/tarball');
const url = req('@verdaccio/url');
const semver = req('semver');

const inputDir = path.join(__dirname, 'input');
const outDir = path.join(__dirname, 'expected');
fs.mkdirSync(outDir, { recursive: true });

const stringify = (v) => JSON.stringify(v, undefined, '  ') + '\n';
const etag = (body) => '"' + crypto.createHash('md5').update(body).digest('hex') + '"';
const write = (name, value) => fs.writeFileSync(path.join(outDir, name), JSON.stringify(value, null, 1) + '\n');

// Request shapes shared by every manifest. `base` inputs mirror fastopm's BaseUrlInput.
const requests = [
  { id: 'no-host', host: null, proto: null, scheme: 'http', prefix: null },
  { id: 'localhost', host: 'localhost:4873', proto: null, scheme: 'http', prefix: null },
  { id: 'proxied', host: 'registry.example.com', proto: 'https,http', scheme: 'http', prefix: 'npm' },
];

function requestOptions(r) {
  const headers = {};
  if (r.host) headers.host = r.host;
  if (r.proto) headers['x-forwarded-proto'] = r.proto;
  return { host: r.host, protocol: r.scheme, headers };
}

// `Storage#getPackage` normalization (dist-tags are already normalized by fastopm-store).
function served(manifest) {
  return Object.assign({}, manifest, { ...store.cleanUpLinksRef(manifest), _attachments: {} });
}

function errorCase(err) {
  if (err.status) return { status: err.status, body: stringify({ error: err.message }) };
  return { status: 500, body: stringify({ error: 'internal server error' }) };
}

// `Storage#getPackageByVersion` after `getPackage`.
function byVersion(manifest, query, options, prefix) {
  const version = store.getVersion(manifest.versions, query);
  if (typeof version !== 'undefined') {
    return tarball.convertDistVersionToLocalTarballsUrl(manifest.name, version, options, prefix);
  }
  const tags = manifest['dist-tags'];
  if (tags != null && Object.hasOwn(tags, query) && tags[query] != null) {
    const tagged = store.getVersion(manifest.versions, tags[query]);
    if (typeof tagged !== 'undefined') {
      return tarball.convertDistVersionToLocalTarballsUrl(manifest.name, tagged, options, prefix);
    }
  }
  const err = new Error(`this version doesn't exist: ${query}`);
  err.status = 404;
  throw err;
}

function okCase(value) {
  const body = typeof value === 'string' ? value : stringify(value);
  return { status: 200, body, etag: etag(body) };
}

const versionQueries = {
  'odd.json': ['1.0.0', 'v1.0.0', '=1.0.0', ' 1.0.0 ', '1.0.0+anything', 'latest', 'next', 'beta',
    '2.0.0-rc.1', '1.0.1-beta.1', '3.0.0', '4.0.0', '9.9.9', 'nope', '1.0', 'constructor'],
  'invalid-key.json': ['1.0.0', 'v1.0.0', 'garbage', '7', 'latest', 'num', 'nul', 'missing', '2.0.0',
    'nope'],
  'precision.json': ['1.0.0', 'latest'],
};

for (const file of fs.readdirSync(inputDir).filter((f) => f.endsWith('.json')).sort()) {
  const text = fs.readFileSync(path.join(inputDir, file), 'utf8');
  const cases = [];
  for (const r of requests) {
    const options = requestOptions(r);
    const full = tarball.convertDistRemoteToLocalTarballUrls(served(JSON.parse(text)), options, r.prefix);
    cases.push({ request: r.id, kind: 'full', ...okCase(full) });
    const abbreviated = store.Storage.prototype.convertAbbreviatedManifest(full);
    cases.push({ request: r.id, kind: 'abbreviated', ...okCase(abbreviated) });
    for (const query of versionQueries[file] ?? ['latest', '0.0.0-missing']) {
      let result;
      try {
        result = okCase(byVersion(served(JSON.parse(text)), query, options, r.prefix));
      } catch (err) {
        result = errorCase(err);
      }
      cases.push({ request: r.id, kind: 'version', query, ...result });
    }
  }
  write(file, { requests, cases });
}

// Number#toString on doubles drawn over the whole bit space, plus edge values.
{
  const buf = new DataView(new ArrayBuffer(8));
  const values = [];
  const push = (x) => {
    buf.setFloat64(0, x);
    values.push([buf.getBigUint64(0).toString(16), JSON.stringify(x)]);
  };
  let seed = 0x9e3779b97f4a7c15n;
  const next = () => {
    seed ^= (seed << 13n) & 0xffffffffffffffffn;
    seed ^= seed >> 7n;
    seed ^= (seed << 17n) & 0xffffffffffffffffn;
    return seed;
  };
  for (let i = 0; i < 5000; i++) {
    buf.setBigUint64(0, next());
    const x = buf.getFloat64(0);
    if (Number.isFinite(x)) push(x);
  }
  for (let e = -330; e <= 310; e++) {
    push(Number(`1e${e}`));
    push(Number(`1.5e${e}`));
    push(Number(`123456789e${e}`));
  }
  for (let i = 0; i < 2000; i++) push(Math.round((Number(next() % 2000000n) - 1000000) * 10 ** (i % 25)));
  [0, -0, 0.1, 0.2 + 0.1, 2 ** 53, 2 ** 53 + 2, 2 ** 64, 2 ** 70, Number.MAX_VALUE, Number.MIN_VALUE,
    Number.EPSILON, 1e21, 1e-7, 999999999999999900000, 1e-6, 123e-20].forEach(push);
  write('numbers.json', values);
}

// QuoteJSONString for every BMP code point that is not a surrogate, sampled.
{
  const strings = [];
  for (let c = 0; c < 0x800; c++) strings.push(String.fromCharCode(c));
  for (let c = 0x800; c < 0x10000; c += 97) if (c < 0xd800 || c > 0xdfff) strings.push(String.fromCharCode(c));
  strings.push('😀', 'a"b\\c', '  ', '');
  write('strings.json', strings.map((s) => [s, JSON.stringify(s)]));
}

// semver.parse(x, true) and compare.
{
  const inputs = ['1.2.3', 'v1.2.3', '=1.2.3', ' =v 1.2.3 ', 'v=1.2.3', ' 1.2.3', '﻿1.2.3',
    '1.2.3 ', '01.002.0003', '1.2.3beta', '1.2.3-beta', '1.2.3--', '1.2.3-', '1.2.3+', '1.2.3+b',
    '1.2.3-a.b.c+b.1', '1.2.3-0a', '1.2.3-a..b', '1.2.3-a.', '1.2.34.5', '1.2.345.6.7', '1.2.3.4',
    '1.2.34-5', '1.2.34+5', '1.2', '1', 'latest', '', ' ', '1.2.3 x', '1.2.3x y', 'V1.2.3', '1.2.3-ÿ',
    '9007199254740991.0.0', '9007199254740992.0.0', '1.2.3-9007199254740990', '1.2.3-9007199254740991',
    '1.2.3-99999999999999999999', '1.2.3-01', '1.2.3-1', '1.2.3-alpha.1', '1.2.3-alpha.beta',
    '1.2.3-alpha.10', '1.2.3-alpha.2', '1.2.3-ALPHA', '1.2.3-a-b', '1.2.3-+', '1.0.0-rc.1+build.5',
    '2.0.0-rc.1', '1.2.3-' + 'a'.repeat(249), ' '.repeat(250) + '1.2.3', '1.2.3-' + 'a'.repeat(251),
    '1.2.3-99999999999999999999.a', '1.2.3-99999999999999999998.b', '1.2.3-1.a', '1.2.3-1.b'];
  const parsed = inputs.map((s) => [s, semver.parse(s, true)?.version ?? null]);
  const valid = inputs.filter((s) => semver.parse(s, true));
  const compares = [];
  for (const a of valid) for (const b of valid) compares.push([a, b, semver.parse(a, true).compare(semver.parse(b, true))]);
  write('semver.json', { parsed, compares });
}

// isHost and getPublicUrl (with the `Host` check of getLocalRegistryTarballUri).
{
  const hosts = ['localhost', 'localhost:4873', 'localhost:', 'LOCALHOST:80', '127.0.0.1', '127.0.0.1:443',
    '01.2.3.4', '1.2.3.256', '0x7f.1', '[::1]', '[::1]:4873', '[fe80::1%eth0]', '[::1', 'a..b',
    'example.com.', 'foo_bar.com', '-a.com', 'a-.com', 'xn--bcher-kva.example', 'bücher.example',
    'ｆｕｌｌ.example', 'user:pw@host', 'user@host', ':@host', '@host', 'a:b:c@host', 'host/path',
    'host/path?q#f', 'host?x', 'host#x', 'example.com:abc', 'example.com:0', 'example.com:65535',
    'example.com:65536', 'example.com:0080', 'a b', '<a>', 'mailto:x', '//host', 'http://host',
    'javascript:alert(1)@host', 'a%41:b@host', '1.2.3.4.5', '123', 'host.123', 'host.a1', '*.host',
    'a'.repeat(63) + '.com', 'a'.repeat(64) + '.com', 'h\u0085', 'h x', 'Ab.Cd'];
  write('hosts.json', hosts.map((h) => [h, url.isHost(h)]));

  const prefixes = [null, '', 'npm', '/npm', 'npm/', '/a/b/', '//evil/', 'http://x/', '../up/', 'sp ace/'];
  const cases = [];
  const publicUrls = [null, 'https://public.example/', 'https://public.example/base', 'http://p.example:8080/a/b/?q=1#frag'];
  const reqs = [
    { host: null, proto: null, scheme: 'http' },
    { host: '', proto: null, scheme: 'http' },
    { host: 'localhost:4873', proto: null, scheme: 'http' },
    { host: 'LocalHost:80', proto: null, scheme: 'http' },
    { host: 'r.example:443', proto: 'https', scheme: 'http' },
    { host: 'r.example', proto: 'https,http', scheme: 'http' },
    { host: 'r.example', proto: ',https', scheme: 'https' },
    { host: 'r.example', proto: '', scheme: 'https' },
    { host: 'r.example', proto: 'ftp', scheme: 'https' },
    { host: '[::1]:4873', proto: null, scheme: 'http' },
    { host: 'user:pw@r.example', proto: null, scheme: 'http' },
    { host: 'r.example/sub', proto: null, scheme: 'http' },
    { host: 'example.com:abc', proto: null, scheme: 'http' },
    { host: 'bad_host', proto: null, scheme: 'http' },
    { host: 'bücher.example', proto: null, scheme: 'http' },
  ];
  for (const publicUrl of publicUrls) {
    if (publicUrl) process.env.VERDACCIO_PUBLIC_URL = publicUrl;
    else delete process.env.VERDACCIO_PUBLIC_URL;
    for (const r of reqs) {
      for (const prefix of prefixes) {
        let result = null;
        if (r.host) {
          const headers = { host: r.host };
          if (r.proto !== null) headers['x-forwarded-proto'] = r.proto;
          try {
            result = url.getPublicUrl(prefix ?? '', { host: r.host, protocol: r.scheme, headers });
          } catch {
            result = 'ERROR';
          }
        }
        cases.push({ ...r, prefix, publicUrl, result });
      }
    }
  }
  delete process.env.VERDACCIO_PUBLIC_URL;
  write('public-url.json', cases);
}
