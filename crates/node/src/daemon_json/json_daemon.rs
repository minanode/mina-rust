use ledger::scan_state::currency::Slot;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Daemon {
    txpool_max_size: Option<usize>,
    peer_list_url: Option<String>,
    slot_tx_end: Option<u32>,
    slot_chain_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hard_fork_genesis_slot_delta: Option<u32>,
}

impl Daemon {
    pub fn stop_slots(&self) -> mina_core::constants::StopSlots {
        mina_core::constants::StopSlots {
            slot_tx_end: self.slot_tx_end,
            slot_chain_end: self.slot_chain_end,
            hard_fork_genesis_slot_delta: self.hard_fork_genesis_slot_delta,
        }
    }

    pub const DEFAULT: Daemon = Daemon {
        txpool_max_size: Some(3000),
        peer_list_url: None,
        slot_tx_end: None,
        slot_chain_end: None,
        hard_fork_genesis_slot_delta: None,
    };

    pub fn tx_pool_max_size(&self) -> usize {
        self.txpool_max_size
            .unwrap_or(Self::DEFAULT.txpool_max_size.unwrap())
    }

    pub fn peer_list_url(&self) -> Option<String> {
        self.peer_list_url.clone()
    }

    pub fn slot_tx_end(&self) -> Option<Slot> {
        self.slot_tx_end.map(Slot::from_u32)
    }

    pub fn slot_chain_end(&self) -> Option<Slot> {
        self.slot_chain_end.map(Slot::from_u32)
    }
}
