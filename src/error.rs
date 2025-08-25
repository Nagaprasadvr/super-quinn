use quinn::ConnectionError;
use thiserror::Error;
use tokio::io;
use tokio::sync::mpsc::error::{SendError, TryRecvError};
use tokio::task::JoinError;

use crate::message::SendMessageError;

#[derive(Debug, Error)]
pub enum SuperQuinnError {
    #[error("Invalid quic config")]
    InvalidQuicConfig,
    #[error("Invalid message")]
    InvalidMessage,
    #[error("Message send error")]
    MessageSendError(#[from] SendError<String>),
    #[error("Message recv error")]
    MessageRecvError(#[from] TryRecvError),
    #[error("Quinn conn error")]
    QuinnConnError(#[from] ConnectionError),
    #[error("IO error")]
    IoError(#[from] io::Error),
    #[error("Send msg error")]
    SendMsgError(SendMessageError),
    #[error("Tokio task join error")]
    JoinError(#[from] JoinError),
    #[error("anyhow result")]
    AnyhowRes(#[from] anyhow::Error),
}

pub type SuperQuinnResult<T> = Result<T, SuperQuinnError>;
