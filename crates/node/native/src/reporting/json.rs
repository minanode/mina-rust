//! Compact JSON with OCaml's `Yojson.Safe.to_string` spelling.
//!
//! The uptime signature covers the exact bytes of the `data` object, so the
//! reports are written in the field order OCaml's derived serializers use,
//! with Yojson's string escapes, rather than through `serde_json`, whose
//! map order and escapes differ.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L38-L49>

/// A JSON value, objects in insertion order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<Json>),
    Assoc(Vec<(String, Json)>),
}

impl Json {
    pub fn str(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    pub fn assoc<K: Into<String>>(fields: impl IntoIterator<Item = (K, Json)>) -> Self {
        Self::Assoc(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// `None` as `null`, as derived `to_yojson` writes options.
    pub fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> Json) -> Self {
        value.map_or(Self::Null, f)
    }

    /// Keys keep `serde_json`'s order (sorted, without `preserve_order`).
    /// Non-integer numbers keep `serde_json`'s spelling, which Yojson's
    /// float printing does not always match; the reports built here contain
    /// none outside free-form error details.
    pub fn from_serde(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(*b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Self::Int(i),
                None => Self::String(n.to_string()),
            },
            serde_json::Value::String(s) => Self::String(s.clone()),
            serde_json::Value::Array(items) => {
                Self::List(items.iter().map(Self::from_serde).collect())
            }
            serde_json::Value::Object(map) => Self::Assoc(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from_serde(v)))
                    .collect(),
            ),
        }
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(i) => out.push_str(&i.to_string()),
            Self::String(s) => write_string(out, s),
            Self::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Assoc(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, key);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_compact_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// Yojson's string body escapes: `"`, `\`, the short forms of backspace,
/// form feed, newline, carriage return and tab, and `\u00XX` for the other
/// control characters and DEL. Other bytes, UTF-8 included, are written as
/// they are.
///
/// OCaml (yojson 2.x `write.ml`, `write_string_body`).
pub fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The bytes `write_string` uses for `c`.
pub fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '\u{0}'..='\u{1f}' | '\u{7f}' => 6,
        c => c.len_utf8(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_ordered_and_escaped() {
        let json = Json::assoc([
            ("b", Json::Int(-1)),
            ("a", Json::str("q\"\\\n\u{1}\u{7f}é")),
            ("c", Json::List(vec![Json::Null, Json::Bool(true)])),
        ]);
        assert_eq!(
            json.to_compact_string(),
            r#"{"b":-1,"a":"q\"\\\n\u0001\u007fé","c":[null,true]}"#
        );
    }
}
