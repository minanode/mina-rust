//! Read precomputed blocks, the JSON form nodes publish (for example to the
//! public `mina_network_block_data` bucket), into the wire types.
//!
//! `mina_p2p_messages::v2::precomputed` reads OCaml's form, whose balance
//! changes are decimal MINA and precondition amounts nanomina. Each update is
//! checked against the digest OCaml writes next to it, which catches any field
//! read differently.

pub mod dump;

pub mod verify;

use std::{fs::File, io::Read, path::Path};

use anyhow::{bail, Context};
use ledger::scan_state::transaction_logic::zkapp_command::AccountUpdate;
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    bigint::BigInt,
    v2::{
        precomputed, MinaBaseAccountUpdateTStableV1, MinaStateProtocolStateValueStableV2,
        StagedLedgerDiffDiffStableV2,
    },
};
use serde::Deserialize;
use serde_json::Value;

pub mod compare;

/// The parts of a precomputed block that applying it needs.
#[derive(Deserialize)]
pub struct Block {
    pub protocol_state: MinaStateProtocolStateValueStableV2,
    pub staged_ledger_diff: StagedLedgerDiffDiffStableV2,
}

/// Reads `<height>-<hash>.json`, or its zstd-compressed `.json.zst`.
pub fn read_block(dir: &Path, height: u32, hash: &str) -> anyhow::Result<Block> {
    parse_block(&read_block_bytes(dir, height, hash)?)
        .with_context(|| format!("block {height} {hash}"))
}

/// The bytes of `<height>-<hash>.json`, or of its zstd-compressed `.json.zst`,
/// decompressed.
pub fn read_block_bytes(dir: &Path, height: u32, hash: &str) -> anyhow::Result<Vec<u8>> {
    read_file(&dir.join(format!("{height}-{hash}.json")))
}

/// The bytes of a block file, decompressed if it ends in `.zst`, or if only a
/// `.zst` next to it exists.
pub fn read_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let compressed = if path.extension().is_some_and(|e| e == "zst") {
        path.to_owned()
    } else {
        path.with_extension("json.zst")
    };
    let mut bytes = Vec::new();
    if compressed.exists() {
        zstd::Decoder::new(File::open(&compressed)?)?.read_to_end(&mut bytes)?;
    } else {
        File::open(path)
            .with_context(|| format!("{}", path.display()))?
            .read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

pub fn parse_block(bytes: &[u8]) -> anyhow::Result<Block> {
    parse_block_data(block_json(bytes)?)
}

/// [`parse_block`] from the block's [`block_json`].
pub fn parse_block_data(mut data: Value) -> anyhow::Result<Block> {
    precomputed::ocaml_forms_to_ours(&mut data)?;
    let block: Block = serde_path_to_error::deserialize(&data)
        .map_err(|e| anyhow::anyhow!("at `{}`: {}", e.path(), e.inner()))?;
    for_each_account_update(&mut data["staged_ledger_diff"], &mut check_digest)?;
    Ok(block)
}

/// The block object of a precomputed block, as JSON: `data` in the
/// `{ "version", "data" }` form, or the whole object. Non-UTF8 byte strings
/// use the shared precomputed reader. Amounts remain as OCaml wrote them.
pub fn block_json(bytes: &[u8]) -> anyhow::Result<Value> {
    let mut value = precomputed::ocaml_json_to_value(bytes)?;
    Ok(match value.get_mut("data") {
        Some(data) => data.take(),
        None => value,
    })
}

/// Reads a precomputed block file into [`block_json`], decompressing it first.
pub fn read_block_json(path: &Path) -> anyhow::Result<Value> {
    block_json(&read_file(path)?).with_context(|| format!("{}", path.display()))
}

/// Calls `f` on every account update (`{ account_update, account_update_digest,
/// calls }`) of every zkApp command in a staged ledger diff, depth first.
pub(crate) fn for_each_account_update(
    diff: &mut Value,
    f: &mut impl FnMut(&mut Value) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    fn forest(
        updates: &mut Value,
        f: &mut impl FnMut(&mut Value) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        for tree in updates.as_array_mut().into_iter().flatten() {
            let elt = &mut tree["elt"];
            f(elt)?;
            forest(&mut elt["calls"], f)?;
        }
        Ok(())
    }
    // `get_mut` rather than indexing, which would turn a missing second part
    // (`null`) into an object.
    let parts = diff.get_mut("diff").and_then(Value::as_array_mut);
    for part in parts.into_iter().flatten() {
        let commands = part.get_mut("commands").and_then(Value::as_array_mut);
        for command in commands.into_iter().flatten() {
            if command["data"][0] == "Zkapp_command" {
                forest(&mut command["data"][1]["account_updates"], f)?;
            }
        }
    }
    Ok(())
}

fn check_digest(update: &mut Value) -> anyhow::Result<()> {
    let wire: MinaBaseAccountUpdateTStableV1 =
        serde_json::from_value(update["account_update"].clone())?;
    let computed = AccountUpdate::try_from(&wire)?.digest();
    let expected: BigInt = serde_json::from_value(update["account_update_digest"].clone())?;
    if computed != expected.to_field::<Fp>()? {
        bail!(
            "account update of {} reads differently: digest {} here, {} in the block",
            update["account_update"]["body"]["public_key"],
            computed,
            expected.to_field::<Fp>()?
        );
    }
    Ok(())
}

/// Prints where a profile's samples fell, by function: those with the most
/// samples of their own, and with the most including callees, 25 of each or
/// `MINA_PROFILE_TOP`.
#[cfg(feature = "profile")]
pub fn print_profile(report: &pprof::Report) {
    use std::collections::{HashMap, HashSet};

    let (mut own, mut inclusive, mut total) = (HashMap::new(), HashMap::new(), 0isize);
    for (frames, count) in &report.data {
        total += count;
        let names: Vec<String> = frames
            .frames
            .iter()
            .flat_map(|frame| frame.iter().map(|symbol| symbol.name()))
            .collect();
        if let Some(first) = names.first() {
            *own.entry(first.clone()).or_insert(0) += count;
        }
        let mut seen = HashSet::new();
        for name in names {
            if seen.insert(name.clone()) {
                *inclusive.entry(name).or_insert(0) += count;
            }
        }
    }
    for (title, map) in [("self", &own), ("inclusive", &inclusive)] {
        println!("\n{title} ({total} samples)");
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        let top = std::env::var("MINA_PROFILE_TOP")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(25);
        for (name, count) in entries.into_iter().take(top) {
            let name: String = name.chars().take(140).collect();
            println!("{:5.1}%  {name}", *count as f64 * 100.0 / total as f64);
        }
    }
}
