import { withNewVersion } from '../fixtures/packages.mjs';
import { act, get, NPM_INSTALL_ACCEPT, wait } from './helpers.mjs';

const MAXAGE = { upstream: { config: { maxage: '2s' } } };

export default [
  {
    name: 'maxage/default-2m',
    description: 'default maxage (2m): repeated metadata requests do not reach the uplink',
    steps: [get('/simple-pkg'), get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }), wait(1000), get('/simple-pkg/1.0.0'), get('/simple-pkg')],
  },
  {
    name: 'maxage/expired-not-modified',
    description: 'maxage 2s: fresh hit stays local; after expiry the uplink gets If-None-Match and answers 304',
    uplinks: MAXAGE,
    steps: [
      get('/simple-pkg'),
      get('/simple-pkg'),
      wait(2500),
      get('/simple-pkg'),
      get('/simple-pkg'),
      wait(2500),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }),
    ],
  },
  {
    name: 'maxage/expired-content-changed',
    description: 'after expiry the uplink has a new version and a moved latest tag: merged into the cache',
    uplinks: MAXAGE,
    steps: [
      get('/simple-pkg'),
      get('/simple-pkg/-/simple-pkg-1.0.0.tgz'),
      act('uplink publishes 1.2.0 (latest)', (s) =>
        s.uplink().setPackage('simple-pkg', (d) =>
          withNewVersion(d, '1.2.0', '2020-04-01T00:00:00.000Z', { dependencies: { 'dep-a': '^1.3.0' } })
        )
      ),
      get('/simple-pkg'),
      wait(2500),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/simple-pkg/latest'),
      get('/simple-pkg/-/simple-pkg-1.2.0.tgz'),
    ],
  },
  {
    name: 'maxage/upstream-removes-version',
    description: 'after expiry the uplink drops a version and a tag: Verdaccio keeps cached versions',
    uplinks: MAXAGE,
    steps: [
      get('/simple-pkg'),
      act('uplink unpublishes 2.0.0-beta.1 and the beta tag', (s) =>
        s.uplink().setPackage('simple-pkg', (d) => ({
          ...d,
          versions: d.versions.filter((v) => v.version !== '2.0.0-beta.1'),
          distTags: { latest: '1.1.0' },
        }))
      ),
      wait(2500),
      get('/simple-pkg'),
      get('/simple-pkg/beta'),
      get('/simple-pkg/2.0.0-beta.1'),
    ],
  },
  {
    name: 'maxage/zero',
    description: 'maxage 0: every metadata request revalidates with the uplink',
    uplinks: { upstream: { config: { maxage: '0' } } },
    steps: [get('/simple-pkg'), get('/simple-pkg'), get('/simple-pkg/1.0.0')],
  },
];
