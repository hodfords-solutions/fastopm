// Step builders shared by scenario files.

/** Accept header npm 10 sends for installs (abbreviated metadata preferred). */
export const NPM_INSTALL_ACCEPT =
  'application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*';
export const ABBREVIATED = 'application/vnd.npm.install-v1+json';

/** GET on both sides. Header values may be functions of the side (e.g. its previous ETag). */
export function get(path, headers = {}, extra = {}) {
  return { request: { method: 'GET', path, headers }, ...extra };
}

export function req(method, path, { headers = {}, body, ...extra } = {}) {
  return { request: { method, path, headers, body }, ...extra };
}

/** Run `fn(side)` on both sides (identically), e.g. to change mock uplink behaviour. */
export function act(name, fn) {
  return { name, do: fn };
}

export function wait(ms) {
  return { wait: ms };
}

/** Header value: the ETag this side returned for the step saved as `key` (or the last response). */
export const ownEtag = (key) => (side) =>
  (key ? side.responses[key] : side.last)?.headers?.etag ?? '"missing-etag"';
