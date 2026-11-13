//! `PrimJS` global scripts to modules of the pinned `QuickJS` fork.
//!
//! [`convert_script`] turns a [`Program`] the reader returned into the bytes
//! of a fork module that [`quickjs_rust_bridge::Context::complete_module_bytecode`]
//! accepts and that runs as the script would. The design is
//! `docs/primjs-bytecode-conversion-design.md` (§4, §6); this module
//! documentation states what the code does.
//!
//! # The module
//!
//! A native Lynx section is a global script; in this engine a card body is
//! an ES module whose free names include the realm's imports. The output is
//! a module whose function is a fixed sequence, with the converted script
//! function as its one child:
//!
//! ```text
//! push_this; if_false L; return_undef        linking: nothing to initialize
//! L: get_var globalThis                      the script's `this`
//!    fclosure <script function>
//!    call_method 0                           the completion value
//!    put_var_ref *default* | drop
//!    undefined; return_async
//! ```
//!
//! With [`Completion::ExportScript`] the evaluated part is
//! `fclosure <script function>; put_var_ref *default*; undefined;
//! return_async`: the script runs when the default export is called.
//!
//! The module function owns the top-level closure variables; the script
//! function and everything below reach them through closure references:
//!
//! | Free name | Module function's closure variable | From below |
//! | --- | --- | --- |
//! | declared by the script (`define_var`, `check_define_var`, `define_func`) | `GLOBAL_DECL`, lexical/const from the instruction's flags, `GLOBAL_FUNCTION_DECL` for `define_func` | `GLOBAL_REF`; `get_var`, `put_var`, `put_var_init`, `put_var_ref` of a declared function |
//! | in [`ConvertOptions::imports`] | `MODULE_IMPORT`, with an import entry and a request per specifier | `REF`; reads are `get_var_ref_check`, a write is `throw_error VAR_RO` |
//! | in [`ConvertOptions::undefined_names`] | a non-lexical `MODULE_DECL`, which holds `undefined` from the start | as an import |
//! | anything else | `GLOBAL` | `GLOBAL_REF`; the `get_var` family |
//!
//! A name the script declares wins over both lists. Global declarations
//! are instantiated, and redeclarations refused, when the module links.
//!
//! # Instructions
//!
//! [`OpcodeClass`] says, for each `PrimJS` opcode, whether it is copied by
//! name ([`classify::SAME`]), rewritten ([`classify::TRANSLATED`], with the
//! sequence each becomes) or refused ([`classify::REJECTED`]). Jumps are
//! written in their 32-bit forms and re-addressed through the index of each
//! input instruction's first output instruction.
//!
//! # Records
//!
//! Closure variables keep `PrimJS`'s entries in order (`is_local` and
//! `is_arg` become `LOCAL`, `ARG` or `REF`) and append one per name the
//! function reaches. Captured arguments and locals are numbered from the
//! references to them; vardefs stripped from the input are rebuilt with
//! `JS_ATOM_NULL` names. `js_mode` keeps only strict. An inline line table
//! is re-encoded with remapped pcs; external debug info becomes the module
//! name with no line table.
//!
//! # Checks
//!
//! The output is built through [`quickjs_rust_bridge::bytecode`]'s model and
//! passed through its [`verify`](quickjs_rust_bridge::bytecode::verify)
//! before it is written, so a program the fork's interpreter must not run
//! is a [`ConvertError::Verify`] naming the function and the instruction.
//! Conversion is linear in the input apart from the closure variables the
//! names add (bounded by nesting depth times distinct names), recursion is
//! bounded by the reader's nesting limit, and no input makes it panic.

mod atoms;
mod classify;
mod debug;
mod function;

use std::collections::HashMap;
use std::fmt;

use atoms::Atoms;
pub use classify::{OpcodeClass, REJECTED, SAME, TRANSLATED, opcode_class};
use function::{NameSlots, ParentNames, Role};
use quickjs_rust_bridge::bytecode::{
    self as fork, ClosureType, Export, Import, Instruction, Module, ModuleRequest, Operand as Fo,
    RegExpCompiler,
};
use quickjs_rust_bridge::bytecode_abi::{js_mode, limits, op, var_kind};

use super::model::{Atom, Function, Program};
use super::tables::Opcode;

/// What happens to the script's completion value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// It becomes the module's default export.
    DefaultExport,
    /// It is dropped.
    Discard,
    /// The script is not run when the module evaluates: the module's
    /// default export is the converted script function itself, which a
    /// caller runs as often as it likes, with the `this` it chooses (a
    /// named Lepus chunk, which `__LoadLepusChunk` runs on every call with
    /// `this` = `globalThis`). Each call returns the completion value.
    ExportScript,
}

/// How a script becomes a module.
#[derive(Debug, Clone, Copy)]
pub struct ConvertOptions<'a> {
    /// The name the module is registered under; written into the module
    /// record and used as the file name of its debug information.
    pub module_name: &'a str,
    /// Names the module imports: per module specifier, the imported names
    /// (each imported under its own name).
    pub imports: &'a [(&'a str, &'a [&'a str])],
    /// Names bound to `undefined` in the module.
    pub undefined_names: &'a [&'a str],
    /// What happens to the completion value.
    pub completion: Completion,
}

impl<'a> ConvertOptions<'a> {
    /// A module named `module_name` with no imports, no names bound to
    /// `undefined`, and the completion value dropped.
    #[must_use]
    pub const fn script(module_name: &'a str) -> Self {
        Self {
            module_name,
            imports: &[],
            undefined_names: &[],
            completion: Completion::Discard,
        }
    }
}

/// Why a program was not converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    /// An opcode, or an operand form of one, the converter refuses.
    Unsupported {
        /// The function: its path from the script (`script`, then
        /// `.cpool[i]` per level) and its name.
        function: String,
        /// The instruction's byte offset in the `PrimJS` function.
        pc: u32,
        /// The opcode.
        opcode: &'static str,
        /// Why it is refused.
        reason: &'static str,
    },
    /// Code or records the converter has no translation for.
    Shape {
        /// The function, as for [`Self::Unsupported`].
        function: String,
        /// The instruction's byte offset, when the problem is one
        /// instruction.
        pc: Option<u32>,
        /// What was found.
        detail: String,
    },
    /// A regular expression literal that cannot be recompiled.
    RegExp {
        /// The function.
        function: String,
        /// The `regexp` instruction's byte offset.
        pc: u32,
        /// What was wrong.
        detail: String,
    },
    /// A function that would need more than `JS_MAX_LOCAL_VARS` closure
    /// variables.
    TooManyClosureVariables {
        /// The function.
        function: String,
        /// The count it would need.
        count: usize,
    },
    /// The options are inconsistent.
    Options(String),
    /// The output breaks a rule of the fork's verifier: the function
    /// (with the output's path, `module` for the module function,
    /// `module.cpool[0]` for the script), the output instruction index
    /// and the rule.
    Verify(fork::VerifyError),
    /// The output cannot be written.
    Write(String),
}

impl ConvertError {
    fn shape(function: &str, pc: Option<u32>, detail: &str) -> Self {
        Self::Shape {
            function: function.to_owned(),
            pc,
            detail: detail.to_owned(),
        }
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported {
                function,
                pc,
                opcode,
                reason,
            } => write!(
                formatter,
                "{function} at pc {pc}: {opcode} is not supported: {reason}"
            ),
            Self::Shape {
                function,
                pc: Some(pc),
                detail,
            } => write!(formatter, "{function} at pc {pc}: {detail}"),
            Self::Shape {
                function,
                pc: None,
                detail,
            } => write!(formatter, "{function}: {detail}"),
            Self::RegExp {
                function,
                pc,
                detail,
            } => write!(
                formatter,
                "{function} at pc {pc}: regular expression literal: {detail}"
            ),
            Self::TooManyClosureVariables { function, count } => write!(
                formatter,
                "{function}: needs {count} closure variables, more than {}",
                limits::JS_MAX_LOCAL_VARS
            ),
            Self::Options(detail) => write!(formatter, "conversion options: {detail}"),
            Self::Verify(error) => write!(formatter, "converted bytecode refused: {error}"),
            Self::Write(detail) => {
                write!(formatter, "converted bytecode cannot be written: {detail}")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

/// What a free name of the script is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameKind {
    /// A property of the global object.
    Global,
    /// A global the script declares.
    Declared {
        /// `let`, `const` or `class`.
        lexical: bool,
        /// A function declaration (`define_func`).
        function: bool,
    },
    /// An import: the index of its entry in [`ConvertOptions::imports`].
    Import(u32),
    /// A module binding that holds `undefined`.
    Undefined,
}

impl NameKind {
    /// Whether the binding is a global one (reached by `GLOBAL_REF` and
    /// the `get_var` family).
    const fn is_global(self) -> bool {
        matches!(self, Self::Global | Self::Declared { .. })
    }
}

/// One free name of the script.
struct Name {
    atom: fork::Atom,
    kind: NameKind,
    is_const: bool,
    is_lexical: bool,
    var_kind: u8,
}

/// State of one conversion.
struct Converter<'a, 'p> {
    atoms: Atoms<'p>,
    regexps: &'a dyn RegExpCompiler,
    names: Vec<Name>,
    name_ids: HashMap<fork::Atom, u32>,
    /// The names each function reaches (its own and its descendants'),
    /// sorted, in pre-order of the function tree.
    needs: Vec<Vec<u32>>,
    module_name: fork::Atom,
    done: fork::Atom,
    value: fork::Atom,
    return_atom: fork::Atom,
    /// The name given to a closure variable whose name was stripped and
    /// cannot be recovered from its parent: the text the fork's
    /// `JS_ThrowReferenceErrorUninitialized` prints for an unnamed
    /// variable.
    unnamed: fork::Atom,
}

/// Converts a `PrimJS` global script into the bytes of a fork module (see
/// the module documentation). `regexps` compiles the regular expression
/// literals; pass the realm the module will run in.
#[allow(
    clippy::too_many_lines,
    reason = "the module record and function, in the order the format lists them"
)]
pub fn convert_script(
    program: &Program,
    options: &ConvertOptions<'_>,
    regexps: &dyn RegExpCompiler,
) -> Result<Vec<u8>, ConvertError> {
    let mut atoms = Atoms::new(program);
    let module_name = atoms.intern_str(options.module_name);
    if module_name == fork::Atom::NULL || options.module_name.is_empty() {
        return Err(ConvertError::Options("an empty module name".to_owned()));
    }
    let done = atoms.intern_str("done");
    let value = atoms.intern_str("value");
    let return_atom = atoms.intern_str("return");
    let unnamed = atoms.intern_str("lexical variable");
    let mut converter = Converter {
        atoms,
        regexps,
        names: Vec::new(),
        name_ids: HashMap::new(),
        needs: Vec::new(),
        module_name,
        done,
        value,
        return_atom,
        unnamed,
    };

    // The names the options bind.
    let mut bound: HashMap<fork::Atom, NameKind> = HashMap::new();
    let mut entries = Vec::new();
    for (request, (_, names)) in options.imports.iter().enumerate() {
        for name in *names {
            let atom = converter.atoms.intern_str(name);
            let index = u32::try_from(entries.len()).unwrap_or(u32::MAX);
            if bound.insert(atom, NameKind::Import(index)).is_some() {
                return Err(ConvertError::Options(format!("`{name}` is bound twice")));
            }
            entries.push((atom, u32::try_from(request).unwrap_or(u32::MAX)));
        }
    }
    for name in options.undefined_names {
        let atom = converter.atoms.intern_str(name);
        if bound.insert(atom, NameKind::Undefined).is_some() {
            return Err(ConvertError::Options(format!("`{name}` is bound twice")));
        }
    }

    // Declarations of the script, then every name each function reaches.
    converter.declarations(&program.root)?;
    converter.collect(&program.root, &bound, 0)?;

    // The module function's closure variables: every declared name and
    // every name the script reaches, then `globalThis` when no entry of
    // the script's provides it, then the default export.
    let script_needs = converter.needs.first().cloned().unwrap_or_default();
    let mut module_ids: Vec<u32> = script_needs.clone();
    for (id, name) in converter.names.iter().enumerate() {
        if matches!(name.kind, NameKind::Declared { .. }) {
            module_ids.push(u32::try_from(id).unwrap_or(u32::MAX));
        }
    }
    module_ids.sort_unstable();
    module_ids.dedup();
    let mut closure_vars = Vec::with_capacity(module_ids.len() + 2);
    let mut imports = Vec::new();
    for &id in &module_ids {
        let name = &converter.names[id as usize];
        let var_idx = u16::try_from(closure_vars.len()).unwrap_or(u16::MAX);
        let closure_type = match name.kind {
            NameKind::Global => ClosureType::Global,
            NameKind::Declared { .. } => ClosureType::GlobalDecl,
            NameKind::Import(entry) => {
                let (import_name, request) = entries[entry as usize];
                imports.push(Import {
                    var_idx: u32::from(var_idx),
                    is_star: false,
                    name: import_name,
                    request,
                });
                ClosureType::ModuleImport
            }
            NameKind::Undefined => ClosureType::ModuleDecl,
        };
        closure_vars.push(fork::ClosureVar {
            name: name.atom,
            var_idx: 0,
            closure_type,
            is_const: name.is_const,
            is_lexical: name.is_lexical,
            var_kind: name.var_kind,
        });
    }
    let global_this = converter.atoms.intern_str("globalThis");
    let this_index = match converter.name_ids.get(&global_this) {
        Some(id)
            if converter.names[*id as usize].kind == NameKind::Global
                && module_ids.binary_search(id).is_ok() =>
        {
            u16::try_from(module_ids.binary_search(id).unwrap_or(0)).unwrap_or(u16::MAX)
        }
        _ => {
            closure_vars.push(fork::ClosureVar::new(global_this, ClosureType::Global, 0));
            u16::try_from(closure_vars.len() - 1).unwrap_or(u16::MAX)
        }
    };
    let mut exports = Vec::new();
    let default_index = if options.completion == Completion::Discard {
        None
    } else {
        let default_atom = converter.atoms.intern_str("*default*");
        let mut closure_var = fork::ClosureVar::new(default_atom, ClosureType::ModuleDecl, 0);
        closure_var.is_lexical = true;
        closure_vars.push(closure_var);
        let index = u16::try_from(closure_vars.len() - 1).unwrap_or(u16::MAX);
        exports.push(Export::Local {
            var_idx: u32::from(index),
            name: converter.atoms.intern_str("default"),
        });
        Some(index)
    };
    if closure_vars.len() > limits::JS_MAX_LOCAL_VARS as usize {
        return Err(ConvertError::TooManyClosureVariables {
            function: "module".to_owned(),
            count: closure_vars.len(),
        });
    }

    // The script function, a child of the module function.
    let module_slots = NameSlots {
        ids: &module_ids,
        base: 0,
    };
    let mut id = 0usize;
    let script = converter.function(
        &program.root,
        "script",
        Role::Script,
        &module_slots,
        &ParentNames::default(),
        &mut id,
    )?;

    let mut function = fork::Function::new(fork::FunctionKind::Async);
    function.js_mode = js_mode::STRICT;
    function.name = converter.atoms.intern_str("<eval>");
    function.closure_vars = closure_vars;
    function.cpool = vec![fork::Value::Function(Box::new(script))];
    function.debug = Some(fork::DebugInfo {
        filename: module_name,
        pc2line: Vec::new(),
        source: Vec::new(),
    });
    let head = [
        Instruction::simple(op::PUSH_THIS),
        Instruction::new(op::IF_FALSE, &[Fo::Target(3)]),
        Instruction::simple(op::RETURN_UNDEF),
    ];
    let body: Vec<_> = match (options.completion, default_index) {
        (Completion::ExportScript, Some(index)) => vec![
            Instruction::new(op::FCLOSURE, &[Fo::Const(0)]),
            Instruction::new(op::PUT_VAR_REF, &[Fo::ClosureVar(index)]),
        ],
        (_, default_index) => vec![
            Instruction::new(op::GET_VAR, &[Fo::ClosureVar(this_index)]),
            Instruction::new(op::FCLOSURE, &[Fo::Const(0)]),
            Instruction::new(op::CALL_METHOD, &[Fo::Immediate(0)]),
            match default_index {
                Some(index) => Instruction::new(op::PUT_VAR_REF, &[Fo::ClosureVar(index)]),
                None => Instruction::simple(op::DROP),
            },
        ],
    };
    let tail = [
        Instruction::simple(op::UNDEFINED),
        Instruction::simple(op::RETURN_ASYNC),
    ];
    let code: Vec<_> = head.into_iter().chain(body).chain(tail).collect();
    function.instructions = code
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|error| ConvertError::shape("module", None, &error.message))?;

    let requests = options
        .imports
        .iter()
        .map(|(specifier, _)| ModuleRequest {
            specifier: converter.atoms.intern_str(specifier),
            attributes: fork::Value::Undefined,
        })
        .collect();
    let module = Module {
        name: module_name,
        requests,
        exports,
        star_exports: Vec::new(),
        imports,
        has_tla: false,
        function,
    };
    let program = fork::Program {
        atoms: std::mem::take(&mut converter.atoms.table),
        root: fork::Value::Module(Box::new(module)),
    };
    fork::verify(&program, regexps).map_err(ConvertError::Verify)?;
    fork::write_program(&program).map_err(|error| ConvertError::Write(error.message))
}

impl Converter<'_, '_> {
    fn atom(&mut self, atom: Atom, path: &str) -> Result<fork::Atom, ConvertError> {
        self.atoms
            .map(atom)
            .ok_or_else(|| ConvertError::shape(path, None, &format!("atom {atom:?} out of range")))
    }

    /// Registers a name with `kind`, or returns its id.
    fn register(&mut self, atom: fork::Atom, kind: NameKind) -> u32 {
        if let Some(&id) = self.name_ids.get(&atom) {
            return id;
        }
        let id = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push(Name {
            atom,
            kind,
            is_const: matches!(kind, NameKind::Import(_) | NameKind::Undefined),
            is_lexical: matches!(kind, NameKind::Import(_)),
            var_kind: var_kind::NORMAL,
        });
        self.name_ids.insert(atom, id);
        id
    }

    /// The global declarations of the script's prologue: `check_define_var`
    /// and `define_var` carry `DEFINE_GLOBAL_LEX_VAR` (bit 7) and
    /// `DEFINE_GLOBAL_FUNC_VAR` (bit 6); `define_var` of a lexical name
    /// carries `JS_PROP_WRITABLE` (bit 1) unless it is `const`;
    /// `define_func` declares a function.
    fn declarations(&mut self, root: &Function) -> Result<(), ConvertError> {
        for instruction in &root.instructions {
            let (lexical, function, writable) = match instruction.opcode {
                Opcode::CheckDefineVar | Opcode::DefineVar => {
                    let flags = instruction
                        .operands()
                        .iter()
                        .rev()
                        .find_map(|operand| match operand {
                            super::Operand::Immediate(value) => Some(*value),
                            _ => None,
                        })
                        .unwrap_or(0);
                    (flags & 0x80 != 0, flags & 0x40 != 0, flags & 0x02 != 0)
                }
                Opcode::DefineFunc => (false, true, true),
                _ => continue,
            };
            let atom = self.atom(instruction.atom().unwrap_or(Atom::NULL), "script")?;
            if atom == fork::Atom::NULL {
                return Err(ConvertError::shape(
                    "script",
                    Some(instruction.pc),
                    "a global declaration of the null atom",
                ));
            }
            let id = self.register(atom, NameKind::Declared { lexical, function });
            let name = &mut self.names[id as usize];
            let NameKind::Declared {
                lexical: was_lexical,
                function: was_function,
            } = name.kind
            else {
                continue;
            };
            let lexical = lexical || was_lexical;
            let function = (function || was_function) && !lexical;
            name.kind = NameKind::Declared { lexical, function };
            name.is_lexical = lexical;
            name.var_kind = if function {
                var_kind::GLOBAL_FUNCTION_DECL
            } else {
                var_kind::NORMAL
            };
            if instruction.opcode == Opcode::DefineVar {
                name.is_const = lexical && !writable;
            }
        }
        Ok(())
    }

    /// Records, in pre-order, the names each function reaches, and
    /// returns them.
    fn collect(
        &mut self,
        function: &Function,
        bound: &HashMap<fork::Atom, NameKind>,
        depth: usize,
    ) -> Result<Vec<u32>, ConvertError> {
        if depth > super::NESTING_LIMIT {
            return Err(ConvertError::shape(
                "script",
                None,
                "functions nested too deeply",
            ));
        }
        let slot = self.needs.len();
        self.needs.push(Vec::new());
        let mut own = Vec::new();
        for instruction in &function.instructions {
            if !matches!(
                instruction.opcode,
                Opcode::GetVar
                    | Opcode::GetVarUndef
                    | Opcode::PutVar
                    | Opcode::PutVarInit
                    | Opcode::PutVarStrict
                    | Opcode::DefineFunc
            ) {
                continue;
            }
            let atom = self.atom(instruction.atom().unwrap_or(Atom::NULL), "script")?;
            if atom == fork::Atom::NULL {
                return Err(ConvertError::shape(
                    "script",
                    Some(instruction.pc),
                    "a variable access of the null atom",
                ));
            }
            let kind = bound.get(&atom).copied().unwrap_or(NameKind::Global);
            own.push(self.register(atom, kind));
        }
        let mut all = own;
        for value in &function.cpool {
            if let super::Value::Function(child) = value {
                all.extend(self.collect(child, bound, depth + 1)?);
            }
        }
        all.sort_unstable();
        all.dedup();
        self.needs[slot].clone_from(&all);
        Ok(all)
    }
}
