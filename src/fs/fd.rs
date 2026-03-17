use super::vfs::{FileOps, FsError};
use crate::sync::Mutex;
/// File descriptor table for Genesis
///
/// Each process has a file descriptor table mapping integers (fd 0, 1, 2, ...)
/// to open files. This is the POSIX file descriptor interface.
///
/// fd 0 = stdin, fd 1 = stdout, fd 2 = stderr (by convention)
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Maximum file descriptors per process
pub const MAX_FDS: usize = 256;

#[derive(Clone)]
/// An open file descriptor
pub struct FileDescriptor {
    /// The file operations (from the inode)
    pub ops: Arc<dyn FileOps>,
    /// Current read/write offset (shared across dup2 aliases)
    pub offset: Arc<Mutex<u64>>,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
    pub flags: u32,
}

impl FileDescriptor {
    pub fn new(ops: Box<dyn FileOps>, flags: u32) -> Self {
        FileDescriptor {
            ops: ops.into(),
            offset: Arc::new(Mutex::new(0)),
            flags,
        }
    }

    /// Read from this file descriptor
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut offset = self.offset.lock();
        let bytes_read = self.ops.read(*offset, buf)?;
        *offset += bytes_read as u64;
        Ok(bytes_read)
    }

    /// Write to this file descriptor
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, FsError> {
        let mut offset = self.offset.lock();
        let bytes_written = self.ops.write(*offset, buf)?;
        *offset += bytes_written as u64;
        Ok(bytes_written)
    }

    /// Seek to a position
    pub fn seek(&mut self, offset: u64) {
        *self.offset.lock() = offset;
    }
}

/// File descriptor table for a process
pub struct FileDescriptorTable {
    fds: Vec<Option<FileDescriptor>>,
}

impl FileDescriptorTable {
    /// Create a new FD table with stdin/stdout/stderr preallocated
    pub fn new() -> Self {
        let mut fds = Vec::with_capacity(MAX_FDS);
        // Reserve slots 0, 1, 2 for stdin/stdout/stderr
        // They'll be set up when the process is created
        fds.push(None); // fd 0: stdin
        fds.push(None); // fd 1: stdout
        fds.push(None); // fd 2: stderr
        FileDescriptorTable { fds }
    }

    /// Allocate the lowest available file descriptor
    pub fn alloc(&mut self, fd_entry: FileDescriptor) -> Result<usize, FsError> {
        // Find the first None slot
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(fd_entry);
                return Ok(i);
            }
        }

        // No free slot — extend if possible
        if self.fds.len() < MAX_FDS {
            let fd_num = self.fds.len();
            self.fds.push(Some(fd_entry));
            Ok(fd_num)
        } else {
            Err(FsError::TooManyOpenFiles)
        }
    }

    /// Get a mutable reference to an open file descriptor
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FileDescriptor> {
        self.fds.get_mut(fd)?.as_mut()
    }

    /// Get a reference to an open file descriptor
    pub fn get(&self, fd: usize) -> Option<&FileDescriptor> {
        self.fds.get(fd)?.as_ref()
    }

    /// Close a file descriptor
    pub fn close(&mut self, fd: usize) -> Result<(), FsError> {
        if fd >= self.fds.len() {
            return Err(FsError::InvalidArgument);
        }
        self.fds[fd] = None;
        Ok(())
    }

    /// Duplicate a file descriptor to a specific number (for dup2)
    pub fn dup2(&mut self, old_fd: usize, new_fd: usize) -> Result<usize, FsError> {
        if old_fd >= self.fds.len() || self.fds[old_fd].is_none() {
            return Err(FsError::InvalidArgument);
        }

        if old_fd == new_fd {
            return Ok(new_fd);
        }

        // Close the target if it's open
        if new_fd < self.fds.len() && self.fds[new_fd].is_some() {
            self.fds[new_fd] = None;
        }

        // Extend if needed
        while self.fds.len() <= new_fd {
            self.fds.push(None);
        }

        // Duplicate descriptor entry. ops and offset are shared via Arc.
        let entry = self.fds[old_fd]
            .as_ref()
            .cloned()
            .ok_or(FsError::InvalidArgument)?;
        self.fds[new_fd] = Some(entry);
        Ok(new_fd)
    }
}
