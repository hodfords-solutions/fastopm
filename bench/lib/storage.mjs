// Seeded storage shared by every server of a run.

import { cpSync, existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { get } from './load.mjs';
import { sleep, startServer } from './servers.mjs';

export const PKGS = ['ms', 'express', 'typescript'];
export const TARBALLS = ['bench-tarball-1.0.0.tgz', 'bench-tarball-2.0.0.tgz'];

/**
 * Storage written by Verdaccio itself from the mock uplink (`dir/storage`): the common starting
 * point of the warm-cache runs. fastopm reads it as is (that is its compatibility promise).
 */
export async function buildTemplate(uplink, dir, maxage) {
  const storage = join(dir, 'storage');
  const srv = await startServer('verdaccio', { dir, storage, uplink: uplink.origin, maxage });
  try {
    for (const p of PKGS) {
      const r = await get(`${srv.url}/${p}`, { accept: 'application/json' });
      if (r.status !== 200) throw new Error(`template: GET /${p} -> ${r.status}`);
    }
    for (const f of TARBALLS) {
      const r = await get(`${srv.url}/bench-tarball/-/${f}`);
      if (r.status !== 200) throw new Error(`template: GET ${f} -> ${r.status}`);
    }
    await sleep(500); // let the tarball rename land
  } finally {
    await srv.stop();
  }
  for (const f of TARBALLS) {
    if (!existsSync(join(storage, 'bench-tarball', f))) throw new Error(`template: ${f} not cached`);
  }
  return storage;
}

/** Copy the template and set every `_uplinks.*.fetched` to now (so `maxage` starts now). */
export function seedStorage(template, dest) {
  cpSync(template, dest, { recursive: true });
  const now = Date.now();
  for (const name of readdirSync(dest)) {
    const file = join(dest, name, 'package.json');
    if (!existsSync(file)) continue;
    const text = readFileSync(file, 'utf8').replace(/("fetched"\s*:\s*)\d+/g, `$1${now}`);
    writeFileSync(file, text);
  }
}
