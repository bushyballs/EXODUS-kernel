use crate::sync::Mutex;
/// Database cursor for result iteration
///
/// Part of the AIOS database engine. Provides a forward-scrollable
/// cursor over query result sets with column metadata, position
/// tracking, and type-safe value access.
use alloc::string::String;
use alloc::vec::Vec;

pub struct Row {
    pub values: Vec<Value>,
}

impl Row {
    /// Create a new row with the given values
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    /// Get a value by column index
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.values.get(index)
    }

    /// Get a value as i64 by column index
    pub fn get_int(&self, index: usize) -> Option<i64> {
        match self.values.get(index) {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        }
    }

    /// Get a value as string reference by column index
    pub fn get_text(&self, index: usize) -> Option<&str> {
        match self.values.get(index) {
            Some(Value::Text(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get a value as bool by column index
    pub fn get_bool(&self, index: usize) -> Option<bool> {
        match self.values.get(index) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// Number of columns in this row
    pub fn column_count(&self) -> usize {
        self.values.len()
    }
}

pub enum Value {
    Int(i64),
    Text(String),
    Bool(bool),
    Blob(Vec<u8>),
    Null,
}

impl Value {
    /// Check if this value is null
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Get the type name of this value
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Text(_) => "text",
            Value::Bool(_) => "bool",
            Value::Blob(_) => "blob",
            Value::Null => "null",
        }
    }
}

/// Column metadata for result sets
#[derive(Clone)]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// Cursor direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorDirection {
    Forward,
    Backward,
}

pub struct Cursor {
    rows: Vec<Row>,
    position: isize, // -1 = before first, rows.len() = after last
    columns: Vec<ColumnMeta>,
    is_open: bool,
    fetch_size: usize,
    total_rows_fetched: u64,
}

impl Cursor {
    pub fn new() -> Self {
        crate::serial_println!("[db::cursor] new cursor created");
        Self {
            rows: Vec::new(),
            position: -1, // before first row
            columns: Vec::new(),
            is_open: true,
            fetch_size: 100,
            total_rows_fetched: 0,
        }
    }

    /// Create a cursor pre-loaded with a result set
    pub fn from_rows(columns: Vec<ColumnMeta>, rows: Vec<Row>) -> Self {
        let count = rows.len();
        crate::serial_println!(
            "[db::cursor] cursor created with {} rows, {} columns",
            count,
            columns.len()
        );
        Self {
            rows,
            position: -1,
            columns,
            is_open: true,
            fetch_size: 100,
            total_rows_fetched: count as u64,
        }
    }

    /// Add a row to the cursor's result set (used during query execution)
    pub fn add_row(&mut self, row: Row) {
        self.rows.push(row);
        self.total_rows_fetched = self.total_rows_fetched.saturating_add(1);
    }

    /// Set column metadata
    pub fn set_columns(&mut self, columns: Vec<ColumnMeta>) {
        self.columns = columns;
    }

    /// Advance to the next row and return it
    pub fn next(&mut self) -> Option<Row> {
        if !self.is_open {
            return None;
        }

        let next_pos = self.position + 1;
        if next_pos >= 0 && (next_pos as usize) < self.rows.len() {
            self.position = next_pos;
            // Move the row out and replace with an empty row
            let idx = next_pos as usize;
            let mut placeholder = Row::new(Vec::new());
            // Swap the row out
            core::mem::swap(&mut self.rows[idx], &mut placeholder);
            Some(placeholder)
        } else {
            self.position = self.rows.len() as isize;
            None
        }
    }

    /// Peek at the current row without consuming it
    pub fn peek(&self) -> Option<&Row> {
        if !self.is_open || self.position < 0 {
            return None;
        }
        let pos = self.position as usize;
        self.rows.get(pos)
    }

    /// Skip n rows forward
    pub fn skip(&mut self, n: usize) -> usize {
        let mut skipped = 0;
        for _ in 0..n {
            let next_pos = self.position + 1;
            if next_pos >= 0 && (next_pos as usize) < self.rows.len() {
                self.position = next_pos;
                skipped += 1;
            } else {
                break;
            }
        }
        skipped
    }

    pub fn reset(&mut self) {
        self.position = -1;
        crate::serial_println!("[db::cursor] cursor reset to beginning");
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Get the current position (-1 = before first)
    pub fn position(&self) -> isize {
        self.position
    }

    /// Check if the cursor has more rows
    pub fn has_next(&self) -> bool {
        if !self.is_open {
            return false;
        }
        let next_pos = self.position + 1;
        next_pos >= 0 && (next_pos as usize) < self.rows.len()
    }

    /// Close the cursor and release resources
    pub fn close(&mut self) {
        self.is_open = false;
        self.rows.clear();
        self.columns.clear();
        crate::serial_println!(
            "[db::cursor] cursor closed, fetched {} total rows",
            self.total_rows_fetched
        );
    }

    /// Get column metadata
    pub fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    /// Get column count
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Get fetch size
    pub fn fetch_size(&self) -> usize {
        self.fetch_size
    }

    /// Set fetch size for batch operations
    pub fn set_fetch_size(&mut self, size: usize) {
        self.fetch_size = if size == 0 { 1 } else { size };
    }

    /// Check if cursor is open
    pub fn is_open(&self) -> bool {
        self.is_open
    }
}

static CURSOR_POOL: Mutex<Option<CursorPool>> = Mutex::new(None);

struct CursorPool {
    next_id: u64,
    active_count: u32,
    max_active: u32,
}

impl CursorPool {
    fn new() -> Self {
        Self {
            next_id: 1,
            active_count: 0,
            max_active: 256,
        }
    }

    fn allocate(&mut self) -> Option<u64> {
        if self.active_count >= self.max_active {
            crate::serial_println!("[db::cursor] pool exhausted, {} active", self.active_count);
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.active_count = self.active_count.saturating_add(1);
        Some(id)
    }

    fn release(&mut self) {
        if self.active_count > 0 {
            self.active_count -= 1;
        }
    }
}

pub fn init() {
    let pool = CursorPool::new();
    let mut p = CURSOR_POOL.lock();
    *p = Some(pool);
    crate::serial_println!("[db::cursor] cursor subsystem initialized");
}

/// Allocate a cursor ID from the pool
pub fn allocate_cursor() -> Option<u64> {
    let mut p = CURSOR_POOL.lock();
    match p.as_mut() {
        Some(pool) => pool.allocate(),
        None => None,
    }
}

/// Release a cursor back to the pool
pub fn release_cursor() {
    let mut p = CURSOR_POOL.lock();
    if let Some(ref mut pool) = *p {
        pool.release();
    }
}
