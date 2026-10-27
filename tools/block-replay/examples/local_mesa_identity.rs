//! Compute a private Mesa genesis identity using the node's actual loader.
//! No sockets or producer keys are needed. Supports mainnet and devnet profiles.

use std::{fs::File, path::PathBuf};

use anyhow::Context;
use clap::Parser;
use mina_core::{constants, ChainId, NetworkConfig};
use mina_node::transition_frontier::genesis::GenesisConfig;
use mina_p2p_messages::v2;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "mainnet", value_parser = ["mainnet", "devnet"])]
    network: String,
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Read the compact GenesisConfigLoaded format, without loading ledgers.
    #[arg(long)]
    compact: bool,
    /// Only exercise the genesis loader, without preparing circuits or networking.
    #[arg(long)]
    load_only: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    let config = if args.compact {
        GenesisConfig::from_bytes(std::fs::read(&args.config)?)?
    } else {
        GenesisConfig::DaemonJsonFile(args.config)
    };
    // The runtime fork decides the genesis state; a fork other than the
    // release's also needs its own blockchain circuits for the chain id.
    config.initialize_runtime().context("runtime constants")?;
    let (_masks, loaded) = config.load().context("load private genesis")?;
    if args.load_only {
        serde_json::to_writer_pretty(File::create(args.out)?, &loaded)?;
        println!(
            "genesis ledger {}; staking {}; next {}",
            loaded.genesis_ledger_hash,
            loaded.staking_epoch_ledger_hash,
            loaded.next_epoch_ledger_hash
        );
        return Ok(());
    }
    let (_, state, genesis_hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;
    let zero = || v2::CurrencyAmountStableV1(0u64.into());
    let (_, _, chain_id_genesis_hash) = loaded.protocol_states(zero(), zero(), zero())?;
    let fork_circuits = ledger::proofs::fork_circuits::prepare_runtime_fork(
        &chain_id_genesis_hash,
        &loaded.constants,
    )
    .map_err(anyhow::Error::msg)?;
    let chain_id = ChainId::compute(
        mina_core::network::constraint_system_digests(),
        &chain_id_genesis_hash,
        &state.body.constants,
        constants::PROTOCOL_TRANSACTION_VERSION,
        constants::PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(constants::TX_POOL_MAX_SIZE),
    );
    anyhow::ensure!(chain_id != mina_core::MAINNET_CHAIN_ID, "public mainnet ID");
    anyhow::ensure!(chain_id != mina_core::DEVNET_CHAIN_ID, "public devnet ID");
    let result = serde_json::json!({
        "network": args.network,
        "genesis_state_hash": genesis_hash,
        "chain_id_genesis_hash": chain_id_genesis_hash,
        "chain_id": chain_id.to_hex(),
        "genesis_ledger_hash": loaded.genesis_ledger_hash,
        "blockchain_step_digest": mina_core::network::constraint_system_digests()[5]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "runtime_fork_circuits": fork_circuits.map(|f| serde_json::json!({
            "step": f.step_blockchain_gates,
            "wrap": f.wrap_blockchain_gates,
        })),
        "state": state,
    });
    serde_json::to_writer_pretty(File::create(args.out)?, &result)?;
    println!("genesis {genesis_hash}; chain ID {chain_id}");
    Ok(())
}
