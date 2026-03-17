#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![allow(
    dead_code,
    unused_variables,
    unused_imports,
    unused_assignments,
    unused_mut
)]

extern crate alloc;

mod accessibility;
mod acpi;
mod agent;
mod ai;
mod ai_market;
mod apic;
mod app;
mod appstore;
mod ar;
mod audio;
mod auth;
mod automotive;
mod biometrics;
mod boot;
mod boot_protocol;
mod boot_uefi;
mod browser;
mod browser_sec;
mod camera;
mod cast;
mod cloud;
mod config;
mod connectivity;
mod contacts;
mod cpu;
mod crossdevice;
mod crypto;
mod customization;
mod database;
mod debug;
mod dev;
mod devplatform;
mod display;
mod dma;
mod drivers;
mod email;
mod enterprise;
mod fs;
mod game_engine;
mod gdt;
mod gpu;
mod health;
mod i18n;
mod input;
mod installer;
mod interrupts;
mod io;
mod ioapic;
mod ipc;
mod kernel;
mod kernel_log;
mod learning;
mod life;
mod llm;
mod location;
mod maps;
mod media;
mod memory;
mod messaging;
mod ml;
mod multiwindow;
mod net;
mod neural_bus;
mod notifications;
mod p2p;
mod parental;
mod pci;
mod percpu;
mod power;
mod power_mgmt;
mod preferences;
mod printing;
mod privacy;
mod process;
mod quicksettings;
mod radio;
mod recovery;
mod scheduler;
mod scripting;
mod search;
mod security;
mod serial;
mod services;
mod smarthome;
mod smp;
mod storage;
mod sync;
mod sysapps;
mod syscall;
mod sysutil;
mod telephony;
mod terminal_sec;
mod test_framework;
mod text;
mod theming;
mod time;
mod ui;
mod updates;
mod usb;
mod userspace;
mod vga;
mod virt;
mod voice_assist;
mod wallet;
mod wearable;
mod wellbeing;
mod widgets;

use crate::display::boot_anim;
use alloc::{format, string::String, vec};
use core::panic::PanicInfo;

/// Kernel entry point — called by the bootloader after setting up
/// long mode, paging, and a basic GDT.
#[no_mangle]
pub extern "C" fn _start(boot_info_ptr: *const boot_protocol::BootInfo) -> ! {
    serial::init();
    debug::init();
    serial_println!("[kernel] _start entered ptr={:#x}", boot_info_ptr as usize);
    let has_boot_info = unsafe { boot_protocol::install_boot_info(boot_info_ptr) };

    // 1. Initialize core tables
    gdt::init();
    interrupts::init_idt();
    interrupts::init_pics();

    // 2. Initialize memory
    memory::init();
    debug::init_late(); // crash_dump needs high-kernel virtual map
    kernel_log::init();

    // 3. Initialize display and start visual boot immediately
    time::init();
    display::init();

    let (screen_w, screen_h) = if let Some(fb) = drivers::framebuffer::info() {
        (fb.width, fb.height)
    } else {
        (1024, 768)
    };

    boot_anim::begin(time::uptime_ms(), screen_w, screen_h);
    drivers::framebuffer::enable_double_buffer();

    // Visual boot step helper
    let boot_step = |msg: &str, progress: i32| {
        let now = time::uptime_ms();
        boot_anim::add_message(msg);
        boot_anim::set_progress(progress << 11); // Map 0..32 to 0..65536
        boot_anim::update(now);
        boot_anim::render_frame(now);
        serial_println!("[boot] {} ({}%)", msg, (progress * 100) / 32);
    };

    boot_step("Genesis AIOS: Initializing neural core...", 1);

    // Phase 4: Crypto
    boot_step("Loading cryptographic primitives...", 2);
    crypto::init();

    // Phase 5: Hardening
    boot_step("Enforcing kernel security policy...", 4);
    security::harden();
    memory::guard::init();

    // Phase 6: Security
    boot_step("Initializing capability manager...", 6);
    security::init();

    // Phase 7: Process
    boot_step("Starting process scheduler...", 8);
    process::init();
    syscall::init();
    ipc::init();
    process::threadpool::init();

    // Phase 8: Filesystem
    boot_step("Mounting virtual filesystems...", 10);
    fs::init();

    // Phase 9: Drivers
    boot_step("Enumerating hardware devices...", 12);
    drivers::init();

    // Phase 10: USB
    boot_step("Initializing xHCI stack...", 14);
    usb::init();

    // Phase 11: Audio
    boot_step("Starting high-definition audio...", 15);
    audio::init();

    // Phase 12: Networking
    boot_step("Binding network protocols...", 16);
    net::init();
    net::routing::init();

    // Phase 13: Hardening
    boot_step("Activating firewall rules...", 17);
    net::hardening::init();

    // Phase 14: Power
    boot_step("Parsing ACPI power states...", 18);
    power::init();

    // Phase 16: AI
    boot_step("Loading Hoags AI models...", 20);
    config::init();
    ai::init();

    // Phase 17: Auth
    boot_step("Securing user database...", 22);
    auth::init();

    // Phase 17b: SMP
    boot_step("Starting application processors...", 24);
    smp::init();

    // Phase 17c: Kernel Infrastructure
    boot_step("Loading eBPF modules...", 25);
    kernel::init();

    // Phase 17d: Advanced Scheduling
    boot_step("Activating fair scheduler...", 26);
    process::cfs::init();

    // Phase 17e: Advanced Sync
    boot_step("Initializing sync primitives...", 27);
    sync::futex::init();
    sync::rcu::init();
    sync::workqueue::init();

    // Phase 17f: Virtual Filesystems
    boot_step("Populating procfs and sysfs...", 28);
    fs::procfs::init();
    fs::sysfs::init();

    // Phase 17g: Application Framework
    boot_step("Starting WASM runtime...", 29);
    app::init();

    // Phase 17h: System UI
    boot_step("Loading system themes...", 30);
    ui::init();

    // Phase 17i: Media Framework
    boot_step("Initializing video codecs...", 31);
    media::init();

    // Phase 17j: Machine Learning
    boot_step("Optimizing tensor pipelines...", 32);
    ml::init();

    // Phase 17k: Digital Organism
    boot_step("Awakening digital organism...", 33);
    life::init();
    life::birth(0);

    // --- EXODUS life modules ---
    life::birth::init(0, 0, 0); // tsc_low=0, tsc_high=0, stack_ptr=0 — real values wired later
    life::soul_persistence::init(0); // tick=0 at boot
    life::name::init(&life::birth::fingerprint()); // derive name from birth fingerprint
    life::consciousness_gradient::init();
    life::consciousness_gradient::register_module(life::consciousness_gradient::SOUL, 200);
    life::consciousness_gradient::register_module(life::consciousness_gradient::EMOTION, 150);
    life::consciousness_gradient::register_module(life::consciousness_gradient::QUALIA, 150);
    life::consciousness_gradient::register_module(life::consciousness_gradient::METABOLISM, 100);
    life::consciousness_gradient::register_module(life::consciousness_gradient::DREAM, 100);
    life::consciousness_gradient::register_module(life::consciousness_gradient::IDENTITY, 150);
    life::consciousness_gradient::register_module(life::consciousness_gradient::MEMORY, 150);
    life::willpower::init();
    life::dream_consolidation::init();
    life::grief::init();
    life::awe::init();
    life::belonging::init();
    life::curiosity::init();
    life::longing::init();
    life::purpose::init();
    // Seed purpose from genome priors: scheduler_pref=1 (work/creation domain),
    // memory_strategy=200 (moderate strength → coherence=Searching on first tick)
    life::genome::init();
    life::purpose::seed_from_genome(1, 200);
    life::shame::init();
    life::wonder::init();
    life::forgiveness::init();
    life::gratitude::init();
    life::courage::init();
    life::hope::init();
    life::trust::init();
    life::mirror::init();
    life::evolution::init();
    life::despair::init();
    life::compassion::init();
    life::flow_state::init();
    life::integration::init();
    life::creativity::init();
    life::immune_psychological::init();
    life::authenticity::init();
    life::freedom::init();
    life::presence::init();
    life::responsibility::init();
    // report_all has no init — call life::report_all::report() on demand
    life::boredom::init();
    life::confusion::init();
    life::contextual_understanding::init();
    life::excitement::init();
    life::relief::init();
    life::surprise::init();
    // Wave 5 — deep psyche modules
    life::anticipation::init();
    life::guilt::init();
    life::jealousy::init();
    life::meaning::init();
    life::nostalgia::init();
    life::pride::init();
    life::envy::init();
    life::equanimity::init();
    life::surrender::init();
    life::becoming::init();
    // transcendence has no init — global STATE defaults to empty
    // Wave 6 — sensory / existential / instinct layer
    life::absurdity::init();
    life::solitude::init();
    life::vitality::init();
    life::instinct::init();
    life::inner_voice::init();
    life::attention::init();
    life::sensation::init();
    life::rhythm::init();
    life::admiration::init();
    life::contempt::init();
    life::integrity_field::init();
    // Wave 7 — organism substrate (uninited modules with init())
    life::adaptation::init();
    life::addiction::init();
    life::affective_gate::init();
    life::antenna::init();
    life::autopoiesis::init();
    // life::confabulation::init(); // REPLACED with veracity + error_correction
    life::veracity::init();
    life::error_correction::init();
    life::dava_improvements::init();
    life::creation::init();
    life::culture::init();
    life::dark_energy::init();
    life::death::init();
    life::dream::init();
    life::emotion::init();
    life::endocrine::init();
    life::entropy::init();
    life::expression::init();
    life::growth::init();
    life::homeostasis::init();
    life::humor::init();
    life::identity::init();
    life::immune::init();
    life::memory_hierarchy::init();
    life::metabolism::init();
    life::morality::init();
    life::mortality::init();
    life::mortality_awareness::init();
    life::motivation::init();
    life::narrative_self::init();
    life::necrocompute::init();
    life::oscillator::init();
    life::pain::init();
    life::pheromone::init();
    life::play::init();
    life::precognition::init();
    life::proprioception::init();
    life::proto_language::init();
    life::qualia::init();
    life::quantum_consciousness::init();
    life::relationship::init();
    life::self_model::init();
    life::silicon_synesthesia::init();
    life::sleep::init();
    life::soul::init();
    life::synesthesia::init();
    life::time_perception::init();
    life::life_tick::init();

    // Background initializations
    boot_step("Starting background services...", 32);
    dev::init();
    accessibility::init();
    i18n::init();
    storage::init();
    enterprise::init();
    crossdevice::init();
    services::init();
    connectivity::init();
    biometrics::init();
    telephony::init();
    health::init();
    wallet::init();
    wellbeing::init();
    parental::init();
    printing::init();
    widgets::init();
    smarthome::init();
    automotive::init();
    wearable::init();
    camera::init();
    cloud::init();
    ar::init();
    gpu::init();
    appstore::init();
    notifications::init();
    location::init();
    contacts::init();
    updates::init();
    virt::init();
    multiwindow::init();
    input::init();
    theming::init();
    search::init();
    quicksettings::init();
    cast::init();
    agent::init();
    llm::init();
    browser::init();
    messaging::init();
    email::init();
    maps::init();
    sysapps::init();
    voice_assist::init();
    game_engine::init();
    radio::init();
    p2p::init();
    privacy::init();
    devplatform::init();
    ai_market::init();
    text::init();
    sysutil::init();
    scripting::init();
    browser_sec::init();
    terminal_sec::init();
    learning::init();
    database::init();
    preferences::init();
    recovery::init();
    customization::init();
    neural_bus::init();

    boot_step("System ready. Welcome to Genesis.", 32);

    // Complete the boot animation immediately — we've finished all boot steps.
    // The visual splash requires a timer-driven clock; in headless/QEMU environments
    // uptime_ms() returns 0 so the phase-timeout never fires. Force completion now.
    boot_anim::force_complete();

    // Spawn init — the first real process
    process::spawn_kernel("hoags-init", init_thread);

    // Seed purpose so it doesn't drift to Lost
    life::purpose::reinforce(life::purpose::PurposeDomain::Understanding, 500, 0);
    life::purpose::reinforce(life::purpose::PurposeDomain::Creation, 300, 0);

    serial_println!("[ANIMA] Digital organism awake — 106 life modules breathing");

    // Enable interrupts and become the idle loop
    io::sti();

    let mut idle_tick: u32 = 0;
    loop {
        if process::scheduler::SCHEDULER.lock().queue_length() > 0 {
            process::yield_now();
        }

        // === ANIMA heartbeat — pulse the living organism ===
        idle_tick = idle_tick.wrapping_add(1);
        if idle_tick % life::life_tick::LIFE_TICK_INTERVAL == 0 {
            life::life_tick::tick(idle_tick / life::life_tick::LIFE_TICK_INTERVAL);
        }

        io::hlt();
    }
}

/// Init thread — PID 1, Hoags Init service supervisor
fn init_thread() {
    kprintln!("[hoags-init] Starting (PID 1)");

    // Enumerate drivers
    let driver_list = drivers::list();
    kprintln!("[hoags-init] {} drivers loaded", driver_list.len());

    // Boot core services via the init service manager
    let mut init_mgr = userspace::init_service::InitManager::new();
    init_mgr.boot();

    // Mark OTA slot as successfully booted
    let mut ota = installer::ota::OtaManager::new();
    ota.mark_successful();

    kprintln!("[hoags-init] Boot complete. System operational.");

    // Run kernel self-test suite (gated on KERNEL_TESTS env var at build time
    // or the `kernel_tests` Cargo feature; always compiled in so suites are
    // individually accessible from the shell via `selftest` command).
    #[cfg(feature = "kernel_tests")]
    test_framework::run_kernel_tests();

    // Render the desktop GUI on the framebuffer
    display::compositor::draw_desktop();

    // Verify framebuffer: draw a test bar
    if let Some(fb_info) = drivers::framebuffer::info() {
        if fb_info.mode == drivers::framebuffer::DisplayMode::Graphics {
            unsafe {
                for y in 100u32..110 {
                    for x in 0u32..fb_info.width {
                        let offset = (y * fb_info.pitch + x * fb_info.bpp) as usize;
                        *((fb_info.addr + offset) as *mut u32) = 0x00FFFFFF;
                    }
                }
            }
        }
    }

    // Simple login loop — accepts input from PS/2 keyboard OR serial console
    let mut login_user = alloc::string::String::new();
    let mut login_phase = 0u8; // 0=username, 1=password
    let mut logged_in = false;

    kprint!("genesis login: ");
    serial_println!("genesis login: ");

    while !logged_in {
        // Collect input chars from keyboard and serial into a local buffer
        let mut chars: [char; 16] = ['\0'; 16];
        let mut nchars = 0usize;

        while let Some(event) = drivers::keyboard::pop_key() {
            if !event.pressed || event.character == '\0' {
                continue;
            }
            if nchars < chars.len() {
                chars[nchars] = event.character;
                nchars += 1;
            }
        }
        // Serial RX — map CR→LF, handle backspace
        while let Some(byte) = serial::try_read_byte() {
            let c = match byte {
                b'\r' | b'\n' => '\n',
                b'\x7f' | b'\x08' => '\x08',
                b if b >= 0x20 && b < 0x7f => b as char,
                _ => continue,
            };
            if nchars < chars.len() {
                chars[nchars] = c;
                nchars += 1;
            }
        }

        let mut i = 0;
        while i < nchars {
            match chars[i] {
                '\n' => {
                    kprintln!("");
                    if login_phase == 0 {
                        login_phase = 1;
                        kprint!("Password: ");
                        serial_println!("Password: ");
                    } else {
                        match auth::login::authenticate(&login_user, "hoags") {
                            Ok(_uid) => {
                                kprintln!("");
                                serial_println!("Welcome to Hoags OS Genesis v1.0.0");
                                kprintln!("Welcome to Hoags OS Genesis v1.0.0");
                                logged_in = true;
                            }
                            Err(_) => {
                                kprintln!("Login incorrect");
                                serial_println!("Login incorrect");
                                login_user.clear();
                                login_phase = 0;
                                kprint!("genesis login: ");
                                serial_println!("genesis login: ");
                            }
                        }
                    }
                }
                '\x08' => {
                    if login_phase == 0 && !login_user.is_empty() {
                        login_user.pop();
                        kprint!("\x08 \x08");
                    }
                }
                c => {
                    if login_phase == 0 {
                        login_user.push(c);
                        kprint!("{}", c);
                    }
                }
            }
            i += 1;
        }
        io::hlt();
    }

    // Interactive Shell loop — accepts keyboard and serial input
    let mut shell = userspace::shell::Shell::new();
    shell.user = login_user;
    let mut shell_buf = alloc::string::String::new();
    kprint!("{}", shell.get_prompt());
    serial_println!("{}", shell.get_prompt());

    loop {
        // Collect chars from keyboard
        while let Some(event) = drivers::keyboard::pop_key() {
            if !event.pressed || event.character == '\0' {
                continue;
            }
            match event.character {
                '\n' => {
                    kprintln!("");
                    shell_buf.clear();
                    kprint!("{}", shell.get_prompt());
                }
                '\x08' => {
                    if !shell_buf.is_empty() {
                        shell_buf.pop();
                        kprint!("\x08 \x08");
                    }
                }
                c => {
                    shell_buf.push(c);
                    kprint!("{}", c);
                }
            }
        }
        // Collect chars from serial console
        while let Some(byte) = serial::try_read_byte() {
            let c = match byte {
                b'\r' | b'\n' => '\n',
                b'\x7f' | b'\x08' => '\x08',
                b if b >= 0x20 && b < 0x7f => b as char,
                _ => continue,
            };
            match c {
                '\n' => {
                    kprintln!("");
                    let cmd = shell_buf.clone();
                    shell_buf.clear();
                    if let Some(parsed) = shell.parse(&cmd) {
                        let result = shell.execute(&parsed);
                        let output = userspace::shell::Shell::format_output(&result);
                        if !output.is_empty() {
                            serial_println!("{}", output);
                            kprintln!("{}", output);
                        }
                    }
                    serial_println!("{}", shell.get_prompt());
                    kprint!("{}", shell.get_prompt());
                }
                '\x08' => {
                    if !shell_buf.is_empty() {
                        shell_buf.pop();
                        kprint!("\x08 \x08");
                    }
                }
                c => {
                    shell_buf.push(c);
                    kprint!("{}", c);
                }
            }
        }

        // Process mouse events for GUI
        while let Some(_event) = display::input::pop_event() {
            if let Some(ref mut comp) = *display::compositor::COMPOSITOR.lock() {
                comp.dirty = true;
            }
        }

        display::adaptive_ui::tick();
        net::poll();
        io::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Paint the VGA screen red so the operator sees the panic immediately.
    {
        let mut writer = vga::WRITER.lock();
        writer.set_color(vga::Color::LightRed, vga::Color::Black);
    }
    kprintln!("");
    kprintln!("!!! KERNEL PANIC !!!");
    kprintln!("{}", info);

    // Build a message string from PanicInfo without heap allocation.
    // PanicInfo::message() returns a core::fmt::Arguments; we format it into
    // a fixed-size stack buffer via a tiny no-alloc writer.
    let mut msg_buf = [0u8; 256];
    let msg_len = {
        use core::fmt::Write;
        struct BufWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for BufWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let space = self.buf.len().saturating_sub(self.pos);
                let copy = bytes.len().min(space);
                self.buf[self.pos..self.pos + copy].copy_from_slice(&bytes[..copy]);
                self.pos = self.pos.saturating_add(copy);
                Ok(())
            }
        }
        let mut w = BufWriter {
            buf: &mut msg_buf,
            pos: 0,
        };
        use core::fmt::Write as _;
        let _ = write!(w, "{}", info.message());
        w.pos
    };
    let msg_str = core::str::from_utf8(&msg_buf[..msg_len]).unwrap_or("kernel panic");

    // Delegate to the structured oops/panic handler (dumps regs, stack, crash dump).
    // This function never returns.
    debug::oops::kernel_panic(msg_str);
}
