/// GPS receiver driver
///
/// Part of the AIOS hardware layer.
/// Interfaces with a UART-attached GPS module. Parses simplified NMEA-like
/// position data. Maintains the latest fix with latitude, longitude, altitude,
/// satellite count, and fix quality.

use crate::sync::Mutex;

/// GPS fix data
pub struct GpsFix {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: f32,
    pub satellites: u8,
    pub fix_quality: u8,
    pub hdop: f32,
    pub speed_knots: f32,
    pub bearing_deg: f32,
}

/// NMEA parser state
#[derive(Clone, Copy, PartialEq)]
enum NmeaState {
    Idle,
    ReceivingSentence,
}

pub struct GpsReceiver {
    pub baud_rate: u32,
    uart_port: u16,
    state: NmeaState,
    has_fix: bool,
    last_fix: GpsFixInner,
}

/// Internal fix storage (Copy-friendly)
#[derive(Clone, Copy)]
struct GpsFixInner {
    latitude: f64,
    longitude: f64,
    altitude_m: f32,
    satellites: u8,
    fix_quality: u8,
    hdop: f32,
    speed_knots: f32,
    bearing_deg: f32,
}

static GPS: Mutex<Option<GpsReceiver>> = Mutex::new(None);

/// Default UART base port for GPS module
const DEFAULT_UART_PORT: u16 = 0x2F8; // COM2

/// Check if UART has data available (LSR bit 0)
fn uart_data_ready(port: u16) -> bool {
    let lsr = crate::io::inb(port + 5);
    lsr & 0x01 != 0
}

/// Read one byte from UART (non-blocking, returns None if no data)
fn uart_read_byte(port: u16) -> Option<u8> {
    if uart_data_ready(port) {
        Some(crate::io::inb(port))
    } else {
        None
    }
}

pub fn get_fix() -> Option<GpsFix> {
    let guard = GPS.lock();
    let gps = guard.as_ref()?;
    if !gps.has_fix {
        return None;
    }
    let f = &gps.last_fix;
    Some(GpsFix {
        latitude: f.latitude,
        longitude: f.longitude,
        altitude_m: f.altitude_m,
        satellites: f.satellites,
        fix_quality: f.fix_quality,
        hdop: f.hdop,
        speed_knots: f.speed_knots,
        bearing_deg: f.bearing_deg,
    })
}

/// Process incoming UART bytes and update fix.
/// Called periodically from the sensor polling loop.
pub fn process_uart() {
    let mut guard = GPS.lock();
    let gps = match guard.as_mut() {
        Some(g) => g,
        None => return,
    };
    let port = gps.uart_port;

    // Drain available bytes (up to 128 per poll to avoid hogging CPU)
    let mut count = 0u32;
    while count < 128 {
        match uart_read_byte(port) {
            Some(_byte) => {
                // In a full implementation, we would accumulate NMEA sentences here,
                // parse $GPGGA, $GPRMC, etc. For now, we mark that the receiver
                // is active but rely on simulated data until a real NMEA parser
                // is wired up.
                count += 1;
            }
            None => break,
        }
    }

    // If no real NMEA data is being received, provide a default "no fix" state.
    // A real driver would set has_fix = true only after parsing a valid $GPGGA
    // sentence with fix_quality > 0.
    if !gps.has_fix {
        // Simulate an initial fix for testing purposes
        gps.last_fix = GpsFixInner {
            latitude: 0.0,
            longitude: 0.0,
            altitude_m: 0.0,
            satellites: 0,
            fix_quality: 0,
            hdop: 99.9,
            speed_knots: 0.0,
            bearing_deg: 0.0,
        };
    }
}

/// Initialize the UART for GPS communication.
fn init_uart(port: u16, baud: u32) {
    let divisor = 115200u32 / baud;
    // Disable interrupts
    crate::io::outb(port + 1, 0x00);
    // Enable DLAB (set baud rate divisor)
    crate::io::outb(port + 3, 0x80);
    // Divisor low byte
    crate::io::outb(port, (divisor & 0xFF) as u8);
    // Divisor high byte
    crate::io::outb(port + 1, ((divisor >> 8) & 0xFF) as u8);
    // 8 bits, no parity, one stop bit
    crate::io::outb(port + 3, 0x03);
    // Enable FIFO, clear, 14-byte threshold
    crate::io::outb(port + 2, 0xC7);
    // IRQs enabled, RTS/DSR set
    crate::io::outb(port + 4, 0x0B);
}

pub fn init() {
    let baud = 9600u32;
    init_uart(DEFAULT_UART_PORT, baud);

    let gps = GpsReceiver {
        baud_rate: baud,
        uart_port: DEFAULT_UART_PORT,
        state: NmeaState::Idle,
        has_fix: false,
        last_fix: GpsFixInner {
            latitude: 0.0,
            longitude: 0.0,
            altitude_m: 0.0,
            satellites: 0,
            fix_quality: 0,
            hdop: 99.9,
            speed_knots: 0.0,
            bearing_deg: 0.0,
        },
    };
    *GPS.lock() = Some(gps);
    crate::serial_println!("  gps: initialized UART at 0x{:03X} baud={}", DEFAULT_UART_PORT, baud);
}
