use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{genesis::TransitionFrontierGenesisConfig, persist::TransitionFrontierPersistConfig};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransitionFrontierConfig {
    pub genesis: Arc<TransitionFrontierGenesisConfig>,
    /// The frontier store (R20), off by default.
    #[serde(default)]
    pub persist: TransitionFrontierPersistConfig,
    /// The full genesis config, when `genesis` is the one the node kept of it
    /// (I5d2, `GenesisCache`): its ledgers were not loaded at startup, and
    /// are loaded only if the node bootstraps, as a restore from the
    /// frontier store replaces them.
    #[serde(default)]
    pub genesis_ledgers_deferred: Option<Arc<TransitionFrontierGenesisConfig>>,
    /// How many of the newest retired roots keep their scan state and
    /// pending coinbase, from which the node serves their staged ledger to
    /// peers that bootstrap from them. `None` keeps them for every root in
    /// the root history, `k + 10`, as OCaml does. With `Some(0)` a peer that
    /// asks for a retired root's staged ledger gets the answer an unknown
    /// root gets. The root history keeps the retired roots' blocks either
    /// way.
    #[serde(default)]
    pub retired_root_scan_states: Option<usize>,
}

impl TransitionFrontierConfig {
    pub fn new(genesis: Arc<TransitionFrontierGenesisConfig>) -> Self {
        TransitionFrontierConfig {
            genesis,
            persist: Default::default(),
            genesis_ledgers_deferred: None,
            retired_root_scan_states: None,
        }
    }
}
