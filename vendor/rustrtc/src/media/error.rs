use thiserror::Error;

use crate::media::frame::MediaKind;

pub type MediaResult<T> = Result<T, MediaError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MediaError {
    #[error("track has ended")]
    EndOfStream,
    #[error("sender lagged and samples were dropped")]
    Lagged,
    #[error("track closed")]
    Closed,
    #[error("channel is full, would block")]
    WouldBlock,
    #[error("sample kind mismatch: expected {expected:?} got {actual:?}")]
    KindMismatch {
        expected: MediaKind,
        actual: MediaKind,
    },
}
