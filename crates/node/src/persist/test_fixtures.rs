//! Fixtures shared by the frontier store's tests and the ledger thread's:
//! random ledgers and mask chains, real mainnet blocks, and a full ledger
//! streamed from a mask on the calling thread.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use ledger::{Account, AccountIndex, BaseLedger, Mask};
use mina_core::block::{ArcBlockWithHash, BlockWithHash};
use mina_p2p_messages::{
    binprot::BinProtRead, rpc::GetBestTipV2Response, rpc_kernel::ResponsePayload, v2,
};

use super::{
    ledger_codec::{encode_full_part, full_header, CodecError, FullLedgerSource, FullPart},
    LedgerFileHeader,
};

pub const DEPTH: usize = 35;

/// A ledger of `num_accounts` random accounts (zkApp accounts among them).
pub fn ledger_with(num_accounts: usize) -> Mask {
    let mut ledger = Mask::create(DEPTH);
    append(&mut ledger, num_accounts);
    ledger
}

pub fn append(ledger: &mut Mask, num_accounts: usize) {
    for _ in 0..num_accounts {
        let account = Account::rand();
        ledger.get_or_create_account(account.id(), account).unwrap();
    }
}

/// Increments the nonces of the accounts at `changed`.
pub fn change(ledger: &mut Mask, changed: &[u64]) {
    for &index in changed {
        let mut account = ledger.get_at_index(AccountIndex(index)).unwrap();
        account.nonce = account.nonce.incr();
        ledger.set_at_index(AccountIndex(index), account).unwrap();
    }
}

/// A child mask of `parent`, with the accounts at `changed` changed and
/// `appended` new ones.
pub fn child(parent: &Mask, changed: &[u64], appended: usize) -> Mask {
    let mut mask = parent.make_child();
    change(&mut mask, changed);
    append(&mut mask, appended);
    mask
}

pub fn hash_of(ledger: &Mask) -> v2::LedgerHash {
    v2::LedgerHash::from_fp(ledger.clone().merkle_root())
}

/// Two Mesa mainnet blocks captured from OCaml 4.0.2 nodes: 563978 (a gossip
/// message) and 564003 (a best tip answer).
pub fn mainnet_blocks() -> Vec<ArcBlockWithHash> {
    let gossip = include_bytes!("../../../web-verifier/tests/fixtures/mainnet-563978.gossipbin");
    let gossip = v2::MinaBlockBlockStableV2::binprot_read(&mut &gossip[9..]).unwrap();
    let best_tip = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
    let best_tip = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &best_tip[..])
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap()
        .data;
    [gossip, best_tip]
        .into_iter()
        .map(|block| BlockWithHash::try_new(Arc::new(block)).unwrap())
        .collect()
}

/// Slots between two blocks of the synthetic chains.
pub const SLOTS_PER_BLOCK: u32 = 2;

/// The global slot of a chain's root.
pub const ROOT_SLOT: u32 = 1_000;

/// The ledger hashes a synthetic chain's blocks carry.
#[derive(Clone)]
pub struct ChainLedgers {
    pub staking: v2::LedgerHash,
    pub next: v2::LedgerHash,
    pub snarked: v2::LedgerHash,
    /// The root's complete staged ledger hash.
    pub staged: v2::MinaBaseStagedLedgerHashStableV1,
}

/// A copy of a mainnet block at `height` and `slot`, after `parent`, with
/// these ledger hashes: it decodes and hashes as a real block does, which is
/// what the loader checks (I5d1). Its proof and body are the template's.
pub fn block_after(
    parent: v2::StateHash,
    height: u32,
    slot: u32,
    ledgers: &ChainLedgers,
) -> ArcBlockWithHash {
    static TEMPLATE: std::sync::OnceLock<ArcBlockWithHash> = std::sync::OnceLock::new();
    let template = TEMPLATE.get_or_init(|| mainnet_blocks().swap_remove(0));
    let mut block = (*template.block).clone();
    // With delta 0 (mainnet), the delta block chain proof is the parent's
    // hash and no body hashes.
    block.header.delta_block_chain_proof.0 = parent.clone();
    let state = &mut block.header.protocol_state;
    state.previous_state_hash = parent;
    let consensus = &mut state.body.consensus_state;
    consensus.blockchain_length = height.into();
    consensus.curr_global_slot_since_hard_fork.slot_number =
        v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(slot.into());
    consensus.global_slot_since_genesis =
        v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(slot.into());
    consensus.staking_epoch_data.ledger.hash = ledgers.staking.clone();
    consensus.next_epoch_data.ledger.hash = ledgers.next.clone();
    let blockchain = &mut state.body.blockchain_state;
    blockchain.ledger_proof_statement.target.first_pass_ledger = ledgers.snarked.clone();
    blockchain.staged_ledger_hash = ledgers.staged.clone();
    ArcBlockWithHash::try_new(Arc::new(block)).unwrap()
}

/// A chain of `len` linked blocks from height 100 and slot [`ROOT_SLOT`],
/// after `parent`, all carrying these ledger hashes.
pub fn synthetic_chain(
    first_parent: v2::StateHash,
    len: usize,
    ledgers: &ChainLedgers,
) -> Vec<ArcBlockWithHash> {
    let mut blocks: Vec<ArcBlockWithHash> = Vec::with_capacity(len);
    for i in 0..len as u32 {
        let parent = blocks
            .last()
            .map_or_else(|| first_parent.clone(), |block| block.hash().clone());
        blocks.push(block_after(
            parent,
            100 + i,
            ROOT_SLOT + SLOTS_PER_BLOCK * i,
            ledgers,
        ));
    }
    blocks
}

/// A full ledger read a piece at a time from a mask, on the thread that asks:
/// the store thread's view of a pinned ledger, without the ledger thread.
#[derive(Debug)]
pub struct MaskSource {
    mask: Mask,
    header: LedgerFileHeader,
    part_sizes: (u64, u64),
    /// Pieces read so far.
    pub parts: AtomicU32,
    /// The piece (counted from 0) whose read fails, as a released pin's does.
    pub fail_at: Option<u32>,
    /// Whether that read fails as one does once the ledger thread has stopped.
    pub gone: bool,
}

impl MaskSource {
    pub fn new(mask: &Mask, accounts_per_part: u64, hashes_per_part: u64) -> Self {
        Self {
            mask: mask.clone(),
            header: full_header(mask).unwrap(),
            part_sizes: (accounts_per_part, hashes_per_part),
            parts: AtomicU32::new(0),
            fail_at: None,
            gone: false,
        }
    }

    pub fn failing_at(mut self, part: u32) -> Self {
        self.fail_at = Some(part);
        self
    }

    /// Fails at that piece as if the ledger thread had stopped.
    pub fn gone_at(mut self, part: u32) -> Self {
        self.fail_at = Some(part);
        self.gone = true;
        self
    }
}

impl FullLedgerSource for MaskSource {
    fn header(&self) -> &LedgerFileHeader {
        &self.header
    }

    fn read_part(&self, part: FullPart) -> Result<Vec<u8>, CodecError> {
        let index = self.parts.fetch_add(1, Ordering::SeqCst);
        if self.fail_at == Some(index) {
            return Err(if self.gone {
                CodecError::SourceGone("ledger thread stopped (request channel closed)".into())
            } else {
                CodecError::Invalid("ledger pin 1 was released".into())
            });
        }
        let mut bytes = Vec::new();
        encode_full_part(&self.mask, &self.header, part, &mut bytes)?;
        Ok(bytes)
    }

    fn part_sizes(&self) -> (u64, u64) {
        self.part_sizes
    }
}
