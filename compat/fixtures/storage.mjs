// Pre-seeded storage in Verdaccio's on-disk layout (what a Verdaccio 7 instance
// leaves behind), written before the registries start:
//
//   <storage>/.verdaccio-db.json                    {"list":[...local packages], "secret": ...}
//   <storage>/cached-pkg/package.json               cached from uplink "upstream" (old `fetched`)
//   <storage>/cached-pkg/cached-pkg-1.0.0.tgz
//   <storage>/local-pkg/package.json                published locally (has `_attachments`, no `_uplinks`)
//   <storage>/local-pkg/local-pkg-1.0.0.tgz
//   <storage>/local-pkg/local-pkg-1.1.0.tgz
//   <storage>/@scope/local-scoped/package.json      published locally, scoped (matched by a proxy rule)
//   <storage>/@scope/local-scoped/local-scoped-2.0.0.tgz
//
// package.json files are compact JSON like Verdaccio writes them.

import fs from 'node:fs';
import path from 'node:path';

import { buildPackument, buildTarballs, PACKAGES, tarballFilename } from './packages.mjs';
import { buildTarball } from '../lib/tarball.mjs';
import { sha1, sha512b64 } from '../lib/util.mjs';

const PUBLISHER = { name: 'alice', email: 'alice@example.com' };

/** Locally published packages (never served by the mock uplinks). */
const LOCAL = {
  'local-pkg': {
    versions: { '1.0.0': '2023-01-01T10:00:00.000Z', '1.1.0': '2023-02-01T10:00:00.000Z' },
    distTags: { latest: '1.1.0' },
    readme: '# local-pkg\n\nPublished straight to Verdaccio.\n',
    rev: '4-8f1d3c9a2b7e6d50',
  },
  '@scope/local-scoped': {
    versions: { '2.0.0': '2023-03-01T10:00:00.000Z' },
    distTags: { latest: '2.0.0' },
    readme: '',
    rev: '2-00aa11bb22cc33dd',
  },
};

function write(file, data) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data);
}

function localTarball(name, version) {
  return buildTarball({
    'package.json': JSON.stringify({ name, version, main: 'index.js' }, null, 2) + '\n',
    'index.js': `module.exports = ${JSON.stringify(`${name}@${version}`)};\n`,
  });
}

function seedLocal(storage, name, def) {
  const dir = path.join(storage, name);
  const versions = {};
  const attachments = {};
  const time = {};
  const times = Object.values(def.versions).sort();
  time.modified = times.at(-1);
  time.created = times[0];
  for (const [version, published] of Object.entries(def.versions)) {
    const file = tarballFilename(name, version);
    const bytes = localTarball(name, version);
    write(path.join(dir, file), bytes);
    versions[version] = {
      name,
      version,
      main: 'index.js',
      _id: `${name}@${version}`,
      _nodeVersion: '20.11.0',
      _npmVersion: '10.2.4',
      _npmUser: PUBLISHER,
      readme: '',
      dist: {
        integrity: `sha512-${sha512b64(bytes)}`,
        shasum: sha1(bytes),
        tarball: `http://localhost:4873/${name}/-/${file}`,
      },
      contributors: [],
      maintainers: [PUBLISHER],
    };
    attachments[file] = { shasum: sha1(bytes), version };
    time[version] = published;
  }
  const manifest = {
    name,
    versions,
    time,
    users: {},
    'dist-tags': def.distTags,
    _uplinks: {},
    _distfiles: {},
    _attachments: attachments,
    _rev: def.rev,
    _id: name,
    readme: def.readme,
    maintainers: [PUBLISHER],
  };
  write(path.join(dir, 'package.json'), JSON.stringify(manifest));
}

/** A package cached from `uplink` the way Verdaccio stores it after one fetch. */
function seedCached(storage, name, mock, uplinkName) {
  const dir = path.join(storage, name);
  const remote = buildPackument(PACKAGES[name], mock.origin);
  const distfiles = {};
  const versions = {};
  for (const [v, ver] of Object.entries(remote.versions)) {
    const file = tarballFilename(name, v);
    versions[v] = { ...ver, readme: '', contributors: [] };
    distfiles[file] = { url: ver.dist.tarball, sha: ver.dist.shasum, registry: uplinkName };
  }
  for (const [file, bytes] of buildTarballs(PACKAGES[name])) write(path.join(dir, file), bytes);
  const manifest = {
    name,
    versions,
    time: remote.time,
    users: {},
    'dist-tags': remote['dist-tags'],
    _uplinks: { [uplinkName]: { etag: mock.packumentEtag(name), fetched: 1577836800000 } },
    _distfiles: distfiles,
    _attachments: {},
    _rev: '3-1a2b3c4d5e6f7a8b',
    _id: name,
    readme: remote.readme,
    maintainers: [{ name: `$uplink:${uplinkName}/fixture-bot`, email: 'bot@example.com' }],
  };
  write(path.join(dir, 'package.json'), JSON.stringify(manifest));
}

/** `side` is the runner's side object (its mocks are already listening). */
export function seedVerdaccioStorage(storage, side) {
  seedCached(storage, 'cached-pkg', side.uplink('upstream'), 'upstream');
  for (const [name, def] of Object.entries(LOCAL)) seedLocal(storage, name, def);
  write(
    path.join(storage, '.verdaccio-db.json'),
    JSON.stringify({ list: Object.keys(LOCAL), secret: 'seeded-secret-32-chars-for-tests' /* Verdaccio requires exactly 32 */ })
  );
}
