//! Ingestion pipeline: file collection, chunking, and embedding-text assembly.
//!
//! Ports of `oracle/ingestion/*.py`, phase P2 of PLAN.md.

#[cfg(feature = "full")]
pub mod ast_chunker;
#[cfg(feature = "full")]
pub mod chunking;
#[cfg(feature = "full")]
pub mod ckg_build;
pub mod collect;
#[cfg(feature = "full")]
pub mod indexer;
#[cfg(feature = "full")]
pub mod retrieval_text;
pub mod text_extensions;
