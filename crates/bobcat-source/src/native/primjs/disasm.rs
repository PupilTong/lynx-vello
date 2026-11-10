//! A deterministic text dump of a decoded program.

use std::fmt::Write as _;

use super::model::{
    Atom, DebugInfo, Function, FunctionKind, Header, JsString, Operand, Program, Value,
};

/// Renders `program` as text: the header, the atom table, then each function
/// (record, vardefs, closure variables, instructions, debug block, constant
/// pool) with nested functions indented under the pool entry that holds them.
///
/// Atoms are shown as quoted text (`null` for `JS_ATOM_NULL`, `int:N` for an
/// integer atom); jump operands as `-> PC`, the target instruction's pc.
/// The output depends only on the program, so tests can compare it.
#[must_use]
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    match program.header {
        Header::Legacy => out.push_str("header: version 1\n"),
        Header::Versioned(flags) => {
            out.push_str("header: version 9");
            if flags.external_debug_info {
                out.push_str(", external debug info");
            }
            if flags.vardefs_stripped {
                out.push_str(", vardefs stripped");
            }
            out.push('\n');
        }
    }
    let _ = writeln!(out, "atoms: {}", program.atoms.len());
    for (index, atom) in program.atoms.iter().enumerate() {
        let _ = writeln!(out, "  {index}: {}", string(atom));
    }
    function(&mut out, program, &program.root, 0);
    out
}

fn string(value: &JsString) -> String {
    match value {
        JsString::Latin1(_) => format!("{:?}", value.to_string_lossy()),
        JsString::Utf16(_) => format!("u{:?}", value.to_string_lossy()),
    }
}

fn atom(program: &Program, atom: Atom) -> String {
    match atom {
        Atom::NULL => "null".to_owned(),
        Atom::TaggedInt(value) => format!("int:{value}"),
        Atom::Builtin(_) | Atom::File(_) => match program.atom_text(atom) {
            Some(text) => format!("{text:?}"),
            None => format!("{atom:?}"),
        },
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one block per part of the record, in serialized order"
)]
fn function(out: &mut String, program: &Program, function: &Function, indent: usize) {
    pad(out, indent);
    let _ = writeln!(
        out,
        "function @{} {}",
        function.offset,
        atom(program, function.name)
    );
    let inner = indent + 2;

    let flags = function.flags;
    let mut names = Vec::new();
    for (set, name) in [
        (flags.has_prototype, "has_prototype"),
        (flags.has_simple_parameter_list, "has_simple_parameter_list"),
        (
            flags.is_derived_class_constructor,
            "is_derived_class_constructor",
        ),
        (flags.need_home_object, "need_home_object"),
        (flags.new_target_allowed, "new_target_allowed"),
        (flags.super_call_allowed, "super_call_allowed"),
        (flags.super_allowed, "super_allowed"),
        (flags.arguments_allowed, "arguments_allowed"),
        (flags.has_debug, "has_debug"),
    ] {
        if set {
            names.push(name);
        }
    }
    let kind = match flags.kind {
        FunctionKind::Normal => "normal",
        FunctionKind::Generator => "generator",
        FunctionKind::Async => "async",
        FunctionKind::AsyncGenerator => "async_generator",
    };
    pad(out, inner);
    let _ = writeln!(
        out,
        "kind {kind}, js_mode 0x{:02x}, flags [{}]",
        function.js_mode.0,
        names.join(" ")
    );
    pad(out, inner);
    let _ = writeln!(
        out,
        "args {}, vars {}, defined_args {}, stack {}, closure_vars {}, cpool {}, bytecode {} bytes @{}",
        function.arg_count,
        function.var_count,
        function.defined_arg_count,
        function.stack_size,
        function.closure_vars.len(),
        function.cpool.len(),
        function.bytecode_len,
        function.bytecode_offset
    );

    if !function.vardefs.is_empty() {
        pad(out, inner);
        out.push_str("vardefs:\n");
        for (index, var) in function.vardefs.iter().enumerate() {
            pad(out, inner + 2);
            let _ = write!(
                out,
                "{index}: {} scope_level {} scope_next {} kind {}",
                atom(program, var.name),
                var.scope_level,
                var.scope_next,
                var.var_kind
            );
            for (set, name) in [
                (var.is_const, " const"),
                (var.is_lexical, " lexical"),
                (var.is_captured, " captured"),
            ] {
                if set {
                    out.push_str(name);
                }
            }
            out.push('\n');
        }
    }

    if !function.closure_vars.is_empty() {
        pad(out, inner);
        out.push_str("closure_vars:\n");
        for (index, var) in function.closure_vars.iter().enumerate() {
            let source = match (var.is_local, var.is_arg) {
                (true, true) => "arg",
                (true, false) => "loc",
                (false, _) => "ref",
            };
            pad(out, inner + 2);
            let _ = write!(
                out,
                "{index}: {} {source}:{} kind {}",
                atom(program, var.name),
                var.var_idx,
                var.var_kind
            );
            for (set, name) in [(var.is_const, " const"), (var.is_lexical, " lexical")] {
                if set {
                    out.push_str(name);
                }
            }
            out.push('\n');
        }
    }

    pad(out, inner);
    out.push_str("code:\n");
    for instruction in &function.instructions {
        pad(out, inner + 2);
        let _ = write!(out, "{:>5}  {}", instruction.pc, instruction.opcode.name());
        for operand in instruction.operands() {
            out.push(' ');
            match *operand {
                Operand::Atom(value) => out.push_str(&atom(program, value)),
                Operand::Local(index) => {
                    let _ = write!(out, "loc:{index}");
                }
                Operand::Arg(index) => {
                    let _ = write!(out, "arg:{index}");
                }
                Operand::ClosureVar(index) => {
                    let _ = write!(out, "ref:{index}");
                }
                Operand::Const(index) => {
                    let _ = write!(out, "const:{index}");
                }
                Operand::Immediate(value) => {
                    let _ = write!(out, "{value}");
                }
                Operand::Target(index) => {
                    let pc = function
                        .instructions
                        .get(index as usize)
                        .map_or(u32::MAX, |target| target.pc);
                    let _ = write!(out, "-> {pc}");
                }
            }
        }
        out.push('\n');
    }

    match &function.debug {
        None => {}
        Some(DebugInfo::Inline {
            filename,
            line_num,
            pc2line,
        }) => {
            pad(out, inner);
            let _ = writeln!(
                out,
                "debug: inline, filename {}, line {line_num}, pc2line {} bytes",
                atom(program, *filename),
                pc2line.len()
            );
        }
        Some(DebugInfo::External {
            filename,
            function_id,
        }) => {
            pad(out, inner);
            let _ = writeln!(
                out,
                "debug: external, filename {}, function_id {function_id}",
                atom(program, *filename)
            );
        }
    }

    if !function.cpool.is_empty() {
        pad(out, inner);
        out.push_str("cpool:\n");
        for (index, value) in function.cpool.iter().enumerate() {
            pad(out, inner + 2);
            let _ = write!(out, "{index}:");
            if let Value::Function(child) = value {
                out.push('\n');
                self::function(out, program, child, inner + 4);
            } else {
                let mut nested = Vec::new();
                out.push(' ');
                out.push_str(&inline(program, value, &mut nested));
                out.push('\n');
                for child in nested {
                    self::function(out, program, child, inner + 4);
                }
            }
        }
    }
}

/// A value on one line. Functions nested in arrays or objects are named by
/// offset here and collected so the caller can dump them below.
fn inline<'a>(program: &Program, value: &'a Value, nested: &mut Vec<&'a Function>) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Undefined => "undefined".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Int32(value) => format!("int32 {value}"),
        Value::Float64(value) => format!("float64 {value:?}"),
        Value::String(value) => format!("string {}", string(value)),
        Value::BigInt(bytes) => {
            let mut text = String::from("bigint 0x");
            for byte in bytes.iter().rev() {
                let _ = write!(text, "{byte:02x}");
            }
            if bytes.is_empty() {
                text.push('0');
            }
            text
        }
        Value::Array(elements) => format!("array [{}]", list(program, elements, nested)),
        Value::TemplateObject { elements, raw } => format!(
            "template [{}] raw {}",
            list(program, elements, nested),
            inline(program, raw, nested)
        ),
        Value::Object(properties) => {
            let items: Vec<String> = properties
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}: {}",
                        atom(program, *key),
                        inline(program, value, nested)
                    )
                })
                .collect();
            format!("object {{{}}}", items.join(", "))
        }
        Value::Function(function) => {
            nested.push(function);
            format!("function @{}", function.offset)
        }
    }
}

fn list<'a>(program: &Program, values: &'a [Value], nested: &mut Vec<&'a Function>) -> String {
    values
        .iter()
        .map(|value| inline(program, value, nested))
        .collect::<Vec<_>>()
        .join(", ")
}
