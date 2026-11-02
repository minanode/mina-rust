use std::sync::Arc;

use ledger::AccountIndex;
use mina_node::{
    account::AccountPublicKey,
    block_producer::{
        vrf_evaluator::{DelegatorTable, VrfEvaluationOutputWithHash, VrfEvaluatorInput},
        BlockProducerEvent, BlockProducerVrfEvaluatorEvent,
    },
    core::channels::mpsc::{TrackedUnboundedReceiver, UnboundedSender},
    event_source::Event,
};
use mina_p2p_messages::v2::ConsensusVrfOutputTruncatedStableV1;
use mina_signer::Keypair;
use mina_vrf::{Threshold, VrfEvaluationOutput, VrfWonSlot};
#[cfg(not(target_family = "wasm"))]
use rayon::prelude::*;

#[cfg(not(target_family = "wasm"))]
use super::vrf_cache::VrfCache;
use crate::NodeService;

pub fn vrf_evaluator(
    event_sender: UnboundedSender<Event>,
    mut vrf_evaluation_receiver: TrackedUnboundedReceiver<VrfEvaluatorInput>,
    keypair: Keypair,
) {
    let mut worker = VrfEvaluatorWorker::new(keypair, None);
    while let Some(vrf_evaluator_input) = vrf_evaluation_receiver.blocking_recv() {
        // The node sets its work directory after starting this thread: look
        // it up with the first request.
        #[cfg(not(target_family = "wasm"))]
        if worker.work_dir.is_none() {
            worker.work_dir = Some(mina_core::get_work_dir());
        }
        let vrf_result_with_hash = worker.evaluate(&vrf_evaluator_input);

        // send the result back to the state machine
        let _ = event_sender.send(
            BlockProducerEvent::VrfEvaluator(BlockProducerVrfEvaluatorEvent::Evaluated(
                vrf_result_with_hash,
            ))
            .into(),
        );
    }
}

/// The VRF evaluator service's work for one request at a time: a result from
/// the disk cache when it holds one for the same key, seed, ledger, total
/// currency and delegator table, else an evaluation, recorded in the cache.
/// The response carries the request's ledger and generation.
pub struct VrfEvaluatorWorker {
    keypair: Keypair,
    /// Where the disk cache is kept; none without one.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    work_dir: Option<std::path::PathBuf>,
    prepared: Option<(Arc<DelegatorTable>, u64, Vec<PreparedDelegator>)>,
    #[cfg(not(target_family = "wasm"))]
    table_digest: Option<(Arc<DelegatorTable>, String)>,
    #[cfg(not(target_family = "wasm"))]
    cache: Option<VrfCache>,
    cache_hits: u64,
}

/// Whether two requests use the same table: usually the same allocation.
fn same_table(a: &Arc<DelegatorTable>, b: &Arc<DelegatorTable>) -> bool {
    Arc::ptr_eq(a, b) || **a == **b
}

impl VrfEvaluatorWorker {
    pub fn new(keypair: Keypair, work_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            keypair,
            work_dir,
            prepared: None,
            #[cfg(not(target_family = "wasm"))]
            table_digest: None,
            #[cfg(not(target_family = "wasm"))]
            cache: None,
            cache_hits: 0,
        }
    }

    /// How many results came from the disk cache.
    pub fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    pub fn evaluate(&mut self, input: &VrfEvaluatorInput) -> VrfEvaluationOutputWithHash {
        #[cfg(not(target_family = "wasm"))]
        let cached = self.cached(input);
        #[cfg(target_family = "wasm")]
        let cached: Option<VrfEvaluationOutput> = None;
        if cached.is_some() {
            self.cache_hits = self.cache_hits.saturating_add(1);
        }
        let vrf_result = cached.unwrap_or_else(|| {
            if self.prepared.as_ref().is_none_or(|(table, total, _)| {
                !same_table(table, &input.delegator_table) || *total != input.total_currency
            }) {
                self.prepared = Some((
                    input.delegator_table.clone(),
                    input.total_currency,
                    prepare_delegators(input),
                ));
            }
            let result =
                evaluate_slot_prepared(&self.keypair, input, &self.prepared.as_ref().unwrap().2);
            #[cfg(not(target_family = "wasm"))]
            if let Some(cache) = self.cache.as_mut() {
                if let Err(error) = cache.record(&result) {
                    tracing::warn!("VRF cache write failed: {error}");
                }
            }
            result
        });
        VrfEvaluationOutputWithHash::new(vrf_result, input)
    }

    /// The cached result for `input`, opening the cache for its key first.
    #[cfg(not(target_family = "wasm"))]
    fn cached(&mut self, input: &VrfEvaluatorInput) -> Option<VrfEvaluationOutput> {
        let work_dir = self.work_dir.as_ref()?;
        if self
            .table_digest
            .as_ref()
            .is_none_or(|(table, _)| !same_table(table, &input.delegator_table))
        {
            self.table_digest = Some((
                input.delegator_table.clone(),
                super::vrf_cache::table_digest(&input.delegator_table),
            ));
        }
        let digest = &self.table_digest.as_ref()?.1;
        if self
            .cache
            .as_ref()
            .is_none_or(|cache| !cache.matches(&self.keypair, input, digest))
        {
            self.cache = Some(VrfCache::open(work_dir, &self.keypair, input, digest));
        }
        self.cache.as_ref()?.get(input.global_slot)
    }
}

/// Whether the producer won the slot of `input` for any delegator, and with
/// which: of the delegators that won, the one whose truncated VRF output has
/// the greatest Blake2b digest, the first in ledger order on an equal
/// digest. OCaml evaluates every delegator and keeps that one, its block
/// being the likeliest to win a tie with a block for the same slot.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L840-L907>
pub fn evaluate_slot(keypair: &Keypair, input: &VrfEvaluatorInput) -> VrfEvaluationOutput {
    let prepared = prepare_delegators(input);
    evaluate_slot_prepared(keypair, input, &prepared)
}

struct PreparedDelegator {
    index: AccountIndex,
    public_key: AccountPublicKey,
    threshold: Threshold,
}

fn prepare_delegators(input: &VrfEvaluatorInput) -> Vec<PreparedDelegator> {
    input
        .delegator_table
        .iter()
        .map(|(index, (public_key, stake))| PreparedDelegator {
            index: *index,
            public_key: public_key.clone(),
            threshold: Threshold::new((*stake).into(), input.total_currency.into()),
        })
        .collect()
}

fn evaluate_slot_prepared(
    keypair: &Keypair,
    input: &VrfEvaluatorInput,
    prepared: &[PreparedDelegator],
) -> VrfEvaluationOutput {
    let VrfEvaluatorInput {
        epoch_seed,
        delegator_table: _,
        global_slot,
        total_currency: _,
        staking_ledger_hash: _,
        generation: _,
    } = input;

    let candidate =
        |(order, delegator): (usize, &PreparedDelegator)| match mina_vrf::evaluate_vrf_precomputed(
            keypair,
            *global_slot,
            epoch_seed,
            &delegator.public_key,
            delegator.index,
            &delegator.threshold,
        )
        .unwrap()
        {
            VrfEvaluationOutput::SlotWon(won) => Some((vrf_digest(&won), order, won)),
            VrfEvaluationOutput::SlotLost(_) => None,
        };
    let better = |a: (Vec<u8>, usize, VrfWonSlot), b: (Vec<u8>, usize, VrfWonSlot)| {
        if a.0 > b.0 || (a.0 == b.0 && a.1 < b.1) {
            a
        } else {
            b
        }
    };
    #[cfg(not(target_family = "wasm"))]
    let winner = prepared
        .par_iter()
        .enumerate()
        .filter_map(candidate)
        .reduce_with(better);
    #[cfg(target_family = "wasm")]
    let winner = prepared
        .iter()
        .enumerate()
        .filter_map(candidate)
        .reduce(better);
    winner.map_or(
        VrfEvaluationOutput::SlotLost(*global_slot),
        |(_, _, won)| VrfEvaluationOutput::SlotWon(won),
    )
}

/// The Blake2b digest of the truncated VRF output, which OCaml's `select`
/// and `Vrf.check` compare.
fn vrf_digest(won: &VrfWonSlot) -> Vec<u8> {
    ConsensusVrfOutputTruncatedStableV1::from(&*won.vrf_output).blake2b()
}

impl mina_node::block_producer_effectful::vrf_evaluator_effectful::BlockProducerVrfEvaluatorService
    for NodeService
{
    fn evaluate(&mut self, data: VrfEvaluatorInput) {
        if let Some(bp) = self.block_producer.as_mut() {
            let _ = bp.vrf_evaluation_sender.tracked_send(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ledger::AccountIndex;
    use mina_node::{
        account::AccountSecretKey,
        block_producer::vrf_evaluator::{DelegatorTable, VrfEvaluatorInput},
    };
    use mina_p2p_messages::v2;
    use mina_vrf::{VrfEvaluationInput, VrfEvaluationOutput, VrfWonSlot};

    use super::{evaluate_slot, evaluate_slot_prepared, prepare_delegators, vrf_digest};

    fn table_from_csv(path: &str) -> DelegatorTable {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| {
                let mut fields = line.split(',');
                let index = fields.next().unwrap().parse().unwrap();
                let key = fields.next().unwrap().parse().unwrap();
                let stake = fields.next().unwrap().parse().unwrap();
                (AccountIndex(index), (key, stake))
            })
            .collect()
    }

    /// BP1 slot benchmark. Set BP1_TABLE to a CSV of index,key,stake.
    #[test]
    #[ignore]
    fn benchmark_large_delegator_slot() {
        let keypair = mina_signer::Keypair::rand(&mut rand::thread_rng()).unwrap();
        let delegator = AccountSecretKey::genesis_producer().public_key();
        let table_path = std::env::var("BP1_TABLE").ok();
        let total_currency = if table_path.is_some() {
            1_598_130_912_000_001_000
        } else {
            1_000_000_000_000_000
        };
        let table: DelegatorTable = match table_path {
            Some(path) => table_from_csv(&path),
            None => (0..3_600u64)
                .map(|i| {
                    (
                        AccountIndex(i),
                        (delegator.clone(), 1_000_000_000 + i * 100_000),
                    )
                })
                .collect(),
        };
        println!("BP1 table entries: {}", table.len());
        let input = VrfEvaluatorInput::new(
            v2::EpochSeed::zero(),
            Arc::new(table),
            100_000,
            total_currency,
            v2::LedgerHash::zero(),
        );
        let start = std::time::Instant::now();
        let prepared = prepare_delegators(&input);
        let prepared_at = std::time::Instant::now();
        let result = evaluate_slot_prepared(&keypair, &input, &prepared);
        println!(
            "BP1: prepare {:?}, slot {:?}, result {}",
            prepared_at.duration_since(start),
            prepared_at.elapsed(),
            result.global_slot()
        );
    }

    /// Compares the optimized evaluator to the pre-BP1 algorithm over the
    /// public ledger table. BP1_TABLE must point to index,key,stake CSV.
    #[test]
    #[ignore]
    fn parity_real_delegators() {
        let table_path = std::env::var("BP1_TABLE").expect("set BP1_TABLE");
        let table = Arc::new(table_from_csv(&table_path));
        let keypair = mina_signer::Keypair::rand(&mut rand::thread_rng()).unwrap();
        let slots: u32 = std::env::var("BP1_SLOTS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3_000);
        let input = VrfEvaluatorInput::new(
            v2::EpochSeed::zero(),
            table.clone(),
            100_000,
            1_598_130_912_000_001_000,
            v2::LedgerHash::zero(),
        );
        let prepared = prepare_delegators(&input);
        for offset in 0..slots {
            let input = VrfEvaluatorInput {
                global_slot: input.global_slot + offset,
                ..input.clone()
            };
            let old = table
                .iter()
                .filter_map(|(index, (public_key, stake))| {
                    match mina_vrf::evaluate_vrf(VrfEvaluationInput {
                        producer_key: keypair.clone(),
                        global_slot: input.global_slot,
                        epoch_seed: input.epoch_seed.clone(),
                        account_pub_key: public_key.clone(),
                        delegator_index: *index,
                        delegated_stake: (*stake).into(),
                        total_currency: input.total_currency.into(),
                    })
                    .unwrap()
                    {
                        VrfEvaluationOutput::SlotWon(won) => Some(won),
                        VrfEvaluationOutput::SlotLost(_) => None,
                    }
                })
                .fold(None, |best: Option<(Vec<u8>, VrfWonSlot)>, won| {
                    let digest = vrf_digest(&won);
                    match best {
                        Some((best_digest, best)) if best_digest >= digest => {
                            Some((best_digest, best))
                        }
                        _ => Some((digest, won)),
                    }
                })
                .map_or(
                    VrfEvaluationOutput::SlotLost(input.global_slot),
                    |(_, won)| VrfEvaluationOutput::SlotWon(won),
                );
            let new = evaluate_slot_prepared(&keypair, &input, &prepared);
            assert_eq!(new, old, "slot {}", input.global_slot);
            if offset % 100 == 99 {
                println!("BP1 parity slots: {}", offset + 1);
            }
        }
    }

    /// With several winning delegators, the winner is the one OCaml's
    /// `Vrf.check` keeps (proof_of_stake.ml:882-905): the greatest digest of
    /// the truncated output, not the lowest ledger index as the evaluator
    /// used to choose.
    #[test]
    fn several_winners_the_greatest_vrf_digest_wins() {
        let keypair = AccountSecretKey::genesis_producer().into();
        let delegator = AccountSecretKey::genesis_producer().public_key();
        let total_currency = 1_000_000_000_000u64;
        // Each delegator holds all the stake, so wins three slots in four.
        let delegator_table: DelegatorTable = (0..6u64)
            .map(|i| (AccountIndex(i), (delegator.clone(), total_currency)))
            .collect();
        let delegator_table = Arc::new(delegator_table);
        let mut not_first = 0;
        for global_slot in 0..20 {
            let input = VrfEvaluatorInput::new(
                v2::EpochSeed::zero(),
                delegator_table.clone(),
                global_slot,
                total_currency,
                v2::LedgerHash::zero(),
            );
            let winners = (0..6u64)
                .filter_map(|i| {
                    let mut table = DelegatorTable::new();
                    table.insert(AccountIndex(i), (delegator.clone(), total_currency));
                    let one = VrfEvaluatorInput {
                        delegator_table: Arc::new(table),
                        ..input.clone()
                    };
                    match evaluate_slot(&keypair, &one) {
                        VrfEvaluationOutput::SlotWon(won) => Some(won),
                        VrfEvaluationOutput::SlotLost(_) => None,
                    }
                })
                .collect::<Vec<_>>();
            let best = winners.iter().max_by_key(|won| vrf_digest(won));
            match (evaluate_slot(&keypair, &input), best) {
                (VrfEvaluationOutput::SlotLost(slot), None) => assert_eq!(slot, global_slot),
                (VrfEvaluationOutput::SlotWon(won), Some(best)) => {
                    assert_eq!(&won, best, "slot {global_slot}");
                    if won.account_index != winners[0].account_index {
                        not_first += 1;
                    }
                }
                (result, best) => panic!("slot {global_slot}: {result:?}, {best:?}"),
            }
        }
        // The lowest winning index would have been chosen in these slots.
        assert!(not_first > 0);
    }
}
