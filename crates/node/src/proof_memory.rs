//! PC1: where the node keeps SNARK proofs, and how many bytes they are.
//!
//! The report counts proofs by holder. A proof is identified by its address,
//! so one shared through an `Arc` is counted once, by the first holder that
//! reaches it; later holders count it as a reference only. Byte figures are
//! binprot lengths, exact and cheap to get. A decoded proof takes about 1.25
//! times its binprot bytes on the heap, in about 90 allocations
//! (`crates/core/tests/pc1_proof_sizes.rs`). The report is for diagnostics:
//! it walks every proof, so it takes milliseconds to seconds and is served
//! only on request (`/stats/memory/proofs`).

use std::collections::HashMap;

use ledger::scan_state::{
    scan_state::{transaction_snark::TransactionWithWitness, AvailableJobMessage},
    transaction_logic::{
        valid,
        zkapp_command::{Control, ZkAppCommand},
        Transaction, UserCommand,
    },
};
use mina_core::{block::ArcBlockWithHash, snark::SnarkProofs};
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{
        self, MinaBaseControlStableV2, MinaBaseUserCommandStableV2,
        MinaBaseZkappCommandTStableV1WireStableV1,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
        TransactionSnarkWorkTStableV2Proofs,
    },
};
use serde::{Deserialize, Serialize};

use crate::State;

/// Proofs reached through one holder.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ProofTally {
    /// References visited, shared ones included.
    pub refs: u64,
    /// Proofs among them that no earlier holder had.
    pub unique: u64,
    /// The binprot bytes of those unique proofs.
    pub binprot_bytes: u64,
    /// Cached references (including shared ones), whose decoded heap is released.
    pub cached_refs: u64,
    pub cached_binprot_bytes: u64,
}

impl ProofTally {
    pub fn add(&mut self, other: &ProofTally) {
        self.refs += other.refs;
        self.unique += other.unique;
        self.binprot_bytes += other.binprot_bytes;
        self.cached_refs += other.cached_refs;
        self.cached_binprot_bytes += other.cached_binprot_bytes;
    }
}

/// The proofs seen so far, by address, with their binprot bytes.
#[derive(Default)]
pub struct ProofsSeen(HashMap<usize, u64>);

impl ProofsSeen {
    pub fn tally<T: BinProtWrite>(&mut self, tally: &mut ProofTally, proof: &T) {
        tally.refs += 1;
        let address = proof as *const T as usize;
        if !self.0.contains_key(&address) {
            let bytes = binprot_len(proof);
            self.0.insert(address, bytes);
            tally.unique += 1;
            tally.binprot_bytes += bytes;
        }
    }

    pub fn tally_ref<T: BinProtWrite>(
        &mut self,
        tally: &mut ProofTally,
        proof: &mina_p2p_messages::ProofRef<T>,
    ) {
        tally.refs += 1;
        let address = proof.as_ptr() as usize;
        if let Some(blob) = proof.cached_blob() {
            tally.cached_refs += 1;
            tally.cached_binprot_bytes += blob.len();
            self.0.entry(address).or_insert(0);
        } else if let std::collections::hash_map::Entry::Vacant(e) = self.0.entry(address) {
            let bytes = binprot_len(proof);
            e.insert(bytes);
            tally.unique += 1;
            tally.binprot_bytes += bytes;
        }
    }

    pub fn contains<T>(&self, proof: &T) -> bool {
        self.0.contains_key(&(proof as *const T as usize))
    }

    pub fn addresses(&self) -> &HashMap<usize, u64> {
        &self.0
    }

    pub fn into_addresses(self) -> HashMap<usize, u64> {
        self.0
    }
}

/// The binprot length of a value, without allocating it.
pub fn binprot_len<T: BinProtWrite>(value: &T) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = value.binprot_write(&mut count);
    count.0
}

/// Proofs in a set of blocks.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BlocksProofMemory {
    /// Blocks not already counted by an earlier holder.
    pub blocks: u64,
    /// Their whole binprot bytes, proofs included.
    pub block_binprot_bytes: u64,
    /// Protocol state proofs.
    pub block_proofs: ProofTally,
    pub completed_works: u64,
    /// Transaction SNARK proofs of the completed works.
    pub work_proofs: ProofTally,
    /// Proofs authorizing zkApp account updates in the commands.
    pub zkapp_proofs: ProofTally,
}

impl BlocksProofMemory {
    fn add_block(&mut self, seen: &mut ProofsSeen, block: &ArcBlockWithHash) {
        let whole: &v2::MinaBlockBlockStableV2 = &block.block;
        if seen.contains(whole) {
            return;
        }
        let mut whole_tally = ProofTally::default();
        seen.tally(&mut whole_tally, whole);
        self.blocks += 1;
        self.block_binprot_bytes += whole_tally.binprot_bytes;
        seen.tally(
            &mut self.block_proofs,
            &*block.header().protocol_state_proof,
        );
        for work in block.completed_works_iter() {
            self.completed_works += 1;
            for proof in work_proofs(&work.proofs) {
                seen.tally_ref(&mut self.work_proofs, proof);
            }
        }
        for command in block.commands_iter() {
            if let MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = &command.data {
                wire_zkapp_proofs(seen, &mut self.zkapp_proofs, zkapp);
            }
        }
    }
}

/// The transaction SNARK proofs of a work, by reference.
pub fn work_proofs(
    proofs: &TransactionSnarkWorkTStableV2Proofs,
) -> impl Iterator<Item = &mina_p2p_messages::ProofRef<v2::TransactionSnarkProofStableV2>> {
    let (first, second) = match proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => (p, None),
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => (p, Some(q)),
    };
    std::iter::once(first).chain(second).map(|p| &p.0.proof)
}

fn wire_zkapp_proofs(
    seen: &mut ProofsSeen,
    tally: &mut ProofTally,
    zkapp: &MinaBaseZkappCommandTStableV1WireStableV1,
) {
    fn walk(
        seen: &mut ProofsSeen,
        tally: &mut ProofTally,
        node: &MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    ) {
        if let MinaBaseControlStableV2::Proof(proof) = &node.account_update.authorization {
            seen.tally_ref(tally, proof);
        }
        for call in node.calls.iter() {
            walk(seen, tally, &call.elt);
        }
    }
    for update in zkapp.account_updates.iter() {
        walk(seen, tally, &update.elt);
    }
}

fn zkapp_proofs(seen: &mut ProofsSeen, tally: &mut ProofTally, zkapp: &ZkAppCommand) {
    zkapp.account_updates.fold((), |(), update| {
        if let Control::Proof(proof) = &update.authorization {
            seen.tally_ref(tally, proof);
        }
    });
}

/// The zkApp proofs of a scan-state base job.
pub fn base_job_zkapp_proofs(
    seen: &mut ProofsSeen,
    tally: &mut ProofTally,
    job: &TransactionWithWitness,
) {
    if let Transaction::Command(UserCommand::ZkAppCommand(zkapp)) =
        &job.transaction_with_status.data
    {
        zkapp_proofs(seen, tally, zkapp);
    }
}

/// Proofs kept in the proof cache (PC1) rather than decoded.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CachedProofTally {
    pub blobs: u64,
    /// Bytes in files, off the heap, and bytes kept in memory.
    pub file_bytes: u64,
    pub memory_bytes: u64,
}

/// Tallies a work's proofs: decoded ones in `tally`, cached ones in `cached`.
pub fn tally_snark_proofs(
    seen: &mut ProofsSeen,
    tally: &mut ProofTally,
    cached: &mut CachedProofTally,
    proofs: &SnarkProofs,
) {
    if let Some(decoded) = proofs.decoded() {
        for proof in work_proofs(decoded) {
            seen.tally_ref(tally, proof);
        }
    }
    for blob in proofs.cached_blobs() {
        tally.refs += 1;
        if seen.0.insert(blob.as_ptr() as usize, 0).is_none() {
            cached.blobs += 1;
            if blob.is_file() {
                cached.file_bytes += blob.len();
            } else {
                cached.memory_bytes += blob.len();
            }
        }
    }
}

/// Proofs the snark pool holds.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SnarkPoolProofMemory {
    pub jobs: u64,
    /// The input proofs of the merge jobs the pool lists.
    pub job_merge_proofs: ProofTally,
    pub job_bases: u64,
    /// The base jobs' binprot bytes (transaction and ledger witnesses).
    pub job_base_binprot_bytes: u64,
    pub job_base_zkapp_proofs: ProofTally,
    /// Completed works offered for the jobs: decoded proofs, and cached ones.
    pub works: u64,
    pub work_proofs: ProofTally,
    pub work_cached: CachedProofTally,
    /// Works received from peers and not yet added or dropped.
    pub candidate_works: u64,
    pub candidate_proofs: ProofTally,
    pub candidate_cached: CachedProofTally,
}

/// Proofs the node's state holds, by holder.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NodeProofMemory {
    pub best_chain: BlocksProofMemory,
    /// Retired roots the frontier keeps, past the best chain.
    pub root_history: BlocksProofMemory,
    pub snark_pool: SnarkPoolProofMemory,
    pub transaction_pool_zkapp_proofs: ProofTally,
}

/// Proofs the ledger thread holds, by holder.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LedgerProofMemory {
    pub staged_ledgers: u64,
    pub historical_roots: u64,
    /// Distinct merge-job values (`Arc<LedgerProof>`) in the scan states,
    /// and how many times the scan states hold them.
    pub merge_jobs: u64,
    pub merge_job_refs: u64,
    /// Transaction SNARK proofs in the staged ledgers' scan states.
    pub scan_state_proofs: ProofTally,
    /// More in the scan states of retired roots.
    pub historical_root_proofs: ProofTally,
    /// Distinct base jobs (`Arc<TransactionWithWitness>`), and how many
    /// times the scan states hold them.
    pub base_jobs: u64,
    pub base_job_refs: u64,
    pub base_job_zkapp_proofs: ProofTally,
    /// Snarked ledgers (root, epoch ledgers, ledgers being synced) and their
    /// accounts.
    pub snarked_ledgers: Vec<(String, u64)>,
    /// The proofs' addresses and binprot bytes, to find those the node's
    /// state shares.
    #[serde(skip)]
    pub addresses: HashMap<usize, u64>,
}

/// Proofs held both in the node's state and on the ledger thread: one copy
/// if shared, two otherwise.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SharedProofs {
    pub best_chain_work_proofs: u64,
    pub best_chain_zkapp_proofs: u64,
    pub snark_pool_job_merge_proofs: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ProofMemoryReport {
    pub node: NodeProofMemory,
    pub ledger: LedgerProofMemory,
    pub shared_with_ledger: SharedProofs,
    /// All unique decoded proofs, and their binprot bytes.
    pub total: ProofTally,
    /// The node's proof cache.
    pub proof_cache: mina_core::snark::proof_cache::ProofStoreStats,
    pub elapsed_ms: f64,
}

impl ProofMemoryReport {
    pub fn new(state: &State, ledger: LedgerProofMemory, started: std::time::Instant) -> Self {
        let mut seen = ProofsSeen::default();
        let mut node = NodeProofMemory::default();
        let frontier = &state.transition_frontier;

        // Count the best chain alone first, to find the proofs it shares.
        for block in &frontier.best_chain {
            node.best_chain.add_block(&mut seen, block);
        }
        let mut shared = SharedProofs::default();
        for block in &frontier.best_chain {
            for work in block.completed_works_iter() {
                shared.best_chain_work_proofs += work_proofs(&work.proofs)
                    .filter(|p| ledger.addresses.contains_key(&(p.as_ptr() as usize)))
                    .count() as u64;
            }
            let mut zkapps = ProofsSeen::default();
            let mut tally = ProofTally::default();
            for command in block.commands_iter() {
                if let MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = &command.data {
                    wire_zkapp_proofs(&mut zkapps, &mut tally, zkapp);
                }
            }
            shared.best_chain_zkapp_proofs += zkapps
                .addresses()
                .keys()
                .filter(|a| ledger.addresses.contains_key(a))
                .count() as u64;
        }
        for block in frontier.root_history.iter() {
            node.root_history.add_block(&mut seen, block);
        }

        let pool = &mut node.snark_pool;
        for job in state.snark_pool.jobs_iter() {
            pool.jobs += 1;
            for message in job.job.iter() {
                match message {
                    AvailableJobMessage::Base(base) => {
                        pool.job_bases += 1;
                        pool.job_base_binprot_bytes += binprot_len(base);
                        if let v2::MinaTransactionTransactionStableV2::Command(command) =
                            &base.transaction_with_status.data
                        {
                            if let MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = &**command {
                                wire_zkapp_proofs(
                                    &mut seen,
                                    &mut pool.job_base_zkapp_proofs,
                                    zkapp,
                                );
                            }
                        }
                    }
                    AvailableJobMessage::Merge { left, right } => {
                        for proof in [&left.0.proof, &right.0.proof] {
                            if ledger.addresses.contains_key(&(proof.as_ptr() as usize)) {
                                shared.snark_pool_job_merge_proofs += 1;
                            }
                            seen.tally_ref(&mut pool.job_merge_proofs, proof);
                        }
                    }
                }
            }
            if let Some(snark) = &job.snark {
                pool.works += 1;
                tally_snark_proofs(
                    &mut seen,
                    &mut pool.work_proofs,
                    &mut pool.work_cached,
                    &snark.work.proofs,
                );
            }
        }
        for work in state.snark_pool.candidates.works_iter() {
            pool.candidate_works += 1;
            tally_snark_proofs(
                &mut seen,
                &mut pool.candidate_proofs,
                &mut pool.candidate_cached,
                &work.proofs,
            );
        }

        for command in state.transaction_pool.get_all_transactions() {
            if let valid::UserCommand::ZkAppCommand(zkapp) = &command.data {
                zkapp_proofs(
                    &mut seen,
                    &mut node.transaction_pool_zkapp_proofs,
                    &zkapp.zkapp_command,
                );
            }
        }

        let mut total = ProofTally::default();
        for tally in [
            &node.best_chain.block_proofs,
            &node.best_chain.work_proofs,
            &node.best_chain.zkapp_proofs,
            &node.root_history.block_proofs,
            &node.root_history.work_proofs,
            &node.root_history.zkapp_proofs,
            &node.snark_pool.job_merge_proofs,
            &node.snark_pool.job_base_zkapp_proofs,
            &node.snark_pool.work_proofs,
            &node.snark_pool.candidate_proofs,
            &node.transaction_pool_zkapp_proofs,
        ] {
            total.add(tally);
        }
        // The ledger's proofs, less those the node's state shares.
        for (address, bytes) in &ledger.addresses {
            total.refs += 1;
            if *bytes > 0 && !seen.addresses().contains_key(address) {
                total.unique += 1;
                total.binprot_bytes += bytes;
            }
        }

        Self {
            node,
            ledger,
            shared_with_ledger: shared,
            total,
            proof_cache: mina_core::snark::proof_cache::global().stats(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }
}
