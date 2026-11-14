#!/usr/bin/env node
// Fetch the packuments the mock uplink serves (once; they are committed as fixtures/*.json.gz so
// runs never touch the network). `node bench/fetch-fixtures.mjs [--force]`.

import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const dir = join(dirname(fileURLToPath(import.meta.url)), 'fixtures');
const force = process.argv.includes('--force');
// small, medium, large (full packument: ~90 KB, ~0.8 MB, ~16 MB in Nov 2026)
const PACKAGES = ['ms', 'express', 'typescript'];

mkdirSync(dir, { recursive: true });
for (const name of PACKAGES) {
  const file = join(dir, `${name}.json.gz`);
  if (existsSync(file) && !force) {
    console.log(`${file} exists (use --force to refetch)`);
    continue;
  }
  const r = await fetch(`https://registry.npmjs.org/${name}`, { headers: { accept: 'application/json' } });
  if (!r.ok) throw new Error(`${name}: HTTP ${r.status}`);
  const body = Buffer.from(await r.arrayBuffer());
  JSON.parse(body.toString('utf8'));
  writeFileSync(file, gzipSync(body, { level: 9 }));
  console.log(`${name}: ${body.length} bytes`);
}
