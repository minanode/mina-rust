//! Profiles a staged ledger reconstruct from a dump the node writes with
//! `MINA_DUMP_RECONSTRUCT` set (`<data dir>/debug/failed_reconstruct_ctx.binprot`)
//! and prints where the time goes, by function. `MINA_NETWORK` (`mainnet`)
//! chooses the network.
//!
//!   cargo run --release -p mina-block-replay --example profile_reconstruct -- \
//!       /tmp/relay2-node/debug/failed_reconstruct_ctx.binprot

use std::time::Instant;

use ledger::BaseLedger;
use mina_block_replay::dump::ReconstructContext;
use mina_p2p_messages::v2;

fn main() -> anyhow::Result<()> {
    let network = std::env::var("MINA_NETWORK").unwrap_or_else(|_| "mainnet".to_owned());
    mina_core::NetworkConfig::init(&network).map_err(anyhow::Error::msg)?;
    let path = std::env::args().nth(1).expect("reconstruct dump path");
    let ctx = ReconstructContext::read(path.as_ref())?;

    let started = Instant::now();
    let mut ledger = ctx.snarked_ledger()?;
    let root = ledger.merkle_root();
    println!("snarked ledger {}", v2::LedgerHash::from_fp(root));
    println!(
        "{} accounts, ledger built and hashed in {:?}",
        ctx.accounts.len(),
        started.elapsed()
    );

    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(1000)
        .build()?;
    let started = Instant::now();
    let staged_ledger = ctx.reconstruct(&ledger);
    println!("reconstructed in {:?}", started.elapsed());
    if let Err(error) = &staged_ledger {
        println!("reconstruct failed: {error:#}");
    }

    mina_block_replay::print_profile(&guard.report().build()?);
    Ok(())
}
