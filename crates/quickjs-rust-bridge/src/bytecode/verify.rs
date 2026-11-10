//! The rules the interpreter assumes of bytecode and the fork's verifying
//! read does not check. The module documentation lists them with what each
//! protects; this file implements them.

use std::collections::BTreeSet;

use super::error::{Rule, VerifyError};
use super::model::{
    Atom, ClosureType, Export, Function, FunctionKind, Instruction, JsString, Module, Operand,
    Program, ResolvedAtom, Value,
};
use super::read::NESTING_LIMIT;
use crate::bytecode_abi::{
    builtin_string_atom, js_mode, limits, op, regexp_flag, special_object, throw_error, var_kind,
};

/// The engine's regular expression compiler, which the `regexp` rule
/// needs: a regular expression literal's bytecode must be exactly what the
/// engine compiles for its pattern and flags. [`crate::Context`]
/// implements it.
pub trait RegExpCompiler {
    /// The bytecode the engine compiles for `pattern` (UTF-16 code units)
    /// and `flags` (flag letters), or `None` when it refuses them.
    fn compile_regexp(&self, pattern: &[u16], flags: &str) -> Option<Vec<u8>>;
}

impl RegExpCompiler for crate::Context {
    fn compile_regexp(&self, pattern: &[u16], flags: &str) -> Option<Vec<u8>> {
        self.compile_regexp_bytecode(pattern, flags).ok()
    }
}

/// Checks every rule of the module documentation on `program`. `regexps`
/// compiles the regular expression literals the program carries, to
/// compare with their bytecode.
pub fn verify(program: &Program, regexps: &dyn RegExpCompiler) -> Result<(), VerifyError> {
    let mut verifier = Verifier {
        program,
        regexps,
        depth: 0,
    };
    match &program.root {
        Value::Function(function) => {
            verifier.function(function, Placement::ScriptRoot, None, "root")
        }
        Value::Module(module) => verifier.module(module),
        other => verifier.data(other, "root"),
    }
}

/// Where a function sits, which decides its closure types and frames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Placement {
    /// The top-level function of a script: run once as a closure with no
    /// parent.
    ScriptRoot,
    /// A module's function: linked once in a normal frame, evaluated once
    /// as an async function.
    Module,
    /// A constant-pool function, instantiated by its parent.
    Nested,
}

/// The frame a function's code runs in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Frame {
    /// A normal call frame.
    Normal,
    /// A generator's frame.
    Generator,
    /// An async function's frame.
    Async,
    /// An async generator's frame.
    AsyncGenerator,
    /// A module function linked (`this === true`, normal frame).
    ModuleLink,
    /// A module function evaluated (`this === undefined`, async frame).
    ModuleEval,
}

impl Frame {
    fn of(kind: FunctionKind) -> Self {
        match kind {
            FunctionKind::Normal => Self::Normal,
            FunctionKind::Generator => Self::Generator,
            FunctionKind::Async => Self::Async,
            FunctionKind::AsyncGenerator => Self::AsyncGenerator,
        }
    }

    fn is_normal(self) -> bool {
        matches!(self, Self::Normal | Self::ModuleLink)
    }

    fn needs_initial_yield(self) -> bool {
        matches!(self, Self::Generator | Self::AsyncGenerator)
    }

    fn can_await(self) -> bool {
        matches!(self, Self::Async | Self::AsyncGenerator | Self::ModuleEval)
    }

    fn module_this(self) -> Option<bool> {
        match self {
            Self::ModuleLink => Some(true),
            Self::ModuleEval => Some(false),
            _ => None,
        }
    }
}

/// What a verified parent passes to its constant-pool functions.
struct Parent<'a> {
    function: &'a Function,
    /// Locals of the parent that `set_loc_uninitialized` can leave holding
    /// the uninitialized marker.
    uninit_locals: &'a [bool],
    /// Closure variables of the parent that can hold it.
    uninit_closure: &'a [bool],
    /// Closure variables of the parent that are global bindings: the type
    /// and `var_kind` of the `GLOBAL`/`GLOBAL_DECL` entry they lead to.
    global_closure: &'a [Option<(ClosureType, u8)>],
}

struct Verifier<'a> {
    program: &'a Program,
    regexps: &'a dyn RegExpCompiler,
    depth: usize,
}

fn fail(
    path: &str,
    instruction: Option<usize>,
    rule: Rule,
    detail: impl Into<String>,
) -> VerifyError {
    VerifyError {
        function: path.to_owned(),
        instruction: instruction.and_then(|index| u32::try_from(index).ok()),
        rule,
        detail: detail.into(),
    }
}

/// A built-in atom index by text, for the names the engine compares with.
fn builtin(text: &str) -> u32 {
    builtin_string_atom(text).unwrap_or(u32::MAX)
}

impl Verifier<'_> {
    fn enter(&mut self, path: &str) -> Result<(), VerifyError> {
        self.depth += 1;
        if self.depth > NESTING_LIMIT {
            return Err(fail(
                path,
                None,
                Rule::ValuePlacement,
                format!("values nested deeper than {NESTING_LIMIT}"),
            ));
        }
        Ok(())
    }

    fn atom(&self, atom: Atom, path: &str, at: Option<usize>) -> Result<ResolvedAtom, VerifyError> {
        self.program
            .resolve(atom)
            .ok_or_else(|| fail(path, at, Rule::AtomRange, format!("atom {atom:?}")))
    }

    /// A name that can become a property key: valid, and not
    /// `JS_ATOM_NULL`, which an object's shape reads as a deleted
    /// property — the garbage collector does not mark what such a property
    /// holds, and freeing the object does not free it.
    fn key(&self, atom: Atom, path: &str, what: &str) -> Result<(), VerifyError> {
        if self.atom(atom, path, None)? == ResolvedAtom::Null {
            return Err(fail(
                path,
                None,
                Rule::NullAtom,
                format!("{what} is JS_ATOM_NULL"),
            ));
        }
        Ok(())
    }

    /// A data value: no code inside, every atom valid.
    fn data(&mut self, value: &Value, path: &str) -> Result<(), VerifyError> {
        self.enter(path)?;
        let result = self.data_inner(value, path);
        self.depth -= 1;
        result
    }

    fn data_inner(&mut self, value: &Value, path: &str) -> Result<(), VerifyError> {
        match value {
            Value::Object(properties) => {
                for (name, value) in properties {
                    self.key(*name, path, "an object key")?;
                    self.data(value, path)?;
                }
            }
            Value::Array(elements) => {
                for element in elements {
                    self.data(element, path)?;
                }
            }
            Value::TemplateObject { elements, raw } => {
                for element in elements {
                    self.data(element, path)?;
                }
                self.data(raw, path)?;
            }
            Value::Function(_) | Value::Module(_) => {
                return Err(fail(
                    path,
                    None,
                    Rule::ValuePlacement,
                    "code inside a data value",
                ));
            }
            Value::Null
            | Value::Undefined
            | Value::Bool(_)
            | Value::Int32(_)
            | Value::Float64(_)
            | Value::String(_)
            | Value::BigInt(_) => {}
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the module record rules, one after another"
    )]
    fn module(&mut self, module: &Module) -> Result<(), VerifyError> {
        let path = "module";
        self.key(module.name, path, "the module name")?;
        let function = &module.function;
        let closure_count = function.closure_vars.len();
        let requests = module.requests.len();
        let request_ok = |request: u32| (request as usize) < requests;
        for request in &module.requests {
            self.key(request.specifier, path, "a module specifier")?;
            match &request.attributes {
                Value::Undefined | Value::Object(_) => self.data(&request.attributes, path)?,
                _ => {
                    return Err(fail(
                        path,
                        None,
                        Rule::ModuleRecord,
                        "import attributes neither undefined nor an object",
                    ));
                }
            }
        }
        for export in &module.exports {
            match *export {
                Export::Local { var_idx, name } => {
                    self.key(name, path, "an export name")?;
                    let closure_var = function.closure_vars.get(var_idx as usize);
                    if closure_var.map(|closure_var| closure_var.closure_type)
                        != Some(ClosureType::ModuleDecl)
                    {
                        return Err(fail(
                            path,
                            None,
                            Rule::ModuleRecord,
                            format!(
                                "local export of closure variable {var_idx}, not a module binding"
                            ),
                        ));
                    }
                }
                Export::Indirect {
                    request,
                    local_name,
                    name,
                } => {
                    self.key(local_name, path, "an export's local name")?;
                    self.key(name, path, "an export name")?;
                    if !request_ok(request) {
                        return Err(fail(
                            path,
                            None,
                            Rule::ModuleRecord,
                            format!("export from request {request} of {requests}"),
                        ));
                    }
                }
            }
        }
        for &request in &module.star_exports {
            if !request_ok(request) {
                return Err(fail(
                    path,
                    None,
                    Rule::ModuleRecord,
                    format!("star export of request {request} of {requests}"),
                ));
            }
        }
        let mut filled = vec![false; closure_count];
        for import in &module.imports {
            self.key(import.name, path, "an import name")?;
            if !request_ok(import.request) {
                return Err(fail(
                    path,
                    None,
                    Rule::ModuleRecord,
                    format!("import from request {} of {requests}", import.request),
                ));
            }
            let expected = if import.is_star {
                ClosureType::ModuleDecl
            } else {
                ClosureType::ModuleImport
            };
            let index = import.var_idx as usize;
            if function
                .closure_vars
                .get(index)
                .map(|closure_var| closure_var.closure_type)
                != Some(expected)
            {
                return Err(fail(
                    path,
                    None,
                    Rule::ModuleRecord,
                    format!("import into closure variable {index}, not a {expected:?}"),
                ));
            }
            if !import.is_star {
                filled[index] = true;
            }
        }
        for (index, closure_var) in function.closure_vars.iter().enumerate() {
            if closure_var.closure_type == ClosureType::ModuleImport && !filled[index] {
                return Err(fail(
                    path,
                    None,
                    Rule::ModuleRecord,
                    format!("import variable {index} that no import fills"),
                ));
            }
        }
        if function.flags.kind != FunctionKind::Async || function.js_mode & js_mode::STRICT == 0 {
            return Err(fail(
                path,
                None,
                Rule::ModuleFunction,
                "a module function must be async and strict",
            ));
        }
        self.function(function, Placement::Module, None, path)
    }

    #[allow(clippy::too_many_lines, reason = "the record rules, one after another")]
    fn function(
        &mut self,
        function: &Function,
        placement: Placement,
        parent: Option<&Parent<'_>>,
        path: &str,
    ) -> Result<(), VerifyError> {
        self.enter(path)?;
        let named;
        let path = match self.program.atom_text(function.name) {
            Some(name) => {
                named = format!("{path} ({name})");
                named.as_str()
            }
            None => path,
        };

        // Record rules.
        self.atom(function.name, path, None)?;
        if function.js_mode & !(js_mode::STRICT | js_mode::BACKTRACE_BARRIER) != 0 {
            return Err(fail(
                path,
                None,
                Rule::JsMode,
                format!("js_mode {:#04x}", function.js_mode),
            ));
        }
        if function.defined_arg_count > function.arg_count {
            return Err(fail(
                path,
                None,
                Rule::DefinedArgCount,
                format!(
                    "defined_arg_count {} above arg_count {}",
                    function.defined_arg_count, function.arg_count
                ),
            ));
        }
        let locals = usize::from(function.arg_count) + usize::from(function.var_count);
        if !function.vardefs.is_empty() && function.vardefs.len() != locals {
            return Err(fail(
                path,
                None,
                Rule::VardefCount,
                format!("{} vardefs for {locals} variables", function.vardefs.len()),
            ));
        }
        if let Some(debug) = &function.debug {
            self.atom(debug.filename, path, None)?;
        }
        let mut var_refs = BTreeSet::new();
        for (index, vardef) in function.vardefs.iter().enumerate() {
            self.atom(vardef.name, path, None)?;
            if vardef.var_kind > var_kind::GLOBAL_FUNCTION_DECL {
                return Err(fail(
                    path,
                    None,
                    Rule::VarKind,
                    format!("vardef {index} var_kind {}", vardef.var_kind),
                ));
            }
            if vardef.is_captured
                && (vardef.var_ref_idx >= function.var_ref_count
                    || !var_refs.insert(vardef.var_ref_idx))
            {
                return Err(fail(
                    path,
                    None,
                    Rule::VarRefIndex,
                    format!(
                        "vardef {index}: var_ref_idx {} (var_ref_count {}, or used twice)",
                        vardef.var_ref_idx, function.var_ref_count
                    ),
                ));
            }
        }
        if var_refs.len() != usize::from(function.var_ref_count) {
            return Err(fail(
                path,
                None,
                Rule::VarRefIndex,
                format!(
                    "var_ref_count {} for {} captured variables",
                    function.var_ref_count,
                    var_refs.len()
                ),
            ));
        }
        for (index, closure_var) in function.closure_vars.iter().enumerate() {
            // A global binding's name is a key of the global object
            // (`js_closure_define_global_var`, `put_var`'s fallback).
            self.key(
                closure_var.name,
                path,
                &format!("closure variable {index}'s name"),
            )?;
            if closure_var.var_kind > var_kind::GLOBAL_FUNCTION_DECL {
                return Err(fail(
                    path,
                    None,
                    Rule::VarKind,
                    format!("closure variable {index} var_kind {}", closure_var.var_kind),
                ));
            }
            let allowed = match placement {
                Placement::ScriptRoot => {
                    matches!(
                        closure_var.closure_type,
                        ClosureType::Global | ClosureType::GlobalDecl
                    )
                }
                Placement::Module => matches!(
                    closure_var.closure_type,
                    ClosureType::Global
                        | ClosureType::GlobalDecl
                        | ClosureType::ModuleDecl
                        | ClosureType::ModuleImport
                ),
                Placement::Nested => matches!(
                    closure_var.closure_type,
                    ClosureType::Local
                        | ClosureType::Arg
                        | ClosureType::Ref
                        | ClosureType::GlobalRef
                ),
            };
            if !allowed {
                return Err(fail(
                    path,
                    None,
                    Rule::ClosureType,
                    format!(
                        "closure variable {index} is {:?} in a {placement:?} function",
                        closure_var.closure_type
                    ),
                ));
            }
            if placement == Placement::Nested {
                let parent = parent.ok_or_else(|| {
                    fail(
                        path,
                        None,
                        Rule::ClosureParent,
                        "a nested function without a parent",
                    )
                })?;
                let parent_function = parent.function;
                let index_in = closure_var.var_idx;
                let ok = match closure_var.closure_type {
                    ClosureType::Local => {
                        parent_function
                            .local_def(index_in)
                            .is_some_and(|vardef| vardef.is_captured)
                            && index_in < parent_function.var_count
                    }
                    ClosureType::Arg => parent_function
                        .arg_def(index_in)
                        .is_some_and(|vardef| vardef.is_captured),
                    ClosureType::Ref => usize::from(index_in) < parent_function.closure_vars.len(),
                    ClosureType::GlobalRef => parent_function
                        .closure_vars
                        .get(usize::from(index_in))
                        .is_some_and(|parent_var| parent_var.closure_type.is_global()),
                    _ => false,
                };
                if !ok {
                    return Err(fail(
                        path,
                        None,
                        Rule::ClosureParent,
                        format!(
                            "closure variable {index} ({:?} {index_in}) names no captured, \
                             compatible variable of the parent",
                            closure_var.closure_type
                        ),
                    ));
                }
            }
        }

        // What can hold the uninitialized marker.
        let mut uninit_locals = vec![false; usize::from(function.var_count)];
        for instruction in &function.instructions {
            if instruction.opcode() == op::SET_LOC_UNINITIALIZED
                && let Some(Operand::Local(index)) = instruction.operands().first()
                && let Some(slot) = uninit_locals.get_mut(usize::from(*index))
            {
                *slot = true;
            }
        }
        let uninit_closure: Vec<bool> = function
            .closure_vars
            .iter()
            .map(|closure_var| {
                let index = usize::from(closure_var.var_idx);
                match closure_var.closure_type {
                    ClosureType::Arg => false,
                    ClosureType::Local => parent
                        .and_then(|parent| parent.uninit_locals.get(index).copied())
                        .unwrap_or(true),
                    ClosureType::Ref => parent
                        .and_then(|parent| parent.uninit_closure.get(index).copied())
                        .unwrap_or(true),
                    ClosureType::ModuleDecl => closure_var.is_lexical,
                    ClosureType::GlobalRef
                    | ClosureType::GlobalDecl
                    | ClosureType::Global
                    | ClosureType::ModuleImport => true,
                }
            })
            .collect();
        let global_closure: Vec<Option<(ClosureType, u8)>> = function
            .closure_vars
            .iter()
            .map(|closure_var| match closure_var.closure_type {
                ClosureType::Global | ClosureType::GlobalDecl => {
                    Some((closure_var.closure_type, closure_var.var_kind))
                }
                ClosureType::Ref | ClosureType::GlobalRef => parent
                    .and_then(|parent| parent.global_closure.get(usize::from(closure_var.var_idx)))
                    .copied()
                    .flatten(),
                _ => None,
            })
            .collect();

        let code = Code {
            program: self.program,
            regexps: self.regexps,
            function,
            placement,
            path,
            uninit_locals: &uninit_locals,
            uninit_closure: &uninit_closure,
            global_closure: &global_closure,
        };
        code.static_rules()?;
        let frames = if placement == Placement::Module {
            vec![Frame::ModuleLink, Frame::ModuleEval]
        } else {
            vec![Frame::of(function.flags.kind)]
        };
        for frame in frames {
            Flow::run(&code, frame)?;
        }

        // The constant pool.
        let child_parent = Parent {
            function,
            uninit_locals: &uninit_locals,
            uninit_closure: &uninit_closure,
            global_closure: &global_closure,
        };
        for (index, entry) in function.cpool.iter().enumerate() {
            let entry_path = format!("{path}.cpool[{index}]");
            match entry {
                Value::Function(child) => {
                    self.function(child, Placement::Nested, Some(&child_parent), &entry_path)?;
                }
                other => self.data(other, &entry_path)?,
            }
        }
        self.depth -= 1;
        Ok(())
    }
}

/// One function, for the instruction rules and the flow walk.
struct Code<'a> {
    program: &'a Program,
    regexps: &'a dyn RegExpCompiler,
    function: &'a Function,
    placement: Placement,
    path: &'a str,
    uninit_locals: &'a [bool],
    uninit_closure: &'a [bool],
    global_closure: &'a [Option<(ClosureType, u8)>],
}

impl Code<'_> {
    fn fail(&self, at: usize, rule: Rule, detail: impl Into<String>) -> VerifyError {
        fail(self.path, Some(at), rule, detail)
    }

    fn name(&self, at: usize) -> &'static str {
        self.function.instructions[at].info().name
    }

    fn cpool_function(&self, index: u32) -> Option<&Function> {
        match self.function.cpool.get(index as usize) {
            Some(Value::Function(function)) => Some(function),
            _ => None,
        }
    }

    fn captured_local(&self, index: u16) -> bool {
        self.function
            .local_def(index)
            .is_some_and(|vardef| vardef.is_captured)
    }

    fn captured_arg(&self, index: u16) -> bool {
        self.function
            .arg_def(index)
            .is_some_and(|vardef| vardef.is_captured)
    }

    /// A function the parent may instantiate in this frame: one whose
    /// closure variables capture a local or argument creates variable
    /// references into the frame.
    fn captures_frame(child: &Function) -> bool {
        child.closure_vars.iter().any(|closure_var| {
            matches!(
                closure_var.closure_type,
                ClosureType::Local | ClosureType::Arg
            )
        })
    }

    #[allow(clippy::too_many_lines, reason = "one rule per instruction kind")]
    fn static_rules(&self) -> Result<(), VerifyError> {
        let function = self.function;
        let count = function.instructions.len();
        if count == 0 {
            return Err(fail(self.path, None, Rule::FallsOffEnd, "no instructions"));
        }
        let mut has_eval = false;
        for (at, instruction) in function.instructions.iter().enumerate() {
            for operand in instruction.operands() {
                let ok = match *operand {
                    Operand::Local(index) => index < function.var_count,
                    Operand::Arg(index) => index < function.arg_count,
                    Operand::ClosureVar(index) => usize::from(index) < function.closure_vars.len(),
                    Operand::Const(index) => (index as usize) < function.cpool.len(),
                    Operand::Target(index) => {
                        if (index as usize) < count
                            && index as usize <= at
                            && matches!(
                                instruction.opcode(),
                                op::CATCH
                                    | op::GOSUB
                                    | op::WITH_GET_VAR
                                    | op::WITH_PUT_VAR
                                    | op::WITH_DELETE_VAR
                                    | op::WITH_MAKE_REF
                                    | op::WITH_GET_REF
                            )
                        {
                            return Err(self.fail(
                                at,
                                Rule::BackwardTarget,
                                format!("{} to instruction {index}, not after it", self.name(at)),
                            ));
                        }
                        (index as usize) < count
                    }
                    Operand::Atom(atom) => {
                        let resolved = self.program.resolve(atom).ok_or_else(|| {
                            self.fail(at, Rule::AtomRange, format!("atom {atom:?}"))
                        })?;
                        let opcode = instruction.opcode();
                        if resolved == ResolvedAtom::Null
                            && !matches!(
                                opcode,
                                op::THROW_ERROR | op::DEFINE_CLASS | op::DEFINE_CLASS_COMPUTED
                            )
                        {
                            return Err(self.fail(
                                at,
                                Rule::NullAtom,
                                format!("{} with JS_ATOM_NULL", self.name(at)),
                            ));
                        }
                        if opcode == op::PRIVATE_SYMBOL && matches!(resolved, ResolvedAtom::Int(_))
                        {
                            return Err(self.fail(
                                at,
                                Rule::PrivateSymbolAtom,
                                "private_symbol of an integer atom",
                            ));
                        }
                        true
                    }
                    Operand::Immediate(_) => true,
                };
                if !ok {
                    return Err(self.fail(
                        at,
                        Rule::OperandRange,
                        format!("{} operand {operand:?} outside its count", self.name(at)),
                    ));
                }
            }
            let last = instruction.last_immediate().unwrap_or(0);
            let enum_ok = match instruction.opcode() {
                op::SPECIAL_OBJECT => (0..=i32::from(special_object::IMPORT_META)).contains(&last),
                op::THROW_ERROR => {
                    (0..=i32::from(throw_error::ERROR_INVALID_LVALUE_CALL)).contains(&last)
                }
                op::DEFINE_METHOD | op::DEFINE_METHOD_COMPUTED => last & !7 == 0 && last & 3 != 3,
                op::DEFINE_CLASS | op::DEFINE_CLASS_COMPUTED => matches!(last, 0 | 1),
                op::APPLY => (0..=2).contains(&last),
                op::ITERATOR_CALL => (0..=3).contains(&last),
                op::WITH_GET_VAR
                | op::WITH_PUT_VAR
                | op::WITH_DELETE_VAR
                | op::WITH_MAKE_REF
                | op::WITH_GET_REF => matches!(last, 0 | 1),
                _ => true,
            };
            if !enum_ok {
                return Err(self.fail(
                    at,
                    Rule::EnumOperand,
                    format!("{} operand {last}", self.name(at)),
                ));
            }
            let opcode = instruction.opcode();
            if !matches!(
                opcode,
                op::GET_VAR | op::GET_VAR_UNDEF | op::PUT_VAR | op::PUT_VAR_INIT
            ) && let Some(Operand::ClosureVar(index)) = instruction
                .operands()
                .iter()
                .find(|operand| matches!(operand, Operand::ClosureVar(_)))
                && let Some((closure_type, kind)) = self.global_closure[usize::from(*index)]
            {
                // A global binding may be a variable reference parked in
                // the global object's `uninitialized_vars`, which the
                // engine assumes holds the uninitialized marker (adopting
                // it overwrites its value without freeing it). Compiled
                // code reaches globals through `get_var`/`put_var`, and
                // stores a declared function with `put_var_ref`.
                let function_declaration = closure_type == ClosureType::GlobalDecl
                    && kind == var_kind::GLOBAL_FUNCTION_DECL
                    && matches!(
                        opcode,
                        op::PUT_VAR_REF
                            | op::PUT_VAR_REF0
                            | op::PUT_VAR_REF1
                            | op::PUT_VAR_REF2
                            | op::PUT_VAR_REF3
                    );
                if !function_declaration {
                    return Err(self.fail(
                        at,
                        Rule::GlobalBinding,
                        format!(
                            "{} of closure variable {index}, a global binding",
                            self.name(at)
                        ),
                    ));
                }
            }
            match opcode {
                op::FCLOSURE | op::FCLOSURE8 => {
                    if let Some(Operand::Const(index)) = instruction.operands().first()
                        && self.cpool_function(*index).is_none()
                    {
                        return Err(self.fail(
                            at,
                            Rule::ClosureConstant,
                            format!("fclosure of constant {index}, not function bytecode"),
                        ));
                    }
                }
                op::MAKE_LOC_REF | op::CLOSE_LOC => {
                    let local = instruction
                        .operands()
                        .iter()
                        .find_map(|operand| match operand {
                            Operand::Local(index) => Some(*index),
                            _ => None,
                        });
                    if let Some(index) = local
                        && !self.captured_local(index)
                    {
                        return Err(self.fail(
                            at,
                            Rule::VarRefCapture,
                            format!("{} of local {index}, not captured", self.name(at)),
                        ));
                    }
                }
                op::MAKE_ARG_REF => {
                    let arg = instruction
                        .operands()
                        .iter()
                        .find_map(|operand| match operand {
                            Operand::Arg(index) => Some(*index),
                            _ => None,
                        });
                    if let Some(index) = arg
                        && !self.captured_arg(index)
                    {
                        return Err(self.fail(
                            at,
                            Rule::VarRefCapture,
                            format!("make_arg_ref of argument {index}, not captured"),
                        ));
                    }
                }
                op::SPECIAL_OBJECT => {
                    if last == i32::from(special_object::MAPPED_ARGUMENTS)
                        && !(0..function.arg_count).all(|index| self.captured_arg(index))
                    {
                        return Err(self.fail(
                            at,
                            Rule::VarRefCapture,
                            "a mapped arguments object over arguments that are not all captured",
                        ));
                    }
                    if self.placement == Placement::Module
                        && (last == i32::from(special_object::THIS_FUNC)
                            || last == i32::from(special_object::MAPPED_ARGUMENTS))
                    {
                        return Err(self.fail(
                            at,
                            Rule::ModuleThisFunc,
                            "special_object exposing the module function",
                        ));
                    }
                }
                op::EVAL | op::APPLY_EVAL => {
                    has_eval = true;
                    self.eval_scope(at, instruction)?;
                }
                op::REGEXP => self.regexp(at)?,
                _ => {}
            }
        }
        if has_eval {
            // The eval compiler reads `is_lexical` to choose between checked
            // and unchecked reads of what it captures.
            for (index, closure_var) in function.closure_vars.iter().enumerate() {
                if !closure_var.closure_type.is_global()
                    && self.uninit_closure[index]
                    && !closure_var.is_lexical
                {
                    return Err(fail(
                        self.path,
                        None,
                        Rule::Uninitialized,
                        format!(
                            "closure variable {index} can be uninitialized but is not lexical, \
                             in a function with direct eval"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `add_closure_variables`, as the eval compiler runs it on this
    /// function's frame: every variable it captures must be captured, and
    /// its scope walk must end.
    fn eval_scope(&self, at: usize, instruction: &Instruction) -> Result<(), VerifyError> {
        let function = self.function;
        let scope_operand = instruction.last_immediate().unwrap_or(0);
        let scope = scope_operand + crate::bytecode_abi::arg_scope::END;
        let total = usize::from(function.arg_count)
            + usize::from(function.var_count)
            + function.closure_vars.len();
        if total == 0 {
            return Ok(());
        }
        let has_vardefs = !function.vardefs.is_empty();
        if usize::from(function.arg_count) + usize::from(function.var_count) > 0 && !has_vardefs {
            return Err(self.fail(
                at,
                Rule::VarRefCapture,
                "direct eval in a function without vardefs",
            ));
        }
        let need_local = |index: u16| -> Result<(), VerifyError> {
            if self.captured_local(index) {
                Ok(())
            } else {
                Err(self.fail(
                    at,
                    Rule::VarRefCapture,
                    format!("direct eval can capture local {index}, which is not captured"),
                ))
            }
        };
        let mut index = scope;
        let mut steps = 0usize;
        while index >= 0 {
            steps += 1;
            let local = u16::try_from(index)
                .ok()
                .filter(|&local| {
                    local < function.var_count && steps <= usize::from(function.var_count)
                })
                .ok_or_else(|| {
                    self.fail(
                        at,
                        Rule::EvalScope,
                        format!("direct eval scope walk reaches {index} after {steps} steps"),
                    )
                })?;
            let vardef = function.local_def(local).ok_or_else(|| {
                self.fail(
                    at,
                    Rule::EvalScope,
                    "direct eval scope walk without vardefs",
                )
            })?;
            if vardef.has_scope {
                need_local(local)?;
            }
            index = vardef.scope_next;
        }
        let ret = builtin("<ret>");
        let resolved_index = |atom: Atom| match self.program.resolve(atom) {
            Some(ResolvedAtom::BuiltinString(index)) => Some(index),
            _ => None,
        };
        if index == crate::bytecode_abi::arg_scope::END {
            let pseudo = [
                builtin("<home_object>"),
                builtin("this.active_func"),
                builtin("new.target"),
                builtin("this"),
                builtin("<arg_var>"),
            ];
            for local in 0..function.var_count {
                let vardef = function
                    .local_def(local)
                    .ok_or_else(|| self.fail(at, Rule::EvalScope, "direct eval without vardefs"))?;
                let in_arg_scope = vardef.var_kind == var_kind::FUNCTION_NAME
                    || resolved_index(vardef.name).is_some_and(|name| pseudo.contains(&name));
                if !vardef.has_scope && in_arg_scope {
                    need_local(local)?;
                }
            }
        } else {
            for arg in 0..function.arg_count {
                if !self.captured_arg(arg) {
                    return Err(self.fail(
                        at,
                        Rule::VarRefCapture,
                        format!("direct eval captures argument {arg}, which is not captured"),
                    ));
                }
            }
            for local in 0..function.var_count {
                let vardef = function
                    .local_def(local)
                    .ok_or_else(|| self.fail(at, Rule::EvalScope, "direct eval without vardefs"))?;
                if !vardef.has_scope && resolved_index(vardef.name) != Some(ret) {
                    need_local(local)?;
                }
            }
        }
        Ok(())
    }

    /// `push_const pattern; push_const bytecode; regexp`, with the bytecode
    /// what the engine compiles for that pattern and the flags in the
    /// bytecode's header.
    fn regexp(&self, at: usize) -> Result<(), VerifyError> {
        let function = self.function;
        let constant = |index: usize| -> Option<&Value> {
            let instruction = function.instructions.get(index)?;
            if !matches!(instruction.opcode(), op::PUSH_CONST | op::PUSH_CONST8) {
                return None;
            }
            match instruction.operands().first() {
                Some(Operand::Const(entry)) => function.cpool.get(*entry as usize),
                _ => None,
            }
        };
        let (Some(pattern_at), Some(bytecode_at)) = (at.checked_sub(2), at.checked_sub(1)) else {
            return Err(self.fail(at, Rule::RegExp, "regexp without two constants before it"));
        };
        let (Some(Value::String(pattern)), Some(Value::String(bytecode))) =
            (constant(pattern_at), constant(bytecode_at))
        else {
            return Err(self.fail(
                at,
                Rule::RegExp,
                "regexp not directly after push_const of two strings",
            ));
        };
        if self.jump_targets_before(at) {
            return Err(self.fail(
                at,
                Rule::RegExp,
                "a jump lands on the regexp or the push_const before it",
            ));
        }
        let JsString::Latin1(bytes) = bytecode else {
            return Err(self.fail(at, Rule::RegExp, "regexp bytecode is not an 8-bit string"));
        };
        if bytes.len() < 2 {
            return Err(self.fail(at, Rule::RegExp, "regexp bytecode shorter than its header"));
        }
        let header = u16::from_le_bytes([bytes[0], bytes[1]]);
        let mut flags = String::new();
        for (bit, letter) in [
            (regexp_flag::INDICES, 'd'),
            (regexp_flag::GLOBAL, 'g'),
            (regexp_flag::IGNORECASE, 'i'),
            (regexp_flag::MULTILINE, 'm'),
            (regexp_flag::DOTALL, 's'),
            (regexp_flag::UNICODE, 'u'),
            (regexp_flag::UNICODE_SETS, 'v'),
            (regexp_flag::STICKY, 'y'),
        ] {
            if header & bit != 0 {
                flags.push(letter);
            }
        }
        let compiled = self
            .regexps
            .compile_regexp(&pattern.to_utf16(), &flags)
            .ok_or_else(|| {
                self.fail(at, Rule::RegExp, "the engine refuses the regexp's pattern")
            })?;
        if compiled != *bytes {
            return Err(self.fail(
                at,
                Rule::RegExp,
                "regexp bytecode differs from what the engine compiles for its pattern and flags",
            ));
        }
        Ok(())
    }

    /// Whether any jump lands on `at` or `at - 1`.
    fn jump_targets_before(&self, at: usize) -> bool {
        self.function.instructions.iter().any(|instruction| {
            instruction
                .target()
                .is_some_and(|target| target as usize == at || target as usize + 1 == at)
        })
    }
}

/// One slot kind of the operand stack other than an ordinary value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// A catch offset `catch` (this instruction) pushed.
    Catch(u32),
    /// The catch offset 0 that marks an iterator triple.
    Marker,
    /// A return address the `gosub`s of the set pushed.
    Ret(BTreeSet<u32>),
    /// Function bytecode `push_const` pushed (this constant).
    Bytecode(u32),
    /// An ordinary value known to be the module function's `this`:
    /// `true` when linked, `undefined` when evaluated.
    This(bool),
    /// A value an unchecked read took from a variable that can hold the
    /// uninitialized marker (the compiler reads private names this way).
    /// Only the instructions that test their operand's tag take it.
    Taint,
    /// An ordinary value known to be `undefined`, which holds no
    /// reference: what the instructions that overwrite or drop a slot
    /// without freeing it need there.
    Undef,
}

impl Kind {
    fn ordinary(kind: Option<&Self>) -> bool {
        matches!(kind, None | Some(Self::This(_) | Self::Undef))
    }

    /// A slot whose value holds no reference, so overwriting it leaks
    /// nothing.
    fn unreferenced(kind: Option<&Self>) -> bool {
        matches!(kind, Some(Self::Marker | Self::Undef))
    }

    /// What an instruction that only moves values may move: anything but
    /// a catch offset, a return address or function bytecode.
    fn movable(kind: Option<&Self>) -> bool {
        !matches!(
            kind,
            Some(Self::Catch(_) | Self::Ret(_) | Self::Bytecode(_))
        )
    }

    /// The kind a moved slot keeps (`this` becomes an ordinary value).
    fn moved(kind: Option<&Self>) -> Option<Self> {
        match kind {
            Some(Self::Marker) => Some(Self::Marker),
            Some(Self::Taint) => Some(Self::Taint),
            Some(Self::Undef) => Some(Self::Undef),
            _ => None,
        }
    }

    /// Catch offsets, markers, return addresses and function bytecode.
    fn special(&self) -> bool {
        matches!(
            self,
            Self::Catch(_) | Self::Marker | Self::Ret(_) | Self::Bytecode(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    depth: u32,
    /// The fork walk's depth (`compute_stack_size_buf`).
    c_depth: u32,
    /// The fork walk's catch position.
    c_catch: Option<u32>,
    /// Whether `initial_yield` has run.
    started: bool,
    /// Locals that can hold the uninitialized marker here, sorted.
    uninit: Vec<u16>,
    /// The special slots, sorted by index.
    slots: Vec<(u32, Kind)>,
}

impl State {
    fn kind(&self, index: u32) -> Option<&Kind> {
        self.slots
            .binary_search_by_key(&index, |(slot, _)| *slot)
            .ok()
            .map(|position| &self.slots[position].1)
    }

    fn set(&mut self, index: u32, kind: Option<Kind>) {
        match self.slots.binary_search_by_key(&index, |(slot, _)| *slot) {
            Ok(position) => match kind {
                Some(kind) => self.slots[position].1 = kind,
                None => {
                    self.slots.remove(position);
                }
            },
            Err(position) => {
                if let Some(kind) = kind {
                    self.slots.insert(position, (index, kind));
                }
            }
        }
    }

    fn truncate(&mut self, depth: u32) {
        self.slots.retain(|(index, _)| *index < depth);
        self.depth = depth;
    }
}

enum EdgeKind {
    /// An edge the fork's walk also follows.
    Normal,
    /// A `ret` returning after a `gosub`: the fork's walk does not follow
    /// it, so only the interpreter's state is compared.
    Return,
}

struct Flow<'a, 'b> {
    code: &'a Code<'b>,
    frame: Frame,
    states: Vec<Option<State>>,
    work: Vec<usize>,
}

impl<'a, 'b> Flow<'a, 'b> {
    fn run(code: &'a Code<'b>, frame: Frame) -> Result<(), VerifyError> {
        let count = code.function.instructions.len();
        let mut flow = Flow {
            code,
            frame,
            states: vec![None; count],
            work: Vec::new(),
        };
        flow.states[0] = Some(State {
            depth: 0,
            c_depth: 0,
            c_catch: None,
            started: !frame.needs_initial_yield(),
            uninit: Vec::new(),
            slots: Vec::new(),
        });
        flow.work.push(0);
        while let Some(at) = flow.work.pop() {
            let Some(state) = flow.states[at].clone() else {
                continue;
            };
            let edges = flow.step(at, &state)?;
            for (target, next, kind) in edges {
                flow.check_state(at, &next)?;
                flow.merge(at, target, next, &kind)?;
            }
        }
        Ok(())
    }

    fn fail(&self, at: usize, rule: Rule, detail: impl Into<String>) -> VerifyError {
        self.code.fail(at, rule, detail)
    }

    fn check_state(&self, at: usize, state: &State) -> Result<(), VerifyError> {
        if state.depth > limits::JS_STACK_SIZE_MAX {
            return Err(self.fail(at, Rule::StackDepth, "stack deeper than JS_STACK_SIZE_MAX"));
        }
        if state.c_depth < state.depth || state.c_depth > limits::JS_STACK_SIZE_MAX {
            return Err(self.fail(
                at,
                Rule::ForkWalk,
                format!(
                    "the fork's walk computes depth {} where the stack is {} deep",
                    state.c_depth, state.depth
                ),
            ));
        }
        for (index, kind) in &state.slots {
            if *kind == Kind::Marker
                && (*index < 2
                    || !Kind::ordinary(state.kind(index - 1))
                    || !Kind::ordinary(state.kind(index - 2)))
            {
                return Err(self.fail(
                    at,
                    Rule::IteratorSlots,
                    format!("iterator marker at slot {index} without an iterator and next below"),
                ));
            }
        }
        Ok(())
    }

    fn merge(
        &mut self,
        at: usize,
        target: usize,
        next: State,
        kind: &EdgeKind,
    ) -> Result<(), VerifyError> {
        if target >= self.states.len() {
            return Err(self.fail(
                at,
                Rule::FallsOffEnd,
                "control reaches the end of the function",
            ));
        }
        let Some(existing) = &mut self.states[target] else {
            if matches!(kind, EdgeKind::Return) {
                return Err(self.code.fail(
                    at,
                    Rule::Ret,
                    "ret to an instruction no gosub reached",
                ));
            }
            self.states[target] = Some(next);
            self.work.push(target);
            return Ok(());
        };
        let mismatch = |detail: String| fail(self.code.path, Some(target), Rule::Merge, detail);
        if existing.depth != next.depth || existing.started != next.started {
            return Err(mismatch(format!(
                "paths meet with depth {} and {} (from instruction {at})",
                existing.depth, next.depth
            )));
        }
        if matches!(kind, EdgeKind::Normal)
            && (existing.c_depth != next.c_depth || existing.c_catch != next.c_catch)
        {
            return Err(fail(
                self.code.path,
                Some(target),
                Rule::ForkWalk,
                "the fork's walk meets itself with a different depth or catch position",
            ));
        }
        let mut changed = false;
        for local in &next.uninit {
            if let Err(position) = existing.uninit.binary_search(local) {
                existing.uninit.insert(position, *local);
                changed = true;
            }
        }
        let indices: BTreeSet<u32> = existing
            .slots
            .iter()
            .chain(next.slots.iter())
            .map(|(index, _)| *index)
            .collect();
        for index in indices {
            let old = existing.kind(index).cloned();
            let new = next.kind(index).cloned();
            match (old, new) {
                (old, new) if old == new => {}
                (Some(Kind::Ret(mut set)), Some(Kind::Ret(more))) => {
                    let before = set.len();
                    set.extend(more);
                    if set.len() != before {
                        existing.set(index, Some(Kind::Ret(set)));
                        changed = true;
                    }
                }
                (Some(Kind::This(_) | Kind::Undef), None | Some(Kind::This(_) | Kind::Undef)) => {
                    existing.set(index, None);
                    changed = true;
                }
                (None, Some(Kind::This(_) | Kind::Undef))
                | (Some(Kind::Taint), None | Some(Kind::This(_) | Kind::Undef)) => {}
                (None | Some(Kind::This(_) | Kind::Undef), Some(Kind::Taint)) => {
                    existing.set(index, Some(Kind::Taint));
                    changed = true;
                }
                (old, new) => {
                    return Err(mismatch(format!(
                        "paths meet with slot {index} as {old:?} and {new:?} (from instruction {at})"
                    )));
                }
            }
        }
        if changed {
            self.work.push(target);
        }
        Ok(())
    }

    /// Pops `count` ordinary values.
    fn pop(&self, at: usize, state: &mut State, count: u32) -> Result<(), VerifyError> {
        if state.depth < count {
            return Err(self.fail(
                at,
                Rule::StackDepth,
                format!(
                    "{} pops {count} from a stack {} deep",
                    self.code.name(at),
                    state.depth
                ),
            ));
        }
        for index in state.depth - count..state.depth {
            if state.kind(index) == Some(&Kind::Taint) {
                return Err(self.fail(
                    at,
                    Rule::Uninitialized,
                    format!(
                        "{} takes slot {index}, which can hold the uninitialized marker",
                        self.code.name(at)
                    ),
                ));
            }
            if !Kind::ordinary(state.kind(index)) {
                return Err(self.fail(
                    at,
                    Rule::SpecialSlot,
                    format!(
                        "{} takes slot {index}, which holds {:?}",
                        self.code.name(at),
                        state.kind(index)
                    ),
                ));
            }
        }
        state.truncate(state.depth - count);
        Ok(())
    }

    fn need(&self, at: usize, state: &State, count: u32) -> Result<(), VerifyError> {
        if state.depth < count {
            return Err(self.fail(
                at,
                Rule::StackDepth,
                format!(
                    "{} needs {count} slots of a stack {} deep",
                    self.code.name(at),
                    state.depth
                ),
            ));
        }
        Ok(())
    }

    fn require(
        &self,
        at: usize,
        state: &State,
        index: u32,
        ok: impl Fn(Option<&Kind>) -> bool,
        what: &str,
    ) -> Result<(), VerifyError> {
        let kind = state.kind(index);
        if ok(kind) {
            Ok(())
        } else {
            Err(self.fail(
                at,
                Rule::IteratorSlots,
                format!(
                    "{} expects {what} in slot {index}, found {kind:?}",
                    self.code.name(at)
                ),
            ))
        }
    }

    fn frame_rule(&self, at: usize, ok: bool, detail: &str) -> Result<(), VerifyError> {
        if ok {
            Ok(())
        } else {
            Err(self.fail(
                at,
                Rule::FrameKind,
                format!("{} {detail} ({:?} frame)", self.code.name(at), self.frame),
            ))
        }
    }

    /// The fork walk's `check_catch`.
    fn c_check_catch(&self, state: &mut State, catch_level: u32) {
        if let Some(position) = state.c_catch {
            let entry = self.states[position as usize].as_ref();
            let opcode = self.code.function.instructions[position as usize].opcode();
            if let Some(entry) = entry {
                let level = entry.c_depth + u32::from(opcode != op::CATCH);
                if catch_level == level {
                    state.c_catch = entry.c_catch;
                }
            }
        }
    }

    #[allow(clippy::too_many_lines, reason = "one case per instruction kind")]
    fn step(&self, at: usize, state: &State) -> Result<Vec<(usize, State, EdgeKind)>, VerifyError> {
        let function = self.code.function;
        let instruction = &function.instructions[at];
        let opcode = instruction.opcode();
        let next = at + 1;
        let target = instruction.target().map(|target| target as usize);
        let pops = instruction.pops();
        let pushes = instruction.pushes();

        // The fork's walk.
        if state.c_depth < pops {
            return Err(self.fail(at, Rule::StackDepth, "stack underflow in the fork's walk"));
        }
        let mut out = state.clone();
        out.c_depth = state.c_depth - pops + pushes;

        let link = self.frame == Frame::ModuleLink;
        if link
            && !matches!(
                opcode,
                op::PUSH_THIS
                    | op::IF_FALSE
                    | op::IF_FALSE8
                    | op::IF_TRUE
                    | op::IF_TRUE8
                    | op::FCLOSURE
                    | op::FCLOSURE8
                    | op::PUT_VAR_REF
                    | op::PUT_VAR_REF0
                    | op::PUT_VAR_REF1
                    | op::PUT_VAR_REF2
                    | op::PUT_VAR_REF3
                    | op::SET_NAME
                    | op::UNDEFINED
                    | op::RETURN_UNDEF
            )
            && !(matches!(
                opcode,
                op::PUT_LOC
                    | op::PUT_LOC8
                    | op::PUT_LOC0
                    | op::PUT_LOC1
                    | op::PUT_LOC2
                    | op::PUT_LOC3
            ) && state.depth > 0
                && matches!(
                    state.kind(state.depth - 1),
                    Some(Kind::This(_) | Kind::Undef)
                ))
        {
            // A module links in a normal frame of an async function: an
            // exception there leaves through the generator exit, which
            // neither frees the frame's values nor closes its variable
            // references. Only the instructions of the compiler's linking
            // prologue may run, none of which throws but for running out
            // of memory, and a local may only receive `this` or
            // `undefined`, which hold no reference to leak.
            return Err(self.fail(
                at,
                Rule::FrameKind,
                format!("{} while a module links", self.code.name(at)),
            ));
        }
        let mut edges = Vec::with_capacity(2);
        match opcode {
            op::DROP => {
                self.need(at, &out, 1)?;
                out.truncate(out.depth - 1);
                let level = out.c_depth;
                self.c_check_catch(&mut out, level);
                edges.push((next, out, EdgeKind::Normal));
            }
            op::NIP | op::NIP1 => {
                let moved = if opcode == op::NIP { 1 } else { 2 };
                self.need(at, &out, moved + 1)?;
                let mut moved_kinds = Vec::new();
                for index in out.depth - moved..out.depth {
                    match out.kind(index) {
                        kind if Kind::movable(kind) && kind != Some(&Kind::Marker) => {
                            moved_kinds.push(Kind::moved(kind));
                        }
                        _ => {
                            return Err(self.fail(
                                at,
                                Rule::SpecialSlot,
                                format!(
                                    "{} moves slot {index}, which holds {:?}",
                                    self.code.name(at),
                                    out.kind(index)
                                ),
                            ));
                        }
                    }
                }
                let base = out.depth - moved - 1;
                out.truncate(base);
                out.depth += moved;
                for (offset, kind) in moved_kinds.into_iter().enumerate() {
                    out.set(base + offset as u32, kind);
                }
                let level = out.c_depth - 1;
                self.c_check_catch(&mut out, level);
                edges.push((next, out, EdgeKind::Normal));
            }
            op::SWAP
            | op::SWAP2
            | op::ROT3L
            | op::ROT3R
            | op::ROT4L
            | op::ROT5L
            | op::PERM3
            | op::PERM4
            | op::PERM5 => {
                let order: &[u32] = match opcode {
                    op::SWAP => &[1, 0],
                    op::SWAP2 => &[2, 3, 0, 1],
                    op::ROT3L => &[1, 2, 0],
                    op::ROT3R => &[2, 0, 1],
                    op::ROT4L => &[1, 2, 3, 0],
                    op::ROT5L => &[1, 2, 3, 4, 0],
                    op::PERM3 => &[1, 0, 2],
                    op::PERM4 => &[2, 0, 1, 3],
                    _ => &[3, 0, 1, 2, 4],
                };
                let width = order.len() as u32;
                self.need(at, &out, width)?;
                let base = out.depth - width;
                let mut kinds = Vec::with_capacity(order.len());
                for index in base..out.depth {
                    match out.kind(index) {
                        kind if Kind::movable(kind) => kinds.push(Kind::moved(kind)),
                        _ => {
                            return Err(self.fail(
                                at,
                                Rule::SpecialSlot,
                                format!(
                                    "{} moves slot {index}, which holds {:?}",
                                    self.code.name(at),
                                    out.kind(index)
                                ),
                            ));
                        }
                    }
                }
                for (offset, &from) in order.iter().enumerate() {
                    out.set(base + offset as u32, kinds[from as usize].clone());
                }
                edges.push((next, out, EdgeKind::Normal));
            }
            op::FOR_OF_START | op::FOR_AWAIT_OF_START => {
                self.pop(at, &mut out, 1)?;
                out.depth += 3;
                out.set(out.depth - 1, Some(Kind::Marker));
                out.c_catch = Some(at as u32);
                edges.push((next, out, EdgeKind::Normal));
            }
            op::FOR_OF_NEXT => {
                let offset = u32::try_from(instruction.first_immediate().unwrap_or(0)).unwrap_or(0);
                self.need(at, &out, 3 + offset)?;
                let base = out.depth - 3 - offset;
                self.require(at, &out, base, Kind::ordinary, "the iterator")?;
                self.require(at, &out, base + 1, Kind::ordinary, "the next method")?;
                self.require(
                    at,
                    &out,
                    base + 2,
                    |kind| kind == Some(&Kind::Marker),
                    "the iterator marker",
                )?;
                out.set(base, None);
                out.depth += 2;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::FOR_AWAIT_OF_NEXT => {
                self.need(at, &out, 3)?;
                let base = out.depth - 3;
                self.require(at, &out, base, Kind::ordinary, "the iterator")?;
                self.require(at, &out, base + 1, Kind::ordinary, "the next method")?;
                // `js_for_await_of_next` overwrites the marker slot with
                // `undefined` without freeing it.
                self.require(
                    at,
                    &out,
                    base + 2,
                    Kind::unreferenced,
                    "the iterator marker or undefined",
                )?;
                out.set(base + 2, Some(Kind::Undef));
                out.depth += 1;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::ITERATOR_GET_VALUE_DONE => {
                self.need(at, &out, 4)?;
                let base = out.depth - 4;
                self.require(at, &out, base, Kind::ordinary, "the iterator")?;
                self.require(at, &out, base + 1, Kind::ordinary, "the next method")?;
                // `js_iterator_get_value_done` overwrites this slot with the
                // marker without freeing it.
                self.require(
                    at,
                    &out,
                    base + 2,
                    Kind::unreferenced,
                    "the iterator marker or undefined",
                )?;
                self.require(at, &out, base + 3, Kind::ordinary, "the result object")?;
                out.set(base + 2, Some(Kind::Marker));
                out.set(base + 3, None);
                out.depth += 1;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::ITERATOR_NEXT | op::ITERATOR_CALL => {
                self.need(at, &out, 4)?;
                let base = out.depth - 4;
                self.require(at, &out, base, Kind::ordinary, "the iterator")?;
                self.require(at, &out, base + 1, Kind::ordinary, "the next method")?;
                self.require(
                    at,
                    &out,
                    base + 2,
                    |kind| Kind::ordinary(kind) || kind == Some(&Kind::Marker),
                    "the iterator marker",
                )?;
                self.require(at, &out, base + 3, Kind::ordinary, "a value")?;
                out.set(base + 3, None);
                if opcode == op::ITERATOR_CALL {
                    out.depth += 1;
                }
                edges.push((next, out, EdgeKind::Normal));
            }
            op::ITERATOR_CLOSE => {
                self.need(at, &out, 3)?;
                let base = out.depth - 3;
                self.require(at, &out, base, Kind::ordinary, "the iterator")?;
                self.require(at, &out, base + 1, Kind::ordinary, "the next method")?;
                // The top slot is dropped without being freed.
                self.require(
                    at,
                    &out,
                    base + 2,
                    Kind::unreferenced,
                    "the iterator marker or undefined",
                )?;
                out.truncate(base);
                let level = out.c_depth + 2;
                self.c_check_catch(&mut out, level);
                edges.push((next, out, EdgeKind::Normal));
            }
            op::NIP_CATCH => {
                self.need(at, &out, 1)?;
                if !Kind::ordinary(out.kind(out.depth - 1)) {
                    return Err(self.fail(at, Rule::SpecialSlot, "nip_catch keeps a special slot"));
                }
                let found = out
                    .slots
                    .iter()
                    .rev()
                    .find(|(index, kind)| {
                        *index < out.depth - 1 && matches!(kind, Kind::Catch(_) | Kind::Marker)
                    })
                    .map(|(index, _)| *index)
                    .ok_or_else(|| {
                        self.fail(at, Rule::NipCatch, "nip_catch with no catch offset below")
                    })?;
                out.truncate(found);
                out.depth = found + 1;
                let position = state.c_catch.ok_or_else(|| {
                    self.fail(
                        at,
                        Rule::ForkWalk,
                        "the fork's walk has no catch position at nip_catch",
                    )
                })?;
                let entry = self.states[position as usize].as_ref().ok_or_else(|| {
                    self.fail(
                        at,
                        Rule::ForkWalk,
                        "the fork's catch position was not reached",
                    )
                })?;
                let opcode_at = function.instructions[position as usize].opcode();
                out.c_depth = entry.c_depth + u32::from(opcode_at != op::CATCH) + 1;
                out.c_catch = entry.c_catch;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::CATCH => {
                // Not instruction 0: the static rules require the handler after it.
                let handler = target.unwrap_or(0);
                let mut entry = out.clone();
                entry.depth += 1;
                // The handler is entered from anywhere in the try region:
                // any local that can be uninitialized may be.
                entry.uninit = (0..function.var_count)
                    .filter(|&local| {
                        self.code.uninit_locals[usize::from(local)]
                            || state.uninit.binary_search(&local).is_ok()
                    })
                    .collect();
                edges.push((handler, entry, EdgeKind::Normal));
                out.depth += 1;
                out.set(out.depth - 1, Some(Kind::Catch(at as u32)));
                out.c_catch = Some(at as u32);
                edges.push((next, out, EdgeKind::Normal));
            }
            op::GOSUB => {
                let mut entry = out.clone();
                entry.depth += 1;
                entry.c_depth += 1;
                entry.set(
                    entry.depth - 1,
                    Some(Kind::Ret(BTreeSet::from([at as u32]))),
                );
                edges.push((target.unwrap_or(0), entry, EdgeKind::Normal));
                edges.push((next, out, EdgeKind::Normal));
            }
            op::RET => {
                self.need(at, &out, 1)?;
                let Some(Kind::Ret(gosubs)) = out.kind(out.depth - 1).cloned() else {
                    return Err(self.fail(at, Rule::Ret, "ret without a return address on top"));
                };
                out.truncate(out.depth - 1);
                for gosub in gosubs {
                    edges.push((gosub as usize + 1, out.clone(), EdgeKind::Return));
                }
            }
            op::IF_FALSE | op::IF_TRUE | op::IF_FALSE8 | op::IF_TRUE8 => {
                self.need(at, &out, 1)?;
                let known = match out.kind(out.depth - 1) {
                    Some(Kind::This(value)) => Some(*value),
                    _ => None,
                };
                self.pop(at, &mut out, 1)?;
                let jumps_when = matches!(opcode, op::IF_TRUE | op::IF_TRUE8);
                if known != Some(!jumps_when) {
                    edges.push((target.unwrap_or(0), out.clone(), EdgeKind::Normal));
                }
                if known != Some(jumps_when) {
                    edges.push((next, out, EdgeKind::Normal));
                }
            }
            op::GOTO | op::GOTO8 | op::GOTO16 => {
                edges.push((target.unwrap_or(0), out, EdgeKind::Normal));
            }
            op::WITH_GET_VAR
            | op::WITH_DELETE_VAR
            | op::WITH_MAKE_REF
            | op::WITH_GET_REF
            | op::WITH_PUT_VAR => {
                let (popped, jump_pushes): (u32, u32) = match opcode {
                    op::WITH_GET_VAR | op::WITH_DELETE_VAR => (1, 1),
                    op::WITH_MAKE_REF | op::WITH_GET_REF => (1, 2),
                    _ => (2, 0),
                };
                // The object is tested with `JS_HasProperty`, which answers
                // false for anything that is not an object.
                if out.depth >= 1 && out.kind(out.depth - 1) == Some(&Kind::Taint) {
                    out.set(out.depth - 1, None);
                }
                self.pop(at, &mut out, popped)?;
                let mut jumped = out.clone();
                jumped.depth += jump_pushes;
                // The fork's walk: fallthrough `n_push - n_pop`, target as
                // in `compute_stack_size_buf`.
                jumped.c_depth = match opcode {
                    op::WITH_GET_VAR | op::WITH_DELETE_VAR => out.c_depth + 1,
                    op::WITH_MAKE_REF | op::WITH_GET_REF => out.c_depth + 2,
                    _ => out.c_depth - 1,
                };
                edges.push((target.unwrap_or(0), jumped, EdgeKind::Normal));
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::RETURN | op::RETURN_UNDEF | op::TAIL_CALL | op::TAIL_CALL_METHOD => {
                self.frame_rule(
                    at,
                    self.frame.is_normal(),
                    "returns from a generator or async frame",
                )?;
                self.pop(at, &mut out, pops)?;
            }
            op::THROW | op::THROW_ERROR => {
                self.pop(at, &mut out, pops)?;
            }
            op::RETURN_ASYNC => {
                self.frame_rule(at, !self.frame.is_normal(), "in a normal frame")?;
                self.frame_rule(at, state.started, "before initial_yield")?;
                self.pop(at, &mut out, 1)?;
            }
            op::INITIAL_YIELD => {
                self.frame_rule(at, self.frame.needs_initial_yield(), "outside a generator")?;
                self.frame_rule(at, !state.started, "after initial_yield")?;
                if out.slots.iter().any(|(_, kind)| kind.special()) {
                    return Err(self.fail(
                        at,
                        Rule::FrameKind,
                        "initial_yield with special slots on the stack",
                    ));
                }
                out.started = true;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::AWAIT | op::YIELD | op::YIELD_STAR | op::ASYNC_YIELD_STAR => {
                let allowed = match opcode {
                    op::AWAIT => self.frame.can_await(),
                    op::YIELD => self.frame.needs_initial_yield(),
                    op::YIELD_STAR => self.frame == Frame::Generator,
                    _ => self.frame == Frame::AsyncGenerator,
                };
                self.frame_rule(at, allowed, "in a frame that cannot suspend this way")?;
                self.frame_rule(at, state.started, "before initial_yield")?;
                self.pop(at, &mut out, pops)?;
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::PUSH_CONST | op::PUSH_CONST8 => {
                out.depth += 1;
                if let Some(Operand::Const(index)) = instruction.operands().first()
                    && self.code.cpool_function(*index).is_some()
                {
                    out.set(out.depth - 1, Some(Kind::Bytecode(*index)));
                }
                edges.push((next, out, EdgeKind::Normal));
            }
            op::UNDEFINED => {
                out.depth += 1;
                out.set(out.depth - 1, Some(Kind::Undef));
                edges.push((next, out, EdgeKind::Normal));
            }
            op::PUSH_THIS => {
                out.depth += 1;
                if let Some(value) = self.frame.module_this() {
                    out.set(out.depth - 1, Some(Kind::This(value)));
                }
                edges.push((next, out, EdgeKind::Normal));
            }
            op::DEFINE_CLASS | op::DEFINE_CLASS_COMPUTED => {
                self.need(at, &out, pops)?;
                let Some(Kind::Bytecode(index)) = out.kind(out.depth - 1).cloned() else {
                    return Err(self.fail(
                        at,
                        Rule::SpecialSlot,
                        "define_class without function bytecode on top",
                    ));
                };
                let child = self.code.cpool_function(index).ok_or_else(|| {
                    self.fail(
                        at,
                        Rule::ClosureConstant,
                        "define_class of a constant that is not function bytecode",
                    )
                })?;
                // Without the heritage flag, `js_op_define_class` replaces
                // its parent class with `Function.prototype` and overwrites
                // `sp[-2]` without freeing what was there: only `undefined`
                // may be.
                if instruction.last_immediate().unwrap_or(0) & 1 == 0
                    && out.kind(out.depth - 2) != Some(&Kind::Undef)
                {
                    return Err(self.fail(
                        at,
                        Rule::SpecialSlot,
                        "define_class without a parent class over a value that is not undefined",
                    ));
                }
                if child.flags.kind != FunctionKind::Normal {
                    return Err(self.fail(
                        at,
                        Rule::ClassConstructor,
                        "class constructor is not a normal function",
                    ));
                }
                if link && Code::captures_frame(child) {
                    return Err(self.fail(
                        at,
                        Rule::FrameKind,
                        "a closure over the frame while a module links",
                    ));
                }
                out.set(out.depth - 1, None);
                self.pop(at, &mut out, pops)?;
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::GET_LOC
            | op::GET_LOC8
            | op::GET_LOC0
            | op::GET_LOC1
            | op::GET_LOC2
            | op::GET_LOC3
            | op::GET_VAR_REF
            | op::GET_VAR_REF0
            | op::GET_VAR_REF1
            | op::GET_VAR_REF2
            | op::GET_VAR_REF3 => {
                let uninit = match instruction.operands().first() {
                    Some(Operand::Local(index)) => state.uninit.binary_search(index).is_ok(),
                    Some(Operand::ClosureVar(index)) => {
                        self.code.uninit_closure[usize::from(*index)]
                    }
                    _ => false,
                };
                out.depth += 1;
                if uninit {
                    out.set(out.depth - 1, Some(Kind::Taint));
                }
                edges.push((next, out, EdgeKind::Normal));
            }
            op::GET_PRIVATE_FIELD
            | op::PUT_PRIVATE_FIELD
            | op::PRIVATE_IN
            | op::DEFINE_PRIVATE_FIELD
            | op::CHECK_BRAND => {
                // These test the tag of their name (or brand) operand and
                // throw for anything but a symbol (or a method):
                // `JS_GetPrivateField`, `JS_SetPrivateField`,
                // `JS_DefinePrivateField`, `JS_CheckBrand`; `private_in`
                // converts it with `JS_ValueToAtom`, whose fallback names
                // an unknown tag "[unsupported type]".
                let name_slot = if opcode == op::DEFINE_PRIVATE_FIELD {
                    2
                } else {
                    1
                };
                self.need(at, &out, pops)?;
                let index = out.depth - name_slot;
                if out.kind(index) == Some(&Kind::Taint) {
                    out.set(index, None);
                }
                self.pop(at, &mut out, pops)?;
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
            op::COPY_DATA_PROPERTIES => {
                let mask = instruction.first_immediate().unwrap_or(0) as u32;
                for offset in [mask & 3, (mask >> 2) & 7, (mask >> 5) & 7] {
                    self.need(at, &out, offset + 1)?;
                    let index = out.depth - 1 - offset;
                    if !Kind::ordinary(out.kind(index)) {
                        return Err(self.fail(
                            at,
                            Rule::SpecialSlot,
                            format!(
                                "copy_data_properties reads slot {index}, which holds {:?}",
                                out.kind(index)
                            ),
                        ));
                    }
                }
                self.pop(at, &mut out, pops)?;
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
            _ => {
                let local = instruction
                    .operands()
                    .iter()
                    .find_map(|operand| match operand {
                        Operand::Local(index) => Some(*index),
                        _ => None,
                    });
                match (opcode, local) {
                    (op::SET_LOC_UNINITIALIZED, Some(index)) => {
                        if let Err(position) = out.uninit.binary_search(&index) {
                            out.uninit.insert(position, index);
                        }
                    }
                    (op::INC_LOC | op::DEC_LOC | op::ADD_LOC, Some(index))
                        if state.uninit.binary_search(&index).is_ok() =>
                    {
                        return Err(self.fail(
                            at,
                            Rule::Uninitialized,
                            format!(
                                "{} of local {index}, which can be uninitialized here",
                                self.code.name(at)
                            ),
                        ));
                    }
                    (
                        op::PUT_LOC
                        | op::SET_LOC
                        | op::PUT_LOC8
                        | op::SET_LOC8
                        | op::PUT_LOC0
                        | op::PUT_LOC1
                        | op::PUT_LOC2
                        | op::PUT_LOC3
                        | op::SET_LOC0
                        | op::SET_LOC1
                        | op::SET_LOC2
                        | op::SET_LOC3
                        | op::PUT_LOC_CHECK
                        | op::SET_LOC_CHECK
                        | op::PUT_LOC_CHECK_INIT,
                        Some(index),
                    ) => {
                        if let Ok(position) = out.uninit.binary_search(&index) {
                            out.uninit.remove(position);
                        }
                    }
                    _ => {}
                }
                if matches!(opcode, op::EVAL | op::APPLY_EVAL) {
                    // The eval compiler reads a captured local without a
                    // check unless its vardef says it is lexical.
                    for &index in &state.uninit {
                        if !function
                            .local_def(index)
                            .is_some_and(|vardef| vardef.is_lexical)
                        {
                            return Err(self.fail(
                                at,
                                Rule::Uninitialized,
                                format!("direct eval while non-lexical local {index} can be uninitialized"),
                            ));
                        }
                    }
                }
                if link {
                    let creates_refs = match opcode {
                        op::FCLOSURE | op::FCLOSURE8 => match instruction.operands().first() {
                            Some(Operand::Const(index)) => self
                                .code
                                .cpool_function(*index)
                                .is_some_and(Code::captures_frame),
                            _ => false,
                        },
                        op::MAKE_LOC_REF | op::MAKE_ARG_REF | op::EVAL | op::APPLY_EVAL => true,
                        _ => false,
                    };
                    if creates_refs {
                        return Err(self.fail(
                            at,
                            Rule::FrameKind,
                            format!(
                                "{} creates a reference to the frame while a module links",
                                self.code.name(at)
                            ),
                        ));
                    }
                }
                self.pop(at, &mut out, pops)?;
                out.depth += pushes;
                edges.push((next, out, EdgeKind::Normal));
            }
        }
        Ok(edges)
    }
}
