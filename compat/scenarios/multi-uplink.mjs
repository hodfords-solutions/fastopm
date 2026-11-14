import { tarballPath } from '../fixtures/packages.mjs';
import { act, get, NPM_INSTALL_ACCEPT, wait } from './helpers.mjs';

// `proxy: first second` for every package (lib/config.mjs defaultPackages).
const TWO = (first = {}, second = {}) => ({
  first: { packages: ['simple-pkg', '@scope/scoped-pkg'], config: { maxage: '1s', ...first } },
  second: { config: { maxage: '1s', ...second } },
});

export default [
  {
    name: 'multi-uplink/first-has-it',
    description: 'both uplinks configured, the first one has the package: second never asked',
    uplinks: TWO(),
    steps: [get('/simple-pkg'), get(tarballPath('simple-pkg', '1.0.0')), get('/@scope/scoped-pkg/latest')],
  },
  {
    name: 'multi-uplink/first-404',
    description: 'first uplink does not know the package (404), the second serves it',
    uplinks: TWO(),
    steps: [
      get('/other-pkg'),
      get('/other-pkg', { accept: NPM_INSTALL_ACCEPT }),
      get(tarballPath('other-pkg', '0.1.0')),
      wait(1200),
      get('/other-pkg'),
    ],
  },
  {
    name: 'multi-uplink/first-down',
    description: 'first uplink refuses connections, the second serves; then the first comes back',
    uplinks: TWO(),
    setup: (s) => s.uplink('first').setMode('down'),
    steps: [
      get('/simple-pkg'),
      get(tarballPath('simple-pkg', '1.1.0')),
      act('first uplink up', (s) => s.uplink('first').setMode('up')),
      wait(1200),
      get('/simple-pkg'),
    ],
  },
  {
    name: 'multi-uplink/first-503',
    description: 'first uplink answers 503, second serves (merge keeps both _uplinks entries)',
    uplinks: TWO({ max_fails: 100 }),
    setup: (s) => s.uplink('first').setMode({ type: 'status', status: 503 }),
    steps: [
      get('/@scope/scoped-pkg'),
      act('first uplink ok', (s) => s.uplink('first').setMode('up')),
      wait(1200),
      get('/@scope/scoped-pkg'),
      get('/@scope/scoped-pkg/-/scoped-pkg-1.0.0.tgz'),
    ],
  },
  {
    name: 'multi-uplink/all-fail',
    description: 'first 404, second down: nothing to serve',
    uplinks: TWO(),
    setup: (s) => s.uplink('second').setMode('down'),
    steps: [get('/other-pkg'), get(tarballPath('other-pkg', '0.1.0'))],
  },
];
