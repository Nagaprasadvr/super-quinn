use std::sync::Arc;
use std::{fmt::Debug, net::SocketAddr, usize};

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error};

use crate::error::SuperQuinnError;
use crate::{error::SuperQuinnResult, network::MAX_CONCURRENCY};

// MESSAGE FORMAT
// [0..2] = (u16) len of message - N
// [2..4] = (u16) MSG_TYPE_DISCRIMINATOR
// [4..N] = message data

const MSG_LEN_BYTES_LEN: usize = 2;
const MSG_TYPE_BYTES_LEN: usize = 2;

type MessageType = u16;

pub struct WireMessageWithAddr(pub Vec<u8>, pub SocketAddr);

impl WireMessageWithAddr {
    pub fn to_raw_msgs(self) -> SuperQuinnResult<RawMsgsWithAddr> {
        let Self(data, addr) = self;

        let mut raw_msgs_data = Vec::new();
        let mut cursor = 0;

        while cursor + MSG_LEN_BYTES_LEN + MSG_TYPE_BYTES_LEN <= data.len() {
            // parse msg_len
            let msg_len = data
                .get(cursor..cursor + MSG_LEN_BYTES_LEN)
                .and_then(|b| b.try_into().ok())
                .map(|arr: [u8; 2]| u16::from_le_bytes(arr) as usize)
                .ok_or(SuperQuinnError::InvalidMessage)?;

            cursor += MSG_LEN_BYTES_LEN;

            // parse msg_type
            let msg_type = data
                .get(cursor..cursor + MSG_TYPE_BYTES_LEN)
                .and_then(|b| b.try_into().ok())
                .map(|arr: [u8; 2]| u16::from_le_bytes(arr))
                .ok_or(SuperQuinnError::InvalidMessage)?;

            cursor += MSG_TYPE_BYTES_LEN;

            // bounds check
            if cursor + msg_len > data.len() {
                return Err(SuperQuinnError::InvalidMessage);
            }

            // extract payload
            let msg_data = data[cursor..cursor + msg_len].to_vec();
            raw_msgs_data.push(RawMessage(msg_type, msg_data));

            cursor += msg_len;
        }

        Ok(RawMsgsWithAddr(raw_msgs_data, addr))
    }

    pub fn from_raw_msgs(raw_msgs: RawMsgsWithAddr) -> Self {
        let mut wire_msgs_data = Vec::new();

        for m in raw_msgs.0 {
            let mut wire_msg_data = Vec::new();
            wire_msg_data.extend((m.1.len() as u16).to_le_bytes()); // msg len
            wire_msg_data.extend(m.0.to_le_bytes()); // msg type
            wire_msg_data.extend(m.1); // msg data
            wire_msgs_data.extend(wire_msg_data);
        }

        Self(wire_msgs_data, raw_msgs.1)
    }

    pub fn data(self) -> Vec<u8> {
        self.0
    }
}

pub struct RawMsgsWithAddr(pub Vec<RawMessage>, pub SocketAddr);

pub struct RawMessage(MessageType, Vec<u8>);

impl RawMessage {
    pub fn new(msg_type: MessageType, data: Vec<u8>) -> Self {
        Self(msg_type, data)
    }

    pub fn msg_type(&self) -> MessageType {
        self.0
    }

    pub fn msg_data(&self) -> &[u8] {
        self.1.as_slice()
    }
}

impl RawMsgsWithAddr {
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug)]
pub enum SendMessageError {
    SendMsgError,
    SendVecMsgError(usize),
}

impl ToString for SendMessageError {
    fn to_string(&self) -> String {
        match self {
            SendMessageError::SendMsgError => "send msg error".to_string(),
            SendMessageError::SendVecMsgError(count) => format!("send msgs error : {count}"),
        }
    }
}

pub trait Message: Debug + Sized + Sync + Send + 'static {
    const MSG_TYPE_DISCRIMINATOR: u16;
    fn message_len(&self) -> u16;
    fn pack(&self) -> SuperQuinnResult<Vec<u8>>;
    fn unpack(bytes: &[u8]) -> SuperQuinnResult<Self>;
}

pub struct MessageSender {
    tx: Sender<Vec<RawMsgsWithAddr>>,
}

impl MessageSender {
    pub fn new(tx: Sender<Vec<RawMsgsWithAddr>>) -> Self {
        Self { tx }
    }
    pub async fn send_msgs(&self, raw_msgs: Vec<RawMsgsWithAddr>) -> Result<(), SendMessageError> {
        let count = raw_msgs.len();
        debug!(count, "msg_sender::send_msgs");
        self.tx.send(raw_msgs).await.map_err(|_e| {
            error!(count, "msg_sender::send_msgs error");
            SendMessageError::SendVecMsgError(count)
        })
    }
}

pub trait MessageHandler: Clone + Send + Sync + Sized + 'static {
    fn handle_raw_msg(
        &self,
        raw_msg: RawMessage,
        addr: SocketAddr,
    ) -> impl Future<Output = SuperQuinnResult<()>> + Send;
}

pub struct MessageProcessor<H: MessageHandler> {
    rx: Receiver<RawMsgsWithAddr>,
    handler: Arc<H>,
}

impl<H: MessageHandler> MessageProcessor<H> {
    pub fn start(mut self) -> JoinHandle<Result<(), SuperQuinnError>> {
        tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let handler = self.handler;

            while let Some(raw_msgs) = self.rx.recv().await {
                let count = raw_msgs.len();
                debug!(count, "msg_processor::recv_msgs");

                if tasks.len() >= MAX_CONCURRENCY {
                    tasks.join_next().await;
                }
                let handler = handler.clone();
                tasks.spawn(async move {
                    // Try unpacking all messages
                    let msgs: Vec<RawMessage> = raw_msgs.0.into_iter().map(|m| m).collect();
                    let addr = raw_msgs.1;

                    for msg in msgs {
                        // Spawn handler task
                        if let Err(err) = handler.handle_raw_msg(msg, addr).await {
                            error!(?err, "msg_processor::handle_msgs error {err}");
                        }
                    }
                });
            }

            tasks.join_all().await;

            Ok(())
        })
    }
}

pub struct MessageProcessorBuilder<H: MessageHandler> {
    rx: Option<Receiver<RawMsgsWithAddr>>,
    handler: Option<H>,
}

impl<H: MessageHandler> MessageProcessorBuilder<H> {
    pub fn new() -> Self {
        Self {
            rx: None,
            handler: None,
        }
    }

    pub fn with_receiver(mut self, rx: Receiver<RawMsgsWithAddr>) -> Self {
        self.rx = Some(rx);
        self
    }

    pub fn with_handler(mut self, handler: H) -> Self {
        self.handler = Some(handler);
        self
    }

    pub fn build(self) -> MessageProcessor<H> {
        MessageProcessor {
            rx: self.rx.expect("MessageProcessor requires a receiver"),
            handler: Arc::new(self.handler.expect("MessageProcessor requires a handler")),
        }
    }
}

impl<H: MessageHandler> MessageProcessor<H> {
    pub fn builder() -> MessageProcessorBuilder<H> {
        MessageProcessorBuilder::new()
    }
}
