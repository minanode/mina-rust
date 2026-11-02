//! OCaml-compatible uptime submissions: payload, digest and signature.
//!
//! The request is `{"version":1,"data":{...},"signature":"…","submitter":"B62…"}`.
//! The signature is legacy Schnorr over the BLAKE2b-256 digest of the
//! compact `data` JSON, the 32 raw digest bytes taken least significant bit
//! first, in the profile's signature domain. The node's `/make_heartbeat`
//! envelope, which signs a hex digest in the testnet domain, is a different
//! protocol and is not accepted here.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L5-L49>

use base64::{engine::general_purpose::STANDARD, Engine as _};
use blake2::{digest::consts::U32, Blake2b, Digest};
use mina_node::account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2,
};
use mina_signer::{Keypair, NetworkId, PubKey, Signature, Signer};

use super::json::Json;

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L5>
pub const UPTIME_REQUEST_VERSION: i64 = 1;

/// OCaml's `Payload.block_data`, whose fields are signed in this order.
/// Absent options are omitted (`[@default None]`), not `null`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L7-L15>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockData {
    /// Standard Base64 of the whole block, proofs included, in Bin_prot.
    pub block: String,
    /// UTC RFC 3339 time, whole seconds.
    pub created_at: String,
    /// The node's libp2p peer ID.
    pub peer_id: String,
    /// Standard Base64 of an unversioned `Proof_data`
    /// ([`super::uptime_work::ProofData`]), when work is attached.
    pub snark_work: Option<String>,
    pub graphql_control_port: Option<i64>,
    /// The first 8 characters of the build commit, when opted in.
    pub built_with_commit_sha: Option<String>,
}

impl BlockData {
    pub fn to_json(&self) -> Json {
        let mut fields = vec![
            ("block", Json::str(&self.block)),
            ("created_at", Json::str(&self.created_at)),
            ("peer_id", Json::str(&self.peer_id)),
        ];
        if let Some(work) = &self.snark_work {
            fields.push(("snark_work", Json::str(work)));
        }
        if let Some(port) = self.graphql_control_port {
            fields.push(("graphql_control_port", Json::Int(port)));
        }
        if let Some(sha) = &self.built_with_commit_sha {
            fields.push(("built_with_commit_sha", Json::str(sha)));
        }
        Json::assoc(fields)
    }

    /// The exact signed bytes: `Yojson.Safe.to_string` of the data.
    pub fn signed_string(&self) -> String {
        self.to_json().to_compact_string()
    }
}

/// The whole block, proofs included, as OCaml's
/// `Binable.to_string (module Mina_block.Stable.Latest)` writes it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L153-L162>
pub fn encode_block(block: &v2::MinaBlockBlockStableV2) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    block.binprot_write(&mut bytes)?;
    Ok(bytes)
}

pub fn encode_block_base64(block: &v2::MinaBlockBlockStableV2) -> std::io::Result<String> {
    Ok(STANDARD.encode(encode_block(block)?))
}

/// Decodes a submitted block, refusing trailing bytes, so a header alone
/// (the heartbeat's `base64_encoded_header`) is not taken for a block.
pub fn decode_block(bytes: &[u8]) -> Result<v2::MinaBlockBlockStableV2, String> {
    let mut rest = bytes;
    let block = v2::MinaBlockBlockStableV2::binprot_read(&mut rest).map_err(|e| e.to_string())?;
    if !rest.is_empty() {
        return Err(format!("{} bytes after the block", rest.len()));
    }
    Ok(block)
}

/// The first 8 characters of a commit ID, as OCaml's `shorten_commit_id`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_lib/mina_lib.ml#L1362-L1368>
pub fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
}

/// BLAKE2b-256 of the compact data JSON, signed as raw bytes, each byte's
/// bits least significant first (`Blake2.string_to_bits`). Not the hex
/// digits, which `String_sign` (and the heartbeat) would sign.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/payload.ml#L25-L36>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/blake2/blake2.ml#L116-L122>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UptimeDigest(pub [u8; 32]);

impl UptimeDigest {
    pub fn of_data(signed_data: &[u8]) -> Self {
        Self(Blake2b::<U32>::digest(signed_data).into())
    }
}

impl mina_hasher::Hashable for UptimeDigest {
    type D = NetworkId;

    fn to_roinput(&self) -> mina_hasher::ROInput {
        // `append_bytes` takes each byte least significant bit first.
        mina_hasher::ROInput::new().append_bytes(&self.0)
    }

    /// `Mina_signature_kind.t_DEPRECATED`: the profile's legacy domain.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/hash_prefixes/hash_prefixes.ml#L1-L67>
    fn domain_string(network_id: Self::D) -> Option<String> {
        Some(
            match network_id {
                NetworkId::MAINNET => mina_core::network::mainnet::SIGNATURE_PREFIX,
                NetworkId::TESTNET => mina_core::network::devnet::SIGNATURE_PREFIX,
            }
            .to_owned(),
        )
    }
}

/// The signature domain of the network the node runs: mainnet signs in the
/// mainnet domain, devnet (and other testnets) in the testnet one.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/signature_kind/mina_signature_kind.ml#L29-L36>
pub fn profile_network_id() -> NetworkId {
    match mina_core::NetworkConfig::global().network_id {
        mina_core::network::NetworkId::MAINNET => NetworkId::MAINNET,
        mina_core::network::NetworkId::TESTNET => NetworkId::TESTNET,
    }
}

/// `Schnorr.Legacy.sign` with the deterministic legacy nonce, as OCaml.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/signature_lib/schnorr.ml#L231-L254>
pub fn sign_data(signed_data: &[u8], key: &AccountSecretKey, network: NetworkId) -> Signature {
    let digest = UptimeDigest::of_data(signed_data);
    let keypair = Keypair::from(key.clone());
    mina_signer::create_legacy::<UptimeDigest>(network).sign(
        &keypair,
        &digest,
        mina_signer::NonceMode::Legacy,
    )
}

pub fn verify_data(
    signed_data: &[u8],
    signature: &Signature,
    submitter: &PubKey,
    network: NetworkId,
) -> bool {
    let digest = UptimeDigest::of_data(signed_data);
    mina_signer::create_legacy::<UptimeDigest>(network).verify(signature, submitter, &digest)
}

/// A request ready to send: serialized and signed once, its bytes reused
/// unchanged by every retry.
#[derive(Clone, Debug)]
pub struct UptimeRequest {
    pub data: BlockData,
    pub signature: Signature,
    pub submitter: AccountPublicKey,
    /// The exact HTTP body.
    pub body: String,
}

impl UptimeRequest {
    /// OCaml's `Payload.create_request` then `request_to_yojson`.
    pub fn create(data: BlockData, key: &AccountSecretKey, network: NetworkId) -> Self {
        let signature = sign_data(data.signed_string().as_bytes(), key, network);
        Self::with_signature(data, signature, key.public_key())
    }

    /// The request with `signature` by `submitter`, written as [`Self::create`]
    /// writes it: to compare its encoding with a captured request's.
    pub fn with_signature(
        data: BlockData,
        signature: Signature,
        submitter: AccountPublicKey,
    ) -> Self {
        let body = request_json(&data.signed_string(), &signature, &submitter);
        Self {
            data,
            signature,
            submitter,
            body,
        }
    }
}

/// `{"version":1,"data":…,"signature":…,"submitter":…}`, the signature in
/// Mina's versioned Base58Check (`[0x9a,0x01] || rx || s`).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/signature.ml#L17-L72>
fn request_json(signed_data: &str, signature: &Signature, submitter: &AccountPublicKey) -> String {
    let signature = v2::Signature::from(v2::MinaBaseSignatureStableV1::from(signature));
    let mut out = String::with_capacity(signed_data.len() + 256);
    out.push_str("{\"version\":");
    out.push_str(&UPTIME_REQUEST_VERSION.to_string());
    out.push_str(",\"data\":");
    out.push_str(signed_data);
    out.push_str(",\"signature\":");
    super::json::write_string(&mut out, &signature.to_string());
    out.push_str(",\"submitter\":");
    super::json::write_string(&mut out, &submitter.to_string());
    out.push('}');
    out
}

/// A dry run's output: `body`, the exact bytes that would be POSTed, as
/// `dir/uptime-<created_at>-<kind>-<state hash>.json`, with `created_at`'s
/// `-` and `:` dropped. Written to a temporary file and renamed, so a reader
/// never sees part of one; a second request with the same name replaces it.
pub fn write_dry_run(
    dir: &std::path::Path,
    created_at: &str,
    kind: &str,
    state_hash: &str,
    body: &str,
) -> std::io::Result<std::path::PathBuf> {
    let time: String = created_at
        .chars()
        .filter(|c| !matches!(c, '-' | ':'))
        .collect();
    let path = dir.join(format!("uptime-{time}-{kind}-{state_hash}.json"));
    let partial = dir.join(format!(".uptime-{time}-{kind}-{state_hash}.json.partial"));
    std::fs::write(&partial, body.as_bytes())?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UptimeVerifyError {
    #[error("not JSON: {0}")]
    NotJson(String),
    #[error("not an uptime request: {0}")]
    Shape(&'static str),
    #[error("bad submitter public key")]
    Submitter,
    #[error("bad signature encoding")]
    SignatureEncoding,
    #[error("bad block: {0}")]
    Block(String),
    #[error("the signature does not verify")]
    Signature,
}

/// Checks a request as the receiver does: rebuilds the signed data from the
/// fields in OCaml's order and verifies the signature in `network`'s
/// domain, then decodes the whole block. A heartbeat envelope, a hex-digest
/// signature, another domain or a changed field fail.
///
/// The public receiving service rebuilds the data the same way
/// (o1-labs/uptime-service-backend `MakeSignPayload`); it doesn't decode the
/// block, which the offline `delegation_verify` does.
pub fn verify_request(
    body: &[u8],
    network: NetworkId,
) -> Result<(BlockData, v2::MinaBlockBlockStableV2), UptimeVerifyError> {
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| UptimeVerifyError::NotJson(e.to_string()))?;
    let obj = json
        .as_object()
        .ok_or(UptimeVerifyError::Shape("not an object"))?;
    if obj.get("version").and_then(|v| v.as_i64()) != Some(UPTIME_REQUEST_VERSION) {
        return Err(UptimeVerifyError::Shape("version"));
    }
    let data = obj
        .get("data")
        .and_then(|v| v.as_object())
        .ok_or(UptimeVerifyError::Shape("no data object"))?;
    let string = |key: &'static str| {
        data.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or(UptimeVerifyError::Shape(key))
    };
    let optional = |key: &'static str| match data.get(key) {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(UptimeVerifyError::Shape(key)),
    };
    let block_data = BlockData {
        block: string("block")?,
        created_at: string("created_at")?,
        peer_id: string("peer_id")?,
        snark_work: optional("snark_work")?,
        graphql_control_port: match data.get("graphql_control_port") {
            None => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or(UptimeVerifyError::Shape("graphql_control_port"))?,
            ),
        },
        built_with_commit_sha: optional("built_with_commit_sha")?,
    };
    let submitter = obj
        .get("submitter")
        .and_then(|v| v.as_str())
        .ok_or(UptimeVerifyError::Shape("submitter"))?;
    let submitter = PubKey::from_address(submitter).map_err(|_| UptimeVerifyError::Submitter)?;
    let signature = obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or(UptimeVerifyError::Shape("signature"))?;
    let signature: v2::Signature = signature
        .parse()
        .map_err(|_| UptimeVerifyError::SignatureEncoding)?;
    let signature =
        Signature::try_from(&*signature).map_err(|_| UptimeVerifyError::SignatureEncoding)?;
    if !verify_data(
        block_data.signed_string().as_bytes(),
        &signature,
        &submitter,
        network,
    ) {
        return Err(UptimeVerifyError::Signature);
    }
    let block_bytes = STANDARD
        .decode(&block_data.block)
        .map_err(|e| UptimeVerifyError::Block(e.to_string()))?;
    let block = decode_block(&block_bytes).map_err(UptimeVerifyError::Block)?;
    Ok((block_data, block))
}
