import { get, NPM_INSTALL_ACCEPT, ownEtag } from './helpers.mjs';

// ETags of full packuments contain the random part of `_rev`, so each side is
// sent the ETag it returned itself (ownEtag) rather than a shared literal.
export default [
  {
    name: 'etag/client-if-none-match',
    description: 'client If-None-Match: own ETag -> 304, foreign/garbage ETag -> 200, for full and abbreviated',
    steps: [
      get('/simple-pkg', {}, { saveAs: 'full' }),
      get('/simple-pkg', { 'if-none-match': ownEtag('full') }),
      get('/simple-pkg', { 'if-none-match': '"0123456789abcdef0123456789abcdef"' }),
      get('/simple-pkg', { 'if-none-match': (s) => `W/${ownEtag('full')(s)}` }),
      get('/simple-pkg', { 'if-none-match': (s) => `"nope", ${ownEtag('full')(s)}` }),
      get('/simple-pkg', { 'if-none-match': '*' }),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }, { saveAs: 'abbr' }),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT, 'if-none-match': ownEtag('abbr') }),
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT, 'if-none-match': ownEtag('full') }),
      get('/simple-pkg/1.0.0', {}, { saveAs: 'ver' }),
      get('/simple-pkg/1.0.0', { 'if-none-match': ownEtag('ver') }),
      get('/-/ping', {}, { saveAs: 'ping' }),
      get('/-/ping', { 'if-none-match': ownEtag('ping') }),
      get('/simple-pkg/-/simple-pkg-1.0.0.tgz', { 'if-none-match': ownEtag('ver') }),
      get('/does-not-exist', { 'if-none-match': '*' }),
    ],
  },
];
