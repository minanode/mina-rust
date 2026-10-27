//! OCaml's error texts for malformed arguments, so that clients see the same
//! errors from either node.
//!
//! An argument OCaml's schema cannot coerce (a `PublicKey` or `TokenId` that
//! does not decode) fails before any resolver runs, and OCaml's GraphQL
//! server then answers with that one error and `data: null` for the whole
//! request. Here the resolver finds it, so the error carries a marker
//! extension ([`ARGUMENT_ERROR`]) and the HTTP handler turns the response
//! into OCaml's.

use juniper::{graphql_value, FieldError};

/// The extension that marks an argument error for the HTTP handler.
pub(crate) const ARGUMENT_ERROR: &str = "ocamlArgumentError";

/// A scalar argument OCaml's schema refuses, as ocaml-graphql-server words
/// it: the inner type name (no `!`) and a string value in quotes, unescaped.
///
/// OCaml: <https://github.com/andreas/ocaml-graphql-server/blob/0.13.0/graphql/src/graphql_schema.ml#L194-L205>
pub(crate) fn argument_error(
    arg_name: &str,
    type_name: &str,
    field_name: &str,
    value: &str,
) -> FieldError {
    FieldError::new(
        format!(
            "Argument `{arg_name}` of type `{type_name}` expected on field `{field_name}`, found \"{value}\"."
        ),
        graphql_value!({ "ocamlArgumentError": true }),
    )
}

/// OCaml's error for a Base58Check string that does not decode, checked in
/// its order: the characters, the length (a version byte and a checksum at
/// least), the version byte, then the checksum. `None` when the string
/// passes all four (its payload may still not be a valid value).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/base58_check/base58_check.ml#L62-L104>
pub(crate) fn base58_check_error(s: &str, version_byte: u8, description: &str) -> Option<String> {
    let error = |e: &str| {
        Some(format!(
            "Error decoding {s}\nInvalid base58 {e} in {description}"
        ))
    };
    let Ok(decoded) = bs58::decode(s).into_vec() else {
        return error("character");
    };
    if decoded.len() < 5 {
        return error("check length");
    }
    if decoded[0] != version_byte {
        return error(&format!(
            "version byte \\x{:02X}, expected \\x{version_byte:02X}",
            decoded[0]
        ));
    }
    if bs58::decode(s).with_check(None).into_vec().is_err() {
        return error("checksum");
    }
    None
}

/// The error ocaml-base64 3.4.0 gives for a string that is not padded
/// standard Base64, `None` for one it decodes. It reads the input in blocks
/// of four, each from its last character back: a block that runs past the
/// end is `Wrong padding` before its characters are looked at, a character
/// outside the alphabet (or a `=` not at the end of its block) is
/// `Malformed input`, and padding before the last block is `Too much input`
/// unless the rest is padding only.
///
/// OCaml: <https://github.com/mirage/ocaml-base64/blob/v3.4.0/src/base64.ml#L145-L265>
pub(crate) fn base64_error(s: &str) -> Option<&'static str> {
    const WRONG_PADDING: &str = "Wrong padding";
    const MALFORMED: &str = "Malformed input";
    let input = s.as_bytes();
    let len = input.len();
    let n = len.div_ceil(4) * 4;
    let alphabet = |c: u8| c.is_ascii_alphanumeric() || c == b'+' || c == b'/';
    let mut i = 0;
    while i < n {
        if i + 3 >= len {
            return Some(WRONG_PADDING);
        }
        // From the block's last character back: each `=` must follow the
        // ones after it.
        let mut pad = 0;
        for k in (0..4).rev() {
            let c = input[i + k];
            if alphabet(c) {
                continue;
            }
            if c == b'=' && pad == 3 - k {
                pad += 1;
                continue;
            }
            return Some(MALFORMED);
        }
        if i + 4 == n || pad == 0 {
            i += 4;
            continue;
        }
        // Padding before the last block: only `====` blocks and `=` may
        // follow a fully padded block; any other padding is too much input.
        if pad != 4 {
            return Some("Too much input");
        }
        return input[i + 4..]
            .iter()
            .any(|&c| c != b'=')
            .then_some(MALFORMED);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R32's OCaml devnet node: a public key that does not decode, a state
    /// hash with a character outside the alphabet, and a transaction id with
    /// the wrong length.
    #[test]
    fn texts_as_ocaml() {
        assert_eq!(
            argument_error("publicKey", "PublicKey", "account", "invalid").message(),
            "Argument `publicKey` of type `PublicKey` expected on field `account`, found \"invalid\"."
        );
        assert_eq!(
            base58_check_error("invalid", 0x10, "State hash").as_deref(),
            Some("Error decoding invalid\nInvalid base58 character in State hash")
        );
        assert_eq!(base64_error("invalid"), Some("Wrong padding"));
    }

    #[test]
    fn base58_check_in_ocaml_order() {
        let hash = "3NK5Kp6Qu2CrGJCt3cbRmM11Ehn11WhQSGUtfEmCNUXVx7rk33Xw";
        assert_eq!(base58_check_error(hash, 0x10, "State hash"), None);
        // `2` decodes to one byte: too short.
        assert_eq!(
            base58_check_error("2", 0x10, "State hash").as_deref(),
            Some("Error decoding 2\nInvalid base58 check length in State hash")
        );
        // A ledger hash is valid Base58Check with another version byte.
        let ledger = "jxJp66JhmUbFo4U9rYiELCHQ2hgXzuHwYJffh1yY4Lp93hfH1X4";
        assert_eq!(
            base58_check_error(ledger, 0x10, "State hash").as_deref(),
            Some(&*format!(
                "Error decoding {ledger}\nInvalid base58 version byte \\x05, expected \\x10 in State hash"
            ))
        );
        // The last character changed: the checksum fails.
        let bad = format!("{}x", &hash[..hash.len() - 1]);
        assert_eq!(
            base58_check_error(&bad, 0x10, "State hash").as_deref(),
            Some(&*format!(
                "Error decoding {bad}\nInvalid base58 checksum in State hash"
            ))
        );
    }

    #[test]
    fn base64_as_ocaml_base64() {
        assert_eq!(base64_error(""), None);
        assert_eq!(base64_error("QUJD"), None);
        assert_eq!(base64_error("QUI="), None);
        assert_eq!(base64_error("QQ=="), None);
        assert_eq!(base64_error("QQ==QUJD"), Some("Too much input"));
        assert_eq!(base64_error("QUJD===="), None);
        // The block running past the end is checked before its characters.
        assert_eq!(base64_error("QUJD!"), Some("Wrong padding"));
        assert_eq!(base64_error("!UJDQ"), Some("Malformed input"));
        assert_eq!(base64_error("Q=JD"), Some("Malformed input"));
        assert_eq!(base64_error("QUJ-"), Some("Malformed input"));
    }
}
