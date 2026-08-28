//! Query-time lexical scoring stack, ported verbatim from Python.
//!
//! Ported from `oracle/server/query_engine.py` with golden-verified parity.

pub mod engine;
pub mod lexical;
pub mod pool_embedder;
pub mod redact;
