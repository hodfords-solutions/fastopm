// Deterministic npm-style tarball builder (ustar + gzip, fixed mtime/uid/gid),
// so fixtures need no binary files and shasum/integrity are stable.
import { gzipSync } from 'node:zlib';

function octal(n, width) {
  return n.toString(8).padStart(width - 1, '0') + '\0';
}

function header(name, size) {
  const h = Buffer.alloc(512, 0);
  h.write(name, 0, 100, 'utf8');
  h.write(octal(0o644, 8), 100, 8, 'ascii');
  h.write(octal(0, 8), 108, 8, 'ascii');
  h.write(octal(0, 8), 116, 8, 'ascii');
  h.write(octal(size, 12), 124, 12, 'ascii');
  // 1985-10-26T08:15:00Z, the date npm pack uses for reproducible tarballs
  h.write(octal(499162500, 12), 136, 12, 'ascii');
  h.fill(' ', 148, 156); // checksum placeholder
  h.write('0', 156, 1, 'ascii');
  h.write('ustar\0', 257, 6, 'ascii');
  h.write('00', 263, 2, 'ascii');
  let sum = 0;
  for (const b of h) sum += b;
  h.write(octal(sum, 7) + ' ', 148, 8, 'ascii');
  return h;
}

/**
 * @param {Record<string,string|Buffer>} files paths relative to `package/`
 * @returns {Buffer} gzipped tarball
 */
export function buildTarball(files) {
  const parts = [];
  for (const name of Object.keys(files).sort()) {
    const body = Buffer.isBuffer(files[name]) ? files[name] : Buffer.from(files[name], 'utf8');
    parts.push(header(`package/${name}`, body.length), body);
    const pad = (512 - (body.length % 512)) % 512;
    if (pad) parts.push(Buffer.alloc(pad, 0));
  }
  parts.push(Buffer.alloc(1024, 0));
  return gzipSync(Buffer.concat(parts), { level: 9 });
}
