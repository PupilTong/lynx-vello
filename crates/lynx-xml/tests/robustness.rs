// Copyright 2026 The Lynx Authors. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Input-robustness tests: arbitrary source must be rejected, never fatal.
//!
//! `parser.rs` covers the grammar — what a *correct* document parses to. That
//! cannot establish the property this file is about, because every input there
//! is one a correct author produced. Lynx XML is authored outside the engine,
//! so the question at that boundary is different: can any source string take
//! the process down, and does a rejection still describe itself truthfully?
//!
//! Two invariants ride along with panic-freedom, because a partial-index bug
//! would break them silently rather than loudly:
//!
//! - every returned section borrows from the source, rather than pointing at something the parser
//!   built;
//! - a `ParseError` offset lands on a real UTF-8 boundary inside the source. `lynx-xml` carries a
//!   `debug_assert!` on that, which these tests keep live, and the UTF-16 offset is what a
//!   JavaScript-side error message consumes.
//!
//! Deliberately not a fuzzer. Coverage-guided mutation buys little on a
//! 543-line zero-dependency parser over `&str`: the input is already valid
//! UTF-8, there are no length fields, and nothing allocates on a
//! source-controlled count. What it would buy is not worth a separate package
//! and a scheduled job, so the same search runs here, bounded and
//! deterministic, in the suite everyone already runs.

use lynx_xml::parse;

/// Whether `part` points into `whole`.
///
/// An empty `&str` is exempt: a zero-length slice of the source and a `""`
/// literal are indistinguishable at runtime, and the parser may return either
/// for a present-but-empty section.
fn borrows_from(part: &str, whole: &str) -> bool {
    if part.is_empty() {
        return true;
    }
    let start = whole.as_ptr() as usize;
    let offset = part.as_ptr() as usize;
    offset >= start && offset + part.len() <= start + whole.len()
}

/// Parses `source`, asserts everything that must hold either way, and reports
/// whether it parsed.
///
/// A panic anywhere under `parse` fails the test by itself; that is the
/// headline property and needs no assertion of its own. The return value feeds
/// the coverage floor below.
fn check(source: &str) -> bool {
    let mut ok = false;
    match parse(source) {
        Ok(parsed) => {
            ok = true;
            assert!(
                borrows_from(parsed.engine_version, source),
                "engine_version does not borrow from the source: {source:?}"
            );
            assert!(
                borrows_from(parsed.main_thread_script, source),
                "main_thread_script does not borrow from the source: {source:?}"
            );
            for section in [parsed.style, parsed.background_thread_script]
                .into_iter()
                .flatten()
            {
                assert!(
                    borrows_from(section, source),
                    "an optional section does not borrow from the source: {source:?}"
                );
            }
        }
        Err(error) => {
            let byte_offset = error.byte_offset();
            assert!(
                byte_offset <= source.len(),
                "byte offset {byte_offset} past the end of a {}-byte source: {source:?}",
                source.len()
            );
            assert!(
                source.is_char_boundary(byte_offset),
                "byte offset {byte_offset} is not a UTF-8 boundary: {source:?}"
            );
            assert!(
                error.offset() <= source.encode_utf16().count(),
                "UTF-16 offset past the end of the source: {source:?}"
            );
            // The formatted message is what an embedder surfaces.
            let _ = error.to_string();
        }
    }
    ok
}

const VALID: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<lynx engine-version=\"3.4\">\n",
    "  <!-- a comment -->\n",
    "  <style>.card { width: 100px; }</style>\n",
    "  <script thread=\"main\"><![CDATA[ function render() {} ]]></script>\n",
    "  <script thread=\"background\">globalThis.x = 1;</script>\n",
    "</lynx>\n",
);

/// Seeds. Mutation starts from shapes that already reach deep into the
/// grammar; starting from noise would spend the whole budget failing the
/// `<?xml` prologue.
fn seeds() -> Vec<String> {
    vec![
        VALID.to_owned(),
        "<lynx engine-version=\"1\"><script thread=\"main\"></script></lynx>".to_owned(),
        "\u{feff}<lynx engine-version=\"1\"><script thread=\"main\">x</script></lynx>".to_owned(),
        "<?xml?><lynx engine-version=\"\"><script thread=\"main\"/></lynx>".to_owned(),
        "<lynx><style></style></lynx>".to_owned(),
        String::new(),
    ]
}

/// Characters worth splicing in: every delimiter the grammar switches on, plus
/// the multi-byte and boundary cases that make an offset bug observable.
const INTERESTING: &[char] = &[
    '<', '>', '/', '?', '!', '-', '[', ']', '"', '\'', '=', ' ', '\n', '\t', '\0', '\u{feff}', 'é',
    '中', '𝄞', 'x',
];

/// xorshift64*, so a failure reproduces from the seed printed with it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        // `bound` came from a `usize` and the modulus keeps the result under
        // it, so this narrows back into the range it started in — on a 32-bit
        // target too.
        usize::try_from(self.next() % bound as u64).expect("modulus keeps this under `bound`")
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// One mutation, at character granularity so the result is always valid UTF-8 —
/// which is what `parse` takes, so byte-level mutation would only manufacture
/// inputs the signature cannot express.
fn mutate(source: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    match rng.below(6) {
        0 if !chars.is_empty() => {
            chars.remove(rng.below(chars.len()));
        }
        1 => {
            let at = rng.below(chars.len() + 1);
            chars.insert(at, *rng.pick(INTERESTING));
        }
        2 if !chars.is_empty() => {
            let at = rng.below(chars.len());
            chars[at] = *rng.pick(INTERESTING);
        }
        3 => {
            chars.truncate(rng.below(chars.len() + 1));
        }
        4 if !chars.is_empty() => {
            // Duplicate a run: the cheapest way to reach repeated delimiters
            // and unbalanced nesting.
            let start = rng.below(chars.len());
            let end = start + rng.below(chars.len() - start + 1);
            let run: Vec<char> = chars[start..end].to_vec();
            let at = rng.below(chars.len() + 1);
            chars.splice(at..at, run);
        }
        _ => {
            chars.reverse();
        }
    }
    chars.into_iter().collect()
}

/// Enough to walk the grammar's branches many times over; the whole test runs
/// in well under a tenth of a second, so it stays in the ordinary suite.
const ITERATIONS: usize = 20_000;

#[test]
fn arbitrary_source_is_rejected_rather_than_fatal() {
    let mut rng = Rng(0x5eed_1ea5_f00d_c0de);
    let seeds = seeds();
    let mut current = seeds[0].clone();
    let mut accepted = 0usize;

    for iteration in 0..ITERATIONS {
        // Restart from a seed periodically so the search does not wander into
        // one shape and stay there.
        if iteration % 64 == 0 {
            current = rng.pick(&seeds).clone();
        }
        current = mutate(&current, &mut rng);
        // Keep inputs small: what is under test is the grammar's branches, not
        // throughput, and an unbounded walk would only grow the string.
        if current.chars().count() > 2048 {
            current.truncate(
                current
                    .char_indices()
                    .nth(512)
                    .map_or(0, |(index, _)| index),
            );
        }
        if check(&current) {
            accepted += 1;
        }
    }

    // A floor, not a target. The mutations are aggressive enough that almost
    // everything is rejected, which is fine — the offset invariants are
    // checked on the rejections. But if a grammar change made *nothing* parse,
    // the `Ok` branch above would quietly stop being exercised and this test
    // would still pass. The generator is fixed-seed, so this count is exact;
    // 5 leaves room for the grammar to move without going flaky.
    assert!(
        accepted >= 5,
        "only {accepted} of {ITERATIONS} mutated inputs parsed — \
         the Ok-branch invariants are no longer being exercised"
    );
}

/// Shapes worth naming, rather than hoping the search reaches them: every
/// construct with a terminator is a chance to run off the end of the source.
#[test]
fn unterminated_and_degenerate_constructs_are_rejected() {
    for source in [
        "",
        "\u{feff}",
        "<",
        "<?",
        "<?xml",
        "<?xml version=\"1.0\"",
        "<!--",
        "<!-- unterminated",
        "<lynx",
        "<lynx>",
        "<lynx engine-version=",
        "<lynx engine-version=\"",
        "<lynx engine-version=\"1\">",
        "<lynx engine-version=\"1\"><script",
        "<lynx engine-version=\"1\"><script thread=\"main\">",
        "<lynx engine-version=\"1\"><script thread=\"main\"><![CDATA[",
        "<lynx engine-version=\"1\"><script thread=\"main\"><![CDATA[]]",
        "</lynx>",
        "<lynx engine-version=\"1\"></lynx>",
        // Multi-byte characters at every position an offset could be computed.
        "中<lynx engine-version=\"中\"><script thread=\"main\">中</script></lynx>",
        "𝄞",
        "<lynx engine-version=\"𝄞\">",
        // Nesting the terminators the scanner searches for.
        "<lynx engine-version=\"1\"><!--<script thread=\"main\">--></lynx>",
        "<lynx engine-version=\"1\"><script thread=\"main\"><![CDATA[</script>]]></script></lynx>",
    ] {
        let _ = check(source);
    }
}

/// The BOM and the declaration are both optional prefixes; each combination
/// shifts every later offset, which is where an off-by-one would surface.
#[test]
fn optional_prefixes_do_not_shift_reported_offsets_off_a_boundary() {
    let body = "<lynx engine-version=\"1\"><script thread=\"main\">x</script></lynx>";
    for prefix in [
        "",
        "\u{feff}",
        "<?xml version=\"1.0\"?>",
        "\u{feff}<?xml version=\"1.0\"?>",
        "\u{feff}<?xml version=\"1.0\"?><!-- 中 -->",
    ] {
        let _ = check(&format!("{prefix}{body}"));
        // And the same prefixes in front of something that must fail.
        let _ = check(&format!("{prefix}<lynx engine-version=\"1\">"));
    }
}
