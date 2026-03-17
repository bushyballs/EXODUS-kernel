/// jobs.rs — print job management and IPP stub for Genesis.
///
/// Provides:
/// - `PrintStatus` enum: `Queued`, `Printing`, `Done`, `Failed`.
/// - `PrintJob` struct: fixed 4-KiB data buffer, page count, copies,
///   and status.
/// - A static print queue of 8 jobs.
/// - `submit_print_job(data, copies)` — returns a job ID.
/// - `cancel_job(id)` — mark a job as Failed / remove it.
/// - `send_ipp_request(printer_ip, job)` — IPP (Internet Printing
///   Protocol) stub that logs via `serial_println!`.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of queued print jobs.
pub const PRINT_QUEUE_SIZE: usize = 8;

/// Maximum data size per job (bytes).
pub const JOB_DATA_LEN: usize = 4096;

// ---------------------------------------------------------------------------
// PrintStatus
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrintStatus {
    Queued,
    Printing,
    Done,
    Failed,
}

// ---------------------------------------------------------------------------
// PrintJob
// ---------------------------------------------------------------------------

/// A single print job in the queue.
#[derive(Clone, Copy)]
pub struct PrintJob {
    /// Raw job data (PostScript / PCL / raw bytes), null-padded.
    pub data: [u8; JOB_DATA_LEN],

    /// Actual number of data bytes stored in `data`.
    pub data_len: usize,

    /// Number of pages in the document.
    pub pages: u16,

    /// Number of copies to print.
    pub copies: u8,

    /// Current job status.
    pub status: PrintStatus,

    /// Unique job identifier (1-based, 0 = unused slot).
    pub job_id: u32,

    /// Submission timestamp.
    pub timestamp: u64,
}

impl PrintJob {
    /// Construct an empty / unused job slot.
    pub const fn empty() -> Self {
        Self {
            data: [0u8; JOB_DATA_LEN],
            data_len: 0,
            pages: 0,
            copies: 1,
            status: PrintStatus::Queued,
            job_id: 0,
            timestamp: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static print queue
// ---------------------------------------------------------------------------

static mut PRINT_QUEUE: [Option<PrintJob>; PRINT_QUEUE_SIZE] = [None; PRINT_QUEUE_SIZE];

/// Monotonically increasing job ID counter.
static mut NEXT_JOB_ID: u32 = 1;

// ---------------------------------------------------------------------------
// submit_print_job
// ---------------------------------------------------------------------------

/// Submit a new print job.
///
/// `data` is the raw document bytes (truncated to `JOB_DATA_LEN` if larger).
/// `copies` is the number of copies (clamped to at least 1).
/// `timestamp` is the kernel tick / epoch at submission time.
///
/// Returns the assigned job ID (>= 1), or 0 if the queue is full.
pub fn submit_print_job(data: &[u8], copies: u8, timestamp: u64) -> u32 {
    let copies = if copies == 0 { 1 } else { copies };

    unsafe {
        for slot in PRINT_QUEUE.iter_mut() {
            if slot.is_none() {
                let jid = NEXT_JOB_ID;
                NEXT_JOB_ID = NEXT_JOB_ID.saturating_add(1);

                let mut job = PrintJob::empty();
                job.job_id = jid;
                job.copies = copies;
                job.timestamp = timestamp;
                job.status = PrintStatus::Queued;

                let dlen = data.len().min(JOB_DATA_LEN);
                job.data[..dlen].copy_from_slice(&data[..dlen]);
                job.data_len = dlen;

                // Estimate page count: 1 page per ~3 KiB of data (rough heuristic).
                job.pages = ((dlen as u32).saturating_add(3071) / 3072) as u16;
                if job.pages == 0 {
                    job.pages = 1;
                }

                *slot = Some(job);
                serial_println!(
                    "[jobs] submit_print_job id={} bytes={} pages={} copies={}",
                    jid,
                    dlen,
                    job.pages,
                    copies
                );
                return jid;
            }
        }
    }
    serial_println!("[jobs] submit_print_job: queue full");
    0
}

// ---------------------------------------------------------------------------
// cancel_job
// ---------------------------------------------------------------------------

/// Cancel the job with the given ID.
///
/// Jobs that are `Done` or already `Failed` are left in place (they will
/// be garbage-collected by `gc_completed_jobs`).  Active / queued jobs
/// are marked `Failed` and their slot is freed.
pub fn cancel_job(id: u32) {
    if id == 0 {
        return;
    }
    unsafe {
        for slot in PRINT_QUEUE.iter_mut() {
            if let Some(ref mut job) = slot {
                if job.job_id == id {
                    match job.status {
                        PrintStatus::Done | PrintStatus::Failed => {
                            serial_println!("[jobs] cancel_job id={} already terminal", id);
                        }
                        _ => {
                            job.status = PrintStatus::Failed;
                            serial_println!("[jobs] cancel_job id={} -> Failed", id);
                            // Free the slot so it can be reused.
                            *slot = None;
                        }
                    }
                    return;
                }
            }
        }
    }
    serial_println!("[jobs] cancel_job id={} not found", id);
}

// ---------------------------------------------------------------------------
// advance_job_state (internal helper)
// ---------------------------------------------------------------------------

/// Move a `Queued` job to `Printing`, or a `Printing` job to `Done`.
///
/// This simulates the spooler advancing the lifecycle.  Returns `true`
/// if the job was found and its state changed.
pub fn advance_job_state(id: u32) -> bool {
    unsafe {
        for slot in PRINT_QUEUE.iter_mut() {
            if let Some(ref mut job) = slot {
                if job.job_id == id {
                    match job.status {
                        PrintStatus::Queued => {
                            job.status = PrintStatus::Printing;
                            serial_println!("[jobs] job {} -> Printing", id);
                            return true;
                        }
                        PrintStatus::Printing => {
                            job.status = PrintStatus::Done;
                            serial_println!("[jobs] job {} -> Done", id);
                            return true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// send_ipp_request — IPP stub
// ---------------------------------------------------------------------------

/// Send an IPP/1.1 print request to a printer at `printer_ip`.
///
/// This is a stub that serialises key job attributes to the serial port.
/// A real implementation would:
///   1. Open a TCP socket to `printer_ip:631`.
///   2. Build an IPP/1.1 `Print-Job` request header.
///   3. Append `job.data[..job.data_len]` as the document body.
///   4. Read and parse the IPP response.
pub fn send_ipp_request(printer_ip: [u8; 4], job: &PrintJob) {
    serial_println!(
        "[jobs] IPP Print-Job  printer={}.{}.{}.{}:631  job_id={}  pages={}  copies={}  \
         data_len={}  status={:?}  [stub: would transmit via TCP]",
        printer_ip[0],
        printer_ip[1],
        printer_ip[2],
        printer_ip[3],
        job.job_id,
        job.pages,
        job.copies,
        job.data_len,
        job.status
    );
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Return the current status of job `id`, or `None` if not found.
pub fn job_status(id: u32) -> Option<PrintStatus> {
    unsafe {
        for slot in PRINT_QUEUE.iter() {
            if let Some(ref job) = slot {
                if job.job_id == id {
                    return Some(job.status);
                }
            }
        }
    }
    None
}

/// Return the number of occupied queue slots.
pub fn queue_depth() -> usize {
    let mut count = 0usize;
    unsafe {
        for slot in PRINT_QUEUE.iter() {
            if slot.is_some() {
                count += 1;
            }
        }
    }
    count
}

/// Remove all `Done` and `Failed` jobs, freeing their slots.
///
/// Returns the number of slots reclaimed.
pub fn gc_completed_jobs() -> usize {
    let mut freed = 0usize;
    unsafe {
        for slot in PRINT_QUEUE.iter_mut() {
            if let Some(ref job) = slot {
                if job.status == PrintStatus::Done || job.status == PrintStatus::Failed {
                    *slot = None;
                    freed += 1;
                }
            }
        }
    }
    serial_println!("[jobs] gc_completed_jobs: freed={}", freed);
    freed
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[jobs] print job queue ready (slots={})", PRINT_QUEUE_SIZE);
}
