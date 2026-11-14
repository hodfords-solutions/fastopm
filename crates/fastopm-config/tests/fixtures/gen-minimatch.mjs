// Generates crates/fastopm-config/tests/fixtures/minimatch-vectors.json from the real
// minimatch (the version verdaccio@7.1.0 uses), with Verdaccio's options { nocase: true }.
// Usage: node gen-minimatch.mjs <path-to-minimatch-package> <out.json>
import { createRequire } from 'node:module';
import fs from 'node:fs';
const [, , mmPath, out] = process.argv;
const require = createRequire(import.meta.url);
const { minimatch } = require(mmPath);
const mmVersion = JSON.parse(fs.readFileSync(mmPath + '/package.json', 'utf8')).version;

const patterns = [
  // seen in real configs
  '**', '*', '@*/*', '@scope/*', '@my-company/*', '@*/foo', 'prefix-*', '*-plugin', '@babel/*',
  '@types/*', 'jquery', 'react', '@angular/core', 'test-*', 'private-*', '@private/**', '**/*',
  '*/*', '@*/**', '@*', 'local-*', '@verdaccio/*', 'npm*', 'lodash.*', 'vue-*', '*react*',
  '@*/*-plugin', '@company/**', 'my-*-lib', 'lodash',
  // wildcards and classes
  '?', '??', '???*', 'foo?', '[abc]*', '[!a]*', '[^a]*', '[a-c]*', '[[:alpha:]]*', '[[:digit:]]*',
  '[[:upper:]]*', '[A-Z]*', '[]a]*', '[a-]*', '[\\]]*', '[', '[]', ']', '[!]', '[z-a]*', '[.]*',
  // braces
  '{foo,bar}', '{foo,bar}*', '@{a,b}/*', '@scope/{a,b,c}', '{a..c}*', '{1..3}', '{01..03}',
  'foo{,-bar}', '{a,{b,c}}', '{}', '{a}', 'a{b', '{a,b', '{,}', '{a,b}{c,d}', '{a..e..2}',
  '{c..a}', '{3..1}', '@{scope,other}/{foo,bar}', '{x,@*/*}', '{**,foo}', '{foo/bar,baz}',
  // negation and comments
  '!foo', '!@*/*', '!!foo', '!!!foo', '#foo', '#*', '\\#foo', '\\!foo', '!', '#', '',
  // dots
  '.*', '*.js', '.foo', '**/.foo', '@*/.*', '*.*', '.', '..', './foo', '../foo', 'foo/.',
  // globstar and slashes
  'a/**/b', '**/b', 'a/**', '@scope/**/*', '**/**', 'foo/', '/foo', 'a//b', '***', '**foo',
  'foo**', '*/', '**/', '@*/**/*', 'a/*/b', '*/**/*',
  // extglobs
  '+(a|b)', '*(foo)', '@(foo|bar)', '!(foo)', '?(foo)bar', '!(foo)*', '@(a|b)*', '+(a|b)c',
  '!(@*)/*', '*(a|b|c)', '@(foo)', 'x!(y)z', '!(a|b)', '+(foo|bar)-*',
  // case and escapes
  'FOO', '@Scope/*', '@SCOPE/FOO', '\\*', 'foo\\', 'a\\b', '\\a', 'İ*', 'ü*', 'Ü', 'ß', 'K',
  // odd characters
  'a b', '* *', '(', ')', '|', '+', 'a+b', 'a|b', '$foo', '^foo', 'foo$', 'a.b', '%40scope/*',
  // harder extglobs
  '+(@(a|b))', '!(!(a))', '*(*(a))', '?(a|)', '@(|a)', '@()', '?()', '!()', '+()', '*()', '@(a',
  'a)', 'a|b|c', '@(a|b)/@(c|d)', '!(foo)bar', '*.!(js)', '!(*.*)', '@(*)', '!(@*/*)',
  // harder classes
  '[[:alpha:]-]*', '[a-[:alpha:]]', '[\\!]*', '[^]]*', '[[:alnum:]]', '[[:punct:]]*', '[[:space:]]',
  '[[:xdigit:]]', '[[:lower:]]*', '[[:word:]]*', '[[:bogus:]]',
  // harder braces
  '{,a}', '{a,}', 'a{,b}c', '{a,b}/**', '**/{a,b}', '@*/{foo,bar}*', 'x{a..c}y', '{a..c}{1..2}',
  '{-1..1}', '{a..C}', '{Z..b}', '{1..10..3}', '{-01..1}', '{a,b}{', '}{a,b}', '{{a,b}', '{a,b}}',
  'a{}b', '{a{b,c}d}', 'x{{a,b}}y', '$x{a,b}', '{a,b\\,c}', '{a\\,b}', '\\{a,b}', '{a,b}\\',
  // harder globstars
  '*/**/b', '**/*/b', 'a/**/**/b', 'a/*/../b', '../**', 'a/..', '**/a/**', 'a/**/b/**/c', '**/.*',
];

const names = [
  'foo', 'bar', 'baz', 'FOO', 'Foo', 'foobar', 'foo-bar', 'prefix-x', 'prefix-', 'x-plugin',
  'jquery', 'react', 'react-dom', 'preact', '@scope/foo', '@Scope/Foo', '@SCOPE/FOO', '@scope/bar',
  '@scope/a', '@scope/b', '@other/foo', '@a/x', '@b/x', '@babel/core', '@types/node',
  '@my-company/x', '@company/a/b', '@verdaccio/ui-theme', '@verdaccio/x-plugin', '@private/a',
  '@private/a/b', 'a/b', 'a/b/c', 'a/x/y/b', 'a/x/b', 'x/b', 'b', 'a', 'c', 'd', 'e', 'ab', 'abc',
  'ac', 'bc', 'bd', 'ad', 'cc', 'xyz', 'xz', 'xaz', '1', '2', '3', '01', '02', 'lodash',
  'lodash.merge', 'foo.js', 'a.b', 'a.b.c', 'npm', 'npmlog', 'vue-router', 'my-x-lib', 'my--lib',
  '.foo', '.hidden', '..', '.', '@scope/.foo', '@.scope/foo', '.@scope/foo', '@', '@/foo',
  '#foo', '!foo', '*', '?', '', 'foo/', '/foo', 'foo-', 'a{b', '{}', '{a,b', '[', ']', '(', ')',
  '|', '+', 'a+b', 'a|b', '$foo', '^foo', 'foo$', 'a b', 'x y', '\\', 'ü', 'Ü', 'üx', 'Üx', 'ß',
  'SS', 'İ', 'i', 'ix', 'k', 'K', 'K', 'z', '-', 'aaa', 'aab', 'foobarbaz', 'foo-a',
  'bar-x', 'test-pkg', 'local-pkg', 'private-x', '@angular/core', '@angular/CORE', '%40scope/x',
  'foo/bar', 'xbar', 'xyz', 'xz', '-1', '0', '10', '4', '7', 'x1', 'aC', 'a1', 'b2', '@a/c', '@b/d',
  'a)', '@(a', 'a|b|c', 'Z', '^', '_', '`', 'xay', 'xby', 'xcy', 'abd', 'acd', 'a-b', '{a}',
  'a{}b', 'foo.ts', 'foo.js.map', 'a/b/c/d', 'a/b/x/c', 'a/b/b/c', 'x/a/y', 'a/', '/a', 'a//b',
  '\u0130x', 'ix', '\u0131', '\u00e9', '\u00c9', '\u03c3', '\u03c2', '\u03a3', '\u017f', 's',
  '\uff21', 'a\tb', ' ', '\u00a0', '\u0660', '\u00bd',
];

const uniq = (xs) => [...new Set(xs)];
const P = uniq(patterns);
const N = uniq(names);
const results = P.map((p) =>
  N.map((n) => (minimatch(n, p, { nocase: true }) ? '1' : '0')).join('')
);
const doc = {
  generator: 'gen-minimatch.mjs: minimatch(name, pattern, { nocase: true })',
  minimatch: mmVersion,
  patterns: P,
  names: N,
  results,
};
fs.writeFileSync(out, JSON.stringify(doc, null, 1) + '\n');
console.log(`${P.length} patterns x ${N.length} names -> ${out}`);
