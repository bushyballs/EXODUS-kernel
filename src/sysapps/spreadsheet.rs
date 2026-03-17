/// Spreadsheet application for Genesis OS
///
/// Cell-based grid with A1-style references, formula evaluation
/// (SUM, AVG, COUNT, MIN, MAX), column resizing, row/column sorting,
/// and multi-sheet support. All numeric values stored as Q16 fixed-point.
/// Cell references parsed from hash-encoded A1 notation.
///
/// Inspired by: LibreOffice Calc, Google Sheets, Excel. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

/// 1.0 in Q16
const Q16_ONE: i32 = 65536;

/// Q16 multiplication: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 division: (a << 16) / b
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 {
        return None;
    }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum sheets per workbook
const MAX_SHEETS: usize = 64;
/// Maximum rows per sheet
const MAX_ROWS: usize = 65536;
/// Maximum columns per sheet
const MAX_COLS: usize = 256;
/// Maximum cells with data per sheet
const MAX_CELLS: usize = 500_000;
/// Default column width in Q16 pixels
const DEFAULT_COL_WIDTH: i32 = 80 * Q16_ONE;
/// Default row height in Q16 pixels
const DEFAULT_ROW_HEIGHT: i32 = 20 * Q16_ONE;
/// Maximum formula depth (circular ref guard)
const MAX_FORMULA_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Cell content type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellType {
    Empty,
    Number,
    Text,
    Formula,
    Boolean,
    Error,
}

/// Cell alignment
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Alignment {
    Left,
    Center,
    Right,
    Auto,
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDir {
    Ascending,
    Descending,
}

/// Formula function identifiers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FormulaFn {
    Sum,
    Avg,
    Count,
    Min,
    Max,
    Abs,
    If,
    None,
}

/// Cell error codes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellError {
    DivZero,
    BadRef,
    BadValue,
    CircRef,
    BadFormula,
    None,
}

/// Spreadsheet operation result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SheetResult {
    Success,
    NotFound,
    OutOfBounds,
    LimitReached,
    CircularRef,
    FormulaError,
    ReadOnly,
    IoError,
}

/// A cell reference (row, col) -- zero-indexed
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

/// A range of cells
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellRange {
    pub start: CellRef,
    pub end: CellRef,
}

/// A single cell
#[derive(Debug, Clone)]
pub struct Cell {
    pub pos: CellRef,
    pub cell_type: CellType,
    pub value_q16: i32,
    pub text_hash: u64,
    pub formula_hash: u64,
    pub formula_fn: FormulaFn,
    pub formula_range: CellRange,
    pub error: CellError,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub alignment: Alignment,
    pub fg_color: u32,
    pub bg_color: u32,
    pub format_hash: u64,
}

/// Column metadata
#[derive(Debug, Clone, Copy)]
pub struct ColMeta {
    pub index: u32,
    pub width_q16: i32,
    pub hidden: bool,
    pub frozen: bool,
}

/// Row metadata
#[derive(Debug, Clone, Copy)]
pub struct RowMeta {
    pub index: u32,
    pub height_q16: i32,
    pub hidden: bool,
    pub frozen: bool,
}

/// A single spreadsheet sheet
#[derive(Debug, Clone)]
pub struct Sheet {
    pub id: u64,
    pub name_hash: u64,
    pub cells: Vec<Cell>,
    pub col_meta: Vec<ColMeta>,
    pub row_meta: Vec<RowMeta>,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    pub active_cell: CellRef,
    pub selection_start: CellRef,
    pub selection_end: CellRef,
    pub read_only: bool,
}

/// A workbook containing multiple sheets
#[derive(Debug, Clone)]
pub struct Workbook {
    pub id: u64,
    pub name_hash: u64,
    pub sheets: Vec<Sheet>,
    pub active_sheet: usize,
    pub created: u64,
    pub modified: u64,
}

/// Spreadsheet state
struct SpreadsheetState {
    workbooks: Vec<Workbook>,
    next_workbook_id: u64,
    next_sheet_id: u64,
    timestamp: u64,
    eval_depth: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SPREADSHEET: Mutex<Option<SpreadsheetState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn next_timestamp(state: &mut SpreadsheetState) -> u64 {
    state.timestamp += 1;
    state.timestamp
}

fn default_state() -> SpreadsheetState {
    SpreadsheetState {
        workbooks: Vec::new(),
        next_workbook_id: 1,
        next_sheet_id: 1,
        timestamp: 0,
        eval_depth: 0,
    }
}

fn new_cell(row: u32, col: u32) -> Cell {
    Cell {
        pos: CellRef { row, col },
        cell_type: CellType::Empty,
        value_q16: 0,
        text_hash: 0,
        formula_hash: 0,
        formula_fn: FormulaFn::None,
        formula_range: CellRange {
            start: CellRef { row: 0, col: 0 },
            end: CellRef { row: 0, col: 0 },
        },
        error: CellError::None,
        bold: false,
        italic: false,
        underline: false,
        alignment: Alignment::Auto,
        fg_color: 0xFF00_0000,
        bg_color: 0xFFFF_FFFF,
        format_hash: 0,
    }
}

fn find_cell<'a>(sheet: &'a Sheet, row: u32, col: u32) -> Option<&'a Cell> {
    sheet.cells.iter().find(|c| c.pos.row == row && c.pos.col == col)
}

fn find_cell_mut<'a>(sheet: &'a mut Sheet, row: u32, col: u32) -> Option<&'a mut Cell> {
    sheet.cells.iter_mut().find(|c| c.pos.row == row && c.pos.col == col)
}

fn get_or_create_cell(sheet: &mut Sheet, row: u32, col: u32) -> &mut Cell {
    if !sheet.cells.iter().any(|c| c.pos.row == row && c.pos.col == col) {
        sheet.cells.push(new_cell(row, col));
    }
    sheet.cells.iter_mut().find(|c| c.pos.row == row && c.pos.col == col).unwrap()
}

/// Collect numeric Q16 values from a range on a sheet
fn collect_range_values(sheet: &Sheet, range: &CellRange) -> Vec<i32> {
    let mut values = Vec::new();
    let r_start = if range.start.row < range.end.row { range.start.row } else { range.end.row };
    let r_end = if range.start.row > range.end.row { range.start.row } else { range.end.row };
    let c_start = if range.start.col < range.end.col { range.start.col } else { range.end.col };
    let c_end = if range.start.col > range.end.col { range.start.col } else { range.end.col };

    for r in r_start..=r_end {
        for c in c_start..=c_end {
            if let Some(cell) = find_cell(sheet, r, c) {
                if cell.cell_type == CellType::Number || cell.cell_type == CellType::Formula {
                    values.push(cell.value_q16);
                }
            }
        }
    }
    values
}

/// Evaluate a formula function over a range
fn eval_formula(sheet: &Sheet, func: FormulaFn, range: &CellRange) -> Result<i32, CellError> {
    let values = collect_range_values(sheet, range);
    match func {
        FormulaFn::Sum => {
            let mut total: i64 = 0;
            for &v in values.iter() {
                total += v as i64;
            }
            Ok(total as i32)
        }
        FormulaFn::Avg => {
            if values.is_empty() {
                return Err(CellError::DivZero);
            }
            let mut total: i64 = 0;
            for &v in values.iter() {
                total += v as i64;
            }
            let count = values.len() as i32;
            q16_div(total as i32, count * Q16_ONE)
                .map(|r| q16_mul(r, Q16_ONE))
                .ok_or(CellError::DivZero)
        }
        FormulaFn::Count => {
            Ok((values.len() as i32) * Q16_ONE)
        }
        FormulaFn::Min => {
            if values.is_empty() {
                return Ok(0);
            }
            let mut min_val = values[0];
            for &v in values.iter().skip(1) {
                if v < min_val {
                    min_val = v;
                }
            }
            Ok(min_val)
        }
        FormulaFn::Max => {
            if values.is_empty() {
                return Ok(0);
            }
            let mut max_val = values[0];
            for &v in values.iter().skip(1) {
                if v > max_val {
                    max_val = v;
                }
            }
            Ok(max_val)
        }
        FormulaFn::Abs => {
            if values.is_empty() {
                return Ok(0);
            }
            Ok(if values[0] < 0 { -values[0] } else { values[0] })
        }
        FormulaFn::If | FormulaFn::None => Err(CellError::BadFormula),
    }
}

// ---------------------------------------------------------------------------
// Public API -- Workbook management
// ---------------------------------------------------------------------------

/// Create a new workbook with one sheet
pub fn create_workbook(name_hash: u64) -> Result<u64, SheetResult> {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(SheetResult::IoError),
    };
    let now = next_timestamp(state);
    let wb_id = state.next_workbook_id;
    state.next_workbook_id += 1;
    let sheet_id = state.next_sheet_id;
    state.next_sheet_id += 1;

    let sheet = Sheet {
        id: sheet_id,
        name_hash: 0x5368656574_310000, // "Sheet1" hash
        cells: Vec::new(),
        col_meta: (0..26).map(|i| ColMeta {
            index: i,
            width_q16: DEFAULT_COL_WIDTH,
            hidden: false,
            frozen: false,
        }).collect(),
        row_meta: Vec::new(),
        frozen_rows: 0,
        frozen_cols: 0,
        active_cell: CellRef { row: 0, col: 0 },
        selection_start: CellRef { row: 0, col: 0 },
        selection_end: CellRef { row: 0, col: 0 },
        read_only: false,
    };

    state.workbooks.push(Workbook {
        id: wb_id,
        name_hash,
        sheets: vec![sheet],
        active_sheet: 0,
        created: now,
        modified: now,
    });
    Ok(wb_id)
}

/// Add a sheet to a workbook
pub fn add_sheet(workbook_id: u64, name_hash: u64) -> Result<u64, SheetResult> {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(SheetResult::IoError),
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return Err(SheetResult::NotFound),
    };
    if wb.sheets.len() >= MAX_SHEETS {
        return Err(SheetResult::LimitReached);
    }
    let sheet_id = state.next_sheet_id;
    state.next_sheet_id += 1;
    let now = next_timestamp(state);
    wb.sheets.push(Sheet {
        id: sheet_id,
        name_hash,
        cells: Vec::new(),
        col_meta: (0..26).map(|i| ColMeta {
            index: i,
            width_q16: DEFAULT_COL_WIDTH,
            hidden: false,
            frozen: false,
        }).collect(),
        row_meta: Vec::new(),
        frozen_rows: 0,
        frozen_cols: 0,
        active_cell: CellRef { row: 0, col: 0 },
        selection_start: CellRef { row: 0, col: 0 },
        selection_end: CellRef { row: 0, col: 0 },
        read_only: false,
    });
    wb.modified = now;
    Ok(sheet_id)
}

/// Delete a sheet from a workbook (must have at least 1 remaining)
pub fn delete_sheet(workbook_id: u64, sheet_id: u64) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    if wb.sheets.len() <= 1 {
        return SheetResult::LimitReached;
    }
    let before = wb.sheets.len();
    wb.sheets.retain(|s| s.id != sheet_id);
    if wb.sheets.len() < before {
        if wb.active_sheet >= wb.sheets.len() {
            wb.active_sheet = wb.sheets.len() - 1;
        }
        let now = next_timestamp(state);
        wb.modified = now;
        SheetResult::Success
    } else {
        SheetResult::NotFound
    }
}

// ---------------------------------------------------------------------------
// Public API -- Cell operations
// ---------------------------------------------------------------------------

/// Set a cell's numeric value (Q16)
pub fn set_number(workbook_id: u64, sheet_id: u64, row: u32, col: u32, value_q16: i32) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if sheet.read_only {
        return SheetResult::ReadOnly;
    }
    if row >= MAX_ROWS as u32 || col >= MAX_COLS as u32 {
        return SheetResult::OutOfBounds;
    }
    if sheet.cells.len() >= MAX_CELLS && find_cell(sheet, row, col).is_none() {
        return SheetResult::LimitReached;
    }
    let cell = get_or_create_cell(sheet, row, col);
    cell.cell_type = CellType::Number;
    cell.value_q16 = value_q16;
    cell.error = CellError::None;
    let now = next_timestamp(state);
    wb.modified = now;
    SheetResult::Success
}

/// Set a cell's text value
pub fn set_text(workbook_id: u64, sheet_id: u64, row: u32, col: u32, text_hash: u64) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if sheet.read_only {
        return SheetResult::ReadOnly;
    }
    if row >= MAX_ROWS as u32 || col >= MAX_COLS as u32 {
        return SheetResult::OutOfBounds;
    }
    if sheet.cells.len() >= MAX_CELLS && find_cell(sheet, row, col).is_none() {
        return SheetResult::LimitReached;
    }
    let cell = get_or_create_cell(sheet, row, col);
    cell.cell_type = CellType::Text;
    cell.text_hash = text_hash;
    cell.error = CellError::None;
    let now = next_timestamp(state);
    wb.modified = now;
    SheetResult::Success
}

/// Set a formula on a cell (e.g., SUM over a range)
pub fn set_formula(
    workbook_id: u64,
    sheet_id: u64,
    row: u32,
    col: u32,
    func: FormulaFn,
    range: CellRange,
    formula_hash: u64,
) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if sheet.read_only {
        return SheetResult::ReadOnly;
    }
    if row >= MAX_ROWS as u32 || col >= MAX_COLS as u32 {
        return SheetResult::OutOfBounds;
    }

    // Circular reference check: formula must not reference its own cell
    let r_start = if range.start.row < range.end.row { range.start.row } else { range.end.row };
    let r_end = if range.start.row > range.end.row { range.start.row } else { range.end.row };
    let c_start = if range.start.col < range.end.col { range.start.col } else { range.end.col };
    let c_end = if range.start.col > range.end.col { range.start.col } else { range.end.col };
    if row >= r_start && row <= r_end && col >= c_start && col <= c_end {
        return SheetResult::CircularRef;
    }

    // Evaluate the formula
    let result = eval_formula(sheet, func, &range);
    if sheet.cells.len() >= MAX_CELLS && find_cell(sheet, row, col).is_none() {
        return SheetResult::LimitReached;
    }
    let cell = get_or_create_cell(sheet, row, col);
    cell.cell_type = CellType::Formula;
    cell.formula_fn = func;
    cell.formula_range = range;
    cell.formula_hash = formula_hash;
    match result {
        Ok(val) => {
            cell.value_q16 = val;
            cell.error = CellError::None;
        }
        Err(e) => {
            cell.value_q16 = 0;
            cell.error = e;
        }
    }
    let now = next_timestamp(state);
    wb.modified = now;
    SheetResult::Success
}

/// Get a cell's value
pub fn get_cell(workbook_id: u64, sheet_id: u64, row: u32, col: u32) -> Option<Cell> {
    let guard = SPREADSHEET.lock();
    let state = guard.as_ref()?;
    let wb = state.workbooks.iter().find(|w| w.id == workbook_id)?;
    let sheet = wb.sheets.iter().find(|s| s.id == sheet_id)?;
    find_cell(sheet, row, col).cloned()
}

/// Clear a cell
pub fn clear_cell(workbook_id: u64, sheet_id: u64, row: u32, col: u32) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    sheet.cells.retain(|c| !(c.pos.row == row && c.pos.col == col));
    let now = next_timestamp(state);
    wb.modified = now;
    SheetResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Column / row management
// ---------------------------------------------------------------------------

/// Resize a column
pub fn set_column_width(workbook_id: u64, sheet_id: u64, col: u32, width_q16: i32) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if let Some(cm) = sheet.col_meta.iter_mut().find(|c| c.index == col) {
        cm.width_q16 = width_q16;
    } else {
        sheet.col_meta.push(ColMeta {
            index: col,
            width_q16,
            hidden: false,
            frozen: false,
        });
    }
    SheetResult::Success
}

/// Hide or show a column
pub fn set_column_hidden(workbook_id: u64, sheet_id: u64, col: u32, hidden: bool) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if let Some(cm) = sheet.col_meta.iter_mut().find(|c| c.index == col) {
        cm.hidden = hidden;
        SheetResult::Success
    } else {
        SheetResult::NotFound
    }
}

/// Sort a sheet by a column
pub fn sort_by_column(workbook_id: u64, sheet_id: u64, col: u32, direction: SortDir) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };

    // Gather all unique rows
    let mut rows: Vec<u32> = sheet.cells.iter().map(|c| c.pos.row).collect();
    rows.sort();
    rows.dedup();

    // Build sort key for each row based on the column value
    let mut row_keys: Vec<(u32, i32)> = rows
        .iter()
        .map(|&r| {
            let key = find_cell(sheet, r, col).map(|c| c.value_q16).unwrap_or(0);
            (r, key)
        })
        .collect();

    match direction {
        SortDir::Ascending => row_keys.sort_by(|a, b| a.1.cmp(&b.1)),
        SortDir::Descending => row_keys.sort_by(|a, b| b.1.cmp(&a.1)),
    }

    // Build row remapping
    let mut remap: Vec<(u32, u32)> = Vec::new();
    for (new_row_idx, &(old_row, _)) in row_keys.iter().enumerate() {
        remap.push((old_row, new_row_idx as u32));
    }

    // Apply the remap to all cells
    for cell in sheet.cells.iter_mut() {
        if let Some(&(_, new_row)) = remap.iter().find(|&&(old, _)| old == cell.pos.row) {
            cell.pos.row = new_row;
        }
    }

    let now = next_timestamp(state);
    wb.modified = now;
    SheetResult::Success
}

/// Set frozen rows/columns (freeze panes)
pub fn freeze_panes(workbook_id: u64, sheet_id: u64, rows: u32, cols: u32) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    sheet.frozen_rows = rows;
    sheet.frozen_cols = cols;
    SheetResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Cell formatting
// ---------------------------------------------------------------------------

/// Set cell formatting
pub fn set_cell_format(
    workbook_id: u64,
    sheet_id: u64,
    row: u32,
    col: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    alignment: Alignment,
) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if sheet.cells.len() >= MAX_CELLS && find_cell(sheet, row, col).is_none() {
        return SheetResult::LimitReached;
    }
    let cell = get_or_create_cell(sheet, row, col);
    cell.bold = bold;
    cell.italic = italic;
    cell.underline = underline;
    cell.alignment = alignment;
    SheetResult::Success
}

/// Set cell colors
pub fn set_cell_colors(
    workbook_id: u64,
    sheet_id: u64,
    row: u32,
    col: u32,
    fg: u32,
    bg: u32,
) -> SheetResult {
    let mut guard = SPREADSHEET.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return SheetResult::IoError,
    };
    let wb = match state.workbooks.iter_mut().find(|w| w.id == workbook_id) {
        Some(w) => w,
        None => return SheetResult::NotFound,
    };
    let sheet = match wb.sheets.iter_mut().find(|s| s.id == sheet_id) {
        Some(s) => s,
        None => return SheetResult::NotFound,
    };
    if let Some(cell) = find_cell_mut(sheet, row, col) {
        cell.fg_color = fg;
        cell.bg_color = bg;
        SheetResult::Success
    } else {
        SheetResult::NotFound
    }
}

/// Get workbook count
pub fn workbook_count() -> usize {
    let guard = SPREADSHEET.lock();
    match guard.as_ref() {
        Some(state) => state.workbooks.len(),
        None => 0,
    }
}

/// Get cell count for a sheet
pub fn cell_count(workbook_id: u64, sheet_id: u64) -> usize {
    let guard = SPREADSHEET.lock();
    match guard.as_ref() {
        Some(state) => {
            state.workbooks.iter()
                .find(|w| w.id == workbook_id)
                .and_then(|wb| wb.sheets.iter().find(|s| s.id == sheet_id))
                .map(|s| s.cells.len())
                .unwrap_or(0)
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the spreadsheet subsystem
pub fn init() {
    let mut guard = SPREADSHEET.lock();
    *guard = Some(default_state());
    serial_println!("    Spreadsheet ready (Q16 fixed-point formulas)");
}
