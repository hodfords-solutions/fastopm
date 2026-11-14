import { SERVER_ID } from '../lib/config.mjs';
import { get, req } from './helpers.mjs';

export default [
  {
    name: 'anti-loop/via-header',
    description: `Via containing our server_id (${SERVER_ID}) -> 508; other ids pass; position in the list`,
    steps: [
      get('/simple-pkg', { via: `1.1 ${SERVER_ID} (Verdaccio)` }),
      get('/simple-pkg', { via: `1.1 other-proxy, 1.1 ${SERVER_ID} (Verdaccio)` }),
      get('/simple-pkg', { via: `1.1 ${SERVER_ID}` }),
      get('/simple-pkg/-/simple-pkg-1.0.0.tgz', { via: `1.1 ${SERVER_ID} (Verdaccio)` }),
      get('/-/ping', { via: `1.1 ${SERVER_ID} (Verdaccio)` }),
      get('/simple-pkg', { via: '1.1 other-proxy (Verdaccio)' }),
      get('/simple-pkg', { via: `1.1 ${SERVER_ID}x` }),
      get('/simple-pkg', { via: `${SERVER_ID}` }),
      get('/simple-pkg', { via: '1.1 other-proxy', 'x-forwarded-for': '10.1.2.3' }),
    ],
  },
  {
    name: 'misc/ping-and-unknown-routes',
    description: 'GET /-/ping, unknown paths, HEAD/OPTIONS on read routes, query strings',
    steps: [
      get('/-/ping'),
      get('/-/ping?x=1'),
      get('/-/nope'),
      get('/-/v1/nope'),
      get('/a/b/c/d'),
      get('/simple-pkg/1.0.0/extra'),
      get('/'),
      get('//simple-pkg'),
      req('HEAD', '/-/ping'),
      get('/simple-pkg'),
      req('HEAD', '/simple-pkg'),
      req('HEAD', '/simple-pkg/-/simple-pkg-1.0.0.tgz'),
      req('OPTIONS', '/simple-pkg'),
      req('POST', '/simple-pkg'),
      req('PATCH', '/simple-pkg'),
    ],
  },
  {
    name: 'misc/unrouted-methods',
    description:
      'methods Verdaccio has no route for on read paths: Express 404 HTML page (not 405; PUT is a publish route)',
    steps: [
      req('DELETE', '/-/ping'),
      req('POST', '/-/nope'),
      req('POST', '/@scope/scoped-pkg'),
      req('PATCH', '/@scope%2Fscoped-pkg/1.0.0'),
      req('POST', '/simple-pkg/-/simple-pkg-1.0.0.tgz'),
      req('DELETE', '/simple-pkg/-/simple-pkg-1.0.0.tgz'),
      req('DELETE', '/simple-pkg/1.0.0'),
      req('POST', '/a%3Cb'),
      req('PROPFIND', '/simple-pkg'),
      req('GET', '/simple-pkg'),
    ],
  },
  {
    name: 'misc/client-authorization-ignored',
    description: 'client Authorization header on read routes (anonymous access, nothing forwarded upstream)',
    steps: [
      get('/simple-pkg', { authorization: 'Bearer abc.def.ghi' }),
      get('/@scope/scoped-pkg/1.0.0', { authorization: 'Basic dXNlcjpwYXNz' }),
      get('/simple-pkg/-/simple-pkg-1.0.0.tgz', { authorization: 'Bearer garbage' }),
      get('/does-not-exist', { authorization: 'Bearer garbage' }),
    ],
  },
  {
    name: 'misc/uplink-headers-and-auth',
    description: 'uplink `headers:` and `auth: {type: bearer, token}` are sent upstream on metadata and tarballs',
    uplinks: {
      upstream: {
        config: {
          headers: { 'x-custom': 'one', accept: 'application/vnd.npm.install-v1+json' },
          auth: { type: 'bearer', token: 'uplink-secret-token' },
        },
      },
    },
    steps: [get('/simple-pkg'), get('/simple-pkg/-/simple-pkg-1.1.0.tgz')],
  },
  {
    name: 'misc/uplink-auth-env-token',
    description: 'uplink auth with token_env (NPM_TOKEN) and basic type',
    env: { NPM_TOKEN: 'from-env-token', MY_TOKEN: 'from-my-token' },
    uplinks: {
      first: { packages: ['simple-pkg'], config: { auth: { type: 'basic', token_env: true } } },
      second: { config: { auth: { type: 'bearer', token_env: 'MY_TOKEN' } } },
    },
    steps: [get('/simple-pkg'), get('/other-pkg'), get('/other-pkg/-/other-pkg-0.1.0.tgz')],
  },
  {
    name: 'misc/user-agent-config',
    description: 'user_agent from config is used upstream (and in X-Powered-By)',
    config: { user_agent: 'my-registry/1.2.3' },
    steps: [get('/simple-pkg'), get('/simple-pkg/-/simple-pkg-1.0.0.tgz')],
  },
  {
    name: 'packages/storage-subfolder',
    description: 'packages: rule with `storage: sub` puts matching packages under <storage>/sub/',
    packages: {
      'sub-*': { access: '$all', proxy: 'upstream', storage: 'sub' },
      '**': { access: '$all', proxy: 'upstream' },
    },
    steps: [get('/sub-pkg'), get('/sub-pkg/-/sub-pkg-3.0.0.tgz'), get('/simple-pkg/1.0.0')],
  },
  {
    name: 'packages/no-proxy-rule',
    description: 'a package matching a rule without `proxy:` is never fetched upstream',
    packages: {
      'simple-*': { access: '$all' },
      '**': { access: '$all', proxy: 'upstream' },
    },
    steps: [get('/simple-pkg'), get('/simple-pkg/-/simple-pkg-1.0.0.tgz'), get('/other-pkg/latest')],
  },
];
