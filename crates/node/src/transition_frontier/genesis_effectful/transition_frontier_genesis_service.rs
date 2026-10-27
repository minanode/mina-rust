use std::sync::Arc;

use crate::{service::BlockProducerService, transition_frontier::genesis::GenesisConfig};

pub trait TransitionFrontierGenesisService: BlockProducerService {
    /// Load genesis config and genesis ledger.
    fn load_genesis(&mut self, config: Arc<GenesisConfig>);

    /// Load the ledgers of `config`, deferred at startup (I5d2), off the
    /// state machine's thread, once they are needed. `expected` is the
    /// genesis the node started with, without ledgers: if `config` gives
    /// another, the node stops. Answers with `Event::GenesisLedgersLoad`.
    fn load_genesis_ledgers(&mut self, config: Arc<GenesisConfig>, expected: Arc<GenesisConfig>);
}
