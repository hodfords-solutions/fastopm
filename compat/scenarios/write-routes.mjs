// Write routes. fastopm answers 405 on purpose, so these scenarios are listed in
// lib/rules.mjs ALLOWED_DIFFS: differences are reported but do not fail the run.
// They still document what Verdaccio answers, and catch a candidate that would
// accidentally write to storage or call the uplink.

import { get, req } from './helpers.mjs';

const json = { 'content-type': 'application/json' };

const publishBody = {
  _id: 'new-pkg',
  name: 'new-pkg',
  'dist-tags': { latest: '1.0.0' },
  versions: {
    '1.0.0': {
      name: 'new-pkg',
      version: '1.0.0',
      dist: { shasum: '0000000000000000000000000000000000000000', tarball: 'http://localhost:4873/new-pkg/-/new-pkg-1.0.0.tgz' },
    },
  },
  _attachments: {
    'new-pkg-1.0.0.tgz': { content_type: 'application/octet-stream', data: 'H4sIAAAAAAAAA2NgGAWjYBSMAgAAAP//AwBQ6U4qAAgAAA==', length: 20 },
  },
};

export default [
  {
    name: 'write-routes/publish-and-unpublish',
    description: 'PUT /:package (publish), PUT on a cached package, DELETE routes, without credentials',
    steps: [
      req('PUT', '/new-pkg', { headers: json, body: publishBody }),
      req('PUT', '/@scope%2fnew-pkg', { headers: json, body: { ...publishBody, name: '@scope/new-pkg' } }),
      get('/simple-pkg'),
      req('PUT', '/simple-pkg/-rev/1-0', { headers: json, body: { name: 'simple-pkg', versions: {} } }),
      req('DELETE', '/simple-pkg/-rev/1-0'),
      req('DELETE', '/simple-pkg/-/simple-pkg-1.0.0.tgz/-rev/1-0'),
      // PUT /:package/:_rev? matches these too (publish route)
      req('PUT', '/-/ping'),
      req('PUT', '/simple-pkg/1.0.0'),
      req('PUT', '/new-pkg', {
        headers: { ...json, authorization: 'Bearer not-a-token' },
        body: publishBody,
      }),
      get('/new-pkg'),
    ],
  },
  {
    name: 'write-routes/user-and-token',
    description: 'npm adduser/login/whoami/token routes',
    steps: [
      req('PUT', '/-/user/org.couchdb.user:x', {
        headers: json,
        body: { name: 'x', password: 'secret-password', email: 'x@example.com', type: 'user' },
      }),
      get('/-/user/org.couchdb.user:x'),
      get('/-/whoami'),
      req('POST', '/-/v1/login', { headers: json, body: {} }),
      get('/-/npm/v1/tokens'),
      req('DELETE', '/-/user/token/abc'),
    ],
  },
  {
    name: 'write-routes/dist-tags-and-audit',
    description: 'dist-tag edits and npm audit (out of fastopm scope; search is not compared: its body carries the current time)',
    steps: [
      req('PUT', '/-/package/simple-pkg/dist-tags/next', { headers: json, body: '"1.0.0"' }),
      get('/-/package/simple-pkg/dist-tags'),
      req('POST', '/-/npm/v1/security/advisories/bulk', { headers: json, body: { 'simple-pkg': ['1.0.0'] } }),
      req('POST', '/-/npm/v1/security/audits/quick', { headers: json, body: {} }),
      get('/-/all'),
    ],
  },
];
