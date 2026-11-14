// Small line-based unified diff (LCS), enough for pretty-printed JSON bodies.

export function unifiedDiff(a, b, { context = 3, maxLines = 120, labelA = 'baseline', labelB = 'candidate' } = {}) {
  const A = a.split('\n');
  const B = b.split('\n');
  const n = A.length;
  const m = B.length;
  if (n * m > 25_000_000) {
    return `(texts too large for a line diff: ${n} vs ${m} lines)`;
  }
  // LCS table from the end
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) ops.push([' ', A[i++], j++]);
    else if (dp[i + 1][j] >= dp[i][j + 1]) ops.push(['-', A[i++]]);
    else ops.push(['+', B[j++]]);
  }
  while (i < n) ops.push(['-', A[i++]]);
  while (j < m) ops.push(['+', B[j++]]);

  // group into hunks with context
  const changed = ops.map((o) => o[0] !== ' ');
  const keep = new Array(ops.length).fill(false);
  for (let k = 0; k < ops.length; k++) {
    if (!changed[k]) continue;
    for (let c = Math.max(0, k - context); c <= Math.min(ops.length - 1, k + context); c++) keep[c] = true;
  }
  const out = [`--- ${labelA}`, `+++ ${labelB}`];
  let prevKept = true;
  for (let k = 0; k < ops.length; k++) {
    if (!keep[k]) {
      prevKept = false;
      continue;
    }
    if (!prevKept) out.push('@@ ... @@');
    prevKept = true;
    out.push(`${ops[k][0]}${ops[k][1]}`);
  }
  if (out.length > maxLines + 2) {
    const extra = out.length - maxLines - 2;
    return [...out.slice(0, maxLines + 2), `... (${extra} more diff lines)`].join('\n');
  }
  return out.join('\n');
}

/** First differing byte offset with a short escaped excerpt of both sides. */
export function byteDiffExcerpt(a, b, radius = 40) {
  const len = Math.min(a.length, b.length);
  let k = 0;
  while (k < len && a[k] === b[k]) k++;
  const show = (buf) =>
    JSON.stringify(buf.subarray(Math.max(0, k - radius), k + radius).toString('latin1'));
  return `first difference at byte ${k} (lengths ${a.length} vs ${b.length})\n  baseline : ${show(a)}\n  candidate: ${show(b)}`;
}
