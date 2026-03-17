use crate::sync::Mutex;
/// Print spooler for Genesis
///
/// Job queue, priority scheduling, page range,
/// copies, duplex, color mode.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PrintJobState {
    Queued,
    Printing,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ColorMode {
    Color,
    Grayscale,
    Monochrome,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PaperSize {
    Letter,
    A4,
    Legal,
    A3,
    Photo4x6,
    Custom,
}

struct PrintJob {
    id: u32,
    state: PrintJobState,
    printer_id: u32,
    pages_total: u32,
    pages_printed: u32,
    copies: u32,
    duplex: bool,
    color_mode: ColorMode,
    paper_size: PaperSize,
    data_size_bytes: u64,
    submitted_at: u64,
    priority: u8,
}

struct PrintSpooler {
    jobs: Vec<PrintJob>,
    next_id: u32,
    max_concurrent: u32,
    active_count: u32,
}

static SPOOLER: Mutex<Option<PrintSpooler>> = Mutex::new(None);

impl PrintSpooler {
    fn new() -> Self {
        PrintSpooler {
            jobs: Vec::new(),
            next_id: 1,
            max_concurrent: 3,
            active_count: 0,
        }
    }

    fn submit(
        &mut self,
        printer_id: u32,
        pages: u32,
        copies: u32,
        duplex: bool,
        color: ColorMode,
        paper: PaperSize,
        size: u64,
        timestamp: u64,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let state = if self.active_count < self.max_concurrent {
            self.active_count = self.active_count.saturating_add(1);
            PrintJobState::Printing
        } else {
            PrintJobState::Queued
        };
        self.jobs.push(PrintJob {
            id,
            state,
            printer_id,
            pages_total: pages,
            pages_printed: 0,
            copies,
            duplex,
            color_mode: color,
            paper_size: paper,
            data_size_bytes: size,
            submitted_at: timestamp,
            priority: 5,
        });
        id
    }

    fn cancel(&mut self, job_id: u32) -> bool {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.state == PrintJobState::Printing {
                self.active_count -= 1;
            }
            job.state = PrintJobState::Cancelled;
            self.start_next();
            return true;
        }
        false
    }

    fn complete(&mut self, job_id: u32) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.state = PrintJobState::Completed;
            job.pages_printed = job.pages_total;
            self.active_count -= 1;
        }
        self.start_next();
    }

    fn start_next(&mut self) {
        if self.active_count >= self.max_concurrent {
            return;
        }
        if let Some(job) = self
            .jobs
            .iter_mut()
            .filter(|j| j.state == PrintJobState::Queued)
            .min_by_key(|j| (j.priority, j.submitted_at))
        {
            job.state = PrintJobState::Printing;
            self.active_count = self.active_count.saturating_add(1);
        }
    }
}

pub fn init() {
    let mut s = SPOOLER.lock();
    *s = Some(PrintSpooler::new());
    serial_println!("    Printing: spooler ready");
}
