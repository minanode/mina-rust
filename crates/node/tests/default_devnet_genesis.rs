//! Keep devnet's default genesis and chain id unchanged. Network and runtime
//! constants are process globals, so this runs separately from mainnet tests.
use mina_core::{
    constants::{PROTOCOL_NETWORK_VERSION, PROTOCOL_TRANSACTION_VERSION, TX_POOL_MAX_SIZE},
    network::{devnet, NetworkId},
    ChainId, NetworkConfig, DEVNET_CHAIN_ID,
};
use mina_p2p_messages::v2;

/// The pinned id is from `mina internal chain-id` with OCaml's Mesa 4.0.2
/// devnet profile and config; see `mina_core::DEVNET_CHAIN_ID`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/chain_id/chain_id.ml#L44-L70>
#[test]
fn default_devnet_genesis_has_ocaml_chain_id() {
    NetworkConfig::init("devnet").unwrap();
    let config = mina_node::config::default_genesis_config(&NetworkId::TESTNET);
    let (masks, loaded) = config.load().unwrap();
    assert!(masks.is_empty());
    let zero = || v2::CurrencyAmountStableV1(0u64.into());
    let (_, genesis, genesis_hash) = loaded.protocol_states(zero(), zero(), zero()).unwrap();
    let chain_id = ChainId::compute(
        &devnet::CONSTRAINT_SYSTEM_DIGESTS,
        &genesis_hash,
        &genesis.body.constants,
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    assert_eq!(chain_id, DEVNET_CHAIN_ID);
}
