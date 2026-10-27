use std::{fmt::Display, marker::PhantomData, str::FromStr};

use malloc_size_of::MallocSizeOf;
use serde::{de::Visitor, Deserialize, Serialize};

#[derive(
    Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Deref,
)]
pub struct Number<T>(pub T);

impl<T: std::fmt::Debug> std::fmt::Debug for Number<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid vertical alignment
        f.write_fmt(format_args!("Number({inner:?})", inner = self.0))
    }
}

impl<T> MallocSizeOf for Number<T> {
    fn size_of(&self, _ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        0
    }
}

pub type Int32 = Number<i32>;
pub type UInt32 = Number<u32>;
pub type Int64 = Number<i64>;
pub type UInt64 = Number<u64>;
pub type Float64 = Number<f64>;

impl Int32 {
    pub const fn as_u32(&self) -> u32 {
        self.0 as u32
    }
}

impl Int64 {
    pub const fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

impl UInt32 {
    pub const fn as_u32(&self) -> u32 {
        self.0
    }
}

impl UInt64 {
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u32> for Number<i32> {
    fn from(value: u32) -> Self {
        Self(value as i32)
    }
}

impl From<u64> for Number<i64> {
    fn from(value: u64) -> Self {
        Self(value as i64)
    }
}

impl From<&u32> for Number<i32> {
    fn from(value: &u32) -> Self {
        Self(*value as i32)
    }
}

impl From<&u64> for Number<i64> {
    fn from(value: &u64) -> Self {
        Self(*value as i64)
    }
}

impl From<&u32> for Number<u32> {
    fn from(value: &u32) -> Self {
        Self(*value)
    }
}

impl From<&u64> for Number<u64> {
    fn from(value: &u64) -> Self {
        Self(*value)
    }
}

impl<T> Serialize for Number<T>
where
    T: Serialize + Display,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !serializer.is_human_readable() {
            return self.0.serialize(serializer);
        }
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de, T> Deserialize<'de> for Number<T>
where
    T: Deserialize<'de> + FromStr,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if !deserializer.is_human_readable() {
            return T::deserialize(deserializer).map(Self);
        }
        struct V<T>(PhantomData<T>);
        impl<'de, T> Visitor<'de> for V<T>
        where
            T: FromStr,
        {
            type Value = T;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a stringified number or a literal integer")
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(|_| {
                    serde::de::Error::custom("failed to parse string as number".to_string())
                })
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(|_| {
                    serde::de::Error::custom("failed to parse string as number".to_string())
                })
            }

            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(|_| {
                    serde::de::Error::custom("failed to parse string as number".to_string())
                })
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let s = v.to_string();
                s.parse()
                    .map_err(|_| serde::de::Error::custom("failed to parse integer as number"))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let s = v.to_string();
                s.parse().map_err(|_| {
                    serde::de::Error::custom("failed to parse unsigned integer as number")
                })
            }
        }
        deserializer
            .deserialize_any(V::<T>(Default::default()))
            .map(Self)
    }
}

/// Each number type reads with the bin_prot reader of the OCaml type it
/// stands for, so that we accept exactly what OCaml accepts:
///
/// - `Int32` and `UInt32` are OCaml `int32` (`Unsigned_extended.UInt32` is
///   binable as `Int32`): `bin_read_int32`, which has no 8-byte form;
/// - `Int64` and `UInt64` are OCaml `int` (63 bits): `bin_read_int`. The two
///   wire types that are OCaml `Int64` (`Unsigned_extended.UInt64` and
///   Pickles' `Limb_vector.Constant.Hex64`) read with [`read_ocaml_int64`]
///   instead (`v2::manual`).
macro_rules! binprot_number {
    ($base_type:ident, $binprot_type:ident, $read:path) => {
        impl binprot::BinProtRead for Number<$base_type> {
            fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
            where
                Self: Sized,
            {
                Ok(Self($read(r)? as $base_type))
            }
        }

        impl binprot::BinProtWrite for Number<$base_type> {
            fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
                (self.0 as $binprot_type).binprot_write(w)
            }
        }
    };
}

binprot_number!(i32, i32, binprot::read_int32);
binprot_number!(i64, i64, binprot::read_int);
binprot_number!(u32, i32, binprot::read_int32);
binprot_number!(u64, i64, binprot::read_int);

impl binprot::BinProtRead for Number<f64> {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        f64::binprot_read(r).map(Self)
    }
}

impl binprot::BinProtWrite for Number<f64> {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.0.binprot_write(w)
    }
}

/// A `UInt64` that is an OCaml `Int64` on the wire: `bin_read_int64`, which
/// takes every 64-bit value after `0xfc`, where OCaml's `int` stops at 2^62.
pub fn read_ocaml_int64<R: std::io::Read + ?Sized>(r: &mut R) -> Result<UInt64, binprot::Error> {
    Ok(Number(binprot::read_int64(r)? as u64))
}

#[cfg(feature = "openapi")]
const _: () = {
    use utoipa::{
        openapi::schema::{Object, SchemaType, Type},
        PartialSchema, ToSchema,
    };

    // Number<T> serializes to string in human-readable format.
    // TODO(openapi): specialize schema per T (signed vs unsigned, bit width)
    impl<T: 'static> PartialSchema for Number<T> {
        fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
            Object::builder()
                .schema_type(SchemaType::Type(Type::String))
                .description(Some("Numeric value encoded as a string of digits"))
                .pattern(Some(r"^-?\d+$"))
                .build()
                .into()
        }
    }
    impl<T: 'static> ToSchema for Number<T> {
        fn name() -> std::borrow::Cow<'static, str> {
            std::borrow::Cow::Borrowed("Number")
        }
    }
};

#[cfg(test)]
mod tests {
    use binprot::{BinProtRead, BinProtWrite};

    macro_rules! number_test {
        ($name:ident, $ty:ident) => {
            #[test]
            fn $name() {
                for n in [
                    0,
                    1,
                    u8::MAX as $ty,
                    u16::MAX as $ty,
                    u32::MAX as $ty,
                    u64::MAX as $ty,
                    i8::MAX as $ty,
                    i16::MAX as $ty,
                    i32::MAX as $ty,
                    // OCaml's `max_int`, the largest value an OCaml `int`
                    // holds on a 64-bit platform.
                    ((1i64 << 62) - 1) as $ty,
                ] {
                    let n: super::Number<$ty> = n.into();
                    let mut buf = Vec::new();
                    n.binprot_write(&mut buf).unwrap();
                    let mut r = buf.as_slice();
                    let n_ = super::Number::<$ty>::binprot_read(&mut r).unwrap();
                    assert_eq!(r.len(), 0);
                    assert_eq!(n, n_);
                }
            }
        };
    }

    macro_rules! max_number_test {
        ($name:ident, $ty:ident) => {
            #[test]
            fn $name() {
                let binprot = b"\xff\xff";
                let mut r = &binprot[..];
                let n = super::Number::<$ty>::binprot_read(&mut r).unwrap();
                assert_eq!(n.0, $ty::MAX);

                let n: super::Number<$ty> = $ty::MAX.into();
                let mut buf = Vec::new();
                n.binprot_write(&mut buf).unwrap();
                assert_eq!(buf.as_slice(), b"\xff\xff");
            }
        };
    }

    number_test!(i32_roundtrip, i32);
    number_test!(u32_roundtrip, u32);
    number_test!(i64_roundtrip, i64);
    number_test!(u64_roundtrip, u64);

    max_number_test!(u32_max, u32);
    max_number_test!(u64_max, u64);

    fn read<T: BinProtRead>(bytes: &[u8]) -> Option<T> {
        let mut r = bytes;
        let v = T::binprot_read(&mut r).ok()?;
        assert!(r.is_empty());
        Some(v)
    }

    /// `UInt32` is an OCaml `int32` (`Unsigned_extended.UInt32` is binable as
    /// `Int32`): bin_prot's `bin_read_int32` has no 8-byte form.
    #[test]
    fn uint32_reads_as_ocaml_int32() {
        assert_eq!(read::<super::UInt32>(&[0xfe, 5, 0]).map(|n| n.0), Some(5));
        assert_eq!(
            read::<super::UInt32>(&[0xfd, 5, 0, 0, 0]).map(|n| n.0),
            Some(5)
        );
        assert_eq!(read::<super::UInt32>(&[0xfc, 5, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(read::<super::UInt32>(&[0x85]), None);
    }

    /// `UInt64` and `Int64` are OCaml `int`s (63 bits): `0xfc` with a value
    /// past 2^62 is `Int_overflow`.
    #[test]
    fn uint64_reads_as_ocaml_int() {
        let max = (1i64 << 62) - 1;
        let mut ok = vec![0xfc];
        ok.extend_from_slice(&max.to_le_bytes());
        assert_eq!(read::<super::UInt64>(&ok).map(|n| n.0), Some(max as u64));
        let mut over = vec![0xfc];
        over.extend_from_slice(&(max + 1).to_le_bytes());
        assert_eq!(read::<super::UInt64>(&over), None);
        assert_eq!(read::<super::Int64>(&over), None);
        // But `read_ocaml_int64`, for the wire types that are OCaml `Int64`,
        // takes it, and every other 64-bit value.
        let mut r = &over[..];
        assert_eq!(super::read_ocaml_int64(&mut r).unwrap().0, (max + 1) as u64);
        let mut all = vec![0xfc];
        all.extend_from_slice(&u64::MAX.to_le_bytes());
        let mut r = &all[..];
        assert_eq!(super::read_ocaml_int64(&mut r).unwrap().0, u64::MAX);
    }
}
