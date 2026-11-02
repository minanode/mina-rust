//! JSON-lines adapter driving the block producer through multi-step traces
//! for the compiled-OCaml differential harness (`tools/consensus-diff`).
//!
//! Each request is one trace: a synthetic producer key and delegator table,
//! then steps that set the clock, the best tip (and optionally the root),
//! whether the staking ledger is available, and whether the node restarts.
//! The node's own reducers and enabling conditions run on every action, and
//! the VRF evaluator service's worker answers VRF requests, with its disk
//! cache in a private temporary directory kept across restarts. Requests
//! still in flight when a step changes the tip are answered late, at the
//! start of the next step. Each step reports the epoch data the producer is
//! bound to for the clock's epoch and the next one, its wins in the step's
//! window that a block on the tip can use, whether it starts a block for the
//! clock's slot, and invariant violations.
//!
//! ```text
//! cargo run -p mina-node-common --example producer_trace -- NETWORK
//! ```
//!
//! Accepts only public synthetic scalar identifiers, never a key file.

#[cfg(not(target_family = "wasm"))]
fn main() -> anyhow::Result<()> {
    trace::main()
}

#[cfg(target_family = "wasm")]
fn main() {}

#[cfg(not(target_family = "wasm"))]
mod trace {
    use std::{
        collections::VecDeque,
        io::{self, BufRead, Write},
        sync::Arc,
    };

    use anyhow::{anyhow, bail, Context, Result};
    use ark_ff::PrimeField;
    use ledger::AccountIndex;
    use mina_curves::pasta::{Fp, Fq};
    use mina_node::{
        account::AccountPublicKey,
        block_producer::{
            vrf_evaluator::{
                BlockProducerVrfEvaluatorAction as VrfAction, DelegatorTable, EpochFingerprint,
                VrfEvaluatorInput,
            },
            BlockProducerAction, BlockProducerConfig, BlockProducerCurrentState,
        },
        block_producer_effectful::{
            vrf_evaluator_effectful::BlockProducerVrfEvaluatorEffectfulAction as VrfEffect,
            BlockProducerEffectfulAction,
        },
        core::{
            block::{AppliedBlock, ArcBlockWithHash},
            consensus::ConsensusConstants,
            constants::{constraint_constants, PROTOCOL_CONSTANTS},
        },
        ledger::{
            read::{LedgerReadAction, LedgerReadId, LedgerReadRequest, LedgerReadResponse},
            LedgerConfig,
        },
        ledger_effectful::LedgerEffectfulAction,
        p2p::{identity::SecretKey as P2pSecretKey, P2pConfig, P2pMeshsubConfig},
        snark::{get_srs, BlockVerifier, SnarkConfig, TransactionVerifier},
        transition_frontier::{
            genesis::{GenesisConfigLoaded, TransitionFrontierGenesisState},
            TransitionFrontierConfig,
        },
        Action, BuildEnv, Config, GlobalConfig, State, DEVNET_CONFIG, MAINNET_CONFIG,
    };
    use mina_node_common::block_producer::VrfEvaluatorWorker;
    use mina_p2p_messages::v2::{
        self, ConsensusProofOfStakeDataConsensusStateValueStableV2 as ConsensusState,
    };
    use mina_signer::{Keypair, SecKey};
    use redux::{ActionMeta, Dispatcher, EnablingCondition, Timestamp};
    use serde_json::{json, Value};

    const SLOT_NS: u64 = 90_000_000_000;

    /// The synthetic scalar `id + 1`, as the OCaml hook's `key_id`.
    fn key(id: u64) -> Keypair {
        Keypair::try_from(SecKey::new(Fq::from(id + 1))).unwrap()
    }

    fn payload(data: Option<&EpochFingerprint>) -> Value {
        data.map_or(Value::Null, |data| {
            json!({"seed": data.seed, "ledger": data.ledger,
                "total_currency": data.total_currency.to_string()})
        })
    }

    struct Node {
        state: State,
        worker: VrfEvaluatorWorker,
        tables: VecDeque<(LedgerReadId, LedgerReadRequest)>,
        requests: VecDeque<VrfEvaluatorInput>,
        /// Whether the last table response held a table.
        table_present: bool,
        violations: Vec<String>,
    }

    struct Trace {
        network: String,
        keypair: Keypair,
        table: Arc<DelegatorTable>,
        genesis: ArcBlockWithHash,
        cache: tempfile::TempDir,
        node: Node,
        tip: Option<ArcBlockWithHash>,
        root: Option<ArcBlockWithHash>,
        tips: u64,
    }

    fn genesis_block() -> Result<ArcBlockWithHash> {
        let amount = |nanomina: u64| v2::CurrencyAmountStableV1(nanomina.into());
        let hash = v2::LedgerHash::from_fp(Fp::from(1u64));
        let loaded = GenesisConfigLoaded {
            runtime_config: Default::default(),
            constants: PROTOCOL_CONSTANTS.clone(),
            genesis_ledger_hash: hash.clone(),
            genesis_total_currency: amount(1_000_000_000),
            genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2 {
                indexes: std::iter::empty().collect(),
                depth: 35u64.into(),
                tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(hash.clone()),
            },
            staking_epoch_ledger_hash: hash.clone(),
            staking_epoch_total_currency: amount(1_000_000_000),
            staking_epoch_seed: v2::EpochSeed::zero(),
            next_epoch_ledger_hash: hash,
            next_epoch_total_currency: amount(1_000_000_000),
            next_epoch_seed: v2::EpochSeed::zero(),
        };
        let (_, state, hash) = loaded
            .protocol_states(
                loaded.genesis_total_currency.clone(),
                loaded.staking_epoch_total_currency.clone(),
                loaded.next_epoch_total_currency.clone(),
            )
            .map_err(|e| anyhow!("{e:?}"))?;
        TransitionFrontierGenesisState::build_block_with_dummy_proof(&state, &hash)
            .map_err(|e| anyhow!("{e:?}"))
    }

    impl Node {
        fn new(network: &str, producer: AccountPublicKey, genesis: &ArcBlockWithHash) -> Self {
            let consensus_constants =
                ConsensusConstants::create(constraint_constants(), &PROTOCOL_CONSTANTS);
            let genesis_config = if network == "mainnet" {
                MAINNET_CONFIG.clone()
            } else {
                DEVNET_CONFIG.clone()
            };
            let config = Config {
                ledger: LedgerConfig {},
                snark: SnarkConfig {
                    block_verifier_index: BlockVerifier::make(),
                    block_verifier_srs: get_srs(),
                    work_verifier_index: TransactionVerifier::make(),
                    work_verifier_srs: get_srs(),
                },
                p2p: P2pConfig {
                    libp2p_port: None,
                    listen_port: None,
                    identity_pub_key: P2pSecretKey::deterministic(0).public_key(),
                    initial_peers: Vec::new(),
                    external_addrs: Vec::new(),
                    enabled_channels: Default::default(),
                    timeouts: Default::default(),
                    limits: Default::default(),
                    peer_discovery: false,
                    seed_only: false,
                    sharing: Default::default(),
                    meshsub: P2pMeshsubConfig::default(),
                    catchup_rpc: Default::default(),
                },
                transition_frontier: TransitionFrontierConfig::new(genesis_config),
                archive: None,
                block_producer: Some(BlockProducerConfig::new(producer.into())),
                global: GlobalConfig {
                    snark_work_fee: None,
                    build: BuildEnv::get().into(),
                    snarker: None,
                    consensus_constants: consensus_constants.clone(),
                    client_port: None,
                    testing_run: true,
                    chain_id_override: None,
                    skip_proof_verification: false,
                    worker_control: false,
                    snarker_strategy: Default::default(),
                    block_first_lead: None,
                },
                tx_pool: ledger::transaction_pool::Config {
                    trust_system: (),
                    pool_max_size: 3000,
                    slot_tx_end: None,
                },
            };
            let now = genesis.genesis_timestamp();
            let mut state = State::new(config, &consensus_constants, now);
            state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
                time: now,
                genesis: genesis.clone(),
            };
            state.transition_frontier.best_chain = vec![AppliedBlock {
                block: genesis.clone(),
                just_emitted_a_proof: false,
            }];
            let producer = state.block_producer.as_mut().unwrap();
            producer.vrf_evaluator.genesis_timestamp = genesis.genesis_timestamp();
            Self {
                state,
                // Replaced with the trace's key and cache before use.
                worker: VrfEvaluatorWorker::new(key(0), None),
                tables: VecDeque::new(),
                requests: VecDeque::new(),
                table_present: false,
                violations: Vec::new(),
            }
        }

        /// Runs `action` as the store would once enabled, recursively, and
        /// stops at the ledger and VRF service boundaries.
        fn dispatch(&mut self, action: impl Into<Action>) {
            let action = action.into();
            let now = self.state.time();
            if !action.is_enabled(&self.state, now) {
                return;
            }
            match action {
                Action::BlockProducer(_) | Action::Ledger(_) | Action::CheckTimeouts(_) => {
                    let mut dispatcher = Dispatcher::new();
                    mina_node::reducer(
                        &mut self.state,
                        &ActionMeta::zero_custom(now).with_action(action),
                        &mut dispatcher,
                    );
                    let next: Vec<_> = std::iter::from_fn(|| dispatcher.pop()).collect();
                    for action in next {
                        self.dispatch(action);
                    }
                }
                Action::LedgerEffects(LedgerEffectfulAction::ReadInit { id, request, .. }) => {
                    self.tables.push_back((id, request.clone()));
                    self.dispatch(LedgerReadAction::Pending { id, request });
                }
                Action::BlockProducerEffectful(BlockProducerEffectfulAction::VrfEvaluator(
                    VrfEffect::EvaluateSlot { vrf_input },
                )) => {
                    if !self.table_present {
                        self.violations.push(format!(
                            "VRF request for slot {} after a missing table",
                            vrf_input.global_slot
                        ));
                    }
                    self.requests.push_back(vrf_input);
                }
                _ => (),
            }
        }

        fn set_time(&mut self, time: Timestamp) {
            let mut dispatcher = Dispatcher::new();
            mina_node::reducer(
                &mut self.state,
                &ActionMeta::zero_custom(time)
                    .with_action(Action::from(mina_node::CheckTimeoutsAction {})),
                &mut dispatcher,
            );
        }

        fn answer(&mut self, input: VrfEvaluatorInput) {
            let output = self.worker.evaluate(&input);
            self.dispatch(VrfAction::ProcessSlotEvaluationSuccess {
                vrf_output: output.evaluation_result,
                staking_ledger_hash: output.staking_ledger_hash,
                generation: output.generation,
            });
        }

        /// Answers table reads, and VRF requests below `window_end`.
        fn serve(&mut self, table: &Arc<DelegatorTable>, ledger: bool, window_end: u32) {
            loop {
                if let Some((id, request)) = self.tables.pop_front() {
                    let response = match request {
                        LedgerReadRequest::DelegatorTable(..) => {
                            LedgerReadResponse::DelegatorTable(ledger.then(|| (**table).clone()))
                        }
                        other => {
                            self.violations.push(format!("unexpected read {other:?}"));
                            continue;
                        }
                    };
                    self.table_present = ledger;
                    self.dispatch(LedgerReadAction::Success { id, response });
                } else if self
                    .requests
                    .front()
                    .is_some_and(|input| input.global_slot < window_end)
                {
                    let input = self.requests.pop_front().unwrap();
                    self.answer(input);
                } else {
                    break;
                }
            }
        }
    }

    impl Trace {
        fn new(network: &str, case: &Value) -> Result<Self> {
            let keypair = key(case["key_id"].as_u64().context("key_id")?);
            let producer = AccountPublicKey::from(keypair.public.clone());
            let table: DelegatorTable = case["delegators"]
                .as_array()
                .context("delegators")?
                .iter()
                .map(|d| -> Result<_> {
                    Ok((
                        AccountIndex(d["index"].as_u64().context("index")?),
                        (
                            producer.clone(),
                            d["stake"].as_str().context("stake")?.parse()?,
                        ),
                    ))
                })
                .collect::<Result<_>>()?;
            let genesis = genesis_block()?;
            let cache = tempfile::tempdir()?;
            let mut node = Node::new(network, producer, &genesis);
            node.worker = VrfEvaluatorWorker::new(keypair.clone(), Some(cache.path().into()));
            Ok(Self {
                network: network.to_owned(),
                keypair,
                table: Arc::new(table),
                genesis,
                cache,
                node,
                tip: None,
                root: None,
                tips: 0,
            })
        }

        fn block(&mut self, state: &Value) -> Result<ArcBlockWithHash> {
            let consensus: ConsensusState = serde_json::from_value(state.clone())?;
            let mut block = self.genesis.clone();
            Arc::make_mut(&mut block.block)
                .header
                .protocol_state
                .body
                .consensus_state = consensus;
            // A distinct synthetic hash per supplied state.
            self.tips += 1;
            block.hash = v2::StateHash::from_fp(Fp::from(1_000 + self.tips));
            Ok(block)
        }

        fn step(&mut self, step: &Value) -> Result<Value> {
            if step["restart"].as_bool().unwrap_or(false) {
                // A new process: its in-flight requests are gone, its disk
                // cache stays.
                let producer = AccountPublicKey::from(self.keypair.public.clone());
                self.node = Node::new(&self.network, producer, &self.genesis);
                self.node.worker =
                    VrfEvaluatorWorker::new(self.keypair.clone(), Some(self.cache.path().into()));
            }
            let clock = step["clock"].as_u64().context("clock")? as u32;
            let offset_s = step["offset_s"].as_u64().unwrap_or(1);
            let time = self
                .genesis
                .genesis_timestamp()
                .checked_add(u64::from(clock) * SLOT_NS + offset_s * 1_000_000_000)
                .context("time")?;
            self.node.set_time(time);
            let ledger = step["ledger"].as_bool().unwrap_or(true);
            let window = step["window"].as_u64().unwrap_or(6) as u32;
            let epoch = clock / 7140;
            let window_end = clock.saturating_add(window).min((epoch + 1) * 7140);
            let hits = self.node.worker.cache_hits();

            if !step["root"].is_null() {
                self.root = Some(self.block(&step["root"])?);
            }
            let in_flight: Vec<_> = self.node.requests.drain(..).collect();
            let tip = if !step["tip"].is_null() {
                Some(self.block(&step["tip"])?)
            } else if step["restart"].as_bool().unwrap_or(false) {
                self.tip.clone()
            } else {
                None
            };
            if let Some(tip) = tip {
                self.tip = Some(tip.clone());
                let mut chain = vec![];
                if let Some(root) = self.root.clone() {
                    chain.push(AppliedBlock {
                        block: root,
                        just_emitted_a_proof: false,
                    });
                }
                chain.push(AppliedBlock {
                    block: tip.clone(),
                    just_emitted_a_proof: false,
                });
                self.node.state.transition_frontier.best_chain = chain;
                self.node
                    .dispatch(BlockProducerAction::BestTipUpdate { best_tip: tip });
            } else if self.tip.is_none() {
                bail!("a step without a tip needs an earlier tip");
            }
            // As on every timeout tick.
            self.node.dispatch(BlockProducerAction::EvaluationRecheck);
            // Responses still in flight from before this step arrive late.
            for input in in_flight {
                self.node.answer(input);
            }
            self.node.serve(&self.table.clone(), ledger, window_end);

            let tip = self.tip.clone().context("no tip")?;
            let vrf = self.node.state.block_producer.vrf_evaluator().unwrap();
            let bound = |epoch: u32| {
                payload(
                    vrf.target()
                        .filter(|t| t.epoch == epoch)
                        .or_else(|| vrf.completed_epoch(epoch)),
                )
            };
            // Every retained win in the window: one kept from data the tip no
            // longer selects would not match the oracle's wins.
            let stale = vrf
                .won_slots
                .range(clock..window_end)
                .filter(|(slot, won)| {
                    won.epoch_data.is_none()
                        || won.epoch_data
                            != EpochFingerprint::for_child(tip.consensus_state(), *slot / 7140)
                })
                .count();
            let wins: Vec<Value> = vrf
                .won_slots
                .range(clock..window_end)
                .map(|(slot, won)| {
                    json!({"slot": slot,
                        "hash": won.won_slot.vrf_output.hash().into_bigint().to_string()})
                })
                .collect();
            let observed = json!({
                "bound": bound(epoch),
                "next_bound": bound(epoch + 1),
                "wins": wins,
            });
            if stale > 0 {
                self.node
                    .violations
                    .push(format!("{stale} retained wins of other epoch data"));
            }
            let produce = self.try_produce(clock);
            let node = &mut self.node;
            let mut result = observed;
            result["produce"] = json!(produce);
            result["violations"] = json!(std::mem::take(&mut node.violations));
            result["cache_hits"] = json!(node.worker.cache_hits() - hits);
            Ok(result)
        }

        /// Whether a block for the clock's slot is started on the tip: the
        /// win is found, and the block's parent passes its checks.
        fn try_produce(&mut self, clock: u32) -> Option<u32> {
            let node = &mut self.node;
            node.dispatch(BlockProducerAction::WonSlotSearch);
            let current = node.state.block_producer.current_won_slot()?.global_slot();
            if current != clock {
                return None;
            }
            node.dispatch(BlockProducerAction::WonSlotProduceInit);
            let started = matches!(
                node.state.block_producer.as_ref()?.current,
                BlockProducerCurrentState::WonSlotTransactionsGet { .. }
            );
            // Stop before the staged ledger diff: the trace has no ledger.
            let time = node.state.time();
            if let Some(producer) = node.state.block_producer.as_mut() {
                producer.current = BlockProducerCurrentState::Idle { time };
            }
            started.then_some(clock)
        }
    }

    fn run(network: &str, case: Value) -> Result<Value> {
        match case["op"].as_str() {
            Some("build_info") => Ok(json!(BuildEnv::get())),
            Some("trace") => {
                let mut trace = Trace::new(network, &case)?;
                let steps = case["steps"].as_array().context("steps")?;
                let observed = steps
                    .iter()
                    .map(|step| trace.step(step))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Value::Array(observed))
            }
            op => bail!("unknown operation: {op:?}"),
        }
    }

    pub fn main() -> Result<()> {
        let network = std::env::args().nth(1).unwrap_or_else(|| "devnet".into());
        mina_node::core::NetworkConfig::init(&network).map_err(|e| anyhow!(e))?;
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for line in io::stdin().lock().lines() {
            let response = run(&network, serde_json::from_str(&line?)?)
                .unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
            writeln!(out, "{response}")?;
            out.flush()?;
        }
        Ok(())
    }
}
