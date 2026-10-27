use blake2::{digest::consts::U32, Blake2b, Digest};
use mina_hasher::{Hashable, ROInput};
use mina_signer::{Keypair, NetworkId, NonceMode, PubKey, SecKey, Signature, Signer};
use serde_json::Value;
use std::{fs, path::Path};

// OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L25-L36
// The digest is raw bytes, each byte LSB first. It is not String_sign hex.
#[derive(Clone)]
struct UptimeDigest(Vec<u8>);

impl Hashable for UptimeDigest {
    type D = NetworkId;

    fn to_roinput(&self) -> ROInput {
        ROInput::new().append_bytes(&self.0)
    }

    fn domain_string(network: Self::D) -> Option<String> {
        Some(
            match network {
                NetworkId::MAINNET => "MinaSignatureMainnet",
                NetworkId::TESTNET => "CodaSignature",
            }
            .to_owned(),
        )
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn main() {
    let root = std::env::args().nth(1).expect("pass fixtures directory");
    let mut count = 0;
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() || !path.join("THROWAWAY-TEST-KEY.json").exists() {
            continue;
        }
        let request = read_json(&path.join("uptime-1.json"));
        let manifest = read_json(&path.join("manifest.json"));
        let key = read_json(&path.join("THROWAWAY-TEST-KEY.json"));
        let data = fs::read(path.join("uptime-data.json")).unwrap();
        let digest = UptimeDigest(Blake2b::<U32>::digest(&data).to_vec());
        let network = if manifest["network"] == "mainnet" {
            NetworkId::MAINNET
        } else {
            NetworkId::TESTNET
        };
        let other_network = if network == NetworkId::MAINNET {
            NetworkId::TESTNET
        } else {
            NetworkId::MAINNET
        };
        let pk = PubKey::from_address(request["submitter"].as_str().unwrap()).unwrap();
        let kp = Keypair::try_from(
            SecKey::from_base58(key["private_key_base58"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(kp.public, pk, "throwaway key must match OCaml submitter");
        let expected = Signature::new(
            manifest["signature_field_decimal"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            manifest["signature_scalar_decimal"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        );
        let mut signer = mina_signer::create_legacy::<UptimeDigest>(network);
        assert!(signer.verify(&expected, &pk, &digest));
        assert_eq!(signer.sign(&kp, &digest, NonceMode::Legacy), expected);
        assert!(!mina_signer::create_legacy::<UptimeDigest>(other_network)
            .verify(&expected, &pk, &digest));
        let mut changed = digest.clone();
        changed.0[0] ^= 1;
        assert!(!signer.verify(&expected, &pk, &changed));
        let hex_digest = digest
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(!signer.verify(&expected, &pk, &UptimeDigest(hex_digest.into_bytes())));
        println!(
            "{}: verify, deterministic sign, wrong network, tamper, hex rejection passed",
            path.display()
        );
        count += 1;
    }
    assert!(count > 0);
}
