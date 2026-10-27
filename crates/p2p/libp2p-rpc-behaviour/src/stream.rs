use std::{
    collections::BTreeSet,
    io,
    sync::Arc,
    task::{self, Context, Poll},
};

use libp2p::{
    core::upgrade::ReadyUpgrade, futures::AsyncWrite, swarm::handler::InboundUpgradeSend,
    StreamProtocol,
};
use mina_p2p_messages::rpc_kernel::RpcTag;

use super::{
    behaviour::{Event, StreamId},
    state,
};

pub struct Stream {
    opening_state: Option<OpeningState>,
    inner_state: state::Inner,
    closing: bool,
    write_closed: bool,
}

enum OpeningState {
    Requested,
    Negotiated {
        io: <ReadyUpgrade<StreamProtocol> as InboundUpgradeSend>::Output,
    },
}

pub enum StreamEvent {
    Request(u32),
    Event(Event),
}

impl Stream {
    pub fn new_outgoing(ask_menu: bool) -> Self {
        Stream {
            opening_state: None,
            closing: false,
            write_closed: false,
            // empty menu for outgoing stream
            inner_state: state::Inner::new(Arc::new(BTreeSet::default()), ask_menu),
        }
    }

    pub fn new_incoming(menu: Arc<BTreeSet<(RpcTag, u32)>>) -> Self {
        Stream {
            opening_state: None,
            closing: false,
            write_closed: false,
            inner_state: state::Inner::new(menu, false),
        }
    }

    pub fn negotiated(&mut self, io: <ReadyUpgrade<StreamProtocol> as InboundUpgradeSend>::Output) {
        self.opening_state = Some(OpeningState::Negotiated { io });
    }

    pub fn add(&mut self, bytes: Vec<u8>) {
        self.inner_state.add(bytes);
    }

    pub fn stop_heartbeats(&mut self) {
        self.inner_state.heartbeat_replies = false;
    }

    pub fn close(&mut self) {
        self.closing = true;
    }

    pub fn poll_stream(
        &mut self,
        stream_id: StreamId,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<StreamEvent>> {
        match &mut self.opening_state {
            None => {
                if let StreamId::Outgoing(id) = stream_id {
                    self.opening_state = Some(OpeningState::Requested);
                    Poll::Ready(Ok(StreamEvent::Request(id)))
                } else {
                    Poll::Pending
                }
            }
            Some(OpeningState::Requested) => Poll::Pending,
            Some(OpeningState::Negotiated { io }) => {
                let received = if self.closing {
                    if !self.write_closed {
                        task::ready!(self.inner_state.poll_send(cx, io))?;
                        task::ready!(std::pin::Pin::new(&mut *io).poll_close(cx))?;
                        self.write_closed = true;
                    }
                    self.inner_state.poll_recv(cx, io)
                } else {
                    self.inner_state.poll(cx, io)
                };
                let received = match task::ready!(received) {
                    Err(err) => {
                        if err.kind() == io::ErrorKind::UnexpectedEof
                            && !self.closing
                            && self.inner_state.heartbeat_replies
                        {
                            if let StreamId::Outgoing(id) = stream_id {
                                log::warn!("reopen stream");
                                self.opening_state = Some(OpeningState::Requested);
                                return Poll::Ready(Ok(StreamEvent::Request(id)));
                            } else {
                                return Poll::Ready(Err(err));
                            }
                        } else {
                            return Poll::Ready(Err(err));
                        }
                    }
                    Ok(v) => v,
                };
                Poll::Ready(Ok(StreamEvent::Event(Event::Stream {
                    stream_id,
                    received,
                })))
            }
        }
    }
}
