//! Refusal counts shared by streams and retained across reconnects.

use crate::PeerId;
use redux::Timestamp;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

const INTERVAL: Duration = Duration::from_secs(5);
const MAX_RECORDS: usize = 2048;

#[serde_with::serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RpcRefusalWarnings {
    #[serde_as(as = "Vec<(_, _)>")]
    records: BTreeMap<(PeerId, bool), Counts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Counts {
    since: Timestamp,
    total: u64,
    blocks: u64,
    pending: u64,
    pending_blocks: u64,
    second: u64,
    second_blocks: u64,
    peak_blocks_per_second: u64,
}

impl RpcRefusalWarnings {
    pub fn record(&mut self, peer: PeerId, serving: bool, block: bool, now: Timestamp) {
        let key = (peer, serving);
        if !self.records.contains_key(&key) && self.records.len() >= MAX_RECORDS {
            if let Some(oldest) = self
                .records
                .iter()
                .min_by_key(|(_, c)| c.since)
                .map(|(k, _)| *k)
            {
                if let Some(counts) = self.records.remove(&oldest) {
                    Self::log(oldest, &counts, now);
                }
            }
        }
        let second = u64::from(now) / 1_000_000_000;
        let counts = self.records.entry(key).or_insert(Counts {
            since: now,
            total: 0,
            blocks: 0,
            pending: 0,
            pending_blocks: 0,
            second,
            second_blocks: 0,
            peak_blocks_per_second: 0,
        });
        counts.total = counts.total.saturating_add(1);
        counts.pending = counts.pending.saturating_add(1);
        if block {
            counts.blocks = counts.blocks.saturating_add(1);
            counts.pending_blocks = counts.pending_blocks.saturating_add(1);
            if counts.second != second {
                counts.second = second;
                counts.second_blocks = 0;
            }
            counts.second_blocks = counts.second_blocks.saturating_add(1);
            counts.peak_blocks_per_second = counts.peak_blocks_per_second.max(counts.second_blocks);
        }
    }

    /// Flush after silence or disconnect too; a subsequent refusal is not needed.
    pub fn flush(&mut self, now: Timestamp) -> usize {
        let mut summaries = 0;
        for (key, counts) in &mut self.records {
            if now
                .checked_sub(counts.since)
                .is_some_and(|elapsed| elapsed >= INTERVAL)
            {
                summaries += usize::from(counts.pending != 0);
                Self::log(*key, counts, now);
                counts.pending = 0;
                counts.pending_blocks = 0;
                counts.since = now;
            }
        }
        summaries
    }

    fn log((peer, serving): (PeerId, bool), counts: &Counts, now: Timestamp) {
        if counts.pending == 0 {
            return;
        }
        mina_core::warn!(now;
            kind = if serving { "P2pServedRpcBudgetExceeded" } else { "P2pNetworkRpcErrorResponse" },
            summary = "RPC refusal summary",
            peer_id = display(peer), count = counts.pending,
            block_count = counts.pending_blocks, total = counts.total,
            block_total = counts.blocks, peak_blocks_per_second = counts.peak_blocks_per_second,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_summaries_retain_counts_and_flush_after_silence() {
        let peer = PeerId::from_bytes([42; 32]);
        let mut warnings = RpcRefusalWarnings::default();
        for serving in [false, true] {
            for _ in 0..1000 {
                warnings.record(peer, serving, true, Timestamp::ZERO);
            }
        }
        assert_eq!(
            warnings.flush(Timestamp::ZERO + Duration::from_millis(4999)),
            0
        );
        assert!(warnings.records.values().all(|c| c.pending == 1000));
        assert_eq!(warnings.flush(Timestamp::ZERO + INTERVAL), 2);
        assert_eq!(warnings.flush(Timestamp::ZERO + INTERVAL), 0);
        assert!(warnings.records.values().all(|c| c.pending == 0
            && c.total == 1000
            && c.blocks == 1000
            && c.peak_blocks_per_second == 1000));
        warnings.record(peer, false, false, Timestamp::ZERO + INTERVAL);
        assert_eq!(warnings.flush(Timestamp::ZERO + INTERVAL * 2), 1);
        let c = &warnings.records[&(peer, false)];
        assert_eq!((c.total, c.blocks, c.pending), (1001, 1000, 0));
        assert!(warnings.records.len() <= MAX_RECORDS);
        let json = serde_json::to_value(&warnings).expect("inspection counters serialize");
        let decoded: RpcRefusalWarnings = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.records[&(peer, false)].total, 1001);
    }
}
