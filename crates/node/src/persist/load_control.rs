//! Cooperative load cancellation and completed-work counters (I5d6).
//!
//! Cancellation and handing the result to the ledger thread race through one
//! atomic gate. Once handoff wins, the node waits for that result instead of
//! abandoning ledgers which are already being installed.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use super::ledger_codec::CodecError;

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const HANDOFF: u8 = 2;

#[derive(Default)]
pub struct LoadControl {
    state: AtomicU8,
    progress: AtomicU64,
    #[cfg(test)]
    cancel_at_progress: Option<u64>,
    #[cfg(test)]
    observed_masks: std::sync::Mutex<Vec<ledger::Uuid>>,
}

impl LoadControl {
    /// `false` means the short ledger handoff already owns the result.
    pub fn cancel(&self) -> bool {
        self.state
            .compare_exchange(RUNNING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            || self.is_cancelled()
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    pub fn check(&self) -> Result<(), CodecError> {
        if self.is_cancelled() {
            Err(CodecError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Count real completed work, never a timer heartbeat or a scheduled job.
    pub fn completed(&self, count: u64) -> Result<(), CodecError> {
        self.check()?;
        let _previous = self.progress.fetch_add(count, Ordering::Relaxed);
        #[cfg(test)]
        if self
            .cancel_at_progress
            .is_some_and(|limit| _previous + count >= limit)
        {
            self.cancel();
            self.check()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn cancelling_after(limit: u64) -> Self {
        Self {
            cancel_at_progress: Some(limit),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_mask(&self, mask: &ledger::Mask) {
        use ledger::BaseLedger;
        self.observed_masks.lock().unwrap().push(mask.get_uuid());
    }

    #[cfg(test)]
    pub(crate) fn assert_observed_masks_dropped(&self) {
        let masks = self.observed_masks.lock().unwrap();
        assert!(!masks.is_empty());
        assert!(masks.iter().all(|id| !ledger::mask::is_alive(id)));
    }

    pub fn progress(&self) -> u64 {
        self.progress.load(Ordering::Relaxed)
    }

    pub fn begin_handoff(&self) -> bool {
        self.state
            .compare_exchange(RUNNING, HANDOFF, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_handoff_are_exclusive() {
        for _ in 0..100 {
            let control = LoadControl::default();
            std::thread::scope(|scope| {
                let cancel = scope.spawn(|| control.cancel());
                let handoff = scope.spawn(|| control.begin_handoff());
                assert_ne!(cancel.join().unwrap(), handoff.join().unwrap());
            });
        }
    }

    #[test]
    fn cancelled_work_cannot_publish_more_progress() {
        let control = LoadControl::default();
        control.completed(1).unwrap();
        assert!(control.cancel());
        assert!(matches!(control.completed(1), Err(CodecError::Cancelled)));
        assert_eq!(control.progress(), 1);
    }
}
