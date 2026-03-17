/// SQL-like query parser and executor for Genesis embedded database
///
/// Supports a subset of SQL:
///   - SELECT columns FROM table [WHERE cond] [ORDER BY col [ASC|DESC]] [LIMIT n]
///   - INSERT INTO table (cols) VALUES (vals)
///   - UPDATE table SET col=val [WHERE cond]
///   - DELETE FROM table [WHERE cond]
///
/// WHERE supports: =, !=, <, >, <=, >=, AND, OR, IS NULL, IS NOT NULL
/// No floats — integer literals only. Text literals in single quotes.
///
/// Inspired by: SQLite query compiler, MySQL parser. All code is original.
use crate::{serial_print, serial_println};

use super::table::{ColumnType, Row, Schema, Value};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constant
const Q16_ONE: i32 = 65536;

/// Token types produced by the lexer
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    // Keywords
    Select,
    Insert,
    Update,
    Delete,
    From,
    Into,
    Set,
    Where,
    And,
    Or,
    OrderBy,
    Asc,
    Desc,
    Limit,
    Values,
    Null,
    IsNull,
    IsNotNull,
    Create,
    Table,
    Drop,
    // Operators
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Comma,
    LParen,
    RParen,
    Star,
    Assign,
    // Literals
    Ident(String),
    IntLit(i64),
    TextLit(String),
    BoolLit(bool),
    // End
    Eof,
}

/// Comparison operators for WHERE clauses
#[derive(Clone, Debug)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

/// A single WHERE condition
#[derive(Clone, Debug)]
pub struct Condition {
    pub column: String,
    pub op: CmpOp,
    pub value: Value,
}

/// Logical connective between conditions
#[derive(Clone, Debug)]
pub enum WhereClause {
    Single(Condition),
    And(Vec<WhereClause>),
    Or(Vec<WhereClause>),
    IsNull(String),
    IsNotNull(String),
}

/// Sort direction
#[derive(Clone, Debug)]
pub enum SortDir {
    Asc,
    Desc,
}

/// ORDER BY clause
#[derive(Clone, Debug)]
pub struct OrderByClause {
    pub column: String,
    pub direction: SortDir,
}

/// Parsed query representation
#[derive(Clone, Debug)]
pub enum Query {
    Select {
        columns: Vec<String>, // empty = "*"
        table: String,
        where_clause: Option<WhereClause>,
        order_by: Option<OrderByClause>,
        limit: Option<usize>,
    },
    Insert {
        table: String,
        columns: Vec<String>,
        values: Vec<Value>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Value)>,
        where_clause: Option<WhereClause>,
    },
    Delete {
        table: String,
        where_clause: Option<WhereClause>,
    },
    CreateTable {
        table: String,
        columns: Vec<(String, ColumnType, bool)>, // name, type, nullable
    },
    DropTable {
        table: String,
    },
}

/// Query result
#[derive(Clone, Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub affected_rows: u64,
    pub last_insert_id: Option<i64>,
}

impl QueryResult {
    fn empty() -> Self {
        QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
        }
    }

    fn with_affected(count: u64) -> Self {
        QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            affected_rows: count,
            last_insert_id: None,
        }
    }
}

/// Parse error
#[derive(Debug)]
pub enum ParseError {
    UnexpectedToken,
    UnexpectedEof,
    InvalidSyntax(String),
    UnknownColumn(String),
    ExecutionError(String),
}

/// Tokenize a query string into tokens
fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        // Single-char operators
        match chars[i] {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
                continue;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
                continue;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
                continue;
            }
            _ => {}
        }

        // Two-char operators
        if i + 1 < chars.len() {
            match (chars[i], chars[i + 1]) {
                ('!', '=') => {
                    tokens.push(Token::NotEq);
                    i += 2;
                    continue;
                }
                ('<', '=') => {
                    tokens.push(Token::LtEq);
                    i += 2;
                    continue;
                }
                ('>', '=') => {
                    tokens.push(Token::GtEq);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Single-char operators that might be part of two-char ones
        match chars[i] {
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
                continue;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
                continue;
            }
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
                continue;
            }
            _ => {}
        }

        // String literal (single quotes)
        if chars[i] == '\'' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(ParseError::InvalidSyntax(String::from(
                    "unterminated string",
                )));
            }
            let s: String = chars[start..i].iter().collect();
            tokens.push(Token::TextLit(s));
            i += 1; // skip closing quote
            continue;
        }

        // Number literal (optionally negative)
        if chars[i].is_ascii_digit()
            || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            if chars[i] == '-' {
                i += 1;
            }
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            let val = parse_i64(&s).ok_or(ParseError::InvalidSyntax(String::from("bad number")))?;
            tokens.push(Token::IntLit(val));
            continue;
        }

        // Identifier or keyword
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = to_upper(&word);

            let tok = match upper.as_str() {
                "SELECT" => Token::Select,
                "INSERT" => Token::Insert,
                "UPDATE" => Token::Update,
                "DELETE" => Token::Delete,
                "FROM" => Token::From,
                "INTO" => Token::Into,
                "SET" => Token::Set,
                "WHERE" => Token::Where,
                "AND" => Token::And,
                "OR" => Token::Or,
                "ORDER" => {
                    // Look ahead for "BY"
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    if i + 1 < chars.len() {
                        let next_start = i;
                        while i < chars.len() && chars[i].is_alphabetic() {
                            i += 1;
                        }
                        let next_word: String = chars[next_start..i].iter().collect();
                        if to_upper(&next_word) == "BY" {
                            Token::OrderBy
                        } else {
                            return Err(ParseError::InvalidSyntax(String::from(
                                "expected BY after ORDER",
                            )));
                        }
                    } else {
                        return Err(ParseError::UnexpectedEof);
                    }
                }
                "ASC" => Token::Asc,
                "DESC" => Token::Desc,
                "LIMIT" => Token::Limit,
                "VALUES" => Token::Values,
                "NULL" => Token::Null,
                "IS" => {
                    // Look ahead for NULL or NOT NULL
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    let next_start = i;
                    while i < chars.len() && chars[i].is_alphabetic() {
                        i += 1;
                    }
                    let next_word: String = chars[next_start..i].iter().collect();
                    if to_upper(&next_word) == "NULL" {
                        Token::IsNull
                    } else if to_upper(&next_word) == "NOT" {
                        while i < chars.len() && chars[i].is_whitespace() {
                            i += 1;
                        }
                        let nn_start = i;
                        while i < chars.len() && chars[i].is_alphabetic() {
                            i += 1;
                        }
                        let nn_word: String = chars[nn_start..i].iter().collect();
                        if to_upper(&nn_word) == "NULL" {
                            Token::IsNotNull
                        } else {
                            return Err(ParseError::InvalidSyntax(String::from(
                                "expected NULL after NOT",
                            )));
                        }
                    } else {
                        return Err(ParseError::InvalidSyntax(String::from(
                            "expected NULL or NOT NULL after IS",
                        )));
                    }
                }
                "TRUE" => Token::BoolLit(true),
                "FALSE" => Token::BoolLit(false),
                "CREATE" => Token::Create,
                "TABLE" => Token::Table,
                "DROP" => Token::Drop,
                "INT" | "INTEGER" => Token::Ident(String::from("INT")),
                "TEXT" | "VARCHAR" => Token::Ident(String::from("TEXT")),
                "BOOL" | "BOOLEAN" => Token::Ident(String::from("BOOL")),
                "BLOB" => Token::Ident(String::from("BLOB")),
                _ => Token::Ident(word),
            };
            tokens.push(tok);
            continue;
        }

        return Err(ParseError::InvalidSyntax(String::from(
            "unexpected character",
        )));
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

/// Simple i64 parser (no floats)
fn parse_i64(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut neg = false;
    let mut start = 0;
    if bytes[0] == b'-' {
        neg = true;
        start = 1;
    }
    let mut val: i64 = 0;
    for &b in &bytes[start..] {
        if b < b'0' || b > b'9' {
            return None;
        }
        val = val.wrapping_mul(10).wrapping_add((b - b'0') as i64);
    }
    if neg {
        val = -val;
    }
    Some(val)
}

/// Uppercase conversion without std
fn to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c >= 'a' && c <= 'z' {
                (c as u8 - 32) as char
            } else {
                c
            }
        })
        .collect()
}

/// Parser state
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            &Token::Eof
        }
    }

    fn advance(&mut self) -> Token {
        let tok = if self.pos < self.tokens.len() {
            self.tokens[self.pos].clone()
        } else {
            Token::Eof
        };
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ParseError> {
        let tok = self.advance();
        if core::mem::discriminant(&tok) == core::mem::discriminant(expected) {
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken)
        }
    }

    fn parse_query(&mut self) -> Result<Query, ParseError> {
        match self.peek().clone() {
            Token::Select => self.parse_select(),
            Token::Insert => self.parse_insert(),
            Token::Update => self.parse_update(),
            Token::Delete => self.parse_delete(),
            Token::Create => self.parse_create_table(),
            Token::Drop => self.parse_drop_table(),
            _ => Err(ParseError::UnexpectedToken),
        }
    }

    fn parse_select(&mut self) -> Result<Query, ParseError> {
        self.advance(); // SELECT
        let columns = self.parse_column_list()?;
        self.expect(&Token::From)?;
        let table = self.parse_ident()?;
        let where_clause = self.parse_optional_where()?;
        let order_by = self.parse_optional_order_by()?;
        let limit = self.parse_optional_limit()?;
        Ok(Query::Select {
            columns,
            table,
            where_clause,
            order_by,
            limit,
        })
    }

    fn parse_insert(&mut self) -> Result<Query, ParseError> {
        self.advance(); // INSERT
        self.expect(&Token::Into)?;
        let table = self.parse_ident()?;
        self.expect(&Token::LParen)?;
        let columns = self.parse_ident_list()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::Values)?;
        self.expect(&Token::LParen)?;
        let values = self.parse_value_list()?;
        self.expect(&Token::RParen)?;
        Ok(Query::Insert {
            table,
            columns,
            values,
        })
    }

    fn parse_update(&mut self) -> Result<Query, ParseError> {
        self.advance(); // UPDATE
        let table = self.parse_ident()?;
        self.expect(&Token::Set)?;
        let assignments = self.parse_assignments()?;
        let where_clause = self.parse_optional_where()?;
        Ok(Query::Update {
            table,
            assignments,
            where_clause,
        })
    }

    fn parse_delete(&mut self) -> Result<Query, ParseError> {
        self.advance(); // DELETE
        self.expect(&Token::From)?;
        let table = self.parse_ident()?;
        let where_clause = self.parse_optional_where()?;
        Ok(Query::Delete {
            table,
            where_clause,
        })
    }

    fn parse_create_table(&mut self) -> Result<Query, ParseError> {
        self.advance(); // CREATE
        self.expect(&Token::Table)?;
        let table = self.parse_ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            let col_name = self.parse_ident()?;
            let type_name = self.parse_ident()?;
            let col_type = match type_name.as_str() {
                "INT" | "INTEGER" => ColumnType::Int,
                "TEXT" | "VARCHAR" => ColumnType::Text,
                "BOOL" | "BOOLEAN" => ColumnType::Bool,
                "BLOB" => ColumnType::Blob,
                _ => return Err(ParseError::InvalidSyntax(String::from("unknown type"))),
            };
            // Check for nullable (default true, unless NOT NULL specified)
            let nullable = true; // simplified — always nullable for now
            columns.push((col_name, col_type, nullable));
            if *self.peek() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Ok(Query::CreateTable { table, columns })
    }

    fn parse_drop_table(&mut self) -> Result<Query, ParseError> {
        self.advance(); // DROP
        self.expect(&Token::Table)?;
        let table = self.parse_ident()?;
        Ok(Query::DropTable { table })
    }

    fn parse_column_list(&mut self) -> Result<Vec<String>, ParseError> {
        if *self.peek() == Token::Star {
            self.advance();
            return Ok(Vec::new()); // empty = wildcard
        }
        self.parse_ident_list()
    }

    fn parse_ident_list(&mut self) -> Result<Vec<String>, ParseError> {
        let mut names = Vec::new();
        names.push(self.parse_ident()?);
        while *self.peek() == Token::Comma {
            self.advance();
            names.push(self.parse_ident()?);
        }
        Ok(names)
    }

    fn parse_ident(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            _ => Err(ParseError::UnexpectedToken),
        }
    }

    fn parse_value_list(&mut self) -> Result<Vec<Value>, ParseError> {
        let mut values = Vec::new();
        values.push(self.parse_literal()?);
        while *self.peek() == Token::Comma {
            self.advance();
            values.push(self.parse_literal()?);
        }
        Ok(values)
    }

    fn parse_literal(&mut self) -> Result<Value, ParseError> {
        match self.advance() {
            Token::IntLit(v) => Ok(Value::Int(v)),
            Token::TextLit(s) => Ok(Value::Text(s)),
            Token::BoolLit(b) => Ok(Value::Bool(b)),
            Token::Null => Ok(Value::Null),
            _ => Err(ParseError::UnexpectedToken),
        }
    }

    fn parse_assignments(&mut self) -> Result<Vec<(String, Value)>, ParseError> {
        let mut assigns = Vec::new();
        loop {
            let col = self.parse_ident()?;
            self.expect(&Token::Eq)?;
            let val = self.parse_literal()?;
            assigns.push((col, val));
            if *self.peek() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(assigns)
    }

    fn parse_optional_where(&mut self) -> Result<Option<WhereClause>, ParseError> {
        if *self.peek() != Token::Where {
            return Ok(None);
        }
        self.advance(); // WHERE
        let clause = self.parse_where_expr()?;
        Ok(Some(clause))
    }

    fn parse_where_expr(&mut self) -> Result<WhereClause, ParseError> {
        let left = self.parse_where_atom()?;
        match self.peek().clone() {
            Token::And => {
                self.advance();
                let right = self.parse_where_expr()?;
                Ok(WhereClause::And(vec![left, right]))
            }
            Token::Or => {
                self.advance();
                let right = self.parse_where_expr()?;
                Ok(WhereClause::Or(vec![left, right]))
            }
            _ => Ok(left),
        }
    }

    fn parse_where_atom(&mut self) -> Result<WhereClause, ParseError> {
        let col = self.parse_ident()?;
        match self.peek().clone() {
            Token::IsNull => {
                self.advance();
                Ok(WhereClause::IsNull(col))
            }
            Token::IsNotNull => {
                self.advance();
                Ok(WhereClause::IsNotNull(col))
            }
            _ => {
                let op = match self.advance() {
                    Token::Eq => CmpOp::Eq,
                    Token::NotEq => CmpOp::NotEq,
                    Token::Lt => CmpOp::Lt,
                    Token::Gt => CmpOp::Gt,
                    Token::LtEq => CmpOp::LtEq,
                    Token::GtEq => CmpOp::GtEq,
                    _ => return Err(ParseError::UnexpectedToken),
                };
                let value = self.parse_literal()?;
                Ok(WhereClause::Single(Condition {
                    column: col,
                    op,
                    value,
                }))
            }
        }
    }

    fn parse_optional_order_by(&mut self) -> Result<Option<OrderByClause>, ParseError> {
        if *self.peek() != Token::OrderBy {
            return Ok(None);
        }
        self.advance(); // ORDER BY
        let column = self.parse_ident()?;
        let direction = match self.peek() {
            Token::Desc => {
                self.advance();
                SortDir::Desc
            }
            Token::Asc => {
                self.advance();
                SortDir::Asc
            }
            _ => SortDir::Asc,
        };
        Ok(Some(OrderByClause { column, direction }))
    }

    fn parse_optional_limit(&mut self) -> Result<Option<usize>, ParseError> {
        if *self.peek() != Token::Limit {
            return Ok(None);
        }
        self.advance(); // LIMIT
        match self.advance() {
            Token::IntLit(v) => Ok(Some(v as usize)),
            _ => Err(ParseError::UnexpectedToken),
        }
    }
}

/// Evaluate a WHERE clause against a row
pub fn evaluate_where(clause: &WhereClause, row: &Row, schema: &super::table::Schema) -> bool {
    match clause {
        WhereClause::Single(cond) => {
            if let Some(col_idx) = schema.column_index(&cond.column) {
                if col_idx < row.values.len() {
                    let cmp = row.values[col_idx].cmp_ord(&cond.value);
                    match cond.op {
                        CmpOp::Eq => cmp == 0,
                        CmpOp::NotEq => cmp != 0,
                        CmpOp::Lt => cmp < 0,
                        CmpOp::Gt => cmp > 0,
                        CmpOp::LtEq => cmp <= 0,
                        CmpOp::GtEq => cmp >= 0,
                    }
                } else {
                    false
                }
            } else {
                false
            }
        }
        WhereClause::And(clauses) => clauses.iter().all(|c| evaluate_where(c, row, schema)),
        WhereClause::Or(clauses) => clauses.iter().any(|c| evaluate_where(c, row, schema)),
        WhereClause::IsNull(col) => {
            if let Some(idx) = schema.column_index(col) {
                if idx < row.values.len() {
                    matches!(row.values[idx], Value::Null)
                } else {
                    true
                }
            } else {
                true
            }
        }
        WhereClause::IsNotNull(col) => {
            if let Some(idx) = schema.column_index(col) {
                if idx < row.values.len() {
                    !matches!(row.values[idx], Value::Null)
                } else {
                    false
                }
            } else {
                false
            }
        }
    }
}

/// Parse and execute a SQL query string
pub fn execute(sql: &str) -> Result<QueryResult, ParseError> {
    let tokens = tokenize(sql)?;
    let mut parser = Parser::new(tokens);
    let query = parser.parse_query()?;
    execute_query(&query)
}

/// Execute a parsed query
fn execute_query(query: &Query) -> Result<QueryResult, ParseError> {
    match query {
        Query::Select {
            columns,
            table,
            where_clause,
            order_by,
            limit,
        } => exec_select(table, columns, where_clause, order_by, limit),
        Query::Insert {
            table,
            columns,
            values,
        } => exec_insert(table, columns, values),
        Query::Update {
            table,
            assignments,
            where_clause,
        } => exec_update(table, assignments, where_clause),
        Query::Delete {
            table,
            where_clause,
        } => exec_delete(table, where_clause),
        Query::CreateTable { table, columns } => {
            let mut schema = Schema::new(table);
            for (name, ctype, nullable) in columns {
                schema.add_column(name, *ctype, *nullable);
            }
            super::table::create_table(schema)
                .map_err(|_| ParseError::ExecutionError(String::from("create table failed")))?;
            Ok(QueryResult::with_affected(0))
        }
        Query::DropTable { table } => {
            super::table::drop_table(table)
                .map_err(|_| ParseError::ExecutionError(String::from("drop table failed")))?;
            Ok(QueryResult::with_affected(0))
        }
    }
}

fn exec_select(
    table: &str,
    columns: &[String],
    where_clause: &Option<WhereClause>,
    order_by: &Option<OrderByClause>,
    limit: &Option<usize>,
) -> Result<QueryResult, ParseError> {
    super::table::with_table_ref(table, |tbl| {
        let all_rows = tbl.scan_all();
        let mut result_rows: Vec<Vec<Value>> = Vec::new();

        // Determine column indices
        let col_indices: Vec<usize> = if columns.is_empty() {
            (0..tbl.schema.columns.len()).collect()
        } else {
            columns
                .iter()
                .filter_map(|c| tbl.schema.column_index(c))
                .collect()
        };

        let col_names: Vec<String> = col_indices
            .iter()
            .map(|&i| tbl.schema.columns[i].name.clone())
            .collect();

        // Filter by WHERE
        for row in &all_rows {
            let matches = if let Some(ref wc) = where_clause {
                evaluate_where(wc, row, &tbl.schema)
            } else {
                true
            };
            if matches {
                let projected: Vec<Value> = col_indices
                    .iter()
                    .map(|&i| {
                        if i < row.values.len() {
                            row.values[i].clone()
                        } else {
                            Value::Null
                        }
                    })
                    .collect();
                result_rows.push(projected);
            }
        }

        // Sort by ORDER BY
        if let Some(ref ob) = order_by {
            if let Some(sort_col_pos) = col_names.iter().position(|c| *c == ob.column) {
                result_rows.sort_by(|a, b| {
                    let cmp = a[sort_col_pos].cmp_ord(&b[sort_col_pos]);
                    match ob.direction {
                        SortDir::Asc => match cmp {
                            -1 => core::cmp::Ordering::Less,
                            1 => core::cmp::Ordering::Greater,
                            _ => core::cmp::Ordering::Equal,
                        },
                        SortDir::Desc => match cmp {
                            -1 => core::cmp::Ordering::Greater,
                            1 => core::cmp::Ordering::Less,
                            _ => core::cmp::Ordering::Equal,
                        },
                    }
                });
            }
        }

        // Apply LIMIT
        if let Some(lim) = limit {
            result_rows.truncate(*lim);
        }

        let count = result_rows.len() as u64;
        QueryResult {
            columns: col_names,
            rows: result_rows,
            affected_rows: count,
            last_insert_id: None,
        }
    })
    .map_err(|_| ParseError::ExecutionError(String::from("table not found")))
}

fn exec_insert(
    table: &str,
    columns: &[String],
    values: &[Value],
) -> Result<QueryResult, ParseError> {
    super::table::with_table(table, |tbl| {
        // Build a full row aligning values to schema columns
        let mut row_values: Vec<Value> = vec![Value::Null; tbl.schema.columns.len()];
        for (i, col_name) in columns.iter().enumerate() {
            if let Some(col_idx) = tbl.schema.column_index(col_name) {
                if i < values.len() {
                    row_values[col_idx] = values[i].clone();
                }
            }
        }
        let rowid = tbl
            .insert_row(row_values)
            .map_err(|_| super::table::TableError::RowTooLarge)?;
        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: Some(rowid),
        })
    })
    .map_err(|_| ParseError::ExecutionError(String::from("insert failed")))
}

fn exec_update(
    table: &str,
    assignments: &[(String, Value)],
    where_clause: &Option<WhereClause>,
) -> Result<QueryResult, ParseError> {
    super::table::with_table(table, |tbl| {
        let matching_rowids: Vec<i64> = tbl
            .scan_all()
            .iter()
            .filter(|row| {
                if let Some(ref wc) = where_clause {
                    evaluate_where(wc, row, &tbl.schema)
                } else {
                    true
                }
            })
            .map(|row| row.rowid)
            .collect();

        let mut updated = 0u64;
        for rowid in matching_rowids {
            if let Ok(row) = tbl.get_row(rowid) {
                let mut new_values = row.values.clone();
                for (col, val) in assignments {
                    if let Some(idx) = tbl.schema.column_index(col) {
                        if idx < new_values.len() {
                            new_values[idx] = val.clone();
                        }
                    }
                }
                if tbl.update_row(rowid, new_values).is_ok() {
                    updated += 1;
                }
            }
        }
        Ok(QueryResult::with_affected(updated))
    })
    .map_err(|_| ParseError::ExecutionError(String::from("update failed")))
}

fn exec_delete(table: &str, where_clause: &Option<WhereClause>) -> Result<QueryResult, ParseError> {
    super::table::with_table(table, |tbl| {
        let matching_rowids: Vec<i64> = tbl
            .scan_all()
            .iter()
            .filter(|row| {
                if let Some(ref wc) = where_clause {
                    evaluate_where(wc, row, &tbl.schema)
                } else {
                    true
                }
            })
            .map(|row| row.rowid)
            .collect();

        let mut deleted = 0u64;
        for rowid in matching_rowids {
            if tbl.delete_row(rowid).is_ok() {
                deleted += 1;
            }
        }
        Ok(QueryResult::with_affected(deleted))
    })
    .map_err(|_| ParseError::ExecutionError(String::from("delete failed")))
}

/// Initialize the query engine
pub fn init() {
    serial_println!("    Query engine ready (SELECT, INSERT, UPDATE, DELETE, WHERE, ORDER BY)");
}
