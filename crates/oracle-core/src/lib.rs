//! oracle-core: Rust-native Oracle runtime (indexing and retrieval).
//!
//! Oracle points at files and lines; it does not answer. Module layout:
//! store/, ingest/, query/, embed. `oracle-cli` and `examples/recon_bench`
//! are the evaluation bench for chunking and models.

pub mod cluster;
pub mod config;
pub mod doctor;
pub mod embed;
pub mod embedder;
pub mod ingest;
pub mod lance;
pub mod model_download;
pub mod onnx_embedder;
pub mod query;
pub mod store;

use clap::ValueEnum;

/// Embedding backend selector shared by the CLI and (later) runtime config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    Candle,
    Onnx,
}
