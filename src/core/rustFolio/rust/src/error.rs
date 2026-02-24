// error.rs — FolioError type hierarchy
// Maps to Fantom exception types checked by the testFolio harness.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FolioError {
    #[error("Record not found: {0}")]
    UnknownRec(String),

    #[error("Commit error: {0}")]
    CommitErr(String),

    #[error("Concurrent change: {0}")]
    ConcurrentChange(String),

    #[error("Database is closed")]
    Shutdown,

    #[error("Diff error: {0}")]
    DiffErr(String),

    #[error("Invalid tag value: {0}")]
    InvalidTagVal(String),

    #[error("History config error: {0}")]
    HisConfig(String),

    #[error("History write error: {0}")]
    HisWrite(String),

    #[error("Authentication failed: bad or missing token")]
    AuthFailed,

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Storage error: {0}")]
    Storage(#[from] redb::Error),
}

impl FolioError {
    /// Wire protocol error code — must match Fantom client's switch statement.
    pub fn wire_code(&self) -> u16 {
        match self {
            FolioError::UnknownRec(_)     => 0x0001,
            FolioError::CommitErr(_)      => 0x0002,
            FolioError::ConcurrentChange(_) => 0x0003,
            FolioError::Shutdown          => 0x0004,
            FolioError::DiffErr(_)        => 0x0005,
            FolioError::InvalidTagVal(_)  => 0x0006,
            FolioError::Io(_)             => 0x0007,
            FolioError::HisConfig(_)      => 0x0008,
            FolioError::HisWrite(_)       => 0x0009,
            _                             => 0x00FF,
        }
    }
}

// Allow converting redb::CommitError and redb::TableError
impl From<redb::CommitError> for FolioError {
    fn from(e: redb::CommitError) -> Self {
        FolioError::Storage(redb::Error::from(e))
    }
}

impl From<redb::TableError> for FolioError {
    fn from(e: redb::TableError) -> Self {
        FolioError::Storage(redb::Error::from(e))
    }
}

impl From<redb::StorageError> for FolioError {
    fn from(e: redb::StorageError) -> Self {
        FolioError::Storage(redb::Error::from(e))
    }
}

impl From<redb::TransactionError> for FolioError {
    fn from(e: redb::TransactionError) -> Self {
        FolioError::Storage(redb::Error::from(e))
    }
}

impl From<redb::DatabaseError> for FolioError {
    fn from(e: redb::DatabaseError) -> Self {
        FolioError::Storage(redb::Error::from(e))
    }
}

pub type Result<T> = std::result::Result<T, FolioError>;
