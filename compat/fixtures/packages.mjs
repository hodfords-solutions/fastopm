// Package fixtures served by the mock uplinks.
//
// A definition describes a package the way its author would; `buildPackument()`
// turns it into an npmjs-like packument for a given uplink origin (tarball URLs
// point at the mock that serves it), and `buildTarballs()` returns the bytes.
// Everything is deterministic: same definition -> same bytes, same shasum.

import { buildTarball } from '../lib/tarball.mjs';
import { sha1, sha512b64 } from '../lib/util.mjs';

const NPM_USER = { name: 'fixture-bot', email: 'bot@example.com' };

function v(version, published, extra = {}) {
  return { version, published, ...extra };
}

/** @type {Record<string, any>} */
export const PACKAGES = {
  'simple-pkg': {
    name: 'simple-pkg',
    description: 'A plain package with tags, a bin and dependencies',
    readme: '# simple-pkg\n\nFixture package for the fastopm compatibility harness.\n',
    maintainers: [NPM_USER],
    license: 'MIT',
    created: '2020-01-01T00:00:00.000Z',
    versions: [
      v('1.0.0', '2020-01-01T00:00:00.000Z', { dependencies: { 'dep-a': '^1.0.0' } }),
      v('1.1.0', '2020-02-01T00:00:00.000Z', {
        dependencies: { 'dep-a': '^1.2.0' },
        bin: { simple: 'bin/simple.js' },
        engines: { node: '>=14' },
      }),
      v('2.0.0-beta.1', '2020-03-01T00:00:00.000Z', {
        dependencies: { 'dep-a': '^2.0.0' },
        peerDependencies: { react: '>=17' },
        deprecated: 'beta, do not use',
      }),
    ],
    distTags: { latest: '1.1.0', beta: '2.0.0-beta.1' },
  },
  '@scope/scoped-pkg': {
    name: '@scope/scoped-pkg',
    description: 'A scoped package',
    readme: '# @scope/scoped-pkg\n',
    maintainers: [NPM_USER],
    license: 'ISC',
    created: '2021-05-01T00:00:00.000Z',
    versions: [
      v('1.0.0', '2021-05-01T00:00:00.000Z'),
      v('1.0.1', '2021-05-02T00:00:00.000Z', { optionalDependencies: { fsevents: '^2.0.0' } }),
    ],
    distTags: { latest: '1.0.1' },
  },
  'other-pkg': {
    name: 'other-pkg',
    description: 'Only served by the second uplink in multi-uplink scenarios',
    readme: '# other-pkg\n',
    maintainers: [NPM_USER],
    license: 'MIT',
    created: '2022-01-01T00:00:00.000Z',
    versions: [v('0.1.0', '2022-01-01T00:00:00.000Z')],
    distTags: { latest: '0.1.0' },
  },
  'sub-pkg': {
    name: 'sub-pkg',
    description: 'Matched by a packages: rule with a custom storage subfolder',
    readme: '',
    maintainers: [NPM_USER],
    license: 'MIT',
    created: '2022-06-01T00:00:00.000Z',
    versions: [v('3.0.0', '2022-06-01T00:00:00.000Z')],
    distTags: { latest: '3.0.0' },
  },
  'cached-pkg': {
    name: 'cached-pkg',
    description: 'Already present in the pre-seeded storage fixture',
    readme: '# cached-pkg\n',
    maintainers: [NPM_USER],
    license: 'MIT',
    created: '2019-01-01T00:00:00.000Z',
    versions: [v('1.0.0', '2019-01-01T00:00:00.000Z')],
    distTags: { latest: '1.0.0' },
  },
  'odd-pkg': {
    name: 'odd-pkg',
    description: 'JSON shapes where JavaScript and file order differ, versions out of order',
    readme: '',
    maintainers: [NPM_USER],
    license: 'MIT',
    created: '2023-01-01T00:00:00.000Z',
    // listed out of semver order on purpose; integer-like keys at several levels
    versions: [
      v('1.10.0', '2023-03-01T00:00:00.000Z', {
        raw: { directories: { z: 'last', 10: 'ten', 2: 'two', a: 'first' }, 7: 'seven' },
      }),
      v('1.2.0', '2023-02-01T00:00:00.000Z', { raw: { scripts: { 1: 'one', test: 'x', 0: 'zero' } } }),
      v('1.9.0-rc.1', '2023-02-15T00:00:00.000Z'),
    ],
    distTags: { latest: '1.10.0', 2: '1.2.0', next: '1.9.0-rc.1' },
    raw: { 42: 'answer', x_custom: { 3: 'c', b: 'b', 1: 'a' }, 4294967295: 'not an index' },
    // serialized with integer-like keys last, as a non-JS registry may send them
    integerKeysLast: true,
  },
};

export function tarballFilename(name, version) {
  const base = name.startsWith('@') ? name.split('/')[1] : name;
  return `${base}-${version}.tgz`;
}

function tarballFiles(def, ver) {
  const pkgJson = {
    name: def.name,
    version: ver.version,
    description: def.description,
    license: def.license,
    main: 'index.js',
    ...(ver.bin ? { bin: ver.bin } : {}),
    ...(ver.dependencies ? { dependencies: ver.dependencies } : {}),
  };
  const files = {
    'package.json': JSON.stringify(pkgJson, null, 2) + '\n',
    'index.js': `module.exports = ${JSON.stringify(`${def.name}@${ver.version}`)};\n`,
  };
  if (ver.bin) files['bin/simple.js'] = '#!/usr/bin/env node\nconsole.log("hi");\n';
  if (def.readme) files['README.md'] = def.readme;
  return files;
}

const tarballCache = new Map();
/** @returns {Map<string, Buffer>} filename -> bytes */
export function buildTarballs(def) {
  const out = new Map();
  for (const ver of def.versions) {
    const key = `${def.name}@${ver.version}`;
    if (!tarballCache.has(key)) tarballCache.set(key, buildTarball(tarballFiles(def, ver)));
    out.set(tarballFilename(def.name, ver.version), tarballCache.get(key));
  }
  return out;
}

export function tarballPath(name, version) {
  return `/${name}/-/${tarballFilename(name, version)}`;
}

/** npmjs-like packument; `origin` like `http://127.0.0.1:1234` */
export function buildPackument(def, origin) {
  const tarballs = buildTarballs(def);
  const versions = {};
  const time = { created: def.created };
  for (const ver of def.versions) {
    const file = tarballFilename(def.name, ver.version);
    const bytes = tarballs.get(file);
    const files = tarballFiles(def, ver);
    versions[ver.version] = {
      name: def.name,
      version: ver.version,
      description: def.description,
      license: def.license,
      main: 'index.js',
      ...(ver.bin ? { bin: ver.bin } : {}),
      ...(ver.dependencies ? { dependencies: ver.dependencies } : {}),
      ...(ver.peerDependencies ? { peerDependencies: ver.peerDependencies } : {}),
      ...(ver.optionalDependencies ? { optionalDependencies: ver.optionalDependencies } : {}),
      ...(ver.engines ? { engines: ver.engines } : {}),
      ...(ver.deprecated ? { deprecated: ver.deprecated } : {}),
      _id: `${def.name}@${ver.version}`,
      _nodeVersion: '20.11.0',
      _npmVersion: '10.2.4',
      _npmUser: def.maintainers[0],
      maintainers: def.maintainers,
      dist: {
        shasum: sha1(bytes),
        integrity: `sha512-${sha512b64(bytes)}`,
        tarball: `${origin}/${def.name}/-/${file}`,
        fileCount: Object.keys(files).length,
        unpackedSize: Object.values(files).reduce((n, s) => n + Buffer.byteLength(s), 0),
      },
      directories: {},
      ...(ver.raw ?? {}),
    };
    time[ver.version] = ver.published;
  }
  const modified = def.modified ?? def.versions.map((x) => x.published).sort().at(-1);
  return {
    _id: def.name,
    _rev: `${def.versions.length}-0123456789abcdef0123456789abcdef`,
    name: def.name,
    description: def.description,
    'dist-tags': { ...def.distTags },
    versions,
    time: { modified, ...time },
    maintainers: def.maintainers,
    readme: def.readme,
    readmeFilename: def.readme ? 'README.md' : '',
    license: def.license,
    ...(def.raw ?? {}),
  };
}

/** Returns a modified copy of a definition with one more version (and `latest` moved to it). */
export function withNewVersion(def, version, published, extra = {}) {
  return {
    ...def,
    versions: [...def.versions, v(version, published, extra)],
    distTags: { ...def.distTags, latest: version },
  };
}

/** `JSON.stringify` with integer-like (array index) keys after the other keys, in descending order. */
function stringifyIntegerKeysLast(value) {
  if (Array.isArray(value)) return `[${value.map(stringifyIntegerKeysLast).join(',')}]`;
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  const keys = Object.keys(value);
  const isIndex = (k) => /^(0|[1-9][0-9]*)$/.test(k) && Number(k) < 2 ** 32 - 1;
  const ordered = [...keys.filter((k) => !isIndex(k)), ...keys.filter(isIndex).reverse()];
  return `{${ordered.map((k) => `${JSON.stringify(k)}:${stringifyIntegerKeysLast(value[k])}`).join(',')}}`;
}

/** Packument body bytes as the mock uplink sends them. */
export function serializePackument(def, origin) {
  const packument = buildPackument(def, origin);
  return def.integerKeysLast ? stringifyIntegerKeysLast(packument) : JSON.stringify(packument);
}
