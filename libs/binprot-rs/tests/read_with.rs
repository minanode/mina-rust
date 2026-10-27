use binprot::{BinProtRead, BinProtWrite};
use binprot_derive::{BinProtRead, BinProtWrite};

fn positive<R: std::io::Read + ?Sized>(r: &mut R) -> Result<i64, binprot::Error> {
    let value = i64::binprot_read(r)?;
    if value <= 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "positive only").into());
    }
    Ok(value)
}

#[derive(Debug, PartialEq, BinProtRead, BinProtWrite)]
struct Named {
    #[binprot_read_with("positive")]
    value: i64,
    other: i64,
}

#[derive(Debug, PartialEq, BinProtRead, BinProtWrite)]
struct Tuple(#[binprot_read_with("positive")] i64, i64);

#[derive(Debug, PartialEq, BinProtRead, BinProtWrite)]
enum Variant {
    Named {
        #[binprot_read_with("positive")]
        value: i64,
        other: i64,
    },
    Tuple(#[binprot_read_with("positive")] i64, i64),
}

fn check<T: BinProtRead + BinProtWrite + PartialEq + std::fmt::Debug>(good: T, bad: T) {
    let mut bytes = Vec::new();
    good.binprot_write(&mut bytes).unwrap();
    assert_eq!(T::binprot_read(&mut bytes.as_slice()).unwrap(), good);
    bytes.clear();
    bad.binprot_write(&mut bytes).unwrap();
    assert!(T::binprot_read(&mut bytes.as_slice()).is_err());
}

/// Compile and run without relying on a downstream crate to enable syn/full.
/// The selected reader must reject its own field and leave other fields alone.
#[test]
fn custom_readers_on_struct_and_enum_fields() {
    check(Named { value: 1, other: -1 }, Named { value: 0, other: -1 });
    check(Tuple(1, -1), Tuple(0, -1));
    check(Variant::Named { value: 1, other: -1 }, Variant::Named { value: 0, other: -1 });
    check(Variant::Tuple(1, -1), Variant::Tuple(0, -1));
}
