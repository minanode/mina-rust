use std::fs;

use super::*;

fn block(height: u32) -> BlockRef {
    BlockRef {
        height,
        state_hash: format!("3NHash{height}"),
    }
}

fn parent(height: u32) -> String {
    format!("3NHash{}", height - 1)
}

fn payload(height: u32) -> Vec<u8> {
    format!("diff of block {height}").into_bytes()
}

fn config(dir: &tempfile::TempDir) -> OutboxConfig {
    OutboxConfig::new(dir.path())
}

fn append(outbox: &mut Outbox, height: u32) -> AppendOutcome {
    outbox
        .append(block(height), &parent(height), &payload(height))
        .unwrap()
}

fn queued_gaps(outcome: AppendOutcome) -> Vec<Gap> {
    match outcome {
        AppendOutcome::Queued { gaps, .. } => gaps,
        other => panic!("expected Queued, got {other:?}"),
    }
}

fn heights(outbox: &Outbox) -> Vec<u32> {
    outbox.pending().map(|m| m.block.height).collect()
}

fn deliver_all(outbox: &mut Outbox) -> Vec<Vec<u8>> {
    let mut delivered = Vec::new();
    while let Some(entry) = outbox.next_for_delivery().unwrap() {
        delivered.push(entry.payload);
        outbox.confirm(entry.meta.seq).unwrap();
    }
    delivered
}

fn gap_lines(dir: &tempfile::TempDir) -> Vec<Gap> {
    fs::read_to_string(dir.path().join(GAPS_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn entry_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|d| d.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(ENTRY_SUFFIX))
        .collect();
    files.sort();
    files
}

#[test]
fn entries_survive_a_restart_in_order() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut outbox = Outbox::open(config(&dir)).unwrap();
        for h in 10..13 {
            queued_gaps(append(&mut outbox, h));
        }
    }
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    assert_eq!(heights(&outbox), vec![10, 11, 12]);
    assert_eq!(
        deliver_all(&mut outbox),
        vec![payload(10), payload(11), payload(12)]
    );
    // Sequence numbers continue after a restart.
    let AppendOutcome::Queued { seq, .. } = append(&mut outbox, 13) else {
        panic!()
    };
    assert_eq!(seq, 3);
}

#[test]
fn crash_between_send_and_confirm_resends() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut outbox = Outbox::open(config(&dir)).unwrap();
        append(&mut outbox, 10);
        append(&mut outbox, 11);
        let first = outbox.next_for_delivery().unwrap().unwrap();
        assert_eq!(first.payload, payload(10));
        // Sent, then the node dies before the archive's answer is recorded.
    }
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    assert_eq!(heights(&outbox), vec![10, 11]);
    let again = outbox.next_for_delivery().unwrap().unwrap();
    assert_eq!(again.payload, payload(10));
    outbox.confirm(again.meta.seq).unwrap();
    drop(outbox);

    // Acknowledged means retained: startup replays it too.
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    assert_eq!(heights(&outbox), vec![10, 11]);
    assert_eq!(deliver_all(&mut outbox), vec![payload(10), payload(11)]);
    assert!(Outbox::open(config(&dir)).is_err(), "still locked");
    drop(outbox);
    assert_eq!(Outbox::open(config(&dir)).unwrap().len(), 2);
}

#[test]
fn a_failed_send_keeps_the_entry_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    append(&mut outbox, 10);
    append(&mut outbox, 11);
    for _ in 0..3 {
        let entry = outbox.next_for_delivery().unwrap().unwrap();
        assert_eq!(entry.meta.block, block(10));
        outbox.release();
    }
    assert_eq!(heights(&outbox), vec![10, 11]);
}

#[test]
fn duplicate_blocks_are_queued_once_and_replay_is_not_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    append(&mut outbox, 10);
    append(&mut outbox, 11);
    assert_eq!(
        append(&mut outbox, 11),
        AppendOutcome::AlreadyPending { seq: 1 }
    );
    assert_eq!(heights(&outbox), vec![10, 11]);
    deliver_all(&mut outbox);
    drop(outbox);

    // A restart re-applies the frontier and sends its blocks again: they are
    // queued again (the archive is idempotent) and the chain has no gap.
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    for h in 10..12 {
        assert!(matches!(
            append(&mut outbox, h),
            AppendOutcome::AlreadyPending { .. }
        ));
    }
    assert!(queued_gaps(append(&mut outbox, 12)).is_empty());
    assert_eq!(heights(&outbox), vec![10, 11, 12]);
    assert!(gap_lines(&dir).is_empty());
}

#[test]
fn a_full_outbox_drops_the_oldest_and_records_each_drop() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    config.max_entries = 3;
    let mut outbox = Outbox::open(config.clone()).unwrap();
    for h in 10..13 {
        assert!(queued_gaps(append(&mut outbox, h)).is_empty());
    }
    let gaps = queued_gaps(append(&mut outbox, 13));
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].kind, GapKind::DroppedFull);
    assert_eq!((gaps[0].from_height, gaps[0].to_height), (10, 10));
    assert_eq!(gaps[0].state_hash.as_deref(), Some("3NHash10"));
    queued_gaps(append(&mut outbox, 14));
    assert_eq!(heights(&outbox), vec![12, 13, 14]);
    assert_eq!(entry_files(dir.path()).len(), 3);
    assert_eq!(outbox.stats().dropped_full, 2);
    drop(outbox);

    let dropped: Vec<_> = gap_lines(&dir)
        .into_iter()
        .map(|g| (g.kind, g.state_hash.unwrap()))
        .collect();
    assert_eq!(
        dropped,
        vec![
            (GapKind::DroppedFull, "3NHash10".to_owned()),
            (GapKind::DroppedFull, "3NHash11".to_owned()),
        ]
    );
    let outbox = Outbox::open(config).unwrap();
    assert_eq!(outbox.stats().gaps_total, 2);
    assert_eq!(outbox.stats().recent_gaps.len(), 2);
}

#[test]
fn the_byte_bound_holds_and_spares_the_entry_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    let one = encode_entry(0, &block(10), &parent(10), &payload(10)).len() as u64;
    config.max_bytes = one * 2 + one / 2;
    let mut outbox = Outbox::open(config).unwrap();
    append(&mut outbox, 10);
    append(&mut outbox, 11);
    let in_flight = outbox.next_for_delivery().unwrap().unwrap();
    assert_eq!(in_flight.meta.block, block(10));
    let gaps = queued_gaps(append(&mut outbox, 12));
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].state_hash.as_deref(), Some("3NHash11"));
    assert_eq!(heights(&outbox), vec![10, 12]);
    assert!(outbox.stats().pending_bytes <= one * 2 + one / 2);
    outbox.confirm(in_flight.meta.seq).unwrap();
    assert_eq!(deliver_all(&mut outbox), vec![payload(12)]);
}

#[test]
fn a_corrupt_entry_is_moved_aside_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut outbox = Outbox::open(config(&dir)).unwrap();
        for h in 10..14 {
            append(&mut outbox, h);
        }
    }
    let files = entry_files(dir.path());
    // A flipped payload byte.
    let mut bytes = fs::read(&files[1]).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x40;
    fs::write(&files[1], bytes).unwrap();
    // A torn write.
    let bytes = fs::read(&files[2]).unwrap();
    fs::write(&files[2], &bytes[..bytes.len() - 5]).unwrap();
    // A file named for another block.
    fs::rename(
        &files[3],
        dir.path()
            .join(entry_name(3, &block(99)).replace("3NHash99", "3NHash13x")),
    )
    .unwrap();
    // Leftovers that aren't entries.
    fs::write(dir.path().join("garbage-name.diff"), b"x").unwrap();
    fs::write(
        dir.path()
            .join(format!("{}{TMP_SUFFIX}", entry_name(9, &block(50)))),
        b"partial",
    )
    .unwrap();

    let mut outbox = Outbox::open(config(&dir)).unwrap();
    assert_eq!(heights(&outbox), vec![10, 11, 12, 99]);
    assert_eq!(deliver_all(&mut outbox), vec![payload(10)]);
    assert!(outbox.is_empty());
    assert_eq!(outbox.stats().corrupt, 3);

    let corrupt: Vec<_> = gap_lines(&dir)
        .into_iter()
        .map(|g| {
            assert_eq!(g.kind, GapKind::Corrupt);
            g.from_height
        })
        .collect();
    assert_eq!(corrupt, vec![0, 11, 12, 99]);
    let quarantined = fs::read_dir(dir.path().join(CORRUPT_DIR)).unwrap().count();
    assert_eq!(quarantined, 4, "three bad entries and the unnamed file");
    assert!(entry_files(dir.path()).is_empty());
    assert!(!fs::read_dir(dir.path()).unwrap().any(|d| d
        .unwrap()
        .path()
        .to_string_lossy()
        .ends_with(TMP_SUFFIX)));
}

#[test]
fn a_block_with_an_unseen_parent_opens_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut outbox = Outbox::open(config(&dir)).unwrap();
        // The feed starts here: no gap for what came before.
        for h in 10..13 {
            assert!(queued_gaps(append(&mut outbox, h)).is_empty());
        }
        deliver_all(&mut outbox);
    }
    // The node was down for longer than its frontier: after the restart its
    // first block is far ahead.
    let mut outbox = Outbox::open(config(&dir)).unwrap();
    assert_eq!(outbox.stats().feed_started_at, Some(block(10)));
    let gaps = queued_gaps(append(&mut outbox, 400));
    assert_eq!(gaps.len(), 1);
    let gap = &gaps[0];
    assert_eq!(gap.kind, GapKind::UnseenParent);
    assert_eq!((gap.from_height, gap.to_height), (13, 399));
    assert_eq!(gap.parent_hash.as_deref(), Some("3NHash399"));
    assert_eq!(gap.first_block_after, Some(block(400)));
    // The chain goes on from there without new gaps.
    assert!(queued_gaps(append(&mut outbox, 401)).is_empty());
    // A fork off a parent we never saw, below the highest height.
    let gaps = queued_gaps(
        outbox
            .append(
                BlockRef {
                    height: 401,
                    state_hash: "3NFork401".to_owned(),
                },
                "3NFork400",
                b"fork",
            )
            .unwrap(),
    );
    assert_eq!((gaps[0].from_height, gaps[0].to_height), (400, 400));
    assert_eq!(gap_lines(&dir).len(), 2);
}

#[test]
fn a_second_process_cannot_open_the_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Outbox::open(config(&dir)).unwrap();
    let err = Outbox::open(config(&dir)).err().unwrap();
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn entry_encoding_round_trips() {
    let meta = EntryMeta {
        seq: 7,
        block: block(10),
        size: 0,
    };
    let bytes = encode_entry(7, &block(10), &parent(10), b"payload");
    let entry = decode_entry(&bytes, &meta).unwrap();
    assert_eq!(entry.payload, b"payload");
    assert_eq!(entry.parent_hash, parent(10));
    let other = EntryMeta { seq: 8, ..meta };
    assert!(decode_entry(&bytes, &other).is_err());
    assert_eq!(
        parse_entry_name(&entry_name(7, &block(10))),
        Some((7, block(10)))
    );
}

#[test]
fn acknowledged_entries_share_bounds_and_expire_by_height() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    cfg.retain_blocks = 3;
    cfg.max_entries = 3;
    let mut outbox = Outbox::open(cfg).unwrap();
    append(&mut outbox, 10);
    deliver_all(&mut outbox);
    assert_eq!(outbox.stats().retained_entries, 1);
    assert_eq!(outbox.stats().pending_entries, 0);
    let size = outbox.stats().retained_bytes;
    assert!(size > 0);
    append(&mut outbox, 11);
    append(&mut outbox, 12);
    assert_eq!(outbox.stats().total_entries, 3);
    assert_eq!(
        outbox.stats().total_bytes,
        size + outbox.stats().pending_bytes
    );
    // Capacity evicts retained data first, but reports its loss of repair.
    let gaps = queued_gaps(append(&mut outbox, 13));
    assert_eq!(gaps[0].state_hash.as_deref(), Some("3NHash10"));
    assert_eq!(outbox.stats().retained_entries, 0);
    deliver_all(&mut outbox);
    assert_eq!(outbox.stats().retained_entries, 3);
    outbox.config.max_entries = 10;
    assert!(queued_gaps(append(&mut outbox, 14)).is_empty());
    // Height 11 has reached the retention boundary (14 - 11 = 3).
    assert!(!outbox.retained.values().any(|m| m.block.height == 11));
    assert_eq!(outbox.stats().total_entries, 3);
}

#[test]
fn retained_bytes_count_toward_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    // Blocks 11 and 12 have equal-sized two-digit parent references; block
    // 10's one-digit parent makes its entry one byte smaller.
    let one = encode_entry(0, &block(11), &parent(11), &payload(11)).len() as u64;
    cfg.max_bytes = one * 2;
    let mut outbox = Outbox::open(cfg).unwrap();
    append(&mut outbox, 10);
    deliver_all(&mut outbox);
    append(&mut outbox, 11);
    let gaps = queued_gaps(append(&mut outbox, 12));
    assert_eq!(gaps[0].state_hash.as_deref(), Some("3NHash10"));
    assert!(outbox.stats().total_bytes <= one * 2);
}

fn inject(step: &'static str, crash: bool, action: impl FnOnce() -> io::Result<()>) {
    FAULT.with(|fault| *fault.borrow_mut() = Some((step, crash)));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    if crash {
        assert!(result.is_err(), "crash checkpoint {step} not reached");
    } else {
        assert!(
            result.unwrap().is_err(),
            "I/O checkpoint {step} not reached"
        );
    }
    FAULT.with(|fault| assert!(fault.borrow().is_none(), "unused checkpoint {step}"));
}

const GAP_STEPS: &[&str] = &[
    "gap_open",
    "gap_write",
    "gap_file_sync",
    "gap_dir_sync",
    "gap_durable",
];

#[test]
fn eviction_failure_or_crash_at_every_boundary_preserves_entry_or_gap() {
    for crash in [false, true] {
        for &step in GAP_STEPS
            .iter()
            .chain(["entry_remove", "entry_remove_sync", "entry_removed"].iter())
        {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = config(&dir);
            cfg.max_entries = 1;
            let mut outbox = Outbox::open(cfg.clone()).unwrap();
            append(&mut outbox, 10);
            fs::remove_file(dir.path().join(GAPS_FILE)).unwrap();
            inject(step, crash, || {
                outbox
                    .append(block(11), &parent(11), &payload(11))
                    .map(|_| ())
            });
            drop(outbox);
            let reopened = Outbox::open(cfg).unwrap();
            let has_entry = reopened.pending().any(|m| m.block == block(10));
            let has_gap = gap_lines(&dir).iter().any(|g| {
                g.kind == GapKind::DroppedFull && g.state_hash.as_deref() == Some("3NHash10")
            });
            assert!(has_entry || has_gap, "lost both at {step}, crash={crash}");
            if GAP_STEPS.contains(&step) {
                assert!(has_entry, "removed before gap durability at {step}");
            }
        }
    }
}

#[test]
fn quarantine_failure_or_crash_at_every_boundary_preserves_entry_or_gap() {
    for crash in [false, true] {
        for &step in GAP_STEPS.iter().chain(
            [
                "quarantine_rename",
                "quarantine_dest_sync",
                "quarantine_source_sync",
                "quarantine_done",
            ]
            .iter(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let mut outbox = Outbox::open(config(&dir)).unwrap();
            append(&mut outbox, 10);
            fs::write(entry_files(dir.path())[0].clone(), b"corrupt").unwrap();
            fs::remove_file(dir.path().join(GAPS_FILE)).unwrap();
            inject(step, crash, || outbox.next_for_delivery().map(|_| ()));
            drop(outbox);
            let reopened = Outbox::open(config(&dir)).unwrap();
            let has_entry = reopened.pending().any(|m| m.block == block(10));
            let has_gap = gap_lines(&dir)
                .iter()
                .any(|g| g.kind == GapKind::Corrupt && g.state_hash.as_deref() == Some("3NHash10"));
            assert!(has_entry || has_gap, "lost both at {step}, crash={crash}");
            if GAP_STEPS.contains(&step) {
                assert!(has_entry, "quarantined before gap durability at {step}");
            }
        }
    }
}

#[test]
fn failed_replacement_and_unwritable_gap_log_preserve_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    cfg.max_entries = 1;
    let mut outbox = Outbox::open(cfg.clone()).unwrap();
    append(&mut outbox, 10);
    // The replacement write fails after the eviction. Its victim's gap exists.
    fs::create_dir(
        dir.path()
            .join(format!("{}{}", entry_name(1, &block(11)), TMP_SUFFIX)),
    )
    .unwrap();
    assert!(outbox.append(block(11), &parent(11), &payload(11)).is_err());
    drop(outbox);
    let mut outbox = Outbox::open(cfg).unwrap();
    assert!(outbox.is_empty());
    assert_eq!(outbox.stats().gaps_total, 1);
    append(&mut outbox, 12);
    fs::write(entry_files(dir.path())[0].clone(), b"corrupt").unwrap();
    fs::remove_file(dir.path().join(GAPS_FILE)).unwrap();
    fs::create_dir(dir.path().join(GAPS_FILE)).unwrap();
    assert!(outbox.next_for_delivery().is_err());
    assert_eq!(outbox.len(), 1);
    assert_eq!(entry_files(dir.path()).len(), 1);
}

#[test]
fn torn_gap_tail_is_removed_before_the_next_record() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    cfg.max_entries = 1;
    let mut outbox = Outbox::open(cfg.clone()).unwrap();
    append(&mut outbox, 10);
    append(&mut outbox, 11);
    drop(outbox);
    OpenOptions::new()
        .append(true)
        .open(dir.path().join(GAPS_FILE))
        .unwrap()
        .write_all(b"{\"kind\":\"dropp")
        .unwrap();
    let mut outbox = Outbox::open(cfg).unwrap();
    append(&mut outbox, 12);
    let gaps = gap_lines(&dir);
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[1].state_hash.as_deref(), Some("3NHash11"));
}

#[test]
fn unseen_parent_is_durable_before_cursor_advances() {
    for crash in [false, true] {
        for &step in GAP_STEPS.iter().chain(["cursor_write"].iter()) {
            let dir = tempfile::tempdir().unwrap();
            let mut outbox = Outbox::open(config(&dir)).unwrap();
            append(&mut outbox, 10);
            inject(step, crash, || {
                outbox
                    .append(block(400), &parent(400), &payload(400))
                    .map(|_| ())
            });
            drop(outbox);
            let reopened = Outbox::open(config(&dir)).unwrap();
            assert!(
                !reopened.cursor.contains(&block(400).state_hash)
                    || reopened.stats().gaps_total > 0
            );
            if step == "cursor_write" {
                assert!(reopened.stats().gaps_total > 0);
            }
        }
    }
}

#[test]
fn acknowledgement_and_replay_moves_preserve_index_on_sync_failure() {
    for replay in [false, true] {
        for crash in [false, true] {
            for step in ["move_rename", "move_dest_sync", "move_source_sync"] {
                let dir = tempfile::tempdir().unwrap();
                let mut outbox = Outbox::open(config(&dir)).unwrap();
                append(&mut outbox, 10);
                let entry = outbox.next_for_delivery().unwrap().unwrap();
                if replay {
                    outbox.confirm(entry.meta.seq).unwrap();
                }
                inject(step, crash, || {
                    if replay {
                        outbox.resend_retained().map(|_| ())
                    } else {
                        outbox.confirm(entry.meta.seq)
                    }
                });
                assert_eq!(outbox.stats().total_entries, 1);
                assert!(outbox.stats().total_bytes > 0);
                // No restart is needed to retry an interrupted operator move.
                if replay {
                    outbox.resend_retained().unwrap();
                    assert_eq!(outbox.len(), 1);
                }
                drop(outbox);
                let mut reopened = Outbox::open(config(&dir)).unwrap();
                assert_eq!(deliver_all(&mut reopened), vec![payload(10)]);
                assert_eq!(reopened.stats().total_entries, 1);
            }
        }
    }
}
