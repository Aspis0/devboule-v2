//! Query-side adapter for the resident [`EmbedderPool`].

use anyhow::{Context, Result};

use crate::embed::{CancelFlag, EmbedderPool};
use crate::ingest::indexer::TextEmbedder;
use crate::ingest::retrieval_text::query_embedding_text_for_model;
use crate::query::engine::QueryEmbedder;

/// Embeds queries with the same model and semantic-prefix mode used by the
/// chunk indexer.
pub struct PoolQueryEmbedder<'a> {
    pool: &'a EmbedderPool,
    cancel: &'a CancelFlag,
    uses_semantic_prefix: bool,
}

impl<'a> PoolQueryEmbedder<'a> {
    /// Build an adapter while resolving the model's declared prefix mode.
    ///
    /// The indexer obtains this value through the same `TextEmbedder` trait;
    /// resolving it here prevents query and chunk text from silently taking
    /// different embedding paths.
    pub fn new(pool: &'a EmbedderPool, cancel: &'a CancelFlag) -> Result<Self> {
        let uses_semantic_prefix = <EmbedderPool as TextEmbedder>::uses_semantic_prefix(pool)
            .context("reading embedding model prefix configuration")?;
        Ok(Self {
            pool,
            cancel,
            uses_semantic_prefix,
        })
    }
}

impl QueryEmbedder for PoolQueryEmbedder<'_> {
    fn embed_query(&self, text: &str, _dims: usize) -> Result<Vec<f32>> {
        let model_text = query_embedding_text_for_model(text, None, self.uses_semantic_prefix);
        let vectors = self
            .pool
            .embed(&[model_text], 8, self.cancel)
            .context("embedding Oracle query")?;
        vectors
            .into_iter()
            .next()
            .context("embedding pool returned no query vector")
    }
}
