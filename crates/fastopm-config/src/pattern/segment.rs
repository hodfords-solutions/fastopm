//! One path portion of a glob, matched like `Matcher` from `@isaacs/nonrecursive-glob-match`
//! (the engine behind minimatch 11), always with `nocase`.
//!
//! The pattern is tokenized (literals, `?`, `*`, classes, extglobs `@( ) ?( ) *( ) +( ) !( )`),
//! compiled into a small graph, and run as an NFA over the chars of the portion. A negated
//! extglob `!(…)` keeps a sub-simulation per start position ("run") and lets the rest of the
//! pattern continue wherever the sub-pattern does not match the text consumed so far.
//!
//! Dot rules (`dot: false`): wildcards, classes and negations never match the first char of a
//! portion that starts with `.`, and a `*` cannot even match the empty string there. In the
//! portions `.` and `..`, they never match at all.

use std::collections::BTreeSet;

/// Simple case folding (what a JavaScript `iu` RegExp compares). `char::to_lowercase` is the
/// same for almost every char; the exceptions are the chars whose lower case form is a different
/// lower case letter after folding.
pub(super) fn fold(c: char) -> char {
    match c {
        'ς' => 'σ',
        'ϐ' => 'β',
        'ϑ' => 'θ',
        'ϕ' => 'φ',
        'ϖ' => 'π',
        'ϰ' => 'κ',
        'ϱ' => 'ρ',
        'ϵ' => 'ε',
        'ſ' => 's',
        '\u{0345}' | '\u{1FBE}' => 'ι',
        'ẛ' => 'ṡ',
        _ => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                // İ lower-cases to two chars and has no simple folding
                _ => c,
            }
        }
    }
}

/// Every char that folds to the same char as `c` (as far as a set membership test needs).
fn variants(c: char) -> Vec<char> {
    let f = fold(c);
    let mut out = vec![c];
    let mut push = |x: char| {
        if fold(x) == f && !out.contains(&x) {
            out.push(x);
        }
    };
    push(f);
    for base in [c, f] {
        let mut upper = base.to_uppercase();
        if let (Some(u), None) = (upper.next(), upper.next()) {
            push(u);
        }
    }
    let extra: &[char] = match f {
        'k' => &['\u{212A}'],
        's' => &['ſ'],
        'å' => &['\u{212B}'],
        'ß' => &['ẞ'],
        'σ' => &['ς'],
        'β' => &['ϐ'],
        'θ' => &['ϑ', 'ϴ'],
        'φ' => &['ϕ'],
        'π' => &['ϖ'],
        'κ' => &['ϰ'],
        'ρ' => &['ϱ'],
        'ε' => &['ϵ'],
        'ι' => &['\u{0345}', '\u{1FBE}'],
        'ṡ' => &['ẛ'],
        'ω' => &['\u{2126}'],
        _ => &[],
    };
    for &x in extra {
        push(x);
    }
    out
}

/// POSIX bracket classes, as minimatch translates them to Unicode properties. Outside ASCII
/// the properties are approximated with what `char` offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Posix {
    Alnum,
    Alpha,
    Ascii,
    Blank,
    Cntrl,
    Digit,
    /// `[:graph:]`: neither `\p{Z}` nor `\p{C}` (stored as the negated set `Z|C`).
    ZOrC,
    Lower,
    /// `[:print:]`: not `\p{C}` (stored as the negated set `C`).
    Other,
    Punct,
    Space,
    Upper,
    Word,
    Xdigit,
}

const POSIX_CLASSES: [(&str, Posix, bool); 14] = [
    ("[:alnum:]", Posix::Alnum, false),
    ("[:alpha:]", Posix::Alpha, false),
    ("[:ascii:]", Posix::Ascii, false),
    ("[:blank:]", Posix::Blank, false),
    ("[:cntrl:]", Posix::Cntrl, false),
    ("[:digit:]", Posix::Digit, false),
    ("[:graph:]", Posix::ZOrC, true),
    ("[:lower:]", Posix::Lower, false),
    ("[:print:]", Posix::Other, true),
    ("[:punct:]", Posix::Punct, false),
    ("[:space:]", Posix::Space, false),
    ("[:upper:]", Posix::Upper, false),
    ("[:word:]", Posix::Word, false),
    ("[:xdigit:]", Posix::Xdigit, false),
];

fn is_separator(c: char) -> bool {
    // \p{Z}: space separators plus U+2028 / U+2029
    (c.is_whitespace() && !c.is_control()) || c == '\u{2028}' || c == '\u{2029}'
}

fn is_other(c: char) -> bool {
    // \p{C}: controls, format chars, surrogates, private use, unassigned (approximated)
    c.is_control()
        || matches!(c, '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
        || matches!(c, '\u{E000}'..='\u{F8FF}')
}

fn is_letter(c: char) -> bool {
    // \p{L}\p{Nl}
    c.is_alphabetic()
}

fn is_decimal(c: char) -> bool {
    // \p{Nd}. Outside ASCII, decimal digits come in runs of ten numeric code points, while
    // most other numerics (No/Nl: ½, ², ...) do not.
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    if !c.is_numeric() {
        return false;
    }
    let cp = c as u32;
    (0..10).any(|k| {
        cp.checked_sub(k).is_some_and(|base| {
            (base..base + 10).all(|x| char::from_u32(x).is_some_and(char::is_numeric))
        })
    })
}

fn is_punct_or_symbol(c: char) -> bool {
    // \p{P}\p{S}
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        !c.is_alphanumeric() && !c.is_whitespace() && !is_other(c) && !is_mark(c)
    }
}

fn is_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

impl Posix {
    fn contains(self, c: char) -> bool {
        match self {
            Posix::Alnum => is_letter(c) || is_decimal(c),
            Posix::Alpha => is_letter(c),
            Posix::Ascii => c.is_ascii(),
            Posix::Blank => c == '\t' || (is_separator(c) && c != '\u{2028}' && c != '\u{2029}'),
            Posix::Cntrl => c.is_control(),
            Posix::Digit => is_decimal(c),
            Posix::ZOrC => is_separator(c) || is_other(c),
            Posix::Lower => c.is_lowercase(),
            Posix::Other => is_other(c),
            Posix::Punct => is_punct_or_symbol(c),
            Posix::Space => is_separator(c) || matches!(c, '\t' | '\r' | '\n' | '\u{B}' | '\u{C}'),
            Posix::Upper => c.is_uppercase(),
            Posix::Word => {
                is_letter(c)
                    || is_decimal(c)
                    || c == '_'
                    || matches!(
                        c,
                        '\u{203F}' | '\u{2040}' | '\u{2054}' | '\u{FE33}' | '\u{FE34}' | '\u{FE4D}'
                            ..='\u{FE4F}' | '\u{FF3F}'
                    )
            }
            Posix::Xdigit => c.is_ascii_hexdigit(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Member {
    Char(char),
    Range(char, char),
    Posix(Posix),
}

impl Member {
    fn contains(&self, c: char) -> bool {
        match *self {
            Member::Char(m) => m == c,
            Member::Range(lo, hi) => lo <= c && c <= hi,
            Member::Posix(p) => p.contains(c),
        }
    }
}

/// A bracket class (`parseClass`): `^([ranges]|[^negs])` with `iu` flags, where `!`/`^` at
/// the start flips both parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Class {
    /// `/$./`: an empty or invalid class that poisons the whole portion.
    never: bool,
    negate: bool,
    ranges: Vec<Member>,
    negs: Vec<Member>,
}

impl Class {
    fn never() -> Class {
        Class { never: true, negate: false, ranges: Vec::new(), negs: Vec::new() }
    }

    fn matches(&self, c: char) -> bool {
        if self.never {
            return false;
        }
        // with the `i` flag, a member matches when it folds to the same char as `c`
        let found = |set: &[Member]| variants(c).iter().any(|&v| set.iter().any(|m| m.contains(v)));
        let in_ranges = !self.ranges.is_empty() && (self.negate != found(&self.ranges));
        let in_negs = !self.negs.is_empty() && (self.negate == found(&self.negs));
        in_ranges || in_negs
    }
}

/// Parse the class starting at `chars[pos] == '['`. Returns the class and the number of chars
/// consumed; 0 means the class is not closed and the `[` is a literal.
fn parse_class(chars: &[char], pos: usize) -> (Class, usize) {
    let mut ranges = Vec::new();
    let mut negs = Vec::new();
    let mut i = pos + 1;
    let mut saw_start = false;
    let mut escaping = false;
    let mut negate = false;
    let mut end_pos = pos;
    let mut range_start: Option<char> = None;
    let starts_with =
        |at: usize, s: &str| s.chars().enumerate().all(|(k, sc)| chars.get(at + k) == Some(&sc));
    'outer: while i < chars.len() {
        let c = chars[i];
        if (c == '!' || c == '^') && i == pos + 1 {
            negate = true;
            i += 1;
            continue;
        }
        if c == ']' && saw_start && !escaping {
            end_pos = i + 1;
            break;
        }
        saw_start = true;
        if c == '\\' && !escaping {
            escaping = true;
            i += 1;
            continue;
        }
        if c == '[' && !escaping {
            for (name, posix, neg) in POSIX_CLASSES {
                if starts_with(i, name) {
                    if range_start.is_some() {
                        return (Class::never(), chars.len() - pos);
                    }
                    i += name.chars().count();
                    if neg {
                        negs.push(Member::Posix(posix));
                    } else {
                        ranges.push(Member::Posix(posix));
                    }
                    continue 'outer;
                }
            }
        }
        escaping = false;
        if let Some(start) = range_start.take() {
            if c > start {
                ranges.push(Member::Range(start, c));
            } else if c == start {
                ranges.push(Member::Char(c));
            }
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'-') && chars.get(i + 2) == Some(&']') {
            ranges.push(Member::Char(c));
            ranges.push(Member::Char('-'));
            i += 2;
            continue;
        }
        if chars.get(i + 1) == Some(&'-') {
            range_start = Some(c);
            i += 2;
            continue;
        }
        ranges.push(Member::Char(c));
        i += 1;
    }
    if end_pos < i {
        return (Class::never(), 0);
    }
    if ranges.is_empty() && negs.is_empty() {
        return (Class::never(), chars.len() - pos);
    }
    (Class { never: false, negate, ranges, negs }, end_pos - pos)
}

#[derive(Debug, Clone)]
enum Tok {
    Lit(char),
    Class(Class),
    Open(char),
    Bar,
    Close,
    Star,
    Any,
}

#[derive(Debug, Clone)]
enum Item {
    Lit(char),
    Any,
    Star,
    Class(Class),
    Ext(char, Vec<Vec<Item>>),
}

fn tokenize(chars: &[char], noext: bool) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            // a trailing \ is a literal \
            toks.push(Tok::Lit(chars.get(i + 1).copied().unwrap_or('\\')));
            i += 2;
            continue;
        }
        if c == '[' {
            let (class, consumed) = parse_class(chars, i);
            if consumed > 0 {
                toks.push(Tok::Class(class));
                i += consumed;
                continue;
            }
        }
        if !noext && "!?+*@".contains(c) && chars.get(i + 1) == Some(&'(') {
            toks.push(Tok::Open(c));
            i += 2;
            continue;
        }
        toks.push(match c {
            '|' if !noext => Tok::Bar,
            ')' if !noext => Tok::Close,
            '*' => Tok::Star,
            '?' => Tok::Any,
            _ => Tok::Lit(c),
        });
        i += 1;
    }
    toks
}

/// Pair each open token with its close token.
fn pair(toks: &[Tok]) -> Vec<Option<usize>> {
    let mut pairs = vec![None; toks.len()];
    let mut stack = Vec::new();
    for (i, tok) in toks.iter().enumerate() {
        match tok {
            Tok::Open(_) => stack.push(i),
            Tok::Close => {
                if let Some(open) = stack.pop() {
                    pairs[open] = Some(i);
                }
            }
            _ => {}
        }
    }
    pairs
}

fn build(
    toks: &[Tok],
    pairs: &[Option<usize>],
    start: usize,
    end: usize,
    top: bool,
) -> Vec<Vec<Item>> {
    let mut alts = vec![Vec::new()];
    let mut i = start;
    while i < end {
        if let (Tok::Open(ty), Some(close)) = (&toks[i], pairs[i]) {
            let sub = build(toks, pairs, i + 1, close, false);
            alts.last_mut().expect("alts is never empty").push(Item::Ext(*ty, sub));
            i = close + 1;
            continue;
        }
        let seq = alts.last_mut().expect("alts is never empty");
        match &toks[i] {
            Tok::Bar if !top => alts.push(Vec::new()),
            Tok::Open(ty) => {
                seq.push(Item::Lit(*ty));
                seq.push(Item::Lit('('));
            }
            Tok::Bar => seq.push(Item::Lit('|')),
            Tok::Close => seq.push(Item::Lit(')')),
            Tok::Lit(c) => seq.push(Item::Lit(*c)),
            Tok::Class(class) => seq.push(Item::Class(class.clone())),
            Tok::Star => seq.push(Item::Star),
            Tok::Any => seq.push(Item::Any),
        }
        i += 1;
    }
    alts
}

#[derive(Debug, Clone)]
enum Node {
    Match,
    Lit(char, usize),
    Any(usize),
    Star(usize),
    Class(Class, usize),
    Split(Vec<usize>),
    Neg { sub: usize, next: usize },
}

const MATCH: usize = 0;

struct Compiler {
    nodes: Vec<Node>,
}

impl Compiler {
    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn seq(&mut self, items: &[Item], next: usize) -> usize {
        items.iter().rev().fold(next, |next, it| self.item(it, next))
    }

    fn alts(&mut self, alts: &[Vec<Item>], next: usize) -> Vec<usize> {
        alts.iter().map(|a| self.seq(a, next)).collect()
    }

    fn item(&mut self, it: &Item, next: usize) -> usize {
        match it {
            Item::Lit(c) => self.push(Node::Lit(*c, next)),
            Item::Any => self.push(Node::Any(next)),
            Item::Star => self.push(Node::Star(next)),
            Item::Class(class) => self.push(Node::Class(class.clone(), next)),
            Item::Ext('@', alts) => {
                let outs = self.alts(alts, next);
                self.push(Node::Split(outs))
            }
            Item::Ext('?', alts) => {
                let mut outs = vec![next];
                outs.extend(self.alts(alts, next));
                self.push(Node::Split(outs))
            }
            Item::Ext('!', alts) => {
                let outs = self.alts(alts, MATCH);
                let sub = self.push(Node::Split(outs));
                self.push(Node::Neg { sub, next })
            }
            Item::Ext(ty, alts) => {
                // * and +: loop back after each match
                let after = self.push(Node::Split(Vec::new()));
                let outs = self.alts(alts, after);
                let entry = self.push(Node::Split(outs));
                self.nodes[after] = Node::Split(vec![next, entry]);
                if *ty == '*' { after } else { entry }
            }
        }
    }
}

/// The threads of the simulation at one position.
#[derive(Debug, Clone, Default)]
struct State {
    threads: BTreeSet<usize>,
    matched: bool,
    runs: Vec<Run>,
}

#[derive(Debug, Clone)]
struct Run {
    neg: usize,
    state: State,
}

impl State {
    fn dead(&self) -> bool {
        self.threads.is_empty() && !self.matched && self.runs.is_empty()
    }

    fn same(&self, other: &State) -> bool {
        self.threads == other.threads
            && self.matched == other.matched
            && self.runs.len() == other.runs.len()
            && self
                .runs
                .iter()
                .all(|r| other.runs.iter().any(|o| o.neg == r.neg && o.state.same(&r.state)))
    }
}

/// A state under construction; `visited` stops loops through splits and negations.
#[derive(Default)]
struct Builder {
    state: State,
    visited: BTreeSet<usize>,
}

struct Sim<'a> {
    nodes: &'a [Node],
    /// the portion is `.` or `..`: wildcards never match
    dots: bool,
    /// the portion starts with `.`: wildcards do not match its first char
    dot_start: bool,
}

impl Sim<'_> {
    fn restricted(&self, i: usize, top: bool) -> bool {
        top && (self.dots || (self.dot_start && i == 0))
    }

    fn add(&self, b: &mut Builder, node: usize, i: usize, top: bool) {
        match &self.nodes[node] {
            Node::Split(outs) => {
                if !b.visited.insert(node) {
                    return;
                }
                for &o in outs {
                    self.add(b, o, i, top);
                }
            }
            Node::Neg { sub, .. } => {
                if !b.visited.insert(node) {
                    return;
                }
                let mut sb = Builder::default();
                self.add(&mut sb, *sub, i, false);
                self.add_run(b, Run { neg: node, state: sb.state }, i, top);
            }
            Node::Star(next) => {
                // a * can not start a dotfile, even if it matches nothing
                if self.restricted(i, top) {
                    return;
                }
                if b.state.threads.insert(node) {
                    self.add(b, *next, i, top);
                }
            }
            Node::Match => b.state.matched = true,
            Node::Lit(..) | Node::Any(_) | Node::Class(..) => {
                b.state.threads.insert(node);
            }
        }
    }

    fn add_run(&self, b: &mut Builder, run: Run, i: usize, top: bool) {
        if b.state.runs.iter().any(|r| r.neg == run.neg && r.state.same(&run.state)) {
            return;
        }
        let matched = run.state.matched;
        let Node::Neg { next, .. } = self.nodes[run.neg] else {
            unreachable!("a run always starts at a negation node");
        };
        b.state.runs.push(run);
        if !matched {
            self.add(b, next, i, top);
        }
    }

    fn step(&self, state: &State, c: char, i: usize, top: bool) -> State {
        let mut b = Builder::default();
        let free = !self.restricted(i, top);
        for &th in &state.threads {
            match &self.nodes[th] {
                Node::Lit(ch, next) => {
                    if *ch == c || fold(*ch) == fold(c) {
                        self.add(&mut b, *next, i + 1, top);
                    }
                }
                Node::Any(next) if free => self.add(&mut b, *next, i + 1, top),
                Node::Class(class, next) if free && class.matches(c) => {
                    self.add(&mut b, *next, i + 1, top);
                }
                Node::Star(_) if free => self.add(&mut b, th, i + 1, top),
                _ => {}
            }
        }
        if free {
            for run in &state.runs {
                // a sub-pattern with no threads can not match again
                if run.state.dead() {
                    self.add_run(&mut b, run.clone(), i + 1, top);
                    continue;
                }
                let sub = self.step(&run.state, c, i, false);
                self.add_run(&mut b, Run { neg: run.neg, state: sub }, i + 1, top);
            }
        }
        b.state
    }
}

/// A compiled path portion.
#[derive(Debug, Clone)]
pub(super) struct Matcher {
    /// the pattern is only literal chars: compared with case folding
    literal: Option<Vec<char>>,
    nodes: Vec<Node>,
    start: usize,
}

impl Matcher {
    pub(super) fn new(pattern: &str) -> Matcher {
        let chars: Vec<char> = pattern.chars().collect();
        // `?()`, `*()`, `+()`, `@()` alone are literal
        let noext = matches!(chars.as_slice(), ['?' | '*' | '+' | '@', '(', ')']);
        let toks = tokenize(&chars, noext);
        let pairs = pair(&toks);
        let items = build(&toks, &pairs, 0, toks.len(), true).swap_remove(0);
        let literal = items
            .iter()
            .map(|it| match it {
                Item::Lit(c) => Some(*c),
                _ => None,
            })
            .collect::<Option<Vec<char>>>();
        let mut compiler = Compiler { nodes: vec![Node::Match] };
        let start = compiler.seq(&items, MATCH);
        Matcher { literal, nodes: compiler.nodes, start }
    }

    /// Whether the (non-empty, slash-free) portion `s` matches.
    pub(super) fn test(&self, s: &str) -> bool {
        if let Some(lit) = &self.literal {
            let mut it = s.chars();
            return lit.iter().all(|&l| it.next().is_some_and(|c| c == l || fold(c) == fold(l)))
                && it.next().is_none();
        }
        let dots = s == "." || s == "..";
        let sim = Sim { nodes: &self.nodes, dots, dot_start: !dots && s.starts_with('.') };
        let mut b = Builder::default();
        sim.add(&mut b, self.start, 0, true);
        let mut state = b.state;
        for (i, c) in s.chars().enumerate() {
            if state.dead() {
                return false;
            }
            state = sim.step(&state, c, i, true);
        }
        state.matched
    }
}

/// The index of the `]` closing the class at `start`, or `None` (`classEnd`).
fn class_end(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if matches!(chars.get(i), Some('!' | '^')) {
        i += 1;
    }
    let mut saw_start = false;
    while i < chars.len() {
        let c = chars[i];
        if c == ']' && saw_start {
            return Some(i);
        }
        saw_start = true;
        if c == '\\' {
            i += 1;
        }
        i += 1;
    }
    None
}

/// The string a portion matches when it has no magic, else `None` (minimatch `literal`).
pub(super) fn literal(pattern: &str) -> Option<String> {
    if pattern == "+()" || pattern == "@()" {
        return Some(pattern.to_owned());
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                i += 1;
                lit.push(chars.get(i).copied().unwrap_or('\\'));
            }
            '*' | '?' => return None,
            '[' => match class_end(&chars, i) {
                None => lit.push('['),
                Some(close) => {
                    let mut body = &chars[i + 1..close];
                    if body.first() == Some(&'\\') {
                        body = &body[1..];
                    }
                    if body.len() != 1 || matches!(chars.get(i + 1), Some('!' | '^')) {
                        return None;
                    }
                    lit.push(body[0]);
                    i = close;
                }
            },
            '+' | '@' | '!'
                if chars.get(i + 1) == Some(&'(')
                    && chars.get(i + 2..).is_some_and(|rest| rest.contains(&')')) =>
            {
                return None;
            }
            _ => lit.push(c),
        }
        i += 1;
    }
    Some(lit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool {
        Matcher::new(p).test(s)
    }

    #[test]
    fn wildcards_and_dots() {
        assert!(m("*", "foo"));
        assert!(!m("*", ".foo"));
        assert!(m(".*", ".foo"));
        assert!(!m(".*", "."));
        assert!(m("f?o", "FOO"));
        assert!(!m("[.]foo", ".foo"));
    }

    #[test]
    fn classes() {
        assert!(m("[a-c]x", "Bx"));
        assert!(m("[!a]x", "bx"));
        assert!(!m("[!a]x", "Ax"));
        assert!(m("[[:upper:]]", "a"));
        assert!(!m("[z-a]", "z"));
        assert!(m("[a-]", "-"));
    }

    #[test]
    fn extglobs() {
        assert!(m("@(foo|bar)", "BAR"));
        assert!(m("+(a|b)", "abba"));
        assert!(m("*(a|b)", ""));
        assert!(m("!(foo)", "bar"));
        assert!(!m("!(foo)", "foo"));
        assert!(m("!(foo)", "foobar"));
        assert!(m("?(foo)bar", "bar"));
        assert!(m("x!(y)z", "xz"));
        assert!(!m("x!(y)z", "xyz"));
    }

    #[test]
    fn folding() {
        assert!(m("k*", "\u{212A}x"));
        assert!(m("[k]", "\u{212A}"));
        assert!(m("σ", "ς"));
        assert!(!m("ß", "ss"));
    }

    #[test]
    fn literal_portions() {
        assert_eq!(literal("foo").as_deref(), Some("foo"));
        assert_eq!(literal("[*]").as_deref(), Some("*"));
        assert_eq!(literal("[.]").as_deref(), Some("."));
        assert_eq!(literal("a\\*").as_deref(), Some("a*"));
        assert_eq!(literal("+()").as_deref(), Some("+()"));
        assert_eq!(literal("[!]").as_deref(), Some("[!]"));
        assert_eq!(literal("@(a)"), None);
        assert_eq!(literal("*"), None);
    }
}
