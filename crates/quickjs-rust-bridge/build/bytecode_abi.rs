//! Generates the bridge's `bytecode_abi` tables from the vendored `QuickJS`
//! sources, and the C translation unit that compiles `quickjs.c` together
//! with the checks that those tables equal what the compiled C uses.
//!
//! The opcode and atom tables are read from `quickjs-opcode.h` and
//! `quickjs-atom.h`, honouring the one preprocessor switch they contain
//! (`SHORT_OPCODES`, whose value is read from `quickjs.c`). The other
//! constants are `enum`s and `#define`s that exist only in `quickjs.c` or
//! `libregexp.h`; they are parsed from the text. Nothing parsed here is
//! trusted: [`checked_core_source`] writes a `_Static_assert` for every
//! generated value into the translation unit that compiles `quickjs.c`,
//! and a runtime check for what C cannot compare at compile time (the
//! atom strings and kinds). Any directive or construct this parser does
//! not know fails the build.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// One instruction kind of the final bytecode.
struct Opcode {
    name: String,
    size: i64,
    pops: i64,
    pushes: i64,
    format: String,
}

struct Atom {
    name: String,
    text: String,
}

/// A group of named integer constants taken from one `enum` or a list of
/// `#define`s, emitted as one Rust module.
struct Group {
    module: &'static str,
    doc: &'static str,
    rust_type: &'static str,
    /// The C prefix dropped from each name to make the Rust constant name.
    prefix: &'static str,
    source: GroupSource,
}

enum GroupSource {
    /// `typedef enum ... { ... } <type>;` in `quickjs.c`.
    Enum(&'static str),
    /// `#define`s of `quickjs.c` (`false`) or `libregexp.h` (`true`).
    Defines(&'static [&'static str], bool),
}

const GROUPS: &[Group] = &[
    Group {
        module: "tag",
        doc: "Serialized value tags (`BCTagEnum`): the byte that starts each value.",
        rust_type: "u8",
        prefix: "BC_TAG_",
        source: GroupSource::Enum("BCTagEnum"),
    },
    Group {
        module: "closure_type",
        doc: "Closure variable types (`JSClosureTypeEnum`), the 3-bit `closure_type` field.",
        rust_type: "u8",
        prefix: "JS_CLOSURE_",
        source: GroupSource::Enum("JSClosureTypeEnum"),
    },
    Group {
        module: "var_kind",
        doc: "Variable kinds (`JSVarKindEnum`), the 4-bit `var_kind` field of vardefs and \
              closure variables.",
        rust_type: "u8",
        prefix: "JS_VAR_",
        source: GroupSource::Enum("JSVarKindEnum"),
    },
    Group {
        module: "func_kind",
        doc: "Function kinds (`JSFunctionKindEnum`), the 2-bit `func_kind` flag.",
        rust_type: "u8",
        prefix: "JS_FUNC_",
        source: GroupSource::Enum("JSFunctionKindEnum"),
    },
    Group {
        module: "export_type",
        doc: "Module export entry types (`JSExportTypeEnum`).",
        rust_type: "u8",
        prefix: "JS_EXPORT_TYPE_",
        source: GroupSource::Enum("JSExportTypeEnum"),
    },
    Group {
        module: "special_object",
        doc: "Operands of `special_object` (`OPSpecialObjectEnum`).",
        rust_type: "u8",
        prefix: "OP_SPECIAL_OBJECT_",
        source: GroupSource::Enum("OPSpecialObjectEnum"),
    },
    Group {
        module: "js_mode",
        doc: "Bits of a function's `js_mode` byte (`JS_MODE_*`).",
        rust_type: "u8",
        prefix: "JS_MODE_",
        source: GroupSource::Defines(
            &[
                "JS_MODE_STRICT",
                "JS_MODE_ASYNC",
                "JS_MODE_BACKTRACE_BARRIER",
            ],
            false,
        ),
    },
    Group {
        module: "throw_error",
        doc: "Type operands of `throw_error` (`JS_THROW_*`).",
        rust_type: "u8",
        prefix: "JS_THROW_",
        source: GroupSource::Defines(
            &[
                "JS_THROW_VAR_RO",
                "JS_THROW_VAR_REDECL",
                "JS_THROW_VAR_UNINITIALIZED",
                "JS_THROW_ERROR_DELETE_SUPER",
                "JS_THROW_ERROR_ITERATOR_THROW",
                "JS_THROW_ERROR_INVALID_LVALUE_CALL",
            ],
            false,
        ),
    },
    Group {
        module: "define_method",
        doc: "Operand bits of `define_method` and `define_method_computed` \
              (`OP_DEFINE_METHOD_*`).",
        rust_type: "u8",
        prefix: "OP_DEFINE_METHOD_",
        source: GroupSource::Defines(
            &[
                "OP_DEFINE_METHOD_METHOD",
                "OP_DEFINE_METHOD_GETTER",
                "OP_DEFINE_METHOD_SETTER",
                "OP_DEFINE_METHOD_ENUMERABLE",
            ],
            false,
        ),
    },
    Group {
        module: "pc2line",
        doc: "Constants of the line-number table encoding (`PC2LINE_*`).",
        rust_type: "i32",
        prefix: "PC2LINE_",
        source: GroupSource::Defines(
            &[
                "PC2LINE_BASE",
                "PC2LINE_RANGE",
                "PC2LINE_OP_FIRST",
                "PC2LINE_DIFF_PC_MAX",
            ],
            false,
        ),
    },
    Group {
        module: "arg_scope",
        doc: "Scope markers in a vardef's `scope_next` (`ARG_SCOPE_*`).",
        rust_type: "i32",
        prefix: "ARG_SCOPE_",
        source: GroupSource::Defines(&["ARG_SCOPE_INDEX", "ARG_SCOPE_END"], false),
    },
    Group {
        module: "limits",
        doc: "Limits the compiler, the reader and the interpreter apply.",
        rust_type: "u32",
        prefix: "",
        source: GroupSource::Defines(
            &[
                "JS_MAX_LOCAL_VARS",
                "JS_STACK_SIZE_MAX",
                "JS_STRING_LEN_MAX",
            ],
            false,
        ),
    },
    Group {
        module: "atom_int",
        doc: "Integer atoms: an atom value with `TAG_INT` set is the integer in its \
              other bits, which is how the runtime interns a property name that is a \
              canonical decimal integer up to `MAX_INT`.",
        rust_type: "u32",
        prefix: "JS_ATOM_",
        source: GroupSource::Defines(&["JS_ATOM_TAG_INT", "JS_ATOM_MAX_INT"], false),
    },
    Group {
        module: "regexp_flag",
        doc: "Regular expression flag bits (`LRE_FLAG_*`, `libregexp.h`), as the first \
              bytes of a compiled regular expression carry them.",
        rust_type: "u16",
        prefix: "LRE_FLAG_",
        source: GroupSource::Defines(
            &[
                "LRE_FLAG_GLOBAL",
                "LRE_FLAG_IGNORECASE",
                "LRE_FLAG_MULTILINE",
                "LRE_FLAG_DOTALL",
                "LRE_FLAG_UNICODE",
                "LRE_FLAG_STICKY",
                "LRE_FLAG_INDICES",
                "LRE_FLAG_NAMED_GROUPS",
                "LRE_FLAG_UNICODE_SETS",
            ],
            true,
        ),
    },
];

/// What [`generate`] produced.
pub struct Generated {
    pub rust: String,
    pub checked_core: String,
}

pub fn generate(quickjs_dir: &Path) -> Generated {
    let read = |name: &str| {
        let path = quickjs_dir.join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    };
    let quickjs_c = strip_comments(&read("quickjs.c"));
    let libregexp_h = strip_comments(&read("libregexp.h"));
    let opcode_h = strip_comments(&read("quickjs-opcode.h"));
    let atom_h = strip_comments(&read("quickjs-atom.h"));

    let mut known = HashMap::new();
    let short_opcodes = define_value(&quickjs_c, "SHORT_OPCODES", &known);
    known.insert("SHORT_OPCODES".to_owned(), short_opcodes);
    let bc_version = define_value(&quickjs_c, "BC_VERSION", &known);
    assert!(
        (0..=255).contains(&bc_version),
        "BC_VERSION {bc_version} is not a byte"
    );

    let (formats, opcodes) = parse_opcodes(&opcode_h, &known);
    let atoms = parse_atoms(&atom_h);

    let mut groups = Vec::new();
    for group in GROUPS {
        let entries = match group.source {
            GroupSource::Enum(type_name) => parse_enum(&quickjs_c, type_name, &mut known),
            GroupSource::Defines(names, in_libregexp) => {
                let text = if in_libregexp {
                    &libregexp_h
                } else {
                    &quickjs_c
                };
                names
                    .iter()
                    .map(|name| {
                        let value = define_value(text, name, &known);
                        known.insert((*name).to_owned(), value);
                        ((*name).to_owned(), value)
                    })
                    .collect()
            }
        };
        groups.push((group, entries));
    }

    let rust = rust_source(bc_version, &formats, &opcodes, &atoms, &groups);
    let checked_core =
        checked_core_source(quickjs_dir, bc_version, &formats, &opcodes, &atoms, &groups);
    Generated { rust, checked_core }
}

/// Replaces every comment by a space, keeping string literals and line
/// breaks, so a commented-out definition is never read.
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let end = text[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |offset| index + 2 + offset + 2);
                for &byte in &bytes[index..end] {
                    if byte == b'\n' {
                        out.push('\n');
                    }
                }
                out.push(' ');
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            quote @ (b'"' | b'\'') => {
                let start = index;
                index += 1;
                while index < bytes.len() && bytes[index] != quote {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                index = (index + 1).min(bytes.len());
                out.push_str(&text[start..index]);
            }
            _ => {
                let start = index;
                index += 1;
                while index < bytes.len() && !matches!(bytes[index], b'/' | b'"' | b'\'') {
                    index += 1;
                }
                out.push_str(&text[start..index]);
            }
        }
    }
    out
}

/// The arguments of a `NAME(a, b, ...)` line, split on top-level commas.
fn macro_arguments(line: &str, name: &str) -> Option<Vec<String>> {
    let rest = line.trim().strip_prefix(name)?.trim_start();
    let inner = rest.strip_prefix('(')?;
    let close = inner.rfind(')')?;
    assert!(
        inner[close + 1..].trim().is_empty(),
        "unexpected text after {name}(...): {line}"
    );
    Some(
        split_top_level(&inner[..close])
            .into_iter()
            .map(|argument| argument.trim().to_owned())
            .collect(),
    )
}

fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = 0;
    for (offset, character) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// The operand formats in `FMT` order and the final opcodes in `DEF` order
/// (the numbering of `OPCodeEnum`), skipping the lowercase `def` temporaries
/// and honouring `#if SHORT_OPCODES`.
fn parse_opcodes(header: &str, known: &HashMap<String, i64>) -> (Vec<String>, Vec<Opcode>) {
    let mut formats = Vec::new();
    let mut opcodes = Vec::new();
    // One entry per open conditional: whether its body is compiled.
    let mut active: Vec<bool> = Vec::new();
    for line in header.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(directive) = line.strip_prefix('#') {
            let directive = directive.trim();
            if directive == "endif" {
                active.pop().expect("unbalanced #endif in quickjs-opcode.h");
            } else if let Some(condition) = directive.strip_prefix("if ") {
                active.push(evaluate(condition, known) != 0);
            } else if matches!(directive, "ifdef FMT" | "ifdef DEF" | "ifndef def") {
                // The X-macro guards: the C build includes the header with
                // FMT or DEF defined and `def` mapped to DEF unless defined.
                active.push(true);
            } else if directive.starts_with("define def(")
                || matches!(directive, "undef DEF" | "undef def" | "undef FMT")
            {
            } else {
                panic!("quickjs-opcode.h: directive this generator does not know: #{directive}");
            }
            continue;
        }
        if active.contains(&false) {
            continue;
        }
        if let Some(arguments) = macro_arguments(line, "FMT") {
            assert_eq!(arguments.len(), 1, "quickjs-opcode.h: {line}");
            formats.push(arguments[0].clone());
        } else if let Some(arguments) = macro_arguments(line, "DEF") {
            assert_eq!(arguments.len(), 5, "quickjs-opcode.h: {line}");
            opcodes.push(Opcode {
                name: arguments[0].clone(),
                size: evaluate(&arguments[1], known),
                pops: evaluate(&arguments[2], known),
                pushes: evaluate(&arguments[3], known),
                format: arguments[4].clone(),
            });
        } else if macro_arguments(line, "def").is_none() {
            panic!("quickjs-opcode.h: line this generator does not know: {line}");
        }
    }
    assert!(active.is_empty(), "unbalanced #if in quickjs-opcode.h");
    for opcode in &opcodes {
        assert!(
            formats.contains(&opcode.format),
            "opcode {} names an unknown format {}",
            opcode.name,
            opcode.format
        );
        for value in [opcode.size, opcode.pops, opcode.pushes] {
            assert!(
                (0..=255).contains(&value),
                "opcode {} field out of range",
                opcode.name
            );
        }
    }
    assert!(opcodes.len() <= 256, "more opcodes than a byte numbers");
    (formats, opcodes)
}

fn parse_atoms(header: &str) -> Vec<Atom> {
    let mut atoms = Vec::new();
    for line in header.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(directive) = line.strip_prefix('#') {
            assert!(
                matches!(directive.trim(), "ifdef DEF" | "endif"),
                "quickjs-atom.h: directive this generator does not know: {line}"
            );
            continue;
        }
        let arguments = macro_arguments(line, "DEF")
            .unwrap_or_else(|| panic!("quickjs-atom.h: line this generator does not know: {line}"));
        assert_eq!(arguments.len(), 2, "quickjs-atom.h: {line}");
        atoms.push(Atom {
            name: arguments[0].clone(),
            text: c_string_literal(&arguments[1]),
        });
    }
    atoms
}

fn c_string_literal(literal: &str) -> String {
    let inner = literal
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("not a single C string literal: {literal}"));
    let mut text = String::new();
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('\\') => text.push('\\'),
                Some('"') => text.push('"'),
                Some('n') => text.push('\n'),
                Some('t') => text.push('\t'),
                other => panic!("C escape this generator does not know: \\{other:?}"),
            }
        } else {
            assert!(
                character.is_ascii() && !character.is_ascii_control(),
                "atom string with a byte this generator does not copy: {literal}"
            );
            text.push(character);
        }
    }
    text
}

/// The value of `#define name <expression>` (an object-like macro).
fn define_value(text: &str, name: &str, known: &HashMap<String, i64>) -> i64 {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("define") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix(name) else {
            continue;
        };
        if !after.starts_with([' ', '\t']) {
            continue;
        }
        assert!(found.is_none(), "{name} is defined more than once");
        found = Some(after.trim().to_owned());
    }
    let expression = found.unwrap_or_else(|| panic!("no #define {name}"));
    evaluate(&expression, known)
}

/// The entries of `typedef enum ... { ... } <type_name>;`, adding each one
/// to `known` so later expressions may name it.
fn parse_enum(text: &str, type_name: &str, known: &mut HashMap<String, i64>) -> Vec<(String, i64)> {
    let closing = format!("}} {type_name};");
    let end = text
        .find(&closing)
        .unwrap_or_else(|| panic!("no enum {type_name}"));
    assert!(
        text[end + closing.len()..].find(&closing).is_none(),
        "enum {type_name} is defined more than once"
    );
    let open = text[..end]
        .rfind('{')
        .unwrap_or_else(|| panic!("enum {type_name} has no body"));
    let head = text[..open].rsplit(';').next().unwrap_or("");
    assert!(
        head.contains("enum"),
        "{type_name} is not an enum: {}",
        head.trim()
    );
    let body = &text[open + 1..end];
    assert!(!body.contains('#'), "enum {type_name} contains a directive");
    let mut entries = Vec::new();
    let mut next = 0i64;
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (name, value) = match item.split_once('=') {
            Some((name, expression)) => (name.trim(), evaluate(expression, known)),
            None => (item, next),
        };
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "enum {type_name}: entry this generator does not know: {item}"
        );
        known.insert(name.to_owned(), value);
        entries.push((name.to_owned(), value));
        next = value + 1;
    }
    entries
}

/// Evaluates an integer constant expression of literals, known names and
/// the operators `( ) ~ ! * / % + - << >> & ^ |`.
fn evaluate(expression: &str, known: &HashMap<String, i64>) -> i64 {
    let tokens = tokenize(expression);
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        known,
        expression,
    };
    let value = parser.bitwise_or();
    assert!(
        parser.position == tokens.len(),
        "unexpected text in constant expression: {expression}"
    );
    value
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(i64),
    Name(String),
    Operator(&'static str),
}

fn tokenize(expression: &str) -> Vec<Token> {
    const OPERATORS: [&str; 14] = [
        "<<", ">>", "(", ")", "~", "!", "*", "/", "%", "+", "-", "&", "^", "|",
    ];
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
        } else if byte.is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
                index += 1;
            }
            let literal = expression[start..index].trim_end_matches(['u', 'U', 'l', 'L']);
            let value = if let Some(hex) = literal
                .strip_prefix("0x")
                .or_else(|| literal.strip_prefix("0X"))
            {
                i64::from_str_radix(hex, 16)
            } else {
                literal.parse()
            }
            .unwrap_or_else(|_| {
                panic!("integer literal this generator does not read: {expression}")
            });
            tokens.push(Token::Number(value));
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token::Name(expression[start..index].to_owned()));
        } else {
            let operator = OPERATORS
                .iter()
                .find(|operator| expression[index..].starts_with(**operator))
                .unwrap_or_else(|| {
                    panic!("operator this generator does not know in: {expression}")
                });
            tokens.push(Token::Operator(operator));
            index += operator.len();
        }
    }
    tokens
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    known: &'a HashMap<String, i64>,
    expression: &'a str,
}

impl Parser<'_> {
    fn eat(&mut self, operators: &[&'static str]) -> Option<&'static str> {
        if let Some(Token::Operator(operator)) = self.tokens.get(self.position)
            && operators.contains(operator)
        {
            self.position += 1;
            return Some(operator);
        }
        None
    }

    fn binary(&mut self, operators: &[&'static str], next: fn(&mut Self) -> i64) -> i64 {
        let mut value = next(self);
        while let Some(operator) = self.eat(operators) {
            let right = next(self);
            value = match operator {
                "|" => value | right,
                "^" => value ^ right,
                "&" => value & right,
                "<<" => value << right,
                ">>" => value >> right,
                "+" => value + right,
                "-" => value - right,
                "*" => value * right,
                "/" => value / right,
                "%" => value % right,
                _ => unreachable!(),
            };
        }
        value
    }

    fn bitwise_or(&mut self) -> i64 {
        self.binary(&["|"], Self::bitwise_xor)
    }

    fn bitwise_xor(&mut self) -> i64 {
        self.binary(&["^"], Self::bitwise_and)
    }

    fn bitwise_and(&mut self) -> i64 {
        self.binary(&["&"], Self::shift)
    }

    fn shift(&mut self) -> i64 {
        self.binary(&["<<", ">>"], Self::additive)
    }

    fn additive(&mut self) -> i64 {
        self.binary(&["+", "-"], Self::multiplicative)
    }

    fn multiplicative(&mut self) -> i64 {
        self.binary(&["*", "/", "%"], Self::unary)
    }

    fn unary(&mut self) -> i64 {
        match self.eat(&["-", "+", "~", "!"]) {
            Some("-") => -self.unary(),
            Some("+") => self.unary(),
            Some("~") => !self.unary(),
            Some("!") => i64::from(self.unary() == 0),
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> i64 {
        if self.eat(&["("]).is_some() {
            let value = self.bitwise_or();
            assert!(
                self.eat(&[")"]).is_some(),
                "unbalanced parentheses in: {}",
                self.expression
            );
            return value;
        }
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        match token {
            Some(Token::Number(value)) => value,
            Some(Token::Name(name)) => *self.known.get(&name).unwrap_or_else(|| {
                panic!(
                    "constant expression names {name}, which is not known: {}",
                    self.expression
                )
            }),
            _ => panic!(
                "constant expression this generator does not read: {}",
                self.expression
            ),
        }
    }
}

fn camel_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + characters.as_str()
            })
        })
        .collect()
}

fn rust_string(text: &str) -> String {
    format!("{text:?}")
}

fn atom_kind(index: usize, atoms: &[Atom]) -> &'static str {
    let position = |name: &str| {
        atoms
            .iter()
            .position(|atom| atom.name == name)
            .unwrap_or_else(|| panic!("no atom {name}"))
            + 1
    };
    if index == 0 {
        "Null"
    } else if index == position("Private_brand") {
        "PrivateSymbol"
    } else if index >= position("Symbol_toPrimitive") {
        "Symbol"
    } else {
        "String"
    }
}

fn rust_source(
    bc_version: i64,
    formats: &[String],
    opcodes: &[Opcode],
    atoms: &[Atom],
    groups: &[(&Group, Vec<(String, i64)>)],
) -> String {
    let mut out = String::new();
    out.push_str("// Generated by build/bytecode_abi.rs from vendor/quickjs. Do not edit.\n\n");
    writeln!(
        out,
        "/// `BC_VERSION`: the first byte of every bytecode file.\npub const BC_VERSION: u8 = {bc_version};\n"
    )
    .unwrap();

    out.push_str(
        "/// How an instruction's operand bytes are laid out (`OPCodeFormat`, the\n\
         /// `FMT` entries of `quickjs-opcode.h`), in their C order.\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n#[repr(u8)]\npub enum OperandFormat {\n",
    );
    for (index, format) in formats.iter().enumerate() {
        writeln!(
            out,
            "    /// `OP_FMT_{format}`\n    {} = {index},",
            camel_case(format)
        )
        .unwrap();
    }
    out.push_str("}\n\n");
    rust_opcodes(&mut out, opcodes);
    rust_atoms(&mut out, atoms);
    rust_groups(&mut out, groups);
    out
}

fn rust_opcodes(out: &mut String, opcodes: &[Opcode]) {
    writeln!(
        out,
        "/// The number of opcodes of the final bytecode (`OP_COUNT`): every byte\n\
         /// below it starts an instruction, every byte at or above it is invalid.\n\
         pub const OP_COUNT: usize = {};\n",
        opcodes.len()
    )
    .unwrap();
    out.push_str(
        "/// The opcodes of the final bytecode, indexed by opcode byte: the `DEF`\n\
         /// entries of `quickjs-opcode.h` with the short opcodes the build enables,\n\
         /// without the `def` temporaries the compiler removes before it emits.\n\
         pub static OPCODES: [Opcode; OP_COUNT] = [\n",
    );
    for opcode in opcodes {
        writeln!(
            out,
            "    Opcode {{ name: {}, size: {}, pops: {}, pushes: {}, format: OperandFormat::{} }},",
            rust_string(&opcode.name),
            opcode.size,
            opcode.pops,
            opcode.pushes,
            camel_case(&opcode.format)
        )
        .unwrap();
    }
    out.push_str("];\n\n");
    out.push_str("/// Opcode bytes by name (`OP_<name>`, upper-cased).\npub mod op {\n");
    for (index, opcode) in opcodes.iter().enumerate() {
        writeln!(
            out,
            "    pub const {}: u8 = {index};",
            opcode.name.to_ascii_uppercase()
        )
        .unwrap();
    }
    out.push_str("}\n\n");
}

fn rust_atoms(out: &mut String, atoms: &[Atom]) {
    writeln!(
        out,
        "/// One past the last built-in atom (`JS_ATOM_END`). A bytecode file's atom\n\
         /// indices below it name built-in atoms; its own atom table starts here.\n\
         pub const JS_ATOM_END: u32 = {};\n",
        atoms.len() + 1
    )
    .unwrap();
    out.push_str(
        "/// The built-in atoms by atom number. Index 0 is the null atom\n\
         /// (`JS_ATOM_NULL`), which has no text.\n\
         pub static ATOMS: [BuiltinAtom; JS_ATOM_END as usize] = [\n\
         \x20   BuiltinAtom { name: \"\", text: \"\", kind: AtomKind::Null },\n",
    );
    for (index, atom) in atoms.iter().enumerate() {
        writeln!(
            out,
            "    BuiltinAtom {{ name: {}, text: {}, kind: AtomKind::{} }},",
            rust_string(&atom.name),
            rust_string(&atom.text),
            atom_kind(index + 1, atoms)
        )
        .unwrap();
    }
    out.push_str("];\n\n");
}

fn rust_groups(out: &mut String, groups: &[(&Group, Vec<(String, i64)>)]) {
    for (group, entries) in groups {
        writeln!(out, "/// {}\npub mod {} {{", group.doc, group.module).unwrap();
        for (name, value) in entries {
            let rust_name = name
                .strip_prefix(group.prefix)
                .unwrap_or_else(|| panic!("{name} does not start with {}", group.prefix));
            writeln!(
                out,
                "    /// `{name}`\n    pub const {rust_name}: {} = {};",
                group.rust_type,
                rust_integer(*value)
            )
            .unwrap();
        }
        out.push_str("}\n\n");
    }
}

/// An integer literal with `_` between groups of three digits.
fn rust_integer(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('_');
        }
        grouped.push(digit);
    }
    if value < 0 {
        grouped.insert(0, '-');
    }
    grouped
}

fn c_string(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The translation unit that compiles `quickjs.c`: the file itself, then a
/// static assertion per generated value, then the runtime atom check.
fn checked_core_source(
    quickjs_dir: &Path,
    bc_version: i64,
    formats: &[String],
    opcodes: &[Opcode],
    atoms: &[Atom],
    groups: &[(&Group, Vec<(String, i64)>)],
) -> String {
    let quickjs_c = quickjs_dir.join("quickjs.c");
    let mut out = String::new();
    out.push_str(
        "/* Generated by crates/quickjs-rust-bridge/build/bytecode_abi.rs. Compiles\n   \
         quickjs.c, then checks that the bridge's generated bytecode tables equal\n   \
         what that compile uses. */\n",
    );
    writeln!(
        out,
        "#include {}\n",
        c_string(&quickjs_c.to_string_lossy().replace('\\', "/"))
    )
    .unwrap();
    let assert = |out: &mut String, condition: String, what: &str| {
        writeln!(
            out,
            "_Static_assert({condition}, {});",
            c_string(&format!(
                "bytecode_abi: {what} differs from the compiled QuickJS"
            ))
        )
        .unwrap();
    };
    assert(
        &mut out,
        format!("BC_VERSION == {bc_version}"),
        "BC_VERSION",
    );
    for (index, format) in formats.iter().enumerate() {
        assert(
            &mut out,
            format!("OP_FMT_{format} == {index}"),
            &format!("OP_FMT_{format}"),
        );
    }
    assert(
        &mut out,
        format!("OP_COUNT == {}", opcodes.len()),
        "OP_COUNT",
    );
    c_opcode_asserts(&mut out, opcodes, assert);
    assert(
        &mut out,
        format!("JS_ATOM_END == {}", atoms.len() + 1),
        "JS_ATOM_END",
    );
    for (index, atom) in atoms.iter().enumerate() {
        assert(
            &mut out,
            format!("JS_ATOM_{} == {}", atom.name, index + 1),
            &format!("JS_ATOM_{}", atom.name),
        );
    }
    for (_, entries) in groups {
        for (name, value) in entries {
            assert(&mut out, format!("({name}) == ({value})"), name);
        }
    }
    c_atom_check(&mut out, atoms);
    out
}

/// One assertion per opcode number, then the header expanded again with
/// `DEF` making one assertion per row.
fn c_opcode_asserts(
    out: &mut String,
    opcodes: &[Opcode],
    assert: impl Fn(&mut String, String, &str),
) {
    for (index, opcode) in opcodes.iter().enumerate() {
        assert(
            out,
            format!("OP_{} == {index}", opcode.name),
            &format!("OP_{}", opcode.name),
        );
        writeln!(
            out,
            "#define QJS_ABI_OP_{} {}, {}, {}, OP_FMT_{}",
            opcode.name, opcode.size, opcode.pops, opcode.pushes, opcode.format
        )
        .unwrap();
    }
    // The header expanded again, the way `opcode_info` expands it: one
    // assertion per final opcode comparing its size, stack effect and
    // format with the generated row.
    out.push_str(
        "#define QJS_ABI_ROW_EQ(size, n_pop, n_push, fmt, size2, n_pop2, n_push2, fmt2) \\\n    \
         ((size) == (size2) && (n_pop) == (n_pop2) && (n_push) == (n_push2) && (fmt) == (fmt2))\n\
         #define QJS_ABI_ROW_EQ1(...) QJS_ABI_ROW_EQ(__VA_ARGS__)\n\
         #define FMT(f)\n\
         #define def(id, size, n_pop, n_push, f)\n\
         #define DEF(id, size, n_pop, n_push, f) \\\n    \
         _Static_assert(QJS_ABI_ROW_EQ1(size, n_pop, n_push, OP_FMT_ ## f, QJS_ABI_OP_ ## id), \\\n                   \
         \"bytecode_abi: opcode \" #id \" differs from quickjs-opcode.h\");\n\
         #include \"quickjs-opcode.h\"\n",
    );
}

/// A function comparing the atom texts and kinds of a new runtime, as
/// `JS_InitAtoms` made them, with the generated table.
fn c_atom_check(out: &mut String, atoms: &[Atom]) {
    out.push_str(
        "\n/* Returns -1 when every built-in atom of a new runtime has the text and\n   \
         the kind the bridge's ATOMS table gives it, otherwise the first atom\n   \
         number that differs; -2 when no runtime could be made. */\n\
         int qjs_bytecode_abi_atom_mismatch(void);\n\
         int qjs_bytecode_abi_atom_mismatch(void)\n{\n    \
         static const char *const texts[] = {\n        NULL,\n",
    );
    for atom in atoms {
        writeln!(out, "        {},", c_string(&atom.text)).unwrap();
    }
    out.push_str("    };\n    static const uint8_t kinds[] = {\n        0,\n");
    for index in 1..=atoms.len() {
        let kind = match atom_kind(index, atoms) {
            "String" => "JS_ATOM_KIND_STRING",
            "Symbol" => "JS_ATOM_KIND_SYMBOL",
            "PrivateSymbol" => "JS_ATOM_KIND_PRIVATE",
            other => unreachable!("{other}"),
        };
        writeln!(out, "        {kind},").unwrap();
    }
    out.push_str(
        "    };\n    \
         JSRuntime *rt = JS_NewRuntime();\n    \
         JSContext *ctx;\n    \
         int i, result = -1;\n\n    \
         if (!rt)\n        return -2;\n    \
         ctx = JS_NewContextRaw(rt);\n    \
         if (!ctx) {\n        JS_FreeRuntime(rt);\n        return -2;\n    }\n    \
         for(i = 1; i < JS_ATOM_END && result < 0; i++) {\n        \
         JSAtomStruct *p = rt->atom_array[i];\n        \
         size_t len = strlen(texts[i]);\n        \
         if (JS_AtomGetKind(ctx, i) != kinds[i] || p->is_wide_char ||\n            \
         p->len != len || memcmp(p->u.str8, texts[i], len) != 0)\n            \
         result = i;\n    \
         }\n    \
         JS_FreeContext(ctx);\n    \
         JS_FreeRuntime(rt);\n    \
         return result;\n}\n",
    );
}
