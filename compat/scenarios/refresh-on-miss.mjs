// fastopm-only behaviour: `fastopm.refresh_on_miss`. Verdaccio has no such option and ignores
// the `fastopm:` section, so both sides get the same config (this also checks Verdaccio 7.1.0
// starts with it). Differences on the steps where the option acts are listed in
// lib/rules.mjs ALLOWED_DIFFS; `candidateExpect` asserts what fastopm itself must answer there
// (checked with --candidate-bin only). Every other step must still match Verdaccio.

import { tarballPath, withNewVersion } from '../fixtures/packages.mjs';
import { act, get, NPM_INSTALL_ACCEPT, wait } from './helpers.mjs';

// maxage long enough never to expire during a scenario
const LONG_MAXAGE = { upstream: { config: { maxage: '1h' } } };
const refreshOnMiss = (minInterval = '30s') => ({
  fastopm: { refresh_on_miss: true, refresh_on_miss_min_interval: minInterval },
});
const publish120 = act('uplink publishes 1.2.0 (latest)', (s) =>
  s.uplink().setPackage('simple-pkg', (d) => withNewVersion(d, '1.2.0', '2020-04-01T00:00:00.000Z'))
);
const T120 = tarballPath('simple-pkg', '1.2.0');
const T999 = tarballPath('simple-pkg', '9.9.9');

export default [
  {
    name: 'fastopm/refresh-on-miss-new-version',
    description:
      'refresh_on_miss + maxage 1h: a version published upstream after caching is found by GET /:package/:version and its tarball; GET /:package still follows maxage',
    uplinks: LONG_MAXAGE,
    config: refreshOnMiss(),
    steps: [
      get('/simple-pkg'),
      publish120,
      get('/simple-pkg', {}, { name: 'GET /simple-pkg within maxage (no sync)', candidateExpect: { uplinkRequests: 0 } }),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }, { candidateExpect: { uplinkRequests: 0 } }),
      get('/simple-pkg/1.0.0', {}, { candidateExpect: { status: 200, uplinkRequests: 0 } }),
      get('/simple-pkg/1.2.0', {}, { candidateExpect: { status: 200, uplinkRequests: 1 } }),
      get(T120, {}, { candidateExpect: { status: 200, uplinkRequests: 1 } }),
      get('/simple-pkg/latest', {}, { name: 'GET /simple-pkg/latest after the sync', candidateExpect: { status: 200, uplinkRequests: 0 } }),
    ],
  },
  {
    name: 'fastopm/refresh-on-miss-tarball-first',
    description: 'refresh_on_miss: the tarball of a version published after caching, requested directly',
    uplinks: LONG_MAXAGE,
    config: refreshOnMiss(),
    steps: [
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }),
      publish120,
      get(T120, {}, { candidateExpect: { status: 200, uplinkRequests: 2 } }),
    ],
  },
  {
    name: 'fastopm/refresh-on-miss-rate-limit',
    description:
      'refresh_on_miss_min_interval 2s: missing versions/tarballs sync at most once per interval; existing ones never',
    uplinks: LONG_MAXAGE,
    config: refreshOnMiss('2s'),
    steps: [
      get('/simple-pkg'),
      get('/simple-pkg/9.9.9', {}, { name: 'GET /simple-pkg/9.9.9 (forced sync, 304)', candidateExpect: { status: 404, uplinkRequests: 1 } }),
      get('/simple-pkg/9.9.9', {}, { name: 'GET /simple-pkg/9.9.9 (within interval)', candidateExpect: { status: 404, uplinkRequests: 0 } }),
      get('/simple-pkg/no-such-tag', {}, { candidateExpect: { status: 404, uplinkRequests: 0 } }),
      get(T999, {}, { candidateExpect: { status: 404, uplinkRequests: 0 } }),
      get('/simple-pkg/1.1.0', {}, { candidateExpect: { status: 200, uplinkRequests: 0 } }),
      wait(2200),
      get(T999, {}, { name: 'GET simple-pkg-9.9.9.tgz (after interval: forced sync)', candidateExpect: { status: 404, uplinkRequests: 1 } }),
    ],
  },
  {
    name: 'fastopm/refresh-on-miss-uplink-down',
    description:
      'refresh_on_miss with the uplink down: answers what Verdaccio answers (404, never 5xx); the open breaker is respected after the uplink is back',
    uplinks: LONG_MAXAGE,
    config: refreshOnMiss('1s'),
    steps: [
      get('/simple-pkg'),
      act('uplink down (connection refused)', (s) => s.uplink().setMode('down')),
      get('/simple-pkg/1.2.0', {}, { candidateExpect: { status: 404 }, timeout: 15000 }),
      get(T120, {}, { candidateExpect: { status: 404 }, timeout: 15000 }),
      act('uplink up, publishes 1.2.0', async (s) => {
        await s.uplink().setMode('up');
        s.uplink().setPackage('simple-pkg', (d) => withNewVersion(d, '1.2.0', '2020-04-01T00:00:00.000Z'));
      }),
      wait(1200),
      // the failed sync opened the breaker (max_fails 2, fail_timeout 5m): nothing goes upstream
      get('/simple-pkg/1.2.0', {}, { name: 'GET /simple-pkg/1.2.0 (breaker open)', candidateExpect: { status: 404, uplinkRequests: 0 } }),
      get('/simple-pkg/1.0.0', {}, { candidateExpect: { status: 200, uplinkRequests: 0 } }),
    ],
  },
];
