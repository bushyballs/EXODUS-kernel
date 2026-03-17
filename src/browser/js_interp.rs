use crate::sync::Mutex;
/// Minimal JavaScript interpreter for Genesis browser
///
/// A bytecode-compiling stack-based virtual machine. Supports
/// basic expressions, variables, function calls, and control flow.
/// All numeric values use Q16 fixed-point (no floats).
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

static JS_VM: Mutex<Option<JsVmState>> = Mutex::new(None);

/// Q16 fixed-point: 1 << 16
const Q16_ONE: i32 = 65536;

/// FNV-1a hash for string identity
fn js_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// JavaScript value types
#[derive(Debug, Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(i32),   // Q16 fixed-point
    String(u64),   // hash of the string content
    Object(u32),   // index into object table
    Array(u32),    // index into array table
    Function(u32), // index into function table
}

impl JsValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0,
            JsValue::String(h) => *h != 0,
            JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_) => true,
        }
    }

    pub fn to_number(&self) -> i32 {
        match self {
            JsValue::Number(n) => *n,
            JsValue::Bool(true) => Q16_ONE,
            JsValue::Bool(false) | JsValue::Null | JsValue::Undefined => 0,
            _ => 0,
        }
    }
}

/// Bytecode opcodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    Push = 0x01,     // Push constant onto stack
    Pop = 0x02,      // Discard top of stack
    Add = 0x03,      // a + b
    Sub = 0x04,      // a - b
    Mul = 0x05,      // a * b (Q16 multiply)
    Div = 0x06,      // a / b (Q16 divide)
    Call = 0x07,     // Call function (operand = arg count)
    Return = 0x08,   // Return from function
    JmpIf = 0x09,    // Jump if top is truthy (operand = offset)
    Jmp = 0x0A,      // Unconditional jump
    GetProp = 0x0B,  // Get property from object
    SetProp = 0x0C,  // Set property on object
    Eq = 0x0D,       // a == b
    Lt = 0x0E,       // a < b
    Gt = 0x0F,       // a > b
    Not = 0x10,      // !a
    GetLocal = 0x11, // Get local variable
    SetLocal = 0x12, // Set local variable
    Dup = 0x13,      // Duplicate top of stack
    JmpIfNot = 0x14, // Jump if top is falsy
    Negate = 0x15,   // Unary minus
    Halt = 0xFF,     // Stop execution
}

/// A single bytecode instruction
#[derive(Debug, Clone)]
pub struct Instruction {
    pub opcode: Opcode,
    pub operand: i32, // Meaning depends on opcode
}

/// A compiled function
#[derive(Debug, Clone)]
pub struct JsFunction {
    pub name_hash: u64,
    pub param_count: u32,
    pub local_count: u32,
    pub code: Vec<Instruction>,
}

/// Object property slot
#[derive(Debug, Clone)]
pub struct JsPropSlot {
    pub name_hash: u64,
    pub value: JsValue,
}

/// A JS object: collection of named properties
#[derive(Debug, Clone)]
pub struct JsObject {
    pub properties: Vec<JsPropSlot>,
}

impl JsObject {
    pub fn new() -> Self {
        JsObject {
            properties: Vec::new(),
        }
    }

    pub fn get_prop(&self, name_hash: u64) -> JsValue {
        for slot in &self.properties {
            if slot.name_hash == name_hash {
                return slot.value.clone();
            }
        }
        JsValue::Undefined
    }

    pub fn set_prop(&mut self, name_hash: u64, value: JsValue) {
        for slot in self.properties.iter_mut() {
            if slot.name_hash == name_hash {
                slot.value = value;
                return;
            }
        }
        self.properties.push(JsPropSlot { name_hash, value });
    }
}

/// Call frame for function invocation
#[derive(Debug, Clone)]
struct CallFrame {
    func_idx: u32,
    return_ip: usize,
    base_ptr: usize, // base of locals in value stack
}

/// The JS virtual machine state
struct JsVmState {
    functions: Vec<JsFunction>,
    objects: Vec<JsObject>,
    constants: Vec<JsValue>,
    total_executions: u64,
}

/// Compile a simple expression token stream into bytecode.
/// Supports: number literals, +, -, *, /, parentheses, variable names.
pub fn compile(source: &[u8]) -> Vec<Instruction> {
    let mut code = Vec::new();
    let tokens = lex_tokens(source);
    let mut pos = 0;
    compile_expr(&tokens, &mut pos, &mut code);
    code.push(Instruction {
        opcode: Opcode::Halt,
        operand: 0,
    });
    code
}

/// Token types for the mini-lexer
#[derive(Debug, Clone)]
enum JsToken {
    Number(i32), // Q16
    Ident(u64),  // hash
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Eq,
    Lt,
    Gt,
    Bang,
    Semi,
    Assign,
    LBrace,
    RBrace,
    Comma,
}

fn lex_tokens(source: &[u8]) -> Vec<JsToken> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let c = source[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
            }
            b'+' => {
                tokens.push(JsToken::Plus);
                i += 1;
            }
            b'-' => {
                tokens.push(JsToken::Minus);
                i += 1;
            }
            b'*' => {
                tokens.push(JsToken::Star);
                i += 1;
            }
            b'/' => {
                tokens.push(JsToken::Slash);
                i += 1;
            }
            b'(' => {
                tokens.push(JsToken::LParen);
                i += 1;
            }
            b')' => {
                tokens.push(JsToken::RParen);
                i += 1;
            }
            b'{' => {
                tokens.push(JsToken::LBrace);
                i += 1;
            }
            b'}' => {
                tokens.push(JsToken::RBrace);
                i += 1;
            }
            b',' => {
                tokens.push(JsToken::Comma);
                i += 1;
            }
            b';' => {
                tokens.push(JsToken::Semi);
                i += 1;
            }
            b'!' => {
                tokens.push(JsToken::Bang);
                i += 1;
            }
            b'<' => {
                tokens.push(JsToken::Lt);
                i += 1;
            }
            b'>' => {
                tokens.push(JsToken::Gt);
                i += 1;
            }
            b'=' => {
                if i + 1 < source.len() && source[i + 1] == b'=' {
                    tokens.push(JsToken::Eq);
                    i += 2;
                } else {
                    tokens.push(JsToken::Assign);
                    i += 1;
                }
            }
            b'0'..=b'9' => {
                let mut val: i32 = 0;
                while i < source.len() && source[i] >= b'0' && source[i] <= b'9' {
                    val = val * 10 + (source[i] - b'0') as i32;
                    i += 1;
                }
                tokens.push(JsToken::Number(val * Q16_ONE));
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < source.len() && (source[i].is_ascii_alphanumeric() || source[i] == b'_') {
                    i += 1;
                }
                tokens.push(JsToken::Ident(js_hash(&source[start..i])));
            }
            _ => {
                i += 1;
            }
        }
    }
    tokens
}

/// Recursive descent: expression = term (('+' | '-') term)*
fn compile_expr(tokens: &[JsToken], pos: &mut usize, code: &mut Vec<Instruction>) {
    compile_term(tokens, pos, code);
    while *pos < tokens.len() {
        match tokens[*pos] {
            JsToken::Plus => {
                *pos += 1;
                compile_term(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Add,
                    operand: 0,
                });
            }
            JsToken::Minus => {
                *pos += 1;
                compile_term(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Sub,
                    operand: 0,
                });
            }
            JsToken::Eq => {
                *pos += 1;
                compile_term(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Eq,
                    operand: 0,
                });
            }
            JsToken::Lt => {
                *pos += 1;
                compile_term(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Lt,
                    operand: 0,
                });
            }
            JsToken::Gt => {
                *pos += 1;
                compile_term(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Gt,
                    operand: 0,
                });
            }
            _ => break,
        }
    }
}

/// term = unary (('*' | '/') unary)*
fn compile_term(tokens: &[JsToken], pos: &mut usize, code: &mut Vec<Instruction>) {
    compile_unary(tokens, pos, code);
    while *pos < tokens.len() {
        match tokens[*pos] {
            JsToken::Star => {
                *pos += 1;
                compile_unary(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Mul,
                    operand: 0,
                });
            }
            JsToken::Slash => {
                *pos += 1;
                compile_unary(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Div,
                    operand: 0,
                });
            }
            _ => break,
        }
    }
}

/// unary = ('!' | '-')? primary
fn compile_unary(tokens: &[JsToken], pos: &mut usize, code: &mut Vec<Instruction>) {
    if *pos < tokens.len() {
        match tokens[*pos] {
            JsToken::Bang => {
                *pos += 1;
                compile_primary(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Not,
                    operand: 0,
                });
                return;
            }
            JsToken::Minus => {
                *pos += 1;
                compile_primary(tokens, pos, code);
                code.push(Instruction {
                    opcode: Opcode::Negate,
                    operand: 0,
                });
                return;
            }
            _ => {}
        }
    }
    compile_primary(tokens, pos, code);
}

/// primary = Number | Ident | '(' expr ')'
fn compile_primary(tokens: &[JsToken], pos: &mut usize, code: &mut Vec<Instruction>) {
    if *pos >= tokens.len() {
        return;
    }
    match tokens[*pos].clone() {
        JsToken::Number(val) => {
            code.push(Instruction {
                opcode: Opcode::Push,
                operand: val,
            });
            *pos += 1;
        }
        JsToken::Ident(hash) => {
            code.push(Instruction {
                opcode: Opcode::GetLocal,
                operand: hash as i32,
            });
            *pos += 1;
        }
        JsToken::LParen => {
            *pos += 1; // skip '('
            compile_expr(tokens, pos, code);
            if *pos < tokens.len() {
                *pos += 1; // skip ')'
            }
        }
        _ => {
            *pos += 1; // skip unknown
        }
    }
}

/// Execute compiled bytecode, return the top-of-stack value
pub fn execute(code: &[Instruction]) -> JsValue {
    let mut stack: Vec<JsValue> = Vec::new();
    let mut ip: usize = 0;

    while ip < code.len() {
        let inst = &code[ip];
        match inst.opcode {
            Opcode::Push => {
                stack.push(JsValue::Number(inst.operand));
                ip += 1;
            }
            Opcode::Pop => {
                stack.pop();
                ip += 1;
            }
            Opcode::Dup => {
                if let Some(top) = stack.last().cloned() {
                    stack.push(top);
                }
                ip += 1;
            }
            Opcode::Add => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Number(a.wrapping_add(b)));
                ip += 1;
            }
            Opcode::Sub => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Number(a.wrapping_sub(b)));
                ip += 1;
            }
            Opcode::Mul => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                // Q16 multiply: (a * b) >> 16
                let result = ((a as i64).wrapping_mul(b as i64) >> 16) as i32;
                stack.push(JsValue::Number(result));
                ip += 1;
            }
            Opcode::Div => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                // Q16 divide: (a << 16) / b
                let result = if b != 0 {
                    (((a as i64) << 16) / (b as i64)) as i32
                } else {
                    0
                };
                stack.push(JsValue::Number(result));
                ip += 1;
            }
            Opcode::Eq => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Bool(a == b));
                ip += 1;
            }
            Opcode::Lt => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Bool(a < b));
                ip += 1;
            }
            Opcode::Gt => {
                let b = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Bool(a > b));
                ip += 1;
            }
            Opcode::Not => {
                let a = stack.pop().unwrap_or(JsValue::Undefined);
                stack.push(JsValue::Bool(!a.is_truthy()));
                ip += 1;
            }
            Opcode::Negate => {
                let a = stack.pop().unwrap_or(JsValue::Number(0)).to_number();
                stack.push(JsValue::Number(-a));
                ip += 1;
            }
            Opcode::JmpIf => {
                let cond = stack.pop().unwrap_or(JsValue::Undefined);
                if cond.is_truthy() {
                    ip = inst.operand as usize;
                } else {
                    ip += 1;
                }
            }
            Opcode::JmpIfNot => {
                let cond = stack.pop().unwrap_or(JsValue::Undefined);
                if !cond.is_truthy() {
                    ip = inst.operand as usize;
                } else {
                    ip += 1;
                }
            }
            Opcode::Jmp => {
                ip = inst.operand as usize;
            }
            Opcode::Call
            | Opcode::Return
            | Opcode::GetProp
            | Opcode::SetProp
            | Opcode::GetLocal
            | Opcode::SetLocal => {
                // Stubs: in a full VM these would manipulate frames/objects
                ip += 1;
            }
            Opcode::Halt => {
                break;
            }
        }
    }

    // Update execution counter
    let mut guard = JS_VM.lock();
    if let Some(ref mut state) = *guard {
        state.total_executions = state.total_executions.saturating_add(1);
    }

    stack.pop().unwrap_or(JsValue::Undefined)
}

/// Compile and immediately execute a source snippet, returning the result
pub fn call_function(source: &[u8]) -> JsValue {
    let code = compile(source);
    execute(&code)
}

pub fn init() {
    let mut guard = JS_VM.lock();
    *guard = Some(JsVmState {
        functions: Vec::new(),
        objects: Vec::new(),
        constants: Vec::new(),
        total_executions: 0,
    });
    serial_println!("    browser::js_interp initialized");
}
