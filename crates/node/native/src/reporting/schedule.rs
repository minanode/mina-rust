//! When reports are sent, and the clock and IDs they carry.
//!
//! Times are milliseconds since the Unix epoch: OCaml's `Block_time`, which
//! is wall-clock time without a test time offset.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How many slots apart uptime iterations and node status reports are.
pub const ITERATION_SLOTS: u64 = 5;
/// A produced block is sent if its slot starts within this many slots of
/// the desired iteration start, leaving a full slot to produce it.
pub const PRODUCED_BLOCK_LOOKAHEAD_SLOTS: u64 = 4;
/// How long to wait for the produced block after its slot starts.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L164-L182>
pub const PRODUCED_BLOCK_WAIT: Duration = Duration::from_secs(180);

pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

/// A clock that tells a fixed time, for tests.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

pub trait IdSource: Send + Sync + 'static {
    /// A fresh report ID, as OCaml's `Uuid_unix.create () |> Uuid.to_string`.
    fn new_id(&self) -> String;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RandomUuid;

impl IdSource for RandomUuid {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

/// The first uptime iteration: the next slot boundary in absolute block
/// time, or now when exactly on one. Not the next global slot divisible by
/// five.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L529-L548>
pub fn first_iteration_start(now_ms: u64, slot_ms: u64) -> u64 {
    let slot_ms = slot_ms.max(1);
    match now_ms.checked_rem(slot_ms) {
        Some(0) | None => now_ms,
        Some(_) => now_ms
            .checked_div(slot_ms)
            .unwrap_or(0)
            .saturating_add(1)
            .saturating_mul(slot_ms),
    }
}

/// Each iteration's desired start is five slots after the previous one's,
/// whenever that one actually started or finished.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L527>
pub fn next_iteration_start(desired_start_ms: u64, slot_ms: u64) -> u64 {
    desired_start_ms.saturating_add(slot_ms.saturating_mul(ITERATION_SLOTS))
}

/// Whether an iteration also waits for and sends our produced block: its
/// slot starts no later than four slots after the *desired* start, so a late
/// iteration doesn't move the bound.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L509-L526>
pub fn sends_produced_block(desired_start_ms: u64, production_ms: u64, slot_ms: u64) -> bool {
    production_ms
        <= desired_start_ms.saturating_add(slot_ms.saturating_mul(PRODUCED_BLOCK_LOOKAHEAD_SLOTS))
}

/// Node status is first sent five slots after the reporter starts, then
/// every five slots; not aligned to slot boundaries.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L423-L431>
pub fn status_interval(slot_ms: u64) -> Duration {
    Duration::from_millis(slot_ms.saturating_mul(ITERATION_SLOTS))
}

/// OCaml's `Rfc3339_time.get_rfc3339_time`: `Ptime.to_rfc3339
/// ~tz_offset_s:0`, whole seconds (truncated), `Z` suffix.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_stdlib_unix/rfc3339_time.ml#L3-L10>
pub fn rfc3339(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        ),
        Err(_) => "1970-01-01T00:00:00Z".to_owned(),
    }
}

/// Core's `Time.Span.to_string_hum` with its defaults (three decimals, the
/// largest unit not exceeding the span, trailing zeros dropped), which the
/// error report's `uptime_of_node` uses: `1s`, `2.5m`, `1.25h`, `3.042d`.
pub fn span_to_string_hum(span: Duration) -> String {
    let secs = span.as_secs_f64();
    let (value, unit) = if secs >= 86_400.0 {
        (secs / 86_400.0, "d")
    } else if secs >= 3_600.0 {
        (secs / 3_600.0, "h")
    } else if secs >= 60.0 {
        (secs / 60.0, "m")
    } else if secs >= 1.0 {
        (secs, "s")
    } else if secs >= 0.001 {
        (secs * 1_000.0, "ms")
    } else if secs >= 0.000_001 {
        (secs * 1_000_000.0, "us")
    } else if secs > 0.0 {
        (secs * 1_000_000_000.0, "ns")
    } else {
        return "0s".to_owned();
    };
    let mut text = format!("{value:.3}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    format!("{text}{unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_iteration_is_the_next_slot_boundary() {
        // Captured fixture chains used 2 s slots.
        assert_eq!(first_iteration_start(10_000, 2_000), 10_000);
        assert_eq!(first_iteration_start(10_001, 2_000), 12_000);
        assert_eq!(first_iteration_start(11_999, 2_000), 12_000);
        // Mainnet: 180 s slots, boundaries in absolute time, not every fifth
        // global slot.
        let slot = 180_000;
        let now = 1_761_091_016_123;
        let start = first_iteration_start(now, slot);
        assert_eq!(start % slot, 0);
        assert!(start > now && start - now < slot);
        assert_eq!(next_iteration_start(start, slot), start + 900_000);
    }

    #[test]
    fn produced_block_bound_uses_the_desired_start() {
        let slot = 180_000;
        let start = 1_000_000_000;
        assert!(sends_produced_block(start, start + 4 * slot, slot));
        assert!(!sends_produced_block(start, start + 4 * slot + 1, slot));
        assert!(sends_produced_block(start, start - slot, slot));
    }

    #[test]
    fn rfc3339_matches_the_fixture_spelling() {
        // tools/uptime-fixtures/fixtures/mainnet/uptime-data.json, and the
        // truncation of milliseconds.
        assert_eq!(rfc3339(1_792_627_016_000), "2026-10-21T23:56:56Z");
        assert_eq!(rfc3339(1_792_627_016_999), "2026-10-21T23:56:56Z");
        assert_eq!(rfc3339(1_761_091_016_500), "2025-10-21T23:56:56Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn spans_in_core_hum_format() {
        assert_eq!(span_to_string_hum(Duration::from_secs(1)), "1s");
        assert_eq!(span_to_string_hum(Duration::from_secs(90)), "1.5m");
        assert_eq!(span_to_string_hum(Duration::from_secs(4_500)), "1.25h");
        assert_eq!(span_to_string_hum(Duration::from_millis(1_500)), "1.5s");
        assert_eq!(span_to_string_hum(Duration::ZERO), "0s");
    }
}
