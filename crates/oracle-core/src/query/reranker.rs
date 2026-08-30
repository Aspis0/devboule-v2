//! Optional ONNX cross-encoder used to reorder the dense candidate set.
//!
//! The reranker is deliberately a query-time component.  It never participates
//! in indexing and therefore never changes the embedding recipe or the vectors
//! stored in LanceDB.  A model directory is optional: callers can leave it out
//! and the dense path remains unchanged.

use anyhow::{bail, Context, Result};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokenizers::{
    PaddingDirection, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams,
    TruncationStrategy,
};

use crate::embed::EpArg;

/// Default on-disk location for the optional reranker.
pub const DEFAULT_RERANKER_MODEL_ID: &str = "ms-marco-TinyBERT-L-2-v2";

/// Configuration written next to the downloaded Xenova export.  The ONNX
/// artifact does not carry the pair semantics or the declared tokenizer
/// limit that the query path needs, so keep those facts in our sidecar.
pub const RERANKER_MODEL_CONFIG_JSON: &str = r#"{
  "id": "ms-marco-TinyBERT-L-2-v2",
  "onnx_graph": "onnx/model_quantized.onnx",
  "tokenizer_file": "tokenizer.json",
  "max_seq_tokens": 512,
  "pair": {"mode": "tokenizer_pair", "first": "query", "second": "document"}
}"#;

/// Production rerank depth. Measured on the 160-question bench once per-file
/// deduplication landed: depth 20 and depth 50 reach the same recall@5
/// (0.89687), while 20 ranks better (MRR@10 0.74820 against 0.72600) and costs
/// half the latency (158 ms average against 333 ms). Before deduplication the
/// extra depth was buying variety that duplicate spans had eaten, so 50 was
/// worth its price; it no longer is. The benchmark still overrides this with
/// explicit 20/50 passes, and production can tune it without touching an index
/// because reranking is not part of the embedding recipe.
pub const DEFAULT_RERANKER_CANDIDATES: usize = 20;

/// Query-side candidate depth, clamped to the public retrieval bound.
pub fn resolve_candidate_limit() -> usize {
    std::env::var("ORACLE_RERANK_CANDIDATES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_RERANKER_CANDIDATES)
        .clamp(1, crate::config::MAX_BOUNDED_LIMIT)
}

/// `<oracle-data-root>/models/<id>` for the optional query model.
pub fn default_model_dir(oracle_data_root: &Path) -> PathBuf {
    let configured = std::env::var_os("ORACLE_RERANKER_MODEL_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    configured.unwrap_or_else(|| {
        oracle_data_root
            .join("models")
            .join(DEFAULT_RERANKER_MODEL_ID)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairSide {
    Query,
    Document,
}

impl PairSide {
    fn parse(raw: &str, model_dir: &Path) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "query" => Ok(Self::Query),
            "document" | "doc" => Ok(Self::Document),
            other => bail!(
                "reranker model_config.json at {} has unknown pair side `{other}` \
                 (expected query | document)",
                model_dir.display()
            ),
        }
    }
}

/// Declarative construction of the two sequences passed to the tokenizer.
///
/// This is kept in the artifact config because swapping the sequence order is
/// model behavior, not a property that can be inferred from an ONNX graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairConstruction {
    first: PairSide,
    second: PairSide,
}

impl PairConstruction {
    fn parse(raw: PairConstructionFile, model_dir: &Path) -> Result<Self> {
        let first = PairSide::parse(&raw.first, model_dir)?;
        let second = PairSide::parse(&raw.second, model_dir)?;
        if first == second {
            bail!(
                "reranker model_config.json at {} must declare one query and one document in pair",
                model_dir.display()
            );
        }
        Ok(Self { first, second })
    }

    fn strings<'a>(&self, query: &'a str, document: &'a str) -> (&'a str, &'a str) {
        let side = |which| match which {
            PairSide::Query => query,
            PairSide::Document => document,
        };
        (side(self.first), side(self.second))
    }
}

#[derive(Debug, Deserialize)]
struct PairConstructionFile {
    mode: String,
    first: String,
    second: String,
}

#[derive(Debug, Deserialize)]
struct RerankerConfigFile {
    id: Option<String>,
    onnx_graph: Option<String>,
    tokenizer_file: Option<String>,
    max_seq_tokens: Option<usize>,
    pair: Option<PairConstructionFile>,
}

/// All facts that are not reliably recoverable from the loaded graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankerConfig {
    pub id: String,
    pub onnx_graph: String,
    pub tokenizer_file: String,
    pub max_seq_tokens: usize,
    pub pair: PairConstruction,
}

impl RerankerConfig {
    /// Load and validate the reranker artifact declaration without opening
    /// the graph.  Missing fields are hard errors when a model directory was
    /// explicitly supplied; an absent directory is handled by the optional
    /// handle below.
    pub fn load(model_dir: &Path) -> Result<Self> {
        let path = model_dir.join("model_config.json");
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "missing reranker model_config.json in {} \
                 (onnx_graph, tokenizer_file, max_seq_tokens, and pair must be declared)",
                model_dir.display()
            )
        })?;
        let parsed: RerankerConfigFile =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

        let id = parsed
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                model_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown-reranker".to_string())
            });
        let onnx_graph = required_string(parsed.onnx_graph, "onnx_graph", model_dir)?;
        let tokenizer_file = required_string(parsed.tokenizer_file, "tokenizer_file", model_dir)?;
        let max_seq_tokens = parsed.max_seq_tokens.filter(|&n| n > 0).ok_or_else(|| {
            anyhow::anyhow!(
                "reranker model_config.json at {} is missing required positive field `max_seq_tokens`",
                model_dir.display()
            )
        })?;
        let pair_file = parsed.pair.ok_or_else(|| {
            anyhow::anyhow!(
                "reranker model_config.json at {} is missing required field `pair`",
                model_dir.display()
            )
        })?;
        if !pair_file.mode.trim().eq_ignore_ascii_case("tokenizer_pair") {
            bail!(
                "reranker model_config.json at {} has unsupported pair.mode `{}` \
                 (expected tokenizer_pair)",
                model_dir.display(),
                pair_file.mode
            );
        }

        Ok(Self {
            id,
            onnx_graph,
            tokenizer_file,
            max_seq_tokens,
            pair: PairConstruction::parse(pair_file, model_dir)?,
        })
    }

    pub fn graph_path(&self, model_dir: &Path) -> Result<PathBuf> {
        let path = model_dir.join(&self.onnx_graph);
        if !path.is_file() {
            bail!(
                "declared reranker ONNX graph not found: {} \
                 (reranker model_config.json onnx_graph={} in {})",
                path.display(),
                self.onnx_graph,
                model_dir.display()
            );
        }
        Ok(path)
    }

    pub fn tokenizer_path(&self, model_dir: &Path) -> Result<PathBuf> {
        let path = model_dir.join(&self.tokenizer_file);
        if !path.is_file() {
            bail!(
                "declared reranker tokenizer not found: {} \
                 (reranker model_config.json tokenizer_file={} in {})",
                path.display(),
                self.tokenizer_file,
                model_dir.display()
            );
        }
        Ok(path)
    }
}

/// UI/installer-only check for a complete reranker bundle.  Loading the ONNX
/// session remains lazy; this only validates the declared sidecar and files.
pub fn configured_reranker_present(model_dir: &Path) -> bool {
    let Ok(config) = RerankerConfig::load(model_dir) else {
        return false;
    };
    config.graph_path(model_dir).is_ok() && config.tokenizer_path(model_dir).is_ok()
}

fn required_string(value: Option<String>, field: &str, model_dir: &Path) -> Result<String> {
    value.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "reranker model_config.json at {} is missing required field `{field}`",
            model_dir.display()
        )
    })
}

/// A loaded ONNX cross-encoder.  Graph input/output names are read from the
/// session; only the semantic meaning of the standard BERT input names is used
/// to feed tensors.
pub struct OnnxReranker {
    session: Session,
    tokenizer: Tokenizer,
    config: RerankerConfig,
    output_name: String,
}

impl OnnxReranker {
    pub fn load(model_dir: &Path, ep: EpArg) -> Result<Self> {
        let config = RerankerConfig::load(model_dir)?;
        let model_path = config.graph_path(model_dir)?;
        let tokenizer_path = config.tokenizer_path(model_dir)?;

        let session_builder =
            Session::builder().context("failed to create reranker ONNX session builder")?;
        let mut builder = session_builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("failed to set reranker optimization level: {e}"))?
            .with_intra_threads(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4),
            )
            .map_err(|e| anyhow::anyhow!("failed to set reranker intra-op threads: {e}"))?;

        #[cfg(target_os = "macos")]
        if matches!(ep, EpArg::Coreml) {
            use ort::ep;
            builder = builder
                .with_execution_providers([ep::CoreML::default()
                    .with_model_format(ep::coreml::ModelFormat::MLProgram)
                    .build()])
                .map_err(|e| anyhow::anyhow!("failed to register reranker CoreML provider: {e}"))?;
        }
        #[cfg(not(target_os = "macos"))]
        if matches!(ep, EpArg::Coreml) {
            bail!("--ep coreml is only supported on macOS builds");
        }

        #[cfg(target_os = "windows")]
        if matches!(ep, EpArg::Directml) {
            use ort::ep;
            builder = builder
                .with_execution_providers([ep::DirectML::default().build()])
                .map_err(|e| {
                    anyhow::anyhow!("failed to register reranker DirectML provider: {e}")
                })?;
        }
        #[cfg(not(target_os = "windows"))]
        if matches!(ep, EpArg::Directml) {
            bail!("--ep directml is only supported on Windows builds");
        }

        let session = builder.commit_from_file(&model_path).with_context(|| {
            format!(
                "failed to build reranker ONNX session from {}",
                model_path.display()
            )
        })?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to load reranker tokenizer from {}: {e}",
                tokenizer_path.display()
            )
        })?;

        let output_name = session
            .outputs()
            .first()
            .map(|outlet| outlet.name().to_string())
            .ok_or_else(|| anyhow::anyhow!("reranker ONNX graph has no outputs"))?;
        if session.outputs().len() != 1 {
            bail!(
                "reranker ONNX graph has {} outputs; expected exactly one score output",
                session.outputs().len()
            );
        }

        Ok(Self {
            session,
            tokenizer,
            config,
            output_name,
        })
    }

    pub fn config(&self) -> &RerankerConfig {
        &self.config
    }

    /// Score query/document pairs in order.  The model's raw scalar is used
    /// only for ordering; no sigmoid is applied because ranking is monotonic.
    pub fn score_pairs(&mut self, query: &str, documents: &[String]) -> Result<Vec<f64>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        self.tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            ..Default::default()
        }));
        self.tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: self.config.max_seq_tokens,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                ..Default::default()
            }))
            .map_err(|e| {
                anyhow::anyhow!("failed to configure reranker tokenizer truncation: {e}")
            })?;

        // Keep the full candidate list in one graph call when it is small, but
        // bound the padded attention tensor for callers that choose a larger
        // depth.  The default 50 × 512² is intentionally split into batches.
        let batch_size = std::env::var("ORACLE_RERANK_BATCH_SIZE")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(16);
        let mut scores = Vec::with_capacity(documents.len());
        for group in documents.chunks(batch_size) {
            let pairs: Vec<(&str, &str)> = group
                .iter()
                .map(|document| self.config.pair.strings(query, document))
                .collect();
            let encodings = self
                .tokenizer
                .encode_batch(pairs, true)
                .map_err(|e| anyhow::anyhow!("reranker tokenization failed: {e}"))?;
            let batch = encodings.len();
            let seq_len = encodings
                .iter()
                .map(|encoding| encoding.get_ids().len())
                .max()
                .unwrap_or(1)
                .max(1);
            if seq_len > self.config.max_seq_tokens {
                bail!(
                    "reranker tokenizer produced sequence length {} above declared max_seq_tokens={}",
                    seq_len,
                    self.config.max_seq_tokens
                );
            }
            let mut ids = Vec::with_capacity(batch * seq_len);
            let mut masks = Vec::with_capacity(batch * seq_len);
            let mut type_ids = Vec::with_capacity(batch * seq_len);
            for encoding in &encodings {
                for j in 0..seq_len {
                    ids.push(encoding.get_ids().get(j).copied().unwrap_or(0) as i64);
                    masks.push(encoding.get_attention_mask().get(j).copied().unwrap_or(0) as i64);
                    type_ids.push(encoding.get_type_ids().get(j).copied().unwrap_or(0) as i64);
                }
            }

            let mut ids = Some(ids);
            let mut masks = Some(masks);
            let mut type_ids = Some(type_ids);
            let mut inputs: Vec<(
                std::borrow::Cow<'static, str>,
                ort::session::SessionInputValue<'static>,
            )> = Vec::with_capacity(self.session.inputs().len());
            for outlet in self.session.inputs() {
                let name = outlet.name();
                let value = match name {
                    "input_ids" => Tensor::from_array((
                        [batch, seq_len],
                        ids.take().expect("input_ids is unique").into_boxed_slice(),
                    ))?
                    .into(),
                    "attention_mask" => Tensor::from_array((
                        [batch, seq_len],
                        masks
                            .take()
                            .expect("attention_mask is unique")
                            .into_boxed_slice(),
                    ))?
                    .into(),
                    "token_type_ids" => Tensor::from_array((
                        [batch, seq_len],
                        type_ids
                            .take()
                            .expect("token_type_ids is unique")
                            .into_boxed_slice(),
                    ))?
                    .into(),
                    other => bail!(
                        "unhandled reranker ONNX input `{other}` on model {}",
                        self.config.id
                    ),
                };
                inputs.push((name.to_string().into(), value));
            }
            if ids.is_some() || masks.is_some() || type_ids.is_some() {
                bail!(
                    "reranker ONNX graph does not declare all standard BERT inputs \
                     (input_ids, attention_mask, token_type_ids)"
                );
            }

            let outputs = self
                .session
                .run(inputs)
                .context("reranker ONNX session run failed")?;
            let (shape, values) = outputs[self.output_name.as_str()]
                .try_extract_tensor::<f32>()
                .context("failed to extract reranker score output")?;
            if values.len() != batch {
                bail!(
                    "reranker score output has {} values for batch of {} (shape {shape:?})",
                    values.len(),
                    batch
                );
            }
            scores.extend(values.iter().map(|&value| value as f64));
        }
        Ok(scores)
    }
}

/// Lazy, shareable reranker.  Loading is deferred until the first query so
/// an installed optional bundle does not slow startup; the loaded session is
/// reused across queries and serialized like the embedding pool.
pub struct RerankerHandle {
    model_dir: PathBuf,
    ep: EpArg,
    model: Mutex<Option<OnnxReranker>>,
}

impl RerankerHandle {
    /// Return `None` when the optional bundle is absent or incomplete.  The
    /// graph itself is still loaded lazily on first use.
    pub fn if_present(model_dir: PathBuf, ep: EpArg) -> Option<Self> {
        configured_reranker_present(&model_dir).then_some(Self {
            model_dir,
            ep,
            model: Mutex::new(None),
        })
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn score_pairs(&self, query: &str, documents: &[String]) -> Result<Vec<f64>> {
        let mut guard = self.model.lock().unwrap_or_else(|error| error.into_inner());
        if guard.is_none() {
            *guard = Some(OnnxReranker::load(&self.model_dir, self.ep)?);
        }
        guard
            .as_mut()
            .expect("reranker loaded")
            .score_pairs(query, documents)
    }
}

impl std::fmt::Debug for RerankerHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RerankerHandle")
            .field("model_dir", &self.model_dir)
            .field("ep", &self.ep)
            .field(
                "loaded",
                &self.model.lock().map(|m| m.is_some()).unwrap_or(false),
            )
            .finish()
    }
}

pub type SharedReranker = Arc<RerankerHandle>;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_declares_pair_order_and_limits() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("model_config.json"),
            r#"{
              "id": "tiny-reranker",
              "onnx_graph": "onnx/model_int8.onnx",
              "tokenizer_file": "tokenizer.json",
              "max_seq_tokens": 512,
              "pair": {"mode": "tokenizer_pair", "first": "query", "second": "document"}
            }"#,
        )
        .unwrap();
        let config = RerankerConfig::load(dir.path()).unwrap();
        assert_eq!(config.id, "tiny-reranker");
        assert_eq!(config.max_seq_tokens, 512);
        assert_eq!(config.pair.strings("q", "d"), ("q", "d"));
    }

    #[test]
    fn duplicate_pair_side_is_rejected() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("model_config.json"),
            r#"{
              "onnx_graph": "model.onnx",
              "tokenizer_file": "tokenizer.json",
              "max_seq_tokens": 512,
              "pair": {"mode": "tokenizer_pair", "first": "query", "second": "query"}
            }"#,
        )
        .unwrap();
        let error = RerankerConfig::load(dir.path()).expect_err("pair must contain both sides");
        assert!(format!("{error:#}").contains("one query and one document"));
    }

    #[test]
    fn absent_optional_bundle_is_a_noop() {
        let dir = tempdir().unwrap().path().join("missing");
        assert!(RerankerHandle::if_present(dir, EpArg::Cpu).is_none());
    }

    #[test]
    fn configured_bundle_requires_the_sidecar_and_declared_files() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("model_config.json"),
            RERANKER_MODEL_CONFIG_JSON,
        )
        .unwrap();
        assert!(!configured_reranker_present(dir.path()));

        std::fs::write(dir.path().join("tokenizer.json"), "{}").unwrap();
        assert!(!configured_reranker_present(dir.path()));

        std::fs::create_dir(dir.path().join("onnx")).unwrap();
        std::fs::write(dir.path().join("onnx/model_quantized.onnx"), "test graph").unwrap();
        assert!(configured_reranker_present(dir.path()));
        assert!(RerankerHandle::if_present(dir.path().to_path_buf(), EpArg::Cpu).is_some());
    }
}
