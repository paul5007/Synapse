use thiserror::Error;

/// Storage backend contract.
pub trait Db: Send + Sync {}

/// Storage failures.
#[derive(Debug, Error)]
pub enum StorageError {}
