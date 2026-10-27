use std::collections::{BTreeMap, VecDeque};

use mina_p2p_messages::v2::StateHash;
use redux::Timestamp;
use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};

use crate::ActionKind;

use super::ActionKindWithMeta;

/// Per-block action timings are diagnostics. Keep the most recent blocks.
pub const MAX_BLOCK_ACTION_STATS: usize = 64;
// CompactRange uses a u16 for the flattened action-kind/duration-bucket key.
const _: () = assert!((ActionKind::COUNT as usize) * 9 <= u16::MAX as usize);

#[derive(Default, Clone)]
pub struct ActionStats {
    /// Stats since the start of the node, indexed by `ActionKind`.
    pub(super) since_start: ActionStatsSnapshot,
    pub(super) per_block: VecDeque<ActionStatsForBlock>,
}

impl ActionStats {
    /// Heap owned by the per-block timing history, including spare capacity.
    pub(super) fn per_block_heap_bytes(&self) -> usize {
        use std::mem::size_of;

        self.per_block
            .capacity()
            .saturating_mul(size_of::<ActionStatsForBlock>())
            .saturating_add(self.per_block.iter().fold(0usize, |sum, block| {
                sum.saturating_add(block.stats.heap_bytes())
            }))
    }

    pub fn new_best_tip(&mut self, time: Timestamp, level: u32, hash: StateHash) {
        while self.per_block.len() >= MAX_BLOCK_ACTION_STATS {
            self.per_block.pop_front();
        }
        let id = self
            .per_block
            .back()
            .map_or(0, |v| v.id.checked_add(1).expect("overflow"));
        self.per_block.push_back(ActionStatsForBlock {
            id,
            time,
            block_level: level,
            block_hash: hash,
            cpu_idle: 0,
            cpu_busy: 0,
            stats: Default::default(),
        });
    }

    pub fn add(&mut self, action: &ActionKindWithMeta, prev_action: &ActionKindWithMeta) {
        self.since_start.add(action, prev_action);
        if let Some(stats) = self.per_block.back_mut() {
            stats.new_action(action, prev_action);
        }
    }

    pub fn collect_stats_for_block_with_id(&self, id: Option<u64>) -> Option<ActionStatsForBlock> {
        let blocks = &self.per_block;
        let last = blocks.back()?;
        let id = match id {
            Some(id) => {
                if id == last.id {
                    return Some(last.clone());
                }
                id
            }
            None => return Some(last.clone()),
        };
        let i = id
            .checked_add(blocks.len() as u64)?
            .checked_sub(last.id.checked_add(1)?)?;
        blocks.get(usize::try_from(i).ok()?).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_timings_preserve_dense_http_fields_and_bucket_edges() {
        use redux::ActionMeta;
        let mut snapshot = ActionStatsSnapshot::default();
        let kind = ActionKind::EventSourceWaitForEvents;
        let prev = ActionMeta::ZERO.with_action(kind);
        // Both sides of every inclusive duration boundary, plus zero.
        let durations = [
            0, 1_000, 1_001, 10_000, 10_001, 50_000, 50_001, 100_000, 100_001, 500_000, 500_001,
            1_000_000, 1_000_001, 5_000_000, 5_000_001, 50_000_000, 50_000_001,
        ];
        for duration in durations {
            let meta = ActionMeta::zero_custom(Timestamp::new(duration));
            snapshot.add(&meta.with_action(kind), &prev);
        }
        let json = serde_json::to_value(&snapshot).unwrap();
        let ranges = &json[format!("{kind:?}")];
        for (name, calls, total, max) in [
            ("under_1_us", 2, 1_000, 1_000),
            ("under_10_us", 2, 11_001, 10_000),
            ("under_50_us", 2, 60_001, 50_000),
            ("under_100_us", 2, 150_001, 100_000),
            ("under_500_us", 2, 600_001, 500_000),
            ("under_1_ms", 2, 1_500_001, 1_000_000),
            ("under_5_ms", 2, 6_000_001, 5_000_000),
            ("under_50_ms", 2, 55_000_001, 50_000_000),
            ("above_50_ms", 1, 50_000_001, 50_000_001),
        ] {
            assert_eq!(
                ranges[name],
                serde_json::json!({
                    "total_calls": calls, "total_duration": total, "max_duration": max,
                })
            );
        }
        assert_eq!(json.as_object().unwrap().len(), kind as usize);
        assert_eq!(snapshot.ranges.len(), 9);
        assert!(
            snapshot.heap_bytes()
                < (kind as usize + 1) * std::mem::size_of::<ActionStatsForRanges>()
        );
        // Deserializing used to extend the dense map to COUNT. Keep that too.
        let restored: ActionStatsSnapshot = serde_json::from_value(json.clone()).unwrap();
        let restored_json = serde_json::to_value(&restored).unwrap();
        assert_eq!(
            restored_json.as_object().unwrap().len(),
            ActionKind::COUNT as usize - 1
        );
        for (key, value) in json.as_object().unwrap() {
            assert_eq!(&restored_json[key], value);
        }
        assert_eq!(restored.ranges.len(), 9);
        let none = ActionMeta::ZERO.with_action(ActionKind::None);
        let before = serde_json::to_value(&snapshot).unwrap();
        snapshot.add(&prev, &none);
        assert_eq!(serde_json::to_value(snapshot).unwrap(), before);
    }

    #[test]
    fn compact_timings_saturate_and_keep_high_action_kinds_sparse() {
        use redux::ActionMeta;
        let kind = ActionKind::try_from(ActionKind::COUNT - 1).unwrap();
        let mut stats = ActionStats::default();
        stats.new_best_tip(Timestamp::ZERO, 0, StateHash::zero());
        let prev = ActionMeta::ZERO.with_action(kind);
        let action = ActionMeta::zero_custom(Timestamp::new(u64::MAX)).with_action(kind);
        stats.add(&action, &prev);
        stats.add(&action, &prev);
        let snapshot = &stats.per_block.back().unwrap().stats;
        assert_eq!(snapshot.ranges.len(), 1);
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(
            json[format!("{kind:?}")]["above_50_ms"]["total_duration"],
            u64::MAX
        );
        assert_eq!(json[format!("{kind:?}")]["above_50_ms"]["total_calls"], 2);
        assert!(stats.per_block_heap_bytes() < 4096);
    }

    #[test]
    fn block_stats_evict_oldest_and_keep_ids_contiguous() {
        let mut stats = ActionStats::default();
        let hash = StateHash::zero();
        for level in 0..MAX_BLOCK_ACTION_STATS as u32 {
            stats.new_best_tip(Timestamp::ZERO, level, hash.clone());
        }
        // On wasm32 this index used to wrap to zero and return the first block.
        assert!(stats
            .collect_stats_for_block_with_id(Some(u32::MAX as u64 + 1))
            .is_none());
        assert!(stats
            .collect_stats_for_block_with_id(Some(u64::MAX))
            .is_none());
        let capped_heap = stats.per_block_heap_bytes();
        for level in MAX_BLOCK_ACTION_STATS as u32..MAX_BLOCK_ACTION_STATS as u32 + 5 {
            stats.new_best_tip(Timestamp::ZERO, level, hash.clone());
        }
        assert_eq!(stats.per_block.len(), MAX_BLOCK_ACTION_STATS);
        assert_eq!(stats.per_block_heap_bytes(), capped_heap);
        assert_eq!(stats.per_block.front().unwrap().id, 5);
        assert_eq!(
            stats.per_block.back().unwrap().id,
            MAX_BLOCK_ACTION_STATS as u64 + 4
        );
        assert!(stats.collect_stats_for_block_with_id(Some(4)).is_none());
        assert_eq!(
            stats.collect_stats_for_block_with_id(Some(5)).unwrap().id,
            5
        );
        assert_eq!(
            stats.collect_stats_for_block_with_id(None).unwrap().id,
            MAX_BLOCK_ACTION_STATS as u64 + 4
        );
    }
}

#[derive(Debug, Default, Clone)]
// The custom serializer exposes a dense map, independent of the storage below.
pub struct ActionStatsSnapshot {
    // Preserve the dense HTTP map's extent, including zero-only action kinds.
    len: usize,
    // Only buckets that have been observed allocate storage. Sorted by
    // action-kind * 9 + duration bucket; counts and durations stay u64.
    ranges: Vec<CompactRange>,
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for ActionStatsSnapshot {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new().build().into()
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for ActionStatsSnapshot {}

#[derive(Debug, Clone)]
struct CompactRange {
    index: u16,
    stats: ActionStatsForRange,
}

impl ActionStatsSnapshot {
    pub fn add(&mut self, action: &ActionKindWithMeta, prev_action: &ActionKindWithMeta) {
        if *prev_action.action() == ActionKind::None {
            return;
        }

        let kind_i = *prev_action.action() as usize;
        let duration = action
            .meta()
            .time_as_nanos()
            .saturating_sub(prev_action.meta().time_as_nanos());

        self.len = self.len.max(kind_i + 1);
        let index = u16::try_from(kind_i * 9 + duration_bucket(duration)).expect("action range");
        let i = match self
            .ranges
            .binary_search_by_key(&index, |range| range.index)
        {
            Ok(i) => i,
            Err(i) => {
                self.ranges.insert(
                    i,
                    CompactRange {
                        index,
                        stats: Default::default(),
                    },
                );
                i
            }
        };
        self.ranges[i].stats.add(duration);
    }
}

impl ActionStatsSnapshot {
    pub(super) fn heap_bytes(&self) -> usize {
        self.ranges.capacity() * std::mem::size_of::<CompactRange>()
    }

    fn dense(&self, kind: usize) -> ActionStatsForRanges {
        let mut result = ActionStatsForRanges::default();
        let start = self
            .ranges
            .partition_point(|r| usize::from(r.index) < kind * 9);
        for range in self.ranges[start..]
            .iter()
            .take_while(|r| usize::from(r.index) < (kind + 1) * 9)
        {
            *result.bucket_mut(usize::from(range.index) % 9) = range.stats.clone();
        }
        result
    }
}

impl Serialize for ActionStatsSnapshot {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = s.serialize_map(Some(self.len.saturating_sub(1)))?;
        for i in 1..self.len {
            let kind = ActionKind::try_from(i as u16).unwrap();
            m.serialize_entry(&kind, &self.dense(i))?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for ActionStatsSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let m: BTreeMap<ActionKind, ActionStatsForRanges> = Deserialize::deserialize(deserializer)?;
        // The old deserializer expanded to COUNT, even for an empty map.
        let mut result = Self {
            len: ActionKind::COUNT as usize,
            ranges: Vec::new(),
        };
        for (kind, mut ranges) in m {
            for bucket in 0..9 {
                let stats = std::mem::take(ranges.bucket_mut(bucket));
                if stats.total_calls != 0 || stats.total_duration != 0 || stats.max_duration != 0 {
                    result.ranges.push(CompactRange {
                        index: kind as u16 * 9 + bucket as u16,
                        stats,
                    });
                }
            }
        }
        Ok(result)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionStatsForBlock {
    pub id: u64,
    pub time: Timestamp,
    pub block_level: u32,
    pub block_hash: StateHash,
    pub cpu_idle: u64,
    pub cpu_busy: u64,
    pub stats: ActionStatsSnapshot,
}

impl ActionStatsForBlock {
    fn new_action(&mut self, action: &ActionKindWithMeta, prev_action: &ActionKindWithMeta) {
        let duration = action
            .meta()
            .time_as_nanos()
            .saturating_sub(prev_action.meta().time_as_nanos());
        match prev_action.action() {
            ActionKind::None => {}
            ActionKind::EventSourceWaitForEvents => {
                self.cpu_idle = self.cpu_idle.saturating_add(duration)
            }
            _ => self.cpu_busy = self.cpu_busy.saturating_add(duration),
        }
        self.stats.add(action, prev_action);
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionStatsForRange {
    /// Total number of times this action kind was executed.
    pub total_calls: u64,
    /// Sum of durations from this action till the next one in nanoseconds.
    pub total_duration: u64,
    /// Max duration.
    pub max_duration: u64,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionStatsForRanges {
    pub under_1_us: ActionStatsForRange,
    pub under_10_us: ActionStatsForRange,
    pub under_50_us: ActionStatsForRange,
    pub under_100_us: ActionStatsForRange,
    pub under_500_us: ActionStatsForRange,
    pub under_1_ms: ActionStatsForRange,
    pub under_5_ms: ActionStatsForRange,
    pub under_50_ms: ActionStatsForRange,
    pub above_50_ms: ActionStatsForRange,
}

fn duration_bucket(duration: u64) -> usize {
    [
        1_000, 10_000, 50_000, 100_000, 500_000, 1_000_000, 5_000_000, 50_000_000,
    ]
    .partition_point(|limit| duration > *limit)
}

impl ActionStatsForRange {
    fn add(&mut self, duration: u64) {
        self.total_calls = self.total_calls.saturating_add(1);
        self.total_duration = self.total_duration.saturating_add(duration);
        self.max_duration = self.max_duration.max(duration);
    }
}

impl ActionStatsForRanges {
    fn bucket_mut(&mut self, bucket: usize) -> &mut ActionStatsForRange {
        match bucket {
            0 => &mut self.under_1_us,
            1 => &mut self.under_10_us,
            2 => &mut self.under_50_us,
            3 => &mut self.under_100_us,
            4 => &mut self.under_500_us,
            5 => &mut self.under_1_ms,
            6 => &mut self.under_5_ms,
            7 => &mut self.under_50_ms,
            8 => &mut self.above_50_ms,
            _ => unreachable!("duration bucket"),
        }
    }

    pub fn add(&mut self, duration: u64) {
        self.bucket_mut(duration_bucket(duration)).add(duration);
    }
}
