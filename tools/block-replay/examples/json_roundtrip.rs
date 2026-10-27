//! Parse a precomputed block's staged ledger diff, write it back to JSON and
//! print where the result differs from the original.
//!
//!   cargo run -p mina-block-replay --example json_roundtrip -- block.json[.zst]

use std::io::Read;

use mina_p2p_messages::v2::StagedLedgerDiffDiffStableV2;
use serde_json::Value;

fn diff(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => diff(v, w, &format!("{path}.{k}"), out),
                    None => out.push(format!("{path}.{k}: missing after round trip")),
                }
            }
            for k in y.keys().filter(|k| !x.contains_key(*k)) {
                out.push(format!("{path}.{k}: added by round trip"));
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (v, w)) in x.iter().zip(y).enumerate() {
                diff(v, w, &format!("{path}[{i}]"), out);
            }
        }
        _ if a != b => out.push(format!(
            "{path}: {} -> {}",
            truncate(a.to_string()),
            truncate(b.to_string())
        )),
        _ => {}
    }
}

fn truncate(s: String) -> String {
    if s.len() > 120 {
        format!("{}…", &s[..120])
    } else {
        s
    }
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("block file");
    let mut bytes = Vec::new();
    if path.ends_with(".zst") {
        zstd::Decoder::new(std::fs::File::open(&path)?)?.read_to_end(&mut bytes)?;
    } else {
        bytes = std::fs::read(&path)?;
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    let original = &value.get("data").unwrap_or(&value)["staged_ledger_diff"];
    let parsed: StagedLedgerDiffDiffStableV2 = serde_json::from_value(original.clone())?;
    let mut out = Vec::new();
    diff(
        original,
        &serde_json::to_value(&parsed)?,
        "staged_ledger_diff",
        &mut out,
    );
    for line in &out {
        println!("{line}");
    }
    println!("{} differences", out.len());
    Ok(())
}
