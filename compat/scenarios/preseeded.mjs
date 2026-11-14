import { seedVerdaccioStorage } from '../fixtures/storage.mjs';
import { act, get, NPM_INSTALL_ACCEPT } from './helpers.mjs';

// Storage created beforehand in Verdaccio's layout (fixtures/storage.mjs):
// cached-pkg (from uplink, stale `fetched`), local-pkg and @scope/local-scoped (published locally).
export default [
  {
    name: 'preseeded/read-back',
    description: 'existing Verdaccio storage: cached package revalidated (304), local packages served as-is',
    seed: seedVerdaccioStorage,
    steps: [
      get('/cached-pkg'),
      get('/cached-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/cached-pkg/-/cached-pkg-1.0.0.tgz'),
      get('/local-pkg'),
      get('/local-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/local-pkg/latest'),
      get('/local-pkg/1.0.0'),
      get('/local-pkg/-/local-pkg-1.0.0.tgz'),
      get('/local-pkg/-/local-pkg-1.1.0.tgz'),
      get('/local-pkg/-/local-pkg-9.9.9.tgz'),
      get('/@scope/local-scoped'),
      get('/%40scope%2Flocal-scoped/2.0.0'),
      get('/@scope/local-scoped/-/local-scoped-2.0.0.tgz'),
    ],
  },
  {
    name: 'preseeded/uplink-down',
    description: 'existing storage while the uplink refuses connections',
    seed: seedVerdaccioStorage,
    setup: (s) => s.uplink().setMode('down'),
    steps: [
      get('/cached-pkg'),
      get('/cached-pkg/latest'),
      get('/cached-pkg/-/cached-pkg-1.0.0.tgz'),
      get('/@scope/local-scoped'),
      get('/local-pkg/1.1.0'),
    ],
  },
  {
    name: 'preseeded/upstream-changed',
    description: 'existing storage, uplink has a new packument for cached-pkg (etag no longer matches)',
    seed: seedVerdaccioStorage,
    setup: (s) =>
      s.uplink().setPackage('cached-pkg', (d) => ({
        ...d,
        versions: [...d.versions, { version: '1.0.1', published: '2019-06-01T00:00:00.000Z' }],
        distTags: { latest: '1.0.1' },
      })),
    steps: [
      get('/cached-pkg'),
      get('/cached-pkg/-/cached-pkg-1.0.1.tgz'),
      act('uplink down', (s) => s.uplink().setMode('down')),
      get('/cached-pkg/-/cached-pkg-1.0.0.tgz'),
    ],
  },
];
