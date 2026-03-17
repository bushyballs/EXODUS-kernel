/// execve implementation — replace process image, handle #! scripts.
///
/// Part of the AIOS kernel.
use alloc::string::String;
use alloc::vec::Vec;

/// User stack base for new processes (top of low canonical user space).
/// 8 pages (32 KiB) are allocated below this address.
const USER_STACK_TOP: usize = 0x0000_7FFF_0000_0000;

/// Number of pages to allocate for the user stack (8 pages = 32 KiB).
const USER_STACK_PAGES: usize = 8;

/// Parsed executable header (ELF or script).
pub enum ExecFormat {
    Elf,
    Script {
        interpreter: String,
        arg: Option<String>,
    },
}

/// Detect the executable format from the first bytes.
pub fn detect_format(header: &[u8]) -> Result<ExecFormat, &'static str> {
    if header.len() >= 4
        && header[0] == 0x7F
        && header[1] == b'E'
        && header[2] == b'L'
        && header[3] == b'F'
    {
        return Ok(ExecFormat::Elf);
    }
    if header.len() >= 2 && header[0] == b'#' && header[1] == b'!' {
        // Parse the interpreter path from the shebang line.
        let line_end = header
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(header.len());
        // Collect the shebang payload (after '#!'), skip leading whitespace.
        let payload: Vec<u8> = header[2..line_end]
            .iter()
            .skip_while(|&&b| b == b' ' || b == b'\t')
            .cloned()
            .collect();

        // Split at the first space to separate interpreter from optional arg.
        let space_pos = payload.iter().position(|&b| b == b' ');
        let (interp_bytes, arg_bytes_opt) = match space_pos {
            Some(pos) => (payload[..pos].to_vec(), Some(payload[pos + 1..].to_vec())),
            None => (payload, None),
        };

        let interpreter = String::from(core::str::from_utf8(&interp_bytes).unwrap_or("/bin/sh"));
        let arg = arg_bytes_opt.and_then(|bytes| {
            // Trim trailing whitespace/CR from the argument.
            let trimmed: Vec<u8> = bytes
                .iter()
                .take_while(|&&c| c != b' ' && c != b'\t' && c != b'\r' && c != b'\n')
                .cloned()
                .collect();
            if trimmed.is_empty() {
                None
            } else {
                core::str::from_utf8(&trimmed).ok().map(String::from)
            }
        });
        return Ok(ExecFormat::Script { interpreter, arg });
    }
    Err("exec: unrecognised binary format")
}

/// Execute a new program image in the current process.
///
/// Steps:
///   1. Read file bytes from the VFS.
///   2. Detect format (ELF64 or #! script).
///   3. For scripts: re-exec with the interpreter.
///   4. For ELF: validate magic, class, endianness, and machine (x86-64).
///   5. Reject dynamically-linked ELF (PT_INTERP / PT_DYNAMIC present).
///   6. Load all PT_LOAD segments via `elf::load()` (allocates frames,
///      maps pages, copies file data, zeroes BSS).
///   7. Allocate a user stack (USER_STACK_PAGES pages below USER_STACK_TOP).
///   8. Push argv, envp, and System V AMD64 auxiliary vector onto the stack.
///   9. Update the current process's context (RIP = entry, RSP = new stack).
///  10. Return Ok(()) — the caller's iretq path will jump to user space.
pub fn do_execve(path: &str, argv: &[&str], envp: &[&str]) -> Result<(), &'static str> {
    crate::serial_println!("exec: do_execve(\"{}\")", path);

    // ── Step 1: read the file ────────────────────────────────────────────────
    let file_bytes = crate::fs::vfs::fs_read(path).map_err(|_| "exec: cannot open file")?;

    if file_bytes.len() < 4 {
        return Err("exec: file too small");
    }

    // ── Step 2: detect format ────────────────────────────────────────────────
    let format = detect_format(&file_bytes)?;

    match format {
        ExecFormat::Script { interpreter, arg } => {
            // Build a new argv: [interpreter, optional_arg, path, original_argv[1..]]
            crate::serial_println!("exec: script interpreter=\"{}\"", interpreter);
            let mut new_argv: Vec<&str> = Vec::new();
            let interp_str: &str = &interpreter;
            new_argv.push(interp_str);
            let arg_str_storage;
            if let Some(ref a) = arg {
                arg_str_storage = a.as_str();
                // We cannot push arg_str_storage here because it has a shorter
                // lifetime than file_bytes.  For scripts we fall back to
                // a tail-recursive execve with the interpreter as path.
                let _ = arg_str_storage;
            }
            // Re-invoke do_execve with the interpreter binary.
            // argv[0] = interpreter, argv[1] = original path, rest = original argv[1..]
            let mut rebuilt: Vec<String> = Vec::new();
            rebuilt.push(interpreter.clone());
            if let Some(a) = arg {
                rebuilt.push(a);
            }
            rebuilt.push(String::from(path));
            for a in argv.iter().skip(1) {
                rebuilt.push(String::from(*a));
            }
            let refs: Vec<&str> = rebuilt.iter().map(|s| s.as_str()).collect();
            return do_execve(interpreter.as_str(), &refs, envp);
        }

        ExecFormat::Elf => {
            // ── Step 3 & 4: ELF validation ───────────────────────────────────
            // Magic already confirmed by detect_format.
            // Check class (ELF64 = 2) at byte 4.
            if file_bytes[4] != 2 {
                return Err("exec: not an ELF64 binary");
            }
            // Check data encoding (little-endian = 1) at byte 5.
            if file_bytes[5] != 1 {
                return Err("exec: ELF is not little-endian");
            }

            // Parse the ELF header to check e_machine.
            if file_bytes.len() < 64 {
                return Err("exec: ELF header truncated");
            }
            // e_machine is at offset 18 (u16 LE).
            let e_machine = u16::from_le_bytes([file_bytes[18], file_bytes[19]]);
            if e_machine != 0x3E {
                return Err("exec: ELF is not x86-64 (e_machine != 0x3E)");
            }

            // ── Step 5: reject dynamic/interpreted ELF ───────────────────────
            // Parse program headers to look for PT_DYNAMIC (2) or PT_INTERP (3).
            let e_phoff =
                u64::from_le_bytes(file_bytes[32..40].try_into().unwrap_or([0u8; 8])) as usize;
            let e_phentsize = u16::from_le_bytes([file_bytes[54], file_bytes[55]]) as usize;
            let e_phnum = u16::from_le_bytes([file_bytes[56], file_bytes[57]]) as usize;

            for i in 0..e_phnum {
                let ph_off = e_phoff + i * e_phentsize;
                if ph_off + 4 > file_bytes.len() {
                    break;
                }
                let p_type = u32::from_le_bytes([
                    file_bytes[ph_off],
                    file_bytes[ph_off + 1],
                    file_bytes[ph_off + 2],
                    file_bytes[ph_off + 3],
                ]);
                if p_type == 2 || p_type == 3 {
                    crate::serial_println!(
                        "exec: dynamic linking not supported (PT_DYNAMIC/PT_INTERP in \"{}\")",
                        path
                    );
                    return Err("exec: dynamic linking not supported");
                }
            }

            // ── Step 6: load ELF segments ────────────────────────────────────
            let load_result =
                crate::process::elf::load(&file_bytes).map_err(|_| "exec: ELF load failed")?;

            // ── Step 7: allocate user stack ──────────────────────────────────
            // Allocate USER_STACK_PAGES pages ending at USER_STACK_TOP.
            let stack_size = USER_STACK_PAGES * 4096;
            let stack_base = USER_STACK_TOP.saturating_sub(stack_size);

            // Map writable, non-execute user-accessible pages for the stack.
            use crate::memory::{frame_allocator, paging};
            let stack_flags = paging::flags::USER_ACCESSIBLE
                | paging::flags::WRITABLE
                | paging::flags::NO_EXECUTE;

            for page in (stack_base..USER_STACK_TOP).step_by(4096) {
                let frame = frame_allocator::allocate_frame()
                    .ok_or("exec: stack frame allocation failed")?;
                unsafe {
                    core::ptr::write_bytes(frame.addr as *mut u8, 0, 4096);
                }
                paging::map_page(page, frame.addr, stack_flags)
                    .map_err(|_| "exec: stack page mapping failed")?;
            }

            // ── Step 8: build and push argv, envp, auxv ──────────────────────
            let auxv = crate::process::elf::build_auxv(&load_result, 0, 0);
            let new_sp =
                unsafe { crate::process::elf::setup_user_stack(USER_STACK_TOP, argv, envp, &auxv) };

            // ── Step 9: update process context ───────────────────────────────
            let pid = crate::process::scheduler::SCHEDULER.lock().current();
            {
                let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_mut() {
                    // POSIX exec: reset signal handlers, close CLOEXEC fds.
                    proc.prepare_exec();

                    // Update argv stored in PCB.
                    proc.argv.clear();
                    for a in argv {
                        proc.argv.push(String::from(*a));
                    }

                    // Set the name to basename of path.
                    let name = if let Some(slash) = path.rfind('/') {
                        &path[slash + 1..]
                    } else {
                        path
                    };
                    proc.name = String::from(name);

                    // Program executes in ring 3.
                    proc.context.rip = load_result.entry as u64;
                    proc.context.rsp = new_sp as u64;
                    proc.context.rflags = 0x200; // IF = 1 (interrupts enabled)
                    proc.context.cs = 0x23; // user code segment (RPL 3)
                    proc.context.ss = 0x1b; // user data segment (RPL 3)
                    proc.is_kernel = false;
                }
            }

            crate::serial_println!(
                "exec: \"{}\" loaded — entry={:#x} rsp={:#x}",
                path,
                load_result.entry,
                new_sp
            );

            // ── Step 10: caller performs iretq to user space ─────────────────
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Binary format handler registry
// ---------------------------------------------------------------------------

/// Registered binary format handler (binfmt).
struct BinfmtHandler {
    /// Short name for this handler (e.g. "elf64", "script").
    name: &'static str,
    /// Magic bytes to match at offset 0.
    magic: &'static [u8],
    /// Offset of the magic bytes in the file.
    magic_offset: usize,
}

/// Static registry of binary format handlers.
///
/// Handlers are checked in registration order; the first match wins.
static mut BINFMT_HANDLERS: [Option<BinfmtHandler>; 8] =
    [None, None, None, None, None, None, None, None];
static mut BINFMT_COUNT: usize = 0;

/// Register a binary format handler.
///
/// # Safety
/// Must be called from a single-threaded init context before any
/// `do_execve` call.
pub unsafe fn register_binfmt(name: &'static str, magic: &'static [u8], offset: usize) {
    if BINFMT_COUNT < BINFMT_HANDLERS.len() {
        BINFMT_HANDLERS[BINFMT_COUNT] = Some(BinfmtHandler {
            name,
            magic,
            magic_offset: offset,
        });
        BINFMT_COUNT += 1;
        crate::serial_println!("exec: registered binfmt handler \"{}\"", name);
    }
}

/// Initialize the exec subsystem.
///
/// Registers the built-in ELF64 and #!-script binary format handlers.
pub fn init() {
    // ELF magic: 0x7F 'E' 'L' 'F'
    static ELF_MAGIC: &[u8] = &[0x7F, b'E', b'L', b'F'];
    // Script shebang: '#' '!'
    static SHEBANG_MAGIC: &[u8] = &[b'#', b'!'];

    unsafe {
        register_binfmt("elf64", ELF_MAGIC, 0);
        register_binfmt("script", SHEBANG_MAGIC, 0);
    }
    crate::serial_println!("exec: subsystem initialized (ELF64 + script handlers registered)");
}
