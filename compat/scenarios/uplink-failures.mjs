import { tarballPath } from '../fixtures/packages.mjs';
import { act, get, NPM_INSTALL_ACCEPT, wait } from './helpers.mjs';

const T100 = tarballPath('simple-pkg', '1.0.0');
const T110 = tarballPath('simple-pkg', '1.1.0');
const SHORT = (extra = {}) => ({ upstream: { config: { maxage: '1s', ...extra } } });
const down = act('uplink down (connection refused)', (s) => s.uplink().setMode('down'));
const up = act('uplink up', (s) => s.uplink().setMode('up'));
const status = (code) =>
  act(`uplink answers ${code} to everything`, (s) => s.uplink().setMode({ type: 'status', status: code }));

// Note: Verdaccio retries failed uplink requests (got, `retry` default 2, backoff ~1s+2s),
// and with the default max_fails 2 one fully failed request opens the circuit breaker.
export default [
  {
    name: 'uplink-down/with-cache',
    description: 'cached packument + tarball, uplink goes down, maxage expired: served from cache',
    uplinks: SHORT(),
    steps: [
      get('/simple-pkg'),
      get(T100),
      down,
      wait(1500),
      get('/simple-pkg'),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get('/simple-pkg/1.0.0'),
      get(T100),
      get(T110),
    ],
  },
  {
    name: 'uplink-down/no-cache',
    description: 'uplink down from the start: metadata, version and tarball of an uncached package',
    setup: (s) => s.uplink().setMode('down'),
    steps: [get('/simple-pkg'), get('/simple-pkg/1.0.0'), get(T100), get('/@scope/scoped-pkg')],
  },
  {
    name: 'uplink-503/no-cache',
    description: 'uplink answers 503 for an uncached package (metadata, then tarball)',
    setup: (s) => s.uplink().setMode({ type: 'status', status: 503 }),
    steps: [get('/simple-pkg'), get(T100)],
  },
  {
    name: 'uplink-503/with-cache',
    description: 'uplink starts answering 503 after the package was cached; maxage expired',
    uplinks: SHORT(),
    steps: [get('/simple-pkg'), status(503), wait(1500), get('/simple-pkg'), get('/simple-pkg/latest'), get(T110)],
  },
  {
    name: 'uplink-500-and-other-codes/no-cache',
    description: 'uplink answers 500 / 401 / 403 / 418 (each a fresh package request, breaker disabled)',
    uplinks: { upstream: { config: { max_fails: 100 } } },
    steps: [
      status(500),
      get('/simple-pkg'),
      status(401),
      get('/@scope/scoped-pkg'),
      status(403),
      get('/other-pkg'),
      status(418),
      get('/sub-pkg'),
    ],
  },
  {
    name: 'uplink-404/after-cache',
    description: 'package cached, then the uplink forgets it (404) after maxage: cached copy still served?',
    uplinks: SHORT(),
    steps: [
      get('/simple-pkg'),
      act('uplink: /simple-pkg -> 404', (s) => s.uplink().override('/simple-pkg', { type: 'status', status: 404 })),
      wait(1500),
      get('/simple-pkg'),
      get('/simple-pkg/1.1.0'),
    ],
  },
  {
    name: 'uplink-slow/timeout',
    description: 'uplink slower than `timeout: 1s` for an uncached package, then for a cached one',
    uplinks: SHORT({ timeout: '1s', max_fails: 100 }),
    steps: [
      act('uplink delays every answer 2.5s', (s) => s.uplink().setMode({ type: 'delay', ms: 2500 })),
      get('/@scope/scoped-pkg'),
      act('uplink fast again', (s) => s.uplink().setMode('up')),
      get('/simple-pkg'),
      act('uplink delays every answer 2.5s', (s) => s.uplink().setMode({ type: 'delay', ms: 2500 })),
      wait(1200),
      get('/simple-pkg'),
    ],
  },
  {
    name: 'uplink-slow/under-timeout',
    description: 'uplink answers after 600ms with the default timeout: just slower',
    setup: (s) => s.uplink().setMode({ type: 'delay', ms: 600 }),
    steps: [get('/simple-pkg'), get(T100)],
  },
  {
    name: 'uplink-reset/no-cache',
    description: 'uplink resets the connection without answering',
    setup: (s) => s.uplink().setMode({ type: 'reset' }),
    steps: [get('/simple-pkg')],
  },
  {
    name: 'circuit-breaker',
    description:
      'max_fails 2, fail_timeout 4s: a failed request opens the breaker; requests inside fail_timeout skip the uplink',
    uplinks: { upstream: { config: { maxage: '1s', max_fails: 2, fail_timeout: '4s' } } },
    steps: [
      get('/@scope/scoped-pkg'),
      status(503),
      get('/simple-pkg'),
      get('/simple-pkg'),
      get('/@scope/scoped-pkg'),
      up,
      get('/other-pkg'),
      wait(4500),
      get('/other-pkg'),
      get('/simple-pkg'),
    ],
  },
  {
    name: 'circuit-breaker/max-fails-1',
    description: 'max_fails 1 with connection refused, then recovery after fail_timeout',
    uplinks: { upstream: { config: { max_fails: 1, fail_timeout: '3s' } } },
    steps: [down, get('/simple-pkg'), get('/other-pkg'), up, get('/other-pkg'), wait(3500), get('/other-pkg')],
  },
];
