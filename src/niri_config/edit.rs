//! The text engine of the niri config editor (ADR 0119): a setting at a dotted path is read,
//! set or removed by splicing replacement text into the file's own bytes at the spans the KDL
//! parser gives its nodes. Nothing outside the spliced range changes: comments, `/-` slashdash,
//! blank lines, line endings and a last line with no newline stay as they were. The `kdl` crate's
//! tree is only read, never edited, as that drops value edits and moves comments at the end of a
//! file.
//!
//! Pure: text in, text out. The file, `niri validate` and the backups are `niri_config`'s.
//!
//! Rules a reader of a result should know:
//! - KDL v1 only, as niri 26.04 reads; text written is v1 (`true`, `"quoted"`, `r#"raw"#`).
//! - A path names single-valued nodes: every name but the last is a block, the last a node with at
//!   most one value (`layout.gaps`) or a flag (`layout.focus-ring.off`). Repeated nodes
//!   (`window-rule`, `spawn-at-startup`) and nodes with an argument of their own (`output "x"`)
//!   are not addressable by a path. The one exception is the `output` block, which `apply_in`
//!   reaches by its place among the file's `output` nodes (`Block`), the path below it being a
//!   plain one; `niri_config` finds which block that is.
//! - A node may be set to properties (`position x=1 y=2`, `Value::Props`) when it has no arguments
//!   or block. A plain value never replaces a node that has properties.
//! - A node missing is created at the end of its parent block, which is created when missing, with
//!   the file's own indent and line ending. At the top level it goes before a closing
//!   `include "kanade.kdl"` (the appearance sync keeps that line last).
//! - Removing a node removes its own line (its text, a `;` and a trailing `//` comment). Comments
//!   on lines of their own are the user's and stay, even right above the node.
//! - A leading byte order mark is no part of the KDL and stays first, byte for byte.
//! - A node under `/-` is off; it is not edited, nor made again beside it.

use std::ops::Range;

use kdl::{KdlDocument, KdlNode, KdlValue};

/// Nodes niri takes more than once, which no single path can mean.
const REPEATED: &[&str] = &[
    "spawn-at-startup",
    "spawn-sh-at-startup",
    "window-rule",
    "layer-rule",
    "workspace",
    "output",
    "include",
];

/// Whether `path` is a repeated node without an index (like `"spawn-at-startup"`).
pub fn is_repeated_bare(path: &str) -> Result<bool, String> {
    if path.contains('.') || path.contains('[') {
        return Ok(false);
    }
    Ok(REPEATED.contains(&path))
}

/// An index picking an occurrence of a repeated node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    /// The nth occurrence from 0.
    Nth(usize),

    /// A new occurrence to be appended.
    New,
}

/// Parses a segment like `"spawn-at-startup[0]"` or `"window-rule[new]"` or `"layout"`.
/// Returns `(base_name, Option<Index>)`.
pub fn parse_segment(segment: &str) -> Result<(&str, Option<Index>), String> {
    if let Some((name, rest)) = segment.split_once('[') {
        let Some(index_str) = rest.strip_suffix(']') else {
            return Err(format!("{segment}: unclosed bracket in path segment"));
        };
        if name.is_empty() {
            return Err(format!("{segment}: missing node name before bracket"));
        }
        if !REPEATED.contains(&name) {
            return Err(format!("{name}: only repeated nodes take an index"));
        }
        if index_str == "new" {
            Ok((name, Some(Index::New)))
        } else if let Ok(n) = index_str.parse::<usize>() {
            Ok((name, Some(Index::Nth(n))))
        } else {
            Err(format!(
                "{segment}: invalid index, write it like [0] or [new]"
            ))
        }
    } else if segment.contains(']') {
        Err(format!(
            "{segment}: unexpected closing bracket in path segment"
        ))
    } else {
        Ok((segment, None))
    }
}

fn clean_name(segment: &str) -> &str {
    parse_segment(segment).map_or(segment, |(name, _)| name)
}

/// Nodes niri takes as flags, `off` and not `off true`; set `true` makes one, `false` removes it.
/// (The engine keeps to that; a caller that must keep a flag from being implied, as the page does
/// for `layout.border` whose bare block niri reads as on, sets `off` itself.)
const FLAGS: &[&str] = &[
    "off",
    "on",
    "prefer-no-csd",
    "numlock",
    "tap",
    "dwt",
    "dwtp",
    "drag",
    "drag-lock",
    "natural-scroll",
    "left-handed",
    "middle-emulation",
    "disabled-on-external-mouse",
    "disable-power-key-handling",
    "workspace-auto-back-and-forth",
    "warp-mouse-to-focus",
    "focus-follows-mouse",
    "always-center-single-column",
    "empty-workspace-above-first",
    "center-focused-column-never",
    "hide-when-typing",
    "skip-at-startup",
    "hide-not-bound",
    "disable-cursor-hiding",
    "skip-confirmation",
    "inactive-color-off",
    "focus-at-startup",
];

/// Whether niri takes the node as a flag (`off`, not `off true`).
pub fn is_flag(name: &str) -> bool {
    FLAGS.contains(&name)
}

/// A value niri's config takes in a single node.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),

    /// The properties of a node with no argument: `x=1 y=2`; none is the bare node.
    Props(Vec<(String, Value)>),
}

impl Value {
    /// Properties as typed on a command line: `x=1 y=2` (each value as `parse` reads it); an
    /// empty text is no property.
    pub fn parse_props(text: &str) -> Result<Value, String> {
        let mut props = Vec::new();

        for word in text.split_whitespace() {
            let (name, value) = word
                .split_once('=')
                .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                .ok_or_else(|| format!("{word}: properties look like x=1 y=2"))?;

            if !writable_name(name) {
                return Err(format!("{name}: not a property name Kanade writes"));
            }

            let value = Value::parse(value)?;

            if matches!(value, Value::Props(_)) || props.iter().any(|(had, _)| had == name) {
                return Err(format!("{word}: a property once, with a plain value"));
            }

            props.push((name.to_owned(), value));
        }

        Ok(Value::Props(props))
    }

    /// A value as typed on a command line: `true`/`false`, a number, else text. Text that looks
    /// like one of those is given in double quotes (`'"12"'`), which are not part of it.
    pub fn parse(text: &str) -> Result<Value, String> {
        if let Some(inner) = text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            return Ok(Value::Str(inner.to_owned()));
        }

        if text == "true" || text == "false" {
            return Ok(Value::Bool(text == "true"));
        }

        let digits = text.strip_prefix('-').unwrap_or(text);
        let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        let (whole, rest) = digits.split_once(['.', 'e', 'E']).unwrap_or((digits, ""));

        if numeric(digits) {
            return text
                .parse()
                .map(Value::Int)
                .map_err(|_| format!("{text}: a whole number out of range"));
        }

        // 1.5, 1e5, 0.25e-3: not inf, nan, a bare dot or a hexadecimal
        if numeric(whole)
            && !rest.is_empty()
            && rest.bytes().all(|b| b"0123456789.eE+-".contains(&b))
            && let Ok(number) = text.parse::<f64>()
            && number.is_finite()
        {
            return Ok(Value::Float(number));
        }

        Ok(Value::Str(text.to_owned()))
    }

    /// As KDL v1 writes it.
    pub fn v1(&self) -> String {
        match self {
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => format!("{value:?}"),
            Value::Str(text) => string(text),
            Value::Props(props) => props
                .iter()
                .map(|(name, value)| format!("{name}={}", value.v1()))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// As `get` shows it, for a message: a node with no entry at all is the bare node, `true`.
    pub fn shown(&self) -> String {
        match self {
            Value::Props(props) if props.is_empty() => String::from("true"),
            value => value.v1(),
        }
    }

    fn of(value: &KdlValue) -> Option<Value> {
        match value {
            KdlValue::Bool(value) => Some(Value::Bool(*value)),
            KdlValue::Integer(value) => i64::try_from(*value).ok().map(Value::Int),
            KdlValue::Float(value) => Some(Value::Float(*value)),
            KdlValue::String(text) => Some(Value::Str(text.clone())),
            KdlValue::Null => None,
        }
    }
}

// a string as v1 writes it: quoted, or raw when it holds a quote or a backslash
fn string(text: &str) -> String {
    let plain = !text.chars().any(char::is_control);

    if plain && text.contains(['"', '\\']) {
        // enough #s that no `"#...` inside ends it
        let mut hashes = 0;
        let mut run = 0;
        let mut after_quote = false;

        for char in text.chars() {
            match char {
                '"' => {
                    after_quote = true;
                    run = 0;
                }
                '#' if after_quote => {
                    run += 1;
                    hashes = hashes.max(run);
                }
                _ => after_quote = false,
            }
        }

        let hashes = "#".repeat(hashes + 1);

        return format!("r{hashes}\"{text}\"{hashes}");
    }

    let mut quoted = String::from('"');

    for char in text.chars() {
        match char {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            char if char.is_control() => quoted.push_str(&format!("\\u{{{:x}}}", char as u32)),
            char => quoted.push(char),
        }
    }

    quoted.push('"');
    quoted
}

// a value as `get` shows it
fn shown(value: &KdlValue) -> String {
    match Value::of(value) {
        Some(value) => value.v1(),
        None => match value {
            KdlValue::Integer(value) => value.to_string(),
            _ => String::from("null"),
        },
    }
}

/// What to do at a path.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Set(Value),
    Remove,

    /// Of a whole `output` block (`apply_in` with no path below it): remove it when nothing is
    /// left in it, else leave it.
    DropEmpty,
}

/// Which `output` block, by its place among the file's own: the block niri uses is found by
/// `niri_config`, which knows the monitors and the includes.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// The nth top-level `output` node of the file, from 0.
    Nth(usize),

    /// None yet: made, with this name, when something is set in it.
    New(String),
}

/// A top-level `output "name"` node of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputBlock {
    pub offset: usize,

    /// Its place among the file's `output` nodes.
    pub ordinal: usize,

    /// Its first argument; empty when it has none that is text.
    pub name: String,
}

/// A node found at a path.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Where it starts in its file, to order it among the includes there.
    pub offset: usize,

    /// Its values as `get` prints them: `true` for a flag, `{ ... }` for a block.
    pub shown: String,
}

// byte order marks open a file niri reads (the KDL parser skips more than one); they are no part of
// the KDL, so all of the leading run is split off, and an edit keeps it as it is
fn unbom(text: &str) -> (&str, &str) {
    text.split_at(text.len() - text.trim_start_matches('\u{feff}').len())
}

// the width of the character at `at`, to step over it and never into it
fn step(text: &str, at: usize) -> usize {
    at + text[at..].chars().next().map_or(1, char::len_utf8)
}

/// The document of a v1 file, or the line and reason it is not one.
pub fn parse(text: &str) -> Result<KdlDocument, String> {
    let text = unbom(text).1;

    KdlDocument::parse_v1(text).map_err(|error| {
        let Some(first) = error.diagnostics.first() else {
            return String::from("not valid KDL v1");
        };
        let at = first.span.offset().min(text.len());
        let line = text.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        let what = first
            .message
            .as_deref()
            .or(first.label.as_deref())
            .unwrap_or("not valid KDL v1");

        format!("line {line}: {what} (KDL v1; niri 26.04 does not read v2)")
    })
}

/// The path's words, each a plain node name.
pub fn segments(path: &str) -> Result<Vec<&str>, String> {
    let segments: Vec<&str> = path.split('.').collect();

    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "{path}: a dotted path of node names, like layout.gaps"
        ));
    }

    for segment in &segments {
        parse_segment(segment)?;
    }

    Ok(segments)
}

/// Whether `char` is a bidirectional control character (Unicode Bidi_Control), which could make a
/// name read as another one: it is refused wherever a name is written or typed.
pub fn bidi(char: char) -> bool {
    matches!(
        char,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

// a name Kanade writes bare
fn writable_name(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+'))
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

// what a path can reach, where it is not
fn addressable(segments: &[&str], block: bool) -> Result<(), String> {
    for segment in segments.iter().skip(usize::from(block)) {
        let (name, index) = parse_segment(segment)?;
        if REPEATED.contains(&name) && index.is_none() {
            return Err(format!(
                "{segment}: niri takes it more than once, which a path cannot pick one of"
            ));
        }
    }

    Ok(())
}

// where a node sits in the text, with what an edit needs of it
#[derive(Debug, Clone)]
struct Leaf {
    span: Range<usize>,
    name_end: usize,
    arguments: Vec<Range<usize>>,
    values: Vec<KdlValue>,
    props: Vec<(String, KdlValue)>,

    // where each entry sits, arguments and properties in file order
    entries: Vec<Range<usize>>,

    // where the last entry (argument or property) ends
    entries_end: Option<usize>,
    block: bool,
}

// the bytes of a span, which is miette's type
macro_rules! span {
    ($span:expr) => {{
        let span = $span;

        span.offset()..span.offset() + span.len()
    }};
}

impl Leaf {
    fn of(node: &KdlNode) -> Leaf {
        let positional: Vec<_> = node
            .entries()
            .iter()
            .filter(|entry| entry.name().is_none())
            .collect();

        Leaf {
            span: span!(node.span()),
            name_end: span!(node.name().span()).end,
            arguments: positional.iter().map(|entry| span!(entry.span())).collect(),
            values: positional
                .iter()
                .map(|entry| entry.value().clone())
                .collect(),
            props: node
                .entries()
                .iter()
                .filter_map(|entry| Some((entry.name()?.value().to_owned(), entry.value().clone())))
                .collect(),
            entries: node
                .entries()
                .iter()
                .map(|entry| span!(entry.span()))
                .collect(),
            entries_end: node
                .entries()
                .iter()
                .map(|entry| span!(entry.span()).end)
                .max(),
            block: node.children().is_some(),
        }
    }

    fn shown(&self) -> String {
        if self.block {
            return String::from("{ ... }");
        }

        if self.values.is_empty() && self.props.is_empty() {
            return String::from("true");
        }

        let props = self
            .props
            .iter()
            .map(|(name, value)| format!("{name}={}", shown(value)));

        self.values
            .iter()
            .map(shown)
            .chain(props)
            .collect::<Vec<_>>()
            .join(" ")
    }

    // the node's properties, each from its name to the end of its value, for a change that keeps
    // the spacing and the comments around it: an argument is no property
    fn prop_spans(&self) -> Vec<Range<usize>> {
        self.entries
            .iter()
            .filter(|entry| !self.arguments.iter().any(|argument| argument == *entry))
            .cloned()
            .collect()
    }
}

// a block, or the file, that the rest of a path can be made in
#[derive(Debug, Clone)]
enum Parent {
    File,
    Block {
        // the node, from its name to its closing brace
        node: Range<usize>,
        // between the braces
        inner: Range<usize>,
        // the indent of its first child, when that stands first on its line
        child_indent: Option<String>,
        // the one its own line has
        indent: String,
        // what its indent adds to its parent's, when it adds anything: this block's own step
        step: Option<String>,
    },
}

#[derive(Debug)]
struct Search {
    leaves: Vec<Leaf>,

    // the deepest block the path's first words lead to: how many words, where, and whether the
    // next word is the name of a node under `/-` there
    deepest: (usize, Parent, bool),
}

// the recursion carries the text, its tree, the scope, the parent, the path, the block picked and
// the depth with what it fills
#[allow(clippy::too_many_arguments)]
fn search(
    text: &str,
    document: &KdlDocument,
    scope: Range<usize>,
    parent: Parent,
    segments: &[&str],
    pick: Option<&Block>,
    depth: usize,
    found: &mut Search,
) -> Result<(), String> {
    let (want, want_index) = parse_segment(segments[depth])?;
    let nodes = document.nodes();
    let disabled = if matches!(want_index, Some(Index::New)) {
        false
    } else {
        slashdashed(text, nodes, &scope)
            .iter()
            .any(|(name, argument)| {
                name == want
                    && match (depth, pick) {
                        // a block under `/-` is no block for another name
                        (0, Some(Block::New(new))) => argument.as_deref() == Some(new),
                        (0, Some(Block::Nth(_))) => false,
                        _ => true,
                    }
            })
    };
    let mut ordinal = 0;

    if depth >= found.deepest.0 {
        found.deepest = (depth, parent, disabled);
    }

    for node in nodes.iter().filter(|node| node.name().value() == want) {
        // the block a caller picked, by its place among the file's own; a block still to make is none
        if let (0, Some(block)) = (depth, pick) {
            ordinal += 1;

            if *block != Block::Nth(ordinal - 1) {
                continue;
            }
        }

        let this_ordinal = ordinal;
        ordinal += 1;

        if pick.is_none() {
            match want_index {
                Some(Index::Nth(n)) if this_ordinal != n => continue,
                Some(Index::New) => continue,
                _ => {}
            }
        }

        if depth + 1 == segments.len() {
            found.leaves.push(Leaf::of(node));
            continue;
        }

        let reach = segments[..=depth].join(".");

        let picked = depth == 0 && pick.is_some();

        if !picked && node.entries().iter().any(|entry| entry.name().is_none()) {
            return Err(format!(
                "{reach}: has an argument of its own, which a path cannot name"
            ));
        }

        let Some(children) = node.children() else {
            return Err(format!("{reach}: is not a block"));
        };
        let span = span!(node.span());
        // a block's own span stops after its last node, and an empty one has none, so its
        // braces are found in the node: what a `/-` after the last node is in, and what is empty
        let close = span.end.saturating_sub(1);
        let open = text[span.start..close]
            .rfind('{')
            .filter(|_| children.nodes().is_empty())
            .map_or_else(|| span!(children.span()).start, |at| span.start + at + 1);
        let inner = open..close;
        let first = children.nodes().first().and_then(|first| {
            let start = span!(first.span()).start;
            let line = text[..start].rfind('\n').map_or(0, |at| at + 1);
            let indent = &text[line..start];

            indent.trim().is_empty().then(|| indent.to_owned())
        });

        search(
            text,
            children,
            inner.clone(),
            Parent::Block {
                node: span.clone(),
                inner,
                child_indent: first.clone(),
                indent: indent_at(text, span.start).to_owned(),
                step: first.and_then(|child| {
                    let outer = indent_at(text, span.start);

                    child
                        .strip_prefix(outer)
                        .filter(|step| !step.is_empty())
                        .map(str::to_owned)
                }),
            },
            segments,
            pick,
            depth + 1,
            found,
        )?;
    }

    Ok(())
}

fn find(
    text: &str,
    document: &KdlDocument,
    segments: &[&str],
    pick: Option<&Block>,
) -> Result<Search, String> {
    let mut found = Search {
        leaves: Vec::new(),
        deepest: (0, Parent::File, false),
    };

    search(
        text,
        document,
        0..text.len(),
        Parent::File,
        segments,
        pick,
        0,
        &mut found,
    )?;

    Ok(found)
}

/// The nodes at `segments` in `text`, in file order.
pub fn hits(text: &str, segments: &[&str]) -> Result<Vec<Hit>, String> {
    let text = unbom(text).1;
    let document = parse(text)?;

    Ok(find(text, &document, segments, None)?
        .leaves
        .iter()
        .map(|leaf| Hit {
            offset: leaf.span.start,
            shown: leaf.shown(),
        })
        .collect())
}

/// The nodes at `below` in the nth top-level `output` block of `text`, in file order.
pub fn hits_in(text: &str, ordinal: usize, below: &[&str]) -> Result<Vec<Hit>, String> {
    let text = unbom(text).1;
    let document = parse(text)?;
    let mut segments = vec!["output"];

    segments.extend_from_slice(below);

    Ok(
        find(text, &document, &segments, Some(&Block::Nth(ordinal)))?
            .leaves
            .iter()
            .map(|leaf| Hit {
                offset: leaf.span.start,
                shown: leaf.shown(),
            })
            .collect(),
    )
}

/// The top-level `output` nodes of a file, in file order (those under `/-` are not nodes).
pub fn outputs(text: &str) -> Result<Vec<OutputBlock>, String> {
    let text = unbom(text).1;
    let document = parse(text)?;

    Ok(document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "output")
        .enumerate()
        .map(|(ordinal, node)| OutputBlock {
            offset: span!(node.span()).start,
            ordinal,
            name: node
                .get(0)
                .and_then(KdlValue::as_string)
                .unwrap_or_default()
                .to_owned(),
        })
        .collect())
}

/// The file's own text of the nth top-level `output` block.
pub fn block_text(text: &str, ordinal: usize) -> Result<String, String> {
    let text = unbom(text).1;
    let document = parse(text)?;
    let node = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "output")
        .nth(ordinal)
        .ok_or("no such output block")?;

    Ok(text[span!(node.span())].to_owned())
}

/// The top-level `include` nodes of a file: where each starts, its path, whether it is optional.
pub fn includes(text: &str) -> Result<Vec<(usize, String, bool)>, String> {
    let text = unbom(text).1;
    let document = parse(text)?;

    Ok(document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "include")
        .filter_map(|node| {
            let file = node.get(0)?.as_string()?.to_owned();
            let optional = node
                .get("optional")
                .and_then(KdlValue::as_bool)
                .unwrap_or(false);

            Some((span!(node.span()).start, file, optional))
        })
        .collect())
}

// the names of the nodes in the gaps between `nodes` of a scope that a `/-` turns off
fn slashdashed(
    text: &str,
    nodes: &[KdlNode],
    scope: &Range<usize>,
) -> Vec<(String, Option<String>)> {
    let mut names = Vec::new();
    let mut cursor = scope.start;

    for node in nodes {
        let span = span!(node.span());

        names.extend(slashdashed_pairs(&text[cursor..span.start]));
        cursor = span.end;
    }

    names.extend(slashdashed_pairs(&text[cursor..scope.end]));
    names
}

// the names after a `/-` in text of only trivia and nodes under `/-`
#[cfg(test)]
fn slashdashed_in(gap: &str) -> Vec<String> {
    slashdashed_pairs(gap)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

// the same with each node's first argument, when that is a string (`/-output "DP-1" { ... }`)
fn slashdashed_pairs(gap: &str) -> Vec<(String, Option<String>)> {
    let bytes = gap.as_bytes();
    let mut names = Vec::new();
    let mut at = 0;

    while at < bytes.len() {
        if let Some(end) = trivia(gap, at) {
            at = end;
        } else if gap[at..].starts_with("/-") {
            at = trivia_run(gap, at + 2);

            let (name, end) = name_at(gap, at);
            let argument = gap[end..].trim_start_matches([' ', '\t']);
            let from = gap.len() - argument.len();

            names.push((
                name,
                quoted(gap, from).and_then(|to| decode(&gap[from..to])),
            ));
            at = skip_node(gap, end);
        } else {
            at = step(gap, at);
        }
    }

    names
}

// the end of a `//` or nested `/* */` comment starting at `at`
fn trivia(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at..];

    if rest.starts_with("//") {
        return Some(rest.find('\n').map_or(text.len(), |end| at + end));
    }

    if rest.starts_with("/*") {
        let mut depth = 0;
        let mut index = at;

        while index < text.len() {
            if text[index..].starts_with("/*") {
                depth += 1;
                index += 2;
            } else if text[index..].starts_with("*/") {
                depth -= 1;
                index += 2;

                if depth == 0 {
                    return Some(index);
                }
            } else {
                index = step(text, index);
            }
        }

        return Some(text.len());
    }

    None
}

// past whitespace and comments
fn trivia_run(text: &str, mut at: usize) -> usize {
    loop {
        if let Some(end) = trivia(text, at) {
            at = end;
        } else if text[at..].starts_with(char::is_whitespace) {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        } else {
            return at;
        }
    }
}

// the end of a quoted or raw string starting at `at`, none when it is neither
fn quoted(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at..];

    if rest.starts_with('"') {
        let mut escaped = false;

        for (index, char) in rest.char_indices().skip(1) {
            match char {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => return Some(at + index + 1),
                _ => {}
            }
        }

        return Some(text.len());
    }

    if let Some(after) = rest.strip_prefix('r') {
        let hashes = after.len() - after.trim_start_matches('#').len();

        if after[hashes..].starts_with('"') {
            let close = format!("\"{}", "#".repeat(hashes));
            let from = at + 1 + hashes + 1;

            return Some(
                text[from..]
                    .find(&close)
                    .map_or(text.len(), |end| from + end + close.len()),
            );
        }
    }

    None
}

// the text of a quoted or raw string as written, none when it is neither or is not closed
fn decode(raw: &str) -> Option<String> {
    if let Some(after) = raw.strip_prefix('r') {
        let hashes = after.len() - after.trim_start_matches('#').len();
        let body = after[hashes..].strip_prefix('"')?;
        let close = format!("\"{}", "#".repeat(hashes));

        return body.strip_suffix(close.as_str()).map(str::to_owned);
    }

    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::new();
    let mut chars = inner.chars();

    while let Some(char) = chars.next() {
        if char != '\\' {
            text.push(char);
            continue;
        }

        text.push(match chars.next()? {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            '/' => '/',
            escaped @ ('\\' | '"') => escaped,
            'u' => {
                let hex: String = chars
                    .by_ref()
                    .skip_while(|&c| c == '{')
                    .take_while(|&c| c != '}')
                    .collect();

                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            _ => return None,
        });
    }

    Some(text)
}

// the node name at `at` and where it ends
fn name_at(text: &str, at: usize) -> (String, usize) {
    if let Some(end) = quoted(text, at) {
        let raw = &text[at..end];
        let name = raw
            .trim_start_matches('r')
            .trim_matches('#')
            .trim_matches('"')
            .to_owned();

        return (name, end);
    }

    let end = text[at..]
        .find(|c: char| c.is_whitespace() || "\\/(){}<>;[]=,\"".contains(c))
        .map_or(text.len(), |end| at + end);

    (text[at..end].to_owned(), end)
}

// past the rest of a node under `/-`: to its `;` or line end, or the end of its block
fn skip_node(text: &str, mut at: usize) -> usize {
    let mut depth = 0;

    while at < text.len() {
        if depth == 0 && text[at..].starts_with("//") {
            // a comment ends the node's line, and a quote in it opens no string
            return text[at..].find('\n').map_or(text.len(), |end| at + end);
        }

        if let Some(end) = trivia(text, at) {
            at = end;
        } else if let Some(end) = quoted(text, at) {
            at = end;
        } else {
            match text.as_bytes()[at] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;

                    if depth <= 0 {
                        return at + 1;
                    }
                }
                b';' | b'\n' if depth == 0 => return at,
                b'\\'
                    if text[at + 1..]
                        .trim_start_matches([' ', '\t', '\r'])
                        .starts_with('\n') =>
                {
                    // a line continued
                    at += 1 + text[at + 1..].find('\n').unwrap_or(0);
                }
                _ => {}
            }

            at = step(text, at);
        }
    }

    at
}

// the line ending the file uses
fn eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;

    if crlf > lf { "\r\n" } else { "\n" }
}

// the blanks a line starts with, at `at` in it
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |end| end + 1);
    let blanks = text[line..].len() - text[line..].trim_start_matches([' ', '\t']).len();

    &text[line..line + blanks]
}

// `outer` with one more level under it: the step its own children take, or the file's, so a file
// mixing tabs and spaces gets no indent of either kind in the middle of the other
fn deeper(outer: &str, step: &str) -> String {
    format!("{outer}{step}")
}

// what one level of nesting is in this file, from the first block that has a child on its own line
fn unit(text: &str, document: &KdlDocument) -> String {
    for node in document.nodes() {
        let Some(first) = node
            .children()
            .and_then(|children| children.nodes().first())
        else {
            continue;
        };
        let child = span!(first.span()).start;
        let line = text[..child].rfind('\n').map_or(0, |end| end + 1);

        if !text[line..child].trim().is_empty() {
            continue;
        }

        let outer = indent_at(text, span!(node.span()).start);
        let inner = &text[line..child];

        if let Some(unit) = inner.strip_prefix(outer).filter(|unit| !unit.is_empty()) {
            return unit.to_owned();
        }
    }

    String::from("    ")
}

// the node a flag or a value makes, none for a flag that is off
fn node_text(name: &str, value: &Value) -> Option<String> {
    if name == "window-rule" || name == "layer-rule" {
        return match value {
            Value::Str(s) if s.is_empty() || s == "{}" => Some(format!("{name} {{\n}}")),
            Value::Str(s) => {
                let s = s.trim();
                if s.starts_with('{') && s.ends_with('}') {
                    Some(format!("{name} {s}"))
                } else {
                    Some(format!("{name} {{\n    {s}\n}}"))
                }
            }
            Value::Props(p) if p.is_empty() => Some(format!("{name} {{\n}}")),
            _ => Some(format!("{name} {{\n}}")),
        };
    }

    match value {
        Value::Props(props) if props.is_empty() => Some(name.to_owned()),
        Value::Bool(true) if FLAGS.contains(&name) => Some(name.to_owned()),
        Value::Bool(false) if FLAGS.contains(&name) => None,
        value => Some(format!("{name} {}", value.v1())),
    }
}

// `leaf` under the blocks `segments` name, one to a line
fn chain(segments: &[String], leaf: &str, indent: &str, unit: &str, eol: &str) -> String {
    match segments {
        [_] => format!("{indent}{leaf}{eol}"),
        [name, rest @ ..] => {
            let inner = format!("{indent}{unit}");

            format!(
                "{indent}{name} {{{eol}{}{indent}}}{eol}",
                chain(rest, leaf, &inner, unit, eol)
            )
        }
        [] => String::new(),
    }
}

// the same on a line, inside a block that is on one
fn inline(segments: &[String], leaf: &str) -> String {
    match segments {
        [_] => format!("{leaf};"),
        [name, rest @ ..] => format!("{name} {{ {} }};", inline(rest, leaf)),
        [] => String::new(),
    }
}

type Splice = (Range<usize>, String);

// where `leaf` and the blocks above it go, as a splice
fn create(
    text: &str,
    document: &KdlDocument,
    found: &Search,
    segments: &[&str],
    head: Option<&str>,
    leaf: &str,
) -> Result<Splice, String> {
    let (depth, parent, disabled) = &found.deepest;
    let mut missing: Vec<String> = segments[*depth..]
        .iter()
        .map(|&s| clean_name(s).to_owned())
        .collect();

    // the first block, when it is made, is named: `output "DP-1"`
    if let (0, Some(head)) = (*depth, head) {
        missing[0] = head.to_owned();
    }

    let missing = &missing[..];
    let unit = unit(text, document);
    let eol = eol(text);

    if *disabled {
        return Err(format!(
            "{}: turned off with /- in the file; take the /- off to use it",
            segments[..=*depth].join(".")
        ));
    }

    match parent {
        Parent::File => {
            let first_segment = clean_name(segments[*depth]);

            // If a node of this kind already exists in the file, place directly below the last one
            let last_of_kind =
                if head.is_none() && first_segment != "output" && REPEATED.contains(&first_segment)
                {
                    document
                        .nodes()
                        .iter()
                        .rfind(|node| node.name().value() == first_segment)
                } else {
                    None
                };

            if let Some(last) = last_of_kind {
                let span = span!(last.span());
                let line_end = text[span.end..]
                    .find('\n')
                    .map_or(text.len(), |p| span.end + p + 1);
                let line_start = text[..span.start].rfind('\n').map_or(0, |p| p + 1);
                let indent = &text[line_start..span.start];
                let body = chain(missing, leaf, indent, &unit, eol);
                let lead = if text[..line_end].ends_with('\n') || line_end == 0 {
                    ""
                } else {
                    eol
                };

                return Ok((line_end..line_end, format!("{lead}{body}")));
            }

            // before the appearance sync's include when that ends the file
            let sync = document.nodes().last().filter(|node| {
                node.name().value() == "include"
                    && node.get(0).and_then(KdlValue::as_string) == Some("kanade.kdl")
                    && text[..span!(node.span()).start]
                        .trim_end_matches([' ', '\t'])
                        .ends_with('\n')
            });
            let body = chain(missing, leaf, "", &unit, eol);

            if let Some(sync) = sync {
                let at = span!(sync.span()).start;
                // the include's own line, which may be indented, and the blank line before it
                let line = text[..at].rfind('\n').map_or(0, |end| end + 1);
                let blanks = &text[line..at];
                let before = if blanks.trim().is_empty() { line } else { at };
                let spaced = text[..before].ends_with("\n\n") || text[..before].ends_with("\n\r\n");
                let lead = if spaced || before == 0 { "" } else { eol };

                return Ok((before..before, format!("{lead}{body}{eol}")));
            }

            let lead = if text.is_empty() || text.ends_with("\n\n") || text.ends_with("\n\r\n") {
                String::new()
            } else if text.ends_with('\n') {
                eol.to_owned()
            } else {
                format!("{eol}{eol}")
            };

            Ok((text.len()..text.len(), format!("{lead}{body}")))
        }
        Parent::Block {
            node,
            inner,
            child_indent,
            indent: outer,
            step,
        } => {
            let close = node.end.saturating_sub(1);

            if text.as_bytes().get(close) != Some(&b'}') {
                return Err(String::from(
                    "a block's closing brace was not where it should be",
                ));
            }

            let line = text[..close].rfind('\n').map_or(0, |end| end + 1);
            let own_line = line > node.start && text[line..close].trim().is_empty();
            // the step its own children take, else the file's
            let step = step.as_deref().unwrap_or(&unit);

            if own_line {
                let indent = child_indent.clone().unwrap_or_else(|| deeper(outer, step));

                return Ok((line..line, chain(missing, leaf, &indent, step, eol)));
            }

            if text[inner.clone()].trim().is_empty() {
                let indent = deeper(outer, step);
                let body = chain(missing, leaf, &indent, step, eol);

                return Ok((inner.clone(), format!("{eol}{body}{outer}")));
            }

            let end = inner.start + text[inner.clone()].trim_end().len();
            let lead = if text[..end].ends_with([';', '}']) {
                ""
            } else {
                ";"
            };

            Ok((end..end, format!("{lead} {}", inline(missing, leaf))))
        }
    }
}

// the splice that removes a node's own line, else only the node
fn remove(text: &str, leaf: &Leaf) -> Splice {
    let line = text[..leaf.span.start].rfind('\n').map_or(0, |end| end + 1);
    let end_of_line = text[leaf.span.end..]
        .find('\n')
        .map_or(text.len(), |end| leaf.span.end + end);
    let before = &text[line..leaf.span.start];
    let after = text[leaf.span.end..end_of_line].trim();
    let after = after.strip_prefix(';').unwrap_or(after).trim_start();

    if before.trim().is_empty() && (after.is_empty() || after.starts_with("//")) {
        return (line..(end_of_line + 1).min(text.len()), String::new());
    }

    // among others on its line: the node, its `;` and the blanks after
    let rest = &text[leaf.span.end..end_of_line];
    let mut end = leaf.span.end;
    let trimmed = rest.trim_start_matches([' ', '\t']);

    if let Some(after_semi) = trimmed.strip_prefix(';') {
        end = end_of_line - after_semi.len();

        let blanks = after_semi.len() - after_semi.trim_start_matches([' ', '\t']).len();

        end += blanks;
    }

    (leaf.span.start..end, String::new())
}

fn splice(text: &str, mut splices: Vec<Splice>) -> String {
    splices.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));

    let mut text = text.to_owned();

    for (range, with) in splices {
        text.replace_range(range, &with);
    }

    text
}

// flatten nodes except `skip`, omitting empty ancestor blocks; `gone` removes one indexed output
// before later outputs are counted
fn flat(document: &KdlDocument, skip: &[String], gone: Option<(&str, usize)>) -> Vec<String> {
    fn walk(
        nodes: &[KdlNode],
        path: &mut Vec<String>,
        skip: &[String],
        gone: Option<(&str, usize)>,
        out: &mut Vec<String>,
    ) {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for node in nodes {
            let mut name = node.name().value().to_owned();

            if path.is_empty() && REPEATED.contains(&name.as_str()) {
                let place = *counts.entry(name.clone()).or_insert(0);
                counts.insert(name.clone(), place + 1);

                if name == "output" {
                    let place = match gone {
                        Some(("output", g)) if place == g => continue,
                        Some(("output", g)) if place > g => place - 1,
                        _ => place,
                    };

                    name = format!("output#{place}");
                } else {
                    let place = match gone {
                        Some((k, g)) if k == name && place == g => continue,
                        Some((k, g)) if k == name && place > g => place - 1,
                        _ => place,
                    };

                    name = format!("{name}[{place}]");
                }
            }

            path.push(name);

            if path.as_slice() == skip {
                path.pop();
                continue;
            }

            let entries: Vec<String> = node
                .entries()
                .iter()
                .map(|entry| {
                    let name = entry
                        .name()
                        .map(|name| format!("{}=", name.value()))
                        .unwrap_or_default();

                    format!("{name}{}", shown(entry.value()))
                })
                .collect();
            let line = format!("{} {}", path.join("."), entries.join(" "));
            let on_the_way = path.len() < skip.len() && skip[..path.len()] == path[..];

            match node.children() {
                Some(children) => {
                    let mark = out.len();

                    out.push(format!("{line} {{"));
                    walk(children.nodes(), path, skip, gone, out);

                    if on_the_way && out.len() == mark + 1 {
                        out.pop();
                    }
                }
                None => out.push(line),
            }

            path.pop();
        }
    }

    let mut out = Vec::new();

    walk(document.nodes(), &mut Vec::new(), skip, gone, &mut out);
    out
}

/// `text` with `op` done at `segments`, none when it already is as asked. Says why when it
/// cannot, or when the result is not the old file with only that node changed.
pub fn apply(text: &str, segments: &[&str], op: &Op) -> Result<Option<String>, String> {
    apply_at(text, None, segments, op)
}

/// The same inside an `output` block, `below` being the path under it (`scale`, `layout.gaps`),
/// or nothing for `Op::DropEmpty`. A `Block::New` is made at the end of the file when something
/// is set in it.
pub fn apply_in(
    text: &str,
    block: &Block,
    below: &[&str],
    op: &Op,
) -> Result<Option<String>, String> {
    if matches!(op, Op::DropEmpty) {
        return match (block, below) {
            (Block::Nth(ordinal), []) => drop_empty(text, *ordinal),
            _ => Err(String::from(
                "only a whole output block already in the file can be dropped",
            )),
        };
    }

    if below.is_empty() {
        return Err(String::from("output: name what to set in the block"));
    }

    if matches!(block, Block::New(name) if name.is_empty()) {
        return Err(String::from("output: a block needs a name"));
    }

    if let Block::New(name) = block
        && let Some(char) = name.chars().find(|&char| bidi(char))
    {
        return Err(format!(
            "output: the name holds U+{:04X}, a bidirectional control character that could make it \
             read as another monitor's; not written",
            char as u32
        ));
    }

    let mut segments = vec!["output"];

    segments.extend_from_slice(below);
    apply_at(text, Some(block), &segments, op)
}

// where the empty line that ends `before` starts, if it does (`before` ends at a line's start)
fn blank_line_before(before: &str) -> Option<usize> {
    let line = before.strip_suffix('\n')?;
    let line = line.strip_suffix('\r').unwrap_or(line);

    (line.is_empty() || line.ends_with('\n')).then_some(line.len())
}

// the nth `output` block removed when nothing is in it (no node, no comment, no other entry)
fn drop_empty(text: &str, ordinal: usize) -> Result<Option<String>, String> {
    let (bom, text) = unbom(text);
    let document = parse(text)?;
    let node = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "output")
        .nth(ordinal)
        .ok_or("output: no such block in the file")?;
    let leaf = Leaf::of(node);
    let named_only = leaf.values.len() == 1 && leaf.props.is_empty();
    let tail = leaf
        .entries_end
        .map_or("", |end| text[end..leaf.span.end].trim());
    let bare = tail.is_empty()
        || tail
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
            .is_some_and(|inside| inside.trim().is_empty());

    // a block with a setting of the user's, or only a comment, stays
    if !named_only || !bare {
        return Ok(None);
    }

    // the blank line a block was set off by goes with it, unless the lines after it need it
    let (mut range, with) = remove(text, &leaf);

    if let Some(start) = blank_line_before(&text[..range.start])
        && {
            let after = &text[range.end..];

            after.trim().is_empty() || after.starts_with('\n') || after.starts_with("\r\n")
        }
    {
        range.start = start;
    }

    let edited = splice(text, vec![(range, with)]);
    let after = parse(&edited)
        .map_err(|why| format!("the edit made invalid KDL ({why}); nothing written"))?;
    let count = |document: &KdlDocument| {
        document
            .nodes()
            .iter()
            .filter(|node| node.name().value() == "output")
            .count()
    };

    if flat(&document, &[], Some(("output", ordinal))) != flat(&after, &[], None)
        || count(&after) + 1 != count(&document)
    {
        return Err(String::from(
            "the edit changed more than the block; nothing written",
        ));
    }

    Ok(Some(format!("{bom}{edited}")))
}

fn apply_at(
    text: &str,
    block: Option<&Block>,
    segments: &[&str],
    op: &Op,
) -> Result<Option<String>, String> {
    let (bom, text) = unbom(text);

    addressable(segments, block.is_some())?;

    if matches!(op, Op::Remove | Op::DropEmpty)
        && segments
            .iter()
            .any(|s| parse_segment(s).ok().and_then(|(_, idx)| idx) == Some(Index::New))
    {
        return Err(format!(
            "{}: [new] is only for appending a new node; cannot remove",
            segments.join(".")
        ));
    }

    let path = segments.join(".");
    let document = parse(text)?;
    let found = find(text, &document, segments, block)?;
    let name = clean_name(segments[segments.len() - 1]);
    let outputs = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "output")
        .count();

    // where the block is in the result: where it was, or last of its kind
    let after_block = match block {
        Some(Block::Nth(ordinal)) if *ordinal >= outputs => {
            return Err(format!("{path}: no output block {ordinal} in the file"));
        }
        Some(Block::Nth(ordinal)) => Some(Block::Nth(*ordinal)),
        Some(Block::New(_)) => Some(Block::Nth(outputs)),
        None => None,
    };
    let head = match block {
        Some(Block::New(name)) => Some(format!("output {}", string(name))),
        _ => None,
    };

    if found.leaves.len() > 1 {
        return Err(format!(
            "{path}: set {} times in the file",
            found.leaves.len()
        ));
    }

    let splices: Vec<Splice> = match (found.leaves.first(), op) {
        (None, Op::Remove | Op::DropEmpty) => return Ok(None),
        (None, Op::Set(value)) => {
            if let Some(bad) = segments
                .iter()
                .find(|segment| !writable_name(clean_name(segment)))
            {
                return Err(format!("{path}: {bad} is not a name Kanade writes"));
            }

            match node_text(name, value) {
                Some(leaf) => vec![create(
                    text,
                    &document,
                    &found,
                    segments,
                    head.as_deref(),
                    &leaf,
                )?],
                None => return Ok(None),
            }
        }
        (Some(leaf), Op::Remove) => {
            let (last_name, last_index) = parse_segment(segments[segments.len() - 1])?;
            let is_indexed_repeated = REPEATED.contains(&last_name) && last_index.is_some();

            if leaf.block && !is_indexed_repeated {
                return Err(format!("{path}: is a block; remove what is in it"));
            }

            let mut splice = remove(text, leaf);
            if leaf.block
                && let Some(start) = blank_line_before(&text[..splice.0.start])
                && {
                    let after = &text[splice.0.end..];

                    after.trim().is_empty() || after.starts_with('\n') || after.starts_with("\r\n")
                }
            {
                splice.0.start = start;
            }
            vec![splice]
        }
        (Some(leaf), Op::Set(value)) => {
            let (last_name, last_index) = parse_segment(segments[segments.len() - 1])?;
            let is_indexed_repeated = REPEATED.contains(&last_name) && last_index.is_some();

            if leaf.block && !is_indexed_repeated {
                return Err(format!("{path}: is a block; set what is in it"));
            }

            let flag = leaf.arguments.is_empty() && leaf.props.is_empty();

            match (leaf.arguments.as_slice(), value) {
                ([], Value::Props(props)) => {
                    if props_are(&leaf.props, props) {
                        return Ok(None);
                    }

                    props_splice(text, leaf, props)
                }
                (_, Value::Props(_)) => {
                    return Err(format!(
                        "{path}: has a value of its own; properties are not set beside it"
                    ));
                }
                _ if !leaf.props.is_empty() => {
                    let names: Vec<&str> = leaf.props.iter().map(|(n, _)| n.as_str()).collect();

                    return Err(format!(
                        "{path}: has properties ({}) a plain value would not keep; \
                         change it in the file",
                        names.join(", ")
                    ));
                }
                ([], Value::Bool(on)) if flag => {
                    if *on {
                        return Ok(None);
                    }

                    vec![remove(text, leaf)]
                }
                ([], value) => vec![(leaf.name_end..leaf.name_end, format!(" {}", value.v1()))],
                ([argument], value) => {
                    let new = value.v1();

                    if text[argument.clone()] == new {
                        return Ok(None);
                    }

                    vec![(argument.clone(), new)]
                }
                (arguments, value) if !arguments.is_empty() => {
                    let new = value.v1();
                    let span = arguments[0].start..arguments[arguments.len() - 1].end;
                    if text[span.clone()] == new {
                        return Ok(None);
                    }
                    vec![(span, new)]
                }
                _ => {
                    return Err(format!(
                        "{path}: takes several values; not a single setting"
                    ));
                }
            }
        }
        (Some(_), Op::DropEmpty) => return Ok(None),
    };

    let edited = splice(text, splices);

    // only that node may have changed
    let after = parse(&edited)
        .map_err(|why| format!("the edit made invalid KDL ({why}); nothing written"))?;

    let (first_name, first_index) = parse_segment(segments[0])?;
    let is_repeated_first = REPEATED.contains(&first_name);

    let (skip_doc, skip_after, gone) = match (op, first_index) {
        (Op::Remove, Some(Index::Nth(n))) if segments.len() == 1 && is_repeated_first => {
            (vec![], vec![], Some((first_name, n)))
        }
        (Op::Set(_), Some(Index::New)) if is_repeated_first => {
            let doc_count = document
                .nodes()
                .iter()
                .filter(|node| node.name().value() == first_name)
                .count();
            let new_ordinal = doc_count;
            let mut after_skip = vec![format!("{first_name}[{new_ordinal}]")];
            after_skip.extend(segments[1..].iter().map(|s| s.to_string()));
            (vec![], after_skip, None)
        }
        _ => {
            let mut skip_path: Vec<String> = segments.iter().map(|&s| s.to_owned()).collect();
            if let Some(Block::Nth(place)) = &after_block {
                skip_path[0] = format!("output#{place}");
            }
            (skip_path.clone(), skip_path, None)
        }
    };

    if flat(&document, &skip_doc, gone) != flat(&after, &skip_after, None) {
        return Err(String::from(
            "the edit changed more than its node; nothing written",
        ));
    }

    let check_segments: Vec<String> = match (op, first_index) {
        (Op::Set(_), Some(Index::New)) if is_repeated_first => {
            let doc_count = document
                .nodes()
                .iter()
                .filter(|node| node.name().value() == first_name)
                .count();
            let mut segs = vec![format!("{first_name}[{doc_count}]")];
            segs.extend(segments[1..].iter().map(|s| s.to_string()));
            segs
        }
        _ => segments.iter().map(|&s| s.to_string()).collect(),
    };
    let check_refs: Vec<&str> = check_segments.iter().map(|s| s.as_str()).collect();

    let now = find(&edited, &after, &check_refs, after_block.as_ref())?.leaves;
    let right = match op {
        Op::Remove | Op::DropEmpty if is_repeated_first && segments.len() == 1 => {
            let count_before = document
                .nodes()
                .iter()
                .filter(|n| n.name().value() == first_name)
                .count();
            let count_after = after
                .nodes()
                .iter()
                .filter(|n| n.name().value() == first_name)
                .count();
            count_after + 1 == count_before
        }
        Op::Remove | Op::DropEmpty => now.is_empty(),
        Op::Set(Value::Props(props)) => matches!(
            now.as_slice(),
            [leaf] if leaf.values.is_empty() && !leaf.block && props_are(&leaf.props, props)
        ),
        Op::Set(value) => match (now.as_slice(), value, FLAGS.contains(&name)) {
            ([], Value::Bool(false), true) => true,
            ([leaf], Value::Bool(true), _) if leaf.values.is_empty() && !leaf.block => true,
            ([leaf], _, _)
                if leaf.block && (first_name == "window-rule" || first_name == "layer-rule") =>
            {
                true
            }
            ([leaf], value, _) => {
                leaf.values.len() == 1
                    && leaf.props.is_empty()
                    && Value::of(&leaf.values[0]).as_ref() == Some(value)
            }
            _ => false,
        },
    };

    if !right {
        return Err(String::from(
            "the edit did not give the node the value asked; nothing written",
        ));
    }

    Ok(Some(format!("{bom}{edited}")))
}

// splice existing properties in place and append new ones, preserving order, spacing, and comments;
// dropping one takes the blanks after it, or before it when a comment stands between, and nothing else.
fn props_splice(text: &str, leaf: &Leaf, want: &[(String, Value)]) -> Vec<Splice> {
    let end = leaf.entries_end.unwrap_or(leaf.name_end);

    if want.is_empty() {
        return vec![(leaf.name_end..end, String::new())];
    }

    let spans = leaf.prop_spans();

    if leaf.props.is_empty() || spans.len() != leaf.props.len() {
        return vec![(
            leaf.name_end..end,
            format!(" {}", Value::Props(want.to_vec()).v1()),
        )];
    }

    let mut splices = Vec::new();
    let mut last = leaf.name_end;

    for ((name, _), span) in leaf.props.iter().zip(&spans) {
        match want.iter().find(|(wanted, _)| wanted == name) {
            Some((_, value)) => {
                let written = format!("{name}={}", value.v1());

                if text[span.clone()] != written {
                    splices.push((span.clone(), written));
                }
            }
            // the blanks after it, or the ones before it when a comment stands in between, and
            // nothing else
            None => {
                let after = &text[span.end..];
                let trailing = after.len() - after.trim_start_matches([' ', '\t']).len();
                let before = &text[last..span.start];
                let (from, to) = if trailing > 0 {
                    (span.start, span.end + trailing)
                } else if before.chars().all(|char| matches!(char, ' ' | '\t')) {
                    (span.start - before.len(), span.end)
                } else {
                    (span.start, span.end)
                };

                splices.push((from..to, String::new()));
            }
        }

        last = span.end;
    }

    let added: Vec<(String, Value)> = want
        .iter()
        .filter(|(wanted, _)| !leaf.props.iter().any(|(name, _)| name == wanted))
        .cloned()
        .collect();

    if !added.is_empty() {
        let at = splices
            .iter()
            .map(|(range, _)| range.start)
            .min()
            .map_or(end, |first| first.min(end));

        splices.push((at..at, format!(" {}", Value::Props(added).v1())));
    }

    splices
}

// whether a node's properties are these, in any order: KDL property names are unique, so their
// order says nothing and a change keeps the file's own
fn props_are(have: &[(String, KdlValue)], want: &[(String, Value)]) -> bool {
    have.len() == want.len()
        && want.iter().all(|(want_name, want_value)| {
            have.iter().any(|(name, value)| {
                name == want_name && Value::of(value).as_ref() == Some(want_value)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(text: &str, path: &str, value: &str) -> Result<Option<String>, String> {
        apply(
            text,
            &segments(path).unwrap(),
            &Op::Set(Value::parse(value).unwrap()),
        )
    }

    fn unset(text: &str, path: &str) -> Result<Option<String>, String> {
        apply(text, &segments(path).unwrap(), &Op::Remove)
    }

    #[test]
    fn a_value_is_spliced_and_every_other_byte_stays() {
        // a CRLF file, a trailing comment, a `/-` node and a comment with no newline after it
        let text = "// head\r\nlayout {\r\n    gaps 16 // keep\r\n    /-focus-ring { off; }\r\n    focus-ring { width 4; }\r\n}\r\n// end";
        let edited = set(text, "layout.gaps", "8").unwrap().unwrap();

        assert_eq!(edited, text.replace("gaps 16", "gaps 8"));

        let edited = set(text, "layout.focus-ring.width", "2.5")
            .unwrap()
            .unwrap();

        assert_eq!(edited, text.replace("width 4", "width 2.5"));
    }

    #[test]
    fn a_value_already_there_changes_nothing() {
        assert_eq!(
            set("layout {\n    gaps 4\n}\n", "layout.gaps", "4"),
            Ok(None)
        );
        assert_eq!(set("prefer-no-csd\n", "prefer-no-csd", "true"), Ok(None));
        assert_eq!(unset("layout {\n}\n", "layout.gaps"), Ok(None));
    }

    #[test]
    fn values_are_written_as_kdl_v1() {
        assert_eq!(Value::parse("12"), Ok(Value::Int(12)));
        assert_eq!(Value::parse("-3"), Ok(Value::Int(-3)));
        assert_eq!(Value::parse("0.5"), Ok(Value::Float(0.5)));
        assert_eq!(Value::parse("1e3"), Ok(Value::Float(1000.0)));
        assert_eq!(Value::parse("false"), Ok(Value::Bool(false)));
        assert_eq!(Value::parse("inf"), Ok(Value::Str(String::from("inf"))));
        assert_eq!(Value::parse("0x10"), Ok(Value::Str(String::from("0x10"))));
        assert_eq!(Value::parse("\"12\""), Ok(Value::Str(String::from("12"))));
        assert!(Value::parse("99999999999999999999").is_err());

        assert_eq!(Value::Float(1.0).v1(), "1.0");
        assert_eq!(Value::Str(String::from("us")).v1(), "\"us\"");
        assert_eq!(Value::Str(String::from("a\"b")).v1(), "r#\"a\"b\"#");
        assert_eq!(Value::Str(String::from("a\"#b\\")).v1(), "r##\"a\"#b\\\"##");
        assert_eq!(Value::Str(String::from("a\nb")).v1(), "\"a\\nb\"");

        // what is written reads back as the value
        for value in [
            Value::Str(String::from("C:\\x \"q\"#")),
            Value::Str(String::from("tab\there")),
            Value::Float(0.25),
            Value::Int(-7),
        ] {
            let text = format!("x {}\n", value.v1());
            let hit = &hits(&text, &["x"]).unwrap()[0];

            assert_eq!(hit.shown, value.v1(), "{text}");
        }
    }

    #[test]
    fn a_missing_node_goes_at_the_end_of_its_block_with_the_files_indent() {
        let text = "layout {\n\tgaps 4\n\n\t// last\n}\n";
        let edited = set(text, "layout.always-center-single-column", "true")
            .unwrap()
            .unwrap();

        assert_eq!(
            edited,
            "layout {\n\tgaps 4\n\n\t// last\n\talways-center-single-column\n}\n"
        );

        // nested, with the blocks made
        let text = "layout {\n  gaps 4\n}\n";
        let edited = set(text, "layout.focus-ring.width", "3").unwrap().unwrap();

        assert_eq!(
            edited,
            "layout {\n  gaps 4\n  focus-ring {\n    width 3\n  }\n}\n"
        );
    }

    #[test]
    fn a_missing_block_goes_at_the_end_of_the_file_after_a_blank_line() {
        let edited = set("prefer-no-csd\n", "layout.gaps", "8").unwrap().unwrap();

        assert_eq!(edited, "prefer-no-csd\n\nlayout {\n    gaps 8\n}\n");

        // no newline at the end of a last comment: it gets one, nothing else of it changes
        let edited = set("// end", "screenshot-path", "/tmp/x.png")
            .unwrap()
            .unwrap();

        assert_eq!(edited, "// end\n\nscreenshot-path \"/tmp/x.png\"\n");

        let crlf = set("a\r\n", "layout.gaps", "8").unwrap().unwrap();

        assert_eq!(crlf, "a\r\n\r\nlayout {\r\n    gaps 8\r\n}\r\n");
        assert_eq!(
            set("", "layout.gaps", "8").unwrap().unwrap(),
            "layout {\n    gaps 8\n}\n"
        );
    }

    #[test]
    fn a_new_top_level_node_goes_before_the_appearance_syncs_include() {
        let text = "layout {\n    gaps 4\n}\n\ninclude optional=true \"kanade.kdl\" // Kanade appearance sync\n";
        let edited = set(text, "prefer-no-csd", "true").unwrap().unwrap();

        assert_eq!(
            edited,
            "layout {\n    gaps 4\n}\n\nprefer-no-csd\n\ninclude optional=true \"kanade.kdl\" // Kanade appearance sync\n"
        );
    }

    #[test]
    fn a_new_node_before_an_indented_include_takes_its_line_and_no_whitespace_of_its_own() {
        let text = "layout {\n    gaps 4\n}\n\n  include optional=true \"kanade.kdl\"\n";
        let edited = set(text, "prefer-no-csd", "true").unwrap().unwrap();

        // no line holding only blanks, and the include keeps the indent it had
        assert_eq!(
            edited,
            "layout {\n    gaps 4\n}\n\nprefer-no-csd\n\n  include optional=true \"kanade.kdl\"\n"
        );

        // taking the node away again leaves the include where it was, with the blank line before
        // it that the insert made
        assert_eq!(
            unset(&edited, "prefer-no-csd").unwrap().unwrap(),
            "layout {\n    gaps 4\n}\n\n\n  include optional=true \"kanade.kdl\"\n"
        );
    }

    #[test]
    fn a_file_mixing_tabs_and_spaces_gets_its_own_step_under_a_block() {
        // the block's own step is a tab, though the file's first step is four spaces
        let text = "layout {\n    gaps 4\n    focus-ring {\n    \twidth 3\n    }\n}\n";
        let edited = set(text, "layout.focus-ring.active-color", "red")
            .unwrap()
            .unwrap();

        assert_eq!(
            edited,
            "layout {\n    gaps 4\n    focus-ring {\n    \twidth 3\n    \tactive-color \"red\"\n    }\n}\n"
        );
    }

    #[test]
    fn a_one_line_block_is_added_to_in_place() {
        let text = "layout { gaps 4; }\ndefault { }\n";

        assert_eq!(
            set(text, "layout.focus-ring.off", "true").unwrap().unwrap(),
            "layout { gaps 4; focus-ring { off; }; }\ndefault { }\n"
        );
        assert_eq!(
            set(text, "default.x", "1").unwrap().unwrap(),
            "layout { gaps 4; }\ndefault {\n    x 1\n}\n"
        );
        assert_eq!(
            set("layout {}\n", "layout.gaps", "2").unwrap().unwrap(),
            "layout {\n    gaps 2\n}\n"
        );
    }

    #[test]
    fn a_flag_is_made_by_true_and_removed_by_false() {
        let text = "layout {\n    border {\n        off\n    }\n}\n";

        assert_eq!(
            set(text, "layout.border.off", "false").unwrap().unwrap(),
            "layout {\n    border {\n    }\n}\n"
        );
        assert_eq!(
            set(
                "layout {\n    border {\n    }\n}\n",
                "layout.border.off",
                "true"
            )
            .unwrap()
            .unwrap(),
            text
        );
        assert_eq!(
            hits(text, &["layout", "border", "off"]).unwrap()[0].shown,
            "true"
        );
    }

    #[test]
    fn removing_takes_the_nodes_own_line_and_no_comment_of_another() {
        let text = "layout {\n    // the gap\n    gaps 4 // tight\n\n    // after\n    center-focused-column \"never\"\n}\n";
        let edited = unset(text, "layout.gaps").unwrap().unwrap();

        assert_eq!(
            edited,
            "layout {\n    // the gap\n\n    // after\n    center-focused-column \"never\"\n}\n"
        );

        // among others on its line, only the node and its `;`
        let text = "layout { gaps 4; center-focused-column \"never\"; }\n";

        assert_eq!(
            unset(text, "layout.gaps").unwrap().unwrap(),
            "layout { center-focused-column \"never\"; }\n"
        );

        // the last line with no newline
        assert_eq!(unset("a 1\nb 2", "b").unwrap().unwrap(), "a 1\n");
        assert_eq!(unset("a 1\r\nb 2\r\n", "a").unwrap().unwrap(), "b 2\r\n");
    }

    #[test]
    fn what_a_path_cannot_mean_is_refused() {
        let text = "window-rule {\n    opacity 0.9\n}\noutput \"eDP-1\" {\n    scale 2\n}\nlayout {\n    default-column-width { proportion 0.5; }\n    /-struts { left 1; }\n}\n";

        assert!(
            set(text, "window-rule.opacity", "1")
                .unwrap_err()
                .contains("more than once")
        );
        assert!(
            set(text, "output.scale", "1")
                .unwrap_err()
                .contains("more than once")
        );
        assert!(
            set(text, "layout.default-column-width", "1")
                .unwrap_err()
                .contains("is a block")
        );
        assert!(
            unset(text, "layout.default-column-width")
                .unwrap_err()
                .contains("is a block")
        );
        assert!(
            set(text, "layout.gaps.x", "1").is_ok(),
            "no node `gaps` yet: made"
        );
        assert!(
            set("layout\n", "layout.gaps", "1")
                .unwrap_err()
                .contains("is not a block")
        );
        assert!(
            set("x \"a\" { y 1; }\n", "x.y", "2")
                .unwrap_err()
                .contains("argument of its own")
        );
        assert_eq!(set("a 1 2\n", "a", "3").unwrap().as_deref(), Some("a 3\n"));
        assert!(segments("layout..gaps").is_err());

        // off with `/-`, so not made again beside it
        assert!(
            set(text, "layout.struts.left", "2")
                .unwrap_err()
                .contains("turned off with /-")
        );
        assert!(
            set("/-prefer-no-csd\n", "prefer-no-csd", "true")
                .unwrap_err()
                .contains("/-")
        );
        assert!(
            set("/-layout { gaps 1; }\n", "layout.gaps", "2")
                .unwrap_err()
                .contains("/-")
        );

        // a name Kanade will not write bare
        assert!(
            set("layout {\n}\n", "layout.a b", "1")
                .unwrap_err()
                .contains("not a name")
        );
    }

    #[test]
    fn a_slashdash_inside_a_comment_or_string_is_no_slashdash() {
        let text = "layout {\n    // /-gaps 1\n    /* /-gaps 2 */\n    struts { left 1; }\n}\nscreenshot-path \"/-gaps\"\n";

        assert!(set(text, "layout.gaps", "3").is_ok());
        assert_eq!(
            slashdashed_in("\n/- foo 1 2\n bar\n/-\"baz\" { x; /-y }\n"),
            ["foo", "baz"]
        );
        assert_eq!(slashdashed_in("/*a*/ /- /*c*/ r#\"q\"# 1\n"), ["q"]);
    }

    #[test]
    fn invalid_kdl_v1_is_refused_with_its_line() {
        let why = set("layout {\n    gaps 4\n", "layout.gaps", "1").unwrap_err();

        assert!(why.contains("line"), "{why}");
        assert!(
            set("layout { gaps 4 }\n", "layout.gaps", "1").is_err(),
            "v2 syntax"
        );
    }

    #[test]
    fn a_byte_order_mark_stays_where_it_is_through_every_edit() {
        let bom = "\u{feff}";

        // set on a flag, remove it, and create it in an empty file
        let text = format!("{bom}prefer-no-csd\n");

        assert_eq!(
            unset(&text, "prefer-no-csd").unwrap().unwrap(),
            bom.to_owned()
        );
        assert_eq!(
            set(&text, "layout.gaps", "8").unwrap().unwrap(),
            format!("{bom}prefer-no-csd\n\nlayout {{\n    gaps 8\n}}\n")
        );
        assert_eq!(set(&text, "prefer-no-csd", "true"), Ok(None));
        assert_eq!(hits(&text, &["prefer-no-csd"]).unwrap()[0].shown, "true");
        assert_eq!(
            set(bom, "layout.gaps", "8").unwrap().unwrap(),
            format!("{bom}layout {{\n    gaps 8\n}}\n")
        );

        // a value changed in a block, the rest identical
        let text = format!("{bom}// \u{e9}\nlayout {{\n    gaps 4\n}}\n");

        assert_eq!(
            set(&text, "layout.gaps", "9").unwrap().unwrap(),
            text.replace("gaps 4", "gaps 9")
        );
        assert_eq!(
            includes(&format!("{bom}include \"a.kdl\"\n"))
                .unwrap()
                .len(),
            1
        );
        assert!(
            parse(&format!("{bom}layout {{\n    gaps 4\n"))
                .unwrap_err()
                .contains("line")
        );
    }

    #[test]
    fn a_run_of_byte_order_marks_is_kept_byte_for_byte() {
        let bom = "\u{feff}\u{feff}";
        let text = format!("{bom}layout {{\n    gaps 4\n}}\n\u{e9}");

        assert_eq!(hits(&text, &["layout", "gaps"]).unwrap()[0].shown, "4");
        assert_eq!(
            set(&text, "layout.gaps", "9").unwrap().unwrap(),
            text.replace("gaps 4", "gaps 9")
        );
        assert_eq!(
            unset(&text, "layout.gaps").unwrap().unwrap(),
            format!("{bom}layout {{\n}}\n\u{e9}")
        );
        assert!(
            set(bom, "layout.gaps", "8")
                .unwrap()
                .unwrap()
                .starts_with(bom)
        );
    }

    #[test]
    fn text_that_is_not_ascii_around_a_slashdash_is_no_panic_and_stays() {
        // after a `/-` node's tail, in its comment, in a block under it, and after it
        for (text, path, from, to, value) in [
            (
                "layout {\n  /-gaps 4 // \u{e9}\n  struts { left 1; }\n}",
                "layout.struts.left",
                "left 1",
                "left 2",
                "2",
            ),
            (
                "/-x { y \"\u{e9}\"; /* \u{65e5} */ } // \u{e9}\nlayout { gaps 1; }\n",
                "layout.gaps",
                "gaps 1",
                "gaps 2",
                "2",
            ),
            (
                "// \u{e9}\n/-a 1 // say \"\u{65e5}\nlayout { gaps 1; }\n",
                "layout.gaps",
                "gaps 1",
                "gaps 2",
                "2",
            ),
        ] {
            assert_eq!(
                set(text, path, value).unwrap().unwrap(),
                text.replace(from, to),
                "{text}"
            );
        }

        // a flag turned off by a `/-` that ends the file in multibyte text
        let text = "/-always-center-single-column // \u{65e5}\u{672c}";

        assert!(
            set(text, "always-center-single-column", "true")
                .unwrap_err()
                .contains("/-")
        );
        assert_eq!(
            set(text, "prefer-no-csd", "true").unwrap().unwrap(),
            format!("{text}\n\nprefer-no-csd\n")
        );

        // the names a `/-` turns off are still found past multibyte text and a stray quote
        assert_eq!(
            slashdashed_in("\u{e9}/- a // \u{65e5} \"x\n/- b 1\n"),
            ["a", "b"]
        );
        assert_eq!(slashdashed_in("/*\u{e9}/*\u{e9}*/*/ /- c\n"), ["c"]);
        assert_eq!(slashdashed_in("/- d \"\u{e9}\\\"\" 1\n/- e\n"), ["d", "e"]);
    }

    #[test]
    fn includes_are_listed_in_file_order() {
        let text = "include \"a.kdl\"\nlayout {\n}\ninclude optional=true \"b.kdl\"\n";
        let listed = includes(text).unwrap();

        assert_eq!(
            listed
                .iter()
                .map(|(_, file, optional)| (file.as_str(), *optional))
                .collect::<Vec<_>>(),
            [("a.kdl", false), ("b.kdl", true)]
        );
        assert!(listed[0].0 < listed[1].0);
    }

    // ---- output blocks ----

    fn in_block(
        text: &str,
        block: &Block,
        below: &str,
        value: &str,
    ) -> Result<Option<String>, String> {
        apply_in(
            text,
            block,
            &segments(below).unwrap(),
            &Op::Set(Value::parse(value).unwrap()),
        )
    }

    fn block_hit(text: &str, ordinal: usize, below: &str) -> Option<String> {
        hits_in(text, ordinal, &segments(below).unwrap())
            .unwrap()
            .pop()
            .map(|hit| hit.shown)
    }

    const OUTPUTS: &str = "// monitors\r\noutput \"DP-1\" {\r\n    scale 1\r\n}\r\n\r\noutput \"DP-1\" {\r\n    scale 1\r\n    // keep\r\n    position x=0 y=0\r\n}\r\noutput \"HDMI-A-1\" {\r\n    off\r\n}\r\n// end";

    #[test]
    fn a_block_is_picked_by_its_place_among_blocks_that_look_alike() {
        let first = in_block(OUTPUTS, &Block::Nth(0), "scale", "2")
            .unwrap()
            .unwrap();
        let second = in_block(OUTPUTS, &Block::Nth(1), "scale", "2")
            .unwrap()
            .unwrap();

        // only the picked block changes, every other byte stays (CRLF, comments, no last newline)
        assert_eq!(first, OUTPUTS.replacen("scale 1", "scale 2", 1));
        assert_eq!(
            second,
            OUTPUTS
                .replacen("scale 1", "scale 2", 2)
                .replacen("scale 2", "scale 1", 1)
        );
        assert_eq!(block_hit(&second, 0, "scale").as_deref(), Some("1"));
        assert_eq!(block_hit(&second, 1, "scale").as_deref(), Some("2"));
        assert_eq!(block_hit(OUTPUTS, 2, "scale"), None);
        assert_eq!(block_hit(OUTPUTS, 2, "off").as_deref(), Some("true"));
        assert!(in_block(OUTPUTS, &Block::Nth(3), "scale", "2").is_err());

        let names: Vec<_> = outputs(OUTPUTS)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();

        assert_eq!(names, ["DP-1", "DP-1", "HDMI-A-1"]);
    }

    #[test]
    fn a_missing_block_is_made_at_the_end_with_the_files_own_bytes() {
        let made = in_block(OUTPUTS, &Block::New("eDP-1".into()), "scale", "1.5")
            .unwrap()
            .unwrap();

        assert_eq!(
            made,
            format!("{OUTPUTS}\r\n\r\noutput \"eDP-1\" {{\r\n    scale 1.5\r\n}}\r\n")
        );
        assert_eq!(block_hit(&made, 3, "scale").as_deref(), Some("1.5"));

        // a BOM stays first, and a block goes before the appearance sync's include
        let synced = "\u{feff}layout {\n    gaps 4\n}\n\ninclude \"kanade.kdl\"\n";
        let made = in_block(synced, &Block::New("DP-1".into()), "off", "true")
            .unwrap()
            .unwrap();

        assert_eq!(
            made,
            "\u{feff}layout {\n    gaps 4\n}\n\noutput \"DP-1\" {\n    off\n}\n\ninclude \"kanade.kdl\"\n"
        );
    }

    #[test]
    fn a_block_under_a_slashdash_blocks_only_its_own_name() {
        let text = "/-output \"DP-1\" {\n    scale 2\n}\n";
        let other = in_block(text, &Block::New("HDMI-A-1".into()), "scale", "2")
            .unwrap()
            .unwrap();

        assert!(
            other.starts_with(text) && other.contains("output \"HDMI-A-1\""),
            "{other}"
        );
        assert!(
            in_block(text, &Block::New("DP-1".into()), "scale", "2")
                .unwrap_err()
                .contains("/-"),
        );
    }

    #[test]
    fn a_name_with_spaces_dots_quotes_and_backslashes_comes_back_as_it_went() {
        for name in [
            "Dell Inc. DELL P1917S 0x00001234",
            "say \"hi\"",
            "a\\b",
            "r#\"x\"#",
            "\u{e9}\u{65e5}",
            "tab\there",
        ] {
            let made = in_block("", &Block::New(name.into()), "scale", "2")
                .unwrap()
                .unwrap();
            let blocks = outputs(&made).unwrap();

            assert_eq!(blocks.len(), 1, "{name}: {made}");
            assert_eq!(blocks[0].name, name, "{made}");
            assert_eq!(block_hit(&made, 0, "scale").as_deref(), Some("2"));

            // under `/-` it is the same block's name, for the one name only
            let off = format!("/-{}", made.trim_start());

            assert!(
                in_block(&off, &Block::New(name.into()), "scale", "2").is_err(),
                "{name}"
            );
            assert!(in_block(&off, &Block::New("other".into()), "scale", "2").is_ok());
        }
    }

    #[test]
    fn a_name_that_could_read_as_another_is_refused() {
        // U+202E reverses what follows it, so a name could show as another monitor's
        for (char, code) in [
            ('\u{061c}', "U+061C"),
            ('\u{200e}', "U+200E"),
            ('\u{200f}', "U+200F"),
            ('\u{202a}', "U+202A"),
            ('\u{202e}', "U+202E"),
            ('\u{2066}', "U+2066"),
            ('\u{2069}', "U+2069"),
        ] {
            let name = format!("DP-1{char}eviled");
            let refused = in_block("", &Block::New(name.clone()), "scale", "2").unwrap_err();

            assert!(refused.contains(code), "{code}: {refused}");
            assert!(refused.contains("bidirectional"), "{code}: {refused}");
        }

        // and a name with none of them is written as it always was
        assert!(in_block("", &Block::New("DP-1".into()), "scale", "2").is_ok());
    }

    fn props(text: &str) -> Value {
        Value::parse_props(text).unwrap()
    }

    fn put(text: &str, block: usize, below: &str, value: Value) -> Result<Option<String>, String> {
        apply_in(
            text,
            &Block::Nth(block),
            &segments(below).unwrap(),
            &Op::Set(value),
        )
    }

    #[test]
    fn a_property_change_keeps_the_spacing_and_comments_between_the_others() {
        let text = "output \"DP-1\" {\n    position   x=0   y=0   // left\n    scale 2\n}\n";

        let moved = put(text, 0, "position", props("x=-1920 y=0"))
            .unwrap()
            .unwrap();

        // x is spliced where it was; y is the same and stays byte for byte
        assert_eq!(
            moved,
            "output \"DP-1\" {\n    position   x=-1920   y=0   // left\n    scale 2\n}\n"
        );

        // a comment between two properties stays when both change
        let commented = "output \"DP-1\" {\n    position x=0 /* here */ y=0\n}\n";
        let both = put(commented, 0, "position", props("x=1 y=2"))
            .unwrap()
            .unwrap();

        assert_eq!(
            both,
            "output \"DP-1\" {\n    position x=1 /* here */ y=2\n}\n"
        );

        // and the file's own order is kept, as KDL property names are unique
        let ordered = "output \"DP-1\" {\n    position y=0 x=0\n}\n";
        let kept = put(ordered, 0, "position", props("x=1 y=2"))
            .unwrap()
            .unwrap();

        assert_eq!(kept, "output \"DP-1\" {\n    position y=2 x=1\n}\n");
        assert_eq!(block_hit(&kept, 0, "position").as_deref(), Some("y=2 x=1"));

        // a property dropped goes with the blanks before it and nothing else
        let dropped = put(&both, 0, "position", props("y=2")).unwrap().unwrap();
        assert_eq!(
            dropped,
            "output \"DP-1\" {\n    position /* here */ y=2\n}\n"
        );
    }

    #[test]
    fn properties_replace_the_whole_list_of_a_node_with_no_value() {
        let text = "output \"DP-1\" {\n    position x=0 y=0 // left\n    variable-refresh-rate on-demand=true\n}\n";

        let moved = put(text, 0, "position", props("x=-1920 y=100"))
            .unwrap()
            .unwrap();

        assert_eq!(moved, text.replace("x=0 y=0", "x=-1920 y=100"));
        assert_eq!(
            block_hit(&moved, 0, "position").as_deref(),
            Some("x=-1920 y=100")
        );
        assert_eq!(put(text, 0, "position", props("x=0 y=0")).unwrap(), None);

        // variable refresh rate: off is no node, on is the bare node, on-demand a property
        let on = put(text, 0, "variable-refresh-rate", props(""))
            .unwrap()
            .unwrap();

        assert_eq!(on, text.replace(" on-demand=true", ""));
        assert_eq!(
            block_hit(&on, 0, "variable-refresh-rate").as_deref(),
            Some("true")
        );

        let back = put(&on, 0, "variable-refresh-rate", props("on-demand=true"))
            .unwrap()
            .unwrap();

        assert_eq!(back, text);

        let off = apply_in(
            text,
            &Block::Nth(0),
            &["variable-refresh-rate"],
            &Op::Remove,
        )
        .unwrap()
        .unwrap();

        assert_eq!(off, "output \"DP-1\" {\n    position x=0 y=0 // left\n}\n");

        // made in a block that has none, and in a new block
        let made = put(&off, 0, "variable-refresh-rate", props("on-demand=true"))
            .unwrap()
            .unwrap();

        assert_eq!(made, text);

        let fresh = apply_in(
            "",
            &Block::New("DP-2".into()),
            &["position"],
            &Op::Set(props("x=1 y=2")),
        )
        .unwrap()
        .unwrap();

        assert_eq!(fresh, "output \"DP-2\" {\n    position x=1 y=2\n}\n");
    }

    #[test]
    fn a_plain_value_never_replaces_a_node_that_has_properties() {
        let text = "output \"DP-1\" {\n    mode custom=true \"1920x1080@60.000\"\n    position x=1 y=2\n    variable-refresh-rate on-demand=true\n}\n";

        for (below, value) in [
            ("mode", "\"1280x720@60.000\""),
            ("position", "5"),
            ("variable-refresh-rate", "true"),
        ] {
            let why = in_block(text, &Block::Nth(0), below, value).unwrap_err();

            assert!(why.contains("properties"), "{below}: {why}");
        }

        // and properties are no way past a value of the node's own
        assert!(put(text, 0, "mode", props("x=1")).is_err());
    }

    #[test]
    fn an_empty_block_is_dropped_and_one_with_anything_else_stays() {
        let text = "// a\noutput \"A\" {\n}\noutput \"B\" {\n    focus-at-startup\n}\noutput \"C\" {\n    // note\n}\noutput \"D\" { }\noutput \"E\"\noutput \"F\" {\n    scale 1\n}\n";
        let drop = |text: &str, n| apply_in(text, &Block::Nth(n), &[], &Op::DropEmpty).unwrap();

        assert_eq!(
            drop(text, 0).unwrap(),
            text.replace("output \"A\" {\n}\n", "")
        );
        assert_eq!(drop(text, 1), None);
        assert_eq!(drop(text, 2), None);
        assert_eq!(
            drop(text, 3).unwrap(),
            text.replace("output \"D\" { }\n", "")
        );
        assert_eq!(drop(text, 4).unwrap(), text.replace("output \"E\"\n", ""));
        assert_eq!(drop(text, 5), None);

        // the ones after it keep their blocks, by place
        let rest = drop(text, 0).unwrap();

        assert_eq!(block_hit(&rest, 4, "scale").as_deref(), Some("1"));
        assert!(apply_in(text, &Block::New("Z".into()), &[], &Op::DropEmpty).is_err());
    }
}
