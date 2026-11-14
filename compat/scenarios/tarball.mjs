import { tarballPath } from '../fixtures/packages.mjs';
import { act, get } from './helpers.mjs';

const T100 = tarballPath('simple-pkg', '1.0.0');
const T110 = tarballPath('simple-pkg', '1.1.0');

export default [
  {
    name: 'tarball/cold-then-cached',
    description:
      'tarball without prior metadata request (Verdaccio syncs the packument first), then again from disk',
    steps: [get(T100), get(T100), get(T100, { 'accept-encoding': 'gzip' })],
  },
  {
    name: 'tarball/after-metadata',
    description: 'metadata first (fills _distfiles), then two tarballs, each requested twice',
    steps: [get('/simple-pkg'), get(T100), get(T110), get(T100), get(T110)],
  },
  {
    name: 'tarball/scoped',
    description: 'scoped tarball via /@s/p/-/f and /@s%2fp/-/f and /%40s%2Fp/-/f',
    steps: [
      get('/@scope/scoped-pkg'),
      get('/@scope/scoped-pkg/-/scoped-pkg-1.0.0.tgz'),
      get('/@scope%2fscoped-pkg/-/scoped-pkg-1.0.0.tgz'),
      get('/%40scope%2Fscoped-pkg/-/scoped-pkg-1.0.1.tgz'),
      get('/@scope/scoped-pkg/-/scoped-pkg-1.0.1.tgz'),
    ],
  },
  {
    name: 'tarball/missing',
    description: 'tarball of unknown version, of unknown package, odd file names',
    steps: [
      get('/simple-pkg/-/simple-pkg-9.9.9.tgz'),
      get('/simple-pkg'),
      get('/simple-pkg/-/simple-pkg-9.9.9.tgz'),
      get('/simple-pkg/-/other-pkg-0.1.0.tgz'),
      get('/does-not-exist/-/does-not-exist-1.0.0.tgz'),
      get('/simple-pkg/-/simple-pkg-1.0.0.tar.gz'),
    ],
  },
  {
    name: 'tarball/uplink-errors',
    description: 'packument lists the tarball but the uplink answers 404, then 503, then the request is fine',
    steps: [
      get('/simple-pkg'),
      act('uplink: tarball 1.0.0 -> 404', (s) => s.uplink().override(T100, { type: 'status', status: 404 })),
      get(T100),
      act('uplink: tarball 1.0.0 -> 503', (s) => s.uplink().override(T100, { type: 'status', status: 503 })),
      get(T100),
      act('uplink: tarball ok', (s) => s.uplink().clearOverrides()),
      get(T100),
    ],
  },
  {
    name: 'tarball/uplink-cache-false',
    description: 'uplink with cache: false streams tarballs without writing them to storage',
    uplinks: { upstream: { config: { cache: false } } },
    steps: [get('/simple-pkg'), get(T100), get(T100), get(T110)],
  },
];
