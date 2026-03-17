/// Minimal DER/ASN.1 parser — no heap, no alloc
///
/// Parses BER/DER Tag-Length-Value structures for X.509 and PKCS#1 parsing.
/// All output is into caller-supplied fixed-size buffers or byte slices
/// into the original data (zero-copy where possible).

// ---------------------------------------------------------------------------
// Well-known ASN.1 tag constants
// ---------------------------------------------------------------------------

pub const TAG_BOOLEAN: u8 = 0x01;
pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0C;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;
pub const TAG_CONTEXT_0: u8 = 0xA0;
pub const TAG_CONTEXT_3: u8 = 0xA3;
pub const TAG_PRINTABLE_STRING: u8 = 0x13;
pub const TAG_IA5_STRING: u8 = 0x16;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;

// ---------------------------------------------------------------------------
// Well-known OID byte sequences (DER-encoded OID value bytes, without TL)
// ---------------------------------------------------------------------------

/// RSA encryption: 1.2.840.113549.1.1.1
pub const OID_RSA: [u8; 9] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
/// SHA-256: 2.16.840.1.101.3.4.2.1
pub const OID_SHA256: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
/// sha256WithRSAEncryption: 1.2.840.113549.1.1.11
pub const OID_SHA256_WITH_RSA: [u8; 9] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];
/// EC public key: 1.2.840.10045.2.1
pub const OID_EC_PUBLIC_KEY: [u8; 7] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
/// Common Name: 2.5.4.3
pub const OID_COMMON_NAME: [u8; 3] = [0x55, 0x04, 0x03];
/// Organization: 2.5.4.10
pub const OID_ORGANIZATION: [u8; 3] = [0x55, 0x04, 0x0A];

// ---------------------------------------------------------------------------
// DER/ASN.1 parser — cursor over a borrowed byte slice
// ---------------------------------------------------------------------------

/// Cursor-based DER parser.  All lifetime 'a references point into the
/// original `data` slice — zero allocation.
pub struct Asn1Der<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Asn1Der<'a> {
    /// Create a new parser positioned at the start of `data`.
    #[inline]
    pub fn new(data: &'a [u8]) -> Self {
        Asn1Der { data, pos: 0 }
    }

    // -----------------------------------------------------------------------
    // Low-level field readers
    // -----------------------------------------------------------------------

    /// Read one tag byte and advance the cursor.
    /// Returns `None` if at end of buffer.
    pub fn read_tag(&mut self) -> Option<u8> {
        if self.pos >= self.data.len() {
            return None;
        }
        let tag = self.data[self.pos];
        self.pos = self.pos.saturating_add(1);
        Some(tag)
    }

    /// Decode a DER definite-length field (short or long form up to 3
    /// additional octets → max 16 MiB value).  Advances cursor past the
    /// length octets.
    pub fn read_length(&mut self) -> Option<usize> {
        if self.pos >= self.data.len() {
            return None;
        }
        let first = self.data[self.pos];
        self.pos = self.pos.saturating_add(1);

        if first < 0x80 {
            // Short form: length in low 7 bits
            Some(first as usize)
        } else if first == 0x80 {
            // Indefinite form — not valid in DER
            None
        } else {
            // Long form: low 7 bits = number of subsequent length bytes
            let num_len_bytes = (first & 0x7F) as usize;
            if num_len_bytes > 3 || self.pos.saturating_add(num_len_bytes) > self.data.len() {
                return None;
            }
            let mut length: usize = 0;
            for i in 0..num_len_bytes {
                let byte = self.data[self.pos.saturating_add(i)];
                length = (length << 8) | (byte as usize);
            }
            self.pos = self.pos.saturating_add(num_len_bytes);
            Some(length)
        }
    }

    /// Read a complete TLV element.  Returns `(tag, value_slice)` where the
    /// slice borrows from the original buffer.  Cursor is left after the value.
    pub fn read_tlv(&mut self) -> Option<(u8, &'a [u8])> {
        let tag = self.read_tag()?;
        let len = self.read_length()?;
        if self.pos.saturating_add(len) > self.data.len() {
            return None;
        }
        let value = &self.data[self.pos..self.pos.saturating_add(len)];
        self.pos = self.pos.saturating_add(len);
        Some((tag, value))
    }

    /// Skip over one complete TLV element without inspecting the value.
    /// Returns `true` on success, `false` on parse error.
    pub fn skip_tlv(&mut self) -> bool {
        self.read_tlv().is_some()
    }

    // -----------------------------------------------------------------------
    // Typed integer readers
    // -----------------------------------------------------------------------

    /// Read a DER INTEGER whose value fits in a `u64`.
    ///
    /// Strips any leading 0x00 sign byte that DER uses for positive integers
    /// whose top bit would otherwise be set.  Returns `None` on error or
    /// if the integer is more than 8 bytes.
    pub fn read_integer_u64(&mut self) -> Option<u64> {
        let (tag, val) = self.read_tlv()?;
        if tag != TAG_INTEGER {
            return None;
        }
        // Strip optional leading zero sign byte
        let val = if val.first() == Some(&0x00) && val.len() > 1 {
            &val[1..]
        } else {
            val
        };
        if val.is_empty() || val.len() > 8 {
            return None;
        }
        let mut result: u64 = 0;
        for &b in val {
            result = (result << 8) | (b as u64);
        }
        Some(result)
    }

    /// Read a DER INTEGER of arbitrary size (e.g. RSA modulus / exponent)
    /// into caller's 512-byte buffer.  Strips the leading 0x00 sign byte.
    ///
    /// Returns the number of significant bytes written into `out[0..n]`
    /// (big-endian), or `None` on error.
    pub fn read_large_integer(&mut self, out: &mut [u8; 512]) -> Option<usize> {
        let (tag, val) = self.read_tlv()?;
        if tag != TAG_INTEGER {
            return None;
        }
        // Strip leading zero sign byte
        let val = if val.first() == Some(&0x00) && val.len() > 1 {
            &val[1..]
        } else {
            val
        };
        if val.is_empty() || val.len() > 512 {
            return None;
        }
        // Right-justify into out so byte 0 is the most significant byte of n
        let start = 512usize.saturating_sub(val.len());
        // Zero the prefix
        for b in out[..start].iter_mut() {
            *b = 0;
        }
        out[start..].copy_from_slice(val);
        Some(val.len())
    }

    /// Read a DER OID value into a caller-supplied 16-byte buffer.
    /// Returns the number of bytes written, or `None` on error.
    /// The raw DER OID bytes (not dotted-decimal) are stored in `out`.
    pub fn read_oid(&mut self, out: &mut [u8; 16]) -> Option<usize> {
        let (tag, val) = self.read_tlv()?;
        if tag != TAG_OID {
            return None;
        }
        if val.is_empty() || val.len() > 16 {
            return None;
        }
        out[..val.len()].copy_from_slice(val);
        Some(val.len())
    }

    // -----------------------------------------------------------------------
    // Sub-parser helpers
    // -----------------------------------------------------------------------

    /// Construct a fresh parser over an arbitrary data slice.
    /// Useful when the caller already has a value slice from `read_tlv`.
    #[inline]
    pub fn sub_parser(&self, data: &'a [u8]) -> Self {
        Asn1Der::new(data)
    }

    /// Peek at the current tag byte without advancing the cursor.
    /// Returns `None` if at end of buffer.
    #[inline]
    pub fn peek_tag(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Number of bytes remaining in the buffer.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Current cursor position (bytes consumed so far).
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }
}

// ---------------------------------------------------------------------------
// OID comparison helpers
// ---------------------------------------------------------------------------

/// Compare a raw DER OID value slice against `OID_SHA256_WITH_RSA`.
#[inline]
pub fn oid_is_sha256_with_rsa(oid_val: &[u8]) -> bool {
    oid_val == OID_SHA256_WITH_RSA.as_slice()
}

/// Compare a raw DER OID value slice against `OID_RSA`.
#[inline]
pub fn oid_is_rsa(oid_val: &[u8]) -> bool {
    oid_val == OID_RSA.as_slice()
}

/// Compare a raw DER OID value slice against `OID_COMMON_NAME`.
#[inline]
pub fn oid_is_common_name(oid_val: &[u8]) -> bool {
    oid_val == OID_COMMON_NAME.as_slice()
}

/// Compare a raw DER OID value slice against `OID_ORGANIZATION`.
#[inline]
pub fn oid_is_organization(oid_val: &[u8]) -> bool {
    oid_val == OID_ORGANIZATION.as_slice()
}
