//! JSON-lines adapter for compiled-OCaml differential qualification.
//! This executable accepts only public synthetic scalar identifiers, never keys.
use std::io::{self, BufRead, Write};

use anyhow::{anyhow, bail, Result};
use ark_ff::{BigInteger, BigInteger256, PrimeField};
use ledger::{
    scan_state::currency::Balance, Account, AccountId, AccountIndex, BaseLedger, Mask, TokenId,
};
use mina_core::consensus::{self, ConsensusConstants, ConsensusStateUpdate};
use mina_curves::pasta::{Fp, Fq};
use mina_node::{account::AccountPublicKey, block_producer::vrf_evaluator::*, ledger::LedgerCtx};
use mina_p2p_messages::v2::{self, ConsensusProofOfStakeDataConsensusStateValueStableV2 as State};
use mina_signer::{Keypair, SecKey};
use mina_vrf::{Threshold, VrfEvaluationOutput};
use serde_json::{json, Value};

fn key(id: u64) -> Keypair {
    Keypair::from_secret_key(SecKey::new(Fq::from(id + 1))).unwrap()
}
fn uint(v: &Value, name: &str) -> u64 {
    v[name].as_u64().unwrap()
}
fn number(v: &Value, name: &str) -> u64 {
    v[name].as_str().unwrap().parse().unwrap()
}
fn state(v: &Value, name: &str) -> Result<State> {
    Ok(serde_json::from_value(v[name].clone())?)
}
fn epoch_context_payload(context: &EpochContext) -> Value {
    match context {
        EpochContext::Current(data) | EpochContext::Next(data) => json!({
            "seed": data.seed,
            "ledger": data.ledger,
            "total_currency": data.total_currency.to_string(),
        }),
        EpochContext::Waiting => Value::Null,
    }
}
fn run(v: Value) -> Result<Value> {
    let protocol = v2::MinaBaseProtocolConstantsCheckedValueStableV1::default();
    let constants =
        ConsensusConstants::create(mina_core::constants::constraint_constants(), &protocol);
    let slot = |n: u32| v2::ConsensusGlobalSlotStableV1 {
        slot_number: v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(n.into()),
        slots_per_epoch: protocol.slots_per_epoch,
    };
    Ok(match v["op"].as_str().unwrap() {
        "build_info" => json!(mina_node::BuildEnv::get()),
        "constants" => json!({"slots_per_epoch":constants.slots_per_epoch,
            "slots_per_sub_window":constants.slots_per_sub_window,
            "sub_windows_per_window":constants.sub_windows_per_window,
            "grace_period_end":constants.grace_period_end,
            "slot_duration_ms":constants.slot_duration_ms}),
        "arithmetic" => {
            let s = slot(uint(&v, "slot") as u32);
            let (epoch, within) = mina_node::block_producer::to_epoch_and_slot(&s);
            json!({"epoch":epoch,"slot":within,"seed_update":consensus::in_seed_update_range(within,&protocol),
                "global_sub_window":consensus::global_sub_window(&s,&protocol),
                "relative_sub_window":consensus::relative_sub_window(consensus::global_sub_window(&s,&protocol)),
                "checkpoint_window":consensus::checkpoint_window(&s)})
        }
        "epoch_bounds" => json!(format!(
            "{:?}",
            BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&(uint(&v, "slot") as u32))
        )),
        "density" => {
            let buckets: Vec<u32> = serde_json::from_value(v["buckets"].clone())?;
            let (minimum, buckets) = consensus::update_min_window_density(
                (&constants).into(),
                uint(&v, "prev") as u32,
                uint(&v, "next") as u32,
                &buckets,
                uint(&v, "minimum") as u32,
                v["incr"].as_bool().unwrap(),
            );
            json!({"minimum":minimum,"buckets":buckets})
        }
        "select" => {
            let a = state(&v, "a")?;
            let b = state(&v, "b")?;
            json!({"take":consensus::consensus_take(&a,&b,&serde_json::from_value(v["a_hash"].clone())?,&serde_json::from_value(v["b_hash"].clone())?),
                "short":consensus::is_short_range_fork(&a,&b)})
        }
        "epoch_select" => {
            let s = state(&v, "state")?;
            let target = uint(&v, "epoch") as u32;
            let best =
                mina_node::block_producer::to_epoch_and_slot(&s.curr_global_slot_since_hard_fork).0;
            let mut evaluator = BlockProducerVrfEvaluatorState::new(redux::Timestamp::ZERO);
            let precompute = v["precompute"].as_bool().unwrap_or(false);
            if precompute {
                evaluator.mark_epoch_evaluated(EpochFingerprint::new(
                    best,
                    &s.staking_epoch_data.clone().into(),
                ));
            }
            evaluator.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
                time: redux::Timestamp::ZERO,
                current_epoch: Some(if precompute { best } else { target }),
                best_tip_epoch: best,
                root_block_epoch: best,
                is_next_epoch_seed_finalized: true,
                is_current_epoch_evaluated: precompute,
                is_next_epoch_evaluated: false,
                last_evaluated_epoch: evaluator.last_evaluated_epoch(),
                staking_epoch_data: Box::new(s.staking_epoch_data.clone()),
                next_epoch_data: Box::new(s.next_epoch_data.clone()),
            };
            evaluator.set_epoch_context();
            epoch_context_payload(evaluator.epoch_context())
        }
        "epoch_update" => {
            let s = state(&v, "state")?;
            let updated = consensus::next_consensus_state(
                &constants,
                &s,
                &serde_json::from_value(v["parent_hash"].clone())?,
                ConsensusStateUpdate {
                    curr_global_slot_since_hard_fork: slot(uint(&v, "next") as u32),
                    vrf_hash: Fp::from(number(&v, "vrf_hash")),
                    last_vrf_output: s.last_vrf_output.clone(),
                    snarked_ledger_hash: serde_json::from_value(v["snarked"].clone())?,
                    genesis_ledger_hash: serde_json::from_value(v["genesis"].clone())?,
                    total_currency: Balance::from_u64(number(&v, "total")).into(),
                    block_stake_winner: s.block_stake_winner.clone(),
                    block_creator: s.block_creator.clone(),
                    coinbase_receiver: s.coinbase_receiver.clone(),
                    supercharge_coinbase: s.supercharge_coinbase,
                },
            )
            .map_err(|e| anyhow!(e))?;
            json!({"staking":updated.staking_epoch_data,"next":updated.next_epoch_data,"count":updated.epoch_count.as_u32()})
        }
        "vrf" => {
            let producer = key(uint(&v, "key_id"));
            let threshold = Threshold::new(number(&v, "stake").into(), number(&v, "total").into());
            let mut all = threshold.clone();
            all.threshold_rational = all.total_currency.to_nlimbs::<32>().into();
            let seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(
                v["seed"]
                    .as_str()
                    .unwrap()
                    .parse::<Fp>()
                    .map_err(|_| anyhow!("invalid seed field"))?
                    .into(),
            ));
            let result = mina_vrf::evaluate_vrf_precomputed(
                &producer,
                uint(&v, "slot") as u32,
                &seed,
                &AccountPublicKey::from(producer.public.clone()),
                AccountIndex(uint(&v, "index")),
                &all,
            )?;
            let VrfEvaluationOutput::SlotWon(won) = result else {
                bail!("unconditional VRF did not return output")
            };
            let output = won.vrf_output;
            json!({"hash":output.hash().into_bigint().to_string(),"truncated":hex::encode(output.truncated().into_bigint().to_bytes_le()),
                "won":threshold.threshold_met(output.truncated().into_bigint())})
        }
        "threshold" => {
            let bytes = hex::decode(v["output"].as_str().unwrap())?;
            let bits: Vec<bool> = bytes
                .iter()
                .flat_map(|b| (0..8).map(move |i| (b >> i) & 1 == 1))
                .collect();
            json!(
                Threshold::new(number(&v, "stake").into(), number(&v, "total").into())
                    .threshold_met(BigInteger256::from_bits_le(&bits))
            )
        }
        "delegation" => {
            let mut mask = Mask::create(mina_node::ledger::LEDGER_DEPTH);
            for a in v["accounts"].as_array().unwrap() {
                let owner = key(uint(a, "owner")).public.into_compressed();
                let mut account = Account::create_with(
                    AccountId::new(owner, TokenId::from(uint(a, "token"))),
                    Balance::from_u64(number(a, "balance")),
                );
                account.delegate = a["delegate"]
                    .as_u64()
                    .map(|n| key(n).public.into_compressed());
                mask.get_or_create_account(account.id(), account)
                    .map_err(|e| anyhow!("{e:?}"))?;
            }
            let hash = v2::LedgerHash::from_fp(mask.merkle_root());
            let mut ctx = LedgerCtx::default();
            ctx.insert_genesis_ledger(mask);
            let table = ctx
                .producer_delegator_table(
                    &hash,
                    &AccountPublicKey::from(key(uint(&v, "producer")).public),
                )
                .unwrap();
            Value::Array(table.into_iter().map(|(i,(pk,balance))|json!({"index":i.0,"public_key":pk.to_string(),"balance":balance.to_string()})).collect())
        }
        op => bail!("unknown operation: {op}"),
    })
}
fn main() -> Result<()> {
    let network = std::env::args().nth(1).unwrap_or_else(|| "devnet".into());
    mina_core::NetworkConfig::init(&network).map_err(|e| anyhow!(e))?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in io::stdin().lock().lines() {
        let response =
            run(serde_json::from_str(&line?)?).unwrap_or_else(|e| json!({"error":e.to_string()}));
        writeln!(out, "{response}")?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_context_payload_mutations_cannot_match_the_compiled_oracle() {
        // Expected payload is recorded from compiled OCaml, never Rust output.
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../tools/consensus-diff/regressions.json"
        ))
        .unwrap();
        let fixture = &fixtures["cases"][1];
        let s = state(&fixture["case"], "state").unwrap();
        let original: EpochData = s.next_epoch_data.into();
        let oracle = &fixture["ocaml"];
        let expected = json!({"seed":oracle["seed"], "ledger":oracle["ledger"]["hash"],
            "total_currency":oracle["ledger"]["total_currency"]});
        for next in [false, true] {
            let context = |data| {
                if next {
                    EpochContext::Next(data)
                } else {
                    EpochContext::Current(data)
                }
            };
            assert_eq!(epoch_context_payload(&context(original.clone())), expected);
            for field in ["seed", "ledger", "total_currency"] {
                let mut changed = original.clone();
                match field {
                    "seed" => {
                        changed.seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(
                            Fp::from(0u64).into(),
                        ))
                    }
                    "ledger" => changed.ledger = v2::LedgerHash::from_fp(Fp::from(0u64)),
                    _ => changed.total_currency += 1,
                }
                let observed = epoch_context_payload(&context(changed));
                assert_ne!(
                    observed[field], expected[field],
                    "{field} mutation must be visible"
                );
                assert_ne!(
                    observed, expected,
                    "same variant with stale payload must fail"
                );
            }
        }
    }
}
