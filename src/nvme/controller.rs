//! NVMe Controller Driver
//!
//! High-level NVMe controller management with I/O queue support.

use super::commands::{
    AdminOpcode, CompletionQueueEntry, IdentifyController, IdentifyNamespace, IoOpcode,
    SubmissionQueueEntry,
};
use super::queue::{CommandIdAllocator, QueuePair};
use super::registers::NvmeRegisters;
use super::{NvmeError, Result};
use crate::dma::DmaRegion;
use crate::pci::PciDevice;

/// Maximum number of I/O queue pairs
const MAX_IO_QUEUES: usize = 4;

/// Default queue size
const QUEUE_SIZE: u16 = 64;

/// Command timeout in milliseconds
const COMMAND_TIMEOUT_MS: u32 = 5000;

/// NVMe Controller
pub struct NvmeController {
    pub pci_device: PciDevice,
    pub registers: NvmeRegisters,
    pub admin_queue: QueuePair,
    pub io_queues: [Option<QueuePair>; MAX_IO_QUEUES],
    pub io_queue_count: usize,
    pub command_id: CommandIdAllocator,
    pub identify_data: IdentifyController,
    pub namespace_count: u32,
    pub namespaces: [Option<NamespaceInfo>; 16],
}

/// Namespace Information
#[derive(Debug, Clone, Copy)]
pub struct NamespaceInfo {
    pub nsid: u32,
    pub block_size: usize,
    pub block_count: u64,
    pub capacity_bytes: u64,
}

impl NvmeController {
    /// Initialize a new NVMe controller
    pub fn new(pci_device: PciDevice) -> Result<Self> {
        // Get BAR0 (controller registers)
        let bar0 = pci_device.get_bar(0).ok_or(NvmeError::InvalidBar)?;

        // Enable PCI bus mastering and memory space
        pci_device.enable_bus_mastering();
        pci_device.enable_memory_space();
        pci_device.disable_legacy_interrupts();

        // Create register interface
        let registers = NvmeRegisters::new(bar0);

        // Reset controller
        Self::reset_controller(&registers)?;

        // Configure controller
        registers.configure_standard();

        // Create admin queue pair
        let admin_queue = QueuePair::new(0, QUEUE_SIZE)?;

        // Set admin queue addresses
        registers.set_asq(admin_queue.submission.phys_addr());
        registers.set_acq(admin_queue.completion.phys_addr());

        // Set admin queue size (0-based, so subtract 1)
        registers.set_aqa(QUEUE_SIZE - 1, QUEUE_SIZE - 1);

        // Enable controller
        registers.enable();

        // Wait for controller to become ready
        Self::wait_ready(&registers)?;

        let mut controller = NvmeController {
            pci_device,
            registers,
            admin_queue,
            io_queues: [None, None, None, None],
            io_queue_count: 0,
            command_id: CommandIdAllocator::new(),
            identify_data: IdentifyController::new(),
            namespace_count: 0,
            namespaces: [None; 16],
        };

        // Identify controller
        controller.identify_controller()?;

        // Enumerate namespaces
        controller.enumerate_namespaces()?;

        // Create I/O queues
        controller.create_io_queues(MAX_IO_QUEUES)?;

        Ok(controller)
    }

    /// Reset the controller
    fn reset_controller(registers: &NvmeRegisters) -> Result<()> {
        // Disable controller
        registers.disable();

        // Wait for controller to stop
        let timeout = 10000; // 10 seconds
        for _ in 0..timeout {
            if !registers.is_ready() {
                return Ok(());
            }
            Self::delay_ms(1);
        }

        Err(NvmeError::Timeout)
    }

    /// Wait for controller to become ready
    fn wait_ready(registers: &NvmeRegisters) -> Result<()> {
        // Controller Ready timeout (CAP.TO * 500ms)
        let timeout_500ms = registers.timeout() as u32;
        let timeout_ms = if timeout_500ms > 0 {
            timeout_500ms * 500
        } else {
            5000 // Default 5 seconds
        };

        for _ in 0..(timeout_ms / 10) {
            if registers.is_ready() {
                return Ok(());
            }

            // Check for fatal status
            if registers.controller_fatal_status() {
                return Err(NvmeError::InitializationFailed);
            }

            Self::delay_ms(10);
        }

        Err(NvmeError::Timeout)
    }

    /// Identify controller
    fn identify_controller(&mut self) -> Result<()> {
        // Allocate buffer for identify data
        let mut buffer = DmaRegion::allocate(4096, 4096)
            .ok_or(NvmeError::InitializationFailed)?;
        buffer.zero();

        // Create Identify command (CNS=1 for controller)
        let cmd_id = self.command_id.allocate();
        let cmd = SubmissionQueueEntry::identify(cmd_id, 0x01, buffer.phys_addr());

        // Submit and wait for completion
        self.admin_queue
            .submit_and_wait(&cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

        // Copy identify data
        unsafe {
            let src = buffer.as_ptr::<IdentifyController>();
            self.identify_data = *src;
        }

        // Get namespace count
        self.namespace_count = self.identify_data.nn;

        buffer.free();
        Ok(())
    }

    /// Enumerate namespaces
    fn enumerate_namespaces(&mut self) -> Result<()> {
        for nsid in 1..=self.namespace_count.min(16) {
            if let Ok(ns_info) = self.identify_namespace(nsid) {
                self.namespaces[(nsid - 1) as usize] = Some(ns_info);
            }
        }

        Ok(())
    }

    /// Identify namespace
    fn identify_namespace(&self, nsid: u32) -> Result<NamespaceInfo> {
        // Allocate buffer for identify data
        let mut buffer = DmaRegion::allocate(4096, 4096)
            .ok_or(NvmeError::InitializationFailed)?;
        buffer.zero();

        // Create Identify command (CNS=0 for namespace)
        let cmd_id = self.command_id.allocate();
        let mut cmd = SubmissionQueueEntry::identify(cmd_id, 0x00, buffer.phys_addr());
        cmd.set_namespace(nsid);

        // Submit and wait for completion
        self.admin_queue
            .submit_and_wait(&cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

        // Parse identify data
        let ns_data = unsafe { *(buffer.as_ptr::<IdentifyNamespace>()) };

        let block_size = ns_data.block_size();
        let block_count = ns_data.ncap;
        let capacity_bytes = ns_data.capacity();

        buffer.free();

        Ok(NamespaceInfo {
            nsid,
            block_size,
            block_count,
            capacity_bytes,
        })
    }

    /// Create I/O queue pairs
    fn create_io_queues(&mut self, count: usize) -> Result<()> {
        for i in 0..count.min(MAX_IO_QUEUES) {
            let queue_id = (i + 1) as u16; // Queue ID 0 is admin queue

            // Create queue pair
            let queue_pair = QueuePair::new(queue_id, QUEUE_SIZE)?;

            // Create I/O Completion Queue
            let cmd_id = self.command_id.allocate();
            let create_cq_cmd = SubmissionQueueEntry::create_io_cq(
                cmd_id,
                queue_id,
                QUEUE_SIZE - 1,
                queue_pair.completion.phys_addr(),
                queue_id, // Interrupt vector (same as queue ID)
            );

            self.admin_queue
                .submit_and_wait(&create_cq_cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

            // Create I/O Submission Queue
            let cmd_id = self.command_id.allocate();
            let create_sq_cmd = SubmissionQueueEntry::create_io_sq(
                cmd_id,
                queue_id,
                QUEUE_SIZE - 1,
                queue_pair.submission.phys_addr(),
                queue_id, // Associated CQ ID
            );

            self.admin_queue
                .submit_and_wait(&create_sq_cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

            // Store queue pair
            self.io_queues[i] = Some(queue_pair);
            self.io_queue_count = self.io_queue_count.saturating_add(1);
        }

        Ok(())
    }

    /// Read blocks from namespace
    pub fn read_blocks(
        &self,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer: &mut [u8],
    ) -> Result<()> {
        // Validate namespace
        if namespace_id == 0 || namespace_id > self.namespace_count {
            return Err(NvmeError::InvalidNamespace);
        }

        let ns_info = self.namespaces[(namespace_id - 1) as usize]
            .ok_or(NvmeError::InvalidNamespace)?;

        // Validate buffer size
        let required_size = (block_count as usize) * ns_info.block_size;
        if buffer.len() < required_size {
            return Err(NvmeError::IoError);
        }

        // Allocate DMA buffer
        let mut dma_buffer = DmaRegion::allocate(required_size, 4096)
            .ok_or(NvmeError::InitializationFailed)?;

        // Create Read command
        let cmd_id = self.command_id.allocate();
        let cmd = SubmissionQueueEntry::read(
            cmd_id,
            namespace_id,
            start_lba,
            block_count - 1, // 0-based
            dma_buffer.phys_addr(),
        );

        // Get I/O queue (use first queue for now)
        let io_queue = self.io_queues[0].as_ref().ok_or(NvmeError::QueueFull)?;

        // Submit and wait
        io_queue.submit_and_wait(&cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

        // Copy data from DMA buffer to user buffer
        unsafe {
            core::ptr::copy_nonoverlapping(
                dma_buffer.as_ptr::<u8>(),
                buffer.as_mut_ptr(),
                required_size,
            );
        }

        dma_buffer.free();
        Ok(())
    }

    /// Write blocks to namespace
    pub fn write_blocks(
        &self,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer: &[u8],
    ) -> Result<()> {
        // Validate namespace
        if namespace_id == 0 || namespace_id > self.namespace_count {
            return Err(NvmeError::InvalidNamespace);
        }

        let ns_info = self.namespaces[(namespace_id - 1) as usize]
            .ok_or(NvmeError::InvalidNamespace)?;

        // Validate buffer size
        let required_size = (block_count as usize) * ns_info.block_size;
        if buffer.len() < required_size {
            return Err(NvmeError::IoError);
        }

        // Allocate DMA buffer
        let mut dma_buffer = DmaRegion::allocate(required_size, 4096)
            .ok_or(NvmeError::InitializationFailed)?;

        // Copy data to DMA buffer
        unsafe {
            core::ptr::copy_nonoverlapping(
                buffer.as_ptr(),
                dma_buffer.as_ptr::<u8>(),
                required_size,
            );
        }

        // Create Write command
        let cmd_id = self.command_id.allocate();
        let cmd = SubmissionQueueEntry::write(
            cmd_id,
            namespace_id,
            start_lba,
            block_count - 1, // 0-based
            dma_buffer.phys_addr(),
        );

        // Get I/O queue
        let io_queue = self.io_queues[0].as_ref().ok_or(NvmeError::QueueFull)?;

        // Submit and wait
        io_queue.submit_and_wait(&cmd, &self.registers, COMMAND_TIMEOUT_MS)?;

        dma_buffer.free();
        Ok(())
    }

    /// Submit async read (non-blocking)
    pub fn read_blocks_async(
        &self,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer_phys_addr: u64,
    ) -> Result<u16> {
        // Create Read command
        let cmd_id = self.command_id.allocate();
        let cmd = SubmissionQueueEntry::read(
            cmd_id,
            namespace_id,
            start_lba,
            block_count - 1,
            buffer_phys_addr,
        );

        // Get I/O queue
        let io_queue = self.io_queues[0].as_ref().ok_or(NvmeError::QueueFull)?;

        // Submit without waiting
        io_queue.submit_async(&cmd, &self.registers)
    }

    /// Submit async write (non-blocking)
    pub fn write_blocks_async(
        &self,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer_phys_addr: u64,
    ) -> Result<u16> {
        // Create Write command
        let cmd_id = self.command_id.allocate();
        let cmd = SubmissionQueueEntry::write(
            cmd_id,
            namespace_id,
            start_lba,
            block_count - 1,
            buffer_phys_addr,
        );

        // Get I/O queue
        let io_queue = self.io_queues[0].as_ref().ok_or(NvmeError::QueueFull)?;

        // Submit without waiting
        io_queue.submit_async(&cmd, &self.registers)
    }

    /// Poll for I/O completion
    pub fn poll_completion(&self, queue_index: usize) -> Option<CompletionQueueEntry> {
        if queue_index >= self.io_queue_count {
            return None;
        }

        if let Some(ref queue) = self.io_queues[queue_index] {
            queue.poll(&self.registers)
        } else {
            None
        }
    }

    /// Get controller information
    pub fn get_info<'a>(&'a self) -> ControllerInfo<'a> {
        ControllerInfo {
            vendor_id: self.identify_data.vid,
            model: self.identify_data.model_number(),
            serial: self.identify_data.serial_number(),
            firmware: self.identify_data.firmware_revision(),
            namespace_count: self.namespace_count,
        }
    }

    /// Get namespace information
    pub fn get_namespace_info(&self, namespace_id: u32) -> Option<NamespaceInfo> {
        if namespace_id == 0 || namespace_id > self.namespace_count {
            return None;
        }

        self.namespaces[(namespace_id - 1) as usize]
    }

    /// Simple delay (busy wait)
    fn delay_ms(ms: u32) {
        for _ in 0..(ms * 10000) {
            unsafe {
                core::arch::asm!("pause");
            }
        }
    }
}

/// Controller Information
#[derive(Debug)]
pub struct ControllerInfo<'a> {
    pub vendor_id: u16,
    pub model: &'a str,
    pub serial: &'a str,
    pub firmware: &'a str,
    pub namespace_count: u32,
}
