use std::collections::{BTreeMap, VecDeque};

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use super::super::*;

pub const INITIAL_RECV_BUFFER_CAPACITY: usize = 0x40000; // 256kb
pub const INITIAL_WINDOW_SIZE: u32 = INITIAL_RECV_BUFFER_CAPACITY as u32;
pub const MAX_WINDOW_SIZE: u32 = 16 * 1024 * 1024; // 16mb
/// The most receive window one connection grants beyond its streams' initial
/// windows: a stream's window grows past [`INITIAL_WINDOW_SIZE`] only while
/// the connection's windows add up to less, so a peer can never have more
/// than this plus one initial window per stream in flight, far below the
/// 256 MiB frame limit, however many streams it opens and grows.
pub const MAX_CONNECTION_RECEIVE_WINDOW: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pNetworkYamuxState {
    pub message_size_limit: Limit<usize>,
    pub pending_outgoing_limit: Limit<usize>,
    pub buffer: Vec<u8>,
    pub incoming: VecDeque<YamuxFrame>,
    pub streams: BTreeMap<StreamId, YamuxStreamState>,
    pub terminated: Option<Result<Result<(), YamuxSessionError>, YamuxFrameParseError>>,
    pub init: bool,
}

impl P2pNetworkYamuxState {
    /// Calculates and returns the next available stream ID for outgoing
    /// communication.
    pub fn next_stream_id(&self, kind: YamuxStreamKind, incoming: bool) -> Option<StreamId> {
        if self.init && self.terminated.is_none() {
            Some(kind.stream_id(incoming))
        } else {
            None
        }
    }

    pub fn consume(&mut self, len: usize) {
        // does not need to do anything;
        // we will update the stream window later when we process the `IncomingData' action
        let _ = len;
    }

    /// How much to read from the connection at once: what the peer may send,
    /// at most [`MAX_CONNECTION_RECEIVE_WINDOW`] of data. Reading less than
    /// the peer may send just takes another read.
    pub fn limit(&self) -> usize {
        const SIZE_OF_HEADER: usize = 12;
        let headers = self.streams.len() * 2 + 1;

        windows_ours(&self.streams).min(MAX_CONNECTION_RECEIVE_WINDOW) + headers * SIZE_OF_HEADER
    }

    /// The part of a `wanted` window update for `stream_id` that keeps the
    /// connection's receive windows within [`MAX_CONNECTION_RECEIVE_WINDOW`];
    /// a stream may always get back up to its initial window, so none stalls.
    pub fn receive_window_grant(&self, stream_id: StreamId, wanted: u32) -> u32 {
        let Some(stream) = self.streams.get(&stream_id) else {
            return wanted;
        };
        let room = MAX_CONNECTION_RECEIVE_WINDOW.saturating_sub(windows_ours(&self.streams));
        let to_initial = INITIAL_WINDOW_SIZE.saturating_sub(stream.window_ours) as usize;
        wanted.min(u32::try_from(room.max(to_initial)).unwrap_or(u32::MAX))
    }

    /// The most a data frame on `stream_id` may carry: the receive window we
    /// gave the peer, the initial one for a stream the frame opens, and the
    /// largest we ever give for one we don't know (it may have just closed).
    /// The reducer checks the exact window as it takes each frame.
    fn receive_window_bound(&self, stream_id: StreamId, flags: YamuxFlags) -> u32 {
        if flags.contains(YamuxFlags::SYN) {
            return INITIAL_WINDOW_SIZE;
        }
        self.streams
            .get(&stream_id)
            .map_or(MAX_WINDOW_SIZE, |stream| stream.window_ours)
    }

    /// The stream whose receive window the peer overran, if it did.
    pub fn window_exceeded(&self) -> Option<StreamId> {
        match self.terminated {
            Some(Err(YamuxFrameParseError::WindowExceeded(stream_id))) => Some(stream_id),
            _ => None,
        }
    }

    pub fn set_err(&mut self, err: YamuxFrameParseError) {
        self.terminated = Some(Err(err));
    }

    pub fn set_res(&mut self, res: Result<(), YamuxSessionError>) {
        self.terminated = Some(Ok(res));
    }

    /// Attempts to parse a Yamux frame from the buffer starting at the given offset.
    /// Returns the number of bytes consumed if a frame was successfully parsed.
    pub fn try_parse_frame(&mut self, offset: usize) -> Option<usize> {
        let buf = &self.buffer[offset..];
        if buf.len() < 12 {
            return None;
        }

        let _version = match buf[0] {
            0 => 0,
            unknown => {
                self.set_err(YamuxFrameParseError::Version(unknown));
                return None;
            }
        };

        let flags = u16::from_be_bytes(buf[2..4].try_into().expect("cannot fail"));
        let Some(flags) = YamuxFlags::from_bits(flags) else {
            self.set_err(YamuxFrameParseError::Flags(flags));
            return None;
        };
        let stream_id = u32::from_be_bytes(buf[4..8].try_into().expect("cannot fail"));
        let b = buf[8..12].try_into().expect("cannot fail");

        match buf[1] {
            // Data frame - contains actual payload data for the stream
            0 => {
                let len = u32::from_be_bytes(b) as usize;
                if len > self.message_size_limit {
                    self.set_res(Err(YamuxSessionError::Internal));
                    return None;
                }
                // Refuse a frame that can't fit the window as soon as its
                // header is in, rather than buffering it.
                if len > self.receive_window_bound(stream_id, flags) as usize {
                    self.set_err(YamuxFrameParseError::WindowExceeded(stream_id));
                    return None;
                }
                if buf.len() >= 12 + len {
                    let frame = YamuxFrame {
                        flags,
                        stream_id,
                        inner: YamuxFrameInner::Data(buf[12..(12 + len)].to_vec().into()),
                    };
                    self.incoming.push_back(frame);
                    Some(12 + len)
                } else {
                    None
                }
            }
            // Window Update frame - used for flow control, updates available window size
            1 => {
                let difference = u32::from_be_bytes(b);
                let frame = YamuxFrame {
                    flags,
                    stream_id,
                    inner: YamuxFrameInner::WindowUpdate { difference },
                };
                self.incoming.push_back(frame);
                Some(12)
            }
            // Ping frame - used for keepalive and round-trip time measurements
            2 => {
                let opaque = u32::from_be_bytes(b);
                let frame = YamuxFrame {
                    flags,
                    stream_id,
                    inner: YamuxFrameInner::Ping { opaque },
                };
                self.incoming.push_back(frame);
                Some(12)
            }
            // GoAway frame - signals session termination with optional error code
            3 => {
                let code = u32::from_be_bytes(b);
                let result = match code {
                    0 => Ok(()),                           // Normal termination
                    1 => Err(YamuxSessionError::Protocol), // Protocol error
                    2 => Err(YamuxSessionError::Internal), // Internal error
                    unknown => {
                        self.set_err(YamuxFrameParseError::ErrorCode(unknown));
                        return None;
                    }
                };
                let frame = YamuxFrame {
                    flags,
                    stream_id,
                    inner: YamuxFrameInner::GoAway(result),
                };
                self.incoming.push_back(frame);
                Some(12)
            }
            // Unknown frame type
            unknown => {
                self.set_err(YamuxFrameParseError::Type(unknown));
                None
            }
        }
    }

    /// Attempts to parse all available complete frames from the buffer,
    /// then shifts and compacts the buffer as needed.
    pub fn parse_frames(&mut self) {
        let mut offset = 0;
        while let Some(consumed) = self.try_parse_frame(offset) {
            offset += consumed;
        }
        self.shift_and_compact_buffer(offset);
    }

    fn shift_and_compact_buffer(&mut self, offset: usize) {
        let new_len = self.buffer.len() - offset;
        if self.buffer.capacity() > INITIAL_RECV_BUFFER_CAPACITY * 2
            && new_len < INITIAL_RECV_BUFFER_CAPACITY / 2
        {
            let old_buffer = &self.buffer;
            let mut new_buffer = Vec::with_capacity(INITIAL_RECV_BUFFER_CAPACITY);
            new_buffer.extend_from_slice(&old_buffer[offset..]);
            self.buffer = new_buffer;
        } else {
            self.buffer.copy_within(offset.., 0);
            self.buffer.truncate(new_len);
        }
    }

    /// Extends the internal buffer with new data, ensuring it has appropriate capacity.
    /// On first use, reserves the initial capacity.
    pub fn extend_buffer(&mut self, data: &[u8]) {
        if self.buffer.capacity() == 0 {
            self.buffer.reserve(INITIAL_RECV_BUFFER_CAPACITY);
        }
        self.buffer.extend_from_slice(data);
    }

    /// Returns the number of incoming frames that have been parsed and are ready for processing.
    pub fn incoming_frame_count(&self) -> usize {
        self.incoming.len()
    }
}

fn windows_ours(streams: &BTreeMap<StreamId, YamuxStreamState>) -> usize {
    streams.values().map(|s| s.window_ours as usize).sum()
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct YamuxStreamState {
    pub incoming: bool,
    pub syn_sent: bool,
    pub established: bool,
    pub readable: bool,
    pub writable: bool,
    pub window_theirs: u32,
    pub window_ours: u32,
    pub max_window_size: u32,
    pub pending: VecDeque<YamuxFrame>,
    #[serde(default)]
    #[ignore_malloc_size_of = "primitive"]
    pub pending_since: Option<redux::Timestamp>,
    #[serde(default)]
    #[ignore_malloc_size_of = "primitive"]
    pub pending_first_at: Option<redux::Timestamp>,
}

impl Default for YamuxStreamState {
    fn default() -> Self {
        YamuxStreamState {
            incoming: false,
            syn_sent: false,
            established: false,
            readable: false,
            writable: false,
            window_theirs: INITIAL_WINDOW_SIZE,
            window_ours: INITIAL_WINDOW_SIZE,
            max_window_size: INITIAL_WINDOW_SIZE,
            pending: VecDeque::default(),
            pending_since: None,
            pending_first_at: None,
        }
    }
}

impl YamuxStreamState {
    pub fn incoming() -> Self {
        YamuxStreamState {
            incoming: true,
            ..Default::default()
        }
    }

    /// Takes a data frame to send on this stream and returns what the peer's
    /// window has room for now. The rest waits in `pending` behind any bytes
    /// already waiting there, so that the stream's bytes keep their order. A
    /// reset goes at once, and nothing waiting is sent after it.
    pub fn take_sendable(&mut self, mut frame: YamuxFrame) -> Option<YamuxFrame> {
        if frame.flags.contains(YamuxFlags::RST) {
            self.pending.clear();
            return Some(frame);
        }
        if !self.pending.is_empty() || (self.window_theirs == 0 && frame.len() > 0) {
            self.pending.push_back(frame);
            return None;
        }
        match self.window_theirs.checked_sub(frame.len_as_u32()) {
            Some(window) => self.window_theirs = window,
            None => {
                if let Some(rest) = frame.split_at(self.window_theirs as usize) {
                    self.pending.push_back(rest);
                }
                self.window_theirs = 0;
            }
        }
        Some(frame)
    }

    /// The frames waiting for the peer's window, oldest first, to offer to
    /// `take_sendable` again once the window grows.
    pub fn take_pending(&mut self) -> VecDeque<YamuxFrame> {
        std::mem::take(&mut self.pending)
    }

    /// Bytes waiting for the peer's window.
    pub fn pending_len(&self) -> usize {
        self.pending.iter().map(YamuxFrame::len).sum()
    }
}

bitflags::bitflags! {
    #[derive(Serialize, Deserialize, Debug, Default, Clone, Copy)]
    pub struct YamuxFlags: u16 {
        const SYN = 0b0001;
        const ACK = 0b0010;
        const FIN = 0b0100;
        const RST = 0b1000;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct YamuxPing {
    pub stream_id: StreamId,
    pub opaque: u32,
    pub response: bool,
}

impl YamuxPing {
    pub fn into_frame(self) -> YamuxFrame {
        let YamuxPing {
            stream_id,
            opaque,
            response,
        } = self;
        YamuxFrame {
            flags: if response {
                YamuxFlags::ACK
            } else if stream_id == 0 {
                YamuxFlags::SYN
            } else {
                YamuxFlags::empty()
            },
            stream_id,
            inner: YamuxFrameInner::Ping { opaque },
        }
    }
}

pub type StreamId = u32;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum YamuxFrameParseError {
    /// Unknown version.
    Version(u8),
    /// Unknown flags.
    Flags(u16),
    /// Unknown type.
    Type(u8),
    /// Unknown error code.
    ErrorCode(u32),
    /// A data frame bigger than the stream's receive window.
    WindowExceeded(StreamId),
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct YamuxFrame {
    #[ignore_malloc_size_of = "doesn't allocate"]
    pub flags: YamuxFlags,
    pub stream_id: StreamId,
    pub inner: YamuxFrameInner,
}

impl YamuxFrame {
    pub fn into_bytes(self) -> Vec<u8> {
        let data_len = if let YamuxFrameInner::Data(data) = &self.inner {
            data.len()
        } else {
            0
        };
        let mut vec = Vec::with_capacity(12 + data_len);
        vec.push(0);
        match self.inner {
            YamuxFrameInner::Data(data) => {
                vec.push(0);
                vec.extend_from_slice(&self.flags.bits().to_be_bytes());
                vec.extend_from_slice(&self.stream_id.to_be_bytes());
                vec.extend_from_slice(&(data.len() as u32).to_be_bytes());
                vec.extend_from_slice(&data);
            }
            YamuxFrameInner::WindowUpdate { difference } => {
                vec.push(1);
                vec.extend_from_slice(&self.flags.bits().to_be_bytes());
                vec.extend_from_slice(&self.stream_id.to_be_bytes());
                vec.extend_from_slice(&difference.to_be_bytes());
            }
            YamuxFrameInner::Ping { opaque } => {
                vec.push(2);
                vec.extend_from_slice(&self.flags.bits().to_be_bytes());
                vec.extend_from_slice(&self.stream_id.to_be_bytes());
                vec.extend_from_slice(&opaque.to_be_bytes());
            }
            YamuxFrameInner::GoAway(res) => {
                vec.push(3);
                vec.extend_from_slice(&self.flags.bits().to_be_bytes());
                vec.extend_from_slice(&self.stream_id.to_be_bytes());
                let code = match res {
                    Ok(()) => 0u32,
                    Err(YamuxSessionError::Protocol) => 1,
                    Err(YamuxSessionError::Internal) => 2,
                };
                vec.extend_from_slice(&code.to_be_bytes());
            }
        }

        vec
    }

    pub fn len(&self) -> usize {
        if let YamuxFrameInner::Data(data) = &self.inner {
            data.len()
        } else {
            0
        }
    }

    // When we parse the frame we parse length as u32 and so `data.len()` should always be representable as u32
    pub fn len_as_u32(&self) -> u32 {
        if let YamuxFrameInner::Data(data) = &self.inner {
            u32::try_from(data.len()).unwrap_or(u32::MAX)
        } else {
            0
        }
    }

    /// If this data is bigger then `pos`, keep only first `pos` bytes and return some remaining
    /// otherwise return none
    pub fn split_at(&mut self, pos: usize) -> Option<Self> {
        use std::ops::Sub;

        if let YamuxFrameInner::Data(data) = &mut self.inner {
            if data.len() <= pos {
                return None;
            }
            let (keep, rest) = data.split_at(pos);
            let rest = Data(rest.to_vec().into_boxed_slice());
            *data = Data(keep.to_vec().into_boxed_slice());

            let fin = if self.flags.contains(YamuxFlags::FIN) {
                self.flags.remove(YamuxFlags::FIN);
                YamuxFlags::FIN
            } else {
                YamuxFlags::empty()
            };
            Some(YamuxFrame {
                flags: self.flags.sub(YamuxFlags::SYN | YamuxFlags::ACK) | fin,
                stream_id: self.stream_id,
                inner: YamuxFrameInner::Data(rest),
            })
        } else {
            None
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub enum YamuxFrameInner {
    Data(Data),
    WindowUpdate { difference: u32 },
    Ping { opaque: u32 },
    GoAway(#[ignore_malloc_size_of = "doesn't allocate"] Result<(), YamuxSessionError>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum YamuxSessionError {
    Protocol,
    Internal,
}

#[derive(Debug)]
pub enum YamuxStreamKind {
    Rpc,
    Gossipsub,
    Kademlia,
    Identify,
}

impl YamuxStreamKind {
    pub fn stream_id(self, incoming: bool) -> StreamId {
        (self as StreamId) * 2 + 1 + (incoming as StreamId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// However large its streams' windows, a connection reads at most
    /// `MAX_CONNECTION_RECEIVE_WINDOW` of data at once, and grants no window
    /// growth once its windows add up to that; a stream below its initial
    /// window still gets back up to it.
    #[test]
    fn connection_receive_allowance_is_bounded() {
        let mut yamux = P2pNetworkYamuxState::default();
        for id in 0..128u32 {
            let stream = YamuxStreamState {
                window_ours: MAX_WINDOW_SIZE,
                max_window_size: MAX_WINDOW_SIZE,
                ..YamuxStreamState::incoming()
            };
            yamux.streams.insert(id * 2 + 2, stream);
        }
        assert!(yamux.limit() <= MAX_CONNECTION_RECEIVE_WINDOW + (128 * 2 + 1) * 12);
        assert!(yamux.limit() < 256 * 1024 * 1024);
        assert_eq!(yamux.receive_window_grant(2, 1024 * 1024), 0);
        yamux.streams.get_mut(&2).unwrap().window_ours = 1000;
        assert_eq!(
            yamux.receive_window_grant(2, 1024 * 1024),
            INITIAL_WINDOW_SIZE - 1000
        );
        // An unknown stream is not ours to limit here.
        assert_eq!(yamux.receive_window_grant(1, 7), 7);
    }

    #[test]
    fn yamux_stream_id() {
        use super::YamuxStreamKind::*;
        assert_eq!(Rpc.stream_id(false), 1);
        assert_eq!(Rpc.stream_id(true), 2);
        assert_eq!(Kademlia.stream_id(false), 5);
        assert_eq!(Kademlia.stream_id(true), 6);
    }

    use super::{YamuxFlags, YamuxFrame, YamuxFrameInner, YamuxStreamState};

    fn frame(flags: YamuxFlags, bytes: &[u8]) -> YamuxFrame {
        YamuxFrame {
            flags,
            stream_id: 1,
            inner: YamuxFrameInner::Data(bytes.to_vec().into()),
        }
    }

    fn bytes(frame: &YamuxFrame) -> Vec<u8> {
        match &frame.inner {
            YamuxFrameInner::Data(data) => data.0.to_vec(),
            _ => panic!("not a data frame"),
        }
    }

    /// Offers every waiting frame again, as a window update does, and returns
    /// the bytes sent.
    fn window_update(stream: &mut YamuxStreamState, difference: u32) -> Vec<u8> {
        stream.window_theirs += difference;
        let mut sent = vec![];
        for frame in stream.take_pending() {
            if let Some(frame) = stream.take_sendable(frame) {
                sent.extend(bytes(&frame));
            }
        }
        sent
    }

    #[test]
    fn bytes_that_wait_for_the_window_keep_their_order() {
        let mut stream = YamuxStreamState {
            window_theirs: 4,
            ..Default::default()
        };
        let mut sent = vec![];

        // "abcdef" fills the window: "abcd" goes and "ef" waits.
        let first = stream.take_sendable(frame(YamuxFlags::empty(), b"abcdef"));
        sent.extend(bytes(&first.unwrap()));
        // "gh" waits behind "ef", though it came later. It went ahead of it
        // before: `push_front` on a full window.
        assert!(stream
            .take_sendable(frame(YamuxFlags::empty(), b"gh"))
            .is_none());
        assert_eq!(stream.pending_len(), 4);
        // A FIN waits behind the data too.
        assert!(stream.take_sendable(frame(YamuxFlags::FIN, b"")).is_none());

        // A window of 3 lets "ef" and "g" go; "h" and the FIN wait.
        sent.extend(window_update(&mut stream, 3));
        assert_eq!(sent, b"abcdefg");
        assert_eq!(stream.pending_len(), 1);

        sent.extend(window_update(&mut stream, 10));
        assert_eq!(sent, b"abcdefgh");
        assert!(stream.pending.is_empty());
        assert_eq!(stream.window_theirs, 9);
    }

    #[test]
    fn a_reset_goes_at_once_and_drops_what_waits() {
        let mut stream = YamuxStreamState {
            window_theirs: 2,
            ..Default::default()
        };
        assert!(stream
            .take_sendable(frame(YamuxFlags::empty(), b"abcd"))
            .is_some());
        assert_eq!(stream.pending_len(), 2);

        let reset = stream.take_sendable(frame(YamuxFlags::RST, b""));
        assert!(reset.is_some_and(|f| f.flags.contains(YamuxFlags::RST)));
        assert!(stream.pending.is_empty());
    }
}

mod measurement {
    use std::mem;

    use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};

    use super::{P2pNetworkYamuxState, YamuxFrame};

    impl MallocSizeOf for P2pNetworkYamuxState {
        fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
            self.buffer.capacity()
                + self.incoming.capacity() * mem::size_of::<YamuxFrame>()
                + self
                    .incoming
                    .iter()
                    .map(|frame| frame.size_of(ops))
                    .sum::<usize>()
                + self
                    .streams
                    .iter()
                    .map(|(k, v)| mem::size_of_val(k) + mem::size_of_val(v) + v.size_of(ops))
                    .sum::<usize>()
        }
    }
}
