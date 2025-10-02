use std::io;
use std::string::FromUtf8Error;
use thiserror::Error;

// 错误类型
#[derive(Error, Debug)]
pub enum RexError {
    #[error("Insufficient data in buffer")]
    InsufficientData,

    #[error("Invalid command")]
    InvalidCommand,

    #[error("Invalid return code")]
    InvalidRetCode,

    #[error("Data corrupted or CRC mismatch")]
    DataCorrupted,

    #[error("Version mismatch")]
    VersionMismatch,

    #[error("String encoding error: {0}")]
    InvalidString(#[from] FromUtf8Error),

    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}
