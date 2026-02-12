use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    #[error("Database error: {0}")]
    Db(String),

    #[error("Message not found")]
    NotFound,

    #[error("Client not found: {0}")]
    ClientNotFound(u128),

    #[error("Invalid message format")]
    InvalidFormat,
}

pub type Result<T> = std::result::Result<T, PersistenceError>;
