// Mock npm registry used as uplink. Plain node:http, no dependencies.
//
// - Serves packuments (`GET /<name>`, scoped names accepted as `@s/p`, `@s%2Fp`, `%40s%2Fp`)
//   and tarballs (`GET /<name>/-/<file>.tgz`) built from fixtures/packages.mjs.
// - ETag = "md5(body)" on packuments and tarballs; `If-None-Match` match -> 304.
// - Behaviour is scriptable: `setMode()` for the whole server, `override()` per path.
// - Every request is recorded (method, raw url, headers, status served).

import http from 'node:http';

import { buildTarballs, PACKAGES, serializePackument } from '../fixtures/packages.mjs';
import { md5, sleep } from './util.mjs';

/**
 * Behaviour for a request:
 *   { type: 'ok' }                      serve normally
 *   { type: 'status', status, body? }   reply with that status (body default `{"error":"..."}`)
 *   { type: 'delay', ms, then? }        wait, then apply `then` (default ok)
 *   { type: 'reset' }                   destroy the socket without answering
 *   'down' (setMode only)               stop listening: connection refused
 */
export class MockUplink {
  constructor(name, { packages = Object.keys(PACKAGES) } = {}) {
    this.name = name;
    /** @type {Map<string, any>} package name -> definition */
    this.defs = new Map(packages.map((p) => [p, structuredClone(PACKAGES[p])]));
    this.mode = { type: 'ok' };
    /** @type {Array<{match: (path:string)=>boolean, behaviour:any}>} */
    this.overrides = [];
    this.requests = [];
    this.port = 0;
    this.server = null;
    this.down = false;
  }

  get origin() {
    return `http://127.0.0.1:${this.port}`;
  }

  async start() {
    await this.#listen(this.port);
  }

  async #listen(port) {
    this.server = http.createServer((req, res) => this.#handle(req, res));
    this.server.keepAliveTimeout = 30_000;
    await new Promise((resolve, reject) => {
      this.server.once('error', reject);
      this.server.listen(port, '127.0.0.1', () => {
        this.server.off('error', reject);
        resolve();
      });
    });
    this.port = this.server.address().port;
  }

  async #close() {
    if (!this.server) return;
    const srv = this.server;
    this.server = null;
    srv.closeAllConnections();
    await new Promise((resolve) => srv.close(() => resolve()));
  }

  async stop() {
    await this.#close();
  }

  /** 'up' | 'down' | behaviour object (applies to every request without an override). */
  async setMode(mode) {
    if (mode === 'down') {
      this.down = true;
      await this.#close();
      return;
    }
    if (this.down) {
      this.down = false;
      await this.#listen(this.port); // same port, so stored tarball URLs stay valid
    }
    this.mode = mode === 'up' ? { type: 'ok' } : mode;
  }

  /**
   * Per-request behaviour. `match` is a decoded path string (`/simple-pkg`), a RegExp
   * tested against the decoded path, or a function(decodedPath, rawUrl).
   */
  override(match, behaviour) {
    const fn =
      typeof match === 'function'
        ? match
        : match instanceof RegExp
          ? (p) => match.test(p)
          : (p) => p === match;
    this.overrides.unshift({ match: fn, behaviour });
  }

  clearOverrides() {
    this.overrides = [];
  }

  /** Replace or mutate a package definition (e.g. publish a new version upstream). */
  setPackage(name, defOrFn) {
    const next = typeof defOrFn === 'function' ? defOrFn(this.defs.get(name)) : defOrFn;
    if (next == null) this.defs.delete(name);
    else this.defs.set(name, next);
  }

  /** Requests received since the last call. */
  takeRequests() {
    const out = this.requests;
    this.requests = [];
    return out;
  }

  packumentBody(name) {
    const def = this.defs.get(name);
    return def ? Buffer.from(serializePackument(def, this.origin)) : null;
  }

  /**
   * ETag of a packument. Computed over the packument built with a canonical origin, so the
   * baseline's and the candidate's mocks (different ports) hand out the same ETag for the
   * same content and the stored `_uplinks.*.etag` / `If-None-Match` sent upstream compare equal.
   */
  packumentEtag(name) {
    const def = this.defs.get(name);
    return def ? `"${md5(serializePackument(def, 'http://uplink.invalid'))}"` : null;
  }

  async #handle(req, res) {
    let path;
    try {
      path = decodeURIComponent(req.url.split('?')[0]);
    } catch {
      path = req.url;
    }
    const entry = {
      method: req.method,
      url: req.url,
      headers: { ...req.headers },
      status: null,
    };
    this.requests.push(entry);
    const send = (status, headers, body) => {
      entry.status = status;
      res.writeHead(status, headers);
      res.end(body);
    };

    let behaviour = this.overrides.find((o) => o.match(path, req.url))?.behaviour ?? this.mode;
    while (behaviour.type === 'delay') {
      await sleep(behaviour.ms);
      if (res.destroyed || req.socket.destroyed) {
        entry.status = 'client-gone';
        return;
      }
      behaviour = behaviour.then ?? { type: 'ok' };
    }
    if (behaviour.type === 'reset') {
      entry.status = 'reset';
      req.socket.destroy();
      return;
    }
    if (behaviour.type === 'status') {
      const body =
        behaviour.body ?? JSON.stringify({ error: http.STATUS_CODES[behaviour.status] ?? 'error' });
      return send(
        behaviour.status,
        { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) },
        body
      );
    }

    if (req.method !== 'GET' && req.method !== 'HEAD') {
      return send(405, { 'content-type': 'application/json' }, '{"error":"method not allowed"}');
    }

    // tarball: /<name>/-/<file>
    const tgz = path.match(/^\/(.+)\/-\/([^/]+\.tgz)$/);
    if (tgz) {
      const def = this.defs.get(tgz[1]);
      const bytes = def ? buildTarballs(def).get(tgz[2]) : undefined;
      if (!bytes) return this.#notFound(send);
      const etag = `"${md5(bytes)}"`;
      if (req.headers['if-none-match'] === etag) return send(304, { etag }, undefined);
      return send(
        200,
        {
          'content-type': 'application/octet-stream',
          'content-length': bytes.length,
          etag,
        },
        req.method === 'HEAD' ? undefined : bytes
      );
    }

    const body = this.packumentBody(path.replace(/^\//, ''));
    if (!body) return this.#notFound(send);
    const etag = this.packumentEtag(path.replace(/^\//, ''));
    if (req.headers['if-none-match'] === etag) {
      return send(304, { etag }, undefined);
    }
    return send(
      200,
      {
        'content-type': 'application/json',
        'content-length': body.length,
        etag,
        'cache-control': 'public, max-age=300',
      },
      req.method === 'HEAD' ? undefined : body
    );
  }

  #notFound(send) {
    const body = '{"error":"Not found"}';
    send(404, { 'content-type': 'application/json', 'content-length': body.length }, body);
  }
}
