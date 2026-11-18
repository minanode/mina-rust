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

    /// [`Self::retired_root_scan_states`] for a node with these p2p
    /// settings. A seed-only node that doesn't share, as the web node is by
    /// default, dials only its seeds and refuses incoming connections, so it
    /// serves no other peer's bootstrap: it keeps none. Any other node keeps
    /// them all.
    pub fn retired_root_scan_states_for(p2p: &crate::p2p::P2pConfig) -> Option<usize> {
        (p2p.seed_only && p2p.sharing == crate::p2p::P2pSharing::Off).then_some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::{identity::SecretKey, P2pConfig, P2pSharing};

    fn p2p(seed_only: bool, sharing: P2pSharing) -> P2pConfig {
        P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::deterministic(0).public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: Default::default(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            seed_only,
            sharing,
            meshsub: Default::default(),
            catchup_rpc: Default::default(),
        }
    }

    /// The web node builder sets `seed_only` from its default, seed-only
    /// peering, and `sharing` from the visitor's choice. Native nodes never
    /// set `seed_only`.
    #[test]
    fn only_a_seed_only_node_without_sharing_keeps_no_retired_scan_states() {
        let kept = |p2p: P2pConfig| TransitionFrontierConfig::retired_root_scan_states_for(&p2p);
        // The web node by default: sharing off.
        assert_eq!(kept(p2p(true, P2pSharing::Off)), Some(0));
        // The web node with sharing on serves other browsers.
        assert_eq!(kept(p2p(true, P2pSharing::peer())), None);
        // The web node with the experimental unrestricted peering, and
        // native nodes, relays included.
        assert_eq!(kept(p2p(false, P2pSharing::Off)), None);
        assert_eq!(kept(p2p(false, P2pSharing::relay())), None);
    }
}
