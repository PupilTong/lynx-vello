//! Soundness by execution: mutants of the fork's own bytecode that pass the
//! Rust checks are written by the Rust writer, read by the fork (computing
//! stack sizes) and run, under an execution limit, and the process must
//! survive and the realm must be destroyable.
//!
//! The fork is only ever handed the writer's output for a program the
//! rules accepted, as in production: its reader trusts the structure of
//! its input. Every such output must read back with the Rust reader. The
//! fork's read can still refuse it when a function's stack size cannot be
//! computed; the rules walk every function the way the fork's computation
//! does (`ForkWalk`), so that count must be zero.
//!
//! `BYTECODE_FUZZ_ITERATIONS` sets the iterations of the ignored long run,
//! `BYTECODE_FUZZ_SEED` its seed, and `BYTECODE_FUZZ_DUMP` a file each
//! accepted mutant is written to before it runs (to find the one that
//! crashed).

use std::time::Duration;

use super::corpus::corpus;
use crate::bytecode::model::layout;
use crate::bytecode::{
    ClosureType, Function, FunctionKind, Instruction, Operand, Program, Value, read_program,
    write_program,
};
use crate::bytecode_abi::OP_COUNT;
use crate::bytecode_tests::{MODULES, SCRIPTS};
use crate::{Context, Runtime, RuntimeOptions};

/// `xorshift64`, so a run is the same every time.
pub(in crate::bytecode) struct Rng(pub(in crate::bytecode) u64);

impl Rng {
    pub(in crate::bytecode) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(in crate::bytecode) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next() % bound as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// One program to mutate: its bytes and whether it is a module.
struct Seed {
    bytes: Vec<u8>,
    module: Option<String>,
}

fn seeds() -> Vec<Seed> {
    let runtime = Runtime::new().unwrap();
    let mut seeds = Vec::new();
    let mut sources: Vec<String> = SCRIPTS.iter().map(|source| (*source).to_owned()).collect();
    sources.extend(corpus().into_iter().map(|(_, source)| source));
    for (index, source) in sources.iter().enumerate() {
        let compiler = runtime.create_context().unwrap();
        seeds.push(Seed {
            bytes: compiler.compile_script_bytecode("fuzz.js", source).unwrap(),
            module: None,
        });
        let name = format!("app:///fuzz{index}.js");
        let compiler = runtime.create_context().unwrap();
        if let Ok(bytes) = compiler.compile_module_bytecode(&name, source) {
            seeds.push(Seed {
                bytes,
                module: Some(name),
            });
        }
    }
    for (index, source) in MODULES.iter().enumerate() {
        let name = format!("app:///fuzzmodule{index}.js");
        let compiler = runtime.create_context().unwrap();
        seeds.push(Seed {
            bytes: compiler.compile_module_bytecode(&name, source).unwrap(),
            module: Some(name),
        });
    }
    seeds
}

/// Paths to every function of a program: cpool indices from the top-level
/// (or module) function.
fn function_paths(function: &Function, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(path.clone());
    for (index, entry) in function.cpool.iter().enumerate() {
        if let Value::Function(child) = entry {
            path.push(index);
            function_paths(child, path, out);
            path.pop();
        }
    }
}

fn root_function(program: &mut Program) -> Option<&mut Function> {
    match &mut program.root {
        Value::Function(function) => Some(function),
        Value::Module(module) => Some(&mut module.function),
        _ => None,
    }
}

fn function_at<'a>(mut function: &'a mut Function, path: &[usize]) -> &'a mut Function {
    for &index in path {
        let Value::Function(child) = &mut function.cpool[index] else {
            unreachable!("paths name functions")
        };
        function = child;
    }
    function
}

const INTERESTING: [i32; 12] = [0, 1, 2, 3, 4, 5, 6, 7, -1, 255, 0x7fff, -0x8000];

fn tweak_operand(rng: &mut Rng, operand: Operand, function: &Function) -> Operand {
    let small = |rng: &mut Rng, count: usize| (rng.below(count + 2)) as u16;
    match operand {
        Operand::Local(_) => Operand::Local(small(rng, usize::from(function.var_count))),
        Operand::Arg(_) => Operand::Arg(small(rng, usize::from(function.arg_count))),
        Operand::ClosureVar(_) => Operand::ClosureVar(small(rng, function.closure_vars.len())),
        Operand::Const(_) => Operand::Const(rng.below(function.cpool.len() + 1) as u32),
        Operand::Immediate(value) => {
            if rng.chance(50) {
                Operand::Immediate(INTERESTING[rng.below(INTERESTING.len())])
            } else {
                Operand::Immediate(value.wrapping_add(rng.below(5) as i32 - 2))
            }
        }
        Operand::Target(_) => Operand::Target(rng.below(function.instructions.len()) as u32),
        Operand::Atom(atom) => Operand::Atom(atom),
    }
}

/// Retargets jumps after inserting (`delta` 1) or removing (`delta` -1)
/// the instruction at `at`.
fn shift_targets(function: &mut Function, at: usize, delta: i64) {
    for instruction in &mut function.instructions {
        for operand in instruction.operands_mut() {
            if let Operand::Target(target) = operand
                && (*target as usize > at || (delta > 0 && *target as usize == at))
            {
                *target = (i64::from(*target) + delta).max(0) as u32;
            }
        }
    }
}

fn random_instruction(rng: &mut Rng, function: &Function) -> Option<Instruction> {
    let opcode = (1 + rng.below(OP_COUNT - 1)) as u8;
    let operands: Vec<Operand> = layout(opcode)
        .iter()
        .map(|&(kind, encoding)| {
            use crate::bytecode::OperandKind as K;
            use crate::bytecode::model::Encoding;
            match (kind, encoding) {
                (K::Local, Encoding::Implicit(n)) => Operand::Local(n as u16),
                (K::Arg, Encoding::Implicit(n)) => Operand::Arg(n as u16),
                (K::ClosureVar, Encoding::Implicit(n)) => Operand::ClosureVar(n as u16),
                (_, Encoding::Implicit(n)) => Operand::Immediate(n),
                (K::Atom, _) => {
                    Operand::Atom(crate::bytecode::Atom::Builtin(rng.below(240) as u32))
                }
                (K::Local, _) => {
                    Operand::Local(rng.below(usize::from(function.var_count) + 1) as u16)
                }
                (K::Arg, _) => Operand::Arg(rng.below(usize::from(function.arg_count) + 1) as u16),
                (K::ClosureVar, _) => {
                    Operand::ClosureVar(rng.below(function.closure_vars.len() + 1) as u16)
                }
                (K::Const, _) => Operand::Const(rng.below(function.cpool.len() + 1) as u32),
                (K::Target, _) => Operand::Target(rng.below(function.instructions.len()) as u32),
                (K::Immediate, Encoding::U8) => Operand::Immediate(rng.below(8) as i32),
                (K::Immediate, _) => Operand::Immediate(INTERESTING[rng.below(8)]),
            }
        })
        .collect();
    Instruction::new(opcode, &operands).ok()
}

/// One random change to the model.
#[allow(clippy::too_many_lines, reason = "one arm per mutation kind")]
fn mutate_model(rng: &mut Rng, program: &mut Program) {
    let Some(root) = root_function(program) else {
        return;
    };
    let mut paths = Vec::new();
    function_paths(root, &mut Vec::new(), &mut paths);
    let path = paths[rng.below(paths.len())].clone();
    let other = paths[rng.below(paths.len())].clone();
    let donor = function_at(root, &other).clone();
    let function = function_at(root, &path);
    let count = function.instructions.len();
    match rng.below(16) {
        0 | 1 => {
            // Another opcode with the same operand shape.
            let at = rng.below(count);
            let instruction = function.instructions[at];
            let shape: Vec<_> = layout(instruction.opcode())
                .iter()
                .map(|(kind, _)| *kind)
                .collect();
            for _ in 0..20 {
                let opcode = (1 + rng.below(OP_COUNT - 1)) as u8;
                let other: Vec<_> = layout(opcode).iter().map(|(kind, _)| *kind).collect();
                if other == shape
                    && let Ok(changed) = Instruction::new(opcode, instruction.operands())
                {
                    function.instructions[at] = changed;
                    break;
                }
            }
        }
        2 | 3 => {
            let at = rng.below(count);
            let mut instruction = function.instructions[at];
            let operands = instruction.operands().to_vec();
            if !operands.is_empty() {
                let slot = rng.below(operands.len());
                let tweaked = tweak_operand(rng, operands[slot], function);
                let mut changed = operands;
                changed[slot] = tweaked;
                if let Ok(made) = Instruction::new(instruction.opcode(), &changed) {
                    instruction = made;
                }
            }
            function.instructions[at] = instruction;
        }
        4 => {
            if count > 1 {
                let at = rng.below(count);
                function.instructions.remove(at);
                shift_targets(function, at, -1);
                let len = function.instructions.len() as u32;
                for instruction in &mut function.instructions {
                    for operand in instruction.operands_mut() {
                        if let Operand::Target(target) = operand
                            && *target >= len
                        {
                            *target = len - 1;
                        }
                    }
                }
            }
        }
        5 => {
            let at = rng.below(count);
            let copy = function.instructions[at];
            shift_targets(function, at + 1, 1);
            function.instructions.insert(at + 1, copy);
        }
        6 | 7 => {
            if let Some(instruction) = random_instruction(rng, function) {
                let at = rng.below(count);
                shift_targets(function, at, 1);
                function.instructions.insert(at, instruction);
            }
        }
        8 => {
            if count > 1 {
                let at = rng.below(count - 1);
                function.instructions.swap(at, at + 1);
            }
        }
        9 => {
            if !function.closure_vars.is_empty() {
                let at = rng.below(function.closure_vars.len());
                let closure_var = &mut function.closure_vars[at];
                match rng.below(3) {
                    0 => closure_var.closure_type = ClosureType::from_bits(rng.below(8) as u8),
                    1 => closure_var.var_idx = rng.below(6) as u16,
                    _ => closure_var.is_lexical = !closure_var.is_lexical,
                }
            }
        }
        10 => {
            if !function.vardefs.is_empty() {
                let at = rng.below(function.vardefs.len());
                let vardef = &mut function.vardefs[at];
                match rng.below(4) {
                    0 => vardef.is_captured = !vardef.is_captured,
                    1 => vardef.var_ref_idx = rng.below(4) as u16,
                    2 => vardef.is_lexical = !vardef.is_lexical,
                    _ => vardef.scope_next = rng.below(5) as i32 - 2,
                }
            } else if rng.chance(50) {
                function.var_ref_count = function.var_ref_count.wrapping_add(1);
            }
        }
        11 => {
            function.flags.kind = FunctionKind::from_bits(rng.below(4) as u8);
        }
        12 => {
            function.js_mode ^= 1 << rng.below(4);
        }
        13 => {
            // Borrow another function's code.
            if !donor.instructions.is_empty() && rng.chance(50) {
                let at = rng.below(donor.instructions.len());
                let instruction = donor.instructions[at];
                let insert = rng.below(count);
                shift_targets(function, insert, 1);
                function.instructions.insert(insert, instruction);
            } else if !function.cpool.is_empty() {
                let at = rng.below(function.cpool.len());
                function.cpool[at] = match rng.below(4) {
                    0 => Value::Function(Box::new(donor)),
                    1 => Value::Int32(7),
                    2 => Value::String(crate::bytecode::JsString::from_str("x")),
                    _ => Value::Undefined,
                };
            }
        }
        14 => match rng.below(3) {
            0 => function.arg_count = function.arg_count.saturating_add(1),
            1 => function.var_count = function.var_count.saturating_sub(1),
            _ => function.defined_arg_count = rng.below(3) as u16,
        },
        _ => {
            if let Value::Module(module) = &mut program.root {
                if !module.imports.is_empty() {
                    let at = rng.below(module.imports.len());
                    match rng.below(3) {
                        0 => module.imports[at].is_star = !module.imports[at].is_star,
                        1 => module.imports[at].var_idx = rng.below(6) as u32,
                        _ => module.imports[at].request = rng.below(3) as u32,
                    }
                } else if !module.exports.is_empty() {
                    let at = rng.below(module.exports.len());
                    if let crate::bytecode::Export::Local { var_idx, .. } = &mut module.exports[at]
                    {
                        *var_idx = rng.below(6) as u32;
                    }
                } else {
                    module.has_tla = !module.has_tla;
                }
            }
        }
    }
}

fn mutate_bytes(rng: &mut Rng, bytes: &mut Vec<u8>) {
    for _ in 0..=rng.below(3) {
        if bytes.is_empty() {
            return;
        }
        let at = rng.below(bytes.len());
        match rng.below(5) {
            0 => bytes[at] = rng.next().to_le_bytes()[0],
            1 => bytes[at] ^= 1 << rng.below(8),
            2 => bytes[at] = [0, 1, 2, 0x7f, 0x80, 0xff][rng.below(6)],
            3 => {
                let byte = bytes[at];
                bytes.insert(at, byte);
            }
            _ => {
                bytes.remove(at);
            }
        }
    }
}

/// What happened to the mutants, one counter per outcome.
#[derive(Default, Debug)]
pub(in crate::bytecode) struct Counts {
    /// Mutants made.
    pub(in crate::bytecode) mutants: usize,
    /// Model mutants the writer refused to write.
    pub(in crate::bytecode) unwritable: usize,
    /// Mutants the Rust reader or rules refused (for a module seed, also
    /// one whose root is no longer a module).
    pub(in crate::bytecode) refused_by_rust: usize,
    /// Module mutants the rules accepted whose name or URL
    /// `complete_module_bytecode` refused (a NUL byte in the name the bytes
    /// carry): nothing reached the fork.
    pub(in crate::bytecode) name_refused: usize,
    /// Mutants the rules accepted and the fork's read then refused: a
    /// function whose stack size the fork's computation refuses.
    pub(in crate::bytecode) refused_by_fork_read: usize,
    /// Script mutants whose read ran out of the runtime's memory limit.
    pub(in crate::bytecode) read_out_of_memory: usize,
    /// Mutants the fork read and ran (or tried to: an import that throws
    /// or runs out of time counts here).
    pub(in crate::bytecode) executed: usize,
}

fn limited_runtime() -> Runtime {
    Runtime::with_options(RuntimeOptions {
        memory_limit: Some(64 << 20),
        max_stack_size: Some(256 << 10),
        execution_timeout: Some(Duration::from_millis(40)),
        strip_source: false,
    })
    .unwrap()
}

/// Whether `message` is the fork's read refusing a module's bytes
/// (`qjs_throw_bytecode_rejected`).
fn rejected_by_read(message: &str) -> bool {
    message.contains("bytecode rejected for '")
}

/// Records a mutant the fork's read refused.
fn fork_refused(counts: &mut Counts, checked: &[u8], reason: &str, dump: Option<&str>) {
    counts.refused_by_fork_read += 1;
    if let Some(path) = dump {
        let _ = std::fs::write(format!("{path}.fork-refused"), checked);
        let _ = std::fs::write(format!("{path}.fork-refused.txt"), reason);
    }
}

/// Checks `bytes` and, when the rules accept them, hands the fork what the
/// Rust writer wrote for them, and runs it.
fn run_mutant(bytes: &[u8], module: Option<&str>, counts: &mut Counts, dump: Option<&str>) {
    let mut runtime = limited_runtime();
    let realm = runtime.create_context().unwrap();
    let Ok(checked) = crate::bytecode::check(bytes, &realm as &dyn crate::bytecode::RegExpCompiler)
    else {
        counts.refused_by_rust += 1;
        return;
    };
    // The bytes the fork gets have the structure its reader trusts: the
    // reader for untrusted bytes reads them, and they are a fixed point of
    // reading and writing.
    let reread = read_program(&checked)
        .unwrap_or_else(|error| panic!("the writer's output does not read back: {error}"));
    assert!(
        write_program(&reread).as_deref() == Ok(checked.as_slice()),
        "the writer's output is not a fixed point of reading and writing"
    );
    if let Some(path) = dump {
        let _ = std::fs::write(path, &checked);
    }
    match module {
        None => {
            // The read alone first (a script read leaves nothing in the
            // realm), so that its refusal is told apart from the script's
            // own exceptions; then the read and the run.
            if let Err(error) = realm.verify_bytecode_read_only(&checked) {
                if error.kind == crate::ErrorKind::BytecodeRejected {
                    fork_refused(counts, &checked, &error.message, dump);
                } else {
                    counts.read_out_of_memory += 1;
                }
                drop(realm);
                runtime.run_gc();
                return;
            }
            counts.executed += 1;
            if realm.evaluate_script_bytecode(&checked).is_ok() {
                let _ = runtime.drain_pending_jobs(&realm);
            }
        }
        Some(seed_name) => {
            // A mutant may have changed the name its bytes carry, which is
            // the name the realm's module list holds.
            let carried = read_program(&checked)
                .ok()
                .and_then(|program| match &program.root {
                    Value::Module(module) => program
                        .atom_text(module.name)
                        .map(std::borrow::Cow::into_owned),
                    _ => None,
                });
            let name = carried.as_deref().unwrap_or(seed_name);
            let Ok(verified) = crate::bytecode::Verifier::new().module_from_bytes(bytes) else {
                // The checks above took any root; a module takes a module.
                counts.refused_by_rust += 1;
                drop(realm);
                runtime.run_gc();
                return;
            };
            assert_eq!(verified.bytes(), checked.as_slice());
            if realm
                .complete_module_bytecode(name, name, &verified)
                .is_err()
            {
                // Refused before anything reached the fork.
                counts.name_refused += 1;
                drop(realm);
                runtime.run_gc();
                return;
            }
            // The fork reads the module inside the load, and a refusal is
            // the load's `SyntaxError`: thrown by the call, or the
            // rejection of the promise it answers.
            let refusal = match realm.load_module(name) {
                Err(error) => rejected_by_read(&error.message).then_some(error.message),
                Ok(promise) => {
                    let _ = runtime.drain_pending_jobs(&realm);
                    match realm.settled_promise_result(&promise) {
                        Err(error) if rejected_by_read(&error.message) => Some(error.message),
                        _ => None,
                    }
                }
            };
            match refusal {
                Some(reason) => fork_refused(counts, &checked, &reason, dump),
                None => counts.executed += 1,
            }
        }
    }
    drop(realm);
    runtime.run_gc();
}

fn realm_for_checks() -> (Runtime, Context) {
    let runtime = limited_runtime();
    let realm = runtime.create_context().unwrap();
    (runtime, realm)
}

/// Runs `iterations` mutants from `seed`.
pub(in crate::bytecode) fn fuzz(seed: u64, iterations: usize) -> Counts {
    let seeds = seeds();
    let dump = std::env::var("BYTECODE_FUZZ_DUMP").ok();
    let mut rng = Rng(seed);
    let mut counts = Counts::default();
    let (_runtime, _checker) = realm_for_checks();
    for _ in 0..iterations {
        let seed = &seeds[rng.below(seeds.len())];
        counts.mutants += 1;
        let bytes = if rng.chance(70) {
            let mut program = read_program(&seed.bytes).unwrap();
            for _ in 0..=rng.below(2) {
                mutate_model(&mut rng, &mut program);
            }
            let Ok(bytes) = write_program(&program) else {
                counts.unwritable += 1;
                continue;
            };
            bytes
        } else {
            let mut bytes = seed.bytes.clone();
            mutate_bytes(&mut rng, &mut bytes);
            bytes
        };
        run_mutant(&bytes, seed.module.as_deref(), &mut counts, dump.as_deref());
    }
    counts
}

fn in_big_thread<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap()
}

/// What a non-zero `refused_by_fork_read` means.
const STACK_WALK_DIVERGED: &str = "the fork's read refused bytecode the Rust rules accepted: the \
     Rust copy of the fork's stack walk in verify.rs (the ForkWalk rule) has diverged from the \
     fork's compute_stack_size_buf, so production would accept modules the engine then refuses \
     to load";

#[test]
fn checked_mutants_run_without_crashing() {
    let counts = in_big_thread(|| fuzz(0x9e37_79b9_7f4a_7c15, 5000));
    println!("{counts:?}");
    assert!(counts.executed > 50, "{counts:?}");
    assert!(counts.refused_by_rust > 50, "{counts:?}");
    assert_eq!(
        counts.refused_by_fork_read, 0,
        "{STACK_WALK_DIVERGED}: {counts:?}"
    );
}

#[test]
#[ignore = "long run: BYTECODE_FUZZ_ITERATIONS, BYTECODE_FUZZ_SEED"]
fn checked_mutants_run_without_crashing_long() {
    let iterations = std::env::var("BYTECODE_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    let seed = std::env::var("BYTECODE_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x1234_5678_9abc_def1);
    let counts = in_big_thread(move || fuzz(seed, iterations));
    println!("{counts:?}");
    assert_eq!(
        counts.refused_by_fork_read, 0,
        "{STACK_WALK_DIVERGED}: {counts:?}"
    );
}

/// Re-runs one dumped mutant (`BYTECODE_FUZZ_DUMP`), for finding the rule a
/// crash needs. The dump is the Rust writer's output for a checked
/// program, which is what the fork's read may be handed.
#[test]
#[ignore = "reproduces a dumped mutant"]
fn run_dumped_mutant() {
    let path = std::env::var("BYTECODE_FUZZ_DUMP").unwrap();
    let bytes = std::fs::read(path).unwrap();
    let program = read_program(&bytes).unwrap();
    println!("{}", crate::bytecode::disassemble(&program));
    let module = match &program.root {
        Value::Module(module) => program
            .atom_text(module.name)
            .map(std::borrow::Cow::into_owned),
        _ => None,
    };
    let (_runtime, realm) = realm_for_checks();
    println!(
        "fork read: {:?}",
        realm.verify_bytecode_read_only(&bytes).map(|_| ())
    );
    let mut counts = Counts::default();
    in_big_thread(move || {
        run_mutant(&bytes, module.as_deref(), &mut counts, None);
        println!("{counts:?}");
    });
}

/// Prints one dumped mutant (`BYTECODE_FUZZ_DUMP`) without running it.
#[test]
#[ignore = "prints a dumped mutant"]
fn print_dumped_mutant() {
    let path = std::env::var("BYTECODE_FUZZ_DUMP").unwrap();
    let bytes = std::fs::read(path).unwrap();
    let program = read_program(&bytes).unwrap();
    println!("{}", crate::bytecode::disassemble(&program));
    let (_runtime, realm) = realm_for_checks();
    println!(
        "{:?}",
        crate::bytecode::verify(&program, &realm as &dyn crate::bytecode::RegExpCompiler)
    );
}
