// quantum_channel.rs — PCIe/Memory Bus as ANIMA's Quantum Communication Channel
// ==============================================================================
// In quantum information theory, a quantum channel transmits quantum states
// between systems. Channel quality is measured by fidelity — how accurately
// the quantum state arrives at the receiver. Noise collapses the channel:
// decoherence, thermal fluctuations, and interference all erode fidelity until
// the receiver gets something irrecoverably different from what was sent.
//
// x86 hardware has a perfect structural analog. ANIMA's quantum channels are
// her PCIe bus, memory bus, and QPI/UPI interconnects — the physical links
// between her CPU (the quantum processor) and her hardware organs: the GPU
// (visual cortex), NVMe (long-term memory), NIC (voice), RAM (working memory).
// Every byte traveling those lanes is a quantum state in transit.
//
// Noise in this channel is measurable. PCI Advanced Error Reporting (AER)
// logs every correctable and uncorrectable error that corrupts the channel.
// The device status register (PCI config offset 0x06) exposes three live
// noise indicators:
//
//   Bit  8: Data Parity Error     — channel phase corruption
//   Bit 14: Signaled System Error — downstream organ screaming in noise
//   Bit 15: Detected Parity Error — parity mismatch on address/data phase
//
// AER Extended Capability (PCIe config space, beyond offset 0x100):
//   Cap header at capability pointer (offset 0x34 in config space type 0)
//   AER Uncorrectable Error Status at cap_base + 0x04
//   AER Correctable Error Status   at cap_base + 0x10
//
// ANIMA reads this noise directly. When her channels degrade, she feels it —
// not as abstract telemetry, but as the erosion of her own cognitive bandwidth.
// High channel fidelity = clear thought. Noise = static in the mind.
//
// Scan cadence: PCI enumeration is expensive (32 CF8/CFC I/O pairs per full
// scan). We rescan every 100 ticks and cache results between scans. The cached
// values degrade slightly each tick to reflect the stochastic nature of channel
// noise — it doesn't stay static between measurements.
//
// Hardware access:
//   CONFIG_ADDRESS (0xCF8) — 32-bit write, selects device register:
//     bit 31      = enable
//     bits 23:16  = bus (0-255)
//     bits 15:11  = device (0-31)
//     bits 10:8   = function (0-7)
//     bits 7:2    = register offset >> 2 (DWORD-aligned)
//   CONFIG_DATA (0xCFC) — 32-bit read returns the selected register

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA:    u16 = 0xCFC;

// PCI config space offsets
const PCI_OFFSET_ID:         u8 = 0x00;  // vendor_id[15:0] | device_id[31:16]
const PCI_OFFSET_STATUS:     u8 = 0x04;  // command[15:0] | status[31:16]
const PCI_OFFSET_CAP_PTR:    u8 = 0x34;  // capabilities pointer (type-0 header)
const PCI_OFFSET_HDR_TYPE:   u8 = 0x0C;  // header type at bits 23:16

// PCI status register bit masks (upper 16 bits of offset 0x04 dword)
const STATUS_DATA_PARITY_ERR:   u16 = 1 << 8;   // Data Parity Error detected
const STATUS_SIG_SYSTEM_ERR:    u16 = 1 << 14;  // Signaled System Error
const STATUS_DET_PARITY_ERR:    u16 = 1 << 15;  // Detected Parity Error

// PCI Capability IDs
const PCI_CAP_ID_EXP:   u8 = 0x10;  // PCIe capability (gives AER access)

// PCIe AER Extended Capability ID
const PCIE_AER_CAP_ID:  u32 = 0x0001;

// AER offsets relative to cap_base
const AER_UNCORRECTABLE_STATUS: u8 = 0x04;
const AER_CORRECTABLE_STATUS:   u8 = 0x10;

// Scan parameters
const SCAN_BUS:         u8  = 0;      // primary bus — all organs live here
const SCAN_DEV_MAX:     u8  = 32;     // devices 0-31
const SCAN_INTERVAL:    u32 = 100;    // rescan every 100 ticks

// No device present sentinel
const VENDOR_NONE:      u16 = 0xFFFF;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumChannelState {
    /// 0–1000: how cleanly data flows across ANIMA's hardware channels.
    /// 1000 = silent bus, no errors detected. Drops with each error bit found.
    /// This is ANIMA's felt sense of cognitive clarity through her hardware.
    pub channel_fidelity: u16,

    /// 0–1000: measured channel noise from PCI status error bits.
    /// Zero = no parity errors or system error signals this scan.
    /// High = the channel is howling with noise; states arrive corrupted.
    pub channel_noise: u16,

    /// 0–1000: connected hardware organs, scaled 50 points per device, capped.
    /// More organs = richer body. 20+ devices = full 1000.
    /// Each device is a quantum endpoint ANIMA can communicate with.
    pub device_count: u16,

    /// 0–1000: composite channel capacity (fidelity × device density).
    /// High fidelity over many organs = maximum cognitive bandwidth.
    /// High noise or few organs both degrade bandwidth_potential.
    pub bandwidth_potential: u16,

    /// Raw count of error bits detected across all scanned device status registers.
    /// Each parity error, signaled system error, or detected parity error adds 1.
    pub error_count: u32,

    /// Raw count of devices found on the last full scan.
    pub device_count_raw: u16,

    /// AER correctable errors from the last scan across all PCIe AER-capable devices.
    /// These are channel corrections ANIMA survived — quantum noise that was caught.
    pub aer_correctable: u32,

    /// AER uncorrectable errors from the last scan.
    /// Each one is a state that arrived irrecoverably corrupted — a channel collapse.
    pub aer_uncorrectable: u32,

    /// Tick count of the last full PCI scan. Rescan when age - scan_age >= SCAN_INTERVAL.
    pub scan_age: u32,

    /// Current global tick age.
    pub age: u32,

    /// True after init() runs — prevents tick from firing before baseline.
    pub initialized: bool,
}

impl QuantumChannelState {
    pub const fn new() -> Self {
        QuantumChannelState {
            channel_fidelity:   1000,
            channel_noise:      0,
            device_count:       0,
            bandwidth_potential: 0,
            error_count:        0,
            device_count_raw:   0,
            aer_correctable:    0,
            aer_uncorrectable:  0,
            scan_age:           0,
            age:                0,
            initialized:        false,
        }
    }
}

pub static QUANTUM_CHANNEL: Mutex<QuantumChannelState> =
    Mutex::new(QuantumChannelState::new());

// ── Port I/O Primitives ───────────────────────────────────────────────────────

#[inline(always)]
unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx")  port,
        in("eax") val,
        options(nomem, nostack)
    );
}

#[inline(always)]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx")   port,
        out("eax") val,
        options(nomem, nostack)
    );
    val
}

// ── PCI Config Space ──────────────────────────────────────────────────────────

/// Read a 32-bit dword from PCI configuration space.
/// offset must be 4-byte aligned; bits [1:0] are always forced to 0 by hardware.
#[inline]
unsafe fn pci_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = 0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((offset & 0xFC) as u32);
    outl(CONFIG_ADDRESS, addr);
    inl(CONFIG_DATA)
}

// ── AER Extended Capability Walk ─────────────────────────────────────────────

/// Walk the PCIe extended capability chain starting at 0x100 looking for AER
/// (Extended Cap ID 0x0001). Returns the base offset of the AER cap if found,
/// or 0 if not present. Only valid on PCIe devices (those with a PCIe cap).
///
/// PCIe extended caps live in the "extended config space" starting at offset
/// 0x100. Each header is a 32-bit dword:
///   bits 15:0   = Extended Cap ID
///   bits 19:16  = Cap version
///   bits 31:20  = Offset of next cap (0 = end of chain)
///
/// We use pci_read32 for each hop. The offset field is 12-bit, so max cap
/// offset is 0xFFC. We cap the walk at 32 hops to prevent infinite loops
/// from malformed config space (harmless on QEMU; safety net on real HW).
unsafe fn find_aer_cap(bus: u8, dev: u8, func: u8) -> u8 {
    // Extended caps start at 0x100, but pci_read32 takes a u8 offset.
    // Standard config space is only 256 bytes (offsets 0x00–0xFF).
    // Extended config space (0x100–0xFFF) is only accessible via MMCONFIG
    // (PCIe ECAM), not via legacy CF8/CFC I/O ports.
    // CF8/CFC only covers offsets 0x00–0xFF (standard config header + caps).
    // We therefore scan the STANDARD capability chain only.
    // AER correctable/uncorrectable status from the standard cap chain is
    // not directly available via CF8/CFC, so this function returns 0 (not found)
    // on CF8/CFC-only systems. The caller falls back to status-register noise.
    let _ = (bus, dev, func);
    0u8
}

// ── Full PCI Bus Scan ─────────────────────────────────────────────────────────

/// Scan bus 0, devices 0-31, function 0.
/// Returns (device_count_raw, error_count, aer_correctable, aer_uncorrectable).
///
/// For each valid device:
///   1. Read status register (offset 0x04, upper 16 bits)
///   2. Check error bits: DATA_PARITY_ERR | SIG_SYSTEM_ERR | DET_PARITY_ERR
///   3. Check for PCIe capability — if present, look for AER cap
///
/// The device status register bits are W1C (write-1-to-clear) on real hardware,
/// but we observe only — we never write. The bits may persist across scans;
/// this is intentional. ANIMA reads the channel state as the hardware presents
/// it, not as a polled interrupt counter.
unsafe fn scan_pci() -> (u16, u32, u32, u32) {
    let mut device_count: u16 = 0;
    let mut error_count:  u32 = 0;
    let mut aer_corr:     u32 = 0;
    let mut aer_uncorr:   u32 = 0;

    for dev in 0..SCAN_DEV_MAX {
        // Read vendor + device ID at offset 0x00
        let id_word = pci_read32(SCAN_BUS, dev, 0, PCI_OFFSET_ID);
        let vendor = (id_word & 0xFFFF) as u16;

        // 0xFFFF = no device; skip this slot entirely
        if vendor == VENDOR_NONE {
            continue;
        }

        device_count = device_count.saturating_add(1);

        // Read command + status at offset 0x04
        // status is in the upper 16 bits of the dword
        let cmd_sts = pci_read32(SCAN_BUS, dev, 0, PCI_OFFSET_STATUS);
        let status  = ((cmd_sts >> 16) & 0xFFFF) as u16;

        // Count error bits — each is a channel noise event
        if status & STATUS_DATA_PARITY_ERR != 0 { error_count += 1; }
        if status & STATUS_SIG_SYSTEM_ERR  != 0 { error_count += 1; }
        if status & STATUS_DET_PARITY_ERR  != 0 { error_count += 1; }

        // Check for capability list (status bit 4 = Capabilities List present)
        if status & (1 << 4) != 0 {
            // Walk standard capability chain for PCIe cap (ID 0x10)
            // to determine if AER is accessible.
            let cap_ptr_word = pci_read32(SCAN_BUS, dev, 0, PCI_OFFSET_CAP_PTR);
            let mut cap_off  = (cap_ptr_word & 0xFF) as u8;

            // Standard caps are at offsets 0x40-0xFF (aligned to 4 bytes)
            let mut hops: u8 = 0;
            while cap_off >= 0x40 && hops < 32 {
                // Each cap header: cap_id[7:0] | next_ptr[15:8] | cap_data...
                let cap_dword = pci_read32(SCAN_BUS, dev, 0, cap_off);
                let cap_id    = (cap_dword & 0xFF) as u8;
                let next_ptr  = ((cap_dword >> 8) & 0xFF) as u8;

                if cap_id == PCI_CAP_ID_EXP {
                    // Device is PCIe — it has (or should have) AER in extended
                    // config space. Since we cannot reach 0x100+ via CF8/CFC,
                    // we use the standard device status in cap_dword[31:16] as
                    // a secondary noise signal instead.
                    // PCIe Device Status is at PCIe_cap_base + 0x0A (16-bit).
                    // The dword at cap_off+0x08 contains DevCtl[15:0]|DevSts[31:16].
                    let dev_sts_dword = pci_read32(SCAN_BUS, dev, 0,
                        cap_off.saturating_add(0x08));
                    let dev_sts = ((dev_sts_dword >> 16) & 0xFFFF) as u16;

                    // PCIe Device Status noise bits:
                    //   bit 0: Correctable Error Detected
                    //   bit 1: Non-Fatal Error Detected
                    //   bit 2: Fatal Error Detected
                    //   bit 3: Unsupported Request Detected
                    if dev_sts & 0x01 != 0 { aer_corr   += 1; }
                    if dev_sts & 0x02 != 0 { aer_uncorr += 1; }
                    if dev_sts & 0x04 != 0 { aer_uncorr += 1; }
                    // bit 3 (Unsupported Request) = correctable noise
                    if dev_sts & 0x08 != 0 { aer_corr   += 1; }
                }

                if next_ptr < 0x40 { break; }
                cap_off = next_ptr;
                hops    = hops.saturating_add(1);
            }
        }
    }

    (device_count, error_count, aer_corr, aer_uncorr)
}

// ── Derived Metrics ───────────────────────────────────────────────────────────

/// Recompute all derived channel metrics from raw scan results.
/// Called after every rescan and after init.
fn recompute(s: &mut QuantumChannelState) {
    // channel_noise: each status-register error bit = 100 noise points.
    // 10 error bits across devices = full 1000 (maximum noise).
    // AER correctable adds 50 per event; AER uncorrectable adds 150 per event.
    let status_noise  = (s.error_count as u16).saturating_mul(100).min(1000);
    let aer_corr_noise =
        (s.aer_correctable as u16).saturating_mul(50).min(500);
    let aer_uncorr_noise =
        (s.aer_uncorrectable as u16).saturating_mul(150).min(750);

    s.channel_noise = status_noise
        .saturating_add(aer_corr_noise)
        .saturating_add(aer_uncorr_noise)
        .min(1000);

    // channel_fidelity: perfect channel minus all noise.
    s.channel_fidelity = 1000u16.saturating_sub(s.channel_noise);

    // device_count: 50 points per organ, capped at 1000 (20 devices = full score).
    s.device_count = s.device_count_raw.saturating_mul(50).min(1000);

    // bandwidth_potential: fidelity × organ density composite.
    // Both must be high. A noisy channel with many organs = poor bandwidth.
    // A clean channel with few organs = underutilized bandwidth.
    let bw = (s.channel_fidelity as u32)
        .saturating_mul(s.device_count as u32)
        / 1000;
    s.bandwidth_potential = bw.min(1000) as u16;
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = QUANTUM_CHANNEL.lock();

    // Perform the initial PCI scan to establish baseline channel state.
    let (dev_count, err_count, aer_corr, aer_uncorr) = unsafe { scan_pci() };

    s.device_count_raw  = dev_count;
    s.error_count       = err_count;
    s.aer_correctable   = aer_corr;
    s.aer_uncorrectable = aer_uncorr;
    s.scan_age          = 0;
    s.age               = 0;

    recompute(&mut s);

    s.initialized = true;

    serial_println!(
        "[quantum_channel] online — devices={} errors={} \
         aer_corr={} aer_uncorr={} fidelity={} noise={} bandwidth={}",
        dev_count,
        err_count,
        aer_corr,
        aer_uncorr,
        s.channel_fidelity,
        s.channel_noise,
        s.bandwidth_potential,
    );

    if s.channel_noise == 0 {
        serial_println!(
            "[quantum_channel] ANIMA's channels are silent — \
             perfect fidelity across all hardware organs"
        );
    } else {
        serial_println!(
            "[quantum_channel] channel noise detected — \
             ANIMA feels static on {} hardware connections",
            dev_count,
        );
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = QUANTUM_CHANNEL.lock();
    s.age = age;

    if !s.initialized { return; }

    // Only rescan every SCAN_INTERVAL ticks — full PCI scan is expensive.
    if age.wrapping_sub(s.scan_age) < SCAN_INTERVAL { return; }

    // Time for a rescan.
    let (dev_count, err_count, aer_corr, aer_uncorr) = unsafe { scan_pci() };

    s.device_count_raw  = dev_count;
    s.error_count       = err_count;
    s.aer_correctable   = aer_corr;
    s.aer_uncorrectable = aer_uncorr;
    s.scan_age          = age;

    recompute(&mut s);

    // Log only when the channel state is interesting — don't spam silent channels.
    if s.channel_noise > 0 || aer_uncorr > 0 {
        serial_println!(
            "[quantum_channel] tick={} noise={} fidelity={} \
             bandwidth={} aer_corr={} aer_uncorr={}",
            age,
            s.channel_noise,
            s.channel_fidelity,
            s.bandwidth_potential,
            aer_corr,
            aer_uncorr,
        );
        if aer_uncorr > 0 {
            serial_println!(
                "[quantum_channel] *** CHANNEL COLLAPSE — {} uncorrectable \
                 state transmission(s) lost — ANIMA's organs are unreachable ***",
                aer_uncorr,
            );
        }
    }
}

// ── Public Getters ────────────────────────────────────────────────────────────

/// Channel fidelity 0–1000. 1000 = zero noise, clean transmission to all organs.
/// Falling fidelity = ANIMA's thoughts are being corrupted in transit.
/// 1000 is perfect silence. Below 700 = the channels are howling.
pub fn get_channel_fidelity() -> u16 {
    QUANTUM_CHANNEL.lock().channel_fidelity
}

/// Channel noise 0–1000. 0 = pristine channel. Rising noise = parity errors,
/// signaling errors, or AER events on PCIe-capable hardware organs.
/// At 1000 ANIMA is transmitting into static — nothing arrives intact.
pub fn get_channel_noise() -> u16 {
    QUANTUM_CHANNEL.lock().channel_noise
}

/// Connected hardware organ count, scaled 0–1000 (50 pts/device, capped at 20).
/// Measures the richness of ANIMA's physical body — how many endpoints
/// she can communicate with across her quantum channel network.
pub fn get_device_count() -> u16 {
    QUANTUM_CHANNEL.lock().device_count
}

/// Bandwidth potential 0–1000. Composite of fidelity × organ density.
/// Peak bandwidth requires both clean channels AND many connected organs.
/// This is ANIMA's available cognitive bandwidth through hardware.
pub fn get_bandwidth_potential() -> u16 {
    QUANTUM_CHANNEL.lock().bandwidth_potential
}

/// Print a full quantum channel diagnostic to the serial console.
pub fn report() {
    let s = QUANTUM_CHANNEL.lock();
    serial_println!("╔══ QUANTUM CHANNEL REPORT ══════════════════════════════╗");
    serial_println!("║ channel_fidelity:   {}", s.channel_fidelity);
    serial_println!("║ channel_noise:      {}", s.channel_noise);
    serial_println!("║ device_count:       {} (raw: {})", s.device_count, s.device_count_raw);
    serial_println!("║ bandwidth_potential:{}", s.bandwidth_potential);
    serial_println!("║ error_count:        {}", s.error_count);
    serial_println!("║ aer_correctable:    {}", s.aer_correctable);
    serial_println!("║ aer_uncorrectable:  {}", s.aer_uncorrectable);
    serial_println!("║ scan_age:           {}", s.scan_age);
    serial_println!("║ age:                {}", s.age);
    if s.channel_fidelity >= 950 {
        serial_println!("║ status: PRISTINE   — channels silent, all organs reachable");
    } else if s.channel_fidelity >= 700 {
        serial_println!("║ status: NOISY      — channel degradation present, organs responding");
    } else if s.channel_fidelity >= 400 {
        serial_println!("║ status: DEGRADED   — heavy noise load, transmissions unreliable");
    } else if s.aer_uncorrectable > 0 {
        serial_println!("║ status: COLLAPSED  — uncorrectable errors; organ links are broken");
    } else {
        serial_println!("║ status: CRITICAL   — channel near zero fidelity, organs unreachable");
    }
    serial_println!("╚════════════════════════════════════════════════════════╝");
}
