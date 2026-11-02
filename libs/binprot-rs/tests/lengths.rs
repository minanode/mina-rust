//! A length prefix is input like any other: one the bytes don't back is an
//! error, and reading it allocates for the bytes present, not for the
//! length.

use binprot::{BinProtRead, BinProtWrite, BufferWithLen, Nat0, MAX_PREALLOCATED_BYTES};

fn claimed(len: u64, then: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    Nat0(len).binprot_write(&mut bytes).unwrap();
    bytes.extend_from_slice(then);
    bytes
}

#[test]
fn lengths_beyond_the_input_are_errors() {
    for len in [(1 << 62) - 1, u32::MAX as u64, 1 << 20] {
        let bytes = claimed(len, &[1, 2, 3]);
        assert!(Vec::<u8>::binprot_read(&mut &bytes[..]).is_err(), "{len}");
        assert!(Vec::<u64>::binprot_read(&mut &bytes[..]).is_err(), "{len}");
        assert!(Vec::<(u64, u64, u64, u64)>::binprot_read(&mut &bytes[..]).is_err(), "{len}");
        assert!(BufferWithLen::binprot_read(&mut &bytes[..]).is_err(), "{len}");
    }
}

/// Values longer than the preallocation read back whole.
#[test]
fn long_values_read_back() {
    let len = 3 * MAX_PREALLOCATED_BYTES + 1;
    let values: Vec<u64> = (0..len as u64).collect();
    let mut bytes = Vec::new();
    values.binprot_write(&mut bytes).unwrap();
    assert_eq!(Vec::<u64>::binprot_read(&mut &bytes[..]).unwrap(), values);

    let buffer = BufferWithLen((0..len).map(|i| i as u8).collect());
    let mut bytes = Vec::new();
    buffer.binprot_write(&mut bytes).unwrap();
    assert_eq!(BufferWithLen::binprot_read(&mut &bytes[..]).unwrap(), buffer);
}
