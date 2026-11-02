//! Every decoder and check the node runs on what a peer sends, fuzzed from
//! real captured messages. Each test is one kind of input: gossip, each RPC
//! query we answer, each RPC response we take, and the checks the node then
//! runs on what decoded (conversion to the ledger's types, hashing, block
//! prevalidation, the staged ledger parts check). See the crate's
//! documentation for the time budget; by default each takes a few seconds.
//!
//! The messages are the p2p-messages fixtures: Mesa mainnet blocks, catchup
//! answers and completed SNARK work captured from OCaml nodes.

use std::{collections::BTreeSet, sync::Arc};

use binprot::{BinProtRead, BinProtWrite};
use ledger::{
    scan_state::{
        pending_coinbase::PendingCoinbase,
        scan_state::ScanState,
        transaction_logic::{TransactionStatus, UserCommand, WithStatus},
    },
    staged_ledger::{diff::Diff, hash::StagedLedgerHash, staged_ledger::StagedLedger},
    Account, BaseLedger, Mask,
};
use mina_core::block::BlockWithHash;
use mina_p2p_messages::{
    gossip::{decode_pubsub_data, GossipNetMessageV2},
    list::List,
    rpc,
    rpc_kernel::{
        MessageHeader, NeedsLength, PayloadBinprotReader, Query, QueryHeader, RpcMethod, RpcResult,
    },
    string::CharString,
    v2::{self, MinaBlockBlockStableV2},
};
use mina_peer_input_fuzz::{fuzz, replaying, Budget, TrackingAllocator};

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn fixture(path: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../crates/p2p-messages/tests/files/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn encode<T: BinProtWrite>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// A response as it follows the RPC header: `Ok` and the encoded answer
/// with its length.
fn response<T: BinProtWrite>(answer: T) -> Vec<u8> {
    encode(&RpcResult::<_, mina_p2p_messages::rpc_kernel::Error>(Ok(
        NeedsLength(answer),
    )))
}

fn query<T: BinProtWrite>(query: T) -> Vec<u8> {
    encode(&NeedsLength(query))
}

fn decode<M: RpcMethod + PayloadBinprotReader>(bytes: &[u8]) -> M::Response {
    M::response_payload(&mut &bytes[..]).unwrap()
}

/// The captured blocks: a best tip, its ancestry's root, and catchup blocks
/// with signed commands, zkApp commands and completed work.
fn blocks() -> Vec<MinaBlockBlockStableV2> {
    let mut blocks = Vec::new();
    let tip = decode::<rpc::GetBestTipV2>(&fixture("mesa/get_best_tip_v3.bin")).unwrap();
    blocks.push(tip.data);
    blocks.push(tip.proof.1);
    for name in [
        "get_transition_chain_v2_563900_zkapp.bin",
        "get_transition_chain_v2_multi.bin",
    ] {
        blocks.extend(
            decode::<rpc::GetTransitionChainV2>(&fixture(&format!("mesa/{name}")))
                .unwrap()
                .into_iter(),
        );
    }
    blocks
}

fn commands(blocks: &[MinaBlockBlockStableV2]) -> Vec<v2::MinaBaseUserCommandStableV2> {
    blocks
        .iter()
        .flat_map(|block| {
            let diff = &block.body.staged_ledger_diff.diff;
            let second = diff.1.iter().flat_map(|d| d.commands.iter());
            diff.0.commands.iter().chain(second).map(|c| c.data.clone())
        })
        .collect()
}

/// Gossip as it arrives in a pubsub message: an 8-byte length, then the
/// message.
fn gossip(message: &GossipNetMessageV2) -> Vec<u8> {
    let body = encode(message);
    [(body.len() as u64).to_le_bytes().to_vec(), body].concat()
}

fn gossip_corpus() -> Vec<Vec<u8>> {
    let blocks = blocks();
    let mut corpus: Vec<_> = blocks
        .iter()
        .map(|block| gossip(&GossipNetMessageV2::NewState(Arc::new(block.clone()))))
        .collect();
    for chunk in commands(&blocks).chunks(3) {
        corpus.push(gossip(&GossipNetMessageV2::TransactionPoolDiff {
            message: v2::NetworkPoolTransactionPoolDiffVersionedStableV2(
                chunk.iter().cloned().collect(),
            ),
            nonce: 0u64.into(),
        }));
    }
    let works =
        decode::<rpc::GetCompletedSnarksV1>(&fixture("mesa/get_completed_snarks_v1.bin")).unwrap();
    for work in works.0.iter().take(4) {
        corpus.push(gossip(&GossipNetMessageV2::SnarkPoolDiff {
            message: v2::NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(Box::new((
                mina_core::snark::Snark::from(work.clone()).statement(),
                v2::NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1 {
                    proof: work.proofs.clone(),
                    fee: v2::MinaBaseFeeWithProverStableV1 {
                        fee: work.fee.clone(),
                        prover: work.prover.clone(),
                    },
                },
            ))),
            nonce: 0u64.into(),
        }));
    }
    corpus
}

/// Pubsub data, decoded as the node decodes it.
#[test]
fn gossip_decodes_or_is_refused() {
    let corpus = gossip_corpus();
    let report = fuzz("gossip", &corpus, &Budget::from_env(), |bytes| {
        decode_pubsub_data(bytes).is_ok()
    });
    assert!(replaying() || report.accepted >= corpus.len() as u64);
}

/// What the node does with gossip that decodes, before any proof is
/// verified: hash and prevalidate a block, convert and check commands,
/// and convert work.
#[test]
fn gossip_checks_refuse_without_panics() {
    let corpus = gossip_corpus();
    let genesis = BlockWithHash::try_new(Arc::new(blocks().remove(0))).unwrap();
    fuzz(
        "gossip-checks",
        &corpus,
        &Budget::from_env(),
        |bytes| match decode_pubsub_data(bytes) {
            Ok(GossipNetMessageV2::NewState(block)) => check_block(&block, &genesis),
            Ok(GossipNetMessageV2::TransactionPoolDiff { message, .. }) => {
                message.0.iter().all(check_command)
            }
            Ok(GossipNetMessageV2::SnarkPoolDiff { message, .. }) => check_snark_diff(&message),
            Err(_) => false,
        },
    );
}

fn check_block(
    block: &Arc<MinaBlockBlockStableV2>,
    genesis: &BlockWithHash<Arc<MinaBlockBlockStableV2>>,
) -> bool {
    let Ok(block) = BlockWithHash::try_new(block.clone()) else {
        return false;
    };
    // The fixtures' genesis isn't at hand, so this block stands in for it:
    // prevalidation runs, and its answer is not what is checked here.
    let _ = mina_core::block::prevalidate::prevalidate_block(
        &block,
        genesis,
        block.global_slot(),
        true,
    );
    let body =
        ledger::staged_ledger::validate_block::block_body_hash(&block.body().staged_ledger_diff);
    let diff = Diff::try_from(&block.body().staged_ledger_diff);
    for command in block.commands_iter() {
        let _ = check_command(&command.data);
    }
    body.is_ok() && diff.is_ok()
}

/// The pool's checks before verification, then the verifier's own key and
/// signature checks (no proof is verified).
fn check_command(command: &v2::MinaBaseUserCommandStableV2) -> bool {
    // The pool keys a received command by its hash.
    if command.hash().is_err() {
        return false;
    }
    let Ok(command) = UserCommand::try_from(command) else {
        return false;
    };
    let _ = command.accounts_referenced();
    let well_formed = command.check_well_formedness().is_ok();
    let checked = command
        .to_verifiable(&TransactionStatus::Applied, |_, _| Err("no key".to_owned()))
        .map(|data| {
            ledger::verifier::common::check(WithStatus {
                data,
                status: TransactionStatus::Applied,
            })
        });
    well_formed && checked.is_ok()
}

fn check_snark_diff(diff: &v2::NetworkPoolSnarkPoolDiffVersionedStableV2) -> bool {
    let v2::NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(work) = diff else {
        return false;
    };
    let snark: mina_core::snark::Snark = work.1.clone().into();
    let _ = snark.job_id();
    let statement_matches = snark.statement() == work.0;
    let work = v2::TransactionSnarkWorkTStableV2 {
        fee: work.1.fee.fee.clone(),
        proofs: work.1.proof.clone(),
        prover: work.1.fee.prover.clone(),
    };
    statement_matches
        && ledger::scan_state::scan_state::transaction_snark::work::Work::try_from(&work).is_ok()
}

fn fuzz_response<M>(name: &str, corpus: Vec<Vec<u8>>, check: impl Fn(M::Response) -> bool)
where
    M: RpcMethod + PayloadBinprotReader,
{
    let report = fuzz(name, &corpus, &Budget::from_env(), |bytes| {
        M::response_payload(&mut &bytes[..]).is_ok_and(&check)
    });
    assert!(
        replaying() || report.accepted > 0,
        "{name}: nothing accepted"
    );
}

#[test]
fn best_tip_responses() {
    let genesis = BlockWithHash::try_new(Arc::new(blocks().remove(0))).unwrap();
    fuzz_response::<rpc::GetBestTipV2>(
        "get_best_tip",
        vec![fixture("mesa/get_best_tip_v3.bin"), response(None::<()>)],
        |tip| {
            tip.is_some_and(|tip| {
                let _ = tip.proof.0.iter().count();
                check_block(&Arc::new(tip.data), &genesis)
                    && BlockWithHash::try_new(Arc::new(tip.proof.1)).is_ok()
            })
        },
    );
}

#[test]
fn catchup_responses() {
    let genesis = BlockWithHash::try_new(Arc::new(blocks().remove(0))).unwrap();
    fuzz_response::<rpc::GetAncestryV2>(
        "get_ancestry",
        vec![fixture("mesa/get_ancestry_v3.bin")],
        |ancestry| {
            ancestry.is_some_and(|a| {
                check_block(&Arc::new(a.data), &genesis)
                    && BlockWithHash::try_new(Arc::new(a.proof.1)).is_ok()
            })
        },
    );
    fuzz_response::<rpc::GetTransitionChainV2>(
        "get_transition_chain",
        [
            "get_transition_chain_v2_563900_zkapp.bin",
            "get_transition_chain_v2_adjacent.bin",
            "get_transition_chain_v2_multi.bin",
            "get_transition_chain_v2_over_limit.bin",
        ]
        .iter()
        .map(|name| fixture(&format!("mesa/{name}")))
        .collect(),
        |chain| {
            chain.is_some_and(|chain| {
                chain
                    .into_iter()
                    .all(|block| check_block(&Arc::new(block), &genesis))
            })
        },
    );
    fuzz_response::<rpc::GetTransitionChainProofV1ForV2>(
        "get_transition_chain_proof",
        vec![
            fixture("mesa/get_transition_chain_proof_v1.bin"),
            fixture("mesa/get_transition_chain_proof_v1_unknown.bin"),
        ],
        |proof| {
            proof.is_some_and(|(hash, hashes)| {
                hash.to_field::<mina_curves::pasta::Fp>().is_ok()
                    && hashes
                        .iter()
                        .all(|h| h.to_field::<mina_curves::pasta::Fp>().is_ok())
            })
        },
    );
    fuzz_response::<rpc::GetTransitionKnowledgeV1ForV2>(
        "get_transition_knowledge",
        vec![fixture("mesa/get_transition_knowledge_v1.bin")],
        |hashes| {
            hashes
                .iter()
                .all(|h| h.to_field::<mina_curves::pasta::Fp>().is_ok())
        },
    );
}

#[test]
fn completed_snark_responses() {
    fuzz_response::<rpc::GetCompletedSnarksV1>(
        "get_completed_snarks",
        vec![fixture("mesa/get_completed_snarks_v1.bin")],
        |works| {
            works.is_some_and(|works| {
                works.0.iter().all(|work| {
                    let snark: mina_core::snark::Snark = work.clone().into();
                    let _ = snark.job_id();
                    ledger::scan_state::scan_state::transaction_snark::work::Work::try_from(work)
                        .is_ok()
                })
            })
        },
    );
}

#[test]
fn menu_and_peer_responses() {
    fuzz_response::<rpc::VersionedRpcMenuV1>(
        "versioned_rpc_menu",
        vec![fixture("mesa/versioned_rpc_menu_v1.bin")],
        |menu| !menu.is_empty(),
    );
    let peers: List<v2::NetworkPeerPeerStableV1> = [
        (
            "1.2.3.4",
            "12D3KooWCa1d7G3SkRxy846qTvdAFX69NnoYZ32orWVLqJcDVGHW",
        ),
        (
            "seed.example.org",
            "12D3KooWEdBiTUQqxp3jeuWaZkwiSNcFxC6d6Tdq7u2Lf2ZD2Q6X",
        ),
    ]
    .into_iter()
    .map(|(host, peer_id)| v2::NetworkPeerPeerStableV1 {
        host: host.as_bytes().into(),
        libp2p_port: 8302.into(),
        peer_id: v2::NetworkPeerPeerIdStableV1(peer_id.as_bytes().into()),
    })
    .collect();
    fuzz_response::<rpc::GetSomeInitialPeersV1ForV2>(
        "get_some_initial_peers",
        vec![response(peers)],
        |peers| {
            peers.iter().all(|peer| {
                std::str::from_utf8(&peer.host).is_ok()
                    && std::str::from_utf8(&peer.peer_id.0).is_ok()
            })
        },
    );
}

/// Accounts as a ledger sync answer carries them: converted and hashed as
/// `accounts_set` does.
fn accounts() -> Vec<v2::MinaBaseAccountBinableArgStableV2> {
    (0..6)
        .map(|_| v2::MinaBaseAccountBinableArgStableV2::from(&Account::rand()))
        .collect()
}

#[test]
fn sync_ledger_responses() {
    use v2::MinaLedgerSyncLedgerAnswerStableV2 as Answer;
    let hashes: Vec<v2::LedgerHash> = (0..4u64)
        .map(|i| v2::LedgerHash::from_fp(mina_curves::pasta::Fp::from(i)))
        .collect();
    let corpus = vec![
        response(RpcResult::<_, mina_p2p_messages::core::Error>(Ok(
            Answer::ChildHashesAre(hashes.clone().try_into().unwrap()),
        ))),
        response(RpcResult::<_, mina_p2p_messages::core::Error>(Ok(
            Answer::ContentsAre(accounts().into_iter().collect()),
        ))),
        response(RpcResult::<_, mina_p2p_messages::core::Error>(Ok(
            Answer::NumAccounts(1000.into(), hashes[0].clone()),
        ))),
        response(RpcResult::<Answer, _>(Err(
            mina_p2p_messages::core::Info::new(
                "(Ledger not found (hash jxEbxv9Ww1y1J8wqPqFsBi2mtzX8bxk5aBqmHfwUu6pDYN7m3Yf))",
            ),
        ))),
    ];
    fuzz_response::<rpc::AnswerSyncLedgerQueryV2>("answer_sync_ledger_query", corpus, |answer| {
        match Result::from(answer) {
            Ok(Answer::ChildHashesAre(hashes)) => hashes
                .iter()
                .all(|h| h.to_field::<mina_curves::pasta::Fp>().is_ok()),
            Ok(Answer::ContentsAre(accounts)) => accounts.iter().all(|account| {
                Account::try_from(account).is_ok_and(|account| {
                    let _ = account.hash();
                    true
                })
            }),
            Ok(Answer::NumAccounts(_, hash)) => hash.to_field::<mina_curves::pasta::Fp>().is_ok(),
            Err(_) => true,
        }
    });
}

/// A staged ledger's parts as a peer sends them, from a staged ledger with a
/// few accounts.
fn staged_ledger_parts() -> Vec<u8> {
    let mut ledger = Mask::create(35);
    for account in accounts() {
        let account = Account::try_from(&account).unwrap();
        ledger.get_or_create_account(account.id(), account).unwrap();
    }
    let constants = mina_core::constants::constraint_constants();
    let mut staged = StagedLedger::create_exn(constants.clone(), ledger).unwrap();
    let hash = v2::MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    staged.pending_coinbase_collection_merkle_root();
    response(Some((
        v2::TransactionSnarkScanStateStableV2::from(staged.scan_state()),
        hash.non_snark.ledger_hash.0.clone(),
        v2::MinaBasePendingCoinbaseStableV2::from(staged.pending_coinbase_collection()),
        List::<v2::MinaStateProtocolStateValueStableV2>::new(),
    )))
}

/// The parts check the node runs: convert, hash, and require exactly the
/// protocol states the scan state needs.
fn check_parts(
    parts: <rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2 as RpcMethod>::Response,
) -> bool {
    let Some((scan_state, ledger_hash, pending_coinbase, states)) = parts else {
        return false;
    };
    let (Ok(scan_state), Ok(mut pending_coinbase), Ok(ledger_hash)) = (
        ScanState::try_from(&scan_state),
        PendingCoinbase::try_from(&pending_coinbase),
        ledger_hash.to_field::<mina_curves::pasta::Fp>(),
    ) else {
        return false;
    };
    let _ = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
        scan_state.hash(),
        ledger_hash,
        &mut pending_coinbase,
    );
    let required = scan_state.required_state_hashes();
    let received: Result<BTreeSet<_>, _> = states
        .iter()
        .map(|state| state.try_hash()?.to_field::<mina_curves::pasta::Fp>())
        .collect();
    received.is_ok_and(|received| {
        states.len() == required.len() && required.iter().all(|h| received.contains(h))
    })
}

/// Staged ledger parts as a peer sends them. `PEER_INPUT_FUZZ_STAGED_PARTS`
/// adds a captured answer (tens of MB, so not in the repository); it must
/// pass the check before it is mutated.
#[test]
fn staged_ledger_part_responses() {
    let mut corpus = vec![staged_ledger_parts()];
    if let Ok(path) = std::env::var("PEER_INPUT_FUZZ_STAGED_PARTS") {
        corpus.push(std::fs::read(path).unwrap());
    }
    for parts in &corpus {
        let parts = decode::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2>(parts);
        assert!(check_parts(parts), "unmutated parts fail the check");
    }
    fuzz_response::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2>(
        "get_staged_ledger_aux",
        corpus,
        check_parts,
    );
}

/// A query as it arrives: the RPC message header, then the query.
fn query_message<M: RpcMethod>(id: u64, data: M::Query) -> Vec<u8>
where
    M::Query: BinProtWrite,
{
    encode(&mina_p2p_messages::rpc_kernel::Message::Query(Query {
        tag: M::NAME.into(),
        version: M::VERSION,
        id,
        data: NeedsLength(data),
    }))
}

/// Every query we answer, read as the node reads it: the header, then the
/// payload for the tag and version it names.
#[test]
fn queries() {
    let tip = decode::<rpc::GetBestTipV2>(&fixture("mesa/get_best_tip_v3.bin")).unwrap();
    let hash: mina_p2p_messages::bigint::BigInt = tip
        .data
        .header
        .protocol_state
        .previous_state_hash
        .to_field::<mina_curves::pasta::Fp>()
        .unwrap()
        .into();
    let ledger_hash: mina_p2p_messages::bigint::BigInt = tip
        .data
        .header
        .protocol_state
        .body
        .blockchain_state
        .staged_ledger_hash
        .non_snark
        .ledger_hash
        .to_field::<mina_curves::pasta::Fp>()
        .unwrap()
        .into();
    let address = v2::MerkleAddressBinableArgStableV1(4.into(), vec![0b1010_0000].into());
    let corpus = vec![
        query_message::<rpc::VersionedRpcMenuV1>(1, ()),
        query_message::<rpc::GetBestTipV2>(2, ()),
        query_message::<rpc::GetSomeInitialPeersV1ForV2>(3, ()),
        query_message::<rpc::GetCompletedSnarksV1>(4, ()),
        query_message::<rpc::GetTransitionKnowledgeV1ForV2>(5, ()),
        query_message::<rpc::GetTransitionChainProofV1ForV2>(6, hash.clone()),
        query_message::<rpc::GetTransitionChainV2>(
            7,
            [hash.clone(), hash.clone()].into_iter().collect(),
        ),
        query_message::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2>(8, hash.clone()),
        query_message::<rpc::GetAncestryV2>(
            9,
            rpc::WithHashV1 {
                data: tip.data.header.protocol_state.body.consensus_state.clone(),
                hash: hash.clone(),
            },
        ),
        query_message::<rpc::AnswerSyncLedgerQueryV2>(
            10,
            (
                ledger_hash.clone(),
                v2::MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(address.clone(), 3.into()),
            ),
        ),
        query_message::<rpc::AnswerSyncLedgerQueryV2>(
            11,
            (
                ledger_hash,
                v2::MinaLedgerSyncLedgerQueryStableV1::WhatContents(address),
            ),
        ),
    ];
    let report = fuzz("queries", &corpus, &Budget::from_env(), |bytes| {
        let mut bytes = bytes;
        let Ok(MessageHeader::Query(QueryHeader { tag, version, .. })) =
            MessageHeader::binprot_read(&mut bytes)
        else {
            return false;
        };
        read_query(&tag, version, bytes)
    });
    assert!(replaying() || report.accepted >= corpus.len() as u64);
}

fn read_query(tag: &CharString, version: u32, mut bytes: &[u8]) -> bool {
    macro_rules! read {
        ($($rpc:ty),*) => {
            $(
                if tag.as_ref() == <$rpc>::NAME && version == <$rpc>::VERSION {
                    return <$rpc>::query_payload(&mut bytes).is_ok();
                }
            )*
        };
    }
    read!(
        rpc::VersionedRpcMenuV1,
        rpc::GetBestTipV2,
        rpc::GetSomeInitialPeersV1ForV2,
        rpc::GetCompletedSnarksV1,
        rpc::GetTransitionKnowledgeV1ForV2,
        rpc::GetTransitionChainProofV1ForV2,
        rpc::GetTransitionChainV2,
        rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2,
        rpc::GetAncestryV2,
        rpc::AnswerSyncLedgerQueryV2
    );
    false
}

/// A zkApp command whose account updates nest `levels` deep: each update but
/// the last calls the next. Built from a captured command's first update.
fn nested_zkapp(levels: usize) -> v2::MinaBaseUserCommandStableV2 {
    use v2::{
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA as Update,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA as Tree,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA as Call,
    };
    let mut command = commands(&blocks())
        .into_iter()
        .find_map(|command| match command {
            v2::MinaBaseUserCommandStableV2::ZkappCommand(command) => Some(command),
            _ => None,
        })
        .expect("a captured zkApp command");
    let update = command
        .account_updates
        .front()
        .unwrap()
        .elt
        .account_update
        .clone();
    let leaf = || Tree {
        account_update: update.clone(),
        account_update_digest: (),
        calls: List::new(),
    };
    let mut tree = leaf();
    for _ in 1..levels {
        tree = Tree {
            calls: List::one(Call {
                elt: Box::new(tree),
                stack_hash: (),
            }),
            ..leaf()
        };
    }
    command.account_updates = List::one(Update {
        elt: tree,
        stack_hash: (),
    });
    v2::MinaBaseUserCommandStableV2::ZkappCommand(command)
}

fn transaction_gossip(command: v2::MinaBaseUserCommandStableV2) -> Vec<u8> {
    gossip(&GossipNetMessageV2::TransactionPoolDiff {
        message: v2::NetworkPoolTransactionPoolDiffVersionedStableV2(List::one(command)),
        nonce: 0u64.into(),
    })
}

/// A command nested as deep as decoding allows goes through decoding and the
/// pool's checks on the smallest stack that handles commands, a 2 MiB thread
/// (the ledger and verifier threads), in a release build; debug builds get
/// more for their larger frames. One nested deeper is refused while it is
/// read, at any depth.
#[test]
fn deeply_nested_zkapp_commands() {
    let max = mina_p2p_messages::nesting::MAX_CALL_NESTING;
    let stack = if cfg!(debug_assertions) {
        64 << 20
    } else {
        2 << 20
    };
    let deepest = transaction_gossip(nested_zkapp(max));
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let Ok(GossipNetMessageV2::TransactionPoolDiff { message, .. }) =
                decode_pubsub_data(&deepest)
            else {
                panic!("a command nested {max} deep decodes");
            };
            let _ = check_command(message.0.front().unwrap());
        })
        .unwrap()
        .join()
        .unwrap();

    // Building and encoding such commands recurses too: a large stack here.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            for levels in [max + 1, 2 * max, 20_000] {
                let bytes = transaction_gossip(nested_zkapp(levels));
                assert!(decode_pubsub_data(&bytes).is_err(), "{levels} levels");
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

/// A signed command whose signer doesn't decompress to a point fails to
/// decode, as OCaml's reader decompresses the signer. Converted anyway, the
/// verifier refuses its signature.
#[test]
fn a_signer_off_the_curve_is_refused() {
    use ledger::scan_state::transaction_logic::{signed_command::SignedCommand, verifiable};
    use mina_curves::pasta::Fp;

    let x = (1u64..)
        .map(Fp::from)
        .find(|x| mina_signer::CurvePoint::get_point_from_x_unchecked(*x, false).is_none())
        .unwrap();
    let mut command = commands(&blocks())
        .into_iter()
        .find(|c| matches!(c, v2::MinaBaseUserCommandStableV2::SignedCommand(_)))
        .unwrap();
    let valid = transaction_gossip(command.clone());
    assert!(decode_pubsub_data(&valid).is_ok());
    let v2::MinaBaseUserCommandStableV2::SignedCommand(signed) = &mut command else {
        unreachable!()
    };
    signed.signer = v2::NonZeroCurvePointUncompressedStableV1 {
        x: x.into(),
        is_odd: false,
    }
    .into();
    let signed = signed.clone();
    assert!(decode_pubsub_data(&transaction_gossip(command)).is_err());

    let converted = SignedCommand::try_from(&signed).unwrap();
    assert!(verifiable::check_only_for_signature(Box::new(converted)).is_err());
}

/// An account whose token symbol is longer than 6 bytes fails to decode, as
/// OCaml's reader checks it, and converting one built in memory is an error.
#[test]
fn an_account_token_symbol_over_six_bytes_is_refused() {
    let mut account = accounts().remove(0);
    for (symbol, valid) in [
        (&b"MINA"[..], true),
        (&b"SIXSYM"[..], true),
        (&b"SEVENSY"[..], false),
    ] {
        account.token_symbol = symbol.into();
        let bytes = encode(&account);
        let read = v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut &bytes[..]);
        assert_eq!(read.is_ok(), valid, "{symbol:?}");
        assert_eq!(Account::try_from(&account).is_ok(), valid, "{symbol:?}");
    }
}

/// A verification key whose points include (0, 0), the wire's pair for the
/// point at infinity, converts, hashes and converts back to the same wire
/// value, as OCaml keeps and hashes the pairs as they are.
#[test]
fn a_verification_key_point_at_infinity_round_trips() {
    use ledger::VerificationKey;
    use mina_p2p_messages::bigint::BigInt;

    let mut wire = v2::MinaBaseVerificationKeyWireStableV1::from(&*VerificationKey::dummy());
    wire.wrap_index.generic_comm = (BigInt::zero(), BigInt::zero());
    let key = VerificationKey::try_from(&wire).unwrap();
    let _ = key.hash();
    assert_eq!(v2::MinaBaseVerificationKeyWireStableV1::from(&key), wire);

    let other = VerificationKey::try_from(&v2::MinaBaseVerificationKeyWireStableV1::from(
        &*VerificationKey::dummy(),
    ))
    .unwrap();
    assert_ne!(key.hash(), other.hash());
}
