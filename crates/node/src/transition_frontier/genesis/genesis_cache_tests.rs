//! The genesis kept for a daemon JSON config (I5d2): named by the config's
//! contents and the node's build, read back whole or not at all.

use super::*;

const COMPACT: &[u8] =
    include_bytes!("../../../../../genesis_ledgers/mesa-mainnet-without-ledgers.json");

fn loaded() -> GenesisConfigLoaded {
    match GenesisConfig::from_bytes(COMPACT.to_vec()).unwrap() {
        GenesisConfig::WithoutLedgers(loaded) => *loaded,
        _ => panic!("not a config without ledgers"),
    }
}

fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("genesis-cache-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn an_entry_reads_back_as_written() {
    let dir = dir("round-trip");
    let config = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&config, b"{\"a daemon json\": 1}").unwrap();
    let cache = GenesisCache::for_config(&dir.join("cache"), &config, "build 1").unwrap();
    assert!(matches!(cache.read(), Ok(None)));
    cache.write(&loaded()).unwrap();
    let read = cache.read().unwrap().unwrap();
    assert!(same_genesis(&read, &loaded()));
    // The same contents and build name the same entry.
    let again = GenesisCache::for_config(&dir.join("cache"), &config, "build 1").unwrap();
    assert_eq!(again.path(), cache.path());
    assert!(again.read().unwrap().is_some());
    cache.remove().unwrap();
    assert!(matches!(cache.read(), Ok(None)));
    cache.remove().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn another_config_or_build_has_its_own_entry() {
    let dir = dir("keys");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.json");
    std::fs::write(&config, b"{\"a daemon json\": 1}").unwrap();
    let cache = GenesisCache::for_config(&dir, &config, "build 1").unwrap();
    cache.write(&loaded()).unwrap();
    assert!(GenesisCache::for_config(&dir, &config, "build 2")
        .unwrap()
        .read()
        .unwrap()
        .is_none());
    // One byte more in the config file.
    std::fs::write(&config, b"{\"a daemon json\": 1} ").unwrap();
    assert!(GenesisCache::for_config(&dir, &config, "build 1")
        .unwrap()
        .read()
        .unwrap()
        .is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_entry_is_not_used() {
    let dir = dir("damaged");
    let cache = GenesisCache::new(&dir, "00", "build 1");
    cache.write(&loaded()).unwrap();
    let bytes = std::fs::read(cache.path()).unwrap();
    // A digit of a total currency changed: still JSON, still a config.
    let text = String::from_utf8(bytes.clone()).unwrap();
    let changed = text.replacen("1301593843840039233", "1301593843840039234", 1);
    assert_ne!(changed, text);
    std::fs::write(cache.path(), changed).unwrap();
    assert_eq!(cache.read().err().as_deref(), Some("checksum mismatch"));
    // Cut short.
    std::fs::write(cache.path(), &bytes[..bytes.len() / 2]).unwrap();
    assert!(cache.read().is_err());
    // No checksum line.
    std::fs::write(cache.path(), COMPACT).unwrap();
    assert!(cache.read().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn same_genesis_tells_configs_apart() {
    let a = loaded();
    let mut b = loaded();
    assert!(same_genesis(&a, &b));
    b.next_epoch_seed = b.staking_epoch_seed.clone();
    assert!(!same_genesis(&a, &b));
}

/// Rejection selects the CLI's normal full-config path; its successful load
/// writes the replacement through the same cache writer.
fn rejected_then_replaced(cache: &GenesisCache) {
    assert!(cache.read().is_err());
    cache.remove().unwrap();
    assert!(matches!(cache.read(), Ok(None)));
    cache.write(&loaded()).unwrap();
    assert!(same_genesis(&cache.read().unwrap().unwrap(), &loaded()));
}

#[test]
fn copied_entries_are_bound_to_both_config_and_build() {
    let dir = dir("copied");
    let original = GenesisCache::new(&dir, "config-one", "build one");
    original.write(&loaded()).unwrap();
    for other in [
        GenesisCache::new(&dir, "config-two", "build one"),
        GenesisCache::new(&dir, "config-one", "build two"),
    ] {
        std::fs::copy(original.path(), other.path()).unwrap();
        assert_eq!(
            other.read().err().as_deref(),
            Some("genesis cache config/build key mismatch")
        );
        rejected_then_replaced(&other);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stale_truncated_and_tampered_entries_are_replaced() {
    use super::genesis_cache::blake2b_hex;
    let dir = dir("replacement");
    let cache = GenesisCache::new(&dir, "config-one", "build one");
    cache.write(&loaded()).unwrap();
    let bytes = std::fs::read(cache.path()).unwrap();
    // The previous unkeyed format, even with a valid checksum, is stale.
    let old_json = serde_json::to_vec(&loaded()).unwrap();
    let old = format!(
        "{}\n{}",
        blake2b_hex(&old_json),
        String::from_utf8(old_json).unwrap()
    );
    std::fs::write(cache.path(), old).unwrap();
    rejected_then_replaced(&cache);
    // A stale format version with an otherwise valid keyed entry.
    let split = bytes.iter().position(|b| *b == b'\n').unwrap();
    let mut entry: serde_json::Value = serde_json::from_slice(&bytes[split + 1..]).unwrap();
    entry["version"] = serde_json::json!(0);
    let json = serde_json::to_vec(&entry).unwrap();
    let stale = format!(
        "{}\n{}",
        blake2b_hex(&json),
        String::from_utf8(json).unwrap()
    );
    std::fs::write(cache.path(), stale).unwrap();
    assert_eq!(
        cache.read().err().as_deref(),
        Some("unsupported genesis cache version")
    );
    rejected_then_replaced(&cache);
    std::fs::write(cache.path(), &bytes[..bytes.len() / 2]).unwrap();
    rejected_then_replaced(&cache);
    // Both the payload and the key are protected by the checksum.
    for (from, to) in [
        ("1301593843840039233", "1301593843840039234"),
        ("config-one", "config-two"),
    ] {
        let text = String::from_utf8(bytes.clone()).unwrap();
        let tampered = text.replacen(from, to, 1);
        assert_ne!(tampered, text);
        std::fs::write(cache.path(), tampered).unwrap();
        assert_eq!(cache.read().err().as_deref(), Some("checksum mismatch"));
        rejected_then_replaced(&cache);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
