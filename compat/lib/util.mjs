import { createHash } from 'node:crypto';
import net from 'node:net';

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export const md5 = (buf) => createHash('md5').update(buf).digest('hex');
export const sha1 = (buf) => createHash('sha1').update(buf).digest('hex');
export const sha512b64 = (buf) => createHash('sha512').update(buf).digest('base64');

/** Ask the OS for a free TCP port on 127.0.0.1. */
export function getFreePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

export function isPlainObject(v) {
  return v !== null && typeof v === 'object' && !Array.isArray(v) && !Buffer.isBuffer(v);
}

/** Deep merge `patch` into a copy of `base`. `null` in the patch deletes a key. */
export function deepMerge(base, patch) {
  if (!isPlainObject(base) || !isPlainObject(patch)) return structuredClone(patch);
  const out = structuredClone(base);
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete out[k];
    else if (isPlainObject(v) && isPlainObject(out[k])) out[k] = deepMerge(out[k], v);
    else out[k] = structuredClone(v);
  }
  return out;
}

export function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Run `fn` over items with at most `limit` in flight; results keep input order. */
export async function mapLimit(items, limit, fn) {
  const results = new Array(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.max(1, Math.min(limit, items.length)) }, async () => {
    while (next < items.length) {
      const i = next++;
      results[i] = await fn(items[i], i);
    }
  });
  await Promise.all(workers);
  return results;
}
