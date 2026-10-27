//! Verify every proof a precomputed block carries:
//!
//! - its protocol state proof, the blockchain SNARK, against the blockchain
//!   verifier key, with `verify_block`, as the node checks a block it
//!   receives;
//! - every transaction SNARK of its staged ledger diff's completed works
//!   against the transaction verifier key, with `verify_transaction`, the
//!   block's proofs as one batch as the node does, and one by one only if the
//!   batch fails. Each proof's sok digest must also be the digest of its
//!   work's fee and prover;
//! - every zkApp proof, of the proof-authorized account updates of its applied
//!   zkApp commands, with `verify_zkapp`, against the verification key whose
//!   hash the account update names. The keys come from the account updates
//!   that set them, in any block of the run, so these proofs are checked once
//!   every block has been read, and from the accounts of ledgers given with
//!   the keys ([`Keys::ledger_zkapp`]), for keys set before the run's blocks;
//! - the header checks the node runs on a received block before verifying its
//!   proof, with the node's own functions: the delta block chain proof
//!   (`validate_delta_block_chain`) and the body reference
//!   (`validate_body_reference`), the hash of the staged ledger diff.
//!
//! The replay applies blocks with `SkipVerification::All`, so it checks none
//! of these proofs. It does check each block's staged ledger hash, every
//! signature (`verifier::common::check` runs whatever the skip setting), and
//! that each proof-authorized account update's account holds a key of the
//! hash the update names.
//!
//! A directory of blocks from the public bucket can hold blocks of two chains
//! at the same heights: after a hard fork the old chain goes on for hours
//! (research note R1). Blocks are therefore sorted by the genesis state hash
//! they carry, as OCaml's `validate_genesis_protocol_state` does before any
//! proof check, and only those of the chain being checked are verified. The
//! others are reported apart, by genesis and protocol version, and are not
//! failures.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use ledger::{
    proofs::{
        verification::{verify_block, verify_transaction, verify_zkapp},
        VerifierIndex,
    },
    scan_state::{
        scan_state::transaction_snark::{SokDigest, SokMessage, Statement},
        transaction_logic::{
            zkapp_command::{AuthorizationKind, Control, SetOrKeep, SideLoadedProof},
            zkapp_statement::ZkappStatement,
            TransactionStatus, UserCommand,
        },
    },
    staged_ledger::diff::Diff,
    verifier::get_srs,
    Account, VerificationKey,
};
use mina_core::block::prevalidate::validate_delta_block_chain;
use mina_curves::pasta::{Fp, Fq};
use mina_node::transition_frontier::body_reference::validate_body_reference;
use mina_p2p_messages::{
    bigint::BigInt,
    list::List,
    v2::{
        DataHashLibStateHashStableV1, LedgerHash, MinaBaseStateBodyHashStableV1,
        MinaBlockBlockStableV2, MinaBlockHeaderStableV2, PicklesProofProofsVerified2ReprStableV2,
        StagedLedgerDiffBodyStableV1, StagedLedgerDiffDiffStableV2, StateBodyHash, StateHash,
        TransactionSnarkProofStableV2, TransactionSnarkWorkTStableV2,
        TransactionSnarkWorkTStableV2Proofs,
    },
};
use rayon::prelude::*;
use serde_json::Value;

use crate::{for_each_account_update, parse_block_data, read_block_json};

/// `genesis_state_hash` of every Mesa mainnet block: the Mesa genesis, at
/// height 548147, the child of the Berkeley fork block 548146 `3NKHyxzg…`
/// (research note R1).
pub const MAINNET_MESA_GENESIS: &str = "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7";
/// `genesis_state_hash` of every Mesa devnet block (height 545434).
pub const DEVNET_MESA_GENESIS: &str = "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ";

/// The Mesa genesis state hash of a network, if known.
pub fn mesa_genesis(network: &str) -> Option<&'static str> {
    match network {
        "mainnet" => Some(MAINNET_MESA_GENESIS),
        "devnet" => Some(DEVNET_MESA_GENESIS),
        _ => None,
    }
}

/// The two verifier keys, and zkApp verification keys from ledgers.
#[derive(Clone)]
pub struct Keys {
    pub block: Arc<VerifierIndex<Fq>>,
    pub transaction: Arc<VerifierIndex<Fq>>,
    /// zkApp verification keys held by the accounts of ledgers, like the
    /// genesis ledger or a node's root ledger, by their hash
    /// ([`zkapp_keys_of_accounts`]). They serve the proofs whose key no block
    /// of the run sets: keys set at genesis, or by blocks the run lacks.
    pub ledger_zkapp: Arc<HashMap<Fp, VerificationKey>>,
}

/// The zkApp verification keys of `accounts`, by the hash computed here, not
/// the one stored with the key. A proof is verified against the key whose
/// hash its account update names, as with keys set in blocks, so a key from a
/// ledger can only serve proofs that name it.
pub fn zkapp_keys_of_accounts<'a>(
    accounts: impl IntoIterator<Item = &'a Account>,
) -> HashMap<Fp, VerificationKey> {
    accounts
        .into_iter()
        .filter_map(|account| account.zkapp.as_ref()?.verification_key.as_ref())
        .map(|key| (key.vk().hash(), key.vk().clone()))
        .collect()
}

/// Which checks to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checks {
    pub block: bool,
    pub transactions: bool,
    pub zkapps: bool,
    /// The delta block chain proof and the body reference.
    pub header: bool,
}

impl Checks {
    pub const ALL: Checks = Checks {
        block: true,
        transactions: true,
        zkapps: true,
        header: true,
    };
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Only blocks carrying this genesis state hash are verified.
    pub genesis_state_hash: String,
    /// Verify a block's transaction proofs as one batch, and one by one only
    /// if it fails. Otherwise one by one, which costs about 20 times more.
    pub batch: bool,
    pub checks: Checks,
}

/// A precomputed block file, named `<height>-<state hash>.json[.zst]` as in
/// the replay's directory, or `<network>-<height>-<state hash>.json` as in the
/// public bucket.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockFile {
    pub height: u32,
    pub hash: String,
    pub path: PathBuf,
}

impl BlockFile {
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        let stem = name.strip_suffix(".zst").unwrap_or(name);
        let stem = stem.strip_suffix(".json")?;
        let mut parts = stem.rsplitn(3, '-');
        let hash = parts.next()?;
        let height = parts.next()?.parse().ok()?;
        Some(Self {
            height,
            hash: hash.to_string(),
            path: path.to_path_buf(),
        })
    }
}

/// The block files in a directory, by height then hash. Other files are
/// skipped.
pub fn list_blocks(dir: &Path) -> anyhow::Result<Vec<BlockFile>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("{}", dir.display()))? {
        if let Some(file) = BlockFile::from_path(&entry?.path()) {
            files.push(file);
        }
    }
    files.sort();
    Ok(files)
}

/// What checking one block file found.
#[derive(Clone, Debug)]
pub struct BlockResult {
    pub height: u32,
    /// The state hash in the file name.
    pub hash: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    /// A block of the chain being checked.
    Checked(Box<Checked>),
    /// A block of another chain, by its genesis state hash. Not verified.
    OtherChain {
        genesis_state_hash: String,
        protocol_version: String,
    },
    /// The file could not be read or decoded.
    Unreadable(String),
}

#[derive(Clone, Debug, Default)]
pub struct Checked {
    /// `blockchain_length` in the protocol state.
    pub height: u32,
    /// The protocol state hash we compute.
    pub state_hash: String,
    pub previous_state_hash: String,
    pub protocol_version: String,
    /// Whether the block proof verified; `None` if it wasn't checked.
    pub block_proof: Option<bool>,
    pub block_time: Duration,
    pub works: usize,
    /// Every transaction proof of the completed works, in order; empty if
    /// they weren't checked.
    pub proofs: Vec<ProofResult>,
    /// Whether the batch of the block's transaction proofs verified, in batch
    /// mode with at least one proof.
    pub batch: Option<bool>,
    pub tx_time: Duration,
    /// Verification keys the block's account updates set.
    pub zkapp_keys: Vec<VerificationKey>,
    /// The proofs of the proof-authorized account updates of the block's
    /// applied zkApp commands.
    pub zkapp_proofs: Vec<ZkappProof>,
    pub zkapp_time: Duration,
    /// Whether the delta block chain proof leads to the parent; `None` if it
    /// wasn't checked.
    pub delta_chain: Option<bool>,
    /// Whether the body hashes to the header's body reference; `None` if it
    /// wasn't checked.
    pub body_reference: Option<bool>,
    /// The time the node's body reference check took (not parsing the JSON).
    pub body_time: Duration,
}

#[derive(Clone, Debug)]
pub struct ProofResult {
    /// `<work>.<part>`: the completed work's index in the diff, and which of
    /// its one or two proofs.
    pub label: String,
    /// Wrap branch and step domain, like `N0/d15` (base) or `N2/d15` (merge).
    pub kind: String,
    pub verified: bool,
    /// Whether the statement's sok digest is the digest of the work's fee and
    /// prover.
    pub sok_matches: bool,
}

#[derive(Clone, Debug)]
pub struct ZkappProof {
    /// `<command>.<account update>`: the command's index among the diff's
    /// commands, and the account update's in the command, depth first.
    pub label: String,
    /// The hash of the verification key the account update names.
    pub vk_hash: Fp,
    pub statement: ZkappStatement,
    pub proof: SideLoadedProof,
    pub result: ZkappResult,
    /// Whether the key came from a ledger ([`Keys::ledger_zkapp`]) because no
    /// block of the run sets it.
    pub key_from_ledger: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZkappResult {
    /// Not verified yet: [`check_files`] verifies zkApp proofs last.
    Pending,
    Verified,
    Failed,
    /// Neither a block of the run nor a ledger given has a verification key
    /// of this hash.
    NoKey,
}

/// A field element in hex, `0x` then lower-case digits. OCaml's JSON writes
/// them in upper case.
pub fn hex(fp: Fp) -> String {
    serde_json::to_value(BigInt::from(fp))
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl Checked {
    /// Why the block fails, if it does: one line per reason.
    pub fn failures(&self, file_height: u32, file_hash: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.state_hash != file_hash {
            out.push(format!(
                "protocol state hashes to {}, not to the file name's hash",
                self.state_hash
            ));
        }
        if self.height != file_height {
            out.push(format!(
                "blockchain_length {} is not the file name's height",
                self.height
            ));
        }
        if self.block_proof == Some(false) {
            out.push("block proof does not verify".to_string());
        }
        if self.delta_chain == Some(false) {
            out.push("delta block chain proof does not lead to the parent".to_string());
        }
        if self.body_reference == Some(false) {
            out.push("the body does not hash to the header's body reference".to_string());
        }
        for proof in &self.proofs {
            if !proof.verified {
                out.push(format!(
                    "work {} ({}): transaction proof does not verify",
                    proof.label, proof.kind
                ));
            }
            if !proof.sok_matches {
                out.push(format!(
                    "work {}: sok digest is not the digest of the work's fee and prover",
                    proof.label
                ));
            }
        }
        for proof in &self.zkapp_proofs {
            let what = match proof.result {
                ZkappResult::Verified => continue,
                ZkappResult::Failed => "does not verify",
                ZkappResult::NoKey => {
                    "has no key: neither a block of the run nor a ledger given has one of its hash"
                }
                ZkappResult::Pending => "was not verified",
            };
            out.push(format!(
                "zkApp proof {} (key {}) {what}",
                proof.label,
                hex(proof.vk_hash)
            ));
        }
        out
    }
}

fn protocol_version(data: &Value) -> String {
    let version = &data["protocol_version"];
    match (
        version["transaction"].as_u64(),
        version["network"].as_u64(),
        version["patch"].as_u64(),
    ) {
        (Some(t), Some(n), Some(p)) => format!("{t}.{n}.{p}"),
        _ => "?".to_string(),
    }
}

/// The completed works of both parts of a block's staged ledger diff.
pub fn completed_works(data: &Value) -> anyhow::Result<Vec<TransactionSnarkWorkTStableV2>> {
    let mut works = Vec::new();
    let parts = data["staged_ledger_diff"]["diff"]
        .as_array()
        .context("no staged_ledger_diff.diff")?;
    for (i, part) in parts.iter().enumerate() {
        if part.is_null() {
            continue;
        }
        let list = &part["completed_works"];
        let part_works: Vec<TransactionSnarkWorkTStableV2> = serde_path_to_error::deserialize(list)
            .map_err(|e| {
                anyhow::anyhow!("diff part {i}, completed_works.{}: {}", e.path(), e.inner())
            })?;
        works.extend(part_works);
    }
    Ok(works)
}

/// One transaction proof of a completed work, with its statement.
struct Item {
    label: String,
    kind: String,
    statement: Statement<SokDigest>,
    proof: TransactionSnarkProofStableV2,
    sok_matches: bool,
}

/// The statement of every proof of every completed work, with whether its sok
/// digest is the digest of the work's fee and prover.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/staged_ledger/staged_ledger.ml#L736-L775>
/// pairs each work's proofs with `Sok_message.create ~fee ~prover`, and
/// `Transaction_snark.verify` rejects them all if any statement's sok digest
/// differs from its message's digest.
fn items(works: &[TransactionSnarkWorkTStableV2]) -> anyhow::Result<Vec<Item>> {
    let mut items = Vec::new();
    for (w, work) in works.iter().enumerate() {
        let digest = SokMessage::create((&work.fee).into(), (&work.prover).try_into()?).digest();
        let proofs = match &work.proofs {
            TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
            TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
        };
        for (part, proof) in proofs.into_iter().enumerate() {
            let statement = Statement::<SokDigest>::try_from(&proof.0.statement)?;
            let decoded = proof.0.proof.get()?;
            let branch = &decoded.0.statement.proof_state.deferred_values.branch_data;
            items.push(Item {
                label: format!("{w}.{part}"),
                kind: format!(
                    "{:?}/d{}",
                    branch.proofs_verified,
                    branch.domain_log2.as_u8()
                ),
                sok_matches: statement.sok_digest == digest,
                statement,
                proof: (*decoded).clone(),
            });
        }
    }
    Ok(items)
}

/// Whether any account update of the block is proof-authorized or sets a
/// verification key, before parsing its commands.
fn touches_zkapp_keys(data: &mut Value) -> anyhow::Result<bool> {
    let mut found = false;
    for_each_account_update(&mut data["staged_ledger_diff"], &mut |update| {
        let update = &update["account_update"];
        found |= update["authorization"][0] == "Proof"
            || update["body"]["update"]["verification_key"][0] == "Set";
        Ok(())
    })?;
    Ok(found)
}

/// The verification keys a block's account updates set, and the proofs of the
/// proof-authorized account updates of its applied zkApp commands, with their
/// statements.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/verifier/common.ml#L50-L82>
/// checks each proof against the account's key, which must have the hash the
/// account update names, and
/// <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/verifier/common.ml#L136-L152>
/// skips the proofs of failed commands.
fn zkapp_parts(
    diff: &StagedLedgerDiffDiffStableV2,
) -> anyhow::Result<(Vec<VerificationKey>, Vec<ZkappProof>)> {
    let diff: Diff = diff.try_into()?;
    let (mut keys, mut proofs) = (Vec::new(), Vec::new());
    for (c, command) in diff.commands().into_iter().enumerate() {
        let UserCommand::ZkAppCommand(zkapp) = command.data else {
            continue;
        };
        zkapp.account_updates.fold((), |(), update| {
            if let SetOrKeep::Set(key) = &update.body.update.verification_key {
                keys.push(key.vk().clone());
            }
        });
        if let TransactionStatus::Failed(_) = command.status {
            continue;
        }
        let forest = zkapp.account_updates.map_to(|update| (update.clone(), ()));
        // The statements are of the account updates' digests and stack hashes.
        forest.ensure_hashed();
        let updates = ZkappStatement::zkapp_statements_of_forest_prime(forest)
            .to_zkapp_command_with_hashes_list();
        for (u, ((update, ((), statement)), _)) in updates.into_iter().enumerate() {
            if let (Control::Proof(proof), AuthorizationKind::Proof(vk_hash)) =
                (&update.authorization, &update.body.authorization_kind)
            {
                proofs.push(ZkappProof {
                    label: format!("{c}.{u}"),
                    vk_hash: *vk_hash,
                    statement,
                    proof: proof.clone(),
                    result: ZkappResult::Pending,
                    key_from_ledger: false,
                });
            }
        }
    }
    Ok((keys, proofs))
}

/// Verify a block's proof and its completed works' proofs, as `checks` says.
/// zkApp proofs are not verified here: see [`verify_zkapp_proofs`].
pub fn check(
    header: &MinaBlockHeaderStableV2,
    works: &[TransactionSnarkWorkTStableV2],
    keys: &Keys,
    batch: bool,
    checks: Checks,
) -> anyhow::Result<Checked> {
    let state = &header.protocol_state;
    let state_hash = header
        .try_hash()
        .map_err(|e| anyhow::anyhow!("hashing the protocol state: {e:?}"))?;
    let srs = get_srs::<Fp>();

    let started = Instant::now();
    let block_proof = checks
        .block
        .then(|| verify_block(header, &keys.block, &srs));
    let block_time = started.elapsed();

    let items = if checks.transactions {
        items(works)?
    } else {
        Vec::new()
    };
    let started = Instant::now();
    let batch_ok = (batch && !items.is_empty()).then(|| {
        verify_transaction(
            items.iter().map(|i| (&i.statement, &i.proof)),
            &keys.transaction,
            &srs,
        )
    });
    let verified: Vec<bool> = if batch_ok == Some(true) {
        vec![true; items.len()]
    } else {
        items
            .par_iter()
            .map(|i| {
                verify_transaction(
                    std::iter::once((&i.statement, &i.proof)),
                    &keys.transaction,
                    &srs,
                )
            })
            .collect()
    };
    let tx_time = started.elapsed();

    Ok(Checked {
        height: state.body.consensus_state.blockchain_length.as_u32(),
        state_hash: state_hash.to_string(),
        previous_state_hash: state.previous_state_hash.to_string(),
        block_proof,
        block_time,
        works: works.len(),
        proofs: items
            .into_iter()
            .zip(verified)
            .map(|(i, verified)| ProofResult {
                label: i.label,
                kind: i.kind,
                verified,
                sok_matches: i.sok_matches,
            })
            .collect(),
        batch: batch_ok,
        tx_time,
        ..Default::default()
    })
}

/// Sort a block by its genesis state hash, then check it if it is of the
/// chain being checked, and collect its zkApp keys and proofs.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/mina_block/validation.ml#L289-L296>
/// rejects a block of another genesis before its proof is looked at.
pub fn check_json(mut data: Value, keys: &Keys, options: &Options) -> anyhow::Result<Outcome> {
    let mut header = mina_web_verifier::header_from_precomputed_value(&data)?;
    let protocol_version = protocol_version(&data);
    let genesis = header.protocol_state.body.genesis_state_hash.to_string();
    if genesis != options.genesis_state_hash {
        return Ok(Outcome::OtherChain {
            genesis_state_hash: genesis,
            protocol_version,
        });
    }
    let checks = options.checks;
    let works = completed_works(&data)?;
    let delta_chain = if checks.header {
        header.delta_block_chain_proof = delta_block_chain_proof(&data)?;
        Some(validate_delta_block_chain(&header).is_ok())
    } else {
        None
    };
    let zkapps = checks.zkapps && touches_zkapp_keys(&mut data)?;
    let mut body_reference = None;
    let mut body_time = Duration::ZERO;
    let (zkapp_keys, zkapp_proofs) = if checks.header || zkapps {
        let staged_ledger_diff = parse_block_data(data)?.staged_ledger_diff;
        let block = MinaBlockBlockStableV2 {
            header,
            body: StagedLedgerDiffBodyStableV1 { staged_ledger_diff },
        };
        if checks.header {
            let started = Instant::now();
            body_reference = Some(validate_body_reference(&block).is_ok());
            body_time = started.elapsed();
        }
        let parts = if zkapps {
            zkapp_parts(&block.body.staged_ledger_diff)?
        } else {
            Default::default()
        };
        header = block.header;
        parts
    } else {
        drop(data);
        Default::default()
    };
    let mut checked = check(&header, &works, keys, options.batch, checks)?;
    checked.protocol_version = protocol_version;
    checked.zkapp_keys = zkapp_keys;
    checked.zkapp_proofs = zkapp_proofs;
    checked.delta_chain = delta_chain;
    checked.body_reference = body_reference;
    checked.body_time = body_time;
    Ok(Outcome::Checked(Box::new(checked)))
}

/// `delta_transition_chain_proof` of a precomputed block. OCaml writes its
/// state hash and state body hashes as ledger hashes (`jw…`, `jx…`):
/// <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/mina_block/precomputed_block.ml#L57-L58>
pub fn delta_block_chain_proof(data: &Value) -> anyhow::Result<(StateHash, List<StateBodyHash>)> {
    let (init, body_hashes): (LedgerHash, Vec<LedgerHash>) =
        serde_json::from_value(data["delta_transition_chain_proof"].clone())
            .context("delta_transition_chain_proof")?;
    Ok((
        DataHashLibStateHashStableV1((*init).0.clone()).into(),
        body_hashes
            .iter()
            .map(|hash| MinaBaseStateBodyHashStableV1((**hash).0.clone()).into())
            .collect(),
    ))
}

pub fn check_file(file: &BlockFile, keys: &Keys, options: &Options) -> BlockResult {
    let outcome = read_block_json(&file.path)
        .and_then(|data| check_json(data, keys, options))
        .unwrap_or_else(|e| Outcome::Unreadable(format!("{e:#}")));
    BlockResult {
        height: file.height,
        hash: file.hash.clone(),
        outcome,
    }
}

/// How many zkApp proofs [`verify_zkapp_proofs`] has checked in this process,
/// for progress reports. They are checked after every block has been read,
/// and each costs seconds: `verify_zkapp` builds its key's verifier index,
/// with an SRS and its Lagrange basis, every time.
pub static ZKAPP_PROOFS_CHECKED: AtomicUsize = AtomicUsize::new(0);

/// Verify the zkApp proofs of checked blocks, each against the key of the hash
/// its account update names, from the keys the blocks set, or else from
/// `ledger_keys`. The hash of each key is computed here, not taken from the
/// block.
pub fn verify_zkapp_proofs(
    results: &mut [BlockResult],
    ledger_keys: &HashMap<Fp, VerificationKey>,
) {
    let keys: HashMap<Fp, VerificationKey> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Checked(c) => Some(c.zkapp_keys.iter()),
            _ => None,
        })
        .flatten()
        .map(|key| (key.hash(), key.clone()))
        .collect();
    let srs = get_srs::<Fp>();
    results.par_iter_mut().for_each(|r| {
        let Outcome::Checked(c) = &mut r.outcome else {
            return;
        };
        let started = Instant::now();
        c.zkapp_proofs.par_iter_mut().for_each(|p| {
            let from_blocks = keys.get(&p.vk_hash);
            let from_ledgers = ledger_keys.get(&p.vk_hash);
            p.key_from_ledger = from_blocks.is_none() && from_ledgers.is_some();
            p.result = match from_blocks.or(from_ledgers) {
                None => ZkappResult::NoKey,
                Some(key) => {
                    let Ok(decoded) = p.proof.get() else {
                        p.result = ZkappResult::Failed;
                        return;
                    };
                    let proof: PicklesProofProofsVerified2ReprStableV2 = (&*decoded).into();
                    if verify_zkapp(key, &p.statement, &proof, &srs) {
                        ZkappResult::Verified
                    } else {
                        ZkappResult::Failed
                    }
                }
            };
            ZKAPP_PROOFS_CHECKED.fetch_add(1, Ordering::Relaxed);
        });
        c.zkapp_time = started.elapsed();
    });
}

/// Check blocks in parallel, on the current rayon pool, then their zkApp
/// proofs. `done` is called once per block as its block and transaction
/// proofs are done.
pub fn check_files(
    files: &[BlockFile],
    keys: &Keys,
    options: &Options,
    done: impl Fn(&BlockResult) + Sync,
) -> Vec<BlockResult> {
    let mut results: Vec<BlockResult> = files
        .par_iter()
        .map(|file| {
            let result = check_file(file, keys, options);
            done(&result);
            result
        })
        .collect();
    if options.checks.zkapps {
        verify_zkapp_proofs(&mut results, &keys.ledger_zkapp);
    }
    results
}

/// Totals over a run.
#[derive(Debug, Default)]
pub struct Summary {
    pub genesis_state_hash: String,
    pub files: usize,
    /// Blocks of the chain being checked.
    pub checked: usize,
    pub heights: Option<(u32, u32)>,
    pub protocol_versions: BTreeMap<String, usize>,
    /// Blocks of other chains, by genesis state hash and protocol version.
    pub other_chains: BTreeMap<(String, String), Vec<(u32, String)>>,
    pub unreadable: Vec<(u32, String, String)>,
    pub block_proofs_verified: usize,
    pub block_proofs_failed: usize,
    pub block_time: Duration,
    pub blocks_with_work: usize,
    pub works: usize,
    pub proofs_verified: usize,
    pub proofs_failed: usize,
    pub sok_mismatches: usize,
    /// Proofs verified and failed by kind.
    pub kinds: BTreeMap<String, (usize, usize)>,
    pub batches: usize,
    pub batches_failed: usize,
    pub tx_time: Duration,
    /// Verification keys set, and how many distinct ones.
    pub zkapp_keys: usize,
    pub zkapp_distinct_keys: usize,
    pub blocks_with_zkapp_proofs: usize,
    pub zkapp_proofs_verified: usize,
    /// Failed, without a key, or not verified.
    pub zkapp_proofs_failed: usize,
    /// Proofs whose key came from a ledger, and how many distinct keys.
    pub zkapp_proofs_ledger_key: usize,
    pub zkapp_ledger_keys_used: usize,
    pub zkapp_time: Duration,
    /// Delta block chain proofs and body references that matched and didn't.
    pub delta_chains_ok: usize,
    pub delta_chains_failed: usize,
    pub body_references_ok: usize,
    pub body_references_failed: usize,
    /// The body reference checks' total and longest time, and the height of
    /// the block it was longest for.
    pub body_time: Duration,
    pub body_time_max: (Duration, u32),
    /// Checked blocks whose parent is neither the genesis nor another checked
    /// block: the chain is contiguous from the genesis when there are none.
    pub without_parent: Vec<(u32, String)>,
    /// `(height, hash, reason)`, one per reason.
    pub failures: Vec<(u32, String, String)>,
}

impl Summary {
    pub fn new(genesis_state_hash: &str, results: &[BlockResult]) -> Self {
        let mut s = Summary {
            genesis_state_hash: genesis_state_hash.to_string(),
            files: results.len(),
            ..Default::default()
        };
        let hashes: HashSet<&str> = results
            .iter()
            .filter_map(|r| match &r.outcome {
                Outcome::Checked(c) => Some(c.state_hash.as_str()),
                _ => None,
            })
            .collect();
        let mut keys = HashSet::new();
        let mut ledger_keys = HashSet::new();
        for r in results {
            match &r.outcome {
                Outcome::Unreadable(e) => s.unreadable.push((r.height, r.hash.clone(), e.clone())),
                Outcome::OtherChain {
                    genesis_state_hash,
                    protocol_version,
                } => s
                    .other_chains
                    .entry((genesis_state_hash.clone(), protocol_version.clone()))
                    .or_default()
                    .push((r.height, r.hash.clone())),
                Outcome::Checked(c) => {
                    s.checked += 1;
                    s.heights = Some(match s.heights {
                        None => (r.height, r.height),
                        Some((lo, hi)) => (lo.min(r.height), hi.max(r.height)),
                    });
                    *s.protocol_versions
                        .entry(c.protocol_version.clone())
                        .or_default() += 1;
                    match c.block_proof {
                        Some(true) => s.block_proofs_verified += 1,
                        Some(false) => s.block_proofs_failed += 1,
                        None => {}
                    }
                    s.block_time += c.block_time;
                    match c.delta_chain {
                        Some(true) => s.delta_chains_ok += 1,
                        Some(false) => s.delta_chains_failed += 1,
                        None => {}
                    }
                    match c.body_reference {
                        Some(true) => s.body_references_ok += 1,
                        Some(false) => s.body_references_failed += 1,
                        None => {}
                    }
                    s.body_time += c.body_time;
                    if c.body_time > s.body_time_max.0 {
                        s.body_time_max = (c.body_time, c.height);
                    }
                    if c.works > 0 {
                        s.blocks_with_work += 1;
                    }
                    s.works += c.works;
                    for p in &c.proofs {
                        let kind = s.kinds.entry(p.kind.clone()).or_default();
                        if p.verified {
                            s.proofs_verified += 1;
                            kind.0 += 1;
                        } else {
                            s.proofs_failed += 1;
                            kind.1 += 1;
                        }
                        if !p.sok_matches {
                            s.sok_mismatches += 1;
                        }
                    }
                    if let Some(ok) = c.batch {
                        s.batches += 1;
                        if !ok {
                            s.batches_failed += 1;
                        }
                    }
                    s.tx_time += c.tx_time;
                    s.zkapp_keys += c.zkapp_keys.len();
                    keys.extend(c.zkapp_keys.iter().map(|k| k.hash()));
                    if !c.zkapp_proofs.is_empty() {
                        s.blocks_with_zkapp_proofs += 1;
                    }
                    for p in &c.zkapp_proofs {
                        if p.result == ZkappResult::Verified {
                            s.zkapp_proofs_verified += 1;
                        } else {
                            s.zkapp_proofs_failed += 1;
                        }
                        if p.key_from_ledger {
                            s.zkapp_proofs_ledger_key += 1;
                            ledger_keys.insert(p.vk_hash);
                        }
                    }
                    s.zkapp_time += c.zkapp_time;
                    if c.previous_state_hash != genesis_state_hash
                        && !hashes.contains(c.previous_state_hash.as_str())
                    {
                        s.without_parent.push((r.height, r.hash.clone()));
                    }
                    for reason in c.failures(r.height, &r.hash) {
                        s.failures.push((r.height, r.hash.clone(), reason));
                    }
                }
            }
        }
        s.zkapp_distinct_keys = keys.len();
        s.zkapp_ledger_keys_used = ledger_keys.len();
        s
    }

    /// Every block of the chain verified, and every file could be read.
    pub fn ok(&self) -> bool {
        self.failures.is_empty() && self.unreadable.is_empty()
    }
}

fn per(total: Duration, n: usize) -> String {
    if n == 0 {
        "-".to_string()
    } else {
        format!("{:.1?}", total / n as u32)
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "files: {}", self.files)?;
        let (lo, hi) = self.heights.unwrap_or_default();
        writeln!(
            f,
            "chain of genesis {}: {} blocks, heights {lo} to {hi}, protocol versions {:?}",
            self.genesis_state_hash, self.checked, self.protocol_versions
        )?;
        let other: usize = self.other_chains.values().map(Vec::len).sum();
        writeln!(f, "other chains, excluded: {other} blocks")?;
        for ((genesis, version), blocks) in &self.other_chains {
            let (lo, hi) = (blocks[0].0, blocks[blocks.len() - 1].0);
            writeln!(
                f,
                "  genesis {genesis}, protocol {version}: {} blocks, heights {lo} to {hi}",
                blocks.len()
            )?;
        }
        writeln!(
            f,
            "header checks: delta block chain proofs {} ok, {} failed; body references {} ok, \
             {} failed, {:.0?} in all, {} per block, at most {:.1?} (at {})",
            self.delta_chains_ok,
            self.delta_chains_failed,
            self.body_references_ok,
            self.body_references_failed,
            self.body_time,
            per(
                self.body_time,
                self.body_references_ok + self.body_references_failed
            ),
            self.body_time_max.0,
            self.body_time_max.1
        )?;
        let blocks = self.block_proofs_verified + self.block_proofs_failed;
        writeln!(
            f,
            "block proofs: {} verified, {} failed, {} not checked; {:.0?} in all, {} per block",
            self.block_proofs_verified,
            self.block_proofs_failed,
            self.checked - blocks,
            self.block_time,
            per(self.block_time, blocks)
        )?;
        let proofs = self.proofs_verified + self.proofs_failed;
        writeln!(
            f,
            "transaction proofs: {proofs} in {} works of {} blocks: {} verified, {} failed, \
             {} sok digest mismatches; {}/{} batches failed; {:.0?} in all, {} per proof",
            self.works,
            self.blocks_with_work,
            self.proofs_verified,
            self.proofs_failed,
            self.sok_mismatches,
            self.batches_failed,
            self.batches,
            self.tx_time,
            per(self.tx_time, proofs)
        )?;
        for (kind, (ok, failed)) in &self.kinds {
            writeln!(f, "  {kind}: {ok} verified, {failed} failed")?;
        }
        let zkapp_proofs = self.zkapp_proofs_verified + self.zkapp_proofs_failed;
        writeln!(
            f,
            "zkApp proofs: {zkapp_proofs} in {} blocks: {} verified, {} failed; {} verification \
             keys set, {} distinct; {} proofs with {} keys from ledgers; {:.0?} in all",
            self.blocks_with_zkapp_proofs,
            self.zkapp_proofs_verified,
            self.zkapp_proofs_failed,
            self.zkapp_keys,
            self.zkapp_distinct_keys,
            self.zkapp_proofs_ledger_key,
            self.zkapp_ledger_keys_used,
            self.zkapp_time
        )?;
        writeln!(
            f,
            "blocks whose parent is neither the genesis nor a checked block: {}",
            self.without_parent.len()
        )?;
        for (height, hash) in self.without_parent.iter().take(20) {
            writeln!(f, "  {height} {hash}")?;
        }
        writeln!(f, "unreadable files: {}", self.unreadable.len())?;
        for (height, hash, error) in &self.unreadable {
            writeln!(f, "  {height} {hash}: {error}")?;
        }
        writeln!(f, "failures: {}", self.failures.len())?;
        for (height, hash, reason) in &self.failures {
            writeln!(f, "  {height} {hash}: {reason}")?;
        }
        Ok(())
    }
}
