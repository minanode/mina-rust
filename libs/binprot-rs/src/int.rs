//! Integer encodings, read exactly as Jane Street's bin_prot reads them.
//!
//! OCaml peers ban the sender of a gossip message they fail to decode, and a
//! node that forwards a message is its sender to the next hop. So a reader
//! here must accept exactly the bytes OCaml's reader for the same OCaml type
//! accepts: no more (an invalid prefix byte, a value out of range), and no
//! less (bin_prot accepts non-minimal encodings, such as 5 written as
//! `0xfe 0x05 0x00`).
//!
//! OCaml 4.0.2 (Mina 4.0.2) pins bin_prot v0.14.0:
//! <https://github.com/janestreet/bin_prot/blob/v0.14.0/src/read.ml>. The readers
//! below follow its `bin_read_nat0`, `bin_read_int`, `bin_read_int32` and
//! `bin_read_int64` on a 64-bit platform, where OCaml's `int` has 63 bits.

use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Write};

pub const CODE_NEG_INT8: u8 = 0xff;
pub const CODE_INT16: u8 = 0xfe;
pub const CODE_INT32: u8 = 0xfd;
pub const CODE_INT64: u8 = 0xfc;

/// OCaml's `max_int` on a 64-bit platform, 2^62 - 1.
pub const OCAML_MAX_INT: i64 = (1 << 62) - 1;
/// OCaml's `min_int` on a 64-bit platform, -2^62.
pub const OCAML_MIN_INT: i64 = -(1 << 62);

fn invalid(what: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, what)
}

pub fn write_nat0<W: Write>(w: &mut W, v: u64) -> std::io::Result<()> {
    if v < 0x000000080 {
        w.write_all(&[v as u8])?;
    } else if v < 0x000010000 {
        w.write_all(&[CODE_INT16])?;
        w.write_all(&(v as u16).to_le_bytes())?;
    } else if v < 0x100000000 {
        w.write_all(&[CODE_INT32])?;
        w.write_all(&(v as u32).to_le_bytes())?;
    } else {
        w.write_all(&[CODE_INT64])?;
        w.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

pub fn write_i64<W: Write>(w: &mut W, v: i64) -> std::io::Result<()> {
    if 0 <= v {
        if v < 0x000000080 {
            w.write_all(&[v as u8])?;
        } else if v < 0x00008000 {
            w.write_all(&[CODE_INT16])?;
            w.write_all(&(v as u16).to_le_bytes())?;
        } else if v < 0x80000000 {
            w.write_all(&[CODE_INT32])?;
            w.write_all(&(v as u32).to_le_bytes())?;
        } else {
            w.write_all(&[CODE_INT64])?;
            w.write_all(&v.to_le_bytes())?;
        }
    } else if v >= -0x00000080 {
        w.write_all(&[CODE_NEG_INT8])?;
        w.write_all(&v.to_le_bytes()[..1])?;
    } else if v >= -0x00008000 {
        w.write_all(&[CODE_INT16])?;
        w.write_all(&v.to_le_bytes()[..2])?;
    } else if v >= -0x80000000 {
        w.write_all(&[CODE_INT32])?;
        w.write_all(&v.to_le_bytes()[..4])?;
    } else if v < -0x80000000 {
        w.write_all(&[CODE_INT64])?;
        w.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

/// `safe_bin_read_neg_int8`: the byte after `0xff` must be negative.
fn read_neg_int8<R: Read + ?Sized>(r: &mut R) -> std::io::Result<i64> {
    let i = r.read_i8()? as i64;
    if i >= 0 {
        return Err(invalid("Neg_int8"));
    }
    Ok(i)
}

/// `bin_read_nat0`: lengths and `Nat0.t`. `0x00` to `0x7f` is the value; `0xfe`,
/// `0xfd` and `0xfc` prefix 2, 4 and 8 little-endian bytes, the last at most
/// `max_int`. Any other first byte is `Nat0_code`. Non-minimal forms (5 as
/// `0xfe 0x05 0x00`) are accepted, as bin_prot accepts them.
///
/// bin_prot: <https://github.com/janestreet/bin_prot/blob/v0.14.0/src/read.ml#L202-L266>
pub fn read_nat0<R: Read + ?Sized>(r: &mut R) -> std::io::Result<u64> {
    let c = r.read_u8()?;
    let v = match c {
        0x00..=0x7f => c as u64,
        CODE_INT16 => r.read_u16::<LittleEndian>()? as u64,
        CODE_INT32 => r.read_u32::<LittleEndian>()? as u64,
        CODE_INT64 => {
            let v = r.read_i64::<LittleEndian>()?;
            if !(0..=OCAML_MAX_INT).contains(&v) {
                return Err(invalid("Nat0_overflow"));
            }
            v as u64
        }
        _ => return Err(invalid("Nat0_code")),
    };
    Ok(v)
}

/// `bin_read_int`: OCaml's 63-bit `int`. `0xff` prefixes a negative byte,
/// `0xfe`, `0xfd` and `0xfc` signed 2, 4 and 8 little-endian bytes, the last
/// between `min_int` and `max_int` (`Int_overflow` otherwise). Any other
/// first byte from `0x80` is `Int_code`.
///
/// bin_prot: <https://github.com/janestreet/bin_prot/blob/v0.14.0/src/read.ml#L294-L314>
pub fn read_int<R: Read + ?Sized>(r: &mut R) -> std::io::Result<i64> {
    let c = r.read_u8()?;
    let v = match c {
        0x00..=0x7f => c as i64,
        CODE_NEG_INT8 => read_neg_int8(r)?,
        CODE_INT16 => r.read_i16::<LittleEndian>()? as i64,
        CODE_INT32 => r.read_i32::<LittleEndian>()? as i64,
        CODE_INT64 => {
            let v = r.read_i64::<LittleEndian>()?;
            if !(OCAML_MIN_INT..=OCAML_MAX_INT).contains(&v) {
                return Err(invalid("Int_overflow"));
            }
            v
        }
        _ => return Err(invalid("Int_code")),
    };
    Ok(v)
}

/// `bin_read_int32`: as `bin_read_int` but with no 8-byte form (`0xfc` is
/// `Int32_code`).
///
/// bin_prot: <https://github.com/janestreet/bin_prot/blob/v0.14.0/src/read.ml#L326-L341>
pub fn read_int32<R: Read + ?Sized>(r: &mut R) -> std::io::Result<i32> {
    let c = r.read_u8()?;
    let v = match c {
        0x00..=0x7f => c as i32,
        CODE_NEG_INT8 => read_neg_int8(r)? as i32,
        CODE_INT16 => r.read_i16::<LittleEndian>()? as i32,
        CODE_INT32 => r.read_i32::<LittleEndian>()?,
        _ => return Err(invalid("Int32_code")),
    };
    Ok(v)
}

/// `bin_read_int64`: as `bin_read_int`, with any 64-bit value after `0xfc`.
///
/// bin_prot: <https://github.com/janestreet/bin_prot/blob/v0.14.0/src/read.ml#L343-L360>
pub fn read_int64<R: Read + ?Sized>(r: &mut R) -> std::io::Result<i64> {
    let c = r.read_u8()?;
    let v = match c {
        0x00..=0x7f => c as i64,
        CODE_NEG_INT8 => read_neg_int8(r)?,
        CODE_INT16 => r.read_i16::<LittleEndian>()? as i64,
        CODE_INT32 => r.read_i32::<LittleEndian>()? as i64,
        CODE_INT64 => r.read_i64::<LittleEndian>()?,
        _ => return Err(invalid("Int64_code")),
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nat0(bytes: &[u8]) -> Option<u64> {
        let mut r = bytes;
        let v = read_nat0(&mut r).ok()?;
        assert!(r.is_empty(), "{bytes:?} left bytes");
        Some(v)
    }

    fn int(bytes: &[u8]) -> Option<i64> {
        let mut r = bytes;
        let v = read_int(&mut r).ok()?;
        assert!(r.is_empty(), "{bytes:?} left bytes");
        Some(v)
    }

    fn int32(bytes: &[u8]) -> Option<i32> {
        let mut r = bytes;
        let v = read_int32(&mut r).ok()?;
        assert!(r.is_empty(), "{bytes:?} left bytes");
        Some(v)
    }

    fn int64(bytes: &[u8]) -> Option<i64> {
        let mut r = bytes;
        let v = read_int64(&mut r).ok()?;
        assert!(r.is_empty(), "{bytes:?} left bytes");
        Some(v)
    }

    fn with(code: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![code];
        v.extend_from_slice(payload);
        v
    }

    /// Checks that incorrect input for negative byte is rejected.
    #[test]
    fn invalid_encoding() {
        let mut encoded = &[CODE_NEG_INT8, 0][..];
        assert!(read_int(&mut encoded).is_err())
    }

    /// The single-byte forms: `0x00` to `0x7f` are values, `0x80` to `0xfb`
    /// are invalid codes for every integer type.
    #[test]
    fn single_byte_codes() {
        for c in 0x00..=0x7fu8 {
            assert_eq!(nat0(&[c]), Some(c as u64));
            assert_eq!(int(&[c]), Some(c as i64));
            assert_eq!(int32(&[c]), Some(c as i32));
            assert_eq!(int64(&[c]), Some(c as i64));
        }
        for c in 0x80..=0xfbu8 {
            assert_eq!(nat0(&[c]), None, "{c:#x}");
            assert_eq!(int(&[c]), None, "{c:#x}");
            assert_eq!(int32(&[c]), None, "{c:#x}");
            assert_eq!(int64(&[c]), None, "{c:#x}");
        }
        // 0xff is not a nat0 code.
        assert_eq!(nat0(&[0xff, 0x80]), None);
    }

    #[test]
    fn neg_int8_boundaries() {
        assert_eq!(int(&[0xff, 0xff]), Some(-1));
        assert_eq!(int(&[0xff, 0x80]), Some(-128));
        assert_eq!(int(&[0xff, 0x00]), None);
        assert_eq!(int(&[0xff, 0x7f]), None);
        assert_eq!(int32(&[0xff, 0x80]), Some(-128));
        assert_eq!(int32(&[0xff, 0x01]), None);
        assert_eq!(int64(&[0xff, 0xff]), Some(-1));
        assert_eq!(int64(&[0xff, 0x00]), None);
    }

    /// 16-bit forms, including non-minimal ones, which bin_prot accepts.
    #[test]
    fn int16_boundaries() {
        assert_eq!(nat0(&[0xfe, 0x05, 0x00]), Some(5));
        assert_eq!(nat0(&[0xfe, 0xff, 0xff]), Some(0xffff));
        assert_eq!(int(&[0xfe, 0x05, 0x00]), Some(5));
        assert_eq!(int(&[0xfe, 0xff, 0x7f]), Some(i16::MAX as i64));
        assert_eq!(int(&[0xfe, 0x00, 0x80]), Some(i16::MIN as i64));
        assert_eq!(int(&[0xfe, 0xff, 0xff]), Some(-1));
        assert_eq!(int32(&[0xfe, 0x05, 0x00]), Some(5));
        assert_eq!(int64(&[0xfe, 0x00, 0x80]), Some(i16::MIN as i64));
        assert_eq!(nat0(&[0xfe, 0x05]), None, "truncated");
    }

    #[test]
    fn int32_boundaries() {
        assert_eq!(nat0(&[0xfd, 5, 0, 0, 0]), Some(5));
        assert_eq!(nat0(&[0xfd, 0xff, 0xff, 0xff, 0xff]), Some(u32::MAX as u64));
        assert_eq!(int(&with(0xfd, &i32::MAX.to_le_bytes())), Some(i32::MAX as i64));
        assert_eq!(int(&with(0xfd, &i32::MIN.to_le_bytes())), Some(i32::MIN as i64));
        assert_eq!(int32(&with(0xfd, &i32::MIN.to_le_bytes())), Some(i32::MIN));
        assert_eq!(int32(&with(0xfd, &(-1i32).to_le_bytes())), Some(-1));
        assert_eq!(int64(&with(0xfd, &i32::MIN.to_le_bytes())), Some(i32::MIN as i64));
        assert_eq!(int32(&[0xfd, 5, 0, 0]), None, "truncated");
    }

    /// The 8-byte form: nat0 and int stop at OCaml's 63-bit range, int32 has
    /// no such form, int64 takes every value.
    #[test]
    fn int64_boundaries() {
        let max = with(0xfc, &OCAML_MAX_INT.to_le_bytes());
        let over = with(0xfc, &(OCAML_MAX_INT + 1).to_le_bytes());
        let min = with(0xfc, &OCAML_MIN_INT.to_le_bytes());
        let under = with(0xfc, &(OCAML_MIN_INT - 1).to_le_bytes());
        let five = with(0xfc, &5i64.to_le_bytes());
        let minus_one = with(0xfc, &(-1i64).to_le_bytes());

        assert_eq!(nat0(&max), Some(OCAML_MAX_INT as u64));
        assert_eq!(nat0(&over), None);
        assert_eq!(nat0(&minus_one), None);
        assert_eq!(nat0(&five), Some(5));

        assert_eq!(int(&max), Some(OCAML_MAX_INT));
        assert_eq!(int(&over), None);
        assert_eq!(int(&min), Some(OCAML_MIN_INT));
        assert_eq!(int(&under), None);
        assert_eq!(int(&minus_one), Some(-1));

        assert_eq!(int32(&five), None);

        assert_eq!(int64(&over), Some(OCAML_MAX_INT + 1));
        assert_eq!(int64(&under), Some(OCAML_MIN_INT - 1));
        assert_eq!(int64(&with(0xfc, &i64::MAX.to_le_bytes())), Some(i64::MAX));
        assert_eq!(int64(&with(0xfc, &i64::MIN.to_le_bytes())), Some(i64::MIN));
    }

    /// Our writer only writes what the readers read back.
    #[test]
    fn written_forms_read_back() {
        for v in [
            0i64,
            1,
            0x7f,
            0x80,
            0x7fff,
            0x8000,
            0x7fff_ffff,
            0x8000_0000,
            OCAML_MAX_INT,
            -1,
            -128,
            -129,
            -0x8000,
            -0x8001,
            i32::MIN as i64,
            i32::MIN as i64 - 1,
            OCAML_MIN_INT,
        ] {
            let mut buf = Vec::new();
            write_i64(&mut buf, v).unwrap();
            assert_eq!(int(&buf), Some(v), "{v}");
            assert_eq!(int64(&buf), Some(v), "{v}");
        }
        for v in [0u64, 0x7f, 0x80, 0xffff, 0x1_0000, 0xffff_ffff, OCAML_MAX_INT as u64] {
            let mut buf = Vec::new();
            write_nat0(&mut buf, v).unwrap();
            assert_eq!(nat0(&buf), Some(v), "{v}");
        }
    }
}
