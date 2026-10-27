use super::{
    read::{LedgerReadId, LedgerReadRequest, LedgerReadResponse, LedgerStatus},
    write::{LedgerWriteRequest, LedgerWriteResponse},
    LedgerCtx, LedgerService,
};
use crate::{
    account::AccountPublicKey,
    ledger::LedgerAddress,
    persist::{
        ledger_codec::FullPart,
        writer::{PersistInsert, PersistInserter},
    },
    rpc::AccountQuery,
    transition_frontier::sync::ledger::snarked::TransitionFrontierSyncLedgerSnarkedService,
};
use ledger::{
    staged_ledger::staged_ledger::{SkipVerification, StagedLedger},
    Account, AccountId, Mask,
};
use mina_core::{channels::mpsc, thread};
use mina_p2p_messages::v2::{self, LedgerHash, MinaBaseAccountBinableArgStableV2};
use mina_signer::CompressedPubKey;
use std::collections::BTreeMap;

/// The type enumerating different requests that can be made to the
/// service. Each specific constructor has a specific response
/// constructor associated with it. Unfortunately, this relationship
/// can't be expressed in the Rust type system at the moment. For this
/// reason this type is private while functions wrapping the whole call
/// to the service are exposed as the service's methods.
#[expect(
    clippy::large_enum_variant,
    reason = "This is storing large messages, but size difference is only 2x between largest and second-largest variants"
)]
pub(super) enum LedgerRequest {
    MemoryDebug,
    Write(LedgerWriteRequest),
    Read(LedgerReadId, LedgerReadRequest),
    /// expected response: `LedgerHash`
    AccountsSet {
        snarked_ledger_hash: LedgerHash,
        parent: LedgerAddress,
        expected_hash: LedgerHash,
        accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    },
    /// expected response: `Vec<Account>`
    AccountsGet {
        ledger_hash: LedgerHash,
        account_ids: Vec<AccountId>,
    },
    /// expected response: `ChildHashes`
    ChildHashesGet {
        snarked_ledger_hash: LedgerHash,
        parent: LedgerAddress,
        depth: usize,
    },
    /// expected response: `Success`
    ComputeSnarkedLedgerHashes {
        snarked_ledger_hash: LedgerHash,
    },
    ContainsSnarkedLedger {
        hash: LedgerHash,
    },
    /// expected response: `SnarkedLedgerContentsCopied`
    CopySnarkedLedgerContentsForSync {
        origin_snarked_ledger_hash: Vec<LedgerHash>,
        target_snarked_ledger_hash: LedgerHash,
        overwrite: bool,
    },
    /// expected response: `ProducersWithDelegatesMap`
    GetProducersWithDelegates {
        ledger_hash: LedgerHash,
        filter: fn(&CompressedPubKey) -> bool,
    },
    /// expected response: `LedgerMask`
    GetMask {
        ledger_hash: LedgerHash,
    },
    InsertGenesisLedger {
        mask: Mask,
    },
    /// From the store thread: a piece of a ledger pinned for a checkpoint.
    /// expected response: `PersistRead`
    PersistRead {
        pin: u64,
        part: FullPart,
    },
    /// From the store thread, when a checkpoint is done: releases its pins.
    PersistUnpin {
        pins: Vec<u64>,
    },
    /// From the store thread, a load's ledgers for a restore (I5d1).
    /// expected response: `PersistInserted`
    PersistInsert {
        insert: PersistInsert,
    },
    /// PC1: the proofs this thread holds.
    /// expected response: `ProofMemory`
    ProofMemory,
}

#[derive(Debug)]
pub enum LedgerResponse {
    MemoryDebug(serde_json::Value),
    Write(LedgerWriteResponse),
    Read(LedgerReadId, LedgerReadResponse),
    ChildHashes(Option<Vec<LedgerHash>>),
    AccountsSet(Result<LedgerHash, String>),
    AccountsGet(Result<Vec<Account>, String>),
    LedgerMask(Option<(Mask, bool)>),
    #[allow(clippy::type_complexity)]
    ProducersWithDelegatesMap(
        Option<BTreeMap<AccountPublicKey, Vec<(ledger::AccountIndex, AccountPublicKey, u64)>>>,
    ),
    SnarkedLedgerContentsCopied(Result<bool, String>),
    ContainsSnarkedLedger(bool),
    /// The bytes of a piece of a pinned ledger, and the time taken.
    PersistRead(Result<(Vec<u8>, f64), String>),
    /// A load's ledgers were taken, in this many milliseconds.
    PersistInserted(f64),
    ProofMemory(Box<crate::proof_memory::LedgerProofMemory>),
    Success, // operation was performed and result stored; nothing to return.
}

impl LedgerRequest {
    #[cfg(test)]
    pub(super) fn read_for_tests(
        ctx: &mut LedgerCtx,
        request: LedgerReadRequest,
    ) -> LedgerReadResponse {
        let (sender, _receiver) = mpsc::tracked_unbounded_channel();
        match Self::Read(LedgerReadId::new_unchecked(0, 0), request)
            .handle(ctx, &LedgerCaller(sender))
        {
            LedgerResponse::Read(_, response) => response,
            _ => unreachable!(),
        }
    }

    fn handle(self, ledger_ctx: &mut LedgerCtx, caller: &LedgerCaller) -> LedgerResponse {
        match self {
            Self::MemoryDebug => LedgerResponse::MemoryDebug(ledger_ctx.memory_debug()),
            Self::Write(request) => LedgerResponse::Write(match request {
                LedgerWriteRequest::StagedLedgerReconstruct {
                    snarked_ledger_hash,
                    parts,
                } => {
                    // On this thread: the staged ledger is a mask over the
                    // snarked ledger this thread serves (R38).
                    let (staged_ledger_hash, result) = match ledger_ctx
                        .staged_ledger_reconstruct_sync(snarked_ledger_hash, parts)
                    {
                        Ok(result) => result,
                        Err(e) => (v2::LedgerHash::zero(), Err(String::from(e))),
                    };

                    LedgerWriteResponse::StagedLedgerReconstruct {
                        staged_ledger_hash,
                        result,
                    }
                }
                LedgerWriteRequest::StagedLedgerDiffCreate {
                    pred_block,
                    global_slot_since_genesis: global_slot,
                    is_new_epoch,
                    producer,
                    delegator,
                    coinbase_receiver,
                    completed_snarks,
                    supercharge_coinbase,
                    transactions_by_fee,
                } => {
                    let pred_block_hash = pred_block.hash().clone();
                    let global_slot_since_genesis = global_slot.clone();
                    let result = ledger_ctx.staged_ledger_diff_create(
                        pred_block,
                        global_slot,
                        is_new_epoch,
                        producer,
                        delegator,
                        coinbase_receiver,
                        completed_snarks,
                        supercharge_coinbase,
                        transactions_by_fee,
                    );
                    LedgerWriteResponse::StagedLedgerDiffCreate {
                        pred_block_hash,
                        global_slot_since_genesis,
                        result: result.map(Into::into),
                    }
                }
                LedgerWriteRequest::BlockApply {
                    block,
                    pred_block,
                    skip_verification,
                    restored,
                } => {
                    let block_hash = block.hash().clone();
                    let skip_verification = if skip_verification {
                        Some(SkipVerification::All)
                    } else {
                        None
                    };
                    let result =
                        ledger_ctx.block_apply_with(block, pred_block, skip_verification, restored);
                    LedgerWriteResponse::BlockApply { block_hash, result }
                }
                LedgerWriteRequest::Commit {
                    root_history,
                    ledgers_to_keep,
                    root_snarked_ledger_updates,
                    needed_protocol_states,
                    new_root,
                    new_best_tip,
                } => {
                    let best_tip_hash = new_best_tip.hash().clone();
                    let result = ledger_ctx.commit(
                        root_history,
                        ledgers_to_keep,
                        root_snarked_ledger_updates,
                        needed_protocol_states,
                        &new_root,
                        &new_best_tip,
                    );
                    LedgerWriteResponse::Commit {
                        best_tip_hash,
                        result,
                    }
                }
                LedgerWriteRequest::PersistCheckpoint(request) => {
                    let root = request.checkpoint.root.clone();
                    let result = ledger_ctx.persist_checkpoint(*request, caller);
                    LedgerWriteResponse::PersistCheckpoint { root, result }
                }
            }),
            Self::Read(id, request) => LedgerResponse::Read(
                id,
                match request {
                    LedgerReadRequest::DelegatorTable(ledger_hash, producer) => {
                        let res = ledger_ctx.producer_delegator_table(&ledger_hash, &producer);
                        LedgerReadResponse::DelegatorTable(res)
                    }
                    LedgerReadRequest::GetNumAccounts(ledger_hash) => {
                        let res = ledger_ctx.get_num_accounts(ledger_hash);
                        LedgerReadResponse::GetNumAccounts(res)
                    }
                    LedgerReadRequest::GetChildHashesAtAddr(ledger_hash, addr, depth) => {
                        // Network answers must come from a complete ledger.
                        // The synchronous ChildHashesGet path also reads our
                        // partial mask to compare it with validated peer hashes.
                        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_handler/sync_handler.ml#L53-L115>
                        let res = ledger_ctx.get_child_hashes_for_serving(ledger_hash, addr, depth);
                        LedgerReadResponse::GetChildHashesAtAddr(res)
                    }
                    LedgerReadRequest::GetChildAccountsAtAddr(ledger_hash, addr) => {
                        let res = ledger_ctx.get_child_accounts(ledger_hash, addr);
                        LedgerReadResponse::GetChildAccountsAtAddr(res)
                    }
                    LedgerReadRequest::GetStagedLedgerAuxAndPendingCoinbases(data) => {
                        let res = ledger_ctx.staged_ledger_aux_and_pending_coinbase(
                            &data.block_hash,
                            &data.ledger_hash,
                            data.protocol_states,
                        );
                        LedgerReadResponse::GetStagedLedgerAuxAndPendingCoinbases(res)
                    }
                    LedgerReadRequest::SnarkPoolProver(request) => {
                        LedgerReadResponse::SnarkPoolProver(
                            ledger_ctx.snark_pool_prover_account(
                                &request.ledger_hash,
                                &request.work.snarker,
                            ),
                        )
                    }
                    LedgerReadRequest::WorkCoverage(request) => {
                        LedgerReadResponse::WorkCoverage(ledger_ctx.work_coverage(*request))
                    }
                    LedgerReadRequest::ScanStateSummary(ledger_hash) => {
                        let res = ledger_ctx.scan_state_summary(&ledger_hash);
                        LedgerReadResponse::ScanStateSummary(res)
                    }
                    LedgerReadRequest::GetAccounts(ledger_hash, account_ids, rpc_id) => {
                        let res = ledger_ctx.get_accounts(ledger_hash, account_ids);
                        LedgerReadResponse::GetAccounts(res, rpc_id)
                    }
                    LedgerReadRequest::AccountsForRpc(rpc_id, ledger_hash, account_query) => {
                        let res = match &account_query {
                            AccountQuery::All => ledger_ctx.get_accounts_for_rpc(ledger_hash, None),
                            AccountQuery::SinglePublicKey(public_key) => ledger_ctx
                                .get_accounts_for_rpc(ledger_hash, Some(public_key.clone())),
                            AccountQuery::PubKeyWithTokenId(public_key, token_id_key_hash) => {
                                // Values outside the field are no account id.
                                match (
                                    public_key.clone().try_into(),
                                    ledger::TokenId::try_from(token_id_key_hash),
                                ) {
                                    (Ok(public_key), Ok(token_id)) => ledger_ctx.get_accounts(
                                        ledger_hash,
                                        vec![AccountId {
                                            public_key,
                                            token_id,
                                        }],
                                    ),
                                    _ => vec![],
                                }
                            }
                            AccountQuery::MultipleIds(ids) => {
                                ledger_ctx.get_accounts(ledger_hash, ids.clone())
                            }
                            AccountQuery::TokenOwner(token_id) => {
                                match ledger::TokenId::try_from(token_id) {
                                    Ok(token_id) => {
                                        ledger_ctx.token_owner_for_rpc(ledger_hash, token_id)
                                    }
                                    Err(_) => vec![],
                                }
                            }
                            AccountQuery::StakingEpochIds(ids) => {
                                ledger_ctx.get_accounts(ledger_hash, ids.clone())
                            }
                        };

                        LedgerReadResponse::AccountsForRpc(rpc_id, res, account_query)
                    }
                    LedgerReadRequest::GetLedgerStatus(rpc_id, ledger_hash) => {
                        let res = ledger_ctx.get_num_accounts(ledger_hash).map(
                            |(num_accounts, ledger_hash)| LedgerStatus {
                                num_accounts,
                                best_tip_staged_ledger_hash: ledger_hash,
                            },
                        );

                        LedgerReadResponse::GetLedgerStatus(rpc_id, res)
                    }
                    LedgerReadRequest::GetAccountDelegators(rpc_id, query, account_id) => {
                        let res = ledger_ctx.delegators_for_rpc(&query, &account_id);
                        LedgerReadResponse::GetAccountDelegators(rpc_id, res)
                    }
                    LedgerReadRequest::LedgerExport(rpc_id, request) => {
                        let res = ledger_ctx.export_ledger(&request).map(std::sync::Arc::new);
                        LedgerReadResponse::LedgerExport(rpc_id, res)
                    }
                    LedgerReadRequest::HardForkExport(rpc_id, request) => {
                        let res = ledger_ctx
                            .export_hard_fork(&request)
                            .map(std::sync::Arc::new);
                        LedgerReadResponse::HardForkExport(rpc_id, res)
                    }
                    LedgerReadRequest::LightAccount(rpc_id, request) => {
                        LedgerReadResponse::LightAccount(rpc_id, ledger_ctx.light_account(&request))
                    }
                },
            ),
            LedgerRequest::AccountsSet {
                snarked_ledger_hash,
                parent,
                expected_hash,
                accounts,
            } => LedgerResponse::AccountsSet(ledger_ctx.accounts_set(
                snarked_ledger_hash,
                &parent,
                &expected_hash,
                accounts,
            )),
            LedgerRequest::ChildHashesGet {
                snarked_ledger_hash,
                parent,
                depth,
            } => {
                let res = ledger_ctx.get_child_hashes(snarked_ledger_hash, parent, depth);
                LedgerResponse::ChildHashes(res)
            }
            LedgerRequest::ComputeSnarkedLedgerHashes {
                snarked_ledger_hash,
            } => {
                ledger_ctx
                    .compute_snarked_ledger_hashes(&snarked_ledger_hash)
                    .unwrap();
                LedgerResponse::Success
            }
            LedgerRequest::ContainsSnarkedLedger { hash } => {
                LedgerResponse::ContainsSnarkedLedger(ledger_ctx.contains_snarked_ledger(&hash))
            }
            LedgerRequest::CopySnarkedLedgerContentsForSync {
                origin_snarked_ledger_hash,
                target_snarked_ledger_hash,
                overwrite,
            } => {
                // The candidates come closest to the target first.
                let res = ledger_ctx.copy_snarked_ledger_contents_for_sync(
                    &origin_snarked_ledger_hash,
                    target_snarked_ledger_hash,
                    overwrite,
                );
                LedgerResponse::SnarkedLedgerContentsCopied(res)
            }
            LedgerRequest::GetMask { ledger_hash } => {
                LedgerResponse::LedgerMask(ledger_ctx.mask(&ledger_hash))
            }
            LedgerRequest::GetProducersWithDelegates {
                ledger_hash,
                filter,
            } => {
                let res = ledger_ctx.producers_with_delegates(&ledger_hash, filter);
                LedgerResponse::ProducersWithDelegatesMap(res)
            }
            LedgerRequest::InsertGenesisLedger { mask } => {
                ledger_ctx.insert_genesis_ledger(mask);
                LedgerResponse::Success
            }
            LedgerRequest::AccountsGet {
                ledger_hash,
                account_ids,
            } => {
                // Synchronous consumers must distinguish an unavailable
                // ledger from an available ledger with no matching accounts.
                let res = ledger_ctx
                    .mask(&ledger_hash)
                    .ok_or_else(|| "Ledger unavailable".to_owned())
                    .map(|_| ledger_ctx.get_accounts(ledger_hash, account_ids));
                LedgerResponse::AccountsGet(res)
            }
            LedgerRequest::PersistRead { pin, part } => {
                LedgerResponse::PersistRead(ledger_ctx.persist_read(pin, part))
            }
            LedgerRequest::PersistUnpin { pins } => {
                ledger_ctx.persist_unpin(&pins);
                LedgerResponse::Success
            }
            LedgerRequest::PersistInsert { insert } => {
                LedgerResponse::PersistInserted(ledger_ctx.persist_insert(insert))
            }
            LedgerRequest::ProofMemory => {
                LedgerResponse::ProofMemory(Box::new(ledger_ctx.proof_memory()))
            }
        }
    }
}

struct LedgerRequestWithChan {
    request: LedgerRequest,
    responder: Option<std::sync::mpsc::SyncSender<LedgerResponse>>,
}

pub struct LedgerManager {
    caller: LedgerCaller,
    join_handle: thread::JoinHandle<LedgerCtx>,
}

#[derive(Clone)]
pub(super) struct LedgerCaller(mpsc::TrackedUnboundedSender<LedgerRequestWithChan>);

/// A handle for querying ledger cache sizes from a diagnostic worker.
#[derive(Clone)]
pub struct LedgerMemoryDebugHandle(LedgerCaller);

impl LedgerMemoryDebugHandle {
    pub fn read(&self) -> Option<serde_json::Value> {
        match self.0.try_call_sync(LedgerRequest::MemoryDebug).ok()? {
            LedgerResponse::MemoryDebug(value) => Some(value),
            _ => None,
        }
    }
}

impl LedgerManager {
    /// Clone a handle for a worker to query ledger sizes off the state thread.
    pub fn memory_debug_handle(&self) -> LedgerMemoryDebugHandle {
        LedgerMemoryDebugHandle(self.caller.clone())
    }

    pub fn spawn(mut ledger_ctx: LedgerCtx) -> LedgerManager {
        let (sender, mut receiver) = mpsc::tracked_unbounded_channel();
        let caller = LedgerCaller(sender);
        let ledger_caller = caller.clone();

        let ledger_manager_loop = move || {
            while let Some(msg) = receiver.blocking_recv() {
                let LedgerRequestWithChan { request, responder } = msg.0;
                let response = request.handle(&mut ledger_ctx, &ledger_caller);
                match (response, responder) {
                    (LedgerResponse::Write(resp), None) => {
                        ledger_ctx.send_write_response(resp);
                    }
                    (LedgerResponse::Write(resp), Some(responder)) => {
                        ledger_ctx.send_write_response(resp.clone());
                        let _ = responder.send(LedgerResponse::Write(resp));
                    }
                    (LedgerResponse::Read(id, resp), None) => {
                        ledger_ctx.send_read_response(id, resp);
                    }
                    (LedgerResponse::Read(id, resp), Some(responder)) => {
                        ledger_ctx.send_read_response(id, resp.clone());
                        let _ = responder.send(LedgerResponse::Read(id, resp));
                    }
                    (resp, Some(responder)) => {
                        let _ = responder.send(resp);
                    }
                    (_, None) => {}
                }
            }
            ledger_ctx
        };
        let join_handle = thread::Builder::new()
            .name("ledger-manager".into())
            .spawn(ledger_manager_loop)
            .expect("Failed: ledger manager");
        LedgerManager {
            caller,
            join_handle,
        }
    }

    pub fn pending_calls(&self) -> usize {
        self.caller.0.len()
    }

    pub(super) fn call(&self, request: LedgerRequest) {
        self.caller.call(request)
    }

    pub(super) fn call_sync(&self, request: LedgerRequest) -> LedgerResponse {
        self.caller.call_sync(request)
    }

    pub async fn wait_for_stop(self) -> thread::Result<LedgerCtx> {
        self.join_handle.join()
    }

    pub fn insert_genesis_ledger(&self, mask: Mask) {
        self.call(LedgerRequest::InsertGenesisLedger { mask });
    }

    /// Inserts genesis ledgers from another thread (I5d2: the genesis ledgers
    /// a node loads only if it bootstraps). Requests made after an insert
    /// are handled after it.
    pub fn genesis_ledger_inserter(&self) -> impl Fn(Mask) + Send + 'static {
        let caller = self.caller.clone();
        move |mask| caller.call(LedgerRequest::InsertGenesisLedger { mask })
    }

    /// How the store thread hands a load's ledgers to this ledger thread
    /// (I5d1): it waits for the answer, so that they are in place before the
    /// node hears of the load. A stopped ledger thread is an error, not the
    /// end of the process (the store thread must not exit the node).
    pub fn persist_inserter(&self) -> PersistInserter {
        let caller = self.caller.clone();
        Box::new(move |insert| {
            match caller.try_call_sync(LedgerRequest::PersistInsert { insert }) {
                Ok(LedgerResponse::PersistInserted(ms)) => Ok(ms),
                Ok(other) => Err(format_response_error("persist_insert", other)),
                Err(phase) => Err(format!("ledger thread stopped ({phase})")),
            }
        })
    }

    pub fn get_mask(&self, ledger_hash: &LedgerHash) -> Option<(Mask, bool)> {
        match self.call_sync(LedgerRequest::GetMask {
            ledger_hash: ledger_hash.clone(),
        }) {
            LedgerResponse::LedgerMask(mask) => mask,
            _ => panic!("get_mask failed"),
        }
    }

    /// PC1: the proofs the ledger thread holds, for
    /// [`crate::proof_memory::ProofMemoryReport`]. It waits for the thread.
    pub fn proof_memory(&self) -> crate::proof_memory::LedgerProofMemory {
        match self.call_sync(LedgerRequest::ProofMemory) {
            LedgerResponse::ProofMemory(report) => *report,
            _ => Default::default(),
        }
    }

    pub fn get_accounts(
        &self,
        ledger_hash: &LedgerHash,
        account_ids: Vec<AccountId>,
    ) -> Result<Vec<Account>, String> {
        // TODO: this should be asynchronous
        match self.call_sync(LedgerRequest::AccountsGet {
            ledger_hash: ledger_hash.clone(),
            account_ids,
        }) {
            LedgerResponse::AccountsGet(result) => result,
            response => Err(format_response_error("get_accounts", response)),
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn producers_with_delegates(
        &self,
        ledger_hash: &LedgerHash,
        filter: fn(&CompressedPubKey) -> bool,
    ) -> Option<BTreeMap<AccountPublicKey, Vec<(ledger::AccountIndex, AccountPublicKey, u64)>>>
    {
        match self.call_sync(LedgerRequest::GetProducersWithDelegates {
            ledger_hash: ledger_hash.clone(),
            filter,
        }) {
            LedgerResponse::ProducersWithDelegatesMap(map) => map,
            _ => panic!("producers_with_delegates failed"),
        }
    }
}

// A closed request channel or dropped reply means the ledger worker is gone.
// Continuing would leave reducer requests pending or lose writes. Exit the
// entire native node, even when the caller is a reconstruction callback thread.
#[cold]
fn ledger_worker_stopped(phase: &str) -> ! {
    use std::io::Write;

    let message = format!("Fatal: ledger-manager worker stopped ({phase}); the node cannot continue; exiting with status 1");
    mina_core::log::inner::error!("{message}");
    // Keep the diagnostic visible even when no tracing subscriber is installed.
    let _ = writeln!(std::io::stderr().lock(), "{message}");
    std::process::exit(1);
}

impl LedgerCaller {
    pub fn call(&self, request: LedgerRequest) {
        self.0
            .tracked_send(LedgerRequestWithChan {
                request,
                responder: None,
            })
            .unwrap_or_else(|_| ledger_worker_stopped("request channel closed"));
    }

    /// As [`Self::call`], but a stopped ledger thread is not fatal: for
    /// requests that only release something, such as the store thread's
    /// unpin, which may come while the node shuts down.
    pub(super) fn try_call(&self, request: LedgerRequest) -> bool {
        self.0
            .tracked_send(LedgerRequestWithChan {
                request,
                responder: None,
            })
            .is_ok()
    }

    pub(super) fn call_sync(&self, request: LedgerRequest) -> LedgerResponse {
        self.try_call_sync(request)
            .unwrap_or_else(|phase| ledger_worker_stopped(phase))
    }

    /// As [`Self::call_sync`], but a stopped ledger thread is an error, not
    /// the end of the process: for the frontier store's thread, which must
    /// not be able to exit the node. It turns persistence off instead.
    pub(super) fn try_call_sync(
        &self,
        request: LedgerRequest,
    ) -> Result<LedgerResponse, &'static str> {
        let (responder, receiver) = std::sync::mpsc::sync_channel(0);
        self.0
            .tracked_send(LedgerRequestWithChan {
                request,
                responder: Some(responder),
            })
            .map_err(|_| "request channel closed")?;
        receiver.recv().map_err(|_| "reply channel closed")
    }

    /// A caller of a ledger thread that has stopped: before it took the
    /// request (`before_request`), or after, without answering.
    #[cfg(test)]
    pub(super) fn stopped(before_request: bool) -> Self {
        let (sender, mut receiver) = mpsc::tracked_unbounded_channel();
        if before_request {
            drop(receiver);
        } else {
            thread::spawn(move || while receiver.blocking_recv().is_some() {});
        }
        Self(sender)
    }

    /// A caller whose requests the test takes itself, without a ledger
    /// thread: the function gives the next one sent, if any.
    #[cfg(test)]
    pub(super) fn queued() -> (Self, impl FnMut() -> Option<LedgerRequest>) {
        let (sender, mut receiver) = mpsc::tracked_unbounded_channel();
        let next = move || {
            receiver
                .try_recv()
                .ok()
                .map(|message: mpsc::Tracked<LedgerRequestWithChan>| message.0.request)
        };
        (Self(sender), next)
    }
}

fn format_response_error(method: &str, res: LedgerResponse) -> String {
    format!("LedgerManager::{method}: unexpected response: {res:?}")
}

impl<T: LedgerService> TransitionFrontierSyncLedgerSnarkedService for T {
    fn contains_snarked_ledger(&self, hash: &LedgerHash) -> bool {
        matches!(
            self.ledger_manager()
                .call_sync(LedgerRequest::ContainsSnarkedLedger { hash: hash.clone() }),
            LedgerResponse::ContainsSnarkedLedger(true)
        )
    }

    fn compute_snarked_ledger_hashes(
        &self,
        snarked_ledger_hash: &LedgerHash,
    ) -> Result<(), String> {
        match self
            .ledger_manager()
            .call_sync(LedgerRequest::ComputeSnarkedLedgerHashes {
                snarked_ledger_hash: snarked_ledger_hash.clone(),
            }) {
            LedgerResponse::Success => Ok(()),
            res => Err(format_response_error("compute_snarked_ledger_hashes", res)),
        }
    }

    fn copy_snarked_ledger_contents_for_sync(
        &self,
        origin_snarked_ledger_hash: Vec<LedgerHash>,
        target_snarked_ledger_hash: LedgerHash,
        overwrite: bool,
    ) -> Result<bool, String> {
        match self
            .ledger_manager()
            .call_sync(LedgerRequest::CopySnarkedLedgerContentsForSync {
                origin_snarked_ledger_hash,
                target_snarked_ledger_hash,
                overwrite,
            }) {
            LedgerResponse::SnarkedLedgerContentsCopied(copied) => copied,
            res => Err(format_response_error(
                "copy_snarked_ledger_contents_for_sync",
                res,
            )),
        }
    }

    fn child_hashes_get(
        &self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
        depth: usize,
    ) -> Result<Vec<LedgerHash>, String> {
        match self
            .ledger_manager()
            .call_sync(LedgerRequest::ChildHashesGet {
                snarked_ledger_hash,
                parent: parent.clone(),
                depth,
            }) {
            LedgerResponse::ChildHashes(Some(res)) => Ok(res),
            res => Err(format_response_error("child_hashes_get", res)),
        }
    }

    fn accounts_set(
        &self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
        expected_hash: &LedgerHash,
        accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    ) -> Result<LedgerHash, String> {
        match self.ledger_manager().call_sync(LedgerRequest::AccountsSet {
            snarked_ledger_hash,
            parent: parent.clone(),
            expected_hash: expected_hash.clone(),
            accounts,
        }) {
            LedgerResponse::AccountsSet(res) => res,
            res => Err(format_response_error("accounts_set", res)),
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn account_reads_distinguish_an_unavailable_ledger_from_empty() {
        use ledger::BaseLedger;
        let mut ctx = LedgerCtx::default();
        let (sender, _receiver) = mpsc::tracked_unbounded_channel();
        let caller = LedgerCaller(sender);
        let mut mask = Mask::new_unattached(super::super::LEDGER_DEPTH);
        let hash: LedgerHash = mask.merkle_root().into();
        let read = |hash| LedgerRequest::AccountsGet {
            ledger_hash: hash,
            account_ids: vec![],
        };
        assert!(matches!(read(hash.clone()).handle(&mut ctx, &caller),
            LedgerResponse::AccountsGet(Err(error)) if error == "Ledger unavailable"));
        ctx.insert_genesis_ledger(mask);
        assert!(matches!(read(hash).handle(&mut ctx, &caller),
            LedgerResponse::AccountsGet(Ok(accounts)) if accounts.is_empty()));
    }

    /// An account query whose token id is no field element (one built in
    /// code rather than decoded, which the decoder refuses) answers no
    /// accounts instead of stopping the ledger thread.
    #[test]
    fn account_queries_with_an_out_of_field_token_id_answer_nothing() {
        use ledger::BaseLedger;
        use mina_core::requests::RpcId;
        use mina_p2p_messages::{
            bigint::BigInt,
            v2::{MinaBaseAccountIdDigestStableV1, TokenIdKeyHash},
        };
        let mut ctx = LedgerCtx::default();
        let mut mask = Mask::new_unattached(super::super::LEDGER_DEPTH);
        let hash: LedgerHash = mask.merkle_root().into();
        ctx.insert_genesis_ledger(mask);
        let token = TokenIdKeyHash::from(MinaBaseAccountIdDigestStableV1(BigInt::from_bytes(
            [0xff; 32],
        )));
        let key: AccountPublicKey = "B62qkAhMYM7NWh2M5MABFwpNoqgzoweHr3bv6srWM43bwQMh2pVG82U"
            .parse()
            .unwrap();
        for query in [
            AccountQuery::TokenOwner(token.clone()),
            AccountQuery::PubKeyWithTokenId(key, token.clone()),
        ] {
            let response = LedgerRequest::read_for_tests(
                &mut ctx,
                LedgerReadRequest::AccountsForRpc(RpcId::new_unchecked(0, 0), hash.clone(), query),
            );
            assert!(matches!(
                response,
                LedgerReadResponse::AccountsForRpc(_, accounts, _) if accounts.is_empty()
            ));
        }
    }

    #[test]
    fn dead_ledger_worker_exits() {
        const MODE: &str = "MINA_I36_DEAD_LEDGER_TEST";
        if let Ok(mode) = std::env::var(MODE) {
            let (sender, mut receiver) = mpsc::tracked_unbounded_channel();
            let caller = LedgerCaller(sender);
            if mode == "reply" {
                std::thread::spawn(move || {
                    drop(receiver.blocking_recv().unwrap());
                });
            } else {
                drop(receiver);
            }
            match mode.as_str() {
                "async" => caller.call(LedgerRequest::GetMask {
                    ledger_hash: LedgerHash::zero(),
                }),
                "sync" | "reply" => {
                    caller.call_sync(LedgerRequest::GetMask {
                        ledger_hash: LedgerHash::zero(),
                    });
                }
                "accounts" => {
                    let manager = LedgerManager {
                        caller,
                        join_handle: thread::spawn(LedgerCtx::default),
                    };
                    let _ = manager.get_accounts(&LedgerHash::zero(), vec![]);
                }
                _ => panic!("unknown child test mode"),
            }
            panic!("dead ledger worker call returned");
        }

        for mode in ["async", "sync", "reply", "accounts"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ledger::ledger_manager::tests::dead_ledger_worker_exits",
                    "--nocapture",
                ])
                .env(MODE, mode)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(1), "{mode}: {stderr}");
            assert!(
                stderr.contains("Fatal: ledger-manager worker stopped"),
                "{mode}: {stderr}"
            );
            let phase = if mode == "reply" {
                "reply channel closed"
            } else {
                "request channel closed"
            };
            assert!(stderr.contains(phase), "{mode}: {stderr}");
            assert!(!stderr.contains("panicked"), "{mode}: {stderr}");
        }
    }
}

#[cfg(test)]
mod sync_query_tests {
    use super::*;

    #[test]
    fn network_child_hashes_refuse_partial_sync_masks() {
        let mut ctx = LedgerCtx::default();
        let target = LedgerHash::zero();
        ctx.copy_snarked_ledger_contents_for_sync(&[], target.clone(), false)
            .unwrap();
        let (sender, _receiver) = mpsc::tracked_unbounded_channel();
        let caller = LedgerCaller(sender);
        // The local comparison path needs these hashes while syncing.
        assert!(matches!(
            LedgerRequest::ChildHashesGet {
                snarked_ledger_hash: target.clone(),
                parent: LedgerAddress::root(),
                depth: 1,
            }
            .handle(&mut ctx, &caller),
            LedgerResponse::ChildHashes(Some(_))
        ));
        assert!(matches!(
            LedgerRequest::Read(
                LedgerReadId::new_unchecked(0, 0),
                LedgerReadRequest::GetChildHashesAtAddr(target, LedgerAddress::root(), 1,)
            )
            .handle(&mut ctx, &caller),
            LedgerResponse::Read(_, LedgerReadResponse::GetChildHashesAtAddr(None))
        ));
    }
}
