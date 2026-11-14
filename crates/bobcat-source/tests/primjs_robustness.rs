//! Input robustness of the `PrimJS` bytecode reader: every input is `Ok` or
//! `Err`, never a panic, a stack overflow or an allocation sized by an
//! unchecked count.
//!
//! The mutator starts from real encoder output (`tests/fixtures/primjs/`) and
//! the three empty-program fingerprints, so it reaches deep into function
//! records and instruction streams. Deliberately not a fuzzer, as in
//! `robustness.rs`: fixed seed, bounded, part of the ordinary suite. The
//! named cases below cover what the search is unlikely to reach.

use std::path::Path;

use bobcat_source::native::primjs::{
    self, Function, NESTING_LIMIT, Opcode, Operand, PrimjsError, Program, Value,
};

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/primjs")
}

/// Every function of the program, parents first.
fn functions(program: &Program) -> Vec<&Function> {
    fn walk<'a>(function: &'a Function, out: &mut Vec<&'a Function>) {
        out.push(function);
        for value in &function.cpool {
            if let Value::Function(child) = value {
                walk(child, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&program.root, &mut out);
    out
}

/// Reads `bytes` and checks what must hold for an accepted program. Returns
/// whether it was accepted.
fn check(bytes: &[u8]) -> bool {
    match primjs::read_payload(bytes) {
        Ok((_, program)) => {
            for function in functions(&program) {
                for instruction in &function.instructions {
                    assert!((instruction.pc as usize) < function.bytecode_len as usize);
                    for operand in instruction.operands() {
                        match *operand {
                            Operand::Local(index) => assert!(index < function.var_count),
                            Operand::Arg(index) => assert!(index < function.arg_count),
                            Operand::ClosureVar(index) => {
                                assert!(u32::from(index) < function.closure_var_count());
                            }
                            Operand::Const(index) => {
                                assert!((index as usize) < function.cpool.len());
                            }
                            Operand::Target(index) => {
                                assert!((index as usize) < function.instructions.len());
                            }
                            Operand::Atom(atom) => assert!(program.atom_text(atom).is_some()),
                            Operand::Immediate(_) => {}
                        }
                    }
                }
            }
            let _ = primjs::disassemble(&program);
            true
        }
        Err(error) => {
            let _ = error.to_string();
            false
        }
    }
}

fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(fixture_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("bin"))
        .collect();
    names.sort();
    for path in names {
        seeds.push(std::fs::read(path).unwrap());
    }
    seeds.push(vec![
        9, 204, 1, 176, 202, 3, 0, 0, 0, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 0, 194, 40,
        94, 1, 0,
    ]);
    seeds.push(vec![
        1, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0, 0, 194, 40, 94, 1, 12, 0,
        0, 128, 128, 128, 144, 128, 128, 128, 128, 128, 1,
    ]);
    seeds
}

/// xorshift64*, so a failure reproduces from the seed.
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
        usize::try_from(self.next() % bound as u64).expect("modulus keeps this under `bound`")
    }
}

/// LEB128 encodings of large counts, spliced over a byte to inflate whatever
/// count or index sits there.
const INFLATED: &[&[u8]] = &[
    &[0xff, 0xff, 0xff, 0xff, 0x0f],
    &[0xff, 0xff, 0x03],
    &[0x80, 0x80, 0x80, 0x80, 0x10],
    &[0xff, 0x7f],
    &[0xff],
];

fn mutate(bytes: &mut Vec<u8>, rng: &mut Rng) {
    match rng.below(7) {
        0 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1 << rng.below(8);
        }
        1 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            bytes[at] = (rng.next() & 0xff) as u8;
        }
        2 => bytes.truncate(rng.below(bytes.len() + 1)),
        3 if !bytes.is_empty() => {
            bytes.remove(rng.below(bytes.len()));
        }
        4 => {
            let at = rng.below(bytes.len() + 1);
            bytes.insert(at, (rng.next() & 0xff) as u8);
        }
        5 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            let inflated = INFLATED[rng.below(INFLATED.len())];
            bytes.splice(at..=at, inflated.iter().copied());
        }
        _ if !bytes.is_empty() => {
            let start = rng.below(bytes.len());
            let end = start + rng.below((bytes.len() - start).min(16) + 1);
            let run = bytes[start..end].to_vec();
            let at = rng.below(bytes.len() + 1);
            bytes.splice(at..at, run);
        }
        _ => {}
    }
}

const ITERATIONS: usize = 20_000;

#[test]
fn mutated_bytecode_is_rejected_rather_than_fatal() {
    let mut rng = Rng(0x5eed_b17e_c0de_0042);
    let seeds = seeds();
    assert!(seeds.len() > 50);
    let mut current = seeds[0].clone();
    let mut accepted = 0usize;
    for iteration in 0..ITERATIONS {
        if iteration % 16 == 0 {
            current = seeds[rng.below(seeds.len())].clone();
        }
        mutate(&mut current, &mut rng);
        if current.len() > 64 * 1024 {
            current.truncate(4096);
        }
        if check(&current) {
            accepted += 1;
        }
    }
    // Some single mutations leave a valid program (an immediate changed, a
    // pc2line byte flipped); if none did, the `Ok` branch would no longer
    // be exercised.
    assert!(
        accepted >= 100,
        "only {accepted} of {ITERATIONS} mutated inputs were accepted"
    );
}

#[test]
fn mutated_bundles_are_rejected_rather_than_fatal() {
    let bundle = std::fs::read(fixture_dir().join("sections.lynx.bundle")).unwrap();
    let mut rng = Rng(0x0b0d_1e5e_c710_0005);
    let mut accepted = 0usize;
    for _ in 0..5_000 {
        let mut current = bundle.clone();
        for _ in 0..=rng.below(3) {
            mutate(&mut current, &mut rng);
        }
        if let Ok(sections) = primjs::bundle_bytecode_sections(&current) {
            accepted += 1;
            for section in sections {
                let _ = check(section.bytes);
            }
        }
    }
    assert!(
        accepted >= 100,
        "only {accepted} mutated bundles were accepted"
    );
}

// Synthetic programs.

const PUSH_I32: u8 = Opcode::PushI32.number();
const RETURN: u8 = Opcode::Return.number();
const RETURN_UNDEF: u8 = Opcode::ReturnUndef.number();
const GET_VAR: u8 = Opcode::GetVar.number();
const GOTO: u8 = Opcode::Goto.number();
const MAKE_LOC_REF: u8 = Opcode::MakeLocRef.number();
const PUSH_CONST8: u8 = Opcode::PushConst8.number();
const GET_LOC0: u8 = Opcode::GetLoc0.number();
const GET_LOC1: u8 = Opcode::GetLoc1.number();
const GET_ARG0: u8 = Opcode::GetArg0.number();
const GET_VAR_REF0: u8 = Opcode::GetVarRef0.number();
const GOTO8: u8 = Opcode::Goto8.number();

fn leb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Record<'a> {
    arg_count: u32,
    var_count: u32,
    /// (serialized atom, `var_idx`, flags)
    closure_vars: &'a [(u32, u32, u8)],
    code: &'a [u8],
    cpool: &'a [Vec<u8>],
}

impl Default for Record<'_> {
    fn default() -> Self {
        Self {
            arg_count: 0,
            var_count: 1,
            closure_vars: &[],
            code: &[GET_LOC0, RETURN],
            cpool: &[],
        }
    }
}

fn record(record: &Record<'_>) -> Vec<u8> {
    let mut out = vec![13, 0x00, 0x02, 0];
    leb(0, &mut out); // anonymous
    for count in [
        record.arg_count,
        record.var_count,
        0,
        1,
        u32::try_from(record.closure_vars.len()).unwrap(),
        u32::try_from(record.cpool.len()).unwrap(),
        u32::try_from(record.code.len()).unwrap(),
        0,
    ] {
        leb(count, &mut out);
    }
    for &(atom, index, flags) in record.closure_vars {
        leb(atom, &mut out);
        leb(index, &mut out);
        out.push(flags);
    }
    out.extend_from_slice(record.code);
    for value in record.cpool {
        out.extend_from_slice(value);
    }
    out
}

fn program(root: &[u8]) -> Vec<u8> {
    let mut out = vec![1, 0];
    out.extend_from_slice(root);
    out
}

fn err(bytes: &[u8]) -> PrimjsError {
    match primjs::read_payload(bytes) {
        Ok(_) => panic!("accepted {:02x?}", &bytes[..bytes.len().min(64)]),
        Err(error) => error,
    }
}

#[test]
fn synthetic_baseline_is_accepted() {
    // The builder's default is the empty program; make sure the negative
    // cases below fail for the reason they name.
    assert!(check(&program(&record(&Record::default()))));
}

#[test]
fn degenerate_headers_are_rejected() {
    assert!(matches!(err(&[]), PrimjsError::UnexpectedEof { .. }));
    assert!(matches!(err(&[1]), PrimjsError::UnexpectedEof { .. }));
    assert!(matches!(err(&[9]), PrimjsError::UnexpectedEof { .. }));
    assert!(matches!(
        err(&[2, 0]),
        PrimjsError::UnknownVersion { version: 2 }
    ));
    assert!(matches!(
        err(&[0xff]),
        PrimjsError::UnknownVersion { version: 0xff }
    ));
    assert!(matches!(
        err(&[0x41, 0]),
        PrimjsError::ByteSwapped { version: 0x41 }
    ));
    assert!(matches!(
        err(&[0x49, 0]),
        PrimjsError::ByteSwapped { version: 0x49 }
    ));
    // Version word one below and one above the frozen value.
    for low in [0xCAB0_01CBu32, 0xCAB0_01CD, 0] {
        let mut bytes = vec![9];
        bytes.extend_from_slice(&u64::from(low).to_le_bytes());
        bytes.push(0);
        assert!(matches!(
            err(&bytes),
            PrimjsError::VersionWordMismatch { .. }
        ));
    }
    // A flag bit past the two known ones.
    for bit in [34, 40, 63] {
        let mut bytes = vec![9];
        bytes.extend_from_slice(&(0xCAB0_01CCu64 | 1 << bit).to_le_bytes());
        bytes.push(0);
        assert!(matches!(
            err(&bytes),
            PrimjsError::UnknownHeaderFlags { .. }
        ));
    }
    // 0xFF counts: an atom count of 2^32 - 1 with nothing after it.
    assert!(matches!(
        err(&[1, 0xff, 0xff, 0xff, 0xff, 0x0f]),
        PrimjsError::CountOutOfRange { what: "atom", .. }
    ));
    // A sixth LEB128 byte, and a fifth byte with bits above 32.
    assert!(matches!(
        err(&[1, 0x80, 0x80, 0x80, 0x80, 0x80, 0]),
        PrimjsError::BadLeb128 { .. }
    ));
    assert!(matches!(
        err(&[1, 0xff, 0xff, 0xff, 0xff, 0x1f]),
        PrimjsError::BadLeb128 { .. }
    ));
    // A string atom whose length runs past the end.
    assert!(matches!(
        err(&[1, 1, 0xfe, 0x01]),
        PrimjsError::UnexpectedEof { .. }
    ));
}

#[test]
fn rejected_tags_are_named() {
    assert!(matches!(
        err(&[1, 0, 14]),
        PrimjsError::Module { offset: 2 }
    ));
    assert!(matches!(
        err(&[1, 0, 11]),
        PrimjsError::BigFloat { offset: 2 }
    ));
    assert!(matches!(
        err(&[1, 0, 9, 0]),
        PrimjsError::RootNotFunction { tag: 9, .. }
    ));
    assert!(matches!(
        err(&[1, 0, 0]),
        PrimjsError::UnknownTag { tag: 0, .. }
    ));
    assert!(matches!(
        err(&[1, 0, 15]),
        PrimjsError::UnknownTag { tag: 15, .. }
    ));
    // The same tags inside a constant pool.
    for (tag, check) in [
        (
            11u8,
            (|error: &PrimjsError| matches!(error, PrimjsError::BigFloat { .. }))
                as fn(&PrimjsError) -> bool,
        ),
        (14, |error| matches!(error, PrimjsError::Module { .. })),
        (200, |error| {
            matches!(error, PrimjsError::UnknownTag { tag: 200, .. })
        }),
    ] {
        let bytes = program(&record(&Record {
            cpool: &[vec![tag]],
            ..Record::default()
        }));
        assert!(check(&err(&bytes)), "tag {tag}");
    }
    // Trailing bytes after the root function.
    let mut bytes = program(&record(&Record::default()));
    bytes.push(0);
    assert!(matches!(
        err(&bytes),
        PrimjsError::TrailingBytes { remaining: 1, .. }
    ));
}

#[test]
fn record_counts_are_checked() {
    // arg_count above 16 bits (PrimJS would truncate it).
    let bytes = program(&record(&Record {
        arg_count: 0x1_0000,
        ..Record::default()
    }));
    assert!(matches!(
        err(&bytes),
        PrimjsError::CountOutOfRange {
            what: "argument",
            ..
        }
    ));
    // A vardef count that is neither 0 nor arg_count + var_count.
    let mut root = record(&Record::default());
    let vardef_count_at = root.len() - 3;
    assert_eq!(root[vardef_count_at], 0);
    root[vardef_count_at] = 1;
    root.splice(vardef_count_at + 1..=vardef_count_at, [0, 0, 0, 0]);
    let mut doubled = root.clone();
    doubled[vardef_count_at] = 2;
    assert!(check(&program(&root)));
    assert!(matches!(
        err(&program(&doubled)),
        PrimjsError::CountOutOfRange { .. } | PrimjsError::InvalidRecord { .. }
    ));
    // Undefined function-flag bits.
    let mut root = record(&Record::default());
    root[2] |= 0x08;
    assert!(matches!(
        err(&program(&root)),
        PrimjsError::UnknownFlags {
            what: "function flags",
            ..
        }
    ));
}

#[test]
fn instruction_streams_are_validated() {
    let case =
        |code: &[u8], record: Record<'_>| err(&program(&self::record(&Record { code, ..record })));
    // A truncated last instruction: push_i32 needs four operand bytes.
    assert!(matches!(
        case(&[PUSH_I32, 0, 0], Record::default()),
        PrimjsError::TruncatedInstruction { pc: 0, .. }
    ));
    assert!(matches!(
        case(&[GET_LOC0, PUSH_I32, 0, 0, 0], Record::default()),
        PrimjsError::TruncatedInstruction { pc: 1, .. }
    ));
    // `invalid` and the non-shippable `inc_coverage`, and bytes above it.
    for opcode in [0u8, 242, 255] {
        assert!(matches!(
            case(&[opcode], Record::default()),
            PrimjsError::InvalidOpcode { opcode: o, .. } if o == opcode
        ));
    }
    // A jump into the middle of an instruction: goto8 lands on byte 2 of
    // push_i32.
    assert!(matches!(
        case(&[GOTO8, 3, PUSH_I32, 0, 0, 0, 0, RETURN], Record::default()),
        PrimjsError::BadJumpTarget {
            pc: 0,
            target: 4,
            ..
        }
    ));
    // The same jump to the instruction start is accepted.
    assert!(check(&program(&record(&Record {
        code: &[GOTO8, 1, PUSH_I32, 0, 0, 0, 0, RETURN],
        ..Record::default()
    }))));
    // Jumps before the start and past the end.
    assert!(matches!(
        case(&[GOTO8, 0x80, RETURN], Record::default()),
        PrimjsError::BadJumpTarget { target: -127, .. }
    ));
    assert!(matches!(
        case(&[GOTO, 0x10, 0, 0, 0, RETURN], Record::default()),
        PrimjsError::BadJumpTarget { target: 17, .. }
    ));
    // Implicit local index: get_loc1 with one local.
    assert!(matches!(
        case(&[GET_LOC1, RETURN], Record::default()),
        PrimjsError::IndexOutOfRange {
            kind: "local",
            index: 1,
            limit: 1,
            ..
        }
    ));
    // Implicit argument index: get_arg0 with no arguments.
    assert!(matches!(
        case(&[GET_ARG0, RETURN], Record::default()),
        PrimjsError::IndexOutOfRange {
            kind: "argument",
            index: 0,
            limit: 0,
            ..
        }
    ));
    // Implicit closure index: get_var_ref0 with no closure variables.
    assert!(matches!(
        case(&[GET_VAR_REF0, RETURN], Record::default()),
        PrimjsError::IndexOutOfRange {
            kind: "closure variable",
            ..
        }
    ));
    // Pool index: push_const8 0 with an empty pool.
    assert!(matches!(
        case(&[PUSH_CONST8, 0, RETURN], Record::default()),
        PrimjsError::IndexOutOfRange {
            kind: "constant pool",
            ..
        }
    ));
    // make_loc_ref's local index sits at offset 5.
    assert!(matches!(
        case(
            &[MAKE_LOC_REF, 1, 0, 0, 0, 5, 0, RETURN_UNDEF],
            Record::default()
        ),
        PrimjsError::IndexOutOfRange {
            kind: "local",
            index: 5,
            ..
        }
    ));
    // An atom operand past the file's (empty) atom table.
    assert!(matches!(
        case(&[GET_VAR, 203, 0, 0, 0, RETURN_UNDEF], Record::default()),
        PrimjsError::AtomOutOfRange { index: 203, .. }
    ));
    // A tagged-integer atom and a built-in atom are fine.
    for operand in [[202, 0, 0, 0], [5, 0, 0, 0x80]] {
        let mut code = vec![GET_VAR];
        code.extend_from_slice(&operand);
        code.push(RETURN_UNDEF);
        assert!(check(&program(&record(&Record {
            code: &code,
            ..Record::default()
        }))));
    }
}

#[test]
fn child_closure_variables_are_checked_against_the_parent() {
    // A child naming local 1 of a parent with one local.
    let child = record(&Record {
        closure_vars: &[(0, 1, 0x01)],
        ..Record::default()
    });
    let bytes = program(&record(&Record {
        cpool: &[child],
        ..Record::default()
    }));
    assert!(matches!(
        err(&bytes),
        PrimjsError::ClosureVarOutOfRange {
            kind: "local",
            index: 1,
            limit: 1,
            ..
        }
    ));
    // Local 0 is fine; argument 0 and the parent's closure variable 0 are not.
    for (flags, ok) in [(0x01u8, true), (0x03, false), (0x00, false)] {
        let child = record(&Record {
            closure_vars: &[(0, 0, flags)],
            ..Record::default()
        });
        let bytes = program(&record(&Record {
            cpool: &[child],
            ..Record::default()
        }));
        assert_eq!(check(&bytes), ok, "flags {flags:#x}");
    }
}

#[test]
fn deep_nesting_is_bounded() {
    // Nested arrays in a pool.
    for (depth, ok) in [
        (NESTING_LIMIT - 2, true),
        (NESTING_LIMIT * 4, false),
        (100_000, false),
    ] {
        // Each array holds one element; the innermost holds `null`.
        let mut nested = Vec::new();
        for _ in 0..depth {
            nested.extend_from_slice(&[9, 1]);
        }
        nested.push(1);
        let bytes = program(&record(&Record {
            cpool: &[nested],
            ..Record::default()
        }));
        if ok {
            assert!(check(&bytes), "depth {depth}");
        } else {
            assert!(
                matches!(err(&bytes), PrimjsError::NestingTooDeep { .. }),
                "depth {depth}"
            );
        }
    }
    // Nested functions: each holds the next in its pool.
    for (depth, ok) in [
        (NESTING_LIMIT, true),
        (NESTING_LIMIT + 1, false),
        (10_000, false),
    ] {
        let mut function = record(&Record::default());
        for _ in 1..depth {
            function = record(&Record {
                cpool: &[function],
                ..Record::default()
            });
        }
        let bytes = program(&function);
        if ok {
            assert!(check(&bytes), "depth {depth}");
        } else {
            assert!(
                matches!(err(&bytes), PrimjsError::NestingTooDeep { .. }),
                "depth {depth}"
            );
        }
    }
}

#[test]
fn packed_headers_are_checked() {
    let inner = program(&record(&Record::default()));
    let packed = |version: u32, offset: u32, size: u32, body: &[u8]| {
        let mut bytes = Vec::new();
        for word in [primjs::PACKED_MAGIC, version, offset, size, 3, 2] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(body);
        bytes
    };
    let size = u32::try_from(inner.len()).unwrap();
    let good = packed(1, 24, size, &inner);
    let (header, _) = primjs::read_payload(&good).unwrap();
    assert_eq!(
        header.map(|header| (header.target_sdk_major, header.target_sdk_minor)),
        Some((3, 2))
    );
    for bytes in [
        packed(2, 24, size, &inner),
        packed(1, 24, size + 1, &inner),
        packed(1, 24, size - 1, &inner),
        packed(1, 0, size + 24, &inner),
        packed(1, u32::MAX, size, &inner),
        packed(1, 24, u32::MAX, &inner),
        good[..20].to_vec(),
    ] {
        assert!(matches!(err(&bytes), PrimjsError::BadPackedHeader { .. }));
    }
}
