import { ABBREVIATED, get, NPM_INSTALL_ACCEPT } from './helpers.mjs';

export default [
  {
    name: 'metadata/js-key-order',
    description:
      'uplink JSON with integer-like keys after the others and versions out of semver order: ' +
      'served in JavaScript key order, full, abbreviated and gzip',
    steps: [
      get('/odd-pkg'),
      get('/odd-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/odd-pkg', { 'accept-encoding': 'gzip' }),
      get('/odd-pkg/latest'),
      get('/odd-pkg/-/odd-pkg-1.2.0.tgz'),
    ],
  },
  {
    name: 'metadata/full',
    description: 'GET /:package full packument (first fetch from uplink, then served from cache within maxage)',
    steps: [
      get('/simple-pkg'),
      get('/simple-pkg', { accept: 'application/json' }),
      get('/simple-pkg?write=false'),
    ],
  },
  {
    name: 'metadata/abbreviated',
    description: 'Accept: application/vnd.npm.install-v1+json (plain and npm q-list) as first and later request',
    steps: [
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/simple-pkg', { accept: ABBREVIATED }),
      get('/simple-pkg', { accept: 'application/json; q=1.0, application/vnd.npm.install-v1+json; q=0.5' }),
      get('/simple-pkg'),
    ],
  },
  {
    name: 'metadata/scoped-url-forms',
    description: 'scoped package via /@s/p, /@s%2Fp, /@s%2fp, /%40s%2Fp, /%40s/p, full and abbreviated',
    steps: [
      get('/@scope/scoped-pkg'),
      get('/@scope%2Fscoped-pkg'),
      get('/@scope%2fscoped-pkg'),
      get('/%40scope%2Fscoped-pkg'),
      get('/%40scope/scoped-pkg'),
      get('/@scope/scoped-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/%40scope%2Fscoped-pkg', { accept: NPM_INSTALL_ACCEPT }),
    ],
  },
  {
    name: 'metadata/not-found-upstream',
    description: 'package unknown to the uplink (uplink answers 404), plain and scoped',
    steps: [
      get('/does-not-exist'),
      get('/does-not-exist', { accept: NPM_INSTALL_ACCEPT }),
      get('/@scope/does-not-exist'),
      get('/%40nope%2Fmissing'),
    ],
  },
  {
    name: 'metadata/gzip',
    description: 'Accept-Encoding: gzip on metadata and ping (bodies compared after decoding)',
    steps: [
      get('/simple-pkg', { 'accept-encoding': 'gzip' }),
      get('/simple-pkg', { 'accept-encoding': 'gzip, deflate, br', accept: NPM_INSTALL_ACCEPT }),
      get('/-/ping', { 'accept-encoding': 'gzip' }),
    ],
  },
  {
    name: 'metadata/package-name-edge-cases',
    description: 'upper-case name, dot names, encoded characters',
    steps: [
      get('/Simple-Pkg'),
      get('/simple-pkg%2F..%2Fother-pkg'),
      // Verdaccio 7.1.0 never answers this one (see rules.mjs ALLOWED_DIFFS)
      get('/.hidden', {}, { name: 'GET /.hidden (dotfile segment)', timeout: 3000 }),
      get('/simple%20pkg'),
    ],
  },
];
