//! Oracle store layer: SQLite metadata store, LanceDB vector store, CKG store,
//! and the chunk-index manifest IO.

pub mod ckg;
#[cfg(feature = "full")]
pub mod lance;
#[cfg(feature = "full")]
pub mod manifest;
#[cfg(feature = "full")]
pub mod sqlite;
