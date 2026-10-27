//! Precomputed blocks: the JSON OCaml nodes write for every block they add to
//! their frontier, which fills the public `mina_network_block_data` bucket and
//! which the archive's `mina-archive-blocks` tool reads.
//!
//! The format differs from the JSON our types write elsewhere:
//!
//! - balance changes and timing amounts are decimal MINA; preconditions
//!   remain nanomina
//!   (`"1.5"`), while the same types elsewhere, payments for example, are
//!   nanomina;
//! - byte strings, like the sok digests of completed work, are written as the
//!   bytes themselves, which need not be UTF-8, so a file need not be valid
//!   JSON;
//! - field elements are written in uppercase hex, receipt chain hashes in
//!   base58check, and verification keys as base58check with their hash;
//! - accounts' timings are written as OCaml variants (`["Untimed"]`);
//! - the protocol version's numbers are JSON numbers;
//! - the hashes of the delta transition chain proof are written as ledger
//!   hashes;
//! - `proposed_protocol_version` is left out when there is none.
//!
//! [`PrecomputedBlockData::from_ocaml_json`] reads that form and
//! [`PrecomputedBlockData::to_ocaml_json`] writes it, byte for byte as OCaml
//! does.

use std::io;

use binprot::BinProtWrite;
use binprot_derive::{BinProtRead, BinProtWrite};
use serde::{Deserialize, Serialize};
use serde_json::{
    ser::{CharEscape, Formatter},
    Value,
};

use crate::list::List;

use super::*;

/// The version OCaml writes next to `data`: `Precomputed_block.Stable.V4`,
/// with versioned JSON.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L70-L96>
pub const PRECOMPUTED_BLOCK_VERSION: u32 = 4;

/// The block proof, as base64 (URL-safe alphabet) of its bin_prot.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L5-L43>
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite)]
pub struct PrecomputedBlockProof(pub MinaBaseProofStableV2);

impl Serialize for PrecomputedBlockProof {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        use base64::{engine::general_purpose::URL_SAFE, Engine as _};
        let mut buf = Vec::new();
        self.0
            .binprot_write(&mut buf)
            .map_err(serde::ser::Error::custom)?;
        let base64_data = URL_SAFE.encode(&buf);
        serializer.serialize_str(&base64_data)
    }
}

impl<'de> Deserialize<'de> for PrecomputedBlockProof {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        use base64::{engine::general_purpose::URL_SAFE, Engine as _};
        let base64_data = String::deserialize(deserializer)?;
        let binprot_data = URL_SAFE
            .decode(&base64_data)
            .map_err(serde::de::Error::custom)?;
        let mut read = binprot_data.as_slice();
        let proof: MinaBaseProofStableV2 =
            binprot::BinProtRead::binprot_read(&mut read).map_err(serde::de::Error::custom)?;
        Ok(PrecomputedBlockProof(proof))
    }
}

impl From<MinaBaseProofStableV2> for PrecomputedBlockProof {
    fn from(value: MinaBaseProofStableV2) -> Self {
        Self(value)
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L45-L66>
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct PrecomputedBlock {
    /// When the node wrote the block, not the block's own timestamp.
    pub scheduled_time: BlockTimeTimeStableV1,
    pub protocol_state: MinaStateProtocolStateValueStableV2,
    pub protocol_state_proof: PrecomputedBlockProof,
    pub staged_ledger_diff: StagedLedgerDiffDiffStableV2,
    /// The header's delta block chain proof. OCaml declares it as ledger hashes
    /// rather than a state hash and state body hashes, so its JSON has the
    /// ledger hash version byte; see [`delta_chain_proof_json`].
    #[serde(with = "delta_chain_proof_json")]
    pub delta_transition_chain_proof: (StateHash, List<StateBodyHash>),
    pub protocol_version: ProtocolVersionStableV2,
    /// `[@default None]`: OCaml leaves the field out when it is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_protocol_version: Option<ProtocolVersionStableV2>,
    pub accounts_accessed: List<(crate::number::UInt64, MinaBaseAccountBinableArgStableV2)>,
    pub accounts_created: List<(MinaBaseAccountIdStableV2, CurrencyFeeStableV1)>,
    pub tokens_used: List<(MinaBaseTokenIdStableV2, Option<MinaBaseAccountIdStableV2>)>,
}

/// A precomputed block with its version, `{ "version": 4, "data": … }`, as
/// OCaml writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct PrecomputedBlockData {
    pub version: u32,
    pub data: PrecomputedBlock,
}

impl PrecomputedBlock {
    pub fn with_version(&self, version: u32) -> PrecomputedBlockData {
        PrecomputedBlockData {
            version,
            data: self.clone(),
        }
    }

    /// The block itself.
    pub fn block(&self) -> MinaBlockBlockStableV2 {
        MinaBlockBlockStableV2 {
            header: MinaBlockHeaderStableV2 {
                protocol_state: self.protocol_state.clone(),
                protocol_state_proof: std::sync::Arc::new(self.protocol_state_proof.0.clone()),
                delta_block_chain_proof: self.delta_transition_chain_proof.clone(),
                current_protocol_version: self.protocol_version.clone(),
                proposed_protocol_version_opt: self.proposed_protocol_version.clone(),
            },
            body: StagedLedgerDiffBodyStableV1 {
                staged_ledger_diff: self.staged_ledger_diff.clone(),
            },
        }
    }
}

impl From<PrecomputedBlock> for PrecomputedBlockData {
    fn from(data: PrecomputedBlock) -> Self {
        Self {
            version: PRECOMPUTED_BLOCK_VERSION,
            data,
        }
    }
}

impl PrecomputedBlockData {
    /// Reads a precomputed block as OCaml writes it.
    pub fn from_ocaml_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let mut value = ocaml_json_to_value(bytes)?;
        ocaml_forms_to_ours(&mut value)?;
        serde_json::from_value(value)
    }

    /// Writes the block as OCaml does, which is not necessarily UTF-8: see the
    /// module documentation. `hashes` computes the hashes OCaml writes next to
    /// verification keys and account updates.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L70-L99>
    pub fn to_ocaml_json(
        &self,
        hashes: &dyn PrecomputedHashes,
    ) -> Result<Vec<u8>, serde_json::Error> {
        use serde::ser::Error;
        let mut builder = NodeBuilder::default();
        {
            let mut serializer =
                serde_json::Serializer::with_formatter(io::sink(), Builder(&mut builder));
            self.serialize(&mut serializer)?;
        }
        let mut node = builder
            .root
            .ok_or_else(|| serde_json::Error::custom("nothing serialized"))?;
        ours_to_ocaml(&mut node, &mut Vec::new(), hashes).map_err(serde_json::Error::custom)?;
        let mut out = Vec::new();
        node.write(&mut out);
        Ok(out)
    }
}

/// OCaml declares the delta transition chain proof as
/// `Frozen_ledger_hash.t * Frozen_ledger_hash.t list`, so the hashes in its
/// JSON are base58check with the ledger hash version byte. The bin_prot is
/// that of the header's state hash and state body hashes.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L57-L58>
pub mod delta_chain_proof_json {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::{
        list::List,
        v2::{
            DataHashLibStateHashStableV1, LedgerHash, MinaBaseLedgerHash0StableV1,
            MinaBaseStateBodyHashStableV1, StateBodyHash, StateHash,
        },
    };

    pub fn serialize<S: Serializer>(
        proof: &(StateHash, List<StateBodyHash>),
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if !serializer.is_human_readable() {
            return proof.serialize(serializer);
        }
        let ledger_hash = |hash: &crate::bigint::BigInt| -> LedgerHash {
            MinaBaseLedgerHash0StableV1(hash.clone()).into()
        };
        let (first, rest) = proof;
        (
            ledger_hash(&first.inner().0),
            rest.iter()
                .map(|hash| ledger_hash(&hash.inner().0))
                .collect::<Vec<_>>(),
        )
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<(StateHash, List<StateBodyHash>), D::Error> {
        if !deserializer.is_human_readable() {
            return Deserialize::deserialize(deserializer);
        }
        let (first, rest): (LedgerHash, Vec<LedgerHash>) = Deserialize::deserialize(deserializer)?;
        Ok((
            DataHashLibStateHashStableV1(first.into_inner().0).into(),
            rest.into_iter()
                .map(|hash| MinaBaseStateBodyHashStableV1(hash.into_inner().0).into())
                .collect(),
        ))
    }
}

/// Parses OCaml's JSON, whose byte strings need not be UTF-8. JSON parsers
/// reject those, so every string whose content is not UTF-8 is first rewritten
/// as an array of its bytes, which byte string fields also accept.
pub fn ocaml_json_to_value(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice(&byte_strings_to_arrays(bytes)?)
}

fn byte_strings_to_arrays(input: &[u8]) -> Result<Vec<u8>, serde_json::Error> {
    use serde::de::Error;
    let error = |message: &str, at: usize| serde_json::Error::custom(format!("{message} at {at}"));
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != b'"' {
            out.push(input[i]);
            i += 1;
            continue;
        }
        let start = i;
        let mut content = Vec::new();
        i += 1;
        loop {
            match input.get(i) {
                None => return Err(error("unterminated string", start)),
                Some(b'"') => break,
                Some(b'\\') => {
                    let escaped = *input.get(i + 1).ok_or_else(|| error("bad escape", i))?;
                    i += 2;
                    match escaped {
                        b'n' => content.push(b'\n'),
                        b't' => content.push(b'\t'),
                        b'r' => content.push(b'\r'),
                        b'b' => content.push(8),
                        b'f' => content.push(12),
                        b'u' => {
                            let code = input
                                .get(i..i + 4)
                                .and_then(|hex| std::str::from_utf8(hex).ok())
                                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                                .ok_or_else(|| error("bad escape", i))?;
                            i += 4;
                            match char::from_u32(code) {
                                Some(c) => {
                                    content.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes())
                                }
                                // A surrogate: leave the input to the JSON parser.
                                None => return Ok(input.to_vec()),
                            }
                        }
                        other => content.push(other),
                    }
                    continue;
                }
                Some(&byte) => content.push(byte),
            }
            i += 1;
        }
        i += 1;
        if std::str::from_utf8(&content).is_ok() {
            out.extend_from_slice(&input[start..i]);
        } else {
            out.extend_from_slice(serde_json::to_string(&content)?.as_bytes());
        }
    }
    Ok(out)
}

/// One step of a path into the JSON.
#[derive(Debug, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// How the precomputed format writes a string that our types write another
/// way.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Form {
    /// Decimal MINA, where ours is nanomina.
    Mina,
    /// A receipt chain hash in base58check, where ours is hex.
    ReceiptChainHash,
    /// A token id in base58check, where ours is hex.
    TokenId,
    /// A JSON number, where ours is a string of digits.
    Number,
}

/// The fields of each [`Form`], as path suffixes: `#` is any index in an
/// array, and `*` either bound of an interval (`lower`, `upper`). Found in the
/// bucket's blocks. Account update amounts read as nanomina hash to the digest
/// OCaml writes next to them (`tools/block-replay` checks it).
// Preconditions (network/epoch total currency and account balance) are
// nanomina in Mesa bucket blocks. P1e's devnet fixtures pin this through
// their OCaml account-update digests; converting them to MINA changes the
// digest or overflows. Timing amounts have no held mainnet/devnet sample.
const FORMS: &[(&str, Form)] = &[
    (
        "elt.account_update.body.balance_change.magnitude",
        Form::Mina,
    ),
    (
        "elt.account_update.body.update.timing.1.initial_minimum_balance",
        Form::Mina,
    ),
    (
        "elt.account_update.body.update.timing.1.cliff_amount",
        Form::Mina,
    ),
    (
        "elt.account_update.body.update.timing.1.vesting_increment",
        Form::Mina,
    ),
    (
        "accounts_accessed.#.1.receipt_chain_hash",
        Form::ReceiptChainHash,
    ),
    ("accounts_created.#.0.1", Form::TokenId),
    ("tokens_used.#.0", Form::TokenId),
    ("tokens_used.#.1.1", Form::TokenId),
    ("accounts_accessed.#.0", Form::Number),
    ("data.protocol_version.transaction", Form::Number),
    ("data.protocol_version.network", Form::Number),
    ("data.protocol_version.patch", Form::Number),
    ("data.proposed_protocol_version.transaction", Form::Number),
    ("data.proposed_protocol_version.network", Form::Number),
    ("data.proposed_protocol_version.patch", Form::Number),
];

/// Paths of an account's timing, which OCaml writes as `["Untimed"]` or
/// `["Timed", { … }]` rather than as `"Untimed"` or `{ "Timed": { … } }`.
const TIMINGS: &[&str] = &["accounts_accessed.#.1.timing"];

/// Paths of a vesting period, which OCaml writes as a number rather than as
/// `{ "GlobalSlotSpan": … }`, in OCaml's shape of the JSON.
const VESTING_PERIODS: &[&str] = &[
    "accounts_accessed.#.1.timing.1.vesting_period",
    "elt.account_update.body.update.timing.1.vesting_period",
];

/// Paths of a verification key, which OCaml writes as `{ "data": base58check,
/// "hash": … }` rather than field by field.
const VERIFICATION_KEYS: &[&str] = &[
    "accounts_accessed.#.1.zkapp.verification_key",
    "elt.account_update.body.update.verification_key.1",
];

fn step_matches(pattern: &str, step: &Step) -> bool {
    match step {
        Step::Key(key) => key == pattern || (pattern == "*" && (key == "lower" || key == "upper")),
        Step::Index(index) => pattern == "#" || pattern.parse() == Ok(*index),
    }
}

fn ends_with(path: &[Step], pattern: &str) -> bool {
    let pattern: Vec<_> = pattern.split('.').collect();
    path.len() >= pattern.len()
        && pattern
            .iter()
            .zip(&path[path.len() - pattern.len()..])
            .all(|(pattern, step)| step_matches(pattern, step))
}

fn form(path: &[Step]) -> Option<Form> {
    FORMS
        .iter()
        .find(|(pattern, _)| ends_with(path, pattern))
        .map(|(_, form)| *form)
}

/// Whether a string is a field element as our types write it, `0x` and 64
/// lowercase hex digits, which OCaml writes in uppercase. Strings of users'
/// choosing in accounts and account updates are left alone.
fn is_field_hex(path: &[Step], s: &[u8]) -> bool {
    let user_string =
        path.iter().rev().take(2).any(
            |step| matches!(step, Step::Key(key) if key == "zkapp_uri" || key == "token_symbol"),
        );
    s.len() == 66
        && s.starts_with(b"0x")
        && s[2..].iter().all(u8::is_ascii_hexdigit)
        && !user_string
}

/// Rewrites a precomputed block, or any part of one, from OCaml's forms to
/// ours, so that our types read it.
pub fn ocaml_forms_to_ours(value: &mut Value) -> Result<(), serde_json::Error> {
    fn go(value: &mut Value, path: &mut Vec<Step>) -> Result<(), serde_json::Error> {
        if TIMINGS.iter().any(|pattern| ends_with(path, pattern)) {
            // `["Untimed"]` to `"Untimed"`, `["Timed", t]` to `{ "Timed": t }`.
            if let Value::Array(items) = value {
                *value = match std::mem::take(items).as_mut_slice() {
                    [tag @ Value::String(_)] => tag.take(),
                    [Value::String(tag), timing] => {
                        let mut timing = timing.take();
                        path.push(Step::Index(1));
                        go(&mut timing, path)?;
                        path.pop();
                        Value::Object([(std::mem::take(tag), timing)].into_iter().collect())
                    }
                    other => Value::Array(other.to_vec()),
                };
                return Ok(());
            }
        }
        if VESTING_PERIODS
            .iter()
            .any(|pattern| ends_with(path, pattern))
            && value.is_string()
        {
            *value = serde_json::json!({ "GlobalSlotSpan": value.take() });
            return Ok(());
        }
        match value {
            Value::String(s) => match form(path) {
                Some(Form::Mina) => *s = mina_to_nanomina(s)?.to_string(),
                Some(Form::ReceiptChainHash) => {
                    let hash: ReceiptChainHash = s.parse().map_err(serde::de::Error::custom)?;
                    *value = serde_json::to_value(hash.into_inner())?;
                }
                Some(Form::TokenId) => {
                    let token_id: TokenIdKeyHash = s.parse().map_err(serde::de::Error::custom)?;
                    *value = serde_json::to_value(token_id.into_inner())?;
                }
                Some(Form::Number) | None => {}
            },
            Value::Array(items) => {
                for (i, item) in items.iter_mut().enumerate() {
                    path.push(Step::Index(i));
                    go(item, path)?;
                    path.pop();
                }
            }
            Value::Object(fields) => {
                for (key, field) in fields.iter_mut() {
                    path.push(Step::Key(key.clone()));
                    go(field, path)?;
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }
    go(value, &mut Vec::new())
}

/// `"1.5"` (MINA) to 1500000000 (nanomina).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L98-L112>
pub fn mina_to_nanomina(mina: &str) -> Result<u64, serde_json::Error> {
    use serde::de::Error;
    let (whole, fraction) = mina.split_once('.').unwrap_or((mina, ""));
    if fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(serde_json::Error::custom(format!("bad amount {mina}")));
    }
    format!("{whole}{fraction:0<9}")
        .parse()
        .map_err(|_| serde_json::Error::custom(format!("bad amount {mina}")))
}

/// 1500000000 (nanomina) to `"1.5"` (MINA).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L82-L96>
pub fn nanomina_to_mina(nanomina: u64) -> String {
    let (whole, fraction) = (nanomina / 1_000_000_000, nanomina % 1_000_000_000);
    if fraction == 0 {
        whole.to_string()
    } else {
        let fraction = format!("{fraction:09}");
        format!("{whole}.{}", fraction.trim_end_matches('0'))
    }
}

/// Hashes OCaml writes in precomputed blocks that our types don't keep, and
/// which take the ledger's hashing to compute.
pub trait PrecomputedHashes {
    /// The hash of a verification key.
    fn verification_key(
        &self,
        vk: &MinaBaseVerificationKeyWireStableV1,
    ) -> Result<crate::bigint::BigInt, String>;

    /// The digest and stack hash of each account update of a zkApp command,
    /// depth first: an account update, then the ones it calls.
    fn zkapp_command(
        &self,
        command: &MinaBaseZkappCommandTStableV1WireStableV1,
    ) -> Result<Vec<(crate::bigint::BigInt, crate::bigint::BigInt)>, String>;
}

/// A JSON value as the precomputed format needs it: objects keep their
/// order, and byte strings that are not UTF-8 stay bytes.
#[derive(Clone, Debug, PartialEq)]
enum Node {
    /// A number, `true`, `false` or `null`, as written.
    Raw(String),
    String(Vec<u8>),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl Node {
    fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        match self {
            Node::Object(fields) => fields
                .iter_mut()
                .find(|(k, _)| k == key)
                .map(|(_, field)| field),
            _ => None,
        }
    }

    fn to_value(&self) -> Result<Value, serde_json::Error> {
        Ok(match self {
            Node::Raw(raw) => serde_json::from_str(raw)?,
            Node::String(s) => match std::str::from_utf8(s) {
                Ok(s) => Value::String(s.to_owned()),
                Err(_) => serde_json::to_value(s)?,
            },
            Node::Array(items) => {
                Value::Array(items.iter().map(Node::to_value).collect::<Result<_, _>>()?)
            }
            Node::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, field)| Ok((key.clone(), field.to_value()?)))
                    .collect::<Result<_, serde_json::Error>>()?,
            ),
        })
    }

    /// Writes the value as Yojson does.
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Node::Raw(raw) => out.extend_from_slice(raw.as_bytes()),
            Node::String(s) => write_yojson_string(out, s),
            Node::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    item.write(out);
                }
                out.push(b']');
            }
            Node::Object(fields) => {
                out.push(b'{');
                for (i, (key, field)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_yojson_string(out, key.as_bytes());
                    out.push(b':');
                    field.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

/// Writes a string as Yojson does: `"`, `\\` and the control characters below
/// 0x20 escaped, `\u007f` too, and everything else, including bytes that are
/// not UTF-8, as it is.
fn write_yojson_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    let mut start = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        let escape: &[u8] = match byte {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            8 => b"\\b",
            12 => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0..=0x1f | 0x7f => b"",
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        if escape.is_empty() {
            out.extend_from_slice(format!("\\u{byte:04x}").as_bytes());
        } else {
            out.extend_from_slice(escape);
        }
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

/// Rewrites a [`Node`] from our forms to OCaml's, children first.
fn ours_to_ocaml(
    node: &mut Node,
    path: &mut Vec<Step>,
    hashes: &dyn PrecomputedHashes,
) -> Result<(), String> {
    if ends_with(path, "commands.#.data") {
        fill_zkapp_hashes(node, hashes)?;
    }
    match node {
        Node::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                path.push(Step::Index(i));
                ours_to_ocaml(item, path, hashes)?;
                path.pop();
            }
        }
        Node::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                path.push(Step::Key(key.clone()));
                ours_to_ocaml(field, path, hashes)?;
                path.pop();
            }
        }
        _ => {}
    }
    let bad = |what: &str, node: &Node| format!("{what} at {path:?}: {node:?}");
    let replacement = match &*node {
        Node::String(s) => match form(path) {
            Some(Form::Mina) => {
                let nanomina = std::str::from_utf8(s)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| bad("not an amount", node))?;
                Some(Node::String(nanomina_to_mina(nanomina).into_bytes()))
            }
            Some(Form::ReceiptChainHash) => {
                let hash: MinaBaseReceiptChainHashStableV1 =
                    serde_json::from_value(node.to_value().map_err(|e| e.to_string())?)
                        .map_err(|e| bad(&e.to_string(), node))?;
                Some(Node::String(
                    ReceiptChainHash::from(hash).to_string().into_bytes(),
                ))
            }
            Some(Form::TokenId) => {
                let token_id: MinaBaseAccountIdDigestStableV1 =
                    serde_json::from_value(node.to_value().map_err(|e| e.to_string())?)
                        .map_err(|e| bad(&e.to_string(), node))?;
                Some(Node::String(
                    TokenIdKeyHash::from(token_id).to_string().into_bytes(),
                ))
            }
            Some(Form::Number) if !s.is_empty() && s.iter().all(u8::is_ascii_digit) => {
                Some(Node::Raw(String::from_utf8_lossy(s).into_owned()))
            }
            Some(Form::Number) => return Err(bad("not a number", node)),
            None if is_field_hex(path, s) => {
                let mut upper = s.clone();
                upper[2..].make_ascii_uppercase();
                Some(Node::String(upper))
            }
            None => None,
        },
        // `{ "GlobalSlotSpan": n }` to `n`.
        Node::Object(fields) if fields.len() == 1 && fields[0].0 == "GlobalSlotSpan" => {
            Some(fields[0].1.clone())
        }
        _ => None,
    };
    if let Some(replacement) = replacement {
        *node = replacement;
    }
    if TIMINGS.iter().any(|pattern| ends_with(path, pattern)) {
        // `"Untimed"` to `["Untimed"]`, `{ "Timed": t }` to `["Timed", t]`.
        *node = match std::mem::replace(node, Node::Raw(String::new())) {
            tag @ Node::String(_) => Node::Array(vec![tag]),
            Node::Object(mut fields) if fields.len() == 1 => {
                let (tag, timing) = fields.remove(0);
                Node::Array(vec![Node::String(tag.into_bytes()), timing])
            }
            other => return Err(bad("not a timing", &other)),
        };
    }
    if VERIFICATION_KEYS
        .iter()
        .any(|pattern| ends_with(path, pattern))
        && matches!(node, Node::Object(_))
    {
        let vk: MinaBaseVerificationKeyWireStableV1 =
            serde_json::from_value(node.to_value().map_err(|e| e.to_string())?)
                .map_err(|e| bad(&e.to_string(), node))?;
        let hash = hashes.verification_key(&vk)?;
        let mut hash = serde_json::to_value(hash)
            .map_err(|e| e.to_string())?
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        hash[2..].make_ascii_uppercase();
        *node = Node::Object(vec![
            (
                "data".to_owned(),
                Node::String(vk.to_base58check().map_err(|e| e.to_string())?.into_bytes()),
            ),
            ("hash".to_owned(), Node::String(hash)),
        ]);
    }
    Ok(())
}

/// Fills in the account update digests and stack hashes of a user command
/// (`["Zkapp_command", { … }]`), which our types write as `null`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/legacy_format.ml#L41-L62>
fn fill_zkapp_hashes(node: &mut Node, hashes: &dyn PrecomputedHashes) -> Result<(), String> {
    let Node::Array(items) = node else {
        return Ok(());
    };
    let [Node::String(tag), command] = items.as_mut_slice() else {
        return Ok(());
    };
    if tag != b"Zkapp_command" {
        return Ok(());
    }
    let wire: MinaBaseZkappCommandTStableV1WireStableV1 =
        serde_json::from_value(command.to_value().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let mut digests = hashes.zkapp_command(&wire)?.into_iter();

    fn fill(
        forest: Option<&mut Node>,
        digests: &mut impl Iterator<Item = (crate::bigint::BigInt, crate::bigint::BigInt)>,
    ) -> Result<(), String> {
        let Some(Node::Array(items)) = forest else {
            return Err("account updates are not a list".to_owned());
        };
        for item in items {
            let (digest, stack_hash) = digests.next().ok_or("too few account update digests")?;
            let hex = |hash| -> Result<Node, String> {
                let hex = serde_json::to_value(hash).map_err(|e| e.to_string())?;
                Ok(Node::String(
                    hex.as_str().unwrap_or_default().as_bytes().to_vec(),
                ))
            };
            let elt = item.get_mut("elt").ok_or("account update without `elt`")?;
            *elt.get_mut("account_update_digest")
                .ok_or("account update without a digest")? = hex(digest)?;
            fill(elt.get_mut("calls"), digests)?;
            *item
                .get_mut("stack_hash")
                .ok_or("account update without a stack hash")? = hex(stack_hash)?;
        }
        Ok(())
    }
    fill(command.get_mut("account_updates"), &mut digests)?;
    match digests.next() {
        None => Ok(()),
        Some(_) => Err("too many account update digests".to_owned()),
    }
}

/// Builds a [`Node`] from what `serde_json` serializes, instead of writing
/// it.
#[derive(Default)]
struct NodeBuilder {
    /// Open arrays and objects, with the key of an object's next field.
    open: Vec<(Node, String)>,
    string: Vec<u8>,
    in_key: bool,
    root: Option<Node>,
}

impl NodeBuilder {
    fn push(&mut self, node: Node) {
        match self.open.last_mut() {
            Some((Node::Array(items), _)) => items.push(node),
            Some((Node::Object(fields), key)) => fields.push((std::mem::take(key), node)),
            _ => self.root = Some(node),
        }
    }

    fn scalar(&mut self, raw: String) -> io::Result<()> {
        if self.in_key {
            self.string.extend_from_slice(raw.as_bytes());
        } else {
            self.push(Node::Raw(raw));
        }
        Ok(())
    }

    fn close(&mut self) -> io::Result<()> {
        let (node, _) = self
            .open
            .pop()
            .ok_or_else(|| io::Error::other("unbalanced JSON"))?;
        self.push(node);
        Ok(())
    }
}

struct Builder<'a>(&'a mut NodeBuilder);

macro_rules! scalars {
    ($($method:ident: $ty:ty),*) => {
        $(fn $method<W: ?Sized + io::Write>(&mut self, _: &mut W, value: $ty) -> io::Result<()> {
            self.0.scalar(value.to_string())
        })*
    };
}

impl Formatter for Builder<'_> {
    scalars!(
        write_bool: bool, write_i8: i8, write_i16: i16, write_i32: i32, write_i64: i64,
        write_i128: i128, write_u8: u8, write_u16: u16, write_u32: u32, write_u64: u64,
        write_u128: u128, write_f32: f32, write_f64: f64, write_number_str: &str,
        write_raw_fragment: &str
    );

    fn write_null<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.scalar("null".to_owned())
    }

    fn write_byte_array<W: ?Sized + io::Write>(
        &mut self,
        _: &mut W,
        value: &[u8],
    ) -> io::Result<()> {
        self.0.push(Node::String(value.to_vec()));
        Ok(())
    }

    fn begin_string<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.string.clear();
        Ok(())
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        _: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        self.0.string.extend_from_slice(fragment.as_bytes());
        Ok(())
    }

    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        _: &mut W,
        char_escape: CharEscape,
    ) -> io::Result<()> {
        self.0.string.push(match char_escape {
            CharEscape::Quote => b'"',
            CharEscape::ReverseSolidus => b'\\',
            CharEscape::Solidus => b'/',
            CharEscape::Backspace => 8,
            CharEscape::FormFeed => 12,
            CharEscape::LineFeed => b'\n',
            CharEscape::CarriageReturn => b'\r',
            CharEscape::Tab => b'\t',
            CharEscape::AsciiControl(byte) => byte,
        });
        Ok(())
    }

    fn end_string<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        let string = std::mem::take(&mut self.0.string);
        if !self.0.in_key {
            self.0.push(Node::String(string));
        } else if let Some((_, key)) = self.0.open.last_mut() {
            *key = String::from_utf8(string).map_err(io::Error::other)?;
        }
        Ok(())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.open.push((Node::Array(Vec::new()), String::new()));
        Ok(())
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.close()
    }

    fn begin_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W, _: bool) -> io::Result<()> {
        Ok(())
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.open.push((Node::Object(Vec::new()), String::new()));
        Ok(())
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.0.close()
    }

    fn begin_object_key<W: ?Sized + io::Write>(&mut self, _: &mut W, _: bool) -> io::Result<()> {
        self.0.in_key = true;
        Ok(())
    }

    fn end_object_key<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        if self.0.in_key && !self.0.string.is_empty() {
            // A key written as a number.
            let string = std::mem::take(&mut self.0.string);
            if let Some((_, key)) = self.0.open.last_mut() {
                *key = String::from_utf8(string).map_err(io::Error::other)?;
            }
        }
        self.0.in_key = false;
        Ok(())
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        Ok(())
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        Ok(())
    }
}

impl MinaBaseVerificationKeyWireStableV1 {
    /// The base58check form OCaml uses for JSON: bin_prot behind version byte
    /// 0x1b. The inverse of [`Self::from_base58check`].
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/codable/codable.ml#L80-L99>
    pub fn to_base58check(&self) -> Result<String, conv::Error> {
        let mut bytes = Vec::new();
        self.binprot_write(&mut bytes)?;
        Ok(crate::b58::encode(
            &bytes,
            crate::b58version::VERIFICATION_KEY,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_utf8_token_symbol_from_ocaml_json() {
        use crate::v2::MinaBaseAccountUpdateUpdateStableV1TokenSymbol;

        // Mainnet block 565083 has this raw token-symbol byte in a Set update.
        let json = b"{\"token_symbol\":[\"Set\",\"\xff\"]}";
        let value = ocaml_json_to_value(json).unwrap();
        assert_eq!(value["token_symbol"], serde_json::json!(["Set", [255]]));
        let symbol: MinaBaseAccountUpdateUpdateStableV1TokenSymbol =
            serde_json::from_value(value["token_symbol"].clone()).unwrap();
        match symbol {
            MinaBaseAccountUpdateUpdateStableV1TokenSymbol::Set(bytes) => {
                assert_eq!(bytes.as_ref(), &[0xff]);
            }
            MinaBaseAccountUpdateUpdateStableV1TokenSymbol::Keep => panic!("expected Set"),
        }
    }

    fn path(steps: &str) -> Vec<Step> {
        steps
            .split('.')
            .map(|step| match step.parse() {
                Ok(index) => Step::Index(index),
                Err(_) => Step::Key(step.to_owned()),
            })
            .collect()
    }

    #[test]
    fn mina_strings() {
        for (nanomina, mina) in [
            (0, "0"),
            (1, "0.000000001"),
            (1_500_000_000, "1.5"),
            (156_918_444_467, "156.918444467"),
            (30_000_000, "0.03"),
            (u64::MAX, "18446744073.709551615"),
        ] {
            assert_eq!(nanomina_to_mina(nanomina), mina);
            assert_eq!(mina_to_nanomina(mina).unwrap(), nanomina);
        }
        assert!(mina_to_nanomina("1.0000000001").is_err());
        assert!(mina_to_nanomina("1.-5").is_err());
    }

    #[test]
    fn yojson_escapes() {
        let mut out = Vec::new();
        write_yojson_string(&mut out, b"a\"\\\x08\x0c\n\r\t\x01\x1f\x7f\xd5/z");
        assert_eq!(
            out,
            b"\"a\\\"\\\\\\b\\f\\n\\r\\t\\u0001\\u001f\\u007f\xd5/z\"".to_vec()
        );
        let value = ocaml_json_to_value(&out).unwrap();
        assert_eq!(
            value,
            serde_json::json!([
                b'a', b'"', b'\\', 8, 12, b'\n', b'\r', b'\t', 1, 0x1f, 0x7f, 0xd5, b'/', b'z'
            ])
        );
    }

    #[test]
    fn forms() {
        let update = "data.staged_ledger_diff.diff.0.commands.3.data.1.account_updates.0.elt.calls.2.elt.account_update.body";
        for (at, expected) in [
            (
                format!("{update}.balance_change.magnitude"),
                Some(Form::Mina),
            ),
            (
                format!("{update}.update.timing.1.cliff_amount"),
                Some(Form::Mina),
            ),
            (
                format!("{update}.preconditions.account.balance.1.lower"),
                None,
            ),
            (
                format!("{update}.preconditions.account.balance.1.upper"),
                None,
            ),
            (
                format!("{update}.preconditions.account.nonce.1.lower"),
                None,
            ),
            (format!("{update}.balance_change"), None),
            (
                "data.staged_ledger_diff.diff.0.commands.3.data.1.fee_payer.body.fee".to_owned(),
                None,
            ),
            (
                "data.accounts_accessed.7.1.receipt_chain_hash".to_owned(),
                Some(Form::ReceiptChainHash),
            ),
            ("data.protocol_version.patch".to_owned(), Some(Form::Number)),
        ] {
            assert_eq!(form(&path(&at)), expected, "{at}");
        }
    }

    /// An account as OCaml writes it in `accounts_accessed`, read and written
    /// back.
    #[test]
    fn account_forms() {
        let ocaml = serde_json::json!({ "data": { "accounts_accessed": [[0, {
            "receipt_chain_hash": "2n1Gv4jQZ2WjCdSqKUgMzxhDF7fWZgDhxSoEDM5BajuGKzGQDGwB",
            "timing": ["Timed", {
                "initial_minimum_balance": "66000000000000",
                "cliff_time": "172800",
                "cliff_amount": "16500000000000",
                "vesting_period": "1",
                "vesting_increment": "95486111"
            }],
            "other": "0x0641662e94d68ec970d0afc059d02729bbf4a2cd88c548ccd9fb1e26e570c66c"
        }]], "protocol_version": { "transaction": 4, "network": 0, "patch": 0 } } });
        let mut ours = ocaml.clone();
        ocaml_forms_to_ours(&mut ours).unwrap();
        let account = &ours["data"]["accounts_accessed"][0][1];
        assert!(account["receipt_chain_hash"]
            .as_str()
            .unwrap()
            .starts_with("0x"));
        assert_eq!(
            account["timing"]["Timed"]["vesting_period"],
            serde_json::json!({ "GlobalSlotSpan": "1" })
        );
        let timing: MinaBaseAccountTimingStableV2 =
            serde_json::from_value(account["timing"].clone()).unwrap();
        assert!(matches!(
            timing,
            MinaBaseAccountTimingStableV2::Timed { .. }
        ));

        let mut builder = NodeBuilder::default();
        let mut serializer =
            serde_json::Serializer::with_formatter(io::sink(), Builder(&mut builder));
        ours.serialize(&mut serializer).unwrap();
        let mut node = builder.root.unwrap();
        struct NoHashes;
        impl PrecomputedHashes for NoHashes {
            fn verification_key(
                &self,
                _: &MinaBaseVerificationKeyWireStableV1,
            ) -> Result<crate::bigint::BigInt, String> {
                Err("no verification key expected".to_owned())
            }
            fn zkapp_command(
                &self,
                _: &MinaBaseZkappCommandTStableV1WireStableV1,
            ) -> Result<Vec<(crate::bigint::BigInt, crate::bigint::BigInt)>, String> {
                Err("no zkApp command expected".to_owned())
            }
        }
        ours_to_ocaml(&mut node, &mut Vec::new(), &NoHashes).unwrap();
        let mut written = Vec::new();
        node.write(&mut written);
        let mut expected = ocaml;
        expected["data"]["accounts_accessed"][0][1]["other"] =
            serde_json::json!("0x0641662E94D68EC970D0AFC059D02729BBF4A2CD88C548CCD9FB1E26E570C66C");
        assert_eq!(serde_json::from_slice::<Value>(&written).unwrap(), expected);
    }
}
