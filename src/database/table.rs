/// Table storage for Genesis embedded database
///
/// Manages typed columnar storage with schema definition.
/// Supports column types: Int (i64), Text (String), Bool, Blob (Vec<u8>).
/// Each table has a primary key (auto-increment i64), optional indexes,
/// and row-level storage with schema validation.
///
/// Inspired by: SQLite page-based storage, PostgreSQL heap tuples.
/// All code is original.
use crate::{serial_print, serial_println};

use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Q16 fixed-point constant
const Q16_ONE: i32 = 65536;

/// Maximum columns per table
const MAX_COLUMNS: usize = 64;
/// Maximum tables in the catalog
const MAX_TABLES: usize = 256;
/// Maximum row size in bytes (for validation)
const MAX_ROW_SIZE: usize = 65536;

/// Column data types
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnType {
    Int,
    Text,
    Bool,
    Blob,
}

/// Column definition in a schema
#[derive(Clone, Debug)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
    pub nullable: bool,
    pub indexed: bool,
}

/// A single cell value
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
    Bool(bool),
    Blob(Vec<u8>),
}

impl Value {
    /// Return the type of this value (Null matches any type)
    pub fn value_type(&self) -> Option<ColumnType> {
        match self {
            Value::Null => None,
            Value::Int(_) => Some(ColumnType::Int),
            Value::Text(_) => Some(ColumnType::Text),
            Value::Bool(_) => Some(ColumnType::Bool),
            Value::Blob(_) => Some(ColumnType::Blob),
        }
    }

    /// Compare two values for ordering (returns -1, 0, or 1)
    pub fn cmp_ord(&self, other: &Value) -> i32 {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => {
                if *a < *b {
                    -1
                } else if *a > *b {
                    1
                } else {
                    0
                }
            }
            (Value::Text(a), Value::Text(b)) => {
                if *a < *b {
                    -1
                } else if *a > *b {
                    1
                } else {
                    0
                }
            }
            (Value::Bool(a), Value::Bool(b)) => {
                if *a == *b {
                    0
                } else if *a {
                    1
                } else {
                    -1
                }
            }
            (Value::Null, Value::Null) => 0,
            (Value::Null, _) => -1,
            (_, Value::Null) => 1,
            _ => 0, // Incompatible types treated as equal
        }
    }

    /// Check equality
    pub fn eq_val(&self, other: &Value) -> bool {
        self.cmp_ord(other) == 0
    }

    /// Approximate size in bytes
    pub fn size_bytes(&self) -> usize {
        match self {
            Value::Null => 0,
            Value::Int(_) => 8,
            Value::Text(s) => s.len(),
            Value::Bool(_) => 1,
            Value::Blob(b) => b.len(),
        }
    }

    /// Extract i64 or 0 if not an Int
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(v) => *v,
            _ => 0,
        }
    }

    /// Extract string reference or empty
    pub fn as_text(&self) -> &str {
        match self {
            Value::Text(s) => s.as_str(),
            _ => "",
        }
    }
}

/// Table schema
#[derive(Clone, Debug)]
pub struct Schema {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
}

impl Schema {
    pub fn new(name: &str) -> Self {
        Schema {
            table_name: String::from(name),
            columns: Vec::new(),
        }
    }

    pub fn add_column(&mut self, name: &str, col_type: ColumnType, nullable: bool) {
        if self.columns.len() < MAX_COLUMNS {
            self.columns.push(ColumnDef {
                name: String::from(name),
                col_type,
                nullable,
                indexed: false,
            });
        }
    }

    pub fn add_indexed_column(&mut self, name: &str, col_type: ColumnType, nullable: bool) {
        if self.columns.len() < MAX_COLUMNS {
            self.columns.push(ColumnDef {
                name: String::from(name),
                col_type,
                nullable,
                indexed: true,
            });
        }
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Validate that a row matches this schema
    pub fn validate_row(&self, values: &[Value]) -> Result<(), TableError> {
        if values.len() != self.columns.len() {
            return Err(TableError::ColumnCountMismatch);
        }
        for (i, (col, val)) in self.columns.iter().zip(values.iter()).enumerate() {
            match val {
                Value::Null => {
                    if !col.nullable {
                        return Err(TableError::NullViolation(i));
                    }
                }
                other => {
                    if let Some(vt) = other.value_type() {
                        if vt != col.col_type {
                            return Err(TableError::TypeMismatch(i));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// A single row in the table
#[derive(Clone, Debug)]
pub struct Row {
    pub rowid: i64,
    pub values: Vec<Value>,
    pub deleted: bool,
}

/// Table error types
#[derive(Debug)]
pub enum TableError {
    ColumnCountMismatch,
    TypeMismatch(usize),
    NullViolation(usize),
    RowNotFound(i64),
    TableFull,
    TableNotFound,
    DuplicateTable,
    RowTooLarge,
}

/// A database table with schema and row storage
pub struct Table {
    pub schema: Schema,
    pub table_id: u32,
    rows: Vec<Row>,
    next_rowid: i64,
    btree_index_id: Option<u32>,
    row_count: u64,
    deleted_count: u64,
}

impl Table {
    pub fn new(table_id: u32, schema: Schema) -> Self {
        // Create a B-tree index for the primary key
        let idx_id = super::btree::create_index();
        Table {
            schema,
            table_id,
            rows: Vec::new(),
            next_rowid: 1,
            btree_index_id: Some(idx_id),
            row_count: 0,
            deleted_count: 0,
        }
    }

    /// Insert a row. Returns the assigned rowid.
    pub fn insert_row(&mut self, values: Vec<Value>) -> Result<i64, TableError> {
        self.schema.validate_row(&values)?;

        // Check row size
        let size: usize = values.iter().map(|v| v.size_bytes()).sum();
        if size > MAX_ROW_SIZE {
            return Err(TableError::RowTooLarge);
        }

        let rowid = self.next_rowid;
        self.next_rowid = self.next_rowid.saturating_add(1);

        let row_idx = self.rows.len() as u64;

        // Index by primary key
        if let Some(tree_id) = self.btree_index_id {
            super::btree::pool_insert(tree_id, rowid, row_idx);
        }

        self.rows.push(Row {
            rowid,
            values,
            deleted: false,
        });
        self.row_count = self.row_count.saturating_add(1);

        Ok(rowid)
    }

    /// Get a row by rowid
    pub fn get_row(&self, rowid: i64) -> Result<&Row, TableError> {
        for row in &self.rows {
            if row.rowid == rowid && !row.deleted {
                return Ok(row);
            }
        }
        Err(TableError::RowNotFound(rowid))
    }

    /// Update a row by rowid with new values
    pub fn update_row(&mut self, rowid: i64, values: Vec<Value>) -> Result<(), TableError> {
        self.schema.validate_row(&values)?;
        for row in &mut self.rows {
            if row.rowid == rowid && !row.deleted {
                row.values = values;
                return Ok(());
            }
        }
        Err(TableError::RowNotFound(rowid))
    }

    /// Soft-delete a row by rowid
    pub fn delete_row(&mut self, rowid: i64) -> Result<(), TableError> {
        for row in &mut self.rows {
            if row.rowid == rowid && !row.deleted {
                row.deleted = true;
                self.row_count = self.row_count.saturating_sub(1);
                self.deleted_count = self.deleted_count.saturating_add(1);
                return Ok(());
            }
        }
        Err(TableError::RowNotFound(rowid))
    }

    /// Scan all non-deleted rows
    pub fn scan_all(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| !r.deleted).collect()
    }

    /// Scan rows matching a predicate on a specific column
    pub fn scan_where<F>(&self, col_idx: usize, predicate: F) -> Vec<&Row>
    where
        F: Fn(&Value) -> bool,
    {
        self.rows
            .iter()
            .filter(|r| !r.deleted && col_idx < r.values.len() && predicate(&r.values[col_idx]))
            .collect()
    }

    /// Return active row count
    pub fn active_row_count(&self) -> u64 {
        self.row_count
    }

    /// Return fragmentation ratio as Q16 (deleted / total)
    pub fn fragmentation_q16(&self) -> i32 {
        let total = self.row_count + self.deleted_count;
        if total == 0 {
            return 0;
        }
        (((self.deleted_count as i64) << 16) / (total as i64)) as i32
    }

    /// Compact the table by removing deleted rows (vacuum)
    pub fn vacuum(&mut self) {
        self.rows.retain(|r| !r.deleted);
        self.deleted_count = 0;
        // Rebuild primary key index
        if let Some(tree_id) = self.btree_index_id {
            // Re-insert all remaining rows
            for (i, row) in self.rows.iter().enumerate() {
                super::btree::pool_insert(tree_id, row.rowid, i as u64);
            }
        }
    }
}

/// The table catalog — holds all tables in the database
struct TableCatalog {
    tables: Vec<Table>,
    next_table_id: u32,
}

static CATALOG: Mutex<Option<TableCatalog>> = Mutex::new(None);

impl TableCatalog {
    fn new() -> Self {
        TableCatalog {
            tables: Vec::new(),
            next_table_id: 1,
        }
    }

    fn create_table(&mut self, schema: Schema) -> Result<u32, TableError> {
        if self.tables.len() >= MAX_TABLES {
            return Err(TableError::TableFull);
        }
        // Check for duplicate name
        for t in &self.tables {
            if t.schema.table_name == schema.table_name {
                return Err(TableError::DuplicateTable);
            }
        }
        let id = self.next_table_id;
        self.next_table_id = self.next_table_id.saturating_add(1);
        self.tables.push(Table::new(id, schema));
        Ok(id)
    }

    fn find_table(&self, name: &str) -> Option<usize> {
        self.tables.iter().position(|t| t.schema.table_name == name)
    }

    fn drop_table(&mut self, name: &str) -> Result<(), TableError> {
        let idx = self.find_table(name).ok_or(TableError::TableNotFound)?;
        self.tables.remove(idx);
        Ok(())
    }

    fn table_count(&self) -> usize {
        self.tables.len()
    }

    fn list_tables(&self) -> Vec<String> {
        self.tables
            .iter()
            .map(|t| t.schema.table_name.clone())
            .collect()
    }
}

/// Create a new table in the global catalog
pub fn create_table(schema: Schema) -> Result<u32, TableError> {
    let mut guard = CATALOG.lock();
    if let Some(ref mut catalog) = *guard {
        catalog.create_table(schema)
    } else {
        Err(TableError::TableNotFound)
    }
}

/// Execute an operation on a named table
pub fn with_table<F, R>(name: &str, f: F) -> Result<R, TableError>
where
    F: FnOnce(&mut Table) -> Result<R, TableError>,
{
    let mut guard = CATALOG.lock();
    if let Some(ref mut catalog) = *guard {
        let idx = catalog.find_table(name).ok_or(TableError::TableNotFound)?;
        f(&mut catalog.tables[idx])
    } else {
        Err(TableError::TableNotFound)
    }
}

/// Read-only access to a table
pub fn with_table_ref<F, R>(name: &str, f: F) -> Result<R, TableError>
where
    F: FnOnce(&Table) -> R,
{
    let guard = CATALOG.lock();
    if let Some(ref catalog) = *guard {
        let idx = catalog.find_table(name).ok_or(TableError::TableNotFound)?;
        Ok(f(&catalog.tables[idx]))
    } else {
        Err(TableError::TableNotFound)
    }
}

/// List all table names
pub fn list_tables() -> Vec<String> {
    let guard = CATALOG.lock();
    if let Some(ref catalog) = *guard {
        catalog.list_tables()
    } else {
        Vec::new()
    }
}

/// Drop a table by name
pub fn drop_table(name: &str) -> Result<(), TableError> {
    let mut guard = CATALOG.lock();
    if let Some(ref mut catalog) = *guard {
        catalog.drop_table(name)
    } else {
        Err(TableError::TableNotFound)
    }
}

/// Initialize the table storage subsystem
pub fn init() {
    let mut guard = CATALOG.lock();
    *guard = Some(TableCatalog::new());
    serial_println!("    Table storage ready (typed columns, schema validation, vacuum)");
}
