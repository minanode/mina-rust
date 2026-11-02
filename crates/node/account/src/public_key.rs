use mina_p2p_messages::{
    b58::FromBase58CheckError,
    binprot::{self, macros::BinProtWrite, BinProtRead},
    v2::{NonZeroCurvePoint, NonZeroCurvePointUncompressedStableV1},
};
use mina_signer::{CompressedPubKey, PubKey};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(BinProtWrite, Serialize, Debug, Ord, PartialOrd, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccountPublicKey(NonZeroCurvePoint);

impl From<PubKey> for AccountPublicKey {
    fn from(value: PubKey) -> Self {
        value.into_compressed().into()
    }
}

impl From<CompressedPubKey> for AccountPublicKey {
    fn from(value: CompressedPubKey) -> Self {
        Self(
            NonZeroCurvePointUncompressedStableV1 {
                x: value.x.into(),
                is_odd: value.is_odd,
            }
            .into(),
        )
    }
}

impl TryFrom<AccountPublicKey> for CompressedPubKey {
    type Error = ();

    fn try_from(value: AccountPublicKey) -> Result<Self, Self::Error> {
        Ok(Self {
            is_odd: value.0.is_odd,
            x: value.0.into_inner().x.try_into().map_err(|_| ())?,
        })
    }
}

impl From<NonZeroCurvePoint> for AccountPublicKey {
    fn from(value: NonZeroCurvePoint) -> Self {
        Self(value)
    }
}

impl From<AccountPublicKey> for NonZeroCurvePoint {
    fn from(value: AccountPublicKey) -> Self {
        value.0
    }
}

impl FromStr for AccountPublicKey {
    type Err = FromBase58CheckError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self(s.parse()?)
            .validate()
            .map_err(FromBase58CheckError::Binprot)
    }
}

impl AsRef<NonZeroCurvePoint> for AccountPublicKey {
    fn as_ref(&self) -> &NonZeroCurvePoint {
        &self.0
    }
}

impl fmt::Display for AccountPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let p2p_key = NonZeroCurvePoint::from(self.clone());
        write!(f, "{p2p_key}")
    }
}

// for a simple hashmap or btree map use the hash of the string representation
impl std::hash::Hash for AccountPublicKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state);
    }
}

impl AccountPublicKey {
    /// Reject noncanonical field encodings at every decoding boundary.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/crypto/kimchi_pasta_snarky_backend/field.ml#L180-L189>
    fn validate(self) -> Result<Self, binprot::Error> {
        CompressedPubKey::try_from(self.clone()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "public key x is outside the field",
            )
        })?;
        Ok(self)
    }
}

impl BinProtRead for AccountPublicKey {
    fn binprot_read<R: std::io::Read + ?Sized>(reader: &mut R) -> Result<Self, binprot::Error> {
        Self(NonZeroCurvePoint::binprot_read(reader)?).validate()
    }
}

impl<'de> Deserialize<'de> for AccountPublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self(NonZeroCurvePoint::deserialize(deserializer)?)
            .validate()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_rejects_out_of_field_coordinates() {
        let mut x = [0xff; 32];
        x[31] = 0x7f;
        use mina_p2p_messages::bigint::BigInt;
        // The Pasta Fp modulus used by OCaml's backend, plus the malformed
        // 2^255 - 1 case: the former is below Fq and needs Fp validation.
        for x in [
            BigInt::from_bytes(x),
            BigInt::from_decimal(
                "28948022309329048855892746252171976963363056481941560715954676764349967630337",
            )
            .unwrap(),
        ] {
            let key = AccountPublicKey::from(NonZeroCurvePoint::from(
                NonZeroCurvePointUncompressedStableV1 { x, is_odd: false },
            ));
            use mina_p2p_messages::binprot::BinProtWrite;
            let mut bytes = Vec::new();
            key.binprot_write(&mut bytes).unwrap();
            assert!(AccountPublicKey::binprot_read(&mut bytes.as_slice()).is_err());
            let encoded = key.to_string();
            assert!(encoded.parse::<AccountPublicKey>().is_err());
            assert!(
                serde_json::from_value::<AccountPublicKey>(serde_json::json!(encoded)).is_err()
            );
        }
        let valid = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U";
        assert_eq!(
            valid.parse::<AccountPublicKey>().unwrap().to_string(),
            valid
        );
        assert_eq!(
            serde_json::from_value::<AccountPublicKey>(serde_json::json!(valid))
                .unwrap()
                .to_string(),
            valid
        );
    }
}
