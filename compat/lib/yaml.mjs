// Minimal YAML emitter: enough for the config.yaml files the harness writes
// (nested maps, string/number/boolean scalars, arrays of scalars).

function scalar(v) {
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  if (v === null || v === undefined) return 'null';
  return JSON.stringify(String(v)); // JSON strings are valid double-quoted YAML scalars
}

function key(k) {
  return /^[A-Za-z_][A-Za-z0-9_-]*$/.test(k) ? k : JSON.stringify(k);
}

export function toYaml(obj, indent = 0) {
  const pad = ' '.repeat(indent);
  let out = '';
  for (const [k, v] of Object.entries(obj)) {
    if (v === undefined) continue;
    if (Array.isArray(v)) {
      out += `${pad}${key(k)}:\n`;
      for (const item of v) out += `${pad}  - ${scalar(item)}\n`;
    } else if (v !== null && typeof v === 'object') {
      out += `${pad}${key(k)}:\n${toYaml(v, indent + 2)}`;
    } else {
      out += `${pad}${key(k)}: ${scalar(v)}\n`;
    }
  }
  return out;
}
