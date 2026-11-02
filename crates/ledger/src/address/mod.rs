pub mod raw;

const fn compute_nbytes(nbits: usize) -> usize {
    if nbits.is_multiple_of(8) {
        nbits / 8
    } else {
        (nbits / 8) + 1
    }
}

/// Devnet uses trees of depth 35, which requires addresses of 35 bits
const NBITS: usize = 35;
const NBYTES: usize = compute_nbytes(NBITS);

pub use raw::Direction;
pub type Address = raw::Address<NBYTES>;
pub type AddressIterator = raw::AddressIterator<NBYTES>;

impl Address {
    /// The longest address, so the deepest tree, this type is used for.
    pub const MAX_LENGTH: usize = NBITS;
}

#[cfg(test)]
mod tests {
    use crate::AccountIndex;

    use super::*;

    #[cfg(target_family = "wasm")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_address_iter() {
        use Direction::*;

        let addr = Address::try_from("10101010").unwrap();
        assert_eq!(
            addr.iter().collect::<Vec<_>>(),
            &[Right, Left, Right, Left, Right, Left, Right, Left]
        );

        let addr = Address::try_from("01010101").unwrap();
        assert_eq!(
            addr.iter().collect::<Vec<_>>(),
            &[Left, Right, Left, Right, Left, Right, Left, Right]
        );

        let addr = Address::try_from("010101010").unwrap();
        assert_eq!(
            addr.iter().collect::<Vec<_>>(),
            &[Left, Right, Left, Right, Left, Right, Left, Right, Left]
        );

        let addr = Address::try_from("0101010101").unwrap();
        assert_eq!(
            addr.iter().collect::<Vec<_>>(),
            &[Left, Right, Left, Right, Left, Right, Left, Right, Left, Right]
        );

        let addr = Address::try_from("").unwrap();
        assert!(addr.iter().next().is_none());

        assert!(Address::try_from("0101010101a").is_err());
        assert!(Address::try_from("0".repeat((NBYTES * 8) - 1).as_str()).is_ok());
        assert!(Address::try_from("0".repeat(NBYTES * 8).as_str()).is_err());
    }

    #[test]
    fn test_address_next() {
        let two: usize = 2;

        // prev
        for length in 5..23 {
            let mut addr = Address::last(length);

            elog!("length={length} until={:?}", two.pow(length as u32));
            for _ in 0..two.pow(length as u32) - 1 {
                let prev = addr.prev().unwrap();
                assert_eq!(prev.next().unwrap(), addr);
                addr = prev;
            }

            assert!(addr.prev().is_none());
        }

        // next
        for length in 5..23 {
            let mut addr = Address::first(length);

            elog!("length={length} until={:?}", two.pow(length as u32));
            for _ in 0..two.pow(length as u32) - 1 {
                let next = addr.next().unwrap();
                assert_eq!(next.prev().unwrap(), addr);
                addr = next;
            }

            assert!(addr.next().is_none());
        }
    }

    #[test]
    fn test_address_clear() {
        let mut inner: [u8; NBYTES] = Default::default();
        inner[0] = 0b11111111;
        inner[1] = 0b11111111;

        let mut addr = Address { inner, length: 12 };
        elog!("ADDR={:?}", addr);
        addr.clear_after(6);
        elog!("ADDR={:?}", addr);
    }

    #[test]
    fn test_address_can_reach() {
        let addr = Address::try_from("00").unwrap();
        assert!(addr.is_before(&Address::try_from("00").unwrap()));
        assert!(addr.is_before(&Address::try_from("01").unwrap()));
        assert!(addr.is_before(&Address::try_from("000").unwrap()));
        assert!(addr.is_before(&Address::try_from("001").unwrap()));
        assert!(addr.is_before(&Address::try_from("010").unwrap()));
        assert!(addr.is_before(&Address::try_from("100").unwrap()));

        let addr = Address::try_from("101").unwrap();
        assert!(addr.is_before(&Address::try_from("10100").unwrap()));
        assert!(addr.is_before(&Address::try_from("10111").unwrap()));
        assert!(!addr.is_before(&Address::try_from("10011").unwrap()));
    }

    #[test]
    fn test_address_show() {
        let addr = Address::first(2);
        elog!("LA {:?}", addr);
        let sec = addr.next().unwrap();
        elog!("LA {:?}", sec);
        elog!("LA {:?}", sec.child_left());
    }

    #[test]
    fn test_address_index() {
        for length in 1..20 {
            let mut addr = Address::first(length);

            for index in 0..2u64.pow(length as u32) - 1 {
                let to_index = addr.to_index();

                assert_eq!(to_index, AccountIndex(index));
                assert_eq!(addr, Address::from_index(to_index, length));

                addr = addr.next().unwrap();
            }
        }
    }

    #[test]
    fn test_address_linear() {
        for (index, s) in [
            "", "0", "1", "00", "01", "10", "11", "000", "001", "010", "011", "100", "101", "110",
            "111",
        ]
        .iter()
        .enumerate()
        {
            let addr = Address::try_from(*s).unwrap();
            assert_eq!(index as u64, addr.to_linear_index());
        }
    }

    #[test]
    fn test_address_children() {
        let root = Address::try_from("00").unwrap();
        let iter_children = root.iter_children(4);
        assert_eq!(iter_children.len(), 4);
        assert_eq!(
            iter_children.collect::<Vec<_>>(),
            &[
                Address::try_from("0000").unwrap(),
                Address::try_from("0001").unwrap(),
                Address::try_from("0010").unwrap(),
                Address::try_from("0011").unwrap(),
            ]
        );

        let root = Address::try_from("01").unwrap();
        let iter_children = root.iter_children(4);
        assert_eq!(iter_children.len(), 4);
        assert_eq!(
            iter_children.collect::<Vec<_>>(),
            &[
                Address::try_from("0100").unwrap(),
                Address::try_from("0101").unwrap(),
                Address::try_from("0110").unwrap(),
                Address::try_from("0111").unwrap(),
            ]
        );

        let root = Address::try_from("10").unwrap();
        let iter_children = root.iter_children(4);
        assert_eq!(iter_children.len(), 4);
        assert_eq!(
            iter_children.collect::<Vec<_>>(),
            &[
                Address::try_from("1000").unwrap(),
                Address::try_from("1001").unwrap(),
                Address::try_from("1010").unwrap(),
                Address::try_from("1011").unwrap(),
            ]
        );

        let root = Address::try_from("11").unwrap();
        let iter_children = root.iter_children(4);
        assert_eq!(iter_children.len(), 4);
        assert_eq!(
            iter_children.collect::<Vec<_>>(),
            &[
                Address::try_from("1100").unwrap(),
                Address::try_from("1101").unwrap(),
                Address::try_from("1110").unwrap(),
                Address::try_from("1111").unwrap(),
            ]
        );

        let root = Address::try_from("00").unwrap();
        let iter_children = root.iter_children(6);
        assert_eq!(iter_children.len(), 16);
    }

    #[test]
    fn test_address_children_parent_root_eq() {
        let left = Address::first(1);
        let right = left.next().unwrap();
        assert_eq!(left.parent().unwrap(), Address::root());
        assert_eq!(right.parent().unwrap(), Address::root());
        assert_eq!(left.parent(), right.parent());
    }

    /// Indexes from 2^32 up, at the ledger depth of 35, through every
    /// conversion: no `u32` truncation. The expected addresses follow OCaml:
    /// `of_int_exn` puts the index's most significant bit first, `to_int`
    /// reads it back, and `serialize` pads the bits to whole bytes.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/merkle_address/merkle_address.ml#L148-L166>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/merkle_address/merkle_address.ml#L220-L227>
    #[test]
    fn indexes_past_32_bits_round_trip_at_depth_35() {
        use mina_p2p_messages::v2::MerkleAddressBinableArgStableV1;

        const DEPTH: usize = 35;
        assert_eq!(Address::MAX_LENGTH, DEPTH);
        let cases: [(u64, [u8; 5]); 5] = [
            ((1 << 32) - 1, [0x1f, 0xff, 0xff, 0xff, 0xe0]),
            (1 << 32, [0x20, 0x00, 0x00, 0x00, 0x00]),
            ((1 << 32) + 1, [0x20, 0x00, 0x00, 0x00, 0x20]),
            ((1 << 34) | 0b101, [0x80, 0x00, 0x00, 0x00, 0xa0]),
            ((1 << 35) - 1, [0xff, 0xff, 0xff, 0xff, 0xe0]),
        ];
        for (index, bytes) in cases {
            let addr = Address::from_index(AccountIndex(index), DEPTH);
            assert_eq!(addr.length(), DEPTH);
            assert_eq!(addr.to_index(), AccountIndex(index), "{index}");
            assert_eq!(addr.used_bytes(), &bytes, "{index}");
            assert_eq!(addr.to_linear_index(), (1u64 << DEPTH) - 1 + index);

            // Directions, from the root: the index's bits, highest first.
            let directions = addr.iter().collect::<Vec<_>>();
            assert_eq!(directions.len(), DEPTH);
            for (position, direction) in directions.iter().enumerate() {
                let bit = index >> (DEPTH - 1 - position) & 1;
                let want = if bit == 1 {
                    Direction::Right
                } else {
                    Direction::Left
                };
                assert_eq!(*direction, want, "{index} at {position}");
            }

            // Down from the root along those directions, and back up.
            let walked =
                directions
                    .iter()
                    .fold(Address::root(), |addr, direction| match direction {
                        Direction::Left => addr.child_left(),
                        Direction::Right => addr.child_right(),
                    });
            assert_eq!(walked, addr);
            let mut up = addr.clone();
            for _ in 0..DEPTH {
                up = up.parent().unwrap();
            }
            assert!(up.is_root());

            // The wire form: the length and the padded bits.
            let wire = MerkleAddressBinableArgStableV1::from(addr.clone());
            assert_eq!(wire.0.as_u64(), DEPTH as u64);
            assert_eq!(wire.1.as_ref(), &bytes);
            assert_eq!(Address::from(&wire), addr);
        }

        // Neighbours across the 32-bit boundary.
        let below = Address::from_index(AccountIndex((1 << 32) - 1), DEPTH);
        let above = Address::from_index(AccountIndex(1 << 32), DEPTH);
        assert_eq!(below.next().unwrap(), above);
        assert_eq!(above.prev().unwrap(), below);
        let last = Address::from_index(AccountIndex((1 << 35) - 1), DEPTH);
        assert_eq!(last, Address::last(DEPTH));
        assert!(last.next().is_none());
        assert_eq!(Address::first(DEPTH).to_index(), AccountIndex(0));
        assert!(Address::first(DEPTH).prev().is_none());
    }
}
