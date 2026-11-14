#!/usr/bin/env node
// Fetch the packuments the mock uplink serves (once; they are committed under fixtures/ so runs
// never touch the network). `node bench/fetch-fixtures.mjs [--force]`.
//
// - `ms`, `express`, `typescript` (the throughput benchmark, run.mjs): gzip, `<name>.json.gz`.
// - the large packuments of the memory test (memtest.mjs), seen in the cold-cache `npm install`
//   that got fastopm OOM-killed: brotli (q11, 16 MiB window; ~10x smaller than gzip for the
//   biggest ones), `<file>.json.br` where `@scope/pkg` becomes `scope__pkg`.

import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { brotliCompressSync, constants, gzipSync } from 'node:zlib';

const dir = join(dirname(fileURLToPath(import.meta.url)), 'fixtures');
const force = process.argv.includes('--force');
// small, medium, large (full packument: ~90 KB, ~0.8 MB, ~16 MB in Nov 2026)
const GZIP = ['ms', 'express', 'typescript'];
// full packument size in Nov 2026: vite 39 MB, @firebase/firestore 28 MB, @types/node 11 MB,
// rollup 6.7 MB, webpack 5.2 MB, @aws-sdk/client-s3 4.3 MB, zod 4.2 MB
export const BROTLI = [
  'vite',
  '@firebase/firestore',
  '@types/node',
  'rollup',
  'webpack',
  '@aws-sdk/client-s3',
  'zod',
];

/** `@scope/pkg` -> `scope__pkg` (fixture file stem). */
export const fixtureStem = (name) => name.replace(/^@/, '').replace('/', '__');

async function fetchPackument(name) {
  const path = name.replace('/', '%2F');
  const r = await fetch(`https://registry.npmjs.org/${path}`, {
    headers: { accept: 'application/json' },
  });
  if (!r.ok) throw new Error(`${name}: HTTP ${r.status}`);
  const body = Buffer.from(await r.arrayBuffer());
  JSON.parse(body.toString('utf8'));
  return body;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  mkdirSync(dir, { recursive: true });
  const jobs = [
    ...GZIP.map((name) => ({ name, file: join(dir, `${name}.json.gz`), brotli: false })),
    ...BROTLI.map((name) => ({ name, file: join(dir, `${fixtureStem(name)}.json.br`), brotli: true })),
  ];
  for (const { name, file, brotli } of jobs) {
    if (existsSync(file) && !force) {
      console.log(`${file} exists (use --force to refetch)`);
      continue;
    }
    const body = await fetchPackument(name);
    const packed = brotli
      ? brotliCompressSync(body, {
          params: {
            [constants.BROTLI_PARAM_QUALITY]: 11,
            [constants.BROTLI_PARAM_LGWIN]: 24,
            [constants.BROTLI_PARAM_SIZE_HINT]: body.length,
          },
        })
      : gzipSync(body, { level: 9 });
    writeFileSync(file, packed);
    console.log(`${name}: ${body.length} bytes, ${packed.length} stored`);
  }
}
