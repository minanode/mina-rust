//! Scalars that OCaml's schema uses for arguments.
//!
//! Clients declare their variables with OCaml's scalar names, for example
//! `query Account($publicKey: PublicKey!, $token: TokenId)` in the Mina SDKs
//! and the Auro wallet. A variable must have the type of the argument it is
//! passed to, so an argument typed `String` rejects them. These scalars carry
//! the same strings, and still accept inline string literals.

use juniper::{
    GraphQLScalar, ParseError, ParseScalarResult, ParseScalarValue, Scalar, ScalarToken,
    ScalarValue,
};

/// Base58Check-encoded public key.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2479-L2494>
#[derive(Clone, Debug, GraphQLScalar)]
#[graphql(name = "PublicKey", transparent)]
pub struct PublicKeyScalar(pub String);

/// Base58Check-encoded token id.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2505-L2520>
#[derive(Clone, Debug, GraphQLScalar)]
#[graphql(name = "TokenId", transparent)]
pub struct TokenIdScalar(pub String);

/// An unsigned integer argument, coerced as OCaml's `make_numeric_arg` does.
///
/// OCaml takes a string or an integer:
///
/// - a string must be the number in base 10 exactly as OCaml prints it: it
///   parses it, prints it back and requires the two to be equal, so `"02"`,
///   `"+2"`, `" 2"`, `"1_000"` and values out of range are refused ("Could
///   not decode uint64.");
/// - an integer must not be negative ("Could not convert negative number to
///   uint64."), and is converted with `of_int`, which for `UInt32` keeps the
///   low 32 bits;
/// - anything else is refused ("Invalid format for uint64 type.").
///
/// Juniper keeps integers above 2^31 as floats, from JSON variables and here
/// from literals, so an integral float stands for such an integer; OCaml's
/// integers stop at 2^62 (larger JSON integers are not `Int` to it). A float
/// literal in a query is refused, as OCaml refuses it, but a JSON variable
/// written as `3.0` can't be told from `3` and is taken.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2734-L2797>
fn coerce_unsigned<S: ScalarValue>(v: &Scalar<S>, name: &str, bits: u32) -> Result<String, String> {
    let max: u64 = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let decode = || format!("Could not decode {name}.");
    let negative = || format!("Could not convert negative number to {name}.");
    let format = || format!("Invalid format for {name} type.");

    if let Some(s) = v.try_to_string() {
        crate::input_limits::bounded(&s, if bits == 64 { 20 } else { 10 }).map_err(|_| decode())?;
        // Canonical decimal only: OCaml prints the parsed value back and
        // compares.
        let canonical = !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0'));
        let value = s.parse::<u64>().ok().filter(|v| canonical && *v <= max);
        return value.map(|v| v.to_string()).ok_or_else(decode);
    }
    // OCaml's `of_int` for `Unsigned.UInt32` keeps the low 32 bits.
    let wrap = |n: u64| if bits == 64 { n } else { n & max };
    if let Some(i) = v.try_to_int() {
        if i < 0 {
            return Err(negative());
        }
        return Ok(wrap(i as u64).to_string());
    }
    if let Some(f) = v.try_to_float() {
        const OCAML_MAX_INT: f64 = ((1u64 << 62) - 1) as f64;
        if !f.is_finite() || f.fract() != 0.0 || f > OCAML_MAX_INT {
            return Err(format());
        }
        if f < 0.0 {
            return Err(negative());
        }
        return Ok(wrap(f as u64).to_string());
    }
    Err(format())
}

/// Literals: strings and integers as juniper parses them, integers beyond
/// 32 bits as floats (as juniper keeps them from JSON), and float literals
/// refused, as OCaml refuses them for an integer type.
fn parse_unsigned_token<S: ScalarValue>(token: ScalarToken<'_>) -> ParseScalarResult<S> {
    match token {
        ScalarToken::String(_) => <String as ParseScalarValue<S>>::from_str(token),
        ScalarToken::Int(digits) => match digits.parse::<i32>() {
            Ok(_) => <i32 as ParseScalarValue<S>>::from_str(token),
            Err(_) => digits
                .parse::<f64>()
                .map(S::from)
                .map_err(|_| ParseError::ExpectedScalarError("an integer")),
        },
        // Juniper 0.17 retries a failed custom parser with the built-in Float
        // parser when that type exists in the schema (e.g. verification
        // timings). Preserve the literal spelling so coerce_unsigned rejects
        // its decimal point/exponent instead of accepting an integral f64.
        ScalarToken::Float(digits) => Ok(S::from(digits.to_owned())),
    }
}

/// A 64-bit unsigned integer: amounts and fees in nanomina. Holds the value
/// in canonical decimal.
#[derive(Clone, Debug, GraphQLScalar)]
#[graphql(name = "UInt64", parse_token_with = parse_unsigned_token)]
pub struct UInt64Scalar(pub String);

impl UInt64Scalar {
    fn to_output(&self) -> &str {
        &self.0
    }

    fn from_input<S: ScalarValue>(v: &Scalar<S>) -> Result<Self, String> {
        coerce_unsigned(v, "uint64", 64).map(Self)
    }
}

/// A 32-bit unsigned integer: nonces and slots. Holds the value in canonical
/// decimal.
#[derive(Clone, Debug, GraphQLScalar)]
#[graphql(name = "UInt32", parse_token_with = parse_unsigned_token)]
pub struct UInt32Scalar(pub String);

impl UInt32Scalar {
    fn to_output(&self) -> &str {
        &self.0
    }

    fn from_input<S: ScalarValue>(v: &Scalar<S>) -> Result<Self, String> {
        coerce_unsigned(v, "uint32", 32).map(Self)
    }
}

// OCaml output scalars serialize as strings, but their schema names matter
// for fragments and introspection-driven clients.
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1771-L1843>
macro_rules! output_string_scalar {
    ($ty:ident, $name:literal) => {
        #[derive(Clone, Debug, GraphQLScalar)]
        #[graphql(name = $name, transparent)]
        pub struct $ty(pub String);
    };
}
output_string_scalar!(TransactionIdScalar, "TransactionId");
output_string_scalar!(TransactionHashScalar, "TransactionHash");
output_string_scalar!(UserCommandKindScalar, "UserCommandKind");
output_string_scalar!(GlobalSlotScalar, "Globalslot");
output_string_scalar!(AmountScalar, "Amount");
output_string_scalar!(FeeScalar, "Fee");
output_string_scalar!(TransactionStatusFailureScalar, "TransactionStatusFailure");
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/graphql_lib/mina_base/graphql_scalars.ml#L90-L98>
output_string_scalar!(FieldElemScalar, "FieldElem");

/// OCaml exposes the whole UInt32 nonce as GraphQL Int (a 63-bit OCaml int).
/// Keep Juniper's standard Int input coercion, but avoid wrapping large output
/// nonces to negative i32 values. Every UInt32 is exactly representable in f64;
/// DefaultScalarValue uses that numeric variant above i32::MAX.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1863-L1868>
#[derive(Clone, Debug, GraphQLScalar)]
#[graphql(name = "Int", parse_token(i32))]
pub struct CommandNonceScalar(pub i64);

impl CommandNonceScalar {
    fn to_output<S: ScalarValue>(&self) -> S {
        match i32::try_from(self.0) {
            Ok(nonce) => S::from(nonce),
            Err(_) => S::from(self.0 as f64),
        }
    }

    fn from_input<S: ScalarValue>(value: &Scalar<S>) -> Result<Self, String> {
        value
            .try_to_int()
            .map(|n| Self(i64::from(n)))
            .ok_or_else(|| "Expected an Int".to_owned())
    }
}
