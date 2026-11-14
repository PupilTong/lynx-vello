//! A text listing of a [`Program`], for tests and debugging.

use std::fmt::Write as _;

use super::model::{Atom, Export, Function, JsString, Module, Operand, Program, Value};

/// The program as text: records, then each function's instructions with
/// their index, byte offset, name and operands, then its constant pool.
#[must_use]
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "atoms: {}", program.atoms.len());
    match &program.root {
        Value::Function(function) => function_text(&mut out, program, function, "root", 0),
        Value::Module(module) => module_text(&mut out, program, module),
        other => {
            let _ = writeln!(out, "value {}", value_text(program, other));
        }
    }
    out
}

fn atom_text(program: &Program, atom: Atom) -> String {
    match program.atom_text(atom) {
        Some(text) => format!("{text:?}"),
        None if atom == Atom::NULL => "null-atom".to_owned(),
        None => format!("{atom:?}"),
    }
}

fn string_text(string: &JsString) -> String {
    format!("{:?}", string.to_string_lossy())
}

fn value_text(program: &Program, value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Undefined => "undefined".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Int32(value) => value.to_string(),
        Value::Float64(value) => format!("{value:?}"),
        Value::String(string) => string_text(string),
        Value::BigInt(bytes) => format!("bigint{bytes:02x?}"),
        Value::Array(elements) => format!(
            "[{}]",
            elements
                .iter()
                .map(|element| value_text(program, element))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::TemplateObject { elements, raw } => format!(
            "template[{}] raw {}",
            elements
                .iter()
                .map(|element| value_text(program, element))
                .collect::<Vec<_>>()
                .join(", "),
            value_text(program, raw)
        ),
        Value::Object(properties) => format!(
            "{{{}}}",
            properties
                .iter()
                .map(|(name, value)| format!(
                    "{}: {}",
                    atom_text(program, *name),
                    value_text(program, value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Function(function) => format!("function {}", atom_text(program, function.name)),
        Value::Module(module) => format!("module {}", atom_text(program, module.name)),
    }
}

fn module_text(out: &mut String, program: &Program, module: &Module) {
    let _ = writeln!(out, "module {}", atom_text(program, module.name));
    for (index, request) in module.requests.iter().enumerate() {
        let _ = writeln!(
            out,
            "  request {index}: {} attributes {}",
            atom_text(program, request.specifier),
            value_text(program, &request.attributes)
        );
    }
    for export in &module.exports {
        match *export {
            Export::Local { var_idx, name } => {
                let _ = writeln!(
                    out,
                    "  export {} = closure {var_idx}",
                    atom_text(program, name)
                );
            }
            Export::Indirect {
                request,
                local_name,
                name,
            } => {
                let _ = writeln!(
                    out,
                    "  export {} = {} from request {request}",
                    atom_text(program, name),
                    atom_text(program, local_name)
                );
            }
        }
    }
    for request in &module.star_exports {
        let _ = writeln!(out, "  export * from request {request}");
    }
    for import in &module.imports {
        let _ = writeln!(
            out,
            "  import {}{} from request {} into closure {}",
            if import.is_star { "* as " } else { "" },
            atom_text(program, import.name),
            import.request,
            import.var_idx
        );
    }
    let _ = writeln!(out, "  has_tla {}", module.has_tla);
    function_text(out, program, &module.function, "module", 1);
}

fn function_text(
    out: &mut String,
    program: &Program,
    function: &Function,
    path: &str,
    indent: usize,
) {
    let pad = "  ".repeat(indent);
    let _ = writeln!(
        out,
        "{pad}function {path} {} kind {:?} js_mode {:#x} args {} vars {} defined {} stack {} var_refs {}",
        atom_text(program, function.name),
        function.flags.kind,
        function.js_mode,
        function.arg_count,
        function.var_count,
        function.defined_arg_count,
        function.stack_size,
        function.var_ref_count
    );
    for (index, vardef) in function.vardefs.iter().enumerate() {
        let _ = writeln!(
            out,
            "{pad}  vardef {index} {} scope_next {} var_ref {} kind {}{}{}{}{}",
            atom_text(program, vardef.name),
            vardef.scope_next,
            vardef.var_ref_idx,
            vardef.var_kind,
            if vardef.is_const { " const" } else { "" },
            if vardef.is_lexical { " lexical" } else { "" },
            if vardef.is_captured { " captured" } else { "" },
            if vardef.has_scope { " scoped" } else { "" },
        );
    }
    for (index, closure_var) in function.closure_vars.iter().enumerate() {
        let _ = writeln!(
            out,
            "{pad}  closure {index} {} {:?} {}{}{} kind {}",
            atom_text(program, closure_var.name),
            closure_var.closure_type,
            closure_var.var_idx,
            if closure_var.is_const { " const" } else { "" },
            if closure_var.is_lexical {
                " lexical"
            } else {
                ""
            },
            closure_var.var_kind
        );
    }
    let mut pc = 0usize;
    for (index, instruction) in function.instructions.iter().enumerate() {
        let operands: Vec<String> = instruction
            .operands()
            .iter()
            .map(|operand| match *operand {
                Operand::Atom(atom) => atom_text(program, atom),
                Operand::Local(index) => format!("loc{index}"),
                Operand::Arg(index) => format!("arg{index}"),
                Operand::ClosureVar(index) => format!("var_ref{index}"),
                Operand::Const(index) => format!("const{index}"),
                Operand::Immediate(value) => value.to_string(),
                Operand::Target(target) => format!("@{target}"),
            })
            .collect();
        let _ = writeln!(
            out,
            "{pad}  {index:5} {pc:6}: {} {}",
            instruction.info().name,
            operands.join(", ")
        );
        pc += usize::from(instruction.size());
    }
    if let Some(debug) = &function.debug {
        let _ = writeln!(
            out,
            "{pad}  debug {} pc2line {} bytes source {} bytes",
            atom_text(program, debug.filename),
            debug.pc2line.len(),
            debug.source.len()
        );
    }
    for (index, entry) in function.cpool.iter().enumerate() {
        match entry {
            Value::Function(child) => {
                function_text(
                    out,
                    program,
                    child,
                    &format!("{path}.cpool[{index}]"),
                    indent + 1,
                );
            }
            other => {
                let _ = writeln!(out, "{pad}  const {index}: {}", value_text(program, other));
            }
        }
    }
}
