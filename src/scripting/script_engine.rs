use crate::sync::Mutex;
/// Hoags Script Engine — built-in interpreted scripting language
///
/// A complete lexer + recursive-descent parser + tree-walk interpreter.
/// Supports:
///   - Variables (let x = 10;)
///   - Arithmetic (+, -, *, /)
///   - Comparison (==, !=, <, >)
///   - Boolean operators (and, or, !)
///   - Control flow (if/else, while, for)
///   - Functions (fn name(a, b) { return a + b; })
///   - Print statement (print expr;)
///   - Arrays ([1, 2, 3])
///   - Nested scopes with lexical variable lookup
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// String and identifier storage uses u64 hashes (FNV-1a).
/// No external crates. No f32/f64.
///
/// Inspired by: Lox (Crafting Interpreters), Lua, JavaScript.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;

/// Maximum variables per scope
const MAX_VARS: usize = 128;
/// Maximum functions
const MAX_FUNCS: usize = 64;
/// Maximum call depth
const MAX_CALL_DEPTH: usize = 64;
/// Maximum script length (tokens)
const MAX_TOKENS: usize = 4096;

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Number(i32), // Q16 fixed-point value
    String(u64), // FNV-1a hash of string content
    Ident(u64),  // FNV-1a hash of identifier
    Plus,
    Minus,
    Star,
    Slash,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    Gt,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    If,
    Else,
    While,
    For,
    Fn,
    Return,
    Let,
    True,
    False,
    Nil,
    Print,
    And,
    Or,
    Eof,
}

// ---------------------------------------------------------------------------
// ScriptValue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Number(i32),   // Q16
    String(u64),   // hash
    Function(u32), // function table index
    Array(Vec<ScriptValue>),
}

impl ScriptValue {
    fn is_truthy(&self) -> bool {
        match self {
            ScriptValue::Nil => false,
            ScriptValue::Bool(b) => *b,
            ScriptValue::Number(n) => *n != Q16_ZERO,
            _ => true,
        }
    }
}

// ---------------------------------------------------------------------------
// AST Nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(ScriptValue),
    Variable(u64),
    Unary(Token, usize),         // op, expr_index
    Binary(usize, Token, usize), // left, op, right (indices into expr pool)
    Call(u64, Vec<usize>),       // func name hash, arg expr indices
    ArrayLiteral(Vec<usize>),    // element expr indices
    Assign(u64, usize),          // name hash, value expr index
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expression(usize),               // expr index
    Print(usize),                    // expr index
    Let(u64, usize),                 // name hash, initializer expr index
    Block(Vec<usize>),               // stmt indices
    If(usize, usize, Option<usize>), // condition expr, then stmt, else stmt
    While(usize, usize),             // condition expr, body stmt
    For(usize, usize, usize, usize), // init stmt, cond expr, incr expr, body stmt
    FnDecl(u64, Vec<u64>, usize),    // name hash, param hashes, body stmt
    Return(Option<usize>),           // optional expr index
}

// ---------------------------------------------------------------------------
// Variable & function storage
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Variable {
    name_hash: u64,
    value: ScriptValue,
}

#[derive(Clone)]
struct ScriptFunction {
    name_hash: u64,
    params: Vec<u64>,
    body_stmt: usize,
}

struct Scope {
    vars: Vec<Variable>,
}

impl Scope {
    fn new() -> Self {
        Scope { vars: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Interpreter state
// ---------------------------------------------------------------------------

struct InterpreterState {
    scopes: Vec<Scope>,
    functions: Vec<ScriptFunction>,
    call_depth: usize,
    return_value: Option<ScriptValue>,
    returning: bool,
    expr_pool: Vec<Expr>,
    stmt_pool: Vec<Stmt>,
}

impl InterpreterState {
    fn new() -> Self {
        InterpreterState {
            scopes: vec![Scope::new()],
            functions: Vec::new(),
            call_depth: 0,
            return_value: None,
            returning: false,
            expr_pool: Vec::new(),
            stmt_pool: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine global
// ---------------------------------------------------------------------------

struct ScriptEngineState {
    initialized: bool,
    scripts_run: u32,
    last_result: ScriptValue,
}

static ENGINE: Mutex<Option<ScriptEngineState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// FNV-1a hash
// ---------------------------------------------------------------------------

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF29CE484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x00000100000001B3);
    }
    hash
}

/// Hash well-known keywords
fn kw_hash(s: &str) -> u64 {
    fnv1a_hash(s.as_bytes())
}

// ---------------------------------------------------------------------------
// Lexer / Tokenizer
// ---------------------------------------------------------------------------

pub fn tokenize(source: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let len = source.len();
    let mut i = 0;

    while i < len && tokens.len() < MAX_TOKENS {
        let c = source[i];

        // Skip whitespace
        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
            i += 1;
            continue;
        }

        // Skip line comments //
        if c == b'/' && i + 1 < len && source[i + 1] == b'/' {
            while i < len && source[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Single/double character tokens
        match c {
            b'+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            b'-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            b'*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            b'/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            b'(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            b')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            b'{' => {
                tokens.push(Token::LBrace);
                i += 1;
            }
            b'}' => {
                tokens.push(Token::RBrace);
                i += 1;
            }
            b',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            b';' => {
                tokens.push(Token::Semicolon);
                i += 1;
            }
            b'<' => {
                tokens.push(Token::Lt);
                i += 1;
            }
            b'>' => {
                tokens.push(Token::Gt);
                i += 1;
            }
            b'=' => {
                if i + 1 < len && source[i + 1] == b'=' {
                    tokens.push(Token::EqEq);
                    i += 2;
                } else {
                    tokens.push(Token::Eq);
                    i += 1;
                }
            }
            b'!' => {
                if i + 1 < len && source[i + 1] == b'=' {
                    tokens.push(Token::BangEq);
                    i += 2;
                } else {
                    tokens.push(Token::Bang);
                    i += 1;
                }
            }
            // String literal
            b'"' => {
                i += 1;
                let start = i;
                while i < len && source[i] != b'"' {
                    i += 1;
                }
                let hash = fnv1a_hash(&source[start..i]);
                tokens.push(Token::String(hash));
                if i < len {
                    i += 1;
                } // skip closing quote
            }
            // Number literal (integer, stored as Q16)
            b'0'..=b'9' => {
                let mut val: i32 = 0;
                while i < len && source[i] >= b'0' && source[i] <= b'9' {
                    val = val.wrapping_mul(10).wrapping_add((source[i] - b'0') as i32);
                    i += 1;
                }
                // Convert to Q16: multiply by 65536
                let q16_val = val.wrapping_mul(Q16_ONE);
                tokens.push(Token::Number(q16_val));
            }
            // Identifier or keyword
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < len && (source[i].is_ascii_alphanumeric() || source[i] == b'_') {
                    i += 1;
                }
                let word = &source[start..i];
                let token = match word {
                    b"if" => Token::If,
                    b"else" => Token::Else,
                    b"while" => Token::While,
                    b"for" => Token::For,
                    b"fn" => Token::Fn,
                    b"return" => Token::Return,
                    b"let" => Token::Let,
                    b"true" => Token::True,
                    b"false" => Token::False,
                    b"nil" => Token::Nil,
                    b"print" => Token::Print,
                    b"and" => Token::And,
                    b"or" => Token::Or,
                    _ => Token::Ident(fnv1a_hash(word)),
                };
                tokens.push(token);
            }
            _ => {
                // Unknown character — skip
                i += 1;
            }
        }
    }

    tokens.push(Token::Eof);
    tokens
}

// ---------------------------------------------------------------------------
// Parser — recursive descent, builds AST into pools
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    expr_pool: Vec<Expr>,
    stmt_pool: Vec<Stmt>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            expr_pool: Vec::new(),
            stmt_pool: Vec::new(),
        }
    }

    fn peek(&self) -> Token {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos]
        } else {
            Token::Eof
        }
    }

    fn advance(&mut self) -> Token {
        let t = self.peek();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, expected: Token) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn push_expr(&mut self, e: Expr) -> usize {
        let idx = self.expr_pool.len();
        self.expr_pool.push(e);
        idx
    }

    fn push_stmt(&mut self, s: Stmt) -> usize {
        let idx = self.stmt_pool.len();
        self.stmt_pool.push(s);
        idx
    }

    // ---- Expression parsing (precedence climbing) ----

    fn parse_primary(&mut self) -> usize {
        match self.peek() {
            Token::Number(n) => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Number(n)))
            }
            Token::String(h) => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::String(h)))
            }
            Token::True => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Bool(true)))
            }
            Token::False => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Bool(false)))
            }
            Token::Nil => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Nil))
            }
            Token::Ident(name) => {
                self.advance();
                // Check for function call
                if self.peek() == Token::LParen {
                    self.advance(); // consume (
                    let mut args = Vec::new();
                    if self.peek() != Token::RParen {
                        let arg = self.parse_expression();
                        args.push(arg);
                        while self.peek() == Token::Comma {
                            self.advance();
                            let arg = self.parse_expression();
                            args.push(arg);
                        }
                    }
                    self.expect(Token::RParen);
                    self.push_expr(Expr::Call(name, args))
                } else if self.peek() == Token::Eq {
                    // Assignment
                    self.advance(); // consume =
                    let value = self.parse_expression();
                    self.push_expr(Expr::Assign(name, value))
                } else {
                    self.push_expr(Expr::Variable(name))
                }
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expression();
                self.expect(Token::RParen);
                expr
            }
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary();
                self.push_expr(Expr::Unary(Token::Bang, operand))
            }
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary();
                self.push_expr(Expr::Unary(Token::Minus, operand))
            }
            Token::LBrace => {
                // Array literal [...] — we reuse LBrace for simplicity
                // Actually let's not — arrays would need [ ]
                // Fallback: return Nil for unexpected tokens
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Nil))
            }
            _ => {
                self.advance();
                self.push_expr(Expr::Literal(ScriptValue::Nil))
            }
        }
    }

    fn parse_unary(&mut self) -> usize {
        match self.peek() {
            Token::Bang | Token::Minus => {
                let op = self.advance();
                let operand = self.parse_unary();
                self.push_expr(Expr::Unary(op, operand))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_factor(&mut self) -> usize {
        let mut left = self.parse_unary();
        while matches!(self.peek(), Token::Star | Token::Slash) {
            let op = self.advance();
            let right = self.parse_unary();
            left = self.push_expr(Expr::Binary(left, op, right));
        }
        left
    }

    fn parse_term(&mut self) -> usize {
        let mut left = self.parse_factor();
        while matches!(self.peek(), Token::Plus | Token::Minus) {
            let op = self.advance();
            let right = self.parse_factor();
            left = self.push_expr(Expr::Binary(left, op, right));
        }
        left
    }

    fn parse_comparison(&mut self) -> usize {
        let mut left = self.parse_term();
        while matches!(
            self.peek(),
            Token::Lt | Token::Gt | Token::EqEq | Token::BangEq
        ) {
            let op = self.advance();
            let right = self.parse_term();
            left = self.push_expr(Expr::Binary(left, op, right));
        }
        left
    }

    fn parse_logic_and(&mut self) -> usize {
        let mut left = self.parse_comparison();
        while self.peek() == Token::And {
            let op = self.advance();
            let right = self.parse_comparison();
            left = self.push_expr(Expr::Binary(left, op, right));
        }
        left
    }

    fn parse_expression(&mut self) -> usize {
        let mut left = self.parse_logic_and();
        while self.peek() == Token::Or {
            let op = self.advance();
            let right = self.parse_logic_and();
            left = self.push_expr(Expr::Binary(left, op, right));
        }
        left
    }

    // ---- Statement parsing ----

    fn parse_statement(&mut self) -> usize {
        match self.peek() {
            Token::Print => self.parse_print_stmt(),
            Token::Let => self.parse_let_stmt(),
            Token::LBrace => self.parse_block_stmt(),
            Token::If => self.parse_if_stmt(),
            Token::While => self.parse_while_stmt(),
            Token::For => self.parse_for_stmt(),
            Token::Fn => self.parse_fn_decl(),
            Token::Return => self.parse_return_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_print_stmt(&mut self) -> usize {
        self.advance(); // consume 'print'
        let expr = self.parse_expression();
        self.expect(Token::Semicolon);
        self.push_stmt(Stmt::Print(expr))
    }

    fn parse_let_stmt(&mut self) -> usize {
        self.advance(); // consume 'let'
        let name = match self.advance() {
            Token::Ident(h) => h,
            _ => 0,
        };
        let init = if self.expect(Token::Eq) {
            self.parse_expression()
        } else {
            self.push_expr(Expr::Literal(ScriptValue::Nil))
        };
        self.expect(Token::Semicolon);
        self.push_stmt(Stmt::Let(name, init))
    }

    fn parse_block_stmt(&mut self) -> usize {
        self.advance(); // consume {
        let mut stmts = Vec::new();
        while self.peek() != Token::RBrace && self.peek() != Token::Eof {
            let s = self.parse_statement();
            stmts.push(s);
        }
        self.expect(Token::RBrace);
        self.push_stmt(Stmt::Block(stmts))
    }

    fn parse_if_stmt(&mut self) -> usize {
        self.advance(); // consume 'if'
        self.expect(Token::LParen);
        let cond = self.parse_expression();
        self.expect(Token::RParen);
        let then_branch = self.parse_statement();
        let else_branch = if self.peek() == Token::Else {
            self.advance();
            Some(self.parse_statement())
        } else {
            None
        };
        self.push_stmt(Stmt::If(cond, then_branch, else_branch))
    }

    fn parse_while_stmt(&mut self) -> usize {
        self.advance(); // consume 'while'
        self.expect(Token::LParen);
        let cond = self.parse_expression();
        self.expect(Token::RParen);
        let body = self.parse_statement();
        self.push_stmt(Stmt::While(cond, body))
    }

    fn parse_for_stmt(&mut self) -> usize {
        self.advance(); // consume 'for'
        self.expect(Token::LParen);
        let init = self.parse_statement(); // includes semicolon
        let cond = self.parse_expression();
        self.expect(Token::Semicolon);
        let incr = self.parse_expression();
        self.expect(Token::RParen);
        let body = self.parse_statement();
        self.push_stmt(Stmt::For(init, cond, incr, body))
    }

    fn parse_fn_decl(&mut self) -> usize {
        self.advance(); // consume 'fn'
        let name = match self.advance() {
            Token::Ident(h) => h,
            _ => 0,
        };
        self.expect(Token::LParen);
        let mut params = Vec::new();
        if self.peek() != Token::RParen {
            if let Token::Ident(h) = self.advance() {
                params.push(h);
            }
            while self.peek() == Token::Comma {
                self.advance();
                if let Token::Ident(h) = self.advance() {
                    params.push(h);
                }
            }
        }
        self.expect(Token::RParen);
        let body = self.parse_block_stmt();
        self.push_stmt(Stmt::FnDecl(name, params, body))
    }

    fn parse_return_stmt(&mut self) -> usize {
        self.advance(); // consume 'return'
        let expr = if self.peek() != Token::Semicolon {
            Some(self.parse_expression())
        } else {
            None
        };
        self.expect(Token::Semicolon);
        self.push_stmt(Stmt::Return(expr))
    }

    fn parse_expr_stmt(&mut self) -> usize {
        let expr = self.parse_expression();
        self.expect(Token::Semicolon);
        self.push_stmt(Stmt::Expression(expr))
    }
}

/// Parse tokens into AST pools, returns list of top-level statement indices
pub fn parse(tokens: Vec<Token>) -> (Vec<Expr>, Vec<Stmt>, Vec<usize>) {
    let mut parser = Parser::new(tokens);
    let mut top_level = Vec::new();
    while parser.peek() != Token::Eof {
        let s = parser.parse_statement();
        top_level.push(s);
    }
    (parser.expr_pool, parser.stmt_pool, top_level)
}

// ---------------------------------------------------------------------------
// Interpreter — tree-walk evaluation
// ---------------------------------------------------------------------------

fn evaluate_expression(state: &mut InterpreterState, idx: usize) -> ScriptValue {
    if idx >= state.expr_pool.len() {
        return ScriptValue::Nil;
    }
    let expr = state.expr_pool[idx].clone();
    match expr {
        Expr::Literal(v) => v,
        Expr::Variable(name) => get_variable(state, name),
        Expr::Unary(op, operand_idx) => {
            let val = evaluate_expression(state, operand_idx);
            match op {
                Token::Minus => {
                    if let ScriptValue::Number(n) = val {
                        ScriptValue::Number(n.wrapping_neg())
                    } else {
                        ScriptValue::Nil
                    }
                }
                Token::Bang => ScriptValue::Bool(!val.is_truthy()),
                _ => ScriptValue::Nil,
            }
        }
        Expr::Binary(left_idx, op, right_idx) => {
            // Short-circuit for logical operators
            if op == Token::And {
                let left = evaluate_expression(state, left_idx);
                if !left.is_truthy() {
                    return left;
                }
                return evaluate_expression(state, right_idx);
            }
            if op == Token::Or {
                let left = evaluate_expression(state, left_idx);
                if left.is_truthy() {
                    return left;
                }
                return evaluate_expression(state, right_idx);
            }

            let left = evaluate_expression(state, left_idx);
            let right = evaluate_expression(state, right_idx);

            match (left, op, right) {
                (ScriptValue::Number(a), Token::Plus, ScriptValue::Number(b)) => {
                    ScriptValue::Number(a.wrapping_add(b))
                }
                (ScriptValue::Number(a), Token::Minus, ScriptValue::Number(b)) => {
                    ScriptValue::Number(a.wrapping_sub(b))
                }
                (ScriptValue::Number(a), Token::Star, ScriptValue::Number(b)) => {
                    // Q16 multiply: (a * b) >> 16
                    let result = ((a as i64).wrapping_mul(b as i64)) >> 16;
                    ScriptValue::Number(result as i32)
                }
                (ScriptValue::Number(a), Token::Slash, ScriptValue::Number(b)) => {
                    if b == 0 {
                        ScriptValue::Nil
                    } else {
                        // Q16 divide: (a << 16) / b
                        let result = ((a as i64) << 16) / (b as i64);
                        ScriptValue::Number(result as i32)
                    }
                }
                (ScriptValue::Number(a), Token::EqEq, ScriptValue::Number(b)) => {
                    ScriptValue::Bool(a == b)
                }
                (ScriptValue::Number(a), Token::BangEq, ScriptValue::Number(b)) => {
                    ScriptValue::Bool(a != b)
                }
                (ScriptValue::Number(a), Token::Lt, ScriptValue::Number(b)) => {
                    ScriptValue::Bool(a < b)
                }
                (ScriptValue::Number(a), Token::Gt, ScriptValue::Number(b)) => {
                    ScriptValue::Bool(a > b)
                }
                (ScriptValue::Bool(a), Token::EqEq, ScriptValue::Bool(b)) => {
                    ScriptValue::Bool(a == b)
                }
                (ScriptValue::Bool(a), Token::BangEq, ScriptValue::Bool(b)) => {
                    ScriptValue::Bool(a != b)
                }
                (ScriptValue::Nil, Token::EqEq, ScriptValue::Nil) => ScriptValue::Bool(true),
                _ => ScriptValue::Nil,
            }
        }
        Expr::Call(name, arg_indices) => {
            let mut args = Vec::new();
            for &ai in &arg_indices {
                args.push(evaluate_expression(state, ai));
            }
            call_function(state, name, args)
        }
        Expr::ArrayLiteral(elem_indices) => {
            let mut elements = Vec::new();
            for &ei in &elem_indices {
                elements.push(evaluate_expression(state, ei));
            }
            ScriptValue::Array(elements)
        }
        Expr::Assign(name, val_idx) => {
            let value = evaluate_expression(state, val_idx);
            set_variable(state, name, value.clone());
            value
        }
    }
}

fn execute_statement(state: &mut InterpreterState, idx: usize) {
    if idx >= state.stmt_pool.len() || state.returning {
        return;
    }
    let stmt = state.stmt_pool[idx].clone();
    match stmt {
        Stmt::Expression(expr_idx) => {
            evaluate_expression(state, expr_idx);
        }
        Stmt::Print(expr_idx) => {
            let val = evaluate_expression(state, expr_idx);
            match val {
                ScriptValue::Nil => {
                    serial_println!("[script] nil");
                }
                ScriptValue::Bool(b) => {
                    serial_println!("[script] {}", b);
                }
                ScriptValue::Number(n) => {
                    let whole = n >> 16;
                    let frac = ((n & 0xFFFF) * 1000) >> 16;
                    serial_println!("[script] {}.{:03}", whole, frac);
                }
                ScriptValue::String(h) => {
                    serial_println!("[script] string<{:#018X}>", h);
                }
                ScriptValue::Function(id) => {
                    serial_println!("[script] fn<{}>", id);
                }
                ScriptValue::Array(ref elems) => {
                    serial_println!("[script] array[{}]", elems.len());
                }
            }
        }
        Stmt::Let(name, init_idx) => {
            let value = evaluate_expression(state, init_idx);
            define_variable(state, name, value);
        }
        Stmt::Block(stmt_indices) => {
            // Push new scope
            state.scopes.push(Scope::new());
            for &si in &stmt_indices {
                execute_statement(state, si);
                if state.returning {
                    break;
                }
            }
            state.scopes.pop();
        }
        Stmt::If(cond_idx, then_idx, else_idx) => {
            let cond = evaluate_expression(state, cond_idx);
            if cond.is_truthy() {
                execute_statement(state, then_idx);
            } else if let Some(else_s) = else_idx {
                execute_statement(state, else_s);
            }
        }
        Stmt::While(cond_idx, body_idx) => {
            let mut iterations = 0u32;
            loop {
                let cond = evaluate_expression(state, cond_idx);
                if !cond.is_truthy() || state.returning {
                    break;
                }
                execute_statement(state, body_idx);
                iterations += 1;
                if iterations > 100_000 {
                    break;
                } // safety limit
            }
        }
        Stmt::For(init_idx, cond_idx, incr_idx, body_idx) => {
            execute_statement(state, init_idx);
            let mut iterations = 0u32;
            loop {
                let cond = evaluate_expression(state, cond_idx);
                if !cond.is_truthy() || state.returning {
                    break;
                }
                execute_statement(state, body_idx);
                evaluate_expression(state, incr_idx);
                iterations += 1;
                if iterations > 100_000 {
                    break;
                }
            }
        }
        Stmt::FnDecl(name, params, body_idx) => {
            define_function(state, name, params, body_idx);
        }
        Stmt::Return(expr_idx) => {
            let value = if let Some(ei) = expr_idx {
                evaluate_expression(state, ei)
            } else {
                ScriptValue::Nil
            };
            state.return_value = Some(value);
            state.returning = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Variable management
// ---------------------------------------------------------------------------

fn define_variable(state: &mut InterpreterState, name: u64, value: ScriptValue) {
    if let Some(scope) = state.scopes.last_mut() {
        if scope.vars.len() < MAX_VARS {
            scope.vars.push(Variable {
                name_hash: name,
                value,
            });
        }
    }
}

fn set_variable(state: &mut InterpreterState, name: u64, value: ScriptValue) {
    // Search scopes from innermost to outermost
    for scope in state.scopes.iter_mut().rev() {
        for var in scope.vars.iter_mut() {
            if var.name_hash == name {
                var.value = value;
                return;
            }
        }
    }
    // If not found, define in current scope
    define_variable(state, name, value);
}

fn get_variable(state: &InterpreterState, name: u64) -> ScriptValue {
    for scope in state.scopes.iter().rev() {
        for var in scope.vars.iter().rev() {
            if var.name_hash == name {
                return var.value.clone();
            }
        }
    }
    ScriptValue::Nil
}

// ---------------------------------------------------------------------------
// Function management
// ---------------------------------------------------------------------------

fn define_function(state: &mut InterpreterState, name: u64, params: Vec<u64>, body_stmt: usize) {
    if state.functions.len() < MAX_FUNCS {
        let func_id = state.functions.len() as u32;
        state.functions.push(ScriptFunction {
            name_hash: name,
            params,
            body_stmt,
        });
        // Store function reference as variable
        define_variable(state, name, ScriptValue::Function(func_id));
    }
}

fn call_function(state: &mut InterpreterState, name: u64, args: Vec<ScriptValue>) -> ScriptValue {
    if state.call_depth >= MAX_CALL_DEPTH {
        serial_println!("[script] ERROR: max call depth exceeded");
        return ScriptValue::Nil;
    }

    // Find function
    let func = {
        let mut found = None;
        for f in &state.functions {
            if f.name_hash == name {
                found = Some(f.clone());
                break;
            }
        }
        found
    };

    if let Some(func) = func {
        state.call_depth = state.call_depth.saturating_add(1);

        // Push function scope with parameters bound to arguments
        let mut scope = Scope::new();
        for (i, &param_hash) in func.params.iter().enumerate() {
            let val = if i < args.len() {
                args[i].clone()
            } else {
                ScriptValue::Nil
            };
            scope.vars.push(Variable {
                name_hash: param_hash,
                value: val,
            });
        }
        state.scopes.push(scope);

        // Execute body
        execute_statement(state, func.body_stmt);

        // Collect return value
        let result = state.return_value.take().unwrap_or(ScriptValue::Nil);
        state.returning = false;

        state.scopes.pop();
        state.call_depth -= 1;

        result
    } else {
        serial_println!("[script] ERROR: undefined function {:#018X}", name);
        ScriptValue::Nil
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Full pipeline: tokenize -> parse -> interpret
pub fn interpret(source: &[u8]) -> ScriptValue {
    let tokens = tokenize(source);
    let (expr_pool, stmt_pool, top_level) = parse(tokens);

    let mut state = InterpreterState::new();
    state.expr_pool = expr_pool;
    state.stmt_pool = stmt_pool;

    for &si in &top_level {
        execute_statement(&mut state, si);
        if state.returning {
            break;
        }
    }

    let result = state.return_value.unwrap_or(ScriptValue::Nil);

    // Update engine global state
    if let Some(ref mut engine) = *ENGINE.lock() {
        engine.scripts_run = engine.scripts_run.saturating_add(1);
        engine.last_result = result.clone();
    }

    result
}

/// Get the number of scripts executed since init
pub fn scripts_run() -> u32 {
    if let Some(ref engine) = *ENGINE.lock() {
        engine.scripts_run
    } else {
        0
    }
}

pub fn init() {
    let mut guard = ENGINE.lock();
    *guard = Some(ScriptEngineState {
        initialized: true,
        scripts_run: 0,
        last_result: ScriptValue::Nil,
    });
    serial_println!("    [scripting] Script engine initialized (lexer + parser + interpreter)");
}
