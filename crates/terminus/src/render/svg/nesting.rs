//! The markup's element nesting, bounded before `roxmltree` parses it.
//!
//! `roxmltree` parses an element's content by recursion, one level per
//! nested element, and bounds only entity expansion (ten references inside
//! one another), not element nesting. Its frames take about 700 bytes a
//! level in a release build, so a document nested a few thousand deep would
//! overflow the 2 MiB stack of the decode-pool thread a native host parses
//! on (`bobcat-resources` runs it on tokio's blocking pool), which aborts the
//! process. [`bound`] runs first: one pass over the
//! markup that removes every element nested deeper than the bound, with its
//! content, which is what the converter's own walk does to an element past
//! [`MAX_NESTING`](super::parse::MAX_NESTING).
//!
//! The pass tokenises what changes the nesting the way `roxmltree` does:
//! comments, `CDATA` sections, processing instructions and declarations are
//! skipped whole, a start tag ends at the first `>` outside a quoted
//! attribute value and is empty when a `/` precedes that `>`, and every end
//! tag closes one level. Where the two would disagree the document is not
//! well-formed, and `roxmltree` stops at the first such token, no deeper
//! than the pass has counted.

use std::borrow::Cow;
use std::ops::Range;

/// How many entity references `roxmltree` expands inside one another; each
/// can nest the markup of its replacement text below the reference.
const ENTITY_DEPTH: usize = 10;

/// `text` with every element nested deeper than `limit` (the root element
/// is one deep) removed with its content, borrowed when no element is that
/// deep. `None` when the replacement text of the document's entities nests
/// markup that could take the parse past `limit`: `roxmltree` expands that
/// text itself, where no cut reaches.
pub(super) fn bound(text: &str, limit: usize) -> Option<Cow<'_, str>> {
    let scan = scan(text.as_bytes(), limit, true);
    if scan.literal_depth > 0
        && scan
            .kept_depth
            .saturating_add(ENTITY_DEPTH.saturating_mul(scan.literal_depth))
            > limit
    {
        return None;
    }
    if scan.cuts.is_empty() {
        return Some(Cow::Borrowed(text));
    }
    // Every cut starts at a `<` and ends after a `>` or at the end, so both
    // ends are character boundaries.
    let mut kept = String::with_capacity(text.len());
    let mut at = 0;
    for cut in scan.cuts {
        kept.push_str(&text[at..cut.start]);
        at = cut.end;
    }
    kept.push_str(&text[at..]);
    Some(Cow::Owned(kept))
}

/// What one pass over some markup found.
struct Scan {
    /// The deepest nesting left after the cuts.
    kept_depth: usize,
    /// The elements deeper than the limit, each with its content, in order.
    cuts: Vec<Range<usize>>,
    /// The deepest nesting of markup inside a quoted literal of a
    /// declaration (an entity's replacement text), when literals are read.
    literal_depth: usize,
}

/// One pass over `bytes`. `literals` reads the quoted literals of
/// declarations as markup; off inside a literal, where the parser reads no
/// declaration.
fn scan(bytes: &[u8], limit: usize, literals: bool) -> Scan {
    let mut found = Scan {
        kept_depth: 0,
        cuts: Vec::new(),
        literal_depth: 0,
    };
    let mut depth = 0_usize;
    // The open cut: the depth that closes it, and where it starts.
    let mut cut: Option<(usize, usize)> = None;
    let mut at = 0;
    while let Some(offset) = bytes[at..].iter().position(|&byte| byte == b'<') {
        let start = at + offset;
        let rest = &bytes[start..];
        at = if rest.starts_with(b"<!--") {
            past(bytes, start + 4, b"-->")
        } else if rest.starts_with(b"<![CDATA[") {
            past(bytes, start + 9, b"]]>")
        } else if rest.starts_with(b"<?") {
            past(bytes, start + 2, b"?>")
        } else if rest.starts_with(b"<!") {
            let (end, literal_depth) = declaration(bytes, start + 2, literals);
            found.literal_depth = found.literal_depth.max(literal_depth);
            end
        } else if rest.starts_with(b"</") {
            let end = past(bytes, start + 2, b">");
            depth = depth.saturating_sub(1);
            if let Some((closes_at, from)) = cut
                && depth == closes_at
            {
                found.cuts.push(from..end);
                cut = None;
            }
            end
        } else {
            let (end, empty) = start_tag(bytes, start + 1);
            if cut.is_none() {
                if depth < limit {
                    found.kept_depth = found.kept_depth.max(depth + 1);
                } else if empty {
                    found.cuts.push(start..end);
                } else {
                    cut = Some((depth, start));
                }
            }
            if !empty {
                depth += 1;
            }
            end
        };
    }
    if let Some((_, from)) = cut {
        found.cuts.push(from..bytes.len());
    }
    found
}

/// The offset just past the first `terminator` at or after `from`, or the
/// end.
fn past(bytes: &[u8], from: usize, terminator: &[u8]) -> usize {
    bytes
        .get(from..)
        .and_then(|rest| {
            rest.windows(terminator.len())
                .position(|window| window == terminator)
        })
        .map_or(bytes.len(), |offset| from + offset + terminator.len())
}

/// The start tag whose name begins at `from`: the offset just past its `>`
/// (or the end), and whether it is empty (`/>`). A `>` inside a quoted
/// attribute value does not end it.
fn start_tag(bytes: &[u8], from: usize) -> (usize, bool) {
    let mut quote = None;
    let mut previous = 0_u8;
    for (offset, &byte) in bytes[from..].iter().enumerate() {
        match quote {
            Some(open) if byte == open => quote = None,
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return (from + offset + 1, previous == b'/'),
            Some(_) | None => {}
        }
        previous = byte;
    }
    (bytes.len(), false)
}

/// The declaration (`<!DOCTYPE …>` and the like) whose body begins at
/// `from`: the offset just past it, and the deepest markup nesting inside
/// any of its quoted literals when `literals` asks for it. An internal
/// subset (`[…]`) is read through, its comments and processing
/// instructions skipped.
fn declaration(bytes: &[u8], from: usize, literals: bool) -> (usize, usize) {
    let mut deepest = 0;
    let mut subset = false;
    let mut at = from;
    while let Some(&byte) = bytes.get(at) {
        let rest = &bytes[at..];
        at = match byte {
            b'"' | b'\'' => {
                let end = bytes[at + 1..]
                    .iter()
                    .position(|&other| other == byte)
                    .map_or(bytes.len(), |offset| at + 1 + offset);
                if literals {
                    deepest = deepest.max(scan(&bytes[at + 1..end], usize::MAX, false).kept_depth);
                }
                (end + 1).min(bytes.len())
            }
            b'<' if subset && rest.starts_with(b"<!--") => past(bytes, at + 4, b"-->"),
            b'<' if subset && rest.starts_with(b"<?") => past(bytes, at + 2, b"?>"),
            b'[' if !subset => {
                subset = true;
                at + 1
            }
            b']' if subset => {
                subset = false;
                at + 1
            }
            b'>' if !subset => return (at + 1, deepest),
            _ => at + 1,
        };
    }
    (bytes.len(), deepest)
}
