#[cfg(feature = "async")]
pub mod async_read_write;
#[cfg(feature = "async")]
mod async_traits;

extern crate binprot_derive;
pub mod macros {
    pub use binprot_derive::*;
}

// Re-export byteorder as it can be used by the macros.
#[doc(hidden)]
pub use ::byteorder;

mod error;
mod int;
pub use crate::int::{read_int, read_int32, read_int64, read_nat0, OCAML_MAX_INT, OCAML_MIN_INT};
mod shape;
mod traits;

pub use crate::error::Error;
pub use crate::shape::{Digestible, Shape};
pub use crate::traits::{BinProtRead, BinProtShape, BinProtSize, BinProtWrite, ShapeContext};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::convert::TryFrom;
use std::hash::Hash;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::Arc;

/// This uses the "size-prefixed binary protocol".
/// https://ocaml.janestreet.com/ocaml-core/v0.13/doc/async_unix/Async_unix/Writer/index.html#val-write_bin_prot
pub fn binprot_write_with_size<W: Write, B: BinProtWrite>(b: &B, w: &mut W) -> std::io::Result<()> {
    let len = b.binprot_size();
    w.write_i64::<byteorder::LittleEndian>(len as i64)?;
    b.binprot_write(w)
}

/// This also uses the "size-prefixed binary protocol".
pub fn binprot_read_with_size<R: Read, B: BinProtRead>(r: &mut R) -> Result<B, Error> {
    // TODO: use the length value to avoid reading more that the specified number of bytes.
    let _len = r.read_i64::<byteorder::LittleEndian>()?;
    B::binprot_read(r)
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct Nat0(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmallBytes1k(pub Vec<u8>);

impl std::convert::From<String> for SmallBytes1k {
    fn from(str: String) -> Self {
        SmallBytes1k(str.into_bytes())
    }
}

impl std::convert::From<&str> for SmallBytes1k {
    fn from(str: &str) -> Self {
        SmallBytes1k(str.as_bytes().to_vec())
    }
}

impl std::convert::From<Vec<u8>> for SmallBytes1k {
    fn from(v: Vec<u8>) -> Self {
        SmallBytes1k(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SmallString1k(pub String);

impl std::convert::From<String> for SmallString1k {
    fn from(str: String) -> Self {
        SmallString1k(str.to_string())
    }
}

impl std::convert::From<&str> for SmallString1k {
    fn from(str: &str) -> Self {
        SmallString1k(str.to_string())
    }
}

impl BinProtWrite for Nat0 {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.0)
    }
}

impl BinProtWrite for u64 {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, *self)
    }
}

impl BinProtWrite for i64 {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_i64(w, *self)
    }
}

impl BinProtWrite for f64 {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&self.to_le_bytes())
    }
}

impl BinProtWrite for () {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&[0u8])
    }
}

impl BinProtWrite for bool {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let b = u8::from(*self);
        w.write_all(&[b])
    }
}

impl<T: BinProtWrite> BinProtWrite for Option<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        match self {
            None => w.write_all(&[0u8]),
            Some(v) => {
                w.write_all(&[1u8])?;
                v.binprot_write(w)
            }
        }
    }
}

impl<T: BinProtWrite, E: BinProtWrite> BinProtWrite for Result<T, E> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        match self {
            Ok(v) => {
                w.write_all(&[0u8])?;
                v.binprot_write(w)
            }
            Err(e) => {
                w.write_all(&[1u8])?;
                e.binprot_write(w)
            }
        }
    }
}

impl<T: BinProtWrite> BinProtWrite for Box<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.as_ref().binprot_write(w)
    }
}

impl<T: BinProtWrite> BinProtWrite for Rc<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.as_ref().binprot_write(w)
    }
}

impl<T: BinProtWrite> BinProtWrite for Arc<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.as_ref().binprot_write(w)
    }
}

impl<T: BinProtWrite> BinProtWrite for Vec<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.len() as u64)?;
        for v in self.iter() {
            v.binprot_write(w)?
        }
        Ok(())
    }
}

// Serialization using the same format as:
// type vec32 = (float, Bigarray.float32_elt, Bigarray.fortran_layout) Bigarray.Array1.t
// https://github.com/janestreet/bin_prot/blob/472b29dadede4d432a020be85bf34103aa26cd57/src/write.ml#L344
impl BinProtWrite for Vec<f32> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.len() as u64)?;
        for v in self.iter() {
            w.write_f32::<byteorder::NativeEndian>(*v)?
        }
        Ok(())
    }
}

impl<T: BinProtWrite> BinProtWrite for &[T] {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.len() as u64)?;
        for v in self.iter() {
            v.binprot_write(w)?
        }
        Ok(())
    }
}

impl BinProtWrite for SmallString1k {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let bytes = self.0.as_bytes();
        int::write_nat0(w, bytes.len() as u64)?;
        w.write_all(bytes)
    }
}

impl BinProtWrite for &str {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let bytes = self.as_bytes();
        int::write_nat0(w, bytes.len() as u64)?;
        w.write_all(bytes)
    }
}

impl BinProtWrite for SmallBytes1k {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let bytes = &self.0;
        int::write_nat0(w, bytes.len() as u64)?;
        w.write_all(bytes)
    }
}

impl<K: BinProtWrite, V: BinProtWrite> BinProtWrite for std::collections::BTreeMap<K, V> {
    // The order is unspecified by the protocol
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.len() as u64)?;
        for (k, v) in self.iter() {
            k.binprot_write(w)?;
            v.binprot_write(w)?;
        }
        Ok(())
    }
}

impl<K: BinProtWrite, V: BinProtWrite> BinProtWrite for std::collections::HashMap<K, V> {
    // The order is unspecified by the protocol
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        int::write_nat0(w, self.len() as u64)?;
        for (k, v) in self.iter() {
            k.binprot_write(w)?;
            v.binprot_write(w)?;
        }
        Ok(())
    }
}

macro_rules! tuple_impls {
    ( $( $name:ident )+ ) => {
        impl<$($name: BinProtWrite),+> BinProtWrite for ($($name,)+)
        {
            #[allow(non_snake_case)]
            fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
                let ($($name,)+) = self;
                $($name.binprot_write(w)?;)+
                Ok(())
            }
        }

        impl<$($name: BinProtRead),+> BinProtRead for ($($name,)+)
        {
            #[allow(non_snake_case)]
            fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
            where
                Self: Sized,
            {
                $(let $name = $name::binprot_read(r)?;)+
                Ok(($($name,)+))
            }
        }

        impl<$($name: BinProtShape),+> BinProtShape for ($($name,)+)
        {
            #[allow(non_snake_case)]
            fn binprot_shape_impl(ctxt: &mut ShapeContext) -> Shape
            {
                $(let $name = <$name>::binprot_shape_loop(ctxt);)+
                Shape::Tuple(vec![$($name,)+])
            }
        }
    };
}

tuple_impls! { A }
tuple_impls! { A B }
tuple_impls! { A B C }
tuple_impls! { A B C D }
tuple_impls! { A B C D E }
tuple_impls! { A B C D E F }
tuple_impls! { A B C D E F G }
tuple_impls! { A B C D E F G H }
tuple_impls! { A B C D E F G H I }

impl BinProtRead for Nat0 {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let u64 = int::read_nat0(r)?;
        Ok(Nat0(u64))
    }
}

impl BinProtRead for u64 {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let u64 = int::read_nat0(r)?;
        Ok(u64)
    }
}

impl BinProtRead for i64 {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        // OCaml `int` (63 bits): bin_read_int.
        let i64 = int::read_int(r)?;
        Ok(i64)
    }
}

impl BinProtRead for f64 {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let f64 = r.read_f64::<LittleEndian>()?;
        Ok(f64)
    }
}

impl BinProtRead for () {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let c = r.read_u8()?;
        if c == 0 {
            Ok(())
        } else {
            Err(Error::UnexpectedValueForUnit(c))
        }
    }
}

impl BinProtRead for bool {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let c = r.read_u8()?;
        if c == 0 {
            Ok(false)
        } else if c == 1 {
            Ok(true)
        } else {
            Err(Error::UnexpectedValueForBool(c))
        }
    }
}

impl<T: BinProtRead> BinProtRead for Option<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let c = r.read_u8()?;
        if c == 0 {
            Ok(None)
        } else if c == 1 {
            let v = T::binprot_read(r)?;
            Ok(Some(v))
        } else {
            Err(Error::UnexpectedValueForOption(c))
        }
    }
}

impl<T: BinProtRead, E: BinProtRead> BinProtRead for Result<T, E> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let c = r.read_u8()?;
        if c == 0 {
            let v = T::binprot_read(r)?;
            Ok(Ok(v))
        } else if c == 1 {
            let e = E::binprot_read(r)?;
            Ok(Err(e))
        } else {
            Err(Error::UnexpectedVariantIndex { index: c, ident: "Result" })
        }
    }
}

impl<T: BinProtRead> BinProtRead for Box<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let v = T::binprot_read(r)?;
        Ok(Box::new(v))
    }
}

impl<T: BinProtRead> BinProtRead for Rc<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let v = T::binprot_read(r)?;
        Ok(Rc::new(v))
    }
}

impl<T: BinProtRead> BinProtRead for Arc<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let v = T::binprot_read(r)?;
        Ok(Arc::new(v))
    }
}

impl<T: BinProtRead> BinProtRead for Vec<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        let mut v: Vec<T> = Vec::with_capacity(len as usize);
        for _i in 0..len {
            let item = T::binprot_read(r)?;
            v.push(item)
        }
        Ok(v)
    }
}

// Serialization using the same format as:
// type vec32 = (float, Bigarray.float32_elt, Bigarray.fortran_layout) Bigarray.Array1.t
// https://github.com/janestreet/bin_prot/blob/472b29dadede4d432a020be85bf34103aa26cd57/src/write.ml#L344
impl BinProtRead for Vec<f32> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        let mut v: Vec<f32> = Vec::with_capacity(len as usize);
        for _i in 0..len {
            let item = r.read_f32::<byteorder::NativeEndian>()?;
            v.push(item)
        }
        Ok(v)
    }
}

impl<K: BinProtRead + Ord, V: BinProtRead> BinProtRead for std::collections::BTreeMap<K, V> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        let mut res = std::collections::BTreeMap::new();
        for _i in 0..len {
            let k = K::binprot_read(r)?;
            let v = V::binprot_read(r)?;
            if res.insert(k, v).is_some() {
                return Err(Error::SameKeyAppearsTwiceInMap);
            }
        }
        Ok(res)
    }
}

impl<K: BinProtRead + Hash + Eq, V: BinProtRead> BinProtRead for std::collections::HashMap<K, V> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        let mut res = std::collections::HashMap::new();
        for _i in 0..len {
            let k = K::binprot_read(r)?;
            let v = V::binprot_read(r)?;
            if res.insert(k, v).is_some() {
                return Err(Error::SameKeyAppearsTwiceInMap);
            }
        }
        Ok(res)
    }
}

// NOTE: Removed because of potential for unbounded allocations
impl BinProtRead for SmallString1k {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        if len > 1_000 {
            return Err(Error::TooMuchData(len));
        }
        let mut buf: Vec<u8> = vec![0u8; len as usize];
        r.read_exact(&mut buf)?;
        let str = std::str::from_utf8(&buf)?;
        Ok(SmallString1k(str.to_string()))
    }
}

// NOTE: Removed because of potential for unbounded allocations
impl BinProtRead for SmallBytes1k {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = int::read_nat0(r)?;
        if len > 1_000 {
            return Err(Error::TooMuchData(len));
        }
        let mut buf: Vec<u8> = vec![0u8; len as usize];
        r.read_exact(&mut buf)?;
        Ok(SmallBytes1k(buf))
    }
}

/// A value serialized by first having its size as a nat0, then the
/// encoding of the value itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithLen<T>(pub T);

impl<T: BinProtWrite + BinProtSize> BinProtWrite for WithLen<T> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let len = self.0.binprot_size();
        int::write_nat0(w, len as u64)?;
        self.0.binprot_write(w)
    }
}

impl<T: BinProtRead> BinProtRead for WithLen<T> {
    fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        // TODO: stop reading past this length
        let _len = int::read_nat0(r)?;
        let t = T::binprot_read(r)?;
        Ok(WithLen(t))
    }
}

/// A buffer serialized as its size first as a nat0, then the payload itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferWithLen(pub Vec<u8>);

impl BinProtRead for BufferWithLen {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let len = Nat0::binprot_read(r)?;
        let mut buf: Vec<u8> = vec![0u8; len.0 as usize];
        r.read_exact(&mut buf)?;
        Ok(BufferWithLen(buf))
    }
}

impl BinProtWrite for BufferWithLen {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> Result<(), std::io::Error> {
        let nat0 = Nat0(self.0.len() as u64);
        nat0.binprot_write(w)?;
        w.write_all(&self.0)?;
        Ok(())
    }
}

// Maybe this could be done with some clever use of traits rather
// than a macro but in doing so, I ended up with some potential
// conflicts: "downstream crates may implement trait".
macro_rules! int_impls {
    ( $ty: ty) => {
        impl BinProtWrite for $ty {
            fn binprot_write<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
                int::write_i64(w, (*self).into())
            }
        }

        impl BinProtRead for $ty {
            fn binprot_read<R: Read + ?Sized>(r: &mut R) -> Result<Self, Error>
            where
                Self: Sized,
            {
                // OCaml `int`, narrowed: bin_read_int, then a range check.
                // A type that is an OCaml `int32` must use `read_int32`.
                let i64 = int::read_int(r)?;
                Ok(<$ty>::try_from(i64)?)
            }
        }
    };
}

int_impls!(i32);
int_impls!(u32);
int_impls!(i16);
int_impls!(u16);
int_impls!(i8);
int_impls!(u8);
