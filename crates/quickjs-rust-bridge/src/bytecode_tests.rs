//! Tests of the bytecode tables, the verifying read and the regular
//! expression compiler, on bytecode the fork itself wrote.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{CheckError, Rule, VerifiedModule, Verifier};
use crate::bytecode_abi::{
    ATOMS, AtomKind, BC_VERSION, JS_ATOM_END, OP_COUNT, OPCODES, OperandFormat,
    builtin_string_atom, export_type, op, tag,
};
use crate::ffi;
use crate::implementation::{
    Context, ErrorKind, EvalOptions, EvalSource, RequiredBody, RequiredKind, RequiredSource,
    Runtime, SourceType, Value,
};

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

/// `bytes` through a verifier of its own, which must accept them as they
/// are: what the fork writes is what the bridge's writer writes again.
fn verified(bytes: &[u8]) -> VerifiedModule {
    let module = Verifier::new()
        .module_from_bytes(bytes)
        .unwrap_or_else(|error| panic!("the verifier refuses the fork's module: {error}"));
    assert_eq!(
        module.bytes(),
        bytes,
        "verified bytes differ from the fork's"
    );
    module
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
pub(crate) const SCRIPTS: &[&str] = &[
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
pub(crate) const MODULES: &[&str] = &[
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
            .complete_module_bytecode(&name, &name, &verified(&bytes))
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
        .complete_module_bytecode(
            "app:///lib/dep.js",
            "https://cdn.test/dep.js",
            &verified(&dependency),
        )
        .unwrap();
    realm
        .complete_module_bytecode(
            "app:///lib/entry.js",
            "https://cdn.test/entry.js",
            &verified(&entry),
        )
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

/// The fork's verifying read's own refusal, without the Rust checks in
/// front of it (`crate::bytecode`, whose own tests cover them).
fn refusal(bytes: &[u8]) -> String {
    let (_runtime, checker) = realm();
    let error = checker
        .verify_bytecode_read_only(bytes)
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
fn module_bytecode_the_verifier_refuses_never_becomes_a_module() {
    let name = "app:///checked.js";
    let source = "const x = globalThis.c ? 1 : 2; export const result = String(x);";
    let (_compiler, compiler) = realm();
    let original = compiler.compile_module_bytecode(name, source).unwrap();
    let layout = first_function(&original);
    let verifier = Verifier::new();

    // Paths that meet with different depths: a rule of the verifier.
    let mut broken = original.clone();
    let arm = find_opcode(&broken, &layout, op::PUSH_2);
    broken[arm] = op::NOP;
    match verifier.module_from_bytes(&broken) {
        Err(CheckError::Verify(error)) => assert_eq!(error.rule, Rule::Merge, "{error}"),
        other => panic!("refused by a rule: {other:?}"),
    }

    // A script is not a module.
    let script = compiled_script("1");
    assert!(
        matches!(
            verifier.module_from_bytes(&script),
            Err(CheckError::Root("a module"))
        ),
        "a script is not a module"
    );

    // Bytes that do not read.
    assert!(matches!(
        verifier.module_from_bytes(&original[..original.len() / 2]),
        Err(CheckError::Read(_))
    ));

    // The stored stack size of the bytes is not used: the verifier keeps
    // it, and the realm's read computes its own.
    let (mut runtime, mut realm) = realm();
    let mut understated = original.clone();
    understated[layout.stack_size.0] = 0;
    let module = verifier.module_from_bytes(&understated).unwrap();
    assert_eq!(module.bytes(), understated.as_slice());
    realm.complete_module_bytecode(name, name, &module).unwrap();
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
// Module bytecode as a realm-local source: verified before it is supplied,
// read with the fork's verifying read when compiled

/// Module bytecode the fork compiles for `source` under `name`.
fn module_bytecode(name: &str, source: &str) -> Vec<u8> {
    let (_runtime, compiler) = realm();
    compiler.compile_module_bytecode(name, source).unwrap()
}

/// The fork's module bytecode for `source` with the instruction the fork's
/// stack walk refuses (an arm of a conditional made to push nothing), which
/// the verifier refuses too: only the unchecked entries store it.
fn refused_module_bytecode(name: &str) -> Vec<u8> {
    let source = "const x = globalThis.c ? 1 : 2; export const result = String(x);";
    let mut bytes = module_bytecode(name, source);
    let layout = first_function(&bytes);
    let arm = find_opcode(&bytes, &layout, op::PUSH_2);
    bytes[arm] = op::NOP;
    bytes
}

/// A realm whose synchronous loader (`bobcat:module`'s `loadModuleSync`)
/// answers from `files`.
fn sync_realm(files: Vec<(&str, RequiredSource)>) -> (Runtime, Context) {
    let runtime = Runtime::new().unwrap();
    let mut realm = runtime.create_context().unwrap();
    let files: Rc<RefCell<HashMap<String, RequiredSource>>> = Rc::new(RefCell::new(
        files
            .into_iter()
            .map(|(url, source)| (url.to_owned(), source))
            .collect(),
    ));
    realm
        .register_synchronous_loader("bobcat:module", "loadModuleSync", move |url| {
            files
                .borrow()
                .get(url)
                .cloned()
                .ok_or_else(|| format!("no such file: {url}"))
        })
        .unwrap();
    (runtime, realm)
}

fn bytecode_answer(url: &str, module: VerifiedModule) -> RequiredSource {
    RequiredSource {
        url: url.to_owned(),
        body: RequiredBody::ModuleBytecode(module),
        kind: RequiredKind::Module,
    }
}

fn text_module(url: &str, text: &str) -> RequiredSource {
    RequiredSource {
        url: url.to_owned(),
        body: RequiredBody::Text(text.to_owned()),
        kind: RequiredKind::Module,
    }
}

/// Runs `body` as the module `app:///entry.js` to settlement and answers
/// `globalThis.out`, or the error the evaluation rejected with.
fn run_entry(runtime: &mut Runtime, realm: &mut Context, body: &str) -> Result<String, String> {
    let promise = realm
        .evaluate(
            EvalSource {
                text: body,
                name: Some("app:///entry.js"),
                line_offset: 0,
            },
            EvalOptions {
                source_type: SourceType::Module,
                ..EvalOptions::default()
            },
        )
        .map_err(|error| error.message)?;
    runtime.drain_pending_jobs(realm).unwrap();
    realm
        .settled_promise_result(&promise)
        .map_err(|error| format!("{}: {}", error.name.unwrap_or_default(), error.message))?
        .expect("settled");
    let out = realm
        .evaluate(
            EvalSource::new("String(globalThis.out)"),
            EvalOptions::default(),
        )
        .unwrap();
    Ok(text(&out))
}

const DEPENDENCY: &str = "export const value = 41 + 1;\n\
     export default function () { return this === globalThis; }";

#[test]
fn a_running_module_body_requires_module_bytecode() {
    let bytes = module_bytecode("app:///dep.js", DEPENDENCY);
    let (mut runtime, mut realm) = sync_realm(vec![(
        "app:///dep.js",
        bytecode_answer("app:///dep.js", verified(&bytes)),
    )]);
    let out = run_entry(
        &mut runtime,
        &mut realm,
        "import { loadModuleSync } from 'bobcat:module';\n\
         const loaded = loadModuleSync('app:///dep.js', 'module, exports');\n\
         globalThis.out = [loaded.kind, loaded.url, loaded.value.value, \
         loaded.value.default.call(globalThis)].join();",
    );
    assert_eq!(out.unwrap(), "module,app:///dep.js,42,true");
    // The kept copy was read and freed inside the require.
    assert_eq!(realm.owned_bytecode(), 0);
}

#[test]
fn an_import_linked_inline_reads_module_bytecode() {
    let bytes = module_bytecode("app:///dep.js", DEPENDENCY);
    let (mut runtime, mut realm) = sync_realm(vec![
        (
            "app:///dep.js",
            bytecode_answer("app:///dep.js", verified(&bytes)),
        ),
        (
            "app:///main.js",
            text_module(
                "app:///main.js",
                "import { value } from 'app:///dep.js'; export const doubled = value * 2;",
            ),
        ),
    ]);
    let out = run_entry(
        &mut runtime,
        &mut realm,
        "import { loadModuleSync } from 'bobcat:module';\n\
         globalThis.out = loadModuleSync('app:///main.js', '').value.doubled;",
    );
    assert_eq!(out.unwrap(), "84");
    assert_eq!(realm.owned_bytecode(), 0);
}

#[test]
fn module_bytecode_completed_before_is_required_from_a_running_body() {
    // The host's loader would answer other text: the completed bytecode is
    // the module, read inside the require, and the loader's answer finds
    // the name already answered.
    let (mut runtime, mut realm) = sync_realm(vec![
        (
            "app:///pre.js",
            text_module("app:///pre.js", "export const value = 'host text';"),
        ),
        (
            "app:///main.js",
            text_module(
                "app:///main.js",
                "import { value } from 'app:///pre.js'; export const seen = value;",
            ),
        ),
    ]);
    let bytes = module_bytecode("app:///pre.js", DEPENDENCY);
    realm
        .complete_module_bytecode("app:///pre.js", "app:///pre.js", &verified(&bytes))
        .unwrap();
    assert!(realm.owned_bytecode() > 0);
    let out = run_entry(
        &mut runtime,
        &mut realm,
        "import { loadModuleSync } from 'bobcat:module';\n\
         globalThis.out = [loadModuleSync('app:///main.js', '').value.seen, \
         loadModuleSync('app:///pre.js', '').value.value].join();",
    );
    assert_eq!(out.unwrap(), "42,42");
    assert_eq!(realm.owned_bytecode(), 0);
}

#[test]
fn refused_bytecode_from_the_synchronous_loader_is_a_syntax_error() {
    // Bytes the fork's verifying read refuses, answered by the loader as a
    // module no verifier wrote: the read refuses them inside the load, for
    // a require and for an import linked inline.
    let bad = VerifiedModule::unverified(&refused_module_bytecode("app:///bad.js"));
    let kept = bad.bytes().len();
    let (mut runtime, mut realm) = sync_realm(vec![
        ("app:///bad.js", bytecode_answer("app:///bad.js", bad)),
        (
            "app:///main.js",
            text_module(
                "app:///main.js",
                "import 'app:///bad.js'; export const x = 1;",
            ),
        ),
    ]);
    let out = run_entry(
        &mut runtime,
        &mut realm,
        "import { loadModuleSync } from 'bobcat:module';\n\
         const seen = [];\n\
         for (const url of ['app:///bad.js', 'app:///main.js']) {\n\
           try { loadModuleSync(url, ''); seen.push('loaded'); }\n\
           catch (error) { seen.push(error instanceof SyntaxError, \
             error.message.includes(\"bytecode rejected for 'app:///bad.js'\")); }\n\
         }\n\
         globalThis.out = seen.join();",
    );
    assert_eq!(out.unwrap(), "true,true,true,true");
    // The refused copy stays with the source, answered once, which the
    // realm frees.
    assert_eq!(realm.owned_bytecode(), kept);
    drop(realm);
    let keeper = runtime.create_context().unwrap();
    assert_eq!(keeper.owned_bytecode(), 0);
}

#[test]
fn bytecode_the_verifying_read_refuses_fails_at_compile() {
    let name = "app:///broken.js";
    let broken = refused_module_bytecode(name);

    // An import: the evaluation rejects with the read's refusal.
    let (mut runtime, realm) = realm();
    realm
        .complete_module_bytecode_unchecked(name, name, &broken)
        .unwrap();
    assert_eq!(realm.owned_bytecode(), broken.len());
    for _ in 0..2 {
        let promise = realm.load_module(name).unwrap();
        // The job that settles it reports the rejection; the promise holds it.
        let _ = runtime.drain_pending_jobs(&realm);
        let error = realm
            .settled_promise_result(&promise)
            .expect_err("the import rejects");
        assert_eq!(error.name.as_deref(), Some("SyntaxError"), "{error:?}");
        assert!(
            error
                .message
                .contains("bytecode rejected for 'app:///broken.js'"),
            "{error:?}"
        );
        // The refused copy stays with the source, which frees it.
        assert_eq!(realm.owned_bytecode(), broken.len());
    }
    drop(realm);
    let keeper = runtime.create_context().unwrap();
    assert_eq!(keeper.owned_bytecode(), 0);

    // A require: the same refusal, thrown.
    let (mut runtime, mut realm) = sync_realm(vec![(
        name,
        text_module(name, "export const result = 'host text';"),
    )]);
    realm
        .complete_module_bytecode_unchecked(name, name, &broken)
        .unwrap();
    let out = run_entry(
        &mut runtime,
        &mut realm,
        "import { loadModuleSync } from 'bobcat:module';\n\
         try { loadModuleSync('app:///broken.js', ''); globalThis.out = 'loaded'; }\n\
         catch (error) { globalThis.out = [error instanceof SyntaxError, \
         error.message.includes(\"bytecode rejected for 'app:///broken.js'\")].join(); }",
    );
    assert_eq!(out.unwrap(), "true,true");
}

#[test]
fn kept_module_bytecode_is_freed_on_every_path() {
    let name = "app:///kept.js";
    let bytes = verified(&module_bytecode(name, "export const result = 'kept';"));
    let runtime = Runtime::new().unwrap();
    let keeper = runtime.create_context().unwrap();

    // Read: freed once the import has made the module.
    {
        let mut runtime_ref = Runtime::new().unwrap();
        let mut realm = runtime_ref.create_context().unwrap();
        realm.complete_module_bytecode(name, name, &bytes).unwrap();
        let kept = realm.owned_bytecode();
        assert!(kept > 0);
        // Answered twice: the second answer keeps nothing.
        realm.complete_module_bytecode(name, name, &bytes).unwrap();
        assert_eq!(realm.owned_bytecode(), kept);
        assert_eq!(evaluated_result(&mut runtime_ref, &mut realm, name), "kept");
        assert_eq!(realm.owned_bytecode(), 0);
    }

    // Never imported: freed with the realm.
    let realm = runtime.create_context().unwrap();
    realm.complete_module_bytecode(name, name, &bytes).unwrap();
    assert!(keeper.owned_bytecode() > 0);
    drop(realm);
    assert_eq!(keeper.owned_bytecode(), 0);
}

// ---------------------------------------------------------------------------
// Template objects

/// What a tagged template's strings array looks like: frozen state of it
/// and of `raw`, the flags of `raw` and `length`, and whether one site
/// answers the same array on every call.
const TEMPLATE_STATE: &str = "function tag(strings) { return strings; }\n\
     function site() { return tag`a${1}b\\x41${2}`; }\n\
     const s = site();\n\
     const raw = Object.getOwnPropertyDescriptor(s, 'raw');\n\
     const length = Object.getOwnPropertyDescriptor(s, 'length');\n\
     JSON.stringify([Object.isFrozen(s), Object.isFrozen(s.raw), Object.isExtensible(s), \
     Object.isExtensible(s.raw), s === site(), s.raw === site().raw, raw.writable, \
     raw.enumerable, raw.configurable, length.writable, length.configurable, \
     Object.getOwnPropertyDescriptor(s.raw, 'length').writable, [...s], [...s.raw]])";

const TEMPLATE_EXPECTED: &str = "[true,true,false,false,true,true,false,false,false,false,\
     false,false,[\"a\",\"bA\",\"\"],[\"a\",\"b\\\\x41\",\"\"]]";

#[test]
fn template_objects_read_from_bytecode_are_frozen_like_compiled_ones() {
    // The source run, which the compiler's own objects answer.
    let (_runtime, direct) = realm();
    let expected = direct
        .evaluate(EvalSource::new(TEMPLATE_STATE), EvalOptions::default())
        .unwrap();
    assert_eq!(text(&expected), TEMPLATE_EXPECTED);

    // A script read back from bytecode, by the fork's verifying read alone
    // and through the checked entry.
    let bytes = compiled_script(TEMPLATE_STATE);
    let (_runtime, runner) = realm();
    assert_eq!(
        text(&runner.evaluate_script_bytecode(&bytes).unwrap()),
        TEMPLATE_EXPECTED
    );
    let (_runtime, runner) = realm();
    let checked = runner.verify_bytecode(&bytes).unwrap();
    assert_eq!(
        text(&runner.evaluate_script_bytecode(&checked).unwrap()),
        TEMPLATE_EXPECTED
    );

    // A module, completed as bytecode and registered as bytecode.
    let source = format!("export const result = (() => {{ {TEMPLATE_STATE} }})();")
        .replace("JSON.stringify(", "return JSON.stringify(");
    let name = "app:///template.js";
    let (_compiler, compiler) = realm();
    let module = compiler.compile_module_bytecode(name, &source).unwrap();
    let (mut runtime, mut realm) = realm();
    realm
        .complete_module_bytecode(name, name, &verified(&module))
        .unwrap();
    assert_eq!(
        evaluated_result(&mut runtime, &mut realm, name),
        TEMPLATE_EXPECTED
    );
    let mut runtime = Runtime::new().unwrap();
    runtime
        .register_module_bytecode(name, Box::leak(module.into_boxed_slice()))
        .unwrap();
    let mut realm = runtime.create_context().unwrap();
    assert_eq!(
        evaluated_result(&mut runtime, &mut realm, name),
        TEMPLATE_EXPECTED
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
