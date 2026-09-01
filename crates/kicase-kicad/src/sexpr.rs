//! A minimal s-expression reader for KiCad board documents.
//!
//! KiCad hands the whole board over as its own s-expression text (through the
//! IPC `SaveDocumentToString` command), which is the only representation that
//! carries everything KiCase needs: polygon vertices, pad drills and, above
//! all, the persistent UUID of every object.

use std::fmt;

/// One s-expression node.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A bare token or a quoted string, unescaped.
    Atom(String),
    /// A parenthesised list.
    List(Vec<Node>),
}

impl Node {
    pub fn as_atom(&self) -> Option<&str> {
        match self {
            Node::Atom(text) => Some(text),
            Node::List(_) => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Node]> {
        match self {
            Node::List(items) => Some(items),
            Node::Atom(_) => None,
        }
    }

    /// The head symbol of a list, e.g. `gr_line` for `(gr_line ...)`.
    pub fn head(&self) -> Option<&str> {
        self.as_list()?.first()?.as_atom()
    }

    /// Direct children that are lists headed by `name`.
    pub fn children<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a Node> + 'a {
        let items: &[Node] = self.as_list().unwrap_or(&[]);
        // Own the name so the iterator borrows only `self`.
        let name = name.to_string();
        items.iter().filter(move |child| child.head() == Some(name.as_str()))
    }

    /// The first direct child list headed by `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children(name).next()
    }

    /// Arguments of a list, i.e. everything after the head symbol.
    pub fn args(&self) -> &[Node] {
        match self.as_list() {
            Some(items) if !items.is_empty() => &items[1..],
            _ => &[],
        }
    }

    /// `n`-th argument parsed as a number.
    pub fn number(&self, index: usize) -> Option<f64> {
        self.args().get(index)?.as_atom()?.parse().ok()
    }

    /// `n`-th argument as a string.
    pub fn string(&self, index: usize) -> Option<&str> {
        self.args().get(index)?.as_atom()
    }

    /// Value of a single-argument child, e.g. `(layer "Edge.Cuts")`.
    pub fn child_string(&self, name: &str) -> Option<&str> {
        self.child(name)?.string(0)
    }

    /// Two-number child, e.g. `(start 1.0 2.0)`.
    pub fn child_xy(&self, name: &str) -> Option<(f64, f64)> {
        let node = self.child(name)?;
        Some((node.number(0)?, node.number(1)?))
    }

    /// Recursively walks every list node, depth first.
    pub fn walk(&self, visit: &mut impl FnMut(&Node)) {
        if let Node::List(items) = self {
            visit(self);
            for item in items {
                item.walk(visit);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

/// Parses a complete s-expression document.
pub fn parse(text: &str) -> Result<Node, ParseError> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    skip_whitespace(bytes, &mut pos);
    let node = parse_node(bytes, &mut pos)?;
    skip_whitespace(bytes, &mut pos);
    Ok(node)
}

fn parse_node(bytes: &[u8], pos: &mut usize) -> Result<Node, ParseError> {
    skip_whitespace(bytes, pos);
    match bytes.get(*pos) {
        None => Err(ParseError { message: "unexpected end of input".into(), offset: *pos }),
        Some(b'(') => {
            *pos += 1;
            let mut items = Vec::new();
            loop {
                skip_whitespace(bytes, pos);
                match bytes.get(*pos) {
                    None => {
                        return Err(ParseError { message: "unclosed list".into(), offset: *pos })
                    },
                    Some(b')') => {
                        *pos += 1;
                        return Ok(Node::List(items));
                    },
                    Some(_) => items.push(parse_node(bytes, pos)?),
                }
            }
        },
        Some(b')') => Err(ParseError { message: "unexpected ')'".into(), offset: *pos }),
        Some(b'"') => parse_quoted(bytes, pos),
        Some(_) => parse_bare(bytes, pos),
    }
}

fn parse_quoted(bytes: &[u8], pos: &mut usize) -> Result<Node, ParseError> {
    let start = *pos;
    *pos += 1; // opening quote

    // Copy in runs between escapes, validating each run once. Decoding
    // character by character would mean re-validating the rest of the file for
    // every character, which on a nine-megabyte board is quadratic and turns a
    // parse into minutes.
    let mut out = String::new();
    let mut run = *pos;
    loop {
        match bytes.get(*pos) {
            None => {
                return Err(ParseError { message: "unterminated string".into(), offset: start })
            },
            Some(b'\\') => {
                out.push_str(run_as_str(bytes, run, *pos)?);
                if let Some(&escaped) = bytes.get(*pos + 1) {
                    out.push(escaped as char);
                }
                *pos += 2;
                run = *pos;
            },
            Some(b'"') => {
                out.push_str(run_as_str(bytes, run, *pos)?);
                *pos += 1;
                return Ok(Node::Atom(out));
            },
            Some(_) => *pos += 1,
        }
    }
}

/// Validates one run of string bytes, and only that run.
fn run_as_str(bytes: &[u8], from: usize, to: usize) -> Result<&str, ParseError> {
    std::str::from_utf8(&bytes[from..to])
        .map_err(|_| ParseError { message: "invalid UTF-8".into(), offset: from })
}

fn parse_bare(bytes: &[u8], pos: &mut usize) -> Result<Node, ParseError> {
    let start = *pos;
    while let Some(&byte) = bytes.get(*pos) {
        if byte.is_ascii_whitespace() || byte == b'(' || byte == b')' {
            break;
        }
        *pos += 1;
    }
    let text = std::str::from_utf8(&bytes[start..*pos])
        .map_err(|_| ParseError { message: "invalid UTF-8".into(), offset: start })?;
    Ok(Node::Atom(text.to_string()))
}

fn skip_whitespace(bytes: &[u8], pos: &mut usize) {
    while let Some(&byte) = bytes.get(*pos) {
        if byte.is_ascii_whitespace() {
            *pos += 1;
        } else {
            break;
        }
    }
}

/// What a selective parse found.
pub struct Selection {
    /// The top-level items worth keeping, in file order.
    pub items: Vec<Node>,
    /// Every layer name referenced anywhere in the document, including inside
    /// the parts that were skipped.
    pub layers_seen: Vec<String>,
}

/// Parses only the parts of a board that matter, skipping the rest.
///
/// A board is mostly tracks, vias and filled zones, and an enclosure needs none
/// of it: six user layers, `Edge.Cuts`, the layer table and the footprints.
/// Building a node for every object in a nine-megabyte board costs seconds and
/// hundreds of megabytes; skipping a subtree costs a scan of its bytes.
///
/// `keep` lists the top-level heads to parse. Footprints are parsed with a
/// reduced set of their own, since their silkscreen and courtyard graphics are
/// just as irrelevant as a track.
pub fn parse_selected(text: &str, keep: &[&str]) -> Result<Selection, ParseError> {
    /// Inside a footprint, only these carry anything KiCase uses.
    ///
    /// `layer` is the only thing in the file that says which side a footprint
    /// is on, and `model` is the reference to its 3D shape.
    ///
    /// Every entry here has a price, and it is not noise: adding those two cost
    /// about 2 us per footprint, measured by interleaving two binaries that
    /// differed only in this list — +15% on a 43 MB, 866-footprint board and
    /// +31% on a synthetic one with 2000. Both are far inside the budget, and
    /// both are real. Anything added here should be measured the same way,
    /// because the whole reason this list is short is that a board with 40,000
    /// tracks used to take five minutes to open.
    const FOOTPRINT_KEEP: &[&str] = &["at", "uuid", "property", "pad", "attr", "layer", "model"];

    let bytes = text.as_bytes();
    let mut pos = 0usize;
    let mut items = Vec::new();
    let mut layers_seen: Vec<String> = Vec::new();

    skip_whitespace(bytes, &mut pos);
    if bytes.get(pos) != Some(&b'(') {
        return Err(ParseError { message: "expected a document".into(), offset: pos });
    }
    pos += 1;
    // The document's own head, e.g. `kicad_pcb`.
    let root = parse_bare(bytes, &mut pos)?;
    let root_head = root.as_atom().unwrap_or_default().to_string();
    items.push(Node::Atom(root_head));

    loop {
        skip_whitespace(bytes, &mut pos);
        match bytes.get(pos) {
            None => return Err(ParseError { message: "unclosed document".into(), offset: pos }),
            Some(b')') => break,
            Some(b'(') => {
                let open = pos;
                pos += 1;
                skip_whitespace(bytes, &mut pos);
                let head = parse_bare(bytes, &mut pos)?;
                let head = head.as_atom().unwrap_or_default().to_string();

                if keep.contains(&head.as_str()) {
                    let child_keep = if head == "footprint" { Some(FOOTPRINT_KEEP) } else { None };
                    let node = parse_rest_of_list(bytes, &mut pos, head, child_keep)?;
                    collect_layers(&node, &mut layers_seen);
                    items.push(node);
                } else {
                    pos = open;
                    skip_list(bytes, &mut pos, &mut layers_seen)?;
                }
            },
            Some(_) => {
                // A bare value at document level: keep it, it is cheap.
                items.push(parse_node(bytes, &mut pos)?);
            },
        }
    }

    Ok(Selection { items, layers_seen })
}

/// Parses the remainder of a list whose `(` and head have been consumed.
///
/// When `keep` is given, children whose head is not in it are skipped.
fn parse_rest_of_list(
    bytes: &[u8],
    pos: &mut usize,
    head: String,
    keep: Option<&[&str]>,
) -> Result<Node, ParseError> {
    let mut items = vec![Node::Atom(head)];
    loop {
        skip_whitespace(bytes, pos);
        match bytes.get(*pos) {
            None => return Err(ParseError { message: "unclosed list".into(), offset: *pos }),
            Some(b')') => {
                *pos += 1;
                return Ok(Node::List(items));
            },
            Some(b'(') => match keep {
                None => items.push(parse_node(bytes, pos)?),
                Some(keep) => {
                    let open = *pos;
                    *pos += 1;
                    skip_whitespace(bytes, pos);
                    let child = parse_bare(bytes, pos)?;
                    let child_head = child.as_atom().unwrap_or_default().to_string();
                    if keep.contains(&child_head.as_str()) {
                        items.push(parse_rest_of_list(bytes, pos, child_head, None)?);
                    } else {
                        *pos = open;
                        let mut ignored = Vec::new();
                        skip_list(bytes, pos, &mut ignored)?;
                    }
                },
            },
            Some(_) => items.push(parse_node(bytes, pos)?),
        }
    }
}

/// Walks past a whole list without building anything, noting layer names.
fn skip_list(
    bytes: &[u8],
    pos: &mut usize,
    layers_seen: &mut Vec<String>,
) -> Result<(), ParseError> {
    let start = *pos;
    let mut depth = 0usize;
    loop {
        match bytes.get(*pos) {
            None => return Err(ParseError { message: "unclosed list".into(), offset: start }),
            Some(b'"') => {
                // Skip the string whole: parentheses inside it mean nothing.
                let mut discard = *pos;
                parse_quoted(bytes, &mut discard)?;
                *pos = discard;
            },
            Some(b'(') => {
                // Cheap enough to notice `(layer "X")` on the way past.
                if bytes[*pos..].starts_with(b"(layer ") {
                    let mut at = *pos + b"(layer ".len();
                    skip_whitespace(bytes, &mut at);
                    if bytes.get(at) == Some(&b'"') {
                        if let Ok(Node::Atom(name)) = parse_quoted(bytes, &mut at) {
                            if !layers_seen.contains(&name) {
                                layers_seen.push(name);
                            }
                        }
                    }
                }
                depth += 1;
                *pos += 1;
            },
            Some(b')') => {
                depth -= 1;
                *pos += 1;
                if depth == 0 {
                    return Ok(());
                }
            },
            Some(_) => *pos += 1,
        }
    }
}

/// Records every layer name inside a node that was kept.
fn collect_layers(node: &Node, layers_seen: &mut Vec<String>) {
    node.walk(&mut |child| {
        if child.head() == Some("layer") {
            if let Some(name) = child.string(0) {
                if !layers_seen.iter().any(|seen| seen == name) {
                    layers_seen.push(name.to_string());
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_lists_and_quoted_strings() {
        let node = parse(r#"(kicad_pcb (version 20241229) (layer "User.1" user "Enclosure"))"#)
            .expect("parses");
        assert_eq!(node.head(), Some("kicad_pcb"));
        assert_eq!(node.child("version").and_then(|n| n.number(0)), Some(20241229.0));
        let layer = node.child("layer").expect("layer child");
        assert_eq!(layer.string(0), Some("User.1"));
        assert_eq!(layer.string(2), Some("Enclosure"));
    }

    #[test]
    fn handles_escaped_quotes() {
        let node = parse(r#"(descr "a \"quoted\" word")"#).expect("parses");
        assert_eq!(node.string(0), Some(r#"a "quoted" word"#));
    }

    #[test]
    fn reads_coordinate_children() {
        let node = parse("(gr_line (start 1.5 -2.5) (end 3 4))").expect("parses");
        assert_eq!(node.child_xy("start"), Some((1.5, -2.5)));
        assert_eq!(node.child_xy("end"), Some((3.0, 4.0)));
    }

    #[test]
    fn rejects_unbalanced_input() {
        assert!(parse("(a (b)").is_err());
    }
}
