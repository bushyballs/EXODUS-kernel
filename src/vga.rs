use crate::sync::Mutex;
use crate::{kprint, kprintln};
/// VGA text mode driver for Hoags Kernel Genesis — built from scratch
///
/// Writes directly to the VGA text buffer at 0xb8000.
/// Supports 80x25 character display with 16 colors.
///
/// No external crates. All code is original.
use core::fmt;

/// Volatile wrapper — prevents compiler from optimizing away MMIO writes.
#[repr(transparent)]
struct Volatile<T: Copy>(T);

impl<T: Copy> Volatile<T> {
    pub fn read(&self) -> T {
        unsafe { core::ptr::read_volatile(&self.0 as *const T) }
    }
    pub fn write(&mut self, val: T) {
        unsafe { core::ptr::write_volatile(&mut self.0 as *mut T, val) }
    }
}

const VGA_BUFFER_HEIGHT: usize = 25;
const VGA_BUFFER_WIDTH: usize = 80;
const VGA_BUFFER_ADDR: usize = 0xb8000;

#[allow(dead_code)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Clone, Copy)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    const fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; VGA_BUFFER_WIDTH]; VGA_BUFFER_HEIGHT],
}

pub struct Writer {
    column_position: usize,
    row_position: usize,
    color_code: ColorCode,
    /// VGA buffer address (stored as usize, dereferenced on access)
    buffer_addr: usize,
}

impl Writer {
    const fn new() -> Self {
        Writer {
            column_position: 0,
            row_position: 0,
            color_code: ColorCode::new(Color::LightCyan, Color::Black),
            buffer_addr: VGA_BUFFER_ADDR,
        }
    }

    fn buffer(&mut self) -> &mut Buffer {
        unsafe { &mut *(self.buffer_addr as *mut Buffer) }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column_position >= VGA_BUFFER_WIDTH {
                    self.new_line();
                }
                let row = self.row_position;
                let col = self.column_position;
                let color = self.color_code;
                self.buffer().chars[row][col].write(ScreenChar {
                    ascii_character: byte,
                    color_code: color,
                });
                self.column_position += 1;
            }
        }
    }

    fn new_line(&mut self) {
        if self.row_position < VGA_BUFFER_HEIGHT - 1 {
            self.row_position += 1;
            self.column_position = 0;
        } else {
            // Scroll up
            for row in 1..VGA_BUFFER_HEIGHT {
                for col in 0..VGA_BUFFER_WIDTH {
                    let ch =
                        unsafe { &*(self.buffer_addr as *const Buffer) }.chars[row][col].read();
                    self.buffer().chars[row - 1][col].write(ch);
                }
            }
            self.clear_row(VGA_BUFFER_HEIGHT - 1);
            self.column_position = 0;
        }
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..VGA_BUFFER_WIDTH {
            self.buffer().chars[row][col].write(blank);
        }
    }

    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.color_code = ColorCode::new(fg, bg);
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            match byte {
                // Printable ASCII or newline
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // Non-printable: show a placeholder
                _ => self.write_byte(0xfe),
            }
        }
        Ok(())
    }
}

/// Global VGA writer — const-initialized, no lazy_static needed
pub static WRITER: Mutex<Writer> = Mutex::new(Writer::new());

pub fn clear_screen() {
    let mut writer = WRITER.lock();
    for row in 0..VGA_BUFFER_HEIGHT {
        writer.clear_row(row);
    }
    writer.row_position = 0;
    writer.column_position = 0;
}

pub fn print_banner() {
    {
        let mut writer = WRITER.lock();
        writer.set_color(Color::LightRed, Color::Black);
    }
    kprintln!("  +==============================+");
    kprintln!("  |    HOAGS KERNEL GENESIS       |");
    kprintln!("  |    v0.1.0 -- Bare Metal Rust  |");
    kprintln!("  +==============================+");
    kprintln!("");
    {
        let mut writer = WRITER.lock();
        writer.set_color(Color::LightCyan, Color::Black);
    }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ($crate::vga::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", format_args!($($arg)*)));
}

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    // write_fmt on Writer always returns Ok(()); ignore the result safely
    let _ = WRITER.lock().write_fmt(args);
    // Mirror to serial so init_thread / userspace output is visible in headless QEMU
    crate::serial_print!("{}", args);
}
