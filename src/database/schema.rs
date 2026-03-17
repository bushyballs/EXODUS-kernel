use crate::sync::Mutex;
/// Schema definition and migration
///
/// Part of the AIOS database engine. Manages table definitions,
/// column types, constraints, schema versioning, and migration
/// history for rolling upgrades.
use alloc::string::String;
use alloc::vec::Vec;

pub enum ColumnType {
    Int,
    Text,
    Bool,
    Blob,
}

impl ColumnType {
    fn name(&self) -> &'static str {
        match self {
            ColumnType::Int => "INT",
            ColumnType::Text => "TEXT",
            ColumnType::Bool => "BOOL",
            ColumnType::Blob => "BLOB",
        }
    }

    fn default_size(&self) -> usize {
        match self {
            ColumnType::Int => 8,
            ColumnType::Text => 256,
            ColumnType::Bool => 1,
            ColumnType::Blob => 0,
        }
    }
}

pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
    pub nullable: bool,
}

impl ColumnDef {
    /// Create a new column definition
    pub fn new(name: &str, col_type: ColumnType, nullable: bool) -> Self {
        let mut n = String::new();
        for c in name.chars() {
            n.push(c);
        }
        Self {
            name: n,
            col_type,
            nullable,
        }
    }

    /// Validate the column name (alphanumeric + underscores only)
    fn validate_name(&self) -> bool {
        if self.name.is_empty() {
            return false;
        }
        for c in self.name.chars() {
            if !c.is_alphanumeric() && c != '_' {
                return false;
            }
        }
        // First char must not be a digit
        if let Some(first) = self.name.chars().next() {
            if first.is_ascii_digit() {
                return false;
            }
        }
        true
    }
}

/// A constraint on a table
#[derive(Clone)]
pub enum Constraint {
    PrimaryKey(String), // column name
    Unique(String),     // column name
    NotNull(String),    // column name
    ForeignKey {
        column: String,
        ref_table: String,
        ref_column: String,
    },
    Check(String), // expression string
}

/// A table definition in the schema
struct TableDef {
    name: String,
    columns: Vec<ColumnDef>,
    constraints: Vec<Constraint>,
    created_at_version: u32,
}

impl TableDef {
    fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn has_column(&self, name: &str) -> bool {
        for col in &self.columns {
            if col.name.as_str() == name {
                return true;
            }
        }
        false
    }

    fn row_size_estimate(&self) -> usize {
        let mut size = 0;
        for col in &self.columns {
            size += col.col_type.default_size();
            if col.nullable {
                size += 1; // null bitmap byte
            }
        }
        size
    }
}

/// Migration record
struct Migration {
    from_version: u32,
    to_version: u32,
    description: String,
    applied: bool,
}

pub struct Schema {
    tables: Vec<TableDef>,
    version: u32,
    migrations: Vec<Migration>,
    max_tables: usize,
}

impl Schema {
    pub fn new() -> Self {
        crate::serial_println!("[db::schema] schema manager created, version 0");
        Self {
            tables: Vec::new(),
            version: 0,
            migrations: Vec::new(),
            max_tables: 256,
        }
    }

    pub fn create_table(&mut self, name: &str, columns: Vec<ColumnDef>) -> Result<(), ()> {
        // Check table name validity
        if name.is_empty() {
            crate::serial_println!("[db::schema] error: empty table name");
            return Err(());
        }
        for c in name.chars() {
            if !c.is_alphanumeric() && c != '_' {
                crate::serial_println!("[db::schema] error: invalid char in table name '{}'", name);
                return Err(());
            }
        }

        // Check for duplicate table name
        for table in &self.tables {
            if table.name.as_str() == name {
                crate::serial_println!("[db::schema] error: table '{}' already exists", name);
                return Err(());
            }
        }

        // Check table limit
        if self.tables.len() >= self.max_tables {
            crate::serial_println!(
                "[db::schema] error: max table limit ({}) reached",
                self.max_tables
            );
            return Err(());
        }

        // Validate all column definitions
        if columns.is_empty() {
            crate::serial_println!("[db::schema] error: table must have at least one column");
            return Err(());
        }

        for col in &columns {
            if !col.validate_name() {
                crate::serial_println!("[db::schema] error: invalid column name '{}'", col.name);
                return Err(());
            }
        }

        // Check for duplicate column names
        for i in 0..columns.len() {
            for j in (i + 1)..columns.len() {
                if columns[i].name == columns[j].name {
                    crate::serial_println!(
                        "[db::schema] error: duplicate column name '{}'",
                        columns[i].name
                    );
                    return Err(());
                }
            }
        }

        let mut tname = String::new();
        for c in name.chars() {
            tname.push(c);
        }

        let col_count = columns.len();
        let table = TableDef {
            name: tname,
            columns,
            constraints: Vec::new(),
            created_at_version: self.version,
        };

        let row_est = table.row_size_estimate();
        self.tables.push(table);

        crate::serial_println!(
            "[db::schema] created table '{}': {} columns, ~{} bytes/row",
            name,
            col_count,
            row_est
        );
        Ok(())
    }

    /// Add a constraint to an existing table
    pub fn add_constraint(&mut self, table_name: &str, constraint: Constraint) -> Result<(), ()> {
        for table in &mut self.tables {
            if table.name.as_str() == table_name {
                // Validate the constraint references valid columns
                match &constraint {
                    Constraint::PrimaryKey(col)
                    | Constraint::Unique(col)
                    | Constraint::NotNull(col) => {
                        if !table.has_column(col) {
                            crate::serial_println!(
                                "[db::schema] error: column '{}' not in table '{}'",
                                col,
                                table_name
                            );
                            return Err(());
                        }
                    }
                    Constraint::ForeignKey { column, .. } => {
                        if !table.has_column(column) {
                            return Err(());
                        }
                    }
                    Constraint::Check(_) => {}
                }
                table.constraints.push(constraint);
                return Ok(());
            }
        }
        Err(())
    }

    /// Add a column to an existing table (ALTER TABLE ADD COLUMN)
    pub fn add_column(&mut self, table_name: &str, column: ColumnDef) -> Result<(), ()> {
        if !column.validate_name() {
            return Err(());
        }
        for table in &mut self.tables {
            if table.name.as_str() == table_name {
                // Check for duplicate
                if table.has_column(&column.name) {
                    return Err(());
                }
                // New columns must be nullable (for existing rows)
                if !column.nullable {
                    crate::serial_println!(
                        "[db::schema] warning: new column must be nullable for existing data"
                    );
                }
                crate::serial_println!(
                    "[db::schema] added column '{}' to table '{}'",
                    column.name,
                    table_name
                );
                table.columns.push(column);
                return Ok(());
            }
        }
        Err(())
    }

    /// Drop a table by name
    pub fn drop_table(&mut self, name: &str) -> Result<(), ()> {
        let mut found = false;
        let mut i = 0;
        while i < self.tables.len() {
            if self.tables[i].name.as_str() == name {
                self.tables.remove(i);
                found = true;
                crate::serial_println!("[db::schema] dropped table '{}'", name);
                break;
            }
            i += 1;
        }
        if found {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Migrate the schema to a new version
    pub fn migrate(&mut self, version: u32) -> Result<(), ()> {
        if version <= self.version {
            crate::serial_println!(
                "[db::schema] error: target version {} <= current {}",
                version,
                self.version
            );
            return Err(());
        }

        // Record the migration
        let mut desc = String::new();
        desc.push_str("Migration from v");
        // Simple integer to string
        let old = self.version;
        push_u32_str(&mut desc, old);
        desc.push_str(" to v");
        push_u32_str(&mut desc, version);

        self.migrations.push(Migration {
            from_version: self.version,
            to_version: version,
            description: desc,
            applied: true,
        });

        let old_version = self.version;
        self.version = version;
        crate::serial_println!(
            "[db::schema] migrated schema v{} -> v{}",
            old_version,
            version
        );
        Ok(())
    }

    /// Get the current schema version
    pub fn current_version(&self) -> u32 {
        self.version
    }

    /// Get the number of tables
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// Check if a table exists
    pub fn has_table(&self, name: &str) -> bool {
        for table in &self.tables {
            if table.name.as_str() == name {
                return true;
            }
        }
        false
    }

    /// List all table names
    pub fn table_names(&self) -> Vec<&str> {
        let mut names = Vec::with_capacity(self.tables.len());
        for table in &self.tables {
            names.push(table.name.as_str());
        }
        names
    }

    /// Get migration history count
    pub fn migration_count(&self) -> usize {
        self.migrations.len()
    }
}

/// Helper: push u32 as decimal string
fn push_u32_str(s: &mut String, mut val: u32) {
    if val == 0 {
        s.push('0');
        return;
    }
    let mut digits = Vec::new();
    while val > 0 {
        digits.push((b'0' + (val % 10) as u8) as char);
        val /= 10;
    }
    for i in (0..digits.len()).rev() {
        s.push(digits[i]);
    }
}

static SCHEMA_MANAGER: Mutex<Option<Schema>> = Mutex::new(None);

pub fn init() {
    let schema = Schema::new();
    let mut s = SCHEMA_MANAGER.lock();
    *s = Some(schema);
    crate::serial_println!("[db::schema] schema subsystem initialized");
}
