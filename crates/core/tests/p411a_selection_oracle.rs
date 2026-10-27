//! Test-only transcription of OCaml 4.0.2 chain selection.
//!
//! Reference: MinaProtocol/mina `84b8d96954b218f372b93f65a071abf528e6bb61`,
//! `src/lib/consensus/proof_of_stake.ml`. The oracle deliberately does not
//! call Rust's fork classification or density projection.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use mina_core::consensus::{consensus_take, is_short_range_fork, relative_min_window_density};
use mina_p2p_messages::v2::{
    ConsensusProofOfStakeDataConsensusStateValueStableV2 as State, LedgerHash,
    MinaNumbersGlobalSlotSinceHardForkMStableV1, StateHash,
};
use serde_json::Value;

const SLOTS_PER_EPOCH: u32 = 7140;
const SLOTS_PER_SUB_WINDOW: u32 = 7;
const SUB_WINDOWS_PER_WINDOW: u32 = 11;
// OCaml constants.ml:239-240 (`grace_period_end`).
const GRACE_PERIOD_END: u32 = 2160 + SLOTS_PER_SUB_WINDOW * SUB_WINDOWS_PER_WINDOW;

/// OCaml `Global_slot.slot_number`, proof_of_stake.ml:1195-1198.
fn slot(state: &State) -> u32 {
    state.curr_global_slot_since_hard_fork.slot_number.as_u32()
}

/// OCaml `Consensus_state.curr_epoch_and_slot`,
/// proof_of_stake.ml:2303, used by `is_short_range`.
fn epoch_and_slot(state: &State) -> (u32, u32) {
    let slot = slot(state);
    (slot / SLOTS_PER_EPOCH, slot % SLOTS_PER_EPOCH)
}

/// OCaml `is_short_range`, proof_of_stake.ml:2771-2787.
fn oracle_is_short_range(a: &State, b: &State) -> bool {
    let pred_case = |c1: &State, c2: &State| {
        let (e1, s1) = epoch_and_slot(c1);
        let (e2, _) = epoch_and_slot(c2);
        let c1_next_is_finalized = s1 + 1 >= (SLOTS_PER_EPOCH / 3) * 2;
        e1 + 1 == e2
            && c1_next_is_finalized
            && c1.next_epoch_data.lock_checkpoint == c2.staking_epoch_data.lock_checkpoint
    };
    let (a_epoch, _) = epoch_and_slot(a);
    let (b_epoch, _) = epoch_and_slot(b);
    if a_epoch == b_epoch {
        a.staking_epoch_data.lock_checkpoint == b.staking_epoch_data.lock_checkpoint
    } else {
        pred_case(a, b) || pred_case(b, a)
    }
}

/// OCaml `Min_window_density.update_min_window_density`,
/// proof_of_stake.ml:1165-1279. `incr_window` is false in `select`.
fn oracle_update_min_window_density(
    prev_slot: u32,
    next_slot: u32,
    prev_densities: &[u32],
    prev_min: u32,
    incr_window: bool,
) -> (u32, Vec<u32>) {
    assert!(next_slot >= prev_slot);
    assert_eq!(prev_densities.len(), SUB_WINDOWS_PER_WINDOW as usize);
    let prev_global_sub_window = prev_slot / SLOTS_PER_SUB_WINDOW;
    let next_global_sub_window = next_slot / SLOTS_PER_SUB_WINDOW;
    let prev_relative_sub_window = prev_global_sub_window % SUB_WINDOWS_PER_WINDOW;
    let next_relative_sub_window = next_global_sub_window % SUB_WINDOWS_PER_WINDOW;
    let same_sub_window = prev_global_sub_window == next_global_sub_window;
    let overlapping_window =
        prev_global_sub_window + SUB_WINDOWS_PER_WINDOW >= next_global_sub_window;

    let current_sub_window_densities: Vec<u32> = prev_densities
        .iter()
        .enumerate()
        .map(|(i, &density)| {
            let i = i as u32;
            let gt_prev_sub_window = i > prev_relative_sub_window;
            let lt_next_sub_window = i < next_relative_sub_window;
            let within_range = if prev_relative_sub_window < next_relative_sub_window {
                gt_prev_sub_window && lt_next_sub_window
            } else {
                gt_prev_sub_window || lt_next_sub_window
            };
            if same_sub_window || (overlapping_window && !within_range) {
                density
            } else {
                0
            }
        })
        .collect();

    let current_window_density: u32 = current_sub_window_densities.iter().sum();
    let min_window_density = if same_sub_window || next_slot < GRACE_PERIOD_END {
        prev_min
    } else {
        current_window_density.min(prev_min)
    };

    let next_sub_window_densities = current_sub_window_densities
        .into_iter()
        .enumerate()
        .map(|(i, density)| {
            if i as u32 == next_relative_sub_window {
                let density = if same_sub_window { density } else { 0 };
                density + u32::from(incr_window)
            } else {
                density
            }
        })
        .collect();
    (min_window_density, next_sub_window_densities)
}

/// OCaml `virtual_min_window_density`, proof_of_stake.ml:2868-2878.
fn oracle_virtual_min_window_density(state: &State, max_slot: u32) -> u32 {
    if slot(state) == max_slot {
        state.min_window_density.as_u32()
    } else {
        let densities = state
            .sub_window_densities
            .iter()
            .map(|d| d.as_u32())
            .collect::<Vec<_>>();
        oracle_update_min_window_density(
            slot(state),
            max_slot,
            &densities,
            state.min_window_density.as_u32(),
            false,
        )
        .0
    }
}

/// OCaml `less_than_or_equal_when`, proof_of_stake.ml:2833-2836.
fn less_than_or_equal_when<T: Ord>(existing: T, candidate: T, condition: bool) -> bool {
    existing < candidate || (existing == candidate && condition)
}

/// OCaml `select`, proof_of_stake.ml:2791-2893. Its tie-break is
/// blockchain length, then Blake2b digest of truncated VRF output, then
/// state hash. Hash comparisons use the same decoded type as production.
fn oracle_select(
    existing: &State,
    candidate: &State,
    existing_hash: &StateHash,
    candidate_hash: &StateHash,
) -> bool {
    let candidate_hash_is_bigger = candidate_hash > existing_hash;
    let candidate_vrf_is_bigger = less_than_or_equal_when(
        existing.last_vrf_output.blake2b(),
        candidate.last_vrf_output.blake2b(),
        candidate_hash_is_bigger,
    );
    let blockchain_length_is_longer = less_than_or_equal_when(
        existing.blockchain_length.as_u32(),
        candidate.blockchain_length.as_u32(),
        candidate_vrf_is_bigger,
    );
    if oracle_is_short_range(existing, candidate) {
        blockchain_length_is_longer
    } else {
        let max_slot = slot(existing).max(slot(candidate));
        less_than_or_equal_when(
            oracle_virtual_min_window_density(existing, max_slot),
            oracle_virtual_min_window_density(candidate, max_slot),
            blockchain_length_is_longer,
        )
    }
}

const TIP_FIXTURE: &str = include_str!(
    "../../../tests/files/forks/long-keep-density-161-166-3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG-3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC-tip.json"
);
const CANDIDATE_FIXTURE: &str = include_str!(
    "../../../tests/files/forks/long-keep-density-161-166-3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG-3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC-cnd.json"
);
const TIP_HASH: &str = "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG";
const CANDIDATE_HASH: &str = "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC";

fn fixture_state(json: &str) -> State {
    let mut value: Value = serde_json::from_str(json).unwrap();
    serde_json::from_value(value["body"]["consensus_state"].take()).unwrap()
}

fn at_slot(state: &mut State, value: u32) {
    state.curr_global_slot_since_hard_fork.slot_number =
        MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(value.into());
    state.curr_global_slot_since_hard_fork.slots_per_epoch = SLOTS_PER_EPOCH.into();
    state.epoch_count = (value / SLOTS_PER_EPOCH).into();
}

fn set_density(state: &mut State, minimum: u32, buckets: [u32; 11]) {
    state.min_window_density = minimum.into();
    state.sub_window_densities = buckets.into_iter().map(Into::into).collect();
}

#[test]
fn ocaml_equal_slot_reference_case() {
    // P4.9 review transcribed the OCaml 4.0.2 result at slot 7151:
    // stored minima 20 and 21. See proof_of_stake.ml:2868-2878.
    let mut tip = fixture_state(TIP_FIXTURE);
    let mut candidate = fixture_state(CANDIDATE_FIXTURE);
    at_slot(&mut tip, 7151);
    at_slot(&mut candidate, 7151);
    set_density(&mut tip, 20, [2; 11]);
    set_density(&mut candidate, 21, [1, 2, 1, 2, 1, 2, 1, 2, 1, 1, 7]);
    let tip_hash: StateHash = TIP_HASH.parse().unwrap();
    let candidate_hash: StateHash = CANDIDATE_HASH.parse().unwrap();
    assert!(!oracle_is_short_range(&tip, &candidate));
    assert_eq!(oracle_virtual_min_window_density(&tip, 7151), 20);
    assert_eq!(oracle_virtual_min_window_density(&candidate, 7151), 21);
    assert!(oracle_select(&tip, &candidate, &tip_hash, &candidate_hash));
}

// The fixed seed and named cases make any failure reproducible without a
// random corpus or a running node.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

struct Pair {
    tip: State,
    candidate: State,
    tip_hash: StateHash,
    candidate_hash: StateHash,
    label: String,
}

fn generated_pairs() -> Vec<Pair> {
    let mut rng = Rng(0x411a_84b8_d969_54b2);
    let mut pairs = minimal_pairs();
    let tip_hash: StateHash = TIP_HASH.parse().unwrap();
    let candidate_hash: StateHash = CANDIDATE_HASH.parse().unwrap();
    let anchors = [
        0, 6, 7, 76, 77, 2159, 2160, 2236, 2237, 4758, 4759, 4760, 7139, 7140, 7151, 14279, 14280,
        393801,
    ];
    let deltas = [0, 1, 6, 7, 8, 14, 70, 76, 77, 78, 154, 7140];
    for i in 0..12_000 {
        let mut tip = fixture_state(TIP_FIXTURE);
        let mut candidate = fixture_state(CANDIDATE_FIXTURE);
        let a = if i < anchors.len() * deltas.len() {
            anchors[i % anchors.len()]
        } else {
            rng.below(400_000)
        };
        let b = if i < anchors.len() * deltas.len() {
            a.saturating_add(deltas[(i / anchors.len()) % deltas.len()])
        } else if i % 4 == 0 {
            a
        } else {
            a.saturating_add(deltas[rng.below(deltas.len() as u32) as usize])
        };
        at_slot(&mut tip, a);
        at_slot(&mut candidate, b);
        let mut tip_buckets = [0; 11];
        let mut candidate_buckets = [0; 11];
        for d in &mut tip_buckets {
            *d = rng.below(8);
        }
        for d in &mut candidate_buckets {
            *d = rng.below(8);
        }
        if i % 9 == 0 {
            candidate_buckets = [0; 11];
        }
        let tip_sum: u32 = tip_buckets.iter().sum();
        let candidate_sum: u32 = candidate_buckets.iter().sum();
        let tip_min = rng.below(tip_sum + 1);
        let candidate_min = if i % 7 == 0 {
            tip_min.min(candidate_sum)
        } else {
            rng.below(candidate_sum + 1)
        };
        set_density(&mut tip, tip_min, tip_buckets);
        set_density(&mut candidate, candidate_min, candidate_buckets);
        if i % 3 == 0 {
            candidate.staking_epoch_data.lock_checkpoint =
                tip.staking_epoch_data.lock_checkpoint.clone();
        }
        if i % 5 == 0 {
            candidate.last_vrf_output = tip.last_vrf_output.clone();
        }
        tip.blockchain_length = (1000 + rng.below(5)).into();
        candidate.blockchain_length = if i % 4 == 0 {
            tip.blockchain_length
        } else {
            (1000 + rng.below(5)).into()
        };
        pairs.push(Pair {
            tip,
            candidate,
            tip_hash: tip_hash.clone(),
            candidate_hash: if i % 11 == 0 {
                tip_hash.clone()
            } else {
                candidate_hash.clone()
            },
            label: format!("generated-{i}"),
        });
    }
    // The review's minimal counterexample is deliberately present regardless
    // of the random stream.
    let mut tip = fixture_state(TIP_FIXTURE);
    let mut candidate = fixture_state(CANDIDATE_FIXTURE);
    at_slot(&mut tip, 7151);
    at_slot(&mut candidate, 7151);
    set_density(&mut tip, 20, [2; 11]);
    set_density(&mut candidate, 21, [1, 2, 1, 2, 1, 2, 1, 2, 1, 1, 7]);
    pairs.push(Pair {
        tip,
        candidate,
        tip_hash,
        candidate_hash,
        label: "p49-equal-slot".into(),
    });
    pairs
}

fn minimal_pairs() -> Vec<Pair> {
    // Each one differs in one density bucket and uses equal VRF/hash to
    // expose the ring boundary rather than a later tie-break.
    let specs = [
        ("equal-slot-fast-path", 7151, 7151, 10, false),
        ("same-sub-window", 7151, 7152, 10, true),
        ("next-sub-window-endpoint", 7151, 7157, 10, true),
        ("exact-window-overlap", 7151, 7228, 9, true),
    ];
    specs
        .into_iter()
        .map(|(label, tip_slot, candidate_slot, bucket, tip_is_dense)| {
            let mut tip = fixture_state(TIP_FIXTURE);
            let mut candidate = fixture_state(CANDIDATE_FIXTURE);
            at_slot(&mut tip, tip_slot);
            at_slot(&mut candidate, candidate_slot);
            let mut one = [0; 11];
            one[bucket] = 1;
            if tip_is_dense {
                set_density(&mut tip, 1, one);
                set_density(&mut candidate, 0, [0; 11]);
                tip.blockchain_length = 1u32.into();
                candidate.blockchain_length = 2u32.into();
            } else {
                set_density(&mut tip, 0, [0; 11]);
                set_density(&mut candidate, 1, one);
                tip.blockchain_length = 2u32.into();
                candidate.blockchain_length = 1u32.into();
            }
            candidate.last_vrf_output = tip.last_vrf_output.clone();
            let hash: StateHash = TIP_HASH.parse().unwrap();
            Pair {
                tip,
                candidate,
                tip_hash: hash.clone(),
                candidate_hash: hash,
                label: label.into(),
            }
        })
        .collect()
}

/// Rust's selection took the other tip on each of these before P4.11.
#[test]
fn minimal_long_range_counterexamples() {
    for pair in minimal_pairs() {
        assert!(!oracle_is_short_range(&pair.tip, &pair.candidate));
        let oracle = oracle_select(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        let rust = consensus_take(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        assert_eq!(oracle, rust, "{}", pair.label);
    }
}

fn class(pair: &Pair) -> &'static str {
    if oracle_is_short_range(&pair.tip, &pair.candidate) {
        "short range"
    } else {
        let a = slot(&pair.tip);
        let b = slot(&pair.candidate);
        if a == b {
            "long range, equal slot"
        } else if a / SLOTS_PER_SUB_WINDOW == b / SLOTS_PER_SUB_WINDOW {
            "long range, same sub-window"
        } else if a.abs_diff(b) >= SLOTS_PER_SUB_WINDOW * SUB_WINDOWS_PER_WINDOW {
            "long range, non-overlap"
        } else if a.max(b) < GRACE_PERIOD_END {
            "long range, grace"
        } else {
            "long range, projected ring"
        }
    }
}

#[derive(Default)]
struct Diff {
    cases: usize,
    choice_mismatches: BTreeMap<&'static str, (usize, String)>,
    density_mismatches: BTreeMap<&'static str, (usize, String)>,
}

fn compact(pair: &Pair, want: bool, got: bool) -> String {
    format!(
        "{} slots={}/{} min={}/{} buckets={:?}/{:?} short={}/{} height={}/{} vrf_cmp={:?} hash_cmp={:?} oracle={} rust={}",
        pair.label,
        slot(&pair.tip),
        slot(&pair.candidate),
        pair.tip.min_window_density.as_u32(),
        pair.candidate.min_window_density.as_u32(),
        pair.tip
            .sub_window_densities
            .iter()
            .map(|d| d.as_u32())
            .collect::<Vec<_>>(),
        pair.candidate
            .sub_window_densities
            .iter()
            .map(|d| d.as_u32())
            .collect::<Vec<_>>(),
        oracle_is_short_range(&pair.tip, &pair.candidate),
        is_short_range_fork(&pair.tip, &pair.candidate),
        pair.tip.blockchain_length.as_u32(),
        pair.candidate.blockchain_length.as_u32(),
        pair.candidate.last_vrf_output.blake2b().cmp(&pair.tip.last_vrf_output.blake2b()),
        pair.candidate_hash.cmp(&pair.tip_hash),
        want,
        got,
    )
}

fn compare_generated() -> Diff {
    let mut diff = Diff::default();
    for pair in generated_pairs() {
        diff.cases += 1;
        let want = oracle_select(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        let got = consensus_take(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        if want != got {
            let entry = diff
                .choice_mismatches
                .entry(class(&pair))
                .or_insert((0, compact(&pair, want, got)));
            entry.0 += 1;
        }
        let max_slot = slot(&pair.tip).max(slot(&pair.candidate));
        if oracle_is_short_range(&pair.tip, &pair.candidate) {
            continue;
        }
        for state in [&pair.tip, &pair.candidate] {
            let want = oracle_virtual_min_window_density(state, max_slot);
            let got = relative_min_window_density(
                state,
                if std::ptr::eq(state, &pair.tip) {
                    &pair.candidate
                } else {
                    &pair.tip
                },
            );
            if want != got {
                let name = class(&pair);
                let entry = diff.density_mismatches.entry(name).or_insert((
                    0,
                    format!(
                        "{} projected={} to={} min={} buckets={:?} oracle={} rust={}",
                        pair.label,
                        slot(state),
                        max_slot,
                        state.min_window_density.as_u32(),
                        state
                            .sub_window_densities
                            .iter()
                            .map(|d| d.as_u32())
                            .collect::<Vec<_>>(),
                        want,
                        got
                    ),
                ));
                entry.0 += 1;
            }
        }
    }
    diff
}

#[test]
fn generated_differential_report() {
    let diff = compare_generated();
    eprintln!("P4.11a generated cases: {}", diff.cases);
    for (class, (count, example)) in &diff.choice_mismatches {
        eprintln!("choice mismatch {class}: {count}; first: {example}");
    }
    for (class, (count, example)) in &diff.density_mismatches {
        eprintln!("density mismatch {class}: {count}; first: {example}");
    }
    assert_eq!(diff.cases, 12_005);
}

// Ignored until P4.11 ported OCaml 4.0.2's selection.
#[test]
fn generated_differential_must_agree_after_fix() {
    let diff = compare_generated();
    assert!(
        diff.choice_mismatches.is_empty(),
        "{:?}",
        diff.choice_mismatches
    );
    assert!(
        diff.density_mismatches.is_empty(),
        "{:?}",
        diff.density_mismatches
    );
}

struct CorpusBlock {
    hash: StateHash,
    previous_hash: String,
    state: State,
    /// The block's `blockchain_state` snarked ledger hash, which its
    /// children's consensus transition reads.
    snarked_ledger_hash: LedgerHash,
    genesis_ledger_hash: LedgerHash,
}

// Precomputed blocks may contain malformed escaped Unicode in unrelated
// transaction fields. Extract the balanced protocol_state object so the
// consensus test parses exactly the fields it needs.
fn protocol_state_slice(json: &[u8]) -> &[u8] {
    let marker = b"\"protocol_state\"";
    let start = json
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    let open = (start + marker.len()..json.len())
        .find(|&i| json[i] == b'{')
        .unwrap();
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, &byte) in json.iter().enumerate().skip(open) {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return &json[open..=i];
            }
        }
    }
    panic!("unclosed protocol_state object")
}

fn read_block(path: &Path) -> CorpusBlock {
    let bytes = fs::read(path).unwrap();
    let json = if path.extension().is_some_and(|e| e == "zst") {
        zstd::decode_all(bytes.as_slice()).unwrap()
    } else {
        bytes
    };
    let mut state_json: Value = serde_json::from_slice(protocol_state_slice(&json))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let previous_hash = state_json["previous_state_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let state = serde_json::from_value(state_json["body"]["consensus_state"].take()).unwrap();
    let blockchain_state = &mut state_json["body"]["blockchain_state"];
    let snarked_ledger_hash = serde_json::from_value(
        blockchain_state["ledger_proof_statement"]["target"]["first_pass_ledger"].take(),
    )
    .unwrap();
    let genesis_ledger_hash =
        serde_json::from_value(blockchain_state["genesis_ledger_hash"].take()).unwrap();
    let filename = path.file_name().unwrap().to_string_lossy();
    let hash = filename
        .split('-')
        .next_back()
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    CorpusBlock {
        hash,
        previous_hash,
        state,
        snarked_ledger_hash,
        genesis_ledger_hash,
    }
}

fn chain_file(root: &Path, name: &str) -> PathBuf {
    let zst = root.join(format!("{name}.json.zst"));
    if zst.exists() {
        zst
    } else {
        root.join(format!("{name}.json"))
    }
}

fn compare_real_pair(a: &CorpusBlock, b: &CorpusBlock) {
    let oracle = oracle_select(&a.state, &b.state, &a.hash, &b.hash);
    let rust = consensus_take(&a.state, &b.state, &a.hash, &b.hash);
    assert_eq!(oracle, rust, "corpus pair {} -> {}", a.hash, b.hash);
    if !oracle_is_short_range(&a.state, &b.state) {
        let max_slot = slot(&a.state).max(slot(&b.state));
        assert_eq!(
            oracle_virtual_min_window_density(&a.state, max_slot),
            relative_min_window_density(&a.state, &b.state),
            "density {}",
            a.hash
        );
        assert_eq!(
            oracle_virtual_min_window_density(&b.state, max_slot),
            relative_min_window_density(&b.state, &a.state),
            "density {}",
            b.hash
        );
    }
}

fn check_chain(root: &Path, blocks_dir: &Path, sample_limit: usize) -> usize {
    let names: Vec<String> = fs::read_to_string(root.join("chain.txt"))
        .unwrap()
        .lines()
        .map(|s| s.replace(' ', "-"))
        .collect();
    let pairs = names.len() - 1;
    let mut selected: Vec<usize> = (0..pairs.min(sample_limit / 2)).collect();
    selected.extend(pairs.saturating_sub(sample_limit / 2)..pairs);
    if pairs > sample_limit {
        let step = pairs / (sample_limit / 2).max(1);
        selected.extend((0..pairs).step_by(step));
    }
    selected.sort_unstable();
    selected.dedup();
    let constants = consensus_constants();
    let mut cache = BTreeMap::new();
    for i in &selected {
        for j in [*i, *i + 1] {
            cache
                .entry(j)
                .or_insert_with(|| read_block(&chain_file(blocks_dir, &names[j])));
        }
        let a = &cache[i];
        let b = &cache[&(*i + 1)];
        assert_eq!(b.previous_hash, a.hash.to_string(), "chain linkage at {i}");
        check_real_transition(&constants, a, b);
        compare_real_pair(a, b);
        assert!(
            oracle_select(&a.state, &b.state, &a.hash, &b.hash),
            "child must win over parent at chain index {i}"
        );
    }
    selected.len()
}

fn check_forks(root: &Path, network: &str) -> usize {
    let mut blocks: BTreeMap<String, CorpusBlock> = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if !name.starts_with(network) || !name.ends_with(".json") {
            continue;
        }
        let version: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if version["data"]["protocol_version"]["transaction"] != 4 {
            continue;
        }
        let block = read_block(&path);
        blocks.insert(block.hash.to_string(), block);
    }
    let mut by_parent: BTreeMap<&str, Vec<&CorpusBlock>> = BTreeMap::new();
    for block in blocks.values() {
        by_parent
            .entry(&block.previous_hash)
            .or_default()
            .push(block);
    }
    let mut count = 0;
    for siblings in by_parent.values() {
        for i in 0..siblings.len() {
            for j in i + 1..siblings.len() {
                compare_real_pair(siblings[i], siblings[j]);
                compare_real_pair(siblings[j], siblings[i]);
                count += 2;
            }
        }
    }
    count
}

/// Local saved corpora only; CI has no `.work` or `/tmp/r13-rev`.
/// Set MINA_SELECTION_CORPUS_ROOT to the main checkout and
/// MINA_SELECTION_FORK_DIR to R13's saved block directory.
#[test]
fn saved_mesa_corpus_agrees() {
    let Ok(root) = std::env::var("MINA_SELECTION_CORPUS_ROOT") else {
        eprintln!(
            "P4.11a corpus skipped: set MINA_SELECTION_CORPUS_ROOT and MINA_SELECTION_FORK_DIR"
        );
        return;
    };
    let root = Path::new(&root);
    let mainnet = check_chain(
        &root.join(".work/replay"),
        &root.join(".work/replay/blocks"),
        512,
    );
    let devnet = check_chain(
        &root.join(".work/replay-devnet"),
        &root.join(".work/replay-devnet"),
        512,
    );
    let genesis = check_chain(
        &root.join(".work/replay-devnet/genesis-head"),
        &root.join(".work/replay-devnet/genesis-head"),
        512,
    );
    let fork_dir = std::env::var("MINA_SELECTION_FORK_DIR")
        .expect("set MINA_SELECTION_FORK_DIR for the saved R13 forks");
    let fork_dir = Path::new(&fork_dir);
    let mainnet_forks = check_forks(fork_dir, "mainnet");
    let devnet_forks = check_forks(fork_dir, "devnet");
    assert!(mainnet_forks > 0 && devnet_forks > 0, "expected Mesa forks");
    eprintln!("P4.11a corpus agreements: mainnet {mainnet}, devnet {devnet}, genesis {genesis}, mainnet fork orientations {mainnet_forks}, devnet fork orientations {devnet_forks}");
}

// ---------------------------------------------------------------------------
// P4.11: reachable fork pairs.
//
// Every state below comes from genesis or from a saved Mesa block by the
// consensus transition, block by block: Rust's `next_consensus_state`, which
// the block producer uses, checked at every block against the transcription
// of OCaml's `Consensus_state.update` below. Slots are any increasing ones, as
// any slot can be won by enough stake. VRF outputs and state hashes are
// random field elements, as both are hash outputs; only lock checkpoints and
// the last tie-break read state hashes.

use ark_ff::{BigInteger, BigInteger256, PrimeField};
use mina_core::consensus::{next_consensus_state, ConsensusConstants, ConsensusStateUpdate};
use mina_curves::pasta::Fp;
use mina_p2p_messages::v2;

/// OCaml `checkpoint_window_size_in_slots` for Mesa's 90-second slots: a
/// year's slots over `checkpoints_per_year`, 12.
const CHECKPOINT_WINDOW_SIZE_IN_SLOTS: u32 = (365u64 * 24 * 60 * 60 * 1000 / 90_000 / 12) as u32;

fn consensus_constants() -> ConsensusConstants {
    ConsensusConstants::create(
        mina_core::constants::constraint_constants(),
        &mina_core::constants::PROTOCOL_CONSTANTS,
    )
}

/// What a block brings besides its parent: the arguments of OCaml's
/// `Consensus_state.update`.
#[derive(Clone, Debug)]
struct BlockInputs {
    slot: u32,
    vrf_hash: Fp,
    last_vrf_output: v2::ConsensusVrfOutputTruncatedStableV1,
    /// The parent's snarked ledger hash.
    snarked_ledger_hash: LedgerHash,
    genesis_ledger_hash: LedgerHash,
    total_currency: v2::CurrencyAmountStableV1,
    winner: v2::NonZeroCurvePoint,
    creator: v2::NonZeroCurvePoint,
    receiver: v2::NonZeroCurvePoint,
    supercharge_coinbase: bool,
}

fn global_slot(slot: u32) -> v2::ConsensusGlobalSlotStableV1 {
    v2::ConsensusGlobalSlotStableV1 {
        slot_number: MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(slot.into()),
        slots_per_epoch: SLOTS_PER_EPOCH.into(),
    }
}

impl BlockInputs {
    fn update(&self) -> ConsensusStateUpdate {
        ConsensusStateUpdate {
            curr_global_slot_since_hard_fork: global_slot(self.slot),
            vrf_hash: self.vrf_hash,
            last_vrf_output: self.last_vrf_output.clone(),
            snarked_ledger_hash: self.snarked_ledger_hash.clone(),
            genesis_ledger_hash: self.genesis_ledger_hash.clone(),
            total_currency: self.total_currency.clone(),
            block_stake_winner: self.winner.clone(),
            block_creator: self.creator.clone(),
            coinbase_receiver: self.receiver.clone(),
            supercharge_coinbase: self.supercharge_coinbase,
        }
    }
}

/// OCaml `Epoch_seed.update`, proof_of_stake.ml:140-144.
fn oracle_epoch_seed_update(seed: &v2::EpochSeed, vrf_result: Fp) -> v2::EpochSeed {
    let seed: Fp = seed.to_field().unwrap();
    let hash = poseidon::hash::hash_with_kimchi(
        &poseidon::hash::params::MINA_EPOCH_SEED,
        &[seed, vrf_result],
    );
    v2::MinaBaseEpochSeedStableV1(hash.into()).into()
}

/// OCaml `Consensus_state.update`, proof_of_stake.ml:1855-1955, with
/// `Epoch_data.update_pair` (1062-1109) and `update_min_window_density`
/// (1165-1279, `incr_window` true). Without `update_seed` the seed stays
/// as it is, which only the shrinking search below uses: no selection input
/// depends on the seed.
fn oracle_update(prev: &State, prev_hash: &StateHash, b: &BlockInputs, update_seed: bool) -> State {
    let (prev_epoch, _) = epoch_and_slot(prev);
    let (next_epoch, next_slot) = (b.slot / SLOTS_PER_EPOCH, b.slot % SLOTS_PER_EPOCH);
    assert!(
        b.slot == 0 || slot(prev) < b.slot,
        "(epoch, slot) did not increase"
    );
    let slot_diff = b.slot - slot(prev);
    let next_staking_ledger = if b.snarked_ledger_hash == b.genesis_ledger_hash {
        prev.next_epoch_data.ledger.clone()
    } else {
        v2::MinaBaseEpochLedgerValueStableV1 {
            hash: b.snarked_ledger_hash.clone(),
            total_currency: b.total_currency.clone(),
        }
    };
    let (staking, mut next, epoch_count) = if next_epoch > prev_epoch {
        let n = &prev.next_epoch_data;
        (
            v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 {
                ledger: n.ledger.clone(),
                seed: n.seed.clone(),
                start_checkpoint: n.start_checkpoint.clone(),
                lock_checkpoint: n.lock_checkpoint.clone(),
                epoch_length: n.epoch_length,
            },
            v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
                seed: n.seed.clone(),
                ledger: next_staking_ledger,
                start_checkpoint: prev_hash.clone(),
                lock_checkpoint: StateHash::zero(),
                epoch_length: 1u32.into(),
            },
            (prev.epoch_count.as_u32() + 1).into(),
        )
    } else {
        assert_eq!(next_epoch, prev_epoch);
        let mut n = prev.next_epoch_data.clone();
        n.epoch_length = (n.epoch_length.as_u32() + 1).into();
        (prev.staking_epoch_data.clone(), n, prev.epoch_count)
    };
    // `Slot.in_seed_update_range`.
    if next_slot < (SLOTS_PER_EPOCH / 3) * 2 {
        if update_seed {
            next.seed = oracle_epoch_seed_update(&next.seed, b.vrf_hash);
        }
        next.lock_checkpoint = prev_hash.clone();
    }
    let densities = prev
        .sub_window_densities
        .iter()
        .map(|d| d.as_u32())
        .collect::<Vec<_>>();
    let (min_window_density, sub_window_densities) = oracle_update_min_window_density(
        slot(prev),
        b.slot,
        &densities,
        prev.min_window_density.as_u32(),
        true,
    );
    State {
        blockchain_length: (prev.blockchain_length.as_u32() + 1).into(),
        epoch_count,
        min_window_density: min_window_density.into(),
        sub_window_densities: sub_window_densities.into_iter().map(Into::into).collect(),
        last_vrf_output: b.last_vrf_output.clone(),
        total_currency: b.total_currency.clone(),
        curr_global_slot_since_hard_fork: global_slot(b.slot),
        global_slot_since_genesis: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
            (prev.global_slot_since_genesis.as_u32() + slot_diff).into(),
        ),
        staking_epoch_data: staking,
        next_epoch_data: next,
        has_ancestor_in_same_checkpoint_window: slot(prev) / CHECKPOINT_WINDOW_SIZE_IN_SLOTS
            == b.slot / CHECKPOINT_WINDOW_SIZE_IN_SLOTS,
        block_stake_winner: b.winner.clone(),
        block_creator: b.creator.clone(),
        coinbase_receiver: b.receiver.clone(),
        supercharge_coinbase: b.supercharge_coinbase,
    }
}

/// The truncated VRF output of a VRF output hash: its low 253 bits, as
/// `mina_vrf`'s `VrfOutput::truncated`.
fn truncate(vrf_hash: Fp) -> v2::ConsensusVrfOutputTruncatedStableV1 {
    let mut bytes = vrf_hash.into_bigint().to_bytes_le();
    bytes[31] &= 0x1f;
    v2::ConsensusVrfOutputTruncatedStableV1(bytes.into())
}

/// The VRF output hashes a truncated output can come from: its two missing
/// high bits set every way.
fn vrf_hash_candidates(truncated: &v2::ConsensusVrfOutputTruncatedStableV1) -> Vec<Fp> {
    let bytes: &[u8] = truncated.0.as_ref();
    let mut limbs = [0u64; 4];
    for (limb, chunk) in limbs.iter_mut().zip(bytes.chunks(8)) {
        *limb = u64::from_le_bytes(chunk.try_into().unwrap());
    }
    (0..4u64)
        .filter_map(|high| {
            let mut limbs = limbs;
            limbs[3] |= high << 61;
            Fp::from_bigint(BigInteger256::new(limbs))
        })
        .collect()
}

/// A saved block's consensus state is its parent's after the transition,
/// both Rust's and the oracle's, with the VRF output hash whose seed update
/// gives the saved seed.
fn check_real_transition(
    constants: &ConsensusConstants,
    parent: &CorpusBlock,
    child: &CorpusBlock,
) {
    let candidates = vrf_hash_candidates(&child.state.last_vrf_output);
    let matched = candidates.into_iter().any(|vrf_hash| {
        let inputs = BlockInputs {
            slot: slot(&child.state),
            vrf_hash,
            last_vrf_output: child.state.last_vrf_output.clone(),
            snarked_ledger_hash: parent.snarked_ledger_hash.clone(),
            genesis_ledger_hash: parent.genesis_ledger_hash.clone(),
            total_currency: child.state.total_currency.clone(),
            winner: child.state.block_stake_winner.clone(),
            creator: child.state.block_creator.clone(),
            receiver: child.state.coinbase_receiver.clone(),
            supercharge_coinbase: child.state.supercharge_coinbase,
        };
        let rust =
            next_consensus_state(constants, &parent.state, &parent.hash, inputs.update()).unwrap();
        let ocaml = oracle_update(&parent.state, &parent.hash, &inputs, true);
        assert_eq!(rust, ocaml, "transition {} -> {}", parent.hash, child.hash);
        rust == child.state
    });
    assert!(
        matched,
        "saved consensus state of {} is not its parent's transition",
        child.hash
    );
}

impl Rng {
    fn next_u64(&mut self) -> u64 {
        (u64::from(self.next()) << 32) | u64::from(self.next())
    }
    /// Within `[low, high)`.
    fn range(&mut self, low: u32, high: u32) -> u32 {
        low + self.below(high - low)
    }
}

fn random_fp(rng: &mut Rng) -> Fp {
    let limbs = [
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64() >> 2,
    ];
    Fp::from_bigint(BigInteger256::new(limbs)).unwrap()
}

/// A generated block.
#[derive(Clone, Debug)]
struct Gen {
    state: State,
    hash: StateHash,
    /// This block's own snarked ledger hash, which its children read.
    snarked_ledger_hash: LedgerHash,
}

impl From<&CorpusBlock> for Gen {
    fn from(block: &CorpusBlock) -> Self {
        Self {
            state: block.state.clone(),
            hash: block.hash.clone(),
            snarked_ledger_hash: block.snarked_ledger_hash.clone(),
        }
    }
}

/// What makes a generated block on top of any parent.
#[derive(Clone, Debug)]
struct Gene {
    inputs: BlockInputs,
    hash: StateHash,
    snarked_ledger_hash: LedgerHash,
}

struct World {
    constants: ConsensusConstants,
    template: State,
    genesis_ledger_hash: LedgerHash,
    /// Transitions checked against the oracle.
    checked: usize,
}

impl World {
    fn new() -> Self {
        Self {
            constants: consensus_constants(),
            template: fixture_state(TIP_FIXTURE),
            genesis_ledger_hash: LedgerHash::from_fp(Fp::from(411u64)),
            checked: 0,
        }
    }

    /// OCaml `create_genesis`: the genesis transition, to slot 0, from
    /// `negative_one`, proof_of_stake.ml:1983-2075. Ledger hashes, seeds and
    /// keys are the template's.
    fn genesis(&mut self, rng: &mut Rng) -> Gen {
        let mut negative_one = self.template.clone();
        negative_one.blockchain_length = 0u32.into();
        negative_one.epoch_count = 0u32.into();
        negative_one.min_window_density = (SLOTS_PER_SUB_WINDOW * SUB_WINDOWS_PER_WINDOW).into();
        negative_one.sub_window_densities = std::iter::once(0u32)
            .chain(std::iter::repeat_n(
                SLOTS_PER_SUB_WINDOW,
                SUB_WINDOWS_PER_WINDOW as usize - 1,
            ))
            .map(Into::into)
            .collect();
        negative_one.curr_global_slot_since_hard_fork = global_slot(0);
        negative_one.global_slot_since_genesis =
            v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(0u32.into());
        negative_one.has_ancestor_in_same_checkpoint_window = false;
        let ledger = v2::MinaBaseEpochLedgerValueStableV1 {
            hash: self.genesis_ledger_hash.clone(),
            total_currency: negative_one.total_currency.clone(),
        };
        negative_one.staking_epoch_data.ledger = ledger.clone();
        negative_one.next_epoch_data.ledger = ledger;
        for checkpoint in [
            &mut negative_one.staking_epoch_data.start_checkpoint,
            &mut negative_one.staking_epoch_data.lock_checkpoint,
            &mut negative_one.next_epoch_data.start_checkpoint,
            &mut negative_one.next_epoch_data.lock_checkpoint,
        ] {
            *checkpoint = StateHash::zero();
        }
        negative_one.staking_epoch_data.epoch_length = 1u32.into();
        negative_one.next_epoch_data.epoch_length = 1u32.into();
        let parent = Gen {
            state: negative_one,
            hash: StateHash::from_fp(random_fp(rng)),
            snarked_ledger_hash: self.genesis_ledger_hash.clone(),
        };
        let gene = self.gene(rng, &parent, 0);
        self.apply(&parent, &gene, true)
    }

    fn gene(&self, rng: &mut Rng, parent: &Gen, slot: u32) -> Gene {
        let vrf_hash = random_fp(rng);
        let snarked_ledger_hash = if rng.below(16) == 0 {
            LedgerHash::from_fp(random_fp(rng))
        } else {
            parent.snarked_ledger_hash.clone()
        };
        let total_currency = parent.state.total_currency.0.as_u64() + u64::from(rng.below(1000));
        Gene {
            inputs: BlockInputs {
                slot,
                vrf_hash,
                last_vrf_output: truncate(vrf_hash),
                snarked_ledger_hash: parent.snarked_ledger_hash.clone(),
                genesis_ledger_hash: self.genesis_ledger_hash.clone(),
                total_currency: v2::CurrencyAmountStableV1(total_currency.into()),
                winner: self.template.block_stake_winner.clone(),
                creator: self.template.block_creator.clone(),
                receiver: self.template.coinbase_receiver.clone(),
                supercharge_coinbase: rng.below(2) == 0,
            },
            hash: StateHash::from_fp(random_fp(rng)),
            snarked_ledger_hash,
        }
    }

    /// The block `gene` makes on `parent`: Rust's transition, and with
    /// `check` the oracle's too, which must agree.
    fn apply(&mut self, parent: &Gen, gene: &Gene, check: bool) -> Gen {
        let mut inputs = gene.inputs.clone();
        inputs.snarked_ledger_hash = parent.snarked_ledger_hash.clone();
        let state = if check {
            let rust = next_consensus_state(
                &self.constants,
                &parent.state,
                &parent.hash,
                inputs.update(),
            )
            .unwrap();
            let ocaml = oracle_update(&parent.state, &parent.hash, &inputs, true);
            assert_eq!(rust, ocaml, "transition to slot {}", inputs.slot);
            self.checked += 1;
            rust
        } else {
            oracle_update(&parent.state, &parent.hash, &inputs, false)
        };
        Gen {
            state,
            hash: gene.hash.clone(),
            snarked_ledger_hash: gene.snarked_ledger_hash.clone(),
        }
    }
}

/// A chain's block rate, in blocks per thousand slots, in phases of 7 to 706
/// slots: an outage, a sparse, an ordinary or a dense phase.
struct Pace {
    permille: u32,
    left: u32,
    realistic: bool,
}

impl Pace {
    fn new(realistic: bool) -> Self {
        Self {
            permille: 0,
            left: 0,
            realistic,
        }
    }

    fn next(&mut self, rng: &mut Rng) -> u32 {
        if self.left == 0 {
            self.left = rng.range(7, 707);
            self.permille = match rng.below(100) {
                0..=7 if !self.realistic => 0,
                0..=27 if !self.realistic => rng.range(20, 150),
                0..=77 => rng.range(200, 500),
                _ => rng.range(500, 950),
            };
        }
        self.left -= 1;
        self.permille
    }
}

/// A chain grown from `from` over the following `span` slots.
fn grow(world: &mut World, rng: &mut Rng, from: &Gen, span: u32, realistic: bool) -> Vec<Gen> {
    let mut pace = Pace::new(realistic);
    let mut chain = vec![from.clone()];
    let start = slot(&from.state);
    for s in start + 1..=start + span {
        if rng.below(1000) < pace.next(rng) {
            let parent = chain.last().unwrap();
            let gene = world.gene(rng, parent, s);
            let block = world.apply(parent, &gene, true);
            chain.push(block);
        }
    }
    chain
}

/// A mismatch on reachable states: the fork point and the blocks of each
/// side after it; `existing` and `candidate` are the last of each, or the
/// fork point.
#[derive(Clone)]
struct Witness {
    fork: Gen,
    existing: Vec<Gene>,
    candidate: Vec<Gene>,
}

impl Witness {
    fn blocks(&self) -> usize {
        self.existing.len() + self.candidate.len()
    }

    /// The two tips, by the oracle's transition only, as every block has
    /// been checked already.
    fn tips(&self, world: &mut World) -> (Gen, Gen) {
        let mut tip = |genes: &[Gene]| {
            genes.iter().fold(self.fork.clone(), |parent, gene| {
                world.apply(&parent, gene, false)
            })
        };
        let existing = tip(&self.existing);
        let candidate = tip(&self.candidate);
        (existing, candidate)
    }

    fn describe(&self, world: &mut World) -> String {
        let slots = |genes: &[Gene]| genes.iter().map(|g| g.inputs.slot).collect::<Vec<_>>();
        let (existing, candidate) = self.tips(world);
        let pair = Pair {
            tip: existing.state,
            candidate: candidate.state,
            tip_hash: existing.hash,
            candidate_hash: candidate.hash,
            label: format!(
                "fork slot {} height {} min {} buckets {:?}; existing +{} blocks at slots {:?}; candidate +{} blocks at slots {:?}",
                slot(&self.fork.state),
                self.fork.state.blockchain_length.as_u32(),
                self.fork.state.min_window_density.as_u32(),
                self.fork
                    .state
                    .sub_window_densities
                    .iter()
                    .map(|d| d.as_u32())
                    .collect::<Vec<_>>(),
                self.existing.len(),
                slots(&self.existing),
                self.candidate.len(),
                slots(&self.candidate),
            ),
        };
        let want = oracle_select(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        let got = consensus_take(
            &pair.tip,
            &pair.candidate,
            &pair.tip_hash,
            &pair.candidate_hash,
        );
        compact(&pair, want, got)
    }
}

/// How many of the smallest witnesses of each mismatch class to keep.
const WITNESSES_KEPT: usize = 8;

#[derive(Default)]
struct Reachable {
    pairs: usize,
    trials: usize,
    by_class: BTreeMap<&'static str, usize>,
    /// Per class, the mismatches and the smallest witnesses, smallest first.
    mismatches: BTreeMap<&'static str, (usize, Vec<Witness>)>,
}

fn classify(tip: &State, candidate: &State) -> &'static str {
    if oracle_is_short_range(tip, candidate) {
        if epoch_and_slot(tip).0 == epoch_and_slot(candidate).0 {
            "short range"
        } else {
            "short range, adjacent epochs"
        }
    } else {
        let a = slot(tip);
        let b = slot(candidate);
        let (low, high) = (a.min(b), a.max(b));
        if high < GRACE_PERIOD_END {
            "long range, grace"
        } else if a == b {
            "long range, equal slot"
        } else if low / SLOTS_PER_SUB_WINDOW == high / SLOTS_PER_SUB_WINDOW {
            "long range, same sub-window"
        } else if low / SLOTS_PER_SUB_WINDOW + SUB_WINDOWS_PER_WINDOW < high / SLOTS_PER_SUB_WINDOW
        {
            "long range, non-overlap"
        } else {
            "long range, projected ring"
        }
    }
}

impl Reachable {
    fn compare(&mut self, existing: &Gen, candidate: &Gen, witness: impl FnOnce() -> Witness) {
        self.pairs += 1;
        let class = classify(&existing.state, &candidate.state);
        *self.by_class.entry(class).or_default() += 1;
        let want = oracle_select(
            &existing.state,
            &candidate.state,
            &existing.hash,
            &candidate.hash,
        );
        let got = consensus_take(
            &existing.state,
            &candidate.state,
            &existing.hash,
            &candidate.hash,
        );
        if want != got {
            let (count, smallest) = self.mismatches.entry(class).or_default();
            *count += 1;
            let witness = witness();
            let at = smallest.partition_point(|w| w.blocks() <= witness.blocks());
            if at < WITNESSES_KEPT {
                smallest.insert(at, witness);
                smallest.truncate(WITNESSES_KEPT);
            }
        }
    }
}

/// One fork: a fork point on `trunk` and two chains from it over the same
/// slots, each slot's blocks compared with the other chain's tip, both ways,
/// and now and then with the fork point.
fn fork_trial(
    world: &mut World,
    rng: &mut Rng,
    trunk: &[Gen],
    from_genesis: bool,
    out: &mut Reachable,
) {
    let first = slot(&trunk[0].state);
    let last = slot(&trunk.last().unwrap().state);
    let epoch = rng.range(first / SLOTS_PER_EPOCH, last / SLOTS_PER_EPOCH + 1) * SLOTS_PER_EPOCH;
    // Half the forks last into the next epoch; those from before the seed
    // update range ends, slot 4760, are long-range forks there.
    let long = rng.below(2) == 0;
    let target = match rng.below(6) {
        0 if from_genesis => rng.below(2300),
        0 | 1 if long => epoch + rng.range(2000, 4760),
        1 => epoch + rng.range(4700, 4800),
        2 => epoch + rng.range(3000, 4760),
        3 => epoch + rng.range(7000, 7140),
        4 => rng.range(first, last + 1) / SLOTS_PER_SUB_WINDOW * SLOTS_PER_SUB_WINDOW + 6,
        _ => rng.range(first, last + 1),
    };
    let fork = trunk
        .iter()
        .rev()
        .find(|b| slot(&b.state) <= target)
        .unwrap_or(&trunk[0])
        .clone();
    let fork_slot = slot(&fork.state);
    let span = if long {
        SLOTS_PER_EPOCH - fork_slot % SLOTS_PER_EPOCH + rng.range(1, 3500)
    } else if rng.below(5) < 3 {
        rng.range(1, 80)
    } else {
        rng.range(80, 1500)
    };
    let realistic = rng.below(3) != 0;
    let shared_pace = rng.below(3) == 0;
    let (mut pace_a, mut pace_c) = (Pace::new(realistic), Pace::new(realistic));
    // Now and then a steady thin chain on each side, one to four blocks a
    // window, as after most stake goes offline.
    if rng.below(4) == 0 {
        for pace in [&mut pace_a, &mut pace_c] {
            pace.permille = rng.range(13, 60);
            pace.left = u32::MAX;
        }
    }
    let (mut tip_a, mut tip_c) = (fork.clone(), fork.clone());
    let (mut genes_a, mut genes_c): (Vec<Gene>, Vec<Gene>) = (vec![], vec![]);
    for s in fork_slot + 1..=fork_slot + span {
        let p_a = pace_a.next(rng);
        let p_c = if shared_pace { p_a } else { pace_c.next(rng) };
        let new_a = rng.below(1000) < p_a;
        let new_c = rng.below(1000) < p_c;
        if new_a {
            let gene = world.gene(rng, &tip_a, s);
            tip_a = world.apply(&tip_a, &gene, true);
            genes_a.push(gene);
        }
        if new_c {
            let gene = world.gene(rng, &tip_c, s);
            tip_c = world.apply(&tip_c, &gene, true);
            genes_c.push(gene);
        }
        if !(new_a || new_c) {
            continue;
        }
        let witness = |existing: &[Gene], candidate: &[Gene]| Witness {
            fork: fork.clone(),
            existing: existing.to_vec(),
            candidate: candidate.to_vec(),
        };
        out.compare(&tip_a, &tip_c, || witness(&genes_a, &genes_c));
        out.compare(&tip_c, &tip_a, || witness(&genes_c, &genes_a));
        if s % 61 == 0 {
            for (tip, genes) in [(&tip_a, &genes_a), (&tip_c, &genes_c)] {
                out.compare(&fork, tip, || witness(&[], genes));
                out.compare(tip, &fork, || witness(genes, &[]));
            }
        }
    }
    out.trials += 1;
}

/// Saved Mesa blocks to fork from, when `MINA_SELECTION_CORPUS_ROOT` is set:
/// some of mainnet's and devnet's chains.
fn corpus_bases() -> Vec<CorpusBlock> {
    let Ok(root) = std::env::var("MINA_SELECTION_CORPUS_ROOT") else {
        return vec![];
    };
    let root = Path::new(&root);
    let mut bases = vec![];
    for (chain, blocks, step) in [
        (".work/replay/chain.txt", ".work/replay/blocks", 1_500),
        (
            ".work/replay-devnet/chain.txt",
            ".work/replay-devnet",
            2_500,
        ),
    ] {
        let names = fs::read_to_string(root.join(chain)).unwrap();
        for name in names.lines().step_by(step) {
            bases.push(read_block(&chain_file(
                &root.join(blocks),
                &name.replace(' ', "-"),
            )));
        }
    }
    bases
}

/// Fork pairs on reachable states until there are `target_pairs` from at
/// least `target_forks` forks.
fn reachable_pairs((target_pairs, target_forks): (usize, usize)) -> (Reachable, World) {
    let mut rng = Rng(0x0411_b10c_5eed_0001);
    let mut world = World::new();
    let mut trunks: Vec<(Vec<Gen>, bool)> = vec![];
    let genesis = world.genesis(&mut rng);
    let trunk = grow(&mut world, &mut rng, &genesis, 3 * SLOTS_PER_EPOCH, true);
    trunks.push((trunk, true));
    for base in corpus_bases() {
        let base = Gen::from(&base);
        let trunk = grow(&mut world, &mut rng, &base, SLOTS_PER_EPOCH + 2_000, true);
        trunks.push((trunk, false));
    }
    let mut out = Reachable::default();
    while out.pairs < target_pairs || out.trials < target_forks {
        let (trunk, from_genesis) = &trunks[rng.below(trunks.len() as u32) as usize];
        fork_trial(&mut world, &mut rng, trunk, *from_genesis, &mut out);
    }
    (out, world)
}

/// How many pairs to compare at least, from how many forks:
/// `MINA_REACHABLE_PAIRS` and `MINA_REACHABLE_FORKS` override them.
fn reachable_target() -> (usize, usize) {
    let var = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(default)
    };
    (
        var("MINA_REACHABLE_PAIRS", 60_000),
        var("MINA_REACHABLE_FORKS", 40),
    )
}

#[test]
fn reachable_differential_report() {
    let start = std::time::Instant::now();
    let (out, mut world) = reachable_pairs(reachable_target());
    eprintln!(
        "P4.11 reachable pairs: {} in {} forks, {} transitions checked against the oracle, {:.1} s",
        out.pairs,
        out.trials,
        world.checked,
        start.elapsed().as_secs_f64()
    );
    for (class, count) in &out.by_class {
        eprintln!("reachable {class}: {count}");
    }
    for (class, (count, witnesses)) in &out.mismatches {
        eprintln!(
            "reachable choice mismatch {class}: {count}; smallest ({} blocks): {}",
            witnesses[0].blocks(),
            witnesses[0].describe(&mut world)
        );
    }
    assert!(out.pairs >= reachable_target().0);
    // Before P4.11, three long-range classes differed here.
    assert!(out.mismatches.is_empty());
}

/// The fields of a consensus state that selection reads, and their
/// transition, as `oracle_update` makes them: to shrink witnesses fast.
#[derive(Clone)]
struct Lite {
    slot: u32,
    length: u32,
    min: u32,
    densities: Vec<u32>,
    staking_lock: StateHash,
    next_lock: StateHash,
}

impl Lite {
    fn of(state: &State) -> Self {
        Self {
            slot: slot(state),
            length: state.blockchain_length.as_u32(),
            min: state.min_window_density.as_u32(),
            densities: state
                .sub_window_densities
                .iter()
                .map(|d| d.as_u32())
                .collect(),
            staking_lock: state.staking_epoch_data.lock_checkpoint.clone(),
            next_lock: state.next_epoch_data.lock_checkpoint.clone(),
        }
    }

    fn child(&self, prev_hash: &StateHash, slot: u32) -> Self {
        let (staking_lock, mut next_lock) = if slot / SLOTS_PER_EPOCH > self.slot / SLOTS_PER_EPOCH
        {
            (self.next_lock.clone(), StateHash::zero())
        } else {
            (self.staking_lock.clone(), self.next_lock.clone())
        };
        if slot % SLOTS_PER_EPOCH < (SLOTS_PER_EPOCH / 3) * 2 {
            next_lock = prev_hash.clone();
        }
        let (min, densities) =
            oracle_update_min_window_density(self.slot, slot, &self.densities, self.min, true);
        Self {
            slot,
            length: self.length + 1,
            min,
            densities,
            staking_lock,
            next_lock,
        }
    }
}

impl Witness {
    /// The tips' states as far as selection reads them.
    fn lite_tips(&self) -> ((State, StateHash), (State, StateHash)) {
        let tip = |genes: &[Gene]| {
            let mut lite = Lite::of(&self.fork.state);
            let mut hash = self.fork.hash.clone();
            let mut state = self.fork.state.clone();
            for gene in genes {
                lite = lite.child(&hash, gene.inputs.slot);
                hash = gene.hash.clone();
            }
            if let Some(last) = genes.last() {
                state.curr_global_slot_since_hard_fork = global_slot(lite.slot);
                state.blockchain_length = lite.length.into();
                state.min_window_density = lite.min.into();
                state.sub_window_densities = lite.densities.into_iter().map(Into::into).collect();
                state.staking_epoch_data.lock_checkpoint = lite.staking_lock;
                state.next_epoch_data.lock_checkpoint = lite.next_lock;
                state.last_vrf_output = last.inputs.last_vrf_output.clone();
            }
            (state, hash)
        };
        (tip(&self.existing), tip(&self.candidate))
    }

    fn mismatches_in(&self, class: &str) -> bool {
        let ((existing, existing_hash), (candidate, candidate_hash)) = self.lite_tips();
        classify(&existing, &candidate) == class
            && oracle_select(&existing, &candidate, &existing_hash, &candidate_hash)
                != consensus_take(&existing, &candidate, &existing_hash, &candidate_hash)
    }

    /// A smaller witness of the same class: blocks removed, in halves, then
    /// quarters, down to single ones, while the mismatch stays. Any subset of
    /// a chain's slots is a chain too.
    fn shrink(mut self, class: &str) -> Self {
        assert!(self.mismatches_in(class));
        loop {
            let before = self.blocks();
            for side in [false, true] {
                let mut chunk = (if side {
                    self.candidate.len()
                } else {
                    self.existing.len()
                } / 2)
                    .max(1);
                loop {
                    let mut i = 0;
                    loop {
                        let genes = if side {
                            &self.candidate
                        } else {
                            &self.existing
                        };
                        if i >= genes.len() {
                            break;
                        }
                        let mut fewer = genes.clone();
                        fewer.drain(i..(i + chunk).min(genes.len()));
                        let mut trial = self.clone();
                        if side {
                            trial.candidate = fewer;
                        } else {
                            trial.existing = fewer;
                        }
                        if trial.mismatches_in(class) {
                            self = trial;
                        } else {
                            i += chunk;
                        }
                    }
                    if chunk == 1 {
                        break;
                    }
                    chunk /= 2;
                }
            }
            if self.blocks() == before {
                return self;
            }
        }
    }

    /// The witness again with every transition checked, Rust's against the
    /// oracle's, and the full states compared.
    fn confirm(&self, world: &mut World, class: &str) -> bool {
        let mut tip = |genes: &[Gene]| {
            genes.iter().fold(self.fork.clone(), |parent, gene| {
                world.apply(&parent, gene, true)
            })
        };
        let existing = tip(&self.existing);
        let candidate = tip(&self.candidate);
        classify(&existing.state, &candidate.state) == class
            && oracle_select(
                &existing.state,
                &candidate.state,
                &existing.hash,
                &candidate.hash,
            ) != consensus_take(
                &existing.state,
                &candidate.state,
                &existing.hash,
                &candidate.hash,
            )
    }
}

/// The smallest reachable witness of each mismatch class: run explicitly,
/// with `--ignored`, for the record in private implementation notes,
/// and with `MINA_P411_WITNESS_DIR` to save them as fixtures. Since P4.11
/// there are none.
#[test]
#[ignore = "slow: shrinks the reachable mismatch witnesses"]
fn reachable_minimal_witnesses() {
    let (out, mut world) = reachable_pairs(reachable_target());
    eprintln!("P4.11 reachable pairs: {}", out.pairs);
    let fixtures = std::env::var("MINA_P411_WITNESS_DIR").ok();
    for (class, (count, witnesses)) in out.mismatches {
        let start = std::time::Instant::now();
        let before = witnesses.iter().map(Witness::blocks).collect::<Vec<_>>();
        let small = witnesses
            .into_iter()
            .map(|witness| witness.shrink(class).normalized(&world))
            .filter(|witness| witness.mismatches_in(class))
            .min_by_key(Witness::blocks)
            .unwrap();
        assert!(small.confirm(&mut world, class), "{class}");
        eprintln!(
            "minimal {class} ({count} mismatches): {before:?} -> {} blocks in {:.1} s: {}",
            small.blocks(),
            start.elapsed().as_secs_f64(),
            small.describe(&mut world)
        );
        if let Some(dir) = &fixtures {
            let name = format!(
                "p411-reachable-{}.json",
                class.replace([' ', ','], "-").replace("--", "-")
            );
            let json = serde_json::to_string_pretty(&small.to_json(&mut world, class)).unwrap();
            fs::write(Path::new(dir).join(name), json + "\n").unwrap();
        }
    }
}

impl Gene {
    /// Only what selection reads: the slot, the VRF output hash and the
    /// state hash. The rest is the fork point's (`Gene::from_json`).
    fn to_json(&self) -> Value {
        serde_json::json!([
            self.inputs.slot,
            hex::encode(self.inputs.vrf_hash.into_bigint().to_bytes_le()),
            self.hash,
        ])
    }

    fn from_json(world: &World, fork: &Gen, value: &Value) -> Self {
        let vrf_hash =
            Fp::from_le_bytes_mod_order(&hex::decode(value[1].as_str().unwrap()).unwrap());
        Self {
            inputs: BlockInputs {
                slot: value[0].as_u64().unwrap() as u32,
                vrf_hash,
                last_vrf_output: truncate(vrf_hash),
                snarked_ledger_hash: fork.snarked_ledger_hash.clone(),
                genesis_ledger_hash: world.genesis_ledger_hash.clone(),
                total_currency: fork.state.total_currency.clone(),
                winner: world.template.block_stake_winner.clone(),
                creator: world.template.block_creator.clone(),
                receiver: world.template.coinbase_receiver.clone(),
                supercharge_coinbase: false,
            },
            hash: serde_json::from_value(value[2].clone()).unwrap(),
            snarked_ledger_hash: fork.snarked_ledger_hash.clone(),
        }
    }
}

impl Witness {
    /// A self-contained fixture: the fork point's state and, for each block
    /// on either side, its slot, VRF output hash and state hash.
    fn to_json(&self, world: &mut World, class: &str) -> Value {
        let (existing, candidate) = self.tips(world);
        serde_json::json!({
            "about": "P4.11: a fork pair reachable by the consensus transition on which Rust's selection differed from OCaml 4.0.2's; crates/core/tests/p411a_selection_oracle.rs",
            "class": class,
            "ocaml_take": oracle_select(&existing.state, &candidate.state, &existing.hash, &candidate.hash),
            "fork": {
                "consensus_state": self.fork.state,
                "state_hash": self.fork.hash,
                "snarked_ledger_hash": self.fork.snarked_ledger_hash,
            },
            "existing": self.existing.iter().map(Gene::to_json).collect::<Vec<_>>(),
            "candidate": self.candidate.iter().map(Gene::to_json).collect::<Vec<_>>(),
        })
    }

    fn from_json(world: &World, value: &Value) -> (Self, String, bool) {
        let fork = &value["fork"];
        let fork = Gen {
            state: serde_json::from_value(fork["consensus_state"].clone()).unwrap(),
            hash: serde_json::from_value(fork["state_hash"].clone()).unwrap(),
            snarked_ledger_hash: serde_json::from_value(fork["snarked_ledger_hash"].clone())
                .unwrap(),
        };
        let genes = |key: &str| {
            value[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|gene| Gene::from_json(world, &fork, gene))
                .collect()
        };
        (
            Self {
                existing: genes("existing"),
                candidate: genes("candidate"),
                fork: fork.clone(),
            },
            value["class"].as_str().unwrap().to_owned(),
            value["ocaml_take"].as_bool().unwrap(),
        )
    }

    /// The same blocks with what selection doesn't read set as
    /// `Gene::from_json` sets it.
    fn normalized(&self, world: &World) -> Self {
        let normalize = |genes: &[Gene]| {
            genes
                .iter()
                .map(|gene| Gene::from_json(world, &self.fork, &gene.to_json()))
                .collect()
        };
        Self {
            fork: self.fork.clone(),
            existing: normalize(&self.existing),
            candidate: normalize(&self.candidate),
        }
    }
}

/// The smallest reachable witnesses `reachable_minimal_witnesses` found,
/// saved in tests/files/forks: rebuilt from their fork points with every
/// transition checked, Rust's against the oracle's.
#[test]
fn reachable_witness_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/files/forks");
    let mut world = World::new();
    let mut classes = vec![];
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("p411-reachable-") {
            continue;
        }
        let json: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let (witness, class, ocaml_take) = Witness::from_json(&world, &json);
        let mut tip = |genes: &[Gene]| {
            genes.iter().fold(witness.fork.clone(), |parent, gene| {
                world.apply(&parent, gene, true)
            })
        };
        let existing = tip(&witness.existing);
        let candidate = tip(&witness.candidate);
        assert_eq!(classify(&existing.state, &candidate.state), class, "{name}");
        let (state, hash) = (
            (&existing.state, &candidate.state),
            (&existing.hash, &candidate.hash),
        );
        assert_eq!(
            oracle_select(state.0, state.1, hash.0, hash.1),
            ocaml_take,
            "{name}"
        );
        // Before P4.11 Rust's selection took the other one.
        assert_eq!(
            consensus_take(state.0, state.1, hash.0, hash.1),
            ocaml_take,
            "{name}"
        );
        classes.push(class);
    }
    classes.sort();
    assert_eq!(
        classes,
        [
            "long range, equal slot",
            "long range, projected ring",
            "long range, same sub-window"
        ]
    );
}
