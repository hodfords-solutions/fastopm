import { tarballPath } from '../fixtures/packages.mjs';
import { get, NPM_INSTALL_ACCEPT } from './helpers.mjs';

// dist.tarball in responses is rewritten to the registry's public URL.
const reqs = (headers) => [
  get('/simple-pkg', headers),
  get('/simple-pkg', { ...headers, accept: NPM_INSTALL_ACCEPT }),
  get('/simple-pkg/1.0.0', headers),
  get('/@scope/scoped-pkg/latest', headers),
];

export default [
  {
    name: 'tarball-url/host-header',
    description: 'public URL follows the Host header (custom host, host without port, IPv6, the real listen address)',
    steps: [
      ...reqs({ host: 'registry.example.com:8080' }),
      get('/simple-pkg/1.1.0', { host: 'npm.internal' }),
      get('/simple-pkg/1.1.0', { host: '[::1]:4873' }),
      get('/simple-pkg/1.1.0', { host: undefined }),
      get('/simple-pkg/1.1.0', { host: 'bad host!' }),
      get('/simple-pkg/1.1.0', { host: '' }),
    ],
  },
  {
    name: 'tarball-url/x-forwarded-proto',
    description: 'X-Forwarded-Proto https, HAProxy "https,http", unknown protocol, and empty',
    steps: [
      ...reqs({ 'x-forwarded-proto': 'https' }),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': 'https,http' }),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': 'ftp' }),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': '' }),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': 'HTTPS' }),
    ],
  },
  {
    name: 'tarball-url/url-prefix',
    description: 'url_prefix /npm/ changes generated tarball URLs; routes stay at the root',
    config: { url_prefix: '/npm/' },
    steps: [
      ...reqs({}),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': 'https', host: 'registry.example.com' }),
      get(tarballPath('simple-pkg', '1.0.0')),
      get(`/npm${tarballPath('simple-pkg', '1.1.0')}`),
      get('/npm/simple-pkg'),
    ],
  },
  {
    name: 'tarball-url/url-prefix-no-slashes',
    description: 'url_prefix written without leading/trailing slash',
    config: { url_prefix: 'registry' },
    steps: [get('/simple-pkg/1.0.0'), get('/@scope/scoped-pkg/1.0.0')],
  },
  {
    name: 'tarball-url/public-url-env',
    description: 'VERDACCIO_PUBLIC_URL overrides Host/X-Forwarded-Proto',
    env: { VERDACCIO_PUBLIC_URL: 'https://public.example.com/' },
    steps: [...reqs({}), get('/simple-pkg/1.0.0', { host: 'other.example.com', 'x-forwarded-proto': 'http' })],
  },
  {
    name: 'tarball-url/public-url-env-with-prefix',
    description: 'VERDACCIO_PUBLIC_URL with a path, combined with url_prefix',
    env: { VERDACCIO_PUBLIC_URL: 'https://public.example.com/base/' },
    config: { url_prefix: '/npm/' },
    steps: [get('/simple-pkg/1.0.0'), get('/@scope/scoped-pkg/1.0.0')],
  },
  {
    name: 'tarball-url/public-url-env-invalid',
    description: 'invalid VERDACCIO_PUBLIC_URL is ignored (falls back to the Host header)',
    env: { VERDACCIO_PUBLIC_URL: 'ftp://nope.example.com' },
    steps: [get('/simple-pkg/1.0.0')],
  },
  {
    name: 'tarball-url/forwarded-proto-env',
    description: 'VERDACCIO_FORWARDED_PROTO names another header carrying the protocol',
    env: { VERDACCIO_FORWARDED_PROTO: 'X-Custom-Proto' },
    steps: [
      get('/simple-pkg/1.0.0', { 'x-custom-proto': 'https' }),
      get('/simple-pkg/1.0.0', { 'x-forwarded-proto': 'https' }),
    ],
  },
];
