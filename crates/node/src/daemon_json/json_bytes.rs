//! OCaml strings in runtime-config JSON. A token symbol or zkApp URI is any
//! bytes, and Yojson writes a string's bytes as they are, so a value that is
//! not UTF-8 (mainnet account `B62qjoWP2dCbr8z1Zt9XRhYsKJaDr66kNTJKGh9KKUbQz4hfphMduEZ`
//! has token symbol `0xff` since block 565083) makes the file invalid JSON.
//! OCaml reads it back byte for byte. No valid JSON form does that: OCaml
//! reads `"ÿ"` as the two bytes `c3 bf` and rejects `[255]`.
//!
//! [`write_ocaml_string`] writes the bytes as Yojson does. [`OcamlJsonReader`]
//! turns each string that is not UTF-8 into an array of its bytes, which the
//! byte-string fields of [`super::Account`] read.

use std::io::{self, BufRead, Read, Write};

use serde::{de::DeserializeOwned, Serialize, Serializer};

/// A string field that holds any bytes. `serde_json` writes it as an array of
/// bytes; [`super::write_accounts`] writes it as OCaml does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcamlString(pub Vec<u8>);

impl Serialize for OcamlString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

/// `serde_json`'s compact layout, with byte strings written by
/// [`write_ocaml_string`].
pub(super) struct OcamlFormatter;

impl serde_json::ser::Formatter for OcamlFormatter {
    fn write_byte_array<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        value: &[u8],
    ) -> io::Result<()> {
        write_ocaml_string(writer, value)
    }
}

/// Writes `value` as a JSON string, as Yojson does: the bytes as they are,
/// except `"`, `\` and the control characters, which are escaped.
///
/// OCaml: <https://github.com/ocaml-community/yojson/blob/1.7.0/lib/write.ml#L31-L46>
pub fn write_ocaml_string<W: ?Sized + Write>(writer: &mut W, value: &[u8]) -> io::Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    writer.write_all(b"\"")?;
    let mut start = 0;
    for (i, &byte) in value.iter().enumerate() {
        let control;
        let escaped: &[u8] = match byte {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            0x08 => b"\\b",
            0x0c => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0x00..=0x1f | 0x7f => {
                control = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xf)],
                ];
                &control
            }
            _ => continue,
        };
        writer.write_all(&value[start..i])?;
        writer.write_all(escaped)?;
        start = i + 1;
    }
    writer.write_all(&value[start..])?;
    writer.write_all(b"\"")
}

/// Reads OCaml's JSON (a runtime config or `mina ledger export`) as
/// `serde_json` would read it if each string that is not UTF-8 were an array
/// of its bytes. A string serde_json would read differently from Yojson (raw
/// control characters, `\u` surrogates) is decoded as Yojson decodes it.
/// Other strings pass unchanged. Only one string is held at a time. An object
/// key that is not UTF-8 becomes an array, which serde_json rejects.
pub struct OcamlJsonReader<R> {
    inner: R,
    out: Vec<u8>,
    out_pos: usize,
    in_string: bool,
    escaped: bool,
    string: Vec<u8>,
}

impl<R: BufRead> OcamlJsonReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            out: Vec::new(),
            out_pos: 0,
            in_string: false,
            escaped: false,
            string: Vec::new(),
        }
    }

    /// Converts the next chunk of input into `out`. False at the end.
    fn convert_chunk(&mut self) -> io::Result<bool> {
        let Self {
            inner,
            out,
            in_string,
            escaped,
            string,
            ..
        } = self;
        let chunk = inner.fill_buf()?;
        if chunk.is_empty() {
            if *in_string {
                // Unterminated: let serde_json report it.
                out.push(b'"');
                out.append(string);
                *in_string = false;
                return Ok(!out.is_empty());
            }
            return Ok(false);
        }
        let mut i = 0;
        while i < chunk.len() {
            if !*in_string {
                let end = chunk[i..]
                    .iter()
                    .position(|&b| b == b'"')
                    .map_or(chunk.len(), |p| i + p);
                out.extend_from_slice(&chunk[i..end]);
                if end < chunk.len() {
                    *in_string = true;
                    string.clear();
                }
                i = end + 1;
            } else if *escaped {
                string.push(chunk[i]);
                *escaped = false;
                i += 1;
            } else {
                let end = chunk[i..]
                    .iter()
                    .position(|&b| b == b'"' || b == b'\\')
                    .map_or(chunk.len(), |p| i + p);
                string.extend_from_slice(&chunk[i..end]);
                match chunk.get(end) {
                    Some(b'\\') => {
                        string.push(b'\\');
                        *escaped = true;
                    }
                    Some(_) => {
                        push_string(out, string)?;
                        *in_string = false;
                    }
                    None => {}
                }
                i = end + 1;
            }
        }
        let used = chunk.len();
        inner.consume(used);
        Ok(true)
    }
}

impl<R: BufRead> Read for OcamlJsonReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.out_pos == self.out.len() {
            self.out.clear();
            self.out_pos = 0;
            if !self.convert_chunk()? {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.out.len() - self.out_pos);
        buf[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
        self.out_pos += n;
        Ok(n)
    }
}

/// Writes a string's `raw` content (escapes not decoded) to `out`. A string
/// that serde_json reads as Yojson does passes as it is. Any other is decoded
/// as Yojson decodes it, then written as a JSON string when the bytes are
/// UTF-8 and as an array of the bytes otherwise.
fn push_string(out: &mut Vec<u8>, raw: &[u8]) -> io::Result<()> {
    if !yojson_only(raw) {
        out.push(b'"');
        out.extend_from_slice(raw);
        out.push(b'"');
        return Ok(());
    }
    let bytes = unescape(raw)?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return serde_json::to_writer(out, s).map_err(io::Error::from);
    }
    out.push(b'[');
    for (i, byte) in bytes.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write!(out, "{byte}")?;
    }
    out.push(b']');
    Ok(())
}

/// Whether Yojson accepts string content `raw` that serde_json rejects:
/// bytes that are not UTF-8, raw control characters, or a `\u` surrogate
/// (serde_json refuses a lone one; a pair is decoded the same by both).
fn yojson_only(raw: &[u8]) -> bool {
    if std::str::from_utf8(raw).is_err() || raw.iter().any(|&b| b < 0x20) {
        return true;
    }
    let mut i = 0;
    while let Some(p) = raw
        .get(i..)
        .and_then(|rest| rest.iter().position(|&b| b == b'\\'))
    {
        let escape = i + p + 1;
        if raw.get(escape) == Some(&b'u')
            && hex4(raw.get(escape + 1..escape + 5)).is_some_and(|c| (0xd800..=0xdfff).contains(&c))
        {
            return true;
        }
        i = escape + 1;
    }
    false
}

fn hex4(hex: Option<&[u8]>) -> Option<u32> {
    let hex = hex.filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))?;
    u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

/// Decodes the escapes of a string as Yojson does: a high surrogate must be
/// followed by an escaped low one, and a lone low surrogate is encoded like
/// any other code point.
///
/// OCaml: <https://github.com/ocaml-community/yojson/blob/1.7.0/lib/read.mll#L328-L361>
fn unescape(raw: &[u8]) -> io::Result<Vec<u8>> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let byte = raw[i];
        i += 1;
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let escape = *raw.get(i).ok_or_else(|| invalid("bad escape"))?;
        i += 1;
        match escape {
            b'"' | b'\\' | b'/' => bytes.push(escape),
            b'b' => bytes.push(0x08),
            b'f' => bytes.push(0x0c),
            b'n' => bytes.push(b'\n'),
            b'r' => bytes.push(b'\r'),
            b't' => bytes.push(b'\t'),
            b'u' => {
                let code = hex4(raw.get(i..i + 4)).ok_or_else(|| invalid("bad \\u escape"))?;
                i += 4;
                if (0xd800..=0xdbff).contains(&code) {
                    let low = (raw.get(i..i + 2) == Some(&b"\\u"[..]))
                        .then(|| hex4(raw.get(i + 2..i + 6)))
                        .flatten()
                        .filter(|low| (0xdc00..=0xdfff).contains(low))
                        .ok_or_else(|| invalid("a high surrogate without a low one"))?;
                    i += 6;
                    push_utf8(
                        &mut bytes,
                        0x10000 + (((code - 0xd800) << 10) | (low - 0xdc00)),
                    );
                } else {
                    push_utf8(&mut bytes, code);
                }
            }
            _ => return Err(invalid("bad escape")),
        }
    }
    Ok(bytes)
}

/// Yojson's `utf8_of_code`, which also encodes a lone surrogate.
///
/// OCaml: <https://github.com/ocaml-community/yojson/blob/1.7.0/lib/common.ml#L21-L47>
fn push_utf8(bytes: &mut Vec<u8>, code: u32) {
    let continuation = |shift: u32| 0x80 | ((code >> shift) & 0x3f) as u8;
    match code {
        0..=0x7f => bytes.push(code as u8),
        0x80..=0x7ff => bytes.extend([0xc0 | (code >> 6) as u8, continuation(0)]),
        0x800..=0xffff => {
            bytes.extend([0xe0 | (code >> 12) as u8, continuation(6), continuation(0)])
        }
        _ => bytes.extend([
            0xf0 | (code >> 18) as u8,
            continuation(12),
            continuation(6),
            continuation(0),
        ]),
    }
}

/// `serde_json::from_reader` for OCaml's JSON. Buffer the reader.
pub fn from_ocaml_json_reader<T: DeserializeOwned, R: BufRead>(reader: R) -> serde_json::Result<T> {
    // serde_json reads a byte at a time; only a `BufReader` serves that from
    // its buffer without a call per byte.
    serde_json::from_reader(io::BufReader::with_capacity(
        1 << 16,
        OcamlJsonReader::new(reader),
    ))
}

/// `serde_json::from_slice` for OCaml's JSON.
pub fn from_ocaml_json_slice<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    from_ocaml_json_reader(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_ocaml_string(&mut out, value).unwrap();
        out
    }

    fn converted(input: &[u8], capacity: usize) -> Vec<u8> {
        let mut out = Vec::new();
        OcamlJsonReader::new(io::BufReader::with_capacity(capacity, input))
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    /// Yojson 1.7.0's `write_string` (the escaping rules in its
    /// `write_string_body`).
    #[test]
    fn writes_as_yojson() {
        assert_eq!(written(b"MINA"), b"\"MINA\"");
        assert_eq!(written(b"\xff"), b"\"\xff\"");
        assert_eq!(written("é".as_bytes()), "\"é\"".as_bytes());
        assert_eq!(
            written(b"\"\\\x08\x0c\n\r\t\x00\x1f\x7f/"),
            b"\"\\\"\\\\\\b\\f\\n\\r\\t\\u0000\\u001f\\u007f/\""
        );
    }

    #[test]
    fn non_utf8_strings_become_arrays() {
        let input =
            b"{\"a\":\"\xff\",\"b\":\"MINA\",\"c\":\"\\\"\xc3\\u0000\\n\",\"d\":\"\\\"\xc3\xa9\"}";
        let expected: &[u8] =
            b"{\"a\":[255],\"b\":\"MINA\",\"c\":[34,195,0,10],\"d\":\"\\\"\xc3\xa9\"}";
        // Every buffer size, so strings and escapes span chunk ends.
        for capacity in 1..=input.len() {
            assert_eq!(converted(input, capacity), expected, "capacity {capacity}");
        }
    }

    /// Strings serde_json would read differently from Yojson are decoded as
    /// Yojson decodes them (lib/read.mll, lib/common.ml `utf8_of_code`).
    #[test]
    fn yojson_escapes() {
        let cases: [(&[u8], &[u8]); 6] = [
            (b"\"\xff\\uD83D\\uDE00\"", b"[255,240,159,152,128]"),
            (b"\"\xff\\udc00\"", b"[255,237,176,128]"),
            (b"\"\x01A\"", b"\"\\u0001A\""),
            (b"\"\\ud83d\\ude00\"", "\"\u{1f600}\"".as_bytes()),
            // An escaped backslash, then the text uD800: no escape.
            (b"\"\\\\uD800\"", b"\"\\\\uD800\""),
            (b"\"\\u00e9\"", b"\"\\u00e9\""),
        ];
        for (input, expected) in cases {
            assert_eq!(converted(input, input.len()), expected);
        }
        assert!(!yojson_only(b"ab\\"));
        // Yojson rejects a high surrogate without a low one.
        let mut out = Vec::new();
        let result = OcamlJsonReader::new(&b"\"\xff\\uD83D\""[..]).read_to_end(&mut out);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn written_bytes_read_back() {
        let mut values: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
        values.push((0..=255u8).collect());
        values.push(b"\xc3".to_vec());
        values.push("é".as_bytes().to_vec());
        for value in values {
            let back = match from_ocaml_json_slice(&written(&value)).unwrap() {
                serde_json::Value::String(s) => s.into_bytes(),
                serde_json::Value::Array(bytes) => bytes
                    .iter()
                    .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
                    .collect(),
                other => panic!("{other}"),
            };
            assert_eq!(back, value);
        }
    }
}
