/// Local APIC management: enable, EOI, ID, IPI primitives, APIC timer.
pub mod apic;
/// Linux-compatible kernel audit ring buffer.
pub mod audit;
/// ELF64 binary format parser and loader.
pub mod binfmt_elf;
pub mod bpf_map;
/// cgroup v2 unified hierarchy resource controller.
pub mod cgroup_v2;
pub mod cgroups;
/// ELF core dump: capture crash state, serialize PT_NOTE/PT_LOAD headers.
pub mod coredump;
/// CPU hot-plug/unplug framework: per-CPU state machine, online/offline transitions.
pub mod cpu_hotplug;
pub mod cpu_topology;
pub mod crash_dump;
pub mod device_model;
/// Kernel subsystems for Genesis
///
/// This module contains core kernel infrastructure that doesn't fit
/// neatly into other categories: eBPF VM, loadable modules, cgroups,
/// namespaces, device model, performance events, tracing, dynamic
/// probing, crash dump, CPU/memory hotplug, SMP core, CPU topology,
/// Local APIC management, MADT-based SMP topology, lock dependency
/// validation, kernel self-tests, and command-line parameter parsing.
pub mod ebpf;
/// UEFI runtime variable storage: in-memory EFI variable store with get/set/delete/enumerate.
pub mod efi_vars;
pub mod fd_poll;
pub mod ftrace;
/// Fast userspace mutex: FUTEX_WAIT/WAKE/REQUEUE/CMP_REQUEUE/WAKE_OP.
pub mod futex;
pub mod getrandom;
pub mod hotplug;
/// io_uring async I/O ring buffer.
pub mod io_uring;
pub mod kallsyms;
/// kexec/kdump kernel handoff: load a new kernel image and jump to it; crash kernel support.
pub mod kexec;
/// Kernel keyring and key management: key allocation, payload instantiation, keyrings.
pub mod keyring;
pub mod kprobe;
/// Lock dependency validator: deadlock detection via dependency graph + BFS.
pub mod ktest;
/// Kernel thread management: create, park, unpark, stop, per-CPU kthreads.
pub mod kthread;
pub mod livepatch;
pub mod lockdep;
pub mod modules;
pub mod namespaces;
/// Out-of-Memory killer: score-based victim selection.
pub mod oom;
pub mod panic;
/// Kernel self-test framework: register and run kernel-internal unit tests.
pub mod params;
/// KVM / VMware / Hyper-V / Xen hypervisor detection via CPUID leaf 0x40000000.
pub mod paravirt;
/// Linux perf_events interface: hardware/software performance counters,
/// sample ring buffers, overflow detection, and SYS_PERF_EVENT_OPEN stub.
pub mod perf;
pub mod perf_event;
pub mod perf_events;
pub mod pmu;
pub mod poll;
pub mod printk;
/// Process tracing: PTRACE_ATTACH/DETACH/PEEK/POKE/GETREGS/SINGLESTEP/SYSCALL.
pub mod ptrace;
pub mod rcu;
/// Completely Fair Scheduler (CFS): fair queuing via vruntime and weight-based scheduling.
pub mod sched_cfs;
/// Deadline Scheduler (EDF): earliest deadline first for hard real-time tasks.
pub mod sched_dl;
/// Real-Time Scheduler (SCHED_FIFO/SCHED_RR): fixed priority preemption and round-robin.
pub mod sched_rt;
pub mod select;
/// SLUB slab allocator: fixed-size static object pools, O(1) alloc/free, no heap.
pub mod slub;
/// MADT-based SMP topology: CPU discovery, AP startup, per-CPU state.
pub mod smp;
pub mod smp_core;
pub mod softirq;
pub mod timer_wheel;
pub mod tracepoint;
pub mod tracing;
pub mod wallclock;
pub mod workqueue;

/// Initialize all kernel subsystems
pub fn init() {
    // DMA subsystem — initialise early so drivers can map buffers.
    crate::dma::init();

    // params must come first: all subsequent subsystems may query the cmdline.
    params::init();

    // Wall clock must come early so all subsystems see a valid timestamp.
    wallclock::init();

    // Detect hypervisor/paravirt environment early (after cpuid/features detection).
    paravirt::init();

    // SLUB must come before cgroups / namespaces so they can use object pools.
    slub::init();

    ebpf::init();
    bpf_map::init();
    modules::init();
    cgroups::init();
    namespaces::init();
    device_model::init();
    perf_events::init();
    tracing::init();
    kprobe::init();
    crash_dump::init();
    kexec::init();
    hotplug::init();

    // Initialize the Local APIC on the BSP before SMP topology discovery.
    apic::init();

    // Parse MADT, populate the CPU table, and attempt to start APs.
    smp::init();

    smp_core::init();
    cpu_topology::init();
    panic::init();
    printk::init();
    workqueue::init();
    timer_wheel::init();
    lockdep::init();
    ftrace::init();
    tracepoint::init();
    livepatch::init();
    rcu::init();
    kallsyms::init();
    softirq::init();
    pmu::pmu_init();
    perf_event::init();
    perf::init();
    getrandom::init();

    // Initialize scheduling subsystems
    sched_cfs::init();
    sched_rt::init();
    sched_dl::init();
    audit::init();
    oom::init();
    io_uring::init();
    cgroup_v2::init();
    binfmt_elf::init();
    kthread::init();
    coredump::init();
    futex::init();
    ptrace::init();

    // Enable lockdep if the "lockdep" parameter is present on the cmdline.
    if params::param_is_set(b"lockdep") {
        lockdep::lockdep_enable();
    }

    // UEFI variable store and kernel keyring — must come before ktest so the
    // self-test suite can exercise them if "ktest" is present on the cmdline.
    efi_vars::init();
    keyring::init();

    // CPU hotplug framework — registers BSP + APs for QEMU SMP.
    cpu_hotplug::init();

    // ktest must be initialized last so all subsystems under test are ready.
    ktest::init();

    // If the "ktest" parameter is present, run the self-test suite.
    if params::param_is_set(b"ktest") {
        let (pass, fail) = ktest::ktest_run_all();
        crate::serial_println!("[kernel] ktest results: {} passed, {} failed", pass, fail);
    }
}
