use binprot::BinProtWrite;
use mina_p2p_messages::{
    rpc_kernel::{Message, NeedsLength, Query, RpcMethod},
    v2,
};
use mio::{event::Event, net::TcpStream, Events, Interest, Poll, Registry, Token};
use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    time::{Duration, Instant},
};

const MAX_RECURSION_DEPTH: u8 = 25;

// messages
const HEADER_MSG: [u8; 7] = [2, 253, 82, 80, 67, 0, 1];
const OK_MSG: [u8; 5] = [2, 1, 0, 1, 0];
const HEARTBEAT_MSG: [u8; 1] = [0];

fn prepend_length(message: &[u8]) -> Vec<u8> {
    let length = message.len() as u64;
    let mut length_bytes = length.to_le_bytes().to_vec();
    length_bytes.append(&mut message.to_vec());
    length_bytes
}
pub enum HandleResult {
    MessageSent,
    ConnectionClosed,
    ConnectionAlive,
    MessageWouldBlock,
}

impl HandleResult {
    pub fn should_retry(&self) -> bool {
        matches!(self, Self::ConnectionClosed)
    }
}

pub fn send_diff(address: SocketAddr, data: v2::ArchiveRpc) -> io::Result<HandleResult> {
    let rpc = encode_to_rpc(data)?;
    process_rpc(address, &rpc, None)
}

/// The binprot encoding of a [`v2::ArchiveRpc`], as the archive outbox keeps
/// it.
pub fn encode_payload(data: &v2::ArchiveRpc) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    data.binprot_write(&mut v)?;
    Ok(v)
}

/// Sends a payload from [`encode_payload`] and waits for the archive's
/// answer, at most `timeout` for the whole exchange.
pub fn send_payload(
    address: SocketAddr,
    payload: &[u8],
    timeout: Duration,
) -> io::Result<HandleResult> {
    let rpc = encode_to_rpc(RawBinprot(payload))?;
    process_rpc(address, &rpc, Some(Instant::now() + timeout))
}

/// Bytes that are already binprot.
struct RawBinprot<'a>(&'a [u8]);

impl BinProtWrite for RawBinprot<'_> {
    fn binprot_write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(self.0)
    }
}

fn encode_to_rpc<T: BinProtWrite>(data: T) -> io::Result<Vec<u8>> {
    type Method = mina_p2p_messages::rpc::SendArchiveDiffUnversioned;
    let mut v = vec![0; 8];

    if let Err(e) = Message::Query(Query {
        tag: Method::NAME.into(),
        version: Method::VERSION,
        id: 1,
        data: NeedsLength(data),
    })
    .binprot_write(&mut v)
    {
        mina_node::core::warn!(
            summary = "Failed binprot serializastion",
            error = e.to_string()
        );
        return Err(e);
    }

    let payload_length = (v.len() - 8) as u64;
    v[..8].copy_from_slice(&payload_length.to_le_bytes());
    // Bake in the heartbeat message
    v.splice(0..0, prepend_length(&HEARTBEAT_MSG).iter().cloned());
    // also add the heartbeat message to the end of the message
    v.extend_from_slice(&prepend_length(&HEARTBEAT_MSG));

    Ok(v)
}

fn process_rpc(
    address: SocketAddr,
    data: &[u8],
    deadline: Option<Instant>,
) -> io::Result<HandleResult> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(128);
    let mut event_count = 0;

    // We still need a token even for one connection
    const TOKEN: Token = Token(0);

    let mut stream = TcpStream::connect(address)?;

    let mut handshake_received = false;
    let mut handshake_sent = false;
    let mut message_sent = false;
    let mut first_heartbeat_received = false;
    poll.registry()
        .register(&mut stream, TOKEN, Interest::WRITABLE)?;

    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "archive did not answer in time",
                    ));
                }
                Some(deadline - now)
            }
            None => None,
        };
        if let Err(e) = poll.poll(&mut events, timeout) {
            if interrupted(&e) {
                continue;
            }
            return Err(e);
        }

        for event in events.iter() {
            event_count += 1;
            // Failsafe to prevent infinite loops
            if event_count > super::MAX_EVENT_COUNT {
                return Err(io::Error::other(format!(
                    "FAILSAFE triggered, event count: {}",
                    event_count
                )));
            }
            match event.token() {
                TOKEN => {
                    match handle_connection_event(
                        poll.registry(),
                        &mut stream,
                        event,
                        data,
                        &mut handshake_received,
                        &mut handshake_sent,
                        &mut message_sent,
                        &mut first_heartbeat_received,
                    )? {
                        HandleResult::MessageSent => return Ok(HandleResult::MessageSent),
                        HandleResult::ConnectionClosed => {
                            return Ok(HandleResult::ConnectionClosed)
                        }
                        HandleResult::MessageWouldBlock => {
                            // do nothing, wait for the next event
                            continue;
                        }
                        HandleResult::ConnectionAlive => {
                            // keep swapping between readable and writable until we successfully send the message, then keep in read mode.
                            if message_sent {
                                poll.registry().reregister(
                                    &mut stream,
                                    TOKEN,
                                    Interest::READABLE,
                                )?;
                                continue;
                            }

                            if event.is_writable() {
                                poll.registry().reregister(
                                    &mut stream,
                                    TOKEN,
                                    Interest::READABLE,
                                )?;
                            } else {
                                poll.registry().reregister(
                                    &mut stream,
                                    TOKEN,
                                    Interest::WRITABLE,
                                )?;
                            }
                            continue;
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    }
}

fn _send_heartbeat(connection: &mut TcpStream) -> io::Result<HandleResult> {
    match connection.write_all(&HEARTBEAT_MSG) {
        Ok(_) => {
            connection.flush()?;
            Ok(HandleResult::ConnectionAlive)
        }
        Err(ref err) if would_block(err) => Ok(HandleResult::MessageWouldBlock),
        Err(ref err) if interrupted(err) => Ok(HandleResult::MessageWouldBlock),
        Err(err) => Err(err),
    }
}

struct RecursionGuard {
    count: u8,
    max_depth: u8,
}

impl RecursionGuard {
    fn new(max_depth: u8) -> Self {
        Self {
            count: 0,
            max_depth,
        }
    }

    fn increment(&mut self) -> io::Result<()> {
        self.count += 1;
        if self.count > self.max_depth {
            Err(io::ErrorKind::WriteZero.into())
        } else {
            Ok(())
        }
    }
}

fn send_data<F>(
    connection: &mut TcpStream,
    data: &[u8],
    recursion_guard: &mut RecursionGuard,
    // closure that can be called when the data is sent
    on_success: F,
) -> io::Result<HandleResult>
where
    F: FnOnce() -> io::Result<HandleResult>,
{
    match connection.write(data) {
        Ok(n) if n < data.len() => {
            recursion_guard.increment()?;
            let remaining_data = data[n..].to_vec();
            send_data(connection, &remaining_data, recursion_guard, on_success)
        }
        Ok(_) => {
            connection.flush()?;
            on_success()
        }
        Err(ref err) if would_block(err) => Ok(HandleResult::MessageWouldBlock),
        Err(ref err) if interrupted(err) => {
            recursion_guard
                .increment()
                .map_err(|_| io::ErrorKind::Interrupted)?;
            send_data(connection, data, recursion_guard, on_success)
        }
        Err(err) => Err(err),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_connection_event(
    registry: &Registry,
    connection: &mut TcpStream,
    event: &Event,
    data: &[u8],
    handshake_received: &mut bool,
    handshake_sent: &mut bool,
    message_sent: &mut bool,
    first_heartbeat_received: &mut bool,
) -> io::Result<HandleResult> {
    if event.is_writable() {
        if !*handshake_sent {
            let msg = prepend_length(&HEADER_MSG);
            send_data(
                connection,
                &msg,
                &mut RecursionGuard::new(MAX_RECURSION_DEPTH),
                || {
                    *handshake_sent = true;
                    Ok(HandleResult::ConnectionAlive)
                },
            )?;
            return Ok(HandleResult::ConnectionAlive);
        }

        if *handshake_received && *handshake_sent && !*message_sent && *first_heartbeat_received {
            send_data(
                connection,
                data,
                &mut RecursionGuard::new(MAX_RECURSION_DEPTH),
                || {
                    *message_sent = true;
                    Ok(HandleResult::ConnectionAlive)
                },
            )?;
        }
    }

    if event.is_readable() {
        let mut connection_closed = false;
        let mut received_data = vec![0; 4096];
        let mut bytes_read = 0;

        loop {
            match connection.read(&mut received_data[bytes_read..]) {
                Ok(0) => {
                    connection_closed = true;
                    break;
                }
                Ok(n) => {
                    bytes_read += n;
                    if bytes_read == received_data.len() {
                        received_data.resize(received_data.len() + 1024, 0);
                    }
                }
                // Would block "errors" are the OS's way of saying that the
                // connection is not actually ready to perform this I/O operation.
                Err(ref err) if would_block(err) => break,
                Err(ref err) if interrupted(err) => continue,
                // Other errors we'll consider fatal.
                Err(err) => return Err(err),
            }
        }

        if connection_closed {
            registry.deregister(connection)?;
            connection.shutdown(std::net::Shutdown::Both)?;
            return Ok(HandleResult::ConnectionClosed);
        }

        if bytes_read < 8 {
            // malformed message, at least the length should be present
            return Ok(HandleResult::ConnectionAlive);
        }

        let raw_message = RawMessage::from_bytes(&received_data[..bytes_read]);
        let messages = raw_message.parse_raw()?;

        for message in messages {
            match message {
                ParsedMessage::Header => {
                    *handshake_received = true;
                }
                ParsedMessage::Ok => {
                    connection.flush()?;
                    registry.deregister(connection)?;
                    connection.shutdown(std::net::Shutdown::Both)?;
                    return Ok(HandleResult::MessageSent);
                }
                ParsedMessage::Heartbeat => {
                    *first_heartbeat_received = true;
                }
                ParsedMessage::Unknown(msg) => {
                    registry.deregister(connection)?;
                    connection.shutdown(std::net::Shutdown::Both)?;
                    mina_node::core::warn!(
                        summary = "Received unknown message",
                        msg = format!("{:?}", msg)
                    );
                    return Ok(HandleResult::ConnectionClosed);
                }
            }
        }
    }

    Ok(HandleResult::ConnectionAlive)
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}

enum ParsedMessage {
    Heartbeat,
    Ok,
    Header,
    Unknown(Vec<u8>),
}

struct RawMessage {
    length: usize,
    data: Vec<u8>,
}

impl RawMessage {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            length: bytes.len(),
            data: bytes.to_vec(),
        }
    }

    fn parse_raw(&self) -> io::Result<Vec<ParsedMessage>> {
        let mut parsed_bytes: usize = 0;

        // more than one message can be sent in a single packet
        let mut messages = Vec::new();

        while parsed_bytes < self.length {
            // first 8 bytes are the length in little endian
            let length = u64::from_le_bytes(
                self.data[parsed_bytes..parsed_bytes + 8]
                    .try_into()
                    .unwrap(),
            ) as usize;
            parsed_bytes += 8;

            if parsed_bytes + length > self.length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Message length exceeds raw message length",
                ));
            }

            if length == HEADER_MSG.len()
                && self.data[parsed_bytes..parsed_bytes + length] == HEADER_MSG
            {
                messages.push(ParsedMessage::Header);
            } else if length == OK_MSG.len()
                && self.data[parsed_bytes..parsed_bytes + length] == OK_MSG
            {
                messages.push(ParsedMessage::Ok);
            } else if length == HEARTBEAT_MSG.len()
                && self.data[parsed_bytes..parsed_bytes + length] == HEARTBEAT_MSG
            {
                messages.push(ParsedMessage::Heartbeat);
            } else {
                messages.push(ParsedMessage::Unknown(
                    self.data[parsed_bytes..parsed_bytes + length].to_vec(),
                ));
            }

            parsed_bytes += length;
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_must_match_the_request_id() {
        // Async RPC: Response(id=1, Ok(length=1, unit)).
        let reply = RawMessage::from_bytes(&prepend_length(&OK_MSG));
        assert!(matches!(&reply.parse_raw().unwrap()[0], ParsedMessage::Ok));
        // Same unit success for id=1959, formerly misnamed CLOSE_MSG.
        let other_id = [2, 254, 167, 7, 0, 1, 0];
        let reply = RawMessage::from_bytes(&prepend_length(&other_id));
        assert!(matches!(
            &reply.parse_raw().unwrap()[0],
            ParsedMessage::Unknown(_)
        ));
    }
}
