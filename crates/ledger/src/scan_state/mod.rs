pub mod conv;
pub mod currency;
pub mod fee_excess;
pub mod fee_rate;
mod parallel_scan;
pub mod pending_coinbase;
pub mod protocol_state;
#[allow(clippy::module_inception)]
pub mod scan_state;
mod shared_values;
pub mod snark_work;
pub mod transaction_logic;
pub use parallel_scan::SpacePartition;

use currency::Fee;

pub struct GenesisConstant {
    pub protocol: (),
    pub txpool_max_size: usize,
    pub num_accounts: Option<usize>,
    /// Most segments (proofs, signed single or signed pair updates) a zkApp
    /// command may have.
    pub max_zkapp_segment_per_transaction: usize,
    pub max_event_elements: usize,
    pub max_action_elements: usize,
    /// Most zkApp commands a block may have: `apply_diff` rejects a block with
    /// more.
    pub zkapp_cmd_limit_hardcap: usize,
    /// User commands with a lower fee are not well formed.
    pub minimum_user_command_fee: Fee,
}

/// Mesa values. Berkeley weighed segments with float costs against a limit of
/// 69.45 and allowed 100 event and action elements. The mainnet profile sets
/// the pool size and the minimum fee ("0.001" MINA). P11d checks every value
/// against OCaml (`crates/ledger/tests/ocaml_mainnet_constants.rs`).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/unconfigurable_constants/node_config_unconfigurable_constants.ml#L1-L9>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L27-L35>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_constants/genesis_constants.ml#L404-L424>
pub const GENESIS_CONSTANT: GenesisConstant = GenesisConstant {
    protocol: (),
    txpool_max_size: 3000,
    num_accounts: None,
    max_zkapp_segment_per_transaction: 16,
    max_event_elements: 1024,
    max_action_elements: 1024,
    zkapp_cmd_limit_hardcap: 128,
    minimum_user_command_fee: Fee::of_nanomina_int_exn(1_000_000),
};
