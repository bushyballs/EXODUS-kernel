/// Unicode general category (simplified)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Letter,
    Mark,
    Number,
    Punctuation,
    Symbol,
    Separator,
    Other,
}

/// Bidirectional type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidiType {
    LeftToRight,
    RightToLeft,
    ArabicNumber,
    EuropeanNumber,
    Neutral,
}

/// Get the general category of a Unicode codepoint
pub fn category(cp: u32) -> Category {
    match cp {
        0x0041..=0x005A | 0x0061..=0x007A => Category::Letter, // Basic Latin
        0x00C0..=0x024F => Category::Letter,                   // Latin Extended
        0x0400..=0x04FF => Category::Letter,                   // Cyrillic
        0x0600..=0x065F | 0x066A..=0x06FF => Category::Letter, // Arabic (excl. digits)
        0x3040..=0x309F => Category::Letter,                   // Hiragana
        0x30A0..=0x30FF => Category::Letter,                   // Katakana
        0x4E00..=0x9FFF => Category::Letter,                   // CJK
        0xAC00..=0xD7AF => Category::Letter,                   // Hangul
        0x0300..=0x036F => Category::Mark,                     // Combining marks
        0x0030..=0x0039 => Category::Number,                   // ASCII digits
        0x0660..=0x0669 => Category::Number,                   // Arabic-Indic digits
        0x0020 | 0x00A0 | 0x200B..=0x200C => Category::Separator,
        0x0021..=0x002E | 0x003A..=0x0040 | 0x005B..=0x0060 | 0x007B..=0x007E | 0x2000..=0x206F => {
            Category::Punctuation
        }
        _ => Category::Other,
    }
}

/// Get the bidirectional type of a codepoint
pub fn bidi_type(cp: u32) -> BidiType {
    match cp {
        0x0041..=0x005A | 0x0061..=0x007A => BidiType::LeftToRight,
        0x0600..=0x065F
        | 0x066A..=0x06EF
        | 0x06FA..=0x06FF
        | 0x0750..=0x077F
        | 0xFB50..=0xFDFF
        | 0xFE70..=0xFEFF
        | 0x0590..=0x05FF
        | 0xFB1D..=0xFB4F => BidiType::RightToLeft,
        0x0660..=0x0669 | 0x06F0..=0x06F9 => BidiType::ArabicNumber,
        0x0030..=0x0039 => BidiType::EuropeanNumber,
        _ => BidiType::Neutral,
    }
}

/// Check if a character is a combining mark
pub fn is_combining(cp: u32) -> bool {
    matches!(cp, 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD |
                  0x0610..=0x061A | 0x064B..=0x065F | 0x0670 |
                  0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x0E31 | 0x0E34..=0x0E3A)
}

/// Decode a UTF-8 byte sequence to a codepoint
pub fn decode_utf8(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    if pos >= bytes.len() {
        return None;
    }
    let b0 = bytes[pos];

    if b0 < 0x80 {
        Some((b0 as u32, 1))
    } else if b0 < 0xC0 {
        None // continuation byte
    } else if b0 < 0xE0 {
        if pos + 1 >= bytes.len() {
            return None;
        }
        let cp = ((b0 as u32 & 0x1F) << 6) | (bytes[pos + 1] as u32 & 0x3F);
        Some((cp, 2))
    } else if b0 < 0xF0 {
        if pos + 2 >= bytes.len() {
            return None;
        }
        let cp = ((b0 as u32 & 0x0F) << 12)
            | ((bytes[pos + 1] as u32 & 0x3F) << 6)
            | (bytes[pos + 2] as u32 & 0x3F);
        Some((cp, 3))
    } else if b0 < 0xF8 {
        if pos + 3 >= bytes.len() {
            return None;
        }
        let cp = ((b0 as u32 & 0x07) << 18)
            | ((bytes[pos + 1] as u32 & 0x3F) << 12)
            | ((bytes[pos + 2] as u32 & 0x3F) << 6)
            | (bytes[pos + 3] as u32 & 0x3F);
        Some((cp, 4))
    } else {
        None
    }
}

/// Encode a codepoint to UTF-8 bytes
pub fn encode_utf8(cp: u32, buf: &mut [u8]) -> usize {
    if cp < 0x80 {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = cp as u8;
        1
    } else if cp < 0x800 {
        if buf.len() < 2 {
            return 0;
        }
        buf[0] = 0xC0 | (cp >> 6) as u8;
        buf[1] = 0x80 | (cp & 0x3F) as u8;
        2
    } else if cp < 0x10000 {
        if buf.len() < 3 {
            return 0;
        }
        buf[0] = 0xE0 | (cp >> 12) as u8;
        buf[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        buf[2] = 0x80 | (cp & 0x3F) as u8;
        3
    } else {
        if buf.len() < 4 {
            return 0;
        }
        buf[0] = 0xF0 | (cp >> 18) as u8;
        buf[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
        buf[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        buf[3] = 0x80 | (cp & 0x3F) as u8;
        4
    }
}

/// Count grapheme clusters in a UTF-8 string (simplified)
pub fn grapheme_count(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut count = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        if let Some((cp, len)) = decode_utf8(bytes, pos) {
            if !is_combining(cp) {
                count += 1;
            }
            pos += len;
        } else {
            pos += 1;
            count += 1;
        }
    }
    count
}

pub fn init() {
    crate::serial_println!("  [i18n] Unicode support initialized (UTF-8, bidi, grapheme)");
}
