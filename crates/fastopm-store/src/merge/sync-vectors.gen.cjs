const path = require('path');
const root = path.resolve(process.argv[2], 'node_modules/@verdaccio/store/build');
const { Storage } = require(path.join(root, 'storage.js'));
const u = require(path.join(root, 'lib/storage-utils.js'));
Date.now = () => 1700000000000;
const logger = { error() {}, warn() {}, info() {}, debug() {}, http() {}, trace() {} };
const clone = (x) => JSON.parse(JSON.stringify(x));

async function run(name, disk, remote, etag, uplinkName, uplinkUrl) {
  const local = disk ? u.normalizePackage(clone(disk)) : null;
  const uplink = { uplinkName, maxage: 0, getRemoteMetadata: async () => [clone(remote), etag] };
  const temp = local ? { ...local } : u.generatePackageTemplate(name);
  const merged = await Storage.prototype.mergeCacheRemoteMetadata.call({ logger }, uplink, temp, {});
  let written = null;
  const self = {
    config: { uplinks: { [uplinkName]: { url: uplinkUrl } } },
    readCreatePackage: async () => (disk ? u.normalizePackage(clone(disk)) : u.normalizePackage(u.generatePackageTemplate(name))),
    writePackage: async (n, j) => { written = JSON.stringify(j); },
    updateUplinkToRemoteProtocol: Storage.prototype.updateUplinkToRemoteProtocol,
  };
  const result = await Storage.prototype.updateVersionsNext.call(self, name, merged, uplinkName);
  return { written, served: JSON.stringify(u.normalizeDistTags(result)) };
}

(async () => {
  const remote = {
    name: 'foo', 'dist-tags': { latest: '1.1.0', next: '2.0.0-rc.1' },
    maintainers: [{ name: 'bob', email: 'b@x' }, 'carol'],
    readme: 'Remote readme\n',
    time: { created: 'c', modified: 'm2', '1.0.0': 't1', '1.1.0': 't11', '2.0.0-rc.1': 't2' },
    versions: {
      '1.0.0': { name: 'foo', version: '1.0.0', readme: 'v readme', dist: { tarball: 'http://registry.example.com/foo/-/foo-1.0.0.tgz', shasum: 's1' } },
      '1.1.0': { name: 'foo', version: '1.1.0', contributors: 'someone', dist: { tarball: 'http://registry.example.com/foo/-/foo-1.1.0.tgz', shasum: 's11' } },
      '2.0.0-rc.1': { name: 'foo', version: '2.0.0-rc.1', contributors: [{ name: 'x' }], dist: { tarball: 'https://cdn.example.net/foo-2.0.0-rc.1.tgz' } },
    },
  };
  const fresh = await run('foo', null, remote, '"etag-1"', 'npmjs', 'https://registry.example.com/');
  const disk = {
    name: 'foo', versions: { '1.0.0': { name: 'foo', version: '1.0.0', readme: '', contributors: [], dist: { tarball: 'https://registry.example.com/foo/-/foo-1.0.0.tgz', shasum: 's1' } } },
    time: { created: 'c', modified: 'm1', '1.0.0': 't1' }, users: {}, 'dist-tags': { latest: '1.0.0' }, maintainers: [],
    _uplinks: { npmjs: { etag: '"old"', fetched: 1600000000000 } },
    _distfiles: { 'foo-1.0.0.tgz': { url: 'https://registry.example.com/foo/-/foo-1.0.0.tgz', sha: 's1', registry: 'npmjs' } },
    _attachments: {}, _rev: '3-0123456789abcdef', readme: 'Old readme',
  };
  const existing = await run('foo', disk, remote, undefined, 'npmjs', 'https://registry.example.com/');
  console.log(JSON.stringify({ fresh, existing }, null, 1));
})();
