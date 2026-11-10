//! Tests of the bytecode tables, the verifying read and the regular
//! expression compiler, on bytecode the fork itself wrote.

use crate::bytecode_abi::{
    ATOMS, AtomKind, BC_VERSION, JS_ATOM_END, OP_COUNT, OPCODES, OperandFormat,
    builtin_string_atom, export_type, op, tag,
};
use crate::ffi;
use crate::implementation::{Context, ErrorKind, EvalOptions, EvalSource, Runtime, Value};

fn realm() -> (Runtime, Context) {
    let runtime = Runtime::new().expect("runtime should initialize");
    let realm = runtime.create_context().expect("realm should initialize");
    (runtime, realm)
}

fn text(value: &Value) -> String {
    String::from_utf16(&value.to_utf16().expect("a string")).expect("well-formed")
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

// ---------------------------------------------------------------------------
// Tables

#[test]
fn atom_table_equals_a_new_runtimes_atoms() {
    // SAFETY: the function makes and frees a runtime of its own.
    let mismatch = unsafe { ffi::qjs_bytecode_abi_atom_mismatch() };
    assert_eq!(mismatch, -1, "first built-in atom that differs");
}

#[test]
fn tables_have_the_shapes_the_format_needs() {
    assert_eq!(OPCODES.len(), OP_COUNT);
    assert_eq!(OPCODES[usize::from(op::INVALID)].name, "invalid");
    assert_eq!(OPCODES[usize::from(op::GOTO)].name, "goto");
    assert_eq!(OPCODES[usize::from(op::GOTO)].size, 5);
    assert_eq!(OPCODES[usize::from(op::GOTO)].format, OperandFormat::Label);
    assert_eq!(
        OPCODES[usize::from(op::WITH_GET_VAR)].format,
        OperandFormat::AtomLabelU8
    );
    assert_eq!(OPCODES[usize::from(op::CALL0)].format, OperandFormat::Npopx);
    // The short opcodes come after `nop`, numbered from it.
    assert_eq!(op::PUSH_MINUS1, op::NOP + 1);
    for opcode in &OPCODES {
        assert!(opcode.size >= 1, "{}", opcode.name);
    }

    assert_eq!(ATOMS.len(), JS_ATOM_END as usize);
    assert_eq!(ATOMS[0].kind, AtomKind::Null);
    assert_eq!(ATOMS[1].text, "null");
    let private: Vec<_> = ATOMS
        .iter()
        .filter(|atom| atom.kind == AtomKind::PrivateSymbol)
        .map(|atom| atom.text)
        .collect();
    assert_eq!(private, ["<brand>"]);
    let length = builtin_string_atom("length").expect("a built-in atom");
    assert_eq!(ATOMS[length as usize].text, "length");
    // Symbols are not found by text, so a string with a symbol's
    // description interns as a new atom.
    assert_eq!(builtin_string_atom("Symbol.iterator"), None);
    let mut texts = std::collections::HashSet::new();
    for atom in ATOMS.iter().filter(|atom| atom.kind == AtomKind::String) {
        assert!(
            texts.insert(atom.text),
            "two string atoms read {}",
            atom.text
        );
    }

    let (_runtime, realm) = realm();
    let bytes = realm.compile_script_bytecode("version.js", "1").unwrap();
    assert_eq!(bytes[0], BC_VERSION);
}

// ---------------------------------------------------------------------------
// A reader of just enough of the format to find a function's fields.

fn leb128(bytes: &[u8], position: &mut usize) -> u32 {
    let mut value = 0u32;
    for shift in 0..5 {
        let byte = bytes[*position];
        *position += 1;
        value |= u32::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("leb128 longer than five bytes");
}

/// Where the parts of the first function record of a bytecode file are.
struct FunctionLayout {
    /// The offset of the stack size field and its encoded length.
    stack_size: (usize, usize),
    /// The offset and length of the instructions.
    code: (usize, usize),
}

/// Finds the first `FUNCTION_BYTECODE` record of a script (the top level
/// value) or of a module (its function, after the module record).
fn first_function(bytes: &[u8]) -> FunctionLayout {
    assert_eq!(bytes[0], BC_VERSION);
    let mut position = 1;
    let atom_count = leb128(bytes, &mut position);
    for _ in 0..atom_count {
        let header = leb128(bytes, &mut position);
        position += ((header >> 1) as usize) << (header & 1);
    }
    if bytes[position] == tag::MODULE {
        position += 1;
        leb128(bytes, &mut position); // name
        let requests = leb128(bytes, &mut position);
        for _ in 0..requests {
            leb128(bytes, &mut position); // name
            assert_eq!(
                bytes[position],
                tag::UNDEFINED,
                "attributes in a test module"
            );
            position += 1;
        }
        for _ in 0..leb128(bytes, &mut position) {
            let export_type = bytes[position];
            position += 1;
            leb128(bytes, &mut position); // variable or module index
            if export_type != export_type::LOCAL {
                leb128(bytes, &mut position); // local name
            }
            leb128(bytes, &mut position); // export name
        }
        for _ in 0..leb128(bytes, &mut position) {
            leb128(bytes, &mut position); // module index
        }
        for _ in 0..leb128(bytes, &mut position) {
            leb128(bytes, &mut position); // variable index
            position += 1; // is_star
            leb128(bytes, &mut position); // import name
            leb128(bytes, &mut position); // module index
        }
        position += 1; // has_tla
    }
    assert_eq!(bytes[position], tag::FUNCTION_BYTECODE);
    position += 1 + 2 + 1; // tag, flags, js_mode
    leb128(bytes, &mut position); // name
    for _ in 0..3 {
        leb128(bytes, &mut position); // arg, var, defined arg counts
    }
    let stack_size_start = position;
    leb128(bytes, &mut position);
    let stack_size = (stack_size_start, position - stack_size_start);
    leb128(bytes, &mut position); // var_ref_count
    let closure_var_count = leb128(bytes, &mut position);
    leb128(bytes, &mut position); // cpool_count
    let code_length = leb128(bytes, &mut position) as usize;
    let local_count = leb128(bytes, &mut position);
    for _ in 0..local_count {
        leb128(bytes, &mut position); // name
        leb128(bytes, &mut position); // scope_next
        leb128(bytes, &mut position); // var_ref_idx
        position += 1; // flags
    }
    for _ in 0..closure_var_count {
        leb128(bytes, &mut position); // name
        leb128(bytes, &mut position); // var_idx
        position += 2; // flags
    }
    FunctionLayout {
        stack_size,
        code: (position, code_length),
    }
}

/// The instruction offsets of a function's code, by the opcode table.
fn instructions(bytes: &[u8], layout: &FunctionLayout) -> Vec<usize> {
    let (start, length) = layout.code;
    let mut offsets = Vec::new();
    let mut position = start;
    while position < start + length {
        offsets.push(position);
        position += usize::from(OPCODES[usize::from(bytes[position])].size);
    }
    assert_eq!(position, start + length, "the code decodes to its length");
    offsets
}

fn find_opcode(bytes: &[u8], layout: &FunctionLayout, opcode: u8) -> usize {
    instructions(bytes, layout)
        .into_iter()
        .find(|&offset| bytes[offset] == opcode)
        .unwrap_or_else(|| panic!("no {} in the code", OPCODES[usize::from(opcode)].name))
}

// ---------------------------------------------------------------------------
// The verifying read accepts what the fork writes

/// Scripts whose completion value is a string.
const SCRIPTS: &[&str] = &[
    "String(1 + 2)",
    "function outer(a) { let b = a * 2; return () => a + b; } String(outer(5)())",
    "class A { #x = 1; static s = 'S'; get x() { return this.#x; } \
     m(...rest) { return rest.length; } }\n\
     class B extends A { constructor() { super(); this.y = super.m(1, 2); } }\n\
     const b = new B(); [b.x, b.y, A.s, b instanceof A].join()",
    "function* g(n) { for (let i = 0; i < n; i++) yield i * i; return 'end'; }\n\
     function* h() { const r = yield* g(4); yield r; }\n\
     [...h()].join()",
    "let log = [];\n\
     async function f(x) { await null; log.push(x); return x + 1; }\n\
     async function* ag() { yield 1; yield await f(2); }\n\
     (async () => { for await (const v of ag()) log.push(v); })();\n\
     'scheduled'",
    "function t(k) { const out = []; try { out.push('try'); if (k) throw new Error('e'); \
     out.push('no-throw'); } catch (e) { out.push(e.message); } finally { out.push('fin'); } \
     return out.join(); }\n\
     function u() { for (const x of [1, 2, 3]) { try { if (x === 2) return 'r' + x; } \
     finally { globalThis.fin = (globalThis.fin || 0) + 1; } } }\n\
     [t(0), t(1), u(), globalThis.fin].join('|')",
    "let s = 0; for (const [k, v] of Object.entries({a: 1, b: 2})) s += k.length + v;\n\
     for (const x in {p: 1, q: 2}) s += x.length;\n\
     const {a, b: [c, ...d], ...rest} = {a: 1, b: [2, 3, 4], e: 5, f: 6};\n\
     [s, a, c, d.join(), Object.keys(rest).join()].join()",
    "var o = {p: 'from-with', q: 1};\n\
     function w() { with (o) { q = q + 1; return p + q; } }\n\
     w() + typeof p",
    "const re = /(\\d+)-(?<word>[a-z]+)/gi;\n\
     const m = re.exec('x 12-abc 34-DEF');\n\
     [m[1], m.groups.word, 'aXbXc'.split(/x/i).join(), /é+/u.test('éé')].join()",
    "let n = 0; outer: for (let i = 0; i < 5; i++) { for (let j = 0; j < 5; j++) { \
     if (j === 3) continue outer; if (i === 4) break outer; n += i * j; } }\n\
     switch (n) { case 18: n = 'eighteen'; break; default: n = 'other'; }\n\
     String(n) + `-${n.length}-${typeof 10n}`",
];

#[test]
fn verifying_read_accepts_and_runs_what_the_fork_writes_for_scripts() {
    for (index, source) in SCRIPTS.iter().enumerate() {
        let (_runtime, direct_realm) = realm();
        let direct = direct_realm
            .evaluate(EvalSource::new(source), EvalOptions::default())
            .unwrap_or_else(|error| panic!("script {index}: {error}"));

        let (_compiler, compiler) = realm();
        let name = format!("script{index}.js");
        let bytes = compiler.compile_script_bytecode(&name, source).unwrap();

        let (_runtime, checker) = realm();
        let rewritten = checker
            .verify_bytecode(&bytes)
            .unwrap_or_else(|error| panic!("script {index} refused: {error}"));
        // Every stack size the read computed equals the compiler's.
        assert_eq!(rewritten, bytes, "script {index}");

        let (_runtime, runner) = realm();
        let from_bytecode = runner
            .evaluate_script_bytecode(&bytes)
            .unwrap_or_else(|error| panic!("script {index} from bytecode: {error}"));
        assert_eq!(text(&from_bytecode), text(&direct), "script {index}");
    }
}

/// Modules whose export `result` is a string once evaluated.
const MODULES: &[&str] = &[
    "export const result = [1, 2, 3].map((x) => x * 2).join();",
    "let count = 0;\n\
     export function bump() { return ++count; }\n\
     class Counter { static #made = 0; constructor() { Counter.#made++; } \
     static get made() { return Counter.#made; } }\n\
     new Counter(); new Counter();\n\
     export const result = [bump(), bump(), Counter.made, import.meta.url].join();",
    "const wait = (value) => new Promise((resolve) => resolve(value));\n\
     const first = await wait('a');\n\
     async function* letters() { yield first; yield await wait('b'); }\n\
     const seen = [];\n\
     for await (const letter of letters()) seen.push(letter);\n\
     export const result = seen.join() + /b(?=c)/.exec('abc').index;",
    "function* fib() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } }\n\
     const out = [];\n\
     for (const v of fib()) { if (v > 50) break; out.push(v); }\n\
     let finished = false;\n\
     try { throw new TypeError('t'); } catch ({ message }) { finished = message; } \
     finally { out.push('f'); }\n\
     export const result = out.join() + finished;",
];

fn evaluated_result(runtime: &mut Runtime, realm: &mut Context, name: &str) -> String {
    let promise = realm.load_module(name).expect("the load starts");
    runtime.drain_pending_jobs(realm).expect("jobs settle");
    realm
        .settled_promise_result(&promise)
        .expect("the module evaluates")
        .expect("the load has settled");
    let namespace = realm.module_namespace(name).unwrap();
    let result = realm.property(&namespace, "result").unwrap();
    text(&result)
}

#[test]
fn verifying_read_accepts_and_runs_what_the_fork_writes_for_modules() {
    for (index, source) in MODULES.iter().enumerate() {
        let name = format!("app:///module{index}.js");

        let (mut runtime, mut direct) = realm();
        direct
            .complete_module(&name, Ok((name.as_str(), source)))
            .unwrap();
        let expected = evaluated_result(&mut runtime, &mut direct, &name);

        let (_compiler, compiler) = realm();
        let bytes = compiler.compile_module_bytecode(&name, source).unwrap();

        let (_runtime, checker) = realm();
        assert_eq!(
            checker.verify_bytecode(&bytes).unwrap(),
            bytes,
            "module {index}"
        );

        let (mut runtime, mut realm) = realm();
        realm
            .complete_module_bytecode(&name, &name, &bytes)
            .unwrap_or_else(|error| panic!("module {index} refused: {error}"));
        assert_eq!(
            evaluated_result(&mut runtime, &mut realm, &name),
            expected,
            "module {index}"
        );
    }
}

#[test]
fn modules_completed_as_bytecode_import_each_other() {
    let (_compiler, compiler) = realm();
    let dependency = compiler
        .compile_module_bytecode(
            "app:///lib/dep.js",
            "export const where = import.meta.url; export default 40;",
        )
        .unwrap();
    let entry = compiler
        .compile_module_bytecode(
            "app:///lib/entry.js",
            "import value, { where } from './dep.js';\n\
             export const result = `${value + 2} ${where}`;",
        )
        .unwrap();

    let (mut runtime, mut realm) = realm();
    realm
        .complete_module_bytecode("app:///lib/dep.js", "https://cdn.test/dep.js", &dependency)
        .unwrap();
    realm
        .complete_module_bytecode("app:///lib/entry.js", "https://cdn.test/entry.js", &entry)
        .unwrap();
    assert_eq!(
        evaluated_result(&mut runtime, &mut realm, "app:///lib/entry.js"),
        "42 https://cdn.test/dep.js"
    );
}

// ---------------------------------------------------------------------------
// The verifying read refuses

const BRANCHY: &str = "var r = globalThis.c ? 1 : 2; String(r)";

fn compiled_script(source: &str) -> Vec<u8> {
    let (_runtime, compiler) = realm();
    compiler
        .compile_script_bytecode("patched.js", source)
        .unwrap()
}

fn refusal(bytes: &[u8]) -> String {
    let (_runtime, checker) = realm();
    let error = checker
        .verify_bytecode(bytes)
        .expect_err("the patched bytecode is refused");
    assert_eq!(error.kind, ErrorKind::BytecodeRejected, "{error:?}");
    error.message
}

#[test]
fn an_invalid_opcode_is_refused() {
    let original = compiled_script(BRANCHY);
    let layout = first_function(&original);
    for opcode in [op::INVALID, u8::try_from(OP_COUNT).unwrap(), u8::MAX] {
        let mut bytes = original.clone();
        bytes[layout.code.0] = opcode;
        let message = refusal(&bytes);
        assert!(message.contains("invalid opcode"), "{message}");
    }
}

#[test]
fn an_instruction_running_past_the_end_is_refused() {
    let mut bytes = compiled_script(BRANCHY);
    let layout = first_function(&bytes);
    let last = *instructions(&bytes, &layout).last().unwrap();
    assert_eq!(bytes[last], op::RETURN);
    // A one-byte instruction becomes a five-byte one.
    bytes[last] = op::PUSH_I32;
    let message = refusal(&bytes);
    assert!(message.contains("truncated instruction"), "{message}");
}

#[test]
fn a_stack_underflow_is_refused() {
    let mut bytes = compiled_script(BRANCHY);
    let layout = first_function(&bytes);
    // The first instruction runs at depth 0: make it a `drop`, padding the
    // rest of its bytes with `nop`s so the stream still decodes.
    let first = layout.code.0;
    let size = usize::from(OPCODES[usize::from(bytes[first])].size);
    bytes[first] = op::DROP;
    bytes[first + 1..first + size].fill(op::NOP);
    let message = refusal(&bytes);
    assert!(message.contains("stack underflow"), "{message}");
}

#[test]
fn different_depths_where_paths_meet_are_refused() {
    let mut bytes = compiled_script(BRANCHY);
    let layout = first_function(&bytes);
    // `c ? 1 : 2`: one arm pushes nothing, so the arms meet one apart.
    let arm = find_opcode(&bytes, &layout, op::PUSH_2);
    bytes[arm] = op::NOP;
    let message = refusal(&bytes);
    assert!(message.contains("inconsistent stack size"), "{message}");
}

#[test]
fn a_jump_outside_the_function_is_refused() {
    let original = compiled_script(BRANCHY);
    let layout = first_function(&original);
    let jump = instructions(&original, &layout)
        .into_iter()
        .find(|&offset| {
            matches!(
                OPCODES[usize::from(original[offset])].format,
                OperandFormat::Label8 | OperandFormat::Label16 | OperandFormat::Label
            )
        })
        .expect("a jump");
    let end = layout.code.0 + layout.code.1;
    for forward in [true, false] {
        let mut bytes = original.clone();
        match OPCODES[usize::from(bytes[jump])].format {
            OperandFormat::Label8 => bytes[jump + 1] = if forward { 0x7f } else { 0x80 },
            OperandFormat::Label16 => bytes[jump + 1..jump + 3]
                .copy_from_slice(&(if forward { i16::MAX } else { i16::MIN }).to_le_bytes()),
            _ => bytes[jump + 1..jump + 5]
                .copy_from_slice(&(if forward { i32::MAX } else { i32::MIN }).to_le_bytes()),
        }
        assert!(
            jump + 1 + 0x7f > end,
            "the forward target is outside the code"
        );
        let message = refusal(&bytes);
        assert!(message.contains("bytecode buffer overflow"), "{message}");
    }
}

#[test]
fn a_stack_size_too_small_is_accepted_and_corrected() {
    let original = compiled_script(BRANCHY);
    let layout = first_function(&original);
    let (offset, length) = layout.stack_size;
    assert_eq!(length, 1, "a one-byte stack size");
    assert!(original[offset] > 0);
    let mut bytes = original.clone();
    bytes[offset] = 0;

    let (_runtime, checker) = realm();
    assert_eq!(checker.verify_bytecode(&bytes).unwrap(), original);
    let (_runtime, runner) = realm();
    assert_eq!(text(&runner.evaluate_script_bytecode(&bytes).unwrap()), "2");
}

#[test]
fn completed_module_bytecode_is_verified_and_a_refusal_leaves_nothing() {
    let name = "app:///checked.js";
    let source = "const x = globalThis.c ? 1 : 2; export const result = String(x);";
    let (_compiler, compiler) = realm();
    let original = compiler.compile_module_bytecode(name, source).unwrap();
    let layout = first_function(&original);

    let (mut runtime, mut realm) = realm();
    let mut broken = original.clone();
    let arm = find_opcode(&broken, &layout, op::PUSH_2);
    broken[arm] = op::NOP;
    let error = realm
        .complete_module_bytecode(name, name, &broken)
        .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::BytecodeRejected, "{error:?}");

    let script = compiled_script("1");
    let error = realm
        .complete_module_bytecode(name, name, &script)
        .expect_err("a script is not a module");
    assert_eq!(error.kind, ErrorKind::BytecodeRejected, "{error:?}");

    // Nothing of either stayed: the name is still open, and the stored
    // stack size of the bytes that answer it is not used.
    let mut understated = original.clone();
    understated[layout.stack_size.0] = 0;
    realm
        .complete_module_bytecode(name, name, &understated)
        .unwrap();
    assert_eq!(evaluated_result(&mut runtime, &mut realm, name), "2");
}

// ---------------------------------------------------------------------------
// Mutations

/// `xorshift64`, so the run is the same every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).unwrap()).unwrap()
    }
}

#[test]
fn mutated_bytecode_is_read_or_refused_and_never_run() {
    let mut corpus = Vec::new();
    {
        let (_runtime, compiler) = realm();
        for (index, source) in SCRIPTS.iter().enumerate() {
            corpus.push(
                compiler
                    .compile_script_bytecode(&format!("s{index}.js"), source)
                    .unwrap(),
            );
        }
        for (index, source) in MODULES.iter().enumerate() {
            corpus.push(
                compiler
                    .compile_module_bytecode(&format!("app:///m{index}.js"), source)
                    .unwrap(),
            );
        }
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut accepted, mut refused) = (0usize, 0usize);
    let mut runtime = Runtime::new().unwrap();
    for round in 0..10_000 {
        let original = &corpus[round % corpus.len()];
        let mut bytes = original.clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(bytes.len());
            match rng.below(5) {
                0 => bytes[at] = rng.next().to_le_bytes()[0],
                1 => bytes[at] ^= 1 << rng.below(8),
                2 => bytes[at] = [0, 1, 0x7f, 0x80, 0xff][rng.below(5)],
                3 => bytes.truncate(at.max(1)),
                _ => {
                    let at = rng.below(bytes.len());
                    bytes.insert(at, rng.next().to_le_bytes()[0]);
                }
            }
        }
        // A fresh realm each time: a module that reads stays loaded in it.
        let checker = runtime.create_context().unwrap();
        match checker.verify_bytecode(&bytes) {
            Ok(_) => accepted += 1,
            Err(error) => {
                assert!(
                    matches!(
                        error.kind,
                        ErrorKind::BytecodeRejected | ErrorKind::OutOfMemory
                    ),
                    "{error:?}"
                );
                refused += 1;
            }
        }
        drop(checker);
        if round % 1000 == 0 {
            runtime.run_gc();
        }
    }
    runtime.run_gc();
    assert!(
        accepted > 0 && refused > 0,
        "{accepted} read, {refused} refused"
    );
}

// ---------------------------------------------------------------------------
// Regular expressions

/// Whether `haystack` holds `needle` as a string value: its tag, its
/// length header, then its bytes.
fn holds_string(haystack: &[u8], needle: &[u8]) -> bool {
    let mut encoded = vec![tag::STRING];
    let mut header = u32::try_from(needle.len()).unwrap() << 1;
    loop {
        let byte = u8::try_from(header & 0x7f).unwrap();
        header >>= 7;
        if header == 0 {
            encoded.push(byte);
            break;
        }
        encoded.push(byte | 0x80);
    }
    encoded.extend_from_slice(needle);
    haystack
        .windows(encoded.len())
        .any(|window| window == encoded.as_slice())
}

#[test]
fn regexp_bytecode_equals_what_a_literal_carries() {
    let (_runtime, realm) = realm();
    for (pattern, flags) in [
        ("a+b", "gi"),
        ("(\\d+)-(?<word>[a-z]+)", "y"),
        ("[\\u{1F600}-\\u{1F64F}]", "u"),
        ("é+[à-ÿ]", "i"),
        ("x", ""),
    ] {
        let compiled = realm
            .compile_regexp_bytecode(&utf16(pattern), flags)
            .unwrap();
        let script = realm
            .compile_script_bytecode("re.js", &format!("/{pattern}/{flags}"))
            .unwrap();
        assert!(
            holds_string(&script, &compiled),
            "/{pattern}/{flags} carries other bytes"
        );
        if pattern.contains('é') {
            assert!(compiled.iter().any(|&byte| byte >= 0x80), "{compiled:?}");
        }
    }
}

#[test]
fn regexp_syntax_errors_are_errors() {
    let (_runtime, realm) = realm();
    for (pattern, flags) in [
        ("(", ""),
        ("a", "gg"),
        ("a", "uv"),
        ("a", "q"),
        ("[b-a]", ""),
    ] {
        let error = realm
            .compile_regexp_bytecode(&utf16(pattern), flags)
            .expect_err("refused");
        assert_eq!(
            error.kind,
            ErrorKind::Syntax,
            "/{pattern}/{flags}: {error:?}"
        );
    }
}

#[test]
fn regexp_patterns_are_code_units() {
    let (_runtime, realm) = realm();
    let lone = realm.compile_regexp_bytecode(&[0xd800], "").unwrap();
    let replacement = realm.compile_regexp_bytecode(&[0xfffd], "").unwrap();
    // An unpaired surrogate is not U+FFFD to the compiler, which is why
    // the pattern is code units and not a `&str`.
    assert_ne!(lone, replacement);
}
