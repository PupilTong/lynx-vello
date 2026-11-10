//! The pinned fork's serialized bytecode as a Rust model, with a reader, a
//! writer, a disassembler and the verifier that decides whether bytes from
//! an untrusted writer may run.
//!
//! This is the Rust half of the bytecode safety boundary of
//! `docs/primjs-bytecode-conversion-design.md` (§4.7, ruling 8). The fork's
//! verifying read (`JS_READ_OBJ_VERIFY`) makes the C reader safe on any
//! bytes and computes each function's stack size; it does not make the
//! bytecode safe to run, because the interpreter assumes many properties
//! of compiler output that the read does not check. [`verify`] checks them.
//! [`crate::Context::complete_module_bytecode`] and
//! [`crate::Context::verify_bytecode`] run [`check`] (read, verify, write)
//! on every input and hand the engine the bytes [`write_program`] wrote, so
//! no bytes reach the C reader without passing these rules.
//!
//! The module contains no unsafe code; it works over the generated tables
//! of [`crate::bytecode_abi`].
//!
//! # The model
//!
//! A [`Program`] is the file's atom table ([`Program::atoms`]) and the
//! top-level [`Value`]: a [`Function`] (a global script), a [`Module`], or
//! a data value. It mirrors the `PrimJS` reader's model in `bobcat-source`
//! (`native::primjs`), so that a converter translates one into the other:
//!
//! - **Atoms** are [`Atom`]: `Builtin(i)` (`i < JS_ATOM_END`, `0` is `JS_ATOM_NULL`), `File(i)`
//!   into [`Program::atoms`], or `Int(n)`. [`Program::intern`] turns text into the atom the runtime
//!   interns it to, so a builder never puts a built-in or integer text in the table.
//! - **Functions** carry every field `JS_WriteFunctionTag` writes, in order. `has_debug` is
//!   [`Function::debug`] being present. [`Function::stack_size`] is kept so that read and write
//!   round-trip; the verifying read ignores it and computes its own, so a builder leaves it `0`.
//! - **Instructions** ([`Instruction`]) are an opcode byte (an [`crate::bytecode_abi::op`]
//!   constant) and typed [`Operand`]s in encoding order, implicit operands of the short forms
//!   included. A jump target is [`Operand::Target`]: an index into the same function's
//!   instructions. [`Instruction::new`] refuses operands that do not match the opcode's format.
//! - **Modules** ([`Module`]) carry the record `JS_WriteModule` writes.
//!
//! A converter builds a [`Program`] directly: intern its strings with
//! [`Program::intern`], build each [`Function`] from [`Function::new`],
//! push instructions made with [`Instruction::new`] (using the 32-bit jump
//! forms, since it changes instruction sizes), then call [`verify`] and
//! [`write_program`]. [`check`] is the same sequence for bytes.
//!
//! # Reader and writer
//!
//! [`read_program`] accepts what the verifying read accepts, minus
//! non-canonical encodings (see its documentation), without panicking on
//! any input and with recursion bounded by [`NESTING_LIMIT`].
//! [`write_program`] writes what `JS_WriteObject` writes for the same
//! program; for bytes the fork wrote, `write_program(read_program(b))` is
//! `b`. Its jump policy: the opcode the model gives is kept, a short jump
//! out of range is an error.
//!
//! # The rules
//!
//! [`verify`] walks every function's control flow the way the fork's
//! `compute_stack_size_buf` does, following the same edges (`if_*`,
//! `goto*`, `gosub` to its target and past it, `catch` to its handler at
//! one more slot, the `with_*` targets at their own depths; `ret`,
//! `return*`, `tail_call*`, `throw*` end a path), and adds the edge the
//! interpreter takes and the fork's walk does not: `ret` back to the
//! instruction after each `gosub` whose address it can pop. Per reached
//! instruction it keeps the stack depth, the kinds of the slots that are
//! not plain values (a sparse list), the locals that can hold the
//! uninitialized marker there, whether a generator has passed
//! `initial_yield`, and a copy of the fork walk's own depth and catch
//! position. The *special* slot kinds are a `gosub` return address (with
//! the set of `gosub`s that can have pushed it), a catch offset (with its
//! `catch`), the iterator marker (catch offset 0) of `for_of_start`,
//! `for_await_of_start` and `iterator_get_value_done`, and function
//! bytecode pushed by `push_const`. Three more kinds are plain values
//! with a known property: `undefined` (holds no reference), the module
//! function's `this` (`true` when linked, `undefined` when evaluated), and
//! a value an unchecked read took from a variable that can hold the
//! uninitialized marker. Where paths meet, return-address sets are joined,
//! the two known-value kinds fall back to a plain value, a maybe-marker
//! value stays one, and the sets of maybe-uninitialized locals are joined;
//! everything else must be equal. A `catch` handler starts with every local
//! any `set_loc_uninitialized` of the function touches counted as
//! maybe-uninitialized, since it is entered from anywhere in its region.
//!
//! | Rule ([`Rule`]) | Requirement | What the interpreter would do otherwise |
//! | --- | --- | --- |
//! | `StackDepth` | every pop and addressed slot is on the stack; depth ≤ `JS_STACK_SIZE_MAX` | read or write outside the frame |
//! | `Merge` | where paths meet, depth and special slots are equal (return-address sets are joined) | run an instruction with a different stack than the one checked |
//! | `ForkWalk` | the fork walk's depth is never below the real one and never inconsistent | size the frame too small (`nip_catch` is computed from the fork walk's catch position, which can disagree with the stack) |
//! | `SpecialSlot` | only `drop`, `nip`, `nip1` discard a special slot (one, as the slot they drop); `nip_catch` discards everything above the nearest catch offset or marker; permutations may move a marker and nothing else special; `define_class` consumes function bytecode, and without its heritage flag finds `undefined` under it; nothing else pops, copies, moves or reads one | a return address or catch offset used as a value, copied, or moved so that the unwinder resumes at a wrong depth; raw function bytecode reaching JavaScript |
//! | `IteratorSlots` | `for_of_next n`: `sp[-3-n]`, `sp[-2-n]` plain, `sp[-1-n]` the marker; `iterator_next`/`iterator_call`: `sp[-4]`, `sp[-3]`, `sp[-1]` plain, `sp[-2]` marker or plain; `iterator_close`: two plain values under a top that is the marker or a known `undefined`; `for_await_of_next`: the marker or a known `undefined` on top; `iterator_get_value_done`: four slots, the marker or a known `undefined` in `sp[-2]`; every marker has two plain slots below it | `sp[offset]` read outside the frame; the unwinder closes or frees slots below the stack; the three instructions that overwrite or drop a slot without freeing it leak an object, and the runtime aborts when it is freed (`JS_FreeRuntime`'s leak assertion) |
//! | `NipCatch` | a catch offset or marker exists below the top | the scan runs to the bottom and throws an internal error, after freeing the stack |
//! | `Ret` | `ret` pops a return address; the stack after it equals the stack at each `gosub` it returns to | jump to any in-buffer integer, or resume with a depth the fork walk did not size |
//! | `BackwardTarget` | `catch`, `gosub` and `with_*` jump forward | the interpreter polls for interrupts only on `goto`, `if_*` and calls: a loop closed by these jumps, or by an exception delivered to an earlier handler, never reaches the execution limit (with forward targets every cycle passes a polling jump: a `ret` can only return past a `gosub` that ran again); and a handler at instruction 0 would have catch offset 0, which the unwinder reads as the iterator marker and closes an "iterator" instead of jumping |
//! | `FrameKind` | `await`, `yield`, `yield_star`, `async_yield_star`, `initial_yield`, `return_async` only in frames that resume them; `return`, `return_undef`, `tail_call*` only in normal frames; in a generator, `initial_yield` exactly once, with no special slot, before any suspension or `return_async`; a module function is checked twice: as linked (a normal frame, `this === true`), where only the compiler's linking prologue may run — `push_this`, `if_*`, `fclosure` of functions that capture no local or argument, `put_var_ref`, `set_name`, `undefined`, `return_undef`, and `put_loc` of `this` or `undefined` — and as evaluated (an async frame, `this === undefined`) | leave a normal frame without closing its variable references (use after free), resume a generator with a stack one value short, close variable references twice, `abort()` in `js_async_generator_resume_next`; a module that throws while it links leaves its normal frame through the generator exit, which frees nothing and closes nothing |
//! | `FallsOffEnd` | no path runs past the last instruction | read past the bytecode |
//! | `OperandRange` | local, argument, closure-variable, constant and jump operands inside their counts, implicit ones included | index outside `var_buf`, `arg_buf`, `var_refs`, `cpool` |
//! | `EnumOperand` | `special_object` ≤ 6; `throw_error` type ≤ 5; method flags in {0,1,2}+4; class flags 0/1; `apply` magic ≤ 2; `iterator_call` flags ≤ 3; `with_*` flag 0/1 | `abort()` in `special_object` |
//! | `ClosureConstant` | `fclosure` takes function bytecode | `js_closure` takes any pool value as `JSFunctionBytecode *` |
//! | `RegExp` | `regexp` directly follows `push_const pattern; push_const bytecode` of two strings, no jump lands on the second or on itself, the bytecode is an 8-bit string equal to what the engine compiles for the pattern and the flags in its header | `lre_exec` runs bytes as regular expression bytecode it never checks |
//! | `NullAtom`, `PrivateSymbolAtom`, `AtomRange` | atoms valid; no `JS_ATOM_NULL` operand outside `throw_error`/`define_class*`, closure-variable name, module record name or object key; no integer atom for `private_symbol` | `JS_ValueToAtom` returns `JS_ATOM_NULL` (its failure value) without an exception; a property keyed `JS_ATOM_NULL` reads as deleted to the object's shape, so the garbage collector neither marks nor frees what it holds; `JS_NewSymbolFromAtom` asserts |
//! | `Uninitialized` | a value an unchecked `get_loc*` (of a local that can hold the marker at that point) or `get_var_ref*` (of a closure variable that can: anything but an argument, a parent local no `set_loc_uninitialized` touches, or a non-lexical module binding) pushes is taken only by an instruction that tests its tag — the name of `get_private_field`, `put_private_field`, `define_private_field`, `private_in`, the brand of `check_brand` (after which it is a function), the object of `with_*` — or dropped, or moved; `inc_loc`, `dec_loc`, `add_loc` never read a maybe-uninitialized local; a direct `eval` runs only while every maybe-uninitialized local is lexical | the uninitialized marker becomes a plain value that reaches JavaScript or the host |
//! | `VarRefCapture` | `make_loc_ref`, `make_arg_ref`, `close_loc`, a mapped `arguments`, a direct `eval` and a child's `LOCAL`/`ARG` capture name variables whose vardef is captured | `get_var_ref` dereferences missing vardefs, asserts, indexes `var_refs` out of range |
//! | `EvalScope` | a direct `eval`'s scope walk stays inside the locals and ends | `add_closure_variables` loops or writes past its array |
//! | `GlobalBinding` | a closure variable that leads to a `GLOBAL` or `GLOBAL_DECL` entry is reached only by `get_var`, `get_var_undef`, `put_var`, `put_var_init`, and by `put_var_ref` when it is a declared function (`GLOBAL_DECL`, `GLOBAL_FUNCTION_DECL`) | a global binding may be a variable reference parked in the global object's `uninitialized_vars`; the engine assumes it holds the uninitialized marker and overwrites its value without freeing it when a declaration adopts it |
//! | `ModuleThisFunc` | no `special_object` `THIS_FUNC` or `MAPPED_ARGUMENTS` in a module function | JavaScript calls the module function, a normal-class function whose body is an async body |
//! | `VardefCount`, `VarRefIndex`, `VarKind`, `DefinedArgCount`, `JsMode` | vardefs `arg_count + var_count` or none; captured indices below `var_ref_count`, distinct, counted exactly; kinds in range; `defined_arg_count ≤ arg_count`; `js_mode` only strict and backtrace barrier | the `JS_MODE_ASYNC` bit in a normal frame makes `get_var_ref` treat the C stack as an async frame |
//! | `ClosureType`, `ClosureParent` | a script's top-level function holds `GLOBAL`/`GLOBAL_DECL`; a module's adds `MODULE_DECL`/`MODULE_IMPORT`; a nested function holds `LOCAL`/`ARG`/`REF`/`GLOBAL_REF` naming an existing, captured (and for `GLOBAL_REF`, global) parent entry | `js_closure2` with no parent frame or parent references, `abort()` |
//! | `ModuleRecord`, `ModuleFunction` | imports fill `MODULE_IMPORT` (star: `MODULE_DECL`) variables, every `MODULE_IMPORT` variable is filled, local exports name `MODULE_DECL` variables, request indices in range, attributes `undefined` or an object; the module function is async and strict | the linker dereferences a null variable reference; evaluation of a normal-kind module body frees its frame twice |
//! | `ClassConstructor` | `define_class` constructs from a normal function | `js_op_define_class` asserts |
//! | `ValuePlacement` | functions only as the top level, a module's function or pool entries; a module only at the top level | (the verifying read refuses these too) |
//!
//! Not covered: `JS_FreeRuntime` aborts on a leak, and a module whose
//! linking prologue runs out of memory between `fclosure` and
//! `put_var_ref` (the `set_name` of `export default function`) leaks that
//! closure, as the fork's own compiler output does; regular expression
//! bytecode is checked by comparison with the engine's compiler, not by a
//! verifier of `lre_exec`'s assumptions; a direct `eval` compiles new code
//! against the frame's vardefs and flags, of which the indices and
//! captures are checked and the flags (`new_target_allowed`,
//! `super_allowed`, ...) are not.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "a codec of fixed-width fields: each cast converts between the format's widths, and \
              the values are range-checked where the format requires it"
)]

mod disasm;
mod error;
mod model;
mod read;
mod verify;
mod write;

#[cfg(test)]
mod tests;

pub use disasm::disassemble;
pub use error::{CheckError, ModelError, ReadError, Rule, VerifyError, WriteError};
pub use model::{
    Atom, ClosureType, ClosureVar, DebugInfo, Export, Function, FunctionFlags, FunctionKind,
    Import, Instruction, JsString, Module, ModuleRequest, Operand, OperandKind, Program,
    ResolvedAtom, Value, VarDef,
};
pub use read::{NESTING_LIMIT, read_program};
pub use verify::{RegExpCompiler, verify};
pub use write::write_program;

/// What the top-level value of checked bytes must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// A module record.
    Module,
    /// Anything the format carries.
    Any,
}

/// Reads untrusted `bytes`, checks every rule of [`verify`] and writes the
/// program back: the answer is the bytes to hand the engine, which mean
/// exactly the program that was checked.
pub fn check(
    bytes: &[u8],
    regexps: &dyn RegExpCompiler,
    root: Root,
) -> Result<Vec<u8>, CheckError> {
    let program = read_program(bytes).map_err(CheckError::Read)?;
    if root == Root::Module && !matches!(program.root, Value::Module(_)) {
        return Err(CheckError::Root("a module"));
    }
    verify(&program, regexps).map_err(CheckError::Verify)?;
    write_program(&program).map_err(CheckError::Write)
}
