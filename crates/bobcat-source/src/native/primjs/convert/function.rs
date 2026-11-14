//! One `PrimJS` function to one fork function: records, closure variables,
//! instructions, line table and constant pool.

use quickjs_rust_bridge::bytecode::{
    self as fork, ClosureType, Instruction, Operand as Fo, VarDef as ForkVarDef,
};
use quickjs_rust_bridge::bytecode_abi::{js_mode, limits, op, special_object, throw_error};

use super::super::model::{
    Atom, DebugInfo, Function, FunctionKind, Instruction as PInstruction, Operand, Value,
};
use super::super::tables::Opcode;
use super::classify::{OpcodeClass, opcode_class};
use super::{ConvertError, Converter, NameKind, debug};

/// Where a jump goes: an input instruction, or a label inside a sequence
/// the converter writes.
#[derive(Clone, Copy, Debug)]
enum Label {
    Input(u32),
    Local(usize),
}

/// The output instructions of one function under construction.
struct Emitter {
    code: Vec<Instruction>,
    /// Instructions whose target is still a [`Label`].
    fixups: Vec<(usize, Label)>,
    /// Output index of each local label, once bound.
    locals: Vec<Option<u32>>,
    /// The first instruction the model refused to build, if any.
    broken: Option<String>,
}

impl Emitter {
    fn new(capacity: usize) -> Self {
        Self {
            code: Vec::with_capacity(capacity),
            fixups: Vec::new(),
            locals: Vec::new(),
            broken: None,
        }
    }

    fn label(&mut self) -> Label {
        self.locals.push(None);
        Label::Local(self.locals.len() - 1)
    }

    fn push(&mut self, instruction: Result<Instruction, fork::ModelError>) {
        match instruction {
            Ok(instruction) => self.code.push(instruction),
            Err(error) => {
                self.broken.get_or_insert(error.message);
            }
        }
    }

    fn bind(&mut self, label: Label) {
        if let Label::Local(index) = label {
            self.locals[index] = u32::try_from(self.code.len()).ok();
        }
    }
}

/// The names a function reaches through its closure variables, beyond the
/// ones `PrimJS` gave it: name ids in increasing order, at closure indices
/// `base..base + ids.len()`.
pub(super) struct NameSlots<'n> {
    pub ids: &'n [u32],
    pub base: u16,
}

impl NameSlots<'_> {
    fn index(&self, id: u32) -> Option<u16> {
        let position = self.ids.binary_search(&id).ok()?;
        self.base.checked_add(u16::try_from(position).ok()?)
    }
}

/// The names of a parent's variables, for closure variables whose own
/// name was stripped.
#[derive(Default)]
pub(super) struct ParentNames {
    pub arg_count: usize,
    pub vardefs: Vec<fork::Atom>,
    pub closures: Vec<fork::Atom>,
}

/// What a function is in the output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// The script's top-level function, a child of the module function.
    Script,
    /// A function of some constant pool.
    Nested,
}

impl Converter<'_, '_> {
    /// Converts `function`, whose closure variables for names are `slots`
    /// and whose parent reaches names through `parent`.
    #[allow(
        clippy::too_many_lines,
        reason = "the record fields in the order the format lists them"
    )]
    pub(super) fn function(
        &mut self,
        function: &Function,
        path: &str,
        role: Role,
        parent: &NameSlots<'_>,
        parent_names: &ParentNames,
        id: &mut usize,
    ) -> Result<fork::Function, ConvertError> {
        let my_id = *id;
        *id += 1;
        let needs = std::mem::take(&mut self.needs[my_id]);
        let path = match self.atoms.map(function.name) {
            Some(name) if name != fork::Atom::NULL => {
                format!("{path} ({})", self.atoms.text(name))
            }
            _ => path.to_owned(),
        };

        if role == Role::Script && !function.closure_vars.is_empty() {
            return Err(ConvertError::shape(
                &path,
                None,
                "a global script's top-level function has closure variables",
            ));
        }

        // Closure variables: PrimJS's own, then one per name reached.
        let mut closure_vars = Vec::with_capacity(function.closure_vars.len() + needs.len());
        for closure_var in &function.closure_vars {
            let closure_type = match (closure_var.is_local, closure_var.is_arg) {
                (true, true) => ClosureType::Arg,
                (true, false) => ClosureType::Local,
                (false, _) => ClosureType::Ref,
            };
            let var_idx = u16::try_from(closure_var.var_idx).map_err(|_| {
                ConvertError::shape(&path, None, "a closure variable index above 65535")
            })?;
            // A stripped name is taken from the variable it names in the
            // parent; failing that, the placeholder the fork's own error
            // message uses for an unnamed variable.
            let mut name = self.atom(closure_var.name, &path)?;
            if name == fork::Atom::NULL {
                name = match closure_type {
                    ClosureType::Arg => parent_names.vardefs.get(usize::from(var_idx)),
                    ClosureType::Local => parent_names
                        .vardefs
                        .get(parent_names.arg_count + usize::from(var_idx)),
                    _ => parent_names.closures.get(usize::from(var_idx)),
                }
                .copied()
                .filter(|atom| *atom != fork::Atom::NULL)
                .unwrap_or(self.unnamed);
            }
            closure_vars.push(fork::ClosureVar {
                name,
                var_idx,
                closure_type,
                is_const: closure_var.is_const,
                is_lexical: closure_var.is_lexical,
                var_kind: closure_var.var_kind,
            });
        }
        let base = u16::try_from(closure_vars.len()).unwrap_or(u16::MAX);
        for &name_id in &needs {
            let name = &self.names[name_id as usize];
            let var_idx = parent.index(name_id).ok_or_else(|| {
                ConvertError::shape(&path, None, "a name its parent does not reach")
            })?;
            let closure_type = if name.kind.is_global() {
                ClosureType::GlobalRef
            } else {
                ClosureType::Ref
            };
            closure_vars.push(fork::ClosureVar {
                name: name.atom,
                var_idx,
                closure_type,
                is_const: name.is_const,
                is_lexical: name.is_lexical,
                var_kind: name.var_kind,
            });
        }
        if closure_vars.len() > limits::JS_MAX_LOCAL_VARS as usize {
            return Err(ConvertError::TooManyClosureVariables {
                function: path,
                count: closure_vars.len(),
            });
        }
        let slots = NameSlots { ids: &needs, base };

        // Records.
        let mut out = fork::Function::new(match function.flags.kind {
            FunctionKind::Normal => fork::FunctionKind::Normal,
            FunctionKind::Generator => fork::FunctionKind::Generator,
            FunctionKind::Async => fork::FunctionKind::Async,
            FunctionKind::AsyncGenerator => fork::FunctionKind::AsyncGenerator,
        });
        let flags = &function.flags;
        out.flags.has_prototype = flags.has_prototype;
        out.flags.has_simple_parameter_list = flags.has_simple_parameter_list;
        out.flags.is_derived_class_constructor = flags.is_derived_class_constructor;
        out.flags.need_home_object = flags.need_home_object;
        out.flags.new_target_allowed = flags.new_target_allowed;
        out.flags.super_call_allowed = flags.super_call_allowed;
        out.flags.super_allowed = flags.super_allowed;
        out.flags.arguments_allowed = flags.arguments_allowed;
        out.js_mode = if function.js_mode.is_strict() {
            js_mode::STRICT
        } else {
            0
        };
        out.name = self.atom(function.name, &path)?;
        out.arg_count = function.arg_count;
        out.var_count = function.var_count;
        out.defined_arg_count = function.defined_arg_count;
        out.closure_vars = closure_vars;
        self.vardefs(function, &path, &mut out)?;

        // Instructions.
        let mut appended = Vec::new();
        let (code, starts) = self.instructions(function, &path, role, &slots, &mut appended)?;
        out.instructions = code;
        out.debug = self.debug_info(function, &out.instructions, &starts);

        // Constant pool: PrimJS's entries in order, then the regular
        // expression bytecode this conversion compiled.
        let my_names = ParentNames {
            arg_count: usize::from(out.arg_count),
            vardefs: out.vardefs.iter().map(|vardef| vardef.name).collect(),
            closures: out
                .closure_vars
                .iter()
                .map(|closure_var| closure_var.name)
                .collect(),
        };
        let mut cpool = Vec::with_capacity(function.cpool.len() + appended.len());
        for (index, value) in function.cpool.iter().enumerate() {
            cpool.push(match value {
                Value::Function(child) => {
                    let child_path = format!("{path}.cpool[{index}]");
                    fork::Value::Function(Box::new(self.function(
                        child,
                        &child_path,
                        Role::Nested,
                        &slots,
                        &my_names,
                        id,
                    )?))
                }
                other => self.value(other, &path)?,
            });
        }
        cpool.extend(appended);
        out.cpool = cpool;
        Ok(out)
    }

    /// Vardefs with the capture numbering the fork reads
    /// (`var_ref_idx`): an argument or local is captured when a child's
    /// closure variable names it, when `make_loc_ref`, `make_arg_ref` or
    /// `close_loc` names it, and every argument is when the function
    /// builds a mapped `arguments` object.
    fn vardefs(
        &mut self,
        function: &Function,
        path: &str,
        out: &mut fork::Function,
    ) -> Result<(), ConvertError> {
        let args = usize::from(function.arg_count);
        let count = args + usize::from(function.var_count);
        let mut captured = vec![false; count];
        for value in &function.cpool {
            if let Value::Function(child) = value {
                for closure_var in &child.closure_vars {
                    if !closure_var.is_local {
                        continue;
                    }
                    let index =
                        closure_var.var_idx as usize + if closure_var.is_arg { 0 } else { args };
                    if let Some(slot) = captured.get_mut(index) {
                        *slot = true;
                    }
                }
            }
        }
        for instruction in &function.instructions {
            for operand in instruction.operands() {
                match (instruction.opcode, operand) {
                    (Opcode::MakeLocRef | Opcode::CloseLoc, Operand::Local(index)) => {
                        captured[args + usize::from(*index)] = true;
                    }
                    (Opcode::MakeArgRef, Operand::Arg(index)) => {
                        captured[usize::from(*index)] = true;
                    }
                    (Opcode::SpecialObject, Operand::Immediate(kind))
                        if *kind == i32::from(special_object::MAPPED_ARGUMENTS) =>
                    {
                        captured[..args].fill(true);
                    }
                    _ => {}
                }
            }
        }
        let mut vardefs = Vec::with_capacity(count);
        let mut next_ref = 0u16;
        for (index, is_captured) in captured.into_iter().enumerate() {
            let mut vardef = match function.vardefs.get(index) {
                Some(source) => ForkVarDef {
                    name: self.atom(source.name, path)?,
                    scope_next: source.scope_next,
                    var_ref_idx: 0,
                    var_kind: source.var_kind,
                    is_const: source.is_const,
                    is_lexical: source.is_lexical,
                    is_captured: false,
                    has_scope: source.scope_level != 0,
                },
                None => ForkVarDef::new(fork::Atom::NULL),
            };
            if is_captured {
                vardef.is_captured = true;
                vardef.var_ref_idx = next_ref;
                next_ref = next_ref.checked_add(1).ok_or_else(|| {
                    ConvertError::shape(path, None, "more than 65535 captured variables")
                })?;
            }
            vardefs.push(vardef);
        }
        out.var_ref_count = next_ref;
        out.vardefs = vardefs;
        Ok(())
    }

    /// The debug block: the inline line table re-encoded with remapped
    /// pcs, or the filename alone when the line table is kept outside
    /// the bytecode (its function id has no place in the fork's record).
    fn debug_info(
        &mut self,
        function: &Function,
        code: &[Instruction],
        starts: &[u32],
    ) -> Option<fork::DebugInfo> {
        let debug = function.debug.as_ref()?;
        let mut pcs = Vec::with_capacity(code.len() + 1);
        let mut pc = 0u32;
        for instruction in code {
            pcs.push(pc);
            pc = pc.saturating_add(u32::from(instruction.size()));
        }
        pcs.push(pc);
        match debug {
            DebugInfo::Inline {
                filename,
                line_num,
                pc2line,
            } => {
                // An empty file name (what the encoder writes without a
                // debug URL) becomes the module's name.
                let filename = match self.atoms.map(*filename) {
                    Some(atom) if !self.atoms.text(atom).is_empty() => atom,
                    _ => self.module_name,
                };
                let pc2line = (|| {
                    let start = debug::unpack(i64::from(*line_num))?;
                    let entries = debug::decode_primjs(i64::from(*line_num), pc2line)?;
                    let mut changes = Vec::with_capacity(entries.len());
                    for (input_pc, word) in entries {
                        let position = debug::unpack(word)?;
                        let index = match function
                            .instructions
                            .binary_search_by_key(&input_pc, |instruction| instruction.pc)
                        {
                            Ok(index) => index,
                            Err(index) => index.saturating_sub(1),
                        };
                        let output = starts.get(index).copied().unwrap_or(u32::MAX);
                        let output_pc = pcs.get(output as usize).copied().unwrap_or(pc);
                        changes.push((output_pc, position));
                    }
                    debug::encode_fork(start, &changes)
                })()
                .unwrap_or_default();
                Some(fork::DebugInfo {
                    filename,
                    pc2line,
                    source: Vec::new(),
                })
            }
            DebugInfo::External { .. } => Some(fork::DebugInfo {
                filename: self.module_name,
                pc2line: Vec::new(),
                source: Vec::new(),
            }),
        }
    }

    /// Translates every instruction. Returns the code and, per input
    /// instruction, the index of its first output instruction.
    #[allow(clippy::too_many_lines, reason = "one arm per translated opcode")]
    fn instructions(
        &mut self,
        function: &Function,
        path: &str,
        role: Role,
        slots: &NameSlots<'_>,
        appended: &mut Vec<fork::Value>,
    ) -> Result<(Vec<Instruction>, Vec<u32>), ConvertError> {
        let input = &function.instructions;
        let mut is_target = vec![false; input.len()];
        for instruction in input {
            if let Some(target) = instruction.target()
                && let Some(slot) = is_target.get_mut(target as usize)
            {
                *slot = true;
            }
        }
        let mut emit = Emitter::new(input.len() + input.len() / 4);
        let mut starts = vec![0u32; input.len()];
        let mut at = 0usize;
        while at < input.len() {
            let instruction = &input[at];
            let start = u32::try_from(emit.code.len()).unwrap_or(u32::MAX);
            starts[at] = start;
            let error = |detail: &str| ConvertError::shape(path, Some(instruction.pc), detail);
            let mut consumed = 1usize;
            match opcode_class(instruction.opcode) {
                OpcodeClass::Rejected(reason) => {
                    return Err(ConvertError::Unsupported {
                        function: path.to_owned(),
                        pc: instruction.pc,
                        opcode: instruction.opcode.name(),
                        reason,
                    });
                }
                OpcodeClass::Same(opcode) => {
                    if instruction.opcode == Opcode::SpecialObject
                        && instruction.immediate() == Some(i32::from(special_object::VAR_OBJECT))
                    {
                        return Err(ConvertError::Unsupported {
                            function: path.to_owned(),
                            pc: instruction.pc,
                            opcode: "special_object 5",
                            reason: "the variable object exists only for a direct eval",
                        });
                    }
                    // `apply` magic 0 calls and 1 constructs on both
                    // sides; the fork reads 2 as `Reflect.apply`, which
                    // PrimJS's compiler never emits.
                    if instruction.opcode == Opcode::Apply
                        && !matches!(instruction.immediate(), Some(0 | 1))
                    {
                        return Err(ConvertError::Unsupported {
                            function: path.to_owned(),
                            pc: instruction.pc,
                            opcode: "apply",
                            reason: "a magic other than 0 (call) and 1 (construct)",
                        });
                    }
                    self.same(&mut emit, opcode, instruction, path)?;
                }
                OpcodeClass::Translated(_) => match instruction.opcode {
                    Opcode::IfFalse8 => jump(&mut emit, op::IF_FALSE, target_of(instruction)),
                    Opcode::IfTrue8 => jump(&mut emit, op::IF_TRUE, target_of(instruction)),
                    Opcode::Goto8 | Opcode::Goto16 => {
                        jump(&mut emit, op::GOTO, target_of(instruction));
                    }
                    Opcode::GetVar
                    | Opcode::GetVarUndef
                    | Opcode::PutVar
                    | Opcode::PutVarInit
                    | Opcode::PutVarStrict
                    | Opcode::DefineFunc => {
                        let atom = self.atom(instruction.atom().unwrap_or(Atom::NULL), path)?;
                        let id = *self
                            .name_ids
                            .get(&atom)
                            .ok_or_else(|| error("a name the converter did not collect"))?;
                        let kind = self.names[id as usize].kind;
                        let index = slots
                            .index(id)
                            .ok_or_else(|| error("a name the function does not reach"))?;
                        let var = Fo::ClosureVar(index);
                        let bound = kind.is_global();
                        match instruction.opcode {
                            Opcode::GetVar if bound => put(&mut emit, op::GET_VAR, &[var]),
                            Opcode::GetVarUndef if bound => {
                                put(&mut emit, op::GET_VAR_UNDEF, &[var]);
                            }
                            Opcode::GetVar | Opcode::GetVarUndef => {
                                put(&mut emit, op::GET_VAR_REF_CHECK, &[var]);
                            }
                            Opcode::PutVar if bound => put(&mut emit, op::PUT_VAR, &[var]),
                            Opcode::PutVarStrict if bound => {
                                simple(&mut emit, op::NIP);
                                put(&mut emit, op::PUT_VAR, &[var]);
                            }
                            Opcode::PutVar | Opcode::PutVarStrict => {
                                if instruction.opcode == Opcode::PutVarStrict {
                                    simple(&mut emit, op::NIP);
                                }
                                put(
                                    &mut emit,
                                    op::THROW_ERROR,
                                    &[
                                        Fo::Atom(atom),
                                        Fo::Immediate(i32::from(throw_error::VAR_RO)),
                                    ],
                                );
                            }
                            Opcode::PutVarInit => {
                                if !matches!(kind, NameKind::Declared { .. }) {
                                    return Err(error(
                                        "put_var_init of a name the script does not declare",
                                    ));
                                }
                                put(&mut emit, op::PUT_VAR_INIT, &[var]);
                            }
                            _ => {
                                // define_func: the closure was created by
                                // the instruction before; store it.
                                if role != Role::Script
                                    || !matches!(kind, NameKind::Declared { function: true, .. })
                                {
                                    return Err(error(
                                        "define_func outside the script's top-level function",
                                    ));
                                }
                                put(&mut emit, op::PUT_VAR_REF, &[var]);
                            }
                        }
                    }
                    Opcode::DefineVar | Opcode::CheckDefineVar => {
                        if role != Role::Script {
                            return Err(error(
                                "a global declaration outside the script's top-level function",
                            ));
                        }
                    }
                    Opcode::CheckVar => simple(&mut emit, op::UNDEFINED),
                    Opcode::ThrowVar => {
                        let atom = self.atom(instruction.atom().unwrap_or(Atom::NULL), path)?;
                        let kind = last_immediate(instruction);
                        if !(0..=i32::from(throw_error::ERROR_DELETE_SUPER)).contains(&kind) {
                            return Err(error("throw_var with an unknown error type"));
                        }
                        put(
                            &mut emit,
                            op::THROW_ERROR,
                            &[Fo::Atom(atom), Fo::Immediate(kind)],
                        );
                    }
                    Opcode::GetSuperCtor => simple(&mut emit, op::GET_SUPER),
                    Opcode::IsFunction => simple(&mut emit, op::TYPEOF_IS_FUNCTION),
                    Opcode::ToPropkey2 => {
                        // [object, key] -> [object, property key]; a null
                        // or undefined object raises the TypeError
                        // (get_length reads a property of it) before the
                        // key is converted.
                        let checked = emit.label();
                        simple(&mut emit, op::SWAP);
                        simple(&mut emit, op::DUP);
                        simple(&mut emit, op::IS_UNDEFINED_OR_NULL);
                        jump(&mut emit, op::IF_FALSE, checked);
                        simple(&mut emit, op::GET_LENGTH);
                        emit.bind(checked);
                        simple(&mut emit, op::SWAP);
                        simple(&mut emit, op::TO_PROPKEY);
                    }
                    Opcode::IteratorGetValueDone => {
                        // [result] -> [value, done]: `done` is read before
                        // `value`, as JS_IteratorGetCompleteValue does.
                        simple(&mut emit, op::ITERATOR_CHECK_OBJECT);
                        put(&mut emit, op::GET_FIELD2, &[Fo::Atom(self.done)]);
                        simple(&mut emit, op::SWAP);
                        put(&mut emit, op::GET_FIELD, &[Fo::Atom(self.value)]);
                        simple(&mut emit, op::SWAP);
                    }
                    Opcode::Dup3 => {
                        let pattern = [
                            Opcode::Drop,
                            Opcode::CallMethod,
                            Opcode::Await,
                            Opcode::IteratorGetValueDone,
                        ];
                        let for_await = input.get(at + 1..at + 5).is_some_and(|next| {
                            next.iter().map(|ins| ins.opcode).eq(pattern)
                                && next[1].immediate() == Some(0)
                        }) && !is_target[at + 1..at + 5].iter().any(|&t| t);
                        if for_await {
                            simple(&mut emit, op::FOR_AWAIT_OF_NEXT);
                            simple(&mut emit, op::AWAIT);
                            let get = u32::try_from(emit.code.len()).unwrap_or(u32::MAX);
                            simple(&mut emit, op::ITERATOR_GET_VALUE_DONE);
                            starts[at + 1] = start;
                            starts[at + 2] = start;
                            starts[at + 3] = start + 1;
                            starts[at + 4] = get;
                            consumed = 5;
                        } else {
                            simple(&mut emit, op::DUP3);
                        }
                    }
                    Opcode::IteratorCloseReturn => {
                        simple(&mut emit, op::NIP_CATCH);
                        simple(&mut emit, op::ROT3R);
                        simple(&mut emit, op::UNDEFINED);
                    }
                    Opcode::AsyncIteratorClose => {
                        // [iterator, next, catch offset] -> [result, false]
                        // after calling `return`, or [undefined, true]
                        // when there is no `return` method.
                        let absent = emit.label();
                        let done = emit.label();
                        simple(&mut emit, op::DROP);
                        simple(&mut emit, op::DROP);
                        put(&mut emit, op::GET_FIELD2, &[Fo::Atom(self.return_atom)]);
                        simple(&mut emit, op::DUP);
                        simple(&mut emit, op::IS_UNDEFINED_OR_NULL);
                        jump(&mut emit, op::IF_TRUE, absent);
                        put(&mut emit, op::CALL_METHOD, &[Fo::Immediate(0)]);
                        simple(&mut emit, op::PUSH_FALSE);
                        jump(&mut emit, op::GOTO, done);
                        emit.bind(absent);
                        simple(&mut emit, op::DROP);
                        simple(&mut emit, op::DROP);
                        simple(&mut emit, op::UNDEFINED);
                        simple(&mut emit, op::PUSH_TRUE);
                        emit.bind(done);
                    }
                    Opcode::AsyncIteratorNext => simple(&mut emit, op::ITERATOR_NEXT),
                    Opcode::AsyncIteratorGet => match last_immediate(instruction) {
                        flag @ (0 | 1) => {
                            put(&mut emit, op::ITERATOR_CALL, &[Fo::Immediate(flag)]);
                        }
                        2 => put(
                            &mut emit,
                            op::THROW_ERROR,
                            &[
                                Fo::Atom(fork::Atom::NULL),
                                Fo::Immediate(i32::from(throw_error::ERROR_ITERATOR_THROW)),
                            ],
                        ),
                        _ => return Err(error("async_iterator_get with an unknown flag")),
                    },
                    Opcode::YieldStar => self.yield_star(&mut emit),
                    Opcode::Regexp => {
                        self.regexp(function, at, &is_target, &mut emit, path, appended)?;
                    }
                    _ => {
                        return Err(error(
                            "an opcode classified as translated with no translation",
                        ));
                    }
                },
            }
            at += consumed;
        }
        resolve(&mut emit, &starts, path)?;
        Ok((emit.code, starts))
    }

    /// An instruction copied by name: atoms re-interned, targets
    /// re-addressed, every other operand as it is.
    fn same(
        &mut self,
        emit: &mut Emitter,
        opcode: u8,
        instruction: &PInstruction,
        path: &str,
    ) -> Result<(), ConvertError> {
        let mut operands = [Fo::Immediate(0); 3];
        let count = instruction.operands().len();
        let mut label = None;
        for (slot, operand) in operands.iter_mut().zip(instruction.operands()) {
            *slot = match *operand {
                Operand::Atom(atom) => Fo::Atom(self.atom(atom, path)?),
                Operand::Local(index) => Fo::Local(index),
                Operand::Arg(index) => Fo::Arg(index),
                Operand::ClosureVar(index) => Fo::ClosureVar(index),
                Operand::Const(index) => Fo::Const(index),
                Operand::Immediate(value) => Fo::Immediate(value),
                Operand::Target(index) => {
                    label = Some(Label::Input(index));
                    Fo::Target(0)
                }
            };
        }
        let instruction = Instruction::new(opcode, &operands[..count])
            .map_err(|error| ConvertError::shape(path, Some(instruction.pc), &error.message))?;
        if let Some(label) = label {
            emit.fixups.push((emit.code.len(), label));
        }
        emit.code.push(instruction);
        Ok(())
    }

    /// A synchronous `yield*`. `PrimJS` enters `yield_star` with
    /// `[iterator, next]` and its generator resumption delegates each
    /// `next`, `return` and `throw` to the iterator
    /// (`JS_GENERATOR_STATE_SUSPENDED_YIELD_STAR`), leaving
    /// `[value, is_return]`. This is the fork's own delegation loop
    /// (`js_parse_unary`, the `yield*` case) with its return replaced by
    /// `push_true` and its normal exit followed by `push_false`.
    fn yield_star(&mut self, emit: &mut Emitter) {
        let done_atom = self.done;
        let value_atom = self.value;
        let check_done = |emit: &mut Emitter| {
            simple(emit, op::ITERATOR_CHECK_OBJECT);
            put(emit, op::GET_FIELD2, &[Fo::Atom(done_atom)]);
        };
        let looped = emit.label();
        let yielded = emit.label();
        let next = emit.label();
        let returned = emit.label();
        let returned_value = emit.label();
        let thrown = emit.label();
        let no_throw = emit.label();
        let no_return = emit.label();
        let end = emit.label();
        // [iterator, next] -> [iterator, next, undefined, undefined]: a
        // catch-offset slot that holds no offset, and the first value.
        simple(emit, op::UNDEFINED);
        simple(emit, op::UNDEFINED);
        emit.bind(looped);
        simple(emit, op::ITERATOR_NEXT);
        check_done(emit);
        jump(emit, op::IF_TRUE, next);
        emit.bind(yielded);
        simple(emit, op::YIELD_STAR);
        simple(emit, op::DUP);
        jump(emit, op::IF_TRUE, returned);
        simple(emit, op::DROP);
        jump(emit, op::GOTO, looped);
        emit.bind(returned);
        put(emit, op::PUSH_I32, &[Fo::Immediate(2)]);
        simple(emit, op::STRICT_EQ);
        jump(emit, op::IF_TRUE, thrown);
        // `return`: call the iterator's `return`, keep yielding while it
        // is not done.
        put(emit, op::ITERATOR_CALL, &[Fo::Immediate(0)]);
        jump(emit, op::IF_TRUE, returned_value);
        check_done(emit);
        jump(emit, op::IF_FALSE, yielded);
        put(emit, op::GET_FIELD, &[Fo::Atom(value_atom)]);
        emit.bind(returned_value);
        simple(emit, op::NIP);
        simple(emit, op::NIP);
        simple(emit, op::NIP);
        simple(emit, op::PUSH_TRUE);
        jump(emit, op::GOTO, end);
        // `throw`: call the iterator's `throw`; without one, close the
        // iterator and raise the TypeError.
        emit.bind(thrown);
        put(emit, op::ITERATOR_CALL, &[Fo::Immediate(1)]);
        jump(emit, op::IF_TRUE, no_throw);
        check_done(emit);
        jump(emit, op::IF_FALSE, yielded);
        jump(emit, op::GOTO, next);
        emit.bind(no_throw);
        put(emit, op::ITERATOR_CALL, &[Fo::Immediate(2)]);
        jump(emit, op::IF_TRUE, no_return);
        emit.bind(no_return);
        put(
            emit,
            op::THROW_ERROR,
            &[
                Fo::Atom(fork::Atom::NULL),
                Fo::Immediate(i32::from(throw_error::ERROR_ITERATOR_THROW)),
            ],
        );
        // Done: the value of the last result.
        emit.bind(next);
        put(emit, op::GET_FIELD, &[Fo::Atom(value_atom)]);
        simple(emit, op::NIP);
        simple(emit, op::NIP);
        simple(emit, op::NIP);
        simple(emit, op::PUSH_FALSE);
        emit.bind(end);
    }

    /// `push_const pattern; push_const bytecode; regexp`: the bytecode is
    /// replaced by what the fork compiles for the pattern and the flags of
    /// the `PrimJS` bytecode's header.
    fn regexp(
        &mut self,
        function: &Function,
        at: usize,
        is_target: &[bool],
        emit: &mut Emitter,
        path: &str,
        appended: &mut Vec<fork::Value>,
    ) -> Result<(), ConvertError> {
        let pc = function.instructions[at].pc;
        let error = |detail: &str| ConvertError::RegExp {
            function: path.to_owned(),
            pc,
            detail: detail.to_owned(),
        };
        let string_at = |index: usize| -> Option<&super::super::model::JsString> {
            let instruction = function.instructions.get(index)?;
            if !matches!(instruction.opcode, Opcode::PushConst | Opcode::PushConst8) {
                return None;
            }
            let Some(Operand::Const(entry)) = instruction.operands().first() else {
                return None;
            };
            match function.cpool.get(*entry as usize)? {
                Value::String(text) => Some(text),
                _ => None,
            }
        };
        let (Some(pattern), Some(bytecode)) = (
            at.checked_sub(2).and_then(string_at),
            at.checked_sub(1).and_then(string_at),
        ) else {
            return Err(error("regexp does not follow push_const of two strings"));
        };
        if is_target[at] || is_target[at - 1] {
            return Err(error("a jump lands inside a regular expression literal"));
        }
        let super::super::model::JsString::Latin1(bytecode) = bytecode else {
            return Err(error(
                "the PrimJS regular expression bytecode is not a byte string",
            ));
        };
        let header = *bytecode
            .first()
            .ok_or_else(|| error("empty PrimJS regular expression bytecode"))?;
        // LRE_FLAG_* of PrimJS's libregexp.h: g i m s u y; bit 7 records
        // named groups and is not a flag of the literal.
        if header & 0x40 != 0 {
            return Err(error("unknown regular expression flag bit 6"));
        }
        let mut flags = String::new();
        for (bit, letter) in [(0, 'g'), (1, 'i'), (2, 'm'), (3, 's'), (4, 'u'), (5, 'y')] {
            if header & (1 << bit) != 0 {
                flags.push(letter);
            }
        }
        let units: Vec<u16> = match pattern {
            super::super::model::JsString::Latin1(bytes) => {
                bytes.iter().map(|&byte| u16::from(byte)).collect()
            }
            super::super::model::JsString::Utf16(units) => units.clone(),
        };
        let compiled = self
            .regexps
            .compile_regexp(&units, &flags)
            .map_err(|refusal| error(&refusal.to_string()))?;
        let index = u32::try_from(function.cpool.len() + appended.len())
            .map_err(|_| error("constant pool too large"))?;
        appended.push(fork::Value::String(fork::JsString::Latin1(compiled)));
        let last = emit
            .code
            .last_mut()
            .ok_or_else(|| error("regexp without its operands"))?;
        *last = Instruction::new(op::PUSH_CONST, &[Fo::Const(index)])
            .map_err(|model| error(&model.message))?;
        simple(emit, op::REGEXP);
        Ok(())
    }

    /// A constant-pool value other than a function.
    pub(super) fn value(&mut self, value: &Value, path: &str) -> Result<fork::Value, ConvertError> {
        Ok(match value {
            Value::Null => fork::Value::Null,
            Value::Undefined => fork::Value::Undefined,
            Value::Bool(value) => fork::Value::Bool(*value),
            Value::Int32(value) => fork::Value::Int32(*value),
            Value::Float64(value) => fork::Value::Float64(*value),
            Value::String(text) => fork::Value::String(super::atoms::to_fork_string(text)),
            Value::BigInt(bytes) => fork::Value::BigInt(bytes.clone()),
            Value::Object(properties) => {
                let mut out = Vec::with_capacity(properties.len());
                for (name, value) in properties {
                    out.push((self.atom(*name, path)?, self.value(value, path)?));
                }
                fork::Value::Object(out)
            }
            Value::Array(elements) => fork::Value::Array(
                elements
                    .iter()
                    .map(|element| self.value(element, path))
                    .collect::<Result<_, _>>()?,
            ),
            Value::TemplateObject { elements, raw } => fork::Value::TemplateObject {
                elements: elements
                    .iter()
                    .map(|element| self.value(element, path))
                    .collect::<Result<_, _>>()?,
                raw: Box::new(self.value(raw, path)?),
            },
            Value::Function(_) => {
                return Err(ConvertError::shape(
                    path,
                    None,
                    "function bytecode inside a data value",
                ));
            }
        })
    }
}

fn target_of(instruction: &PInstruction) -> Label {
    Label::Input(instruction.target().unwrap_or(u32::MAX))
}

fn last_immediate(instruction: &PInstruction) -> i32 {
    instruction
        .operands()
        .iter()
        .rev()
        .find_map(|operand| match operand {
            Operand::Immediate(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(-1)
}

/// Appends an instruction whose operands are implicit or absent. A
/// refusal of the model is recorded and reported by [`resolve`].
fn simple(emit: &mut Emitter, opcode: u8) {
    emit.push(Instruction::simple(opcode));
}

/// Appends an instruction with explicit operands.
fn put(emit: &mut Emitter, opcode: u8, operands: &[Fo]) {
    emit.push(Instruction::new(opcode, operands));
}

fn jump(emit: &mut Emitter, opcode: u8, label: Label) {
    emit.fixups.push((emit.code.len(), label));
    emit.push(Instruction::new(opcode, &[Fo::Target(0)]));
}

/// Points every recorded jump at its output instruction.
fn resolve(emit: &mut Emitter, starts: &[u32], path: &str) -> Result<(), ConvertError> {
    if let Some(message) = emit.broken.take() {
        return Err(ConvertError::shape(path, None, &message));
    }
    for &(at, label) in &emit.fixups {
        let target = match label {
            Label::Input(index) => starts.get(index as usize).copied(),
            Label::Local(index) => emit.locals.get(index).copied().flatten(),
        }
        .ok_or_else(|| ConvertError::shape(path, None, "a jump to no instruction"))?;
        for operand in emit.code[at].operands_mut() {
            if let Fo::Target(slot) = operand {
                *slot = target;
            }
        }
    }
    Ok(())
}
