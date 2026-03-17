/// Linux/POSIX compatibility layer
///
/// Part of the AIOS.

pub mod posix;
pub mod linux_abi;
pub mod elf_compat;
pub mod termios;
pub mod ioctl;
pub mod ioctl_compat;
pub mod proc_compat;
pub mod signal_compat;
pub mod fcntl;
pub mod errno;
pub mod mman;

use crate::serial_println;

pub fn init() {
    posix::init();
    linux_abi::init();
    termios::init();
    signal_compat::init();
    proc_compat::init();
    elf_compat::init();
    ioctl::init();
    ioctl_compat::init();
    fcntl::init();
    errno::init();
    mman::init();
    serial_println!("  compat: initialized (POSIX, Linux ABI, ELF, termios, signals, proc, ioctl, fcntl, errno, mman)");
}
