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
}

impl TransitionFrontierConfig {
    pub fn new(genesis: Arc<TransitionFrontierGenesisConfig>) -> Self {
        TransitionFrontierConfig {
            genesis,
            persist: Default::default(),
            genesis_ledgers_deferred: None,
        }
    }
}
