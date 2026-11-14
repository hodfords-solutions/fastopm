import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { gunzipSync, inflateSync, brotliDecompressSync } from 'node:zlib';

import { byteDiffExcerpt, unifiedDiff } from './diff.mjs';
import {
  BODY_DERIVED_HEADERS,
  IGNORED_RESPONSE_HEADERS,
  IGNORED_STORAGE_FILES,
  IGNORED_UPLINK_REQUEST_HEADERS,
} from './rules.mjs';
import { escapeRegExp, isPlainObject, md5, sha1 } from './util.mjs';

// ── normalisation ────────────────────────────────────────────────────────────

/**
 * Per-side text normaliser. `origins` maps a host:port to a label, e.g.
 * { '127.0.0.1:51234': '{uplink:upstream}', '127.0.0.1:51240': '{registry}' }.
 */
export function makeNormaliser(origins) {
  const entries = Object.entries(origins).sort((a, b) => b[0].length - a[0].length);
  const re = entries.length
    ? new RegExp(entries.map(([hp]) => `${escapeRegExp(hp)}(?!\\d)`).join('|'), 'g')
    : null;
  const map = new Map(entries);
  const text = (s) => {
    let out = re ? s.replace(re, (m) => map.get(m)) : s;
    out = out.replace(/("_rev"\s*:\s*"\d+-)([0-9a-f]+)"/g, (_, pre, hex) => `${pre}<hex:${hex.length}>"`);
    return out;
  };
  const deep = (v, keyPath = []) => {
    if (typeof v === 'string') {
      if (keyPath.at(-1) === '_rev' && keyPath.length === 1) {
        const m = v.match(/^(\d+)-([0-9a-f]+)$/);
        if (m) return `${m[1]}-<hex:${m[2].length}>`;
      }
      return text(v);
    }
    if (typeof v === 'number' && keyPath.length === 3 && keyPath[0] === '_uplinks' && keyPath[2] === 'fetched') {
      return '<timestamp>';
    }
    if (Array.isArray(v)) return v.map((x, i) => deep(x, [...keyPath, i]));
    if (isPlainObject(v)) {
      const out = {};
      for (const [k, x] of Object.entries(v)) out[text(k)] = deep(x, [...keyPath, k]);
      return out;
    }
    return v;
  };
  return { text, deep };
}

// ── response comparison ─────────────────────────────────────────────────────

function decodeBody(res) {
  const enc = String(res.headers['content-encoding'] ?? '').toLowerCase();
  try {
    if (enc === 'gzip') return gunzipSync(res.body);
    if (enc === 'deflate') return inflateSync(res.body);
    if (enc === 'br') return brotliDecompressSync(res.body);
  } catch (err) {
    return Buffer.from(`<undecodable ${enc} body: ${err.message}>`);
  }
  return res.body;
}

function isTextual(res, body) {
  const ct = String(res.headers['content-type'] ?? '');
  if (/json|text|javascript|xml/.test(ct)) return true;
  if (!ct && body.length && body.length < 1_000_000) {
    try {
      JSON.parse(body.toString('utf8'));
      return true;
    } catch {
      return false;
    }
  }
  return false;
}

function prettyForDiff(text) {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** Express' default ETag (etag package): W/"<len hex>-<sha1 base64, 27 chars>" */
function expressWeakEtag(buf) {
  if (buf.length === 0) return 'W/"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk"';
  return `W/"${buf.length.toString(16)}-${createHash('sha1').update(buf).digest('base64').substring(0, 27)}"`;
}

const ETAG_KINDS = {
  'md5(body)': (buf) => `"${md5(buf)}"`,
  'express weak etag W/"len-sha1"': expressWeakEtag,
};

function etagKind(value, body) {
  for (const [kind, fn] of Object.entries(ETAG_KINDS)) if (value === fn(body)) return kind;
  return null;
}

function headerValue(v) {
  return Array.isArray(v) ? v.join(', ') : v;
}

/**
 * @returns {Array<{aspect:string, message:string}>}
 */
export function compareResponses(b, c, sideB, sideC) {
  const diffs = [];
  const normB = sideB.norm;
  const normC = sideC.norm;
  if (b.error || c.error) {
    if (b.error !== c.error) {
      diffs.push({
        aspect: 'transport',
        message: `baseline: ${b.error ?? 'ok'}\ncandidate: ${c.error ?? 'ok'}`,
      });
    }
    if (b.status === 0 || c.status === 0) return diffs;
  }
  if (b.status !== c.status) {
    diffs.push({ aspect: 'status', message: `baseline ${b.status}, candidate ${c.status}` });
  }

  const bodyB = decodeBody(b);
  const bodyC = decodeBody(c);
  const textual = isTextual(b, bodyB) || isTextual(c, bodyC);
  const rawEqual = bodyB.equals(bodyC);
  let normEqual = rawEqual;
  if (!rawEqual) {
    if (textual) {
      const tB = normB.text(bodyB.toString('utf8'));
      const tC = normC.text(bodyC.toString('utf8'));
      normEqual = tB === tC;
      if (!normEqual) {
        const pB = prettyForDiff(tB);
        const pC = prettyForDiff(tC);
        const message =
          pB === pC
            ? 'same JSON value, different bytes (formatting/whitespace/escaping):\n' +
              byteDiffExcerpt(Buffer.from(tB), Buffer.from(tC))
            : unifiedDiff(pB, pC);
        diffs.push({ aspect: 'body', message });
      }
    } else {
      diffs.push({
        aspect: 'body',
        message: `binary bodies differ: baseline ${bodyB.length} bytes sha1 ${sha1(bodyB)}, candidate ${bodyC.length} bytes sha1 ${sha1(bodyC)}`,
      });
    }
  }

  // 304 / HEAD: no body to derive ETag and Content-Length from. Relate each side's ETag to
  // the body that side returned earlier with that ETag (sides keep an `etagBook`).
  const bodyless = bodyB.length === 0 && bodyC.length === 0 && (b.status === 304 || b.method === 'HEAD');
  const names = new Set([...Object.keys(b.headers), ...Object.keys(c.headers)]);
  for (const name of [...names].sort()) {
    if (name in IGNORED_RESPONSE_HEADERS) continue;
    const vB = headerValue(b.headers[name]);
    const vC = headerValue(c.headers[name]);
    if (bodyless && BODY_DERIVED_HEADERS.includes(name) && vB !== vC && vB !== undefined && vC !== undefined) {
      const eB = sideB.etagBook.get(headerValue(b.headers.etag));
      const eC = sideC.etagBook.get(headerValue(c.headers.etag));
      let ok = false;
      if (eB && eC && eB.normSha1 === eC.normSha1) {
        ok = name === 'etag' || (Number(vB) === eB.length && Number(vC) === eC.length);
      }
      if (!ok) {
        diffs.push({
          aspect: `header ${name}`,
          message:
            `baseline ${fmt(vB)}, candidate ${fmt(vC)} (no body in this response; ` +
            (eB && eC
              ? `the ETags were earlier returned with ${eB.normSha1 === eC.normSha1 ? 'equivalent' : 'different'} bodies)`
              : `${eB ? 'candidate' : 'baseline'} ETag was never seen with a body)`),
        });
      }
      continue;
    }
    if (BODY_DERIVED_HEADERS.includes(name) && !rawEqual) {
      // bodies differ (only by normalised parts, or for real and already reported above):
      // check each side derives the header from its own body the same way
      if (vB === undefined || vC === undefined) {
        if (vB !== vC) diffs.push({ aspect: `header ${name}`, message: `baseline ${fmt(vB)}, candidate ${fmt(vC)}` });
        continue;
      }
      if (name === 'content-length') {
        const okB = Number(vB) === b.body.length;
        const okC = Number(vC) === c.body.length;
        if (okB !== okC) {
          diffs.push({
            aspect: 'header content-length',
            message: `baseline ${vB} (${okB ? '=' : '≠'} its body length), candidate ${vC} (body is ${c.body.length} bytes)`,
          });
        }
      } else if (name === 'etag') {
        const kB = etagKind(vB, bodyB);
        const kC = etagKind(vC, bodyC);
        if (kB === null ? normB.text(vB) !== normC.text(vC) : kB !== kC) {
          diffs.push({
            aspect: 'header etag',
            message: `baseline ${vB} (${kB ?? 'unknown derivation'}), candidate ${vC} (${kC ?? 'not derived from its body like the baseline'})`,
          });
        }
      }
      continue;
    }
    const nB = vB === undefined ? undefined : normB.text(String(vB));
    const nC = vC === undefined ? undefined : normC.text(String(vC));
    if (nB !== nC) diffs.push({ aspect: `header ${name}`, message: `baseline ${fmt(nB)}, candidate ${fmt(nC)}` });
  }
  return diffs;
}

/** Remember which (normalised) body each ETag was sent with, for later 304/HEAD comparisons. */
export function recordEtag(res, side) {
  const etag = headerValue(res.headers?.etag);
  if (!etag || !res.body?.length) return;
  const body = decodeBody(res);
  side.etagBook.set(etag, { normSha1: sha1(side.norm.text(body.toString('utf8'))), length: body.length });
}

function fmt(v) {
  return v === undefined ? '(absent)' : JSON.stringify(v);
}

// ── uplink request comparison ───────────────────────────────────────────────

function normaliseUplinkRequest(r, norm) {
  const headers = {};
  for (const [k, v] of Object.entries(r.headers).sort(([a], [b]) => a.localeCompare(b))) {
    if (k in IGNORED_UPLINK_REQUEST_HEADERS) continue;
    headers[k] = norm.text(String(v));
  }
  return { method: r.method, url: norm.text(r.url), served: r.status, headers };
}

function describeUplinkRequest(r) {
  return `${r.method} ${r.url} -> ${r.served}\n${Object.entries(r.headers)
    .map(([k, v]) => `    ${k}: ${v}`)
    .join('\n')}`;
}

/** @param {Record<string,Array>} b uplink name -> requests */
export function compareUplinkRequests(b, c, normB, normC) {
  const diffs = [];
  const names = new Set([...Object.keys(b), ...Object.keys(c)]);
  for (const name of names) {
    const lB = (b[name] ?? []).map((r) => normaliseUplinkRequest(r, normB));
    const lC = (c[name] ?? []).map((r) => normaliseUplinkRequest(r, normC));
    const tB = lB.map(describeUplinkRequest).join('\n');
    const tC = lC.map(describeUplinkRequest).join('\n');
    if (tB !== tC) {
      diffs.push({
        aspect: `uplink ${name} requests`,
        message: `baseline sent ${lB.length}, candidate sent ${lC.length}\n${unifiedDiff(tB, tC)}`,
      });
    }
  }
  return diffs;
}

// ── storage comparison ──────────────────────────────────────────────────────

export function snapshotStorage(root) {
  const files = new Map();
  if (!fs.existsSync(root)) return files;
  const walk = (dir) => {
    for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
      const abs = path.join(dir, ent.name);
      const rel = path.relative(root, abs).split(path.sep).join('/');
      if (ent.isDirectory()) {
        files.set(rel + '/', { kind: 'dir' });
        walk(abs);
      } else {
        if (rel in IGNORED_STORAGE_FILES) continue;
        const bytes = fs.readFileSync(abs);
        files.set(rel, { kind: 'file', bytes });
      }
    }
  };
  walk(root);
  return files;
}

function describeJsonFile(bytes, norm) {
  const raw = bytes.toString('utf8');
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { format: 'invalid JSON', text: norm.text(raw) };
  }
  const format =
    raw === JSON.stringify(parsed)
      ? 'compact JSON'
      : raw === JSON.stringify(parsed, null, 2)
        ? 'JSON indented 2'
        : 'JSON (other formatting)';
  return { format, text: JSON.stringify(norm.deep(parsed), null, 2) };
}

export function compareStorage(snapB, snapC, normB, normC) {
  const diffs = [];
  const all = [...new Set([...snapB.keys(), ...snapC.keys()])].sort();
  const onlyB = all.filter((k) => !snapC.has(k));
  const onlyC = all.filter((k) => !snapB.has(k));
  if (onlyB.length || onlyC.length) {
    diffs.push({
      aspect: 'storage files',
      message: [
        ...onlyB.map((f) => `only in baseline : ${f}`),
        ...onlyC.map((f) => `only in candidate: ${f}`),
      ].join('\n'),
    });
  }
  for (const rel of all) {
    const fB = snapB.get(rel);
    const fC = snapC.get(rel);
    if (!fB || !fC || fB.kind === 'dir' || fC.kind === 'dir') continue;
    if (rel.endsWith('.json')) {
      const dB = describeJsonFile(fB.bytes, normB);
      const dC = describeJsonFile(fC.bytes, normC);
      if (dB.format !== dC.format) {
        diffs.push({ aspect: `storage ${rel} format`, message: `baseline ${dB.format}, candidate ${dC.format}` });
      }
      if (dB.text !== dC.text) {
        diffs.push({ aspect: `storage ${rel}`, message: unifiedDiff(dB.text, dC.text) });
      }
    } else if (!fB.bytes.equals(fC.bytes)) {
      diffs.push({
        aspect: `storage ${rel}`,
        message: `bytes differ: baseline ${fB.bytes.length} B sha1 ${sha1(fB.bytes)}, candidate ${fC.bytes.length} B sha1 ${sha1(fC.bytes)}`,
      });
    }
  }
  return diffs;
}
