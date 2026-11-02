//! MINAWEB-RPC1: the best-chain headers, block and sync progress RPCs on a
//! real `State`, with the reducers only (see
//! `transaction_pool::submission_tests` for the harness), over the Mesa
//! mainnet block 564003 and its `get_best_tip` root.

use std::sync::Arc;

use ledger::{scan_state::currency::Balance, Account, AccountId};
use mina_core::block::{AppliedBlock, ArcBlockWithHash, BlockWithHash};
use mina_p2p_messages::{
    binprot::BinProtRead,
    rpc::GetBestTipV2Response,
    rpc_kernel::ResponsePayload,
    v2::{
        MinaBaseSignedCommandPayloadBodyStableV2, MinaBaseTransactionStatusStableV2,
        MinaBaseUserCommandStableV2, MinaBlockBlockStableV2,
    },
};
use mina_signer::CompressedPubKey;

use crate::{
    rpc::{
        blocks_progress, ledger_sync_progress, RpcAccountSummary, RpcAction,
        RpcBestChainBlockGetResponse, RpcBestChainBlockQuery, RpcBestChainHeaders,
        RpcBestChainHeadersQuery, RpcBlockCommandKind, RpcId, RpcLedgerSyncStatus, RpcSyncProgress,
        RpcSyncStage, MAX_BEST_CHAIN_HEADERS,
    },
    rpc_effectful::RpcEffectfulAction,
    transaction_pool::submission_tests::{block, fresh_state, run, synced_state},
    transition_frontier::sync::{TransitionFrontierSyncBlockState, TransitionFrontierSyncState},
    Action, State,
};

fn rpc_id() -> RpcId {
    RpcId::new_unchecked(12, 1)
}

#[test]
fn best_chain_answers_without_a_tip() {
    let mut state = fresh_state();
    assert!(state.transition_frontier.best_tip().is_none());
    let actions = run(
        &mut state,
        RpcAction::BestChain {
            rpc_id: rpc_id(),
            max_length: 1,
        },
    );
    let responses: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::BestChain {
                rpc_id: id,
                best_chain,
            }) if *id == rpc_id() => Some(best_chain),
            _ => None,
        })
        .collect();
    assert_eq!(
        responses.len(),
        1,
        "the no-tip request must reach its responder"
    );
    assert!(responses[0].is_empty());
}

/// The fixture's root block: the `get_best_tip` proof's root.
fn root_block() -> ArcBlockWithHash {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
    );
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
    let root: MinaBlockBlockStableV2 = payload.0.unwrap().0.unwrap().proof.1;
    BlockWithHash::try_new(Arc::new(root)).unwrap()
}

/// Synced, with the fixture's root and tip as the best chain.
fn synced_with_root() -> State {
    let mut state = synced_state();
    state.transition_frontier.best_chain.insert(
        0,
        AppliedBlock {
            block: root_block(),
            just_emitted_a_proof: false,
        },
    );
    state
}

fn headers(state: &mut State, query: RpcBestChainHeadersQuery) -> RpcBestChainHeaders {
    let ran = run(
        state,
        RpcAction::BestChainHeadersGet {
            rpc_id: rpc_id(),
            query,
        },
    );
    let answers: Vec<_> = ran
        .iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::BestChainHeadersGet {
                rpc_id: id,
                response,
            }) if *id == rpc_id() => Some(response.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "one answer");
    answers.into_iter().next().unwrap()
}

fn block_detail(state: &mut State, query: RpcBestChainBlockQuery) -> RpcBestChainBlockGetResponse {
    let ran = run(
        state,
        RpcAction::BestChainBlockGet {
            rpc_id: rpc_id(),
            query,
        },
    );
    let answers: Vec<_> = ran
        .iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::BestChainBlockGet {
                rpc_id: id,
                response,
            }) if *id == rpc_id() => Some(response.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "one answer");
    answers.into_iter().next().unwrap()
}

fn progress(state: &mut State) -> RpcSyncProgress {
    let ran = run(state, RpcAction::SyncProgressGet { rpc_id: rpc_id() });
    let answers: Vec<_> = ran
        .iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::SyncProgressGet {
                rpc_id: id,
                response,
            }) if *id == rpc_id() => Some(response.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "one answer");
    answers.into_iter().next().unwrap()
}

/// The fixture block's commands, read straight from its diff: (signed,
/// zkApp, failed, sum of fees).
fn fixture_command_counts() -> (u32, u32, u32, u64) {
    let tip = block();
    let diff = &tip.block.body.staged_ledger_diff.diff;
    let commands = diff
        .0
        .commands
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.commands.iter()));
    let (mut signed, mut zkapps, mut failed, mut fees) = (0, 0, 0, 0u64);
    for c in commands {
        match &c.data {
            MinaBaseUserCommandStableV2::SignedCommand(s) => {
                signed += 1;
                fees += s.payload.common.fee.as_u64();
            }
            MinaBaseUserCommandStableV2::ZkappCommand(z) => {
                zkapps += 1;
                fees += z.fee_payer.body.fee.as_u64();
            }
        }
        if matches!(c.status, MinaBaseTransactionStatusStableV2::Failed(_)) {
            failed += 1;
        }
    }
    (signed, zkapps, failed, fees)
}

#[test]
fn headers_are_newest_first_with_the_blocks_counts() {
    let mut state = synced_with_root();
    let answer = headers(&mut state, RpcBestChainHeadersQuery::default());
    let tip = block();
    let root = root_block();

    assert_eq!(answer.k, 290, "consensus k on devnet");
    assert_eq!(answer.best_tip_height, Some(564003));
    assert_eq!(answer.root_height, Some(root.height()));
    assert_eq!(answer.headers.len(), 2);
    let newest = &answer.headers[0];
    assert_eq!(newest.height, 564003);
    assert_eq!(&newest.hash, tip.hash());
    assert_eq!(&newest.pred_hash, tip.pred_hash());
    assert_eq!(answer.headers[1].hash, *root.hash());

    let (signed, zkapps, failed, fees) = fixture_command_counts();
    assert!(signed > 0, "the fixture has payments");
    assert_eq!(newest.user_commands, signed);
    assert_eq!(newest.zkapp_commands, zkapps);
    assert_eq!(newest.failed_commands, failed);
    assert_eq!(newest.transaction_fees, fees);
    let works: Vec<_> = tip.completed_works_iter().collect();
    assert_eq!(newest.snark_jobs as usize, works.len());
    assert_eq!(
        newest.snark_fees,
        works.iter().map(|w| w.fee.as_u64()).sum::<u64>()
    );

    // The slot in epoch and the epoch give the slot since the hard fork.
    assert_eq!(newest.epoch * 7140 + newest.slot, newest.global_slot);
    let consensus = &tip.block.header.protocol_state.body.consensus_state;
    assert_eq!(
        newest.producer.to_string(),
        CompressedPubKey::try_from(&consensus.block_creator)
            .unwrap()
            .into_address()
    );
    assert_eq!(
        newest.timestamp_ms,
        tip.block
            .header
            .protocol_state
            .body
            .blockchain_state
            .timestamp
            .as_u64()
    );

    // The JSON a client parses: keys in Base58Check, numbers as numbers.
    let json = serde_json::to_value(newest).unwrap();
    assert!(json["producer"].as_str().unwrap().starts_with("B62"));
    assert!(json["hash"].as_str().unwrap().starts_with("3N"));
    assert_eq!(json["height"], 564003);
    assert_eq!(json["transaction_fees"], fees.to_string());
}

#[test]
fn headers_page_down_by_height_and_cap_the_limit() {
    let mut state = synced_with_root();
    let root_height = root_block().height();

    let one = headers(
        &mut state,
        RpcBestChainHeadersQuery {
            limit: Some(1),
            max_height: None,
        },
    );
    assert_eq!(one.headers.len(), 1);
    assert_eq!(one.headers[0].height, 564003);

    let below = headers(
        &mut state,
        RpcBestChainHeadersQuery {
            limit: Some(10),
            max_height: Some(564002),
        },
    );
    assert_eq!(
        below.headers.iter().map(|h| h.height).collect::<Vec<_>>(),
        vec![root_height]
    );
    assert_eq!(below.best_tip_height, Some(564003), "whole chain's tip");

    let huge = headers(
        &mut state,
        RpcBestChainHeadersQuery {
            limit: Some(usize::MAX),
            max_height: None,
        },
    );
    assert_eq!(huge.headers.len(), 2);
    assert!(MAX_BEST_CHAIN_HEADERS > 290, "a whole frontier fits");

    // Nothing to read before a best chain: an empty answer, at once.
    let mut fresh = fresh_state();
    let empty = headers(&mut fresh, RpcBestChainHeadersQuery::default());
    assert!(empty.headers.is_empty());
    assert_eq!(empty.best_tip_height, None);
    assert!(fresh.rpc.requests.is_empty(), "no request is left behind");
}

#[test]
fn block_detail_lists_what_the_block_applied() {
    let mut state = synced_with_root();
    let tip = block();

    let by_hash =
        block_detail(&mut state, RpcBestChainBlockQuery::Hash(tip.hash().clone())).unwrap();
    let by_height = block_detail(&mut state, RpcBestChainBlockQuery::Height(564003)).unwrap();
    assert_eq!(by_hash, by_height);
    assert_eq!(by_hash.header.height, 564003);
    assert_eq!(&by_hash.staged_ledger_hash, tip.merkle_root_hash());
    assert_eq!(&by_hash.snarked_ledger_hash, tip.snarked_ledger_hash());

    let (signed, zkapps, failed, fees) = fixture_command_counts();
    assert_eq!(by_hash.commands.len() as u32, signed + zkapps);
    assert_eq!(
        by_hash
            .commands
            .iter()
            .filter(|c| c.failure.is_some())
            .count() as u32,
        failed
    );
    assert_eq!(by_hash.commands.iter().map(|c| c.fee).sum::<u64>(), fees);
    assert_eq!(by_hash.snark_work.len() as u32, by_hash.header.snark_jobs);

    // The first command, compared field by field with the raw diff.
    let first = tip.commands_iter().next().unwrap();
    let ours = &by_hash.commands[0];
    assert_eq!(ours.hash.as_ref(), Some(&first.data.hash().unwrap()));
    if let MinaBaseUserCommandStableV2::SignedCommand(s) = &first.data {
        let common = &s.payload.common;
        assert_eq!(ours.nonce, common.nonce.as_u32());
        assert_eq!(ours.memo, common.memo.to_base58check());
        match &s.payload.body {
            MinaBaseSignedCommandPayloadBodyStableV2::Payment(p) => {
                assert_eq!(ours.kind, RpcBlockCommandKind::Payment);
                assert_eq!(ours.amount, Some(p.amount.as_u64()));
            }
            MinaBaseSignedCommandPayloadBodyStableV2::StakeDelegation(_) => {
                assert_eq!(ours.kind, RpcBlockCommandKind::Delegation);
                assert_eq!(ours.amount, None);
            }
        }
    }

    // The coinbase is the protocol's, or twice it when supercharged, less
    // nothing: a SNARK fee paid from it is a separate fee transfer.
    let coinbase = by_hash.coinbase.expect("the diff expands");
    let base = mina_core::constants::constraint_constants().coinbase_amount;
    assert!(
        coinbase == base || coinbase == 2 * base,
        "coinbase {coinbase} vs {base}"
    );
    // Command fees all go out again as fee transfers (to SNARK workers and
    // the coinbase receiver); a fee paid from the coinbase comes on top.
    let paid: u64 = by_hash
        .fee_transfers
        .iter()
        .filter(|t| !t.via_coinbase)
        .map(|t| t.fee)
        .sum();
    assert_eq!(paid, fees);

    assert_eq!(
        block_detail(&mut state, RpcBestChainBlockQuery::Height(1)),
        None
    );
    assert!(state.rpc.requests.is_empty(), "no request is left behind");
}

#[test]
fn sync_progress_when_synced() {
    let mut state = synced_with_root();
    let answer = progress(&mut state);
    assert_eq!(answer.k, 290);
    assert_eq!(answer.stage, RpcSyncStage::Synced);
    assert_eq!(answer.blocks, None);
    for ledger in [
        &answer.staking_epoch_ledger,
        &answer.next_epoch_ledger,
        &answer.root_snarked_ledger,
        &answer.root_staged_ledger,
    ] {
        assert_eq!(ledger.status, RpcLedgerSyncStatus::Done);
    }
    // The message-progress RPC reports no ledger when none is syncing.
    assert!(matches!(ledger_sync_progress(&state), (None, None, None)));

    // A node that just started has not reached any ledger.
    let mut fresh = fresh_state();
    let answer = progress(&mut fresh);
    assert_eq!(answer.stage, RpcSyncStage::Idle);
    assert_eq!(answer.phase, fresh.reported_sync_phase());
    assert_eq!(
        answer.staking_epoch_ledger.status,
        RpcLedgerSyncStatus::Waiting
    );
    assert_eq!(
        answer.root_staged_ledger.status,
        RpcLedgerSyncStatus::Waiting
    );
}

#[test]
fn sync_progress_counts_the_blocks_in_flight() {
    let mut state = synced_with_root();
    let root = root_block();
    let tip = block();
    let time = crate::transaction_pool::submission_tests::now();
    state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
        time,
        chain: vec![
            TransitionFrontierSyncBlockState::ApplySuccess {
                time,
                block: AppliedBlock {
                    block: root.clone(),
                    just_emitted_a_proof: false,
                },
            },
            TransitionFrontierSyncBlockState::ApplyPending {
                time,
                block: root.clone(),
            },
            TransitionFrontierSyncBlockState::FetchSuccess {
                time,
                block: tip.clone(),
            },
            TransitionFrontierSyncBlockState::FetchPending {
                time,
                block_hash: tip.hash().clone(),
                attempts: Default::default(),
            },
        ],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    let answer = progress(&mut state);
    assert_eq!(answer.stage, RpcSyncStage::Blocks);
    assert_eq!(answer.root_height, Some(root.height()));
    // The tip isn't fetched: the target is the root's height plus the
    // chain after it. The fixture's root and tip aren't k apart, so this
    // made-up chain gives root + 3.
    assert_eq!(answer.target_height, Some(root.height() + 3));
    assert_eq!(answer.root_staged_ledger.status, RpcLedgerSyncStatus::Done);
    let blocks = answer.blocks.unwrap();
    assert_eq!(blocks.cells, "AaF.");
    assert_eq!(
        (blocks.total, blocks.fetched, blocks.applied, blocks.failed),
        (4, 3, 1, 0)
    );

    let failed = blocks_progress("AAxf..".chars());
    assert_eq!(
        (failed.total, failed.fetched, failed.applied, failed.failed),
        (6, 3, 2, 1)
    );
}

#[test]
fn account_summary_reads_the_ledger_account() {
    let key =
        CompressedPubKey::from_address("B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV")
            .unwrap();
    let mut account = Account::create_with(
        AccountId::new_with_default_token(key.clone()),
        Balance::from_u64(9_007_199_254_740_993),
    );
    account.delegate = Some(key);
    let summary = RpcAccountSummary::from(&account);
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        json["public_key"],
        "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
    );
    assert_eq!(json["delegate"], json["public_key"]);
    // Amounts as decimal strings: a number loses precision past 2^53.
    assert_eq!(json["balance"], "9007199254740993");
    let back: RpcAccountSummary = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, summary);
    assert_eq!(json["nonce"], 0);
    assert!(json["timing"].is_null());
    assert!(json["zkapp"].is_null());
    // The default token's id, as OCaml prints it.
    assert_eq!(
        json["token_id"],
        "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf"
    );
}
