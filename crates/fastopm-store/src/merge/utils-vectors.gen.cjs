const path = require('path');
const base = path.resolve(process.argv[2], 'node_modules/@verdaccio/store/build/lib');
const u = require(path.join(base, 'storage-utils.js'));
const out = {};
const clone = (x) => JSON.parse(JSON.stringify(x));

// normalizeDistTags
const ndt = [
  {versions: {'1.0.0': {}, '2.0.0-rc.1': {}, '1.5.0': {}, 'bad': {}}, 'dist-tags': {}},
  {versions: {'1.0.0': {}}, 'dist-tags': {latest: 'nope', beta: ['1.0.0', '2.0.0', 'x'], empty: [], bad: ['x'], num: 5}},
  {versions: {'1.0.0': {}, '3.0.0': {}}, 'dist-tags': {latest: '', next: 'v2.0.0'}},
];
out.normalizeDistTags = ndt.map((m) => u.normalizeDistTags(clone(m))['dist-tags']);

// getLatestReadme
const readmes = [
  {readme: '  top \n', versions: {}, 'dist-tags': {}},
  {versions: {'1.0.0': {readme: ' from latest '}}, 'dist-tags': {latest: '1.0.0'}},
  {readme: '', versions: {'1.0.0': {readme: ''}, '2.0.0-b': {readme: 'beta readme\t'}}, 'dist-tags': {latest: '1.0.0', beta: '2.0.0-b'}},
  {versions: {}, 'dist-tags': {}},
];
out.getLatestReadme = readmes.map((m) => u.getLatestReadme(clone(m)));

// mergeVersions
const cache = {name: 'a', versions: {'1.0.0': {version: '1.0.0'}}, time: {}, 'dist-tags': {latest: '1.0.0', beta: '1.1.0-b'}, readme: 'old', _uplinks: {}};
const remote = {name: 'a', readme: 'new', versions: {'1.0.0': {version: '1.0.0', deprecated: 'dep'}, '2.0.0': {version: '2.0.0'}}, time: {'2.0.0': 't2'}, 'dist-tags': {latest: '2.0.0', beta: '1.0.0', next: '3.0.0'}};
out.mergeVersions = u.mergeVersions(clone(cache), clone(remote));
try { u.mergeVersions(clone(cache), {versions: {}, 'dist-tags': {latest: 'not-semver'}}); out.mergeThrows = false } catch (e) { out.mergeThrows = true }
out.mergeKeepsHigherLocal = u.mergeVersions({versions: {}, time: {}, 'dist-tags': {latest: '3.0.0'}, readme: 'keep'}, {versions: {}, 'dist-tags': {latest: '2.0.0'}, readme: 'remote'});

// scopeUpstreamMaintainers
out.scope = u.scopeUpstreamMaintainers(['bob', {email: 'e', name: 'alice'}, {email: 'x'}, '$uplink:other:carol'], 'npmjs');
out.scopeEmpty = u.scopeUpstreamMaintainers(['bob'], '');

// lookupDistFile
const lm = {name: '@s/pkg', _distfiles: {'pkg-1.0.0.tgz': {url: 'https://r/@s/pkg/-/pkg-1.0.0.tgz', sha: 'a', registry: 'npmjs'}},
  versions: {'2.0.0': {dist: {tarball: 'https://cdn/x/pkg-2.0.0.tgz?sig=1', shasum: 'b'}}, '3.0.0': {dist: {tarball: 'https://cdn/odd-name'}}}};
out.lookup = ['pkg-1.0.0.tgz', 'pkg-2.0.0.tgz', 'odd-name', 'pkg-9.9.9.tgz', 'name'].map((f) => u.lookupDistFile(clone(lm), f));
console.log(JSON.stringify(out, null, 1));
