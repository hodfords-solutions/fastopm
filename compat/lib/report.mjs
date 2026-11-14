const useColor = process.stdout.isTTY && !process.env.NO_COLOR;
const c = (code) => (s) => (useColor ? `\x1b[${code}m${s}\x1b[0m` : s);
export const red = c('31');
export const green = c('32');
export const yellow = c('33');
export const dim = c('2');
export const bold = c('1');

const indent = (s, n) =>
  s
    .split('\n')
    .map((l) => ' '.repeat(n) + l)
    .join('\n');

function colorDiffLine(l) {
  if (l.startsWith('+') && !l.startsWith('+++')) return green(l);
  if (l.startsWith('-') && !l.startsWith('---')) return red(l);
  return l;
}

function printDiffs(diffs, n) {
  for (const d of diffs) {
    console.log(indent(`${bold(d.aspect)}:`, n));
    console.log(indent(d.message.split('\n').map(colorDiffLine).join('\n'), n + 2));
  }
}

export function printScenario(r, { verbose = false } = {}) {
  const time = dim(`${(r.ms / 1000).toFixed(1)}s`);
  if (r.status === 'error') {
    console.log(`${red('ERROR')} ${r.name} ${time}`);
    console.log(indent(r.error, 4));
    return;
  }
  const mark = { match: green('✔ match  '), diff: red('✘ DIFF   '), allowed: yellow('≈ allowed') }[r.status];
  console.log(`${mark} ${r.name} ${dim(`(${r.steps.length} requests)`)} ${time}`);
  if (r.status === 'allowed') console.log(indent(dim(`allowed: ${r.allowedReason}`), 12));
  const showAll = verbose || r.status === 'diff';
  for (const s of r.steps) {
    if (!showAll && !(verbose && s.diffs.length)) continue;
    if (!verbose && !s.diffs.length) continue;
    const tag = s.diffs.length ? (s.allowed ? yellow('allowed diff') : red('diff')) : green('ok');
    console.log(indent(`#${s.index} ${s.name}  → ${tag}`, 4));
    if (verbose) {
      const line = (who, x) =>
        `${who}: ${x.status}${x.error ? ` (${x.error})` : ''}${x.body ? ` ${x.body}` : ''}` +
        (x.uplink.length ? `\n           uplink: ${x.uplink.join(' | ')}` : '');
      console.log(indent(dim(line('baseline ', s.baseline)), 8));
      console.log(indent(dim(line('candidate', s.candidate)), 8));
    }
    if (s.diffs.length && (verbose || !s.allowed)) printDiffs(s.diffs, 8);
  }
  if (r.storage.length && (verbose || r.status === 'diff')) {
    console.log(indent('storage after scenario  → ' + (r.status === 'allowed' ? yellow('allowed diff') : red('diff')), 4));
    printDiffs(r.storage, 8);
  }
  if (r.dir) console.log(indent(dim(`kept: ${r.dir}`), 4));
}

export function printSummary(results, { baselineLabel, candidateLabel, totalMs }) {
  const count = (st) => results.filter((r) => r.status === st).length;
  console.log('');
  console.log(bold('Summary'));
  console.log(`  baseline : ${baselineLabel}`);
  console.log(`  candidate: ${candidateLabel}`);
  console.log(
    `  scenarios: ${results.length}  ${green(`match ${count('match')}`)}  ${yellow(`allowed ${count('allowed')}`)}  ${red(`diff ${count('diff')}`)}  ${red(`error ${count('error')}`)}  ${dim(`(${(totalMs / 1000).toFixed(1)}s)`)}`
  );
  const failed = results.filter((r) => r.status === 'diff' || r.status === 'error');
  if (failed.length) {
    console.log(`  failing  : ${failed.map((r) => r.name).join(', ')}`);
  }
}
