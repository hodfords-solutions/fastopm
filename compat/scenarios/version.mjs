import { ABBREVIATED, get, NPM_INSTALL_ACCEPT } from './helpers.mjs';

export default [
  {
    name: 'version/existing',
    description: 'GET /:package/:version for existing versions, plain and scoped (all URL forms)',
    steps: [
      get('/simple-pkg/1.0.0'),
      get('/simple-pkg/2.0.0-beta.1'),
      get('/@scope/scoped-pkg/1.0.1'),
      get('/@scope%2Fscoped-pkg/1.0.0'),
      get('/%40scope%2Fscoped-pkg/1.0.1'),
      get('/%40scope/scoped-pkg/1.0.0'),
      get('/simple-pkg/1.1.0', { accept: NPM_INSTALL_ACCEPT }),
    ],
  },
  {
    name: 'version/dist-tag',
    description: 'GET /:package/:tag resolves dist-tags (latest, beta) and semver-ish inputs',
    steps: [
      get('/simple-pkg/latest'),
      get('/simple-pkg/beta'),
      get('/@scope/scoped-pkg/latest'),
      get('/simple-pkg/latest', { accept: ABBREVIATED }),
      get('/simple-pkg/v1.0.0'),
      get('/simple-pkg/%5E1.0.0'),
      get('/simple-pkg/1.x'),
    ],
  },
  {
    name: 'version/semver-loose-inputs',
    description:
      'version queries node-semver parses loosely or rejects (fastopm has its own semver, not a crate)',
    steps: [
      get('/simple-pkg/%3D1.0.0'),
      get('/simple-pkg/v%3D1.0.0'),
      get('/simple-pkg/%201.0.0%20'),
      get('/simple-pkg/V1.0.0'),
      get('/simple-pkg/01.0.0'),
      get('/simple-pkg/1.0.0%2Bbuild.7'),
      get('/simple-pkg/2.0.0beta.1'),
      get('/simple-pkg/2.0.0-beta.01'),
      get('/simple-pkg/1.0'),
      get('/simple-pkg/1.0.0.0'),
      get('/simple-pkg/9007199254740992.0.0'),
      get('/simple-pkg/1.0.0-'),
      get('/odd-pkg/1.10.0'),
      get('/odd-pkg/2'),
      get('/odd-pkg/next'),
      get('/odd-pkg/%3D1.9.0-rc.1'),
    ],
  },
  {
    name: 'version/missing',
    description: 'unknown version, unknown tag, unknown package with version',
    steps: [
      get('/simple-pkg/9.9.9'),
      get('/simple-pkg/no-such-tag'),
      get('/@scope/scoped-pkg/0.0.1'),
      get('/does-not-exist/1.0.0'),
      get('/@scope/does-not-exist/latest'),
    ],
  },
];
