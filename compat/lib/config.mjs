// Builds the config.yaml for one side of a scenario. Both sides get the same
// document except for paths/ports (storage dir, mock uplink URLs).

import { deepMerge } from './util.mjs';
import { toYaml } from './yaml.mjs';

/** Fixed so the `Via` header, the anti-loop check and logs are identical on both sides. */
export const SERVER_ID = 'compat-srv';

export function defaultPackages(uplinkNames) {
  const proxy = uplinkNames.join(' ');
  return {
    // locally published packages (pre-seeded storage) are never proxied
    'local-*': { access: '$all', publish: '$authenticated' },
    '@*/*': { access: '$all', publish: '$authenticated', unpublish: '$authenticated', proxy },
    '**': { access: '$all', publish: '$authenticated', unpublish: '$authenticated', proxy },
  };
}

/**
 * @param {object} p
 * @param {string} p.storage absolute storage dir
 * @param {Record<string,{origin:string, config?:object}>} p.uplinks uplink name -> mock origin + options
 * @param {object} [p.packages] full `packages:` section (default: everything proxied through all uplinks)
 * @param {object} [p.extra] merged into the top level (url_prefix, ...)
 * @param {object} [p.patch] extra deep-merge applied last (used by --candidate-config)
 */
export function buildConfig({ storage, uplinks, packages, extra = {}, patch }) {
  const uplinkSection = {};
  for (const [name, u] of Object.entries(uplinks)) {
    uplinkSection[name] = { url: `${u.origin}/`, ...(u.config ?? {}) };
  }
  let doc = {
    storage,
    server_id: SERVER_ID,
    // fastopm has no web UI; disabled so unknown paths are not answered by the UI router
    web: { enable: false },
    // registration off: otherwise `npm adduser` creates a user and returns a random token
    // (noise in write-routes/*); fastopm ignores `auth` anyway
    auth: { htpasswd: { file: './htpasswd', max_users: -1 } },
    // without a `middlewares:` section Verdaccio enables verdaccio-audit, which forwards
    // /-/npm/v1/security/* to https://registry.npmjs.org; keep the harness offline
    middlewares: { audit: { enabled: false } },
    uplinks: uplinkSection,
    packages: packages ?? defaultPackages(Object.keys(uplinks)),
    log: { type: 'stdout', format: 'pretty', level: 'http' },
  };
  doc = deepMerge(doc, extra);
  if (patch) doc = deepMerge(doc, patch);
  return { doc, yaml: toYaml(doc) };
}
