//! Per-model embedding description.
//!
//! Rule: deduce what the artifact declares; declare what the artifact is silent about.
//!
//! - Graph facts (ONNX input names, KV geometry, `last_hidden_state` width) are
//!   read from the session at load. No descriptor fields for them.
//! - Silent facts (`pooling`, `uses_semantic_prefix`, query instruction, ONNX filename) come from
//!   `model_config.json`. Pooling and semantic-prefix mode are required;
//!   `query_instruction` is optional. Missing pooling is a hard error.
//!   A declared graph file that is not on disk is a hard error naming the path;
//!   never pick another file from the folder.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How a single window's token hidden states become one vector.
///
/// Not deducible from the ONNX graph. Quantized exports do not ship
/// `1_Pooling/config.json`. Required in `model_config.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolingStrategy {
    LastToken,
    Cls,
    Mean,
}

impl PoolingStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastToken => "last_token",
            Self::Cls => "cls",
            Self::Mean => "mean",
        }
    }

    fn parse(raw: &str, model_dir: &Path) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "last_token" | "last-token" => Ok(Self::LastToken),
            "cls" => Ok(Self::Cls),
            "mean" => Ok(Self::Mean),
            other => bail!(
                "model_config.json at {} has unknown pooling `{other}` \
                 (expected last_token | cls | mean)",
                model_dir.display()
            ),
        }
    }
}

/// KV-cache geometry deduced from ONNX input shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvGeometry {
    pub num_layers: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
}

/// Fields declared in `model_config.json` (what the artifact is silent about).
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredModelConfig {
    pub id: String,
    pub pooling: PoolingStrategy,
    pub normalize: bool,
    pub uses_semantic_prefix: bool,
    /// Optional publisher-declared instruction for query-only embedding.
    pub query_instruction: Option<String>,
    pub max_seq_tokens: usize,
    /// Legacy byte-window overlap retained for the non-tokenized fallback and
    /// older benchmark metadata. ONNX windowing uses `window_overlap_tokens`.
    pub window_overlap_bytes: usize,
    /// Overlap between tokenized ONNX windows. When absent, the legacy
    /// `window_overlap_bytes` value is used as a migration fallback.
    pub window_overlap_tokens: usize,
    pub onnx_graph: String,
    pub onnx_graph_fp32: Option<String>,
    pub tokenizer_file: String,
    /// Optional; when set, must match the graph's `last_hidden_state` width.
    pub dims: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ModelConfigFile {
    id: Option<String>,
    pooling: Option<String>,
    normalize: Option<bool>,
    uses_semantic_prefix: Option<bool>,
    query_instruction: Option<String>,
    max_seq_tokens: Option<usize>,
    window_overlap_bytes: Option<usize>,
    window_overlap_tokens: Option<usize>,
    onnx_graph: Option<String>,
    onnx_graph_fp32: Option<String>,
    tokenizer_file: Option<String>,
    dims: Option<usize>,
}

/// Completed descriptor: declared config plus facts read from the session.
#[derive(Debug, Clone)]
pub struct ModelDescriptor {
    pub id: String,
    pub dims: usize,
    pub max_seq_tokens: usize,
    pub pooling: PoolingStrategy,
    pub normalize: bool,
    pub uses_semantic_prefix: bool,
    pub query_instruction: Option<String>,
    pub has_kv_cache: bool,
    pub kv_geometry: Option<KvGeometry>,
    pub onnx_graph: String,
    pub tokenizer_file: String,
    pub window_overlap_bytes: usize,
    pub window_overlap_tokens: usize,
    pub model_dir: PathBuf,
}

pub const QWEN3_MODEL_CONFIG_JSON: &str = r#"{
  "id": "Qwen3-Embedding-0.6B",
  "pooling": "last_token",
  "normalize": true,
  "uses_semantic_prefix": true,
  "max_seq_tokens": 2560,
  "window_overlap_bytes": 256,
  "window_overlap_tokens": 256,
  "onnx_graph": "onnx/model_int8.onnx",
  "onnx_graph_fp32": "onnx/model.onnx",
  "tokenizer_file": "tokenizer.json",
  "dims": 1024
}
"#;

pub const BGE_SMALL_MODEL_CONFIG_JSON: &str = r#"{
  "id": "bge-small-en-v1.5",
  "pooling": "cls",
  "normalize": true,
  "uses_semantic_prefix": false,
  "query_instruction": "Represent this sentence for searching relevant passages: ",
  "max_seq_tokens": 512,
  "window_overlap_bytes": 82,
  "window_overlap_tokens": 82,
  "onnx_graph": "onnx/model_quantized.onnx",
  "tokenizer_file": "tokenizer.json",
  "dims": 384
}
"#;

impl DeclaredModelConfig {
    /// Load and validate `model_config.json` in `model_dir`. Does not open ONNX.
    pub fn load(model_dir: &Path) -> Result<Self> {
        let path = model_dir.join("model_config.json");
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "missing model_config.json in {} \
                 (pooling, uses_semantic_prefix, and onnx_graph must be declared)",
                model_dir.display()
            )
        })?;
        let parsed: ModelConfigFile =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

        let pooling = match parsed.pooling {
            Some(ref s) => PoolingStrategy::parse(s, model_dir)?,
            None => bail!(
                "model_config.json at {} is missing required field `pooling` \
                 (the artifact does not declare pooling; quantized ONNX exports \
                 do not ship 1_Pooling/config.json)",
                model_dir.display()
            ),
        };
        let uses_semantic_prefix = parsed.uses_semantic_prefix.ok_or_else(|| {
            anyhow::anyhow!(
                "model_config.json at {} is missing required field `uses_semantic_prefix`",
                model_dir.display()
            )
        })?;
        if uses_semantic_prefix && parsed.query_instruction.is_some() {
            bail!(
                "model_config.json at {} cannot combine `uses_semantic_prefix` with `query_instruction`",
                model_dir.display()
            );
        }
        let onnx_graph = parsed
            .onnx_graph
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "model_config.json at {} is missing required field `onnx_graph` \
                 (the graph filename is a publisher convention, not in the artifact)",
                    model_dir.display()
                )
            })?;

        let id = parsed
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                model_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_string())
            });

        let max_seq_tokens = match parsed.max_seq_tokens.filter(|&n| n > 0) {
            Some(n) => n,
            None => max_seq_from_tokenizer_config(model_dir).with_context(|| {
                format!(
                    "model_config.json at {} has no max_seq_tokens and \
                     tokenizer_config.json has no model_max_length",
                    model_dir.display()
                )
            })?,
        };

        let window_overlap_bytes = parsed
            .window_overlap_bytes
            .filter(|&n| n > 0)
            .unwrap_or(super::EMBED_WINDOW_OVERLAP_BYTES);
        let window_overlap_tokens = parsed
            .window_overlap_tokens
            .filter(|&n| n > 0)
            .unwrap_or(window_overlap_bytes);

        Ok(Self {
            id,
            pooling,
            normalize: parsed.normalize.unwrap_or(true),
            uses_semantic_prefix,
            query_instruction: parsed.query_instruction,
            max_seq_tokens,
            window_overlap_bytes,
            window_overlap_tokens,
            onnx_graph,
            onnx_graph_fp32: parsed.onnx_graph_fp32.filter(|s| !s.trim().is_empty()),
            tokenizer_file: parsed
                .tokenizer_file
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "tokenizer.json".to_string()),
            dims: parsed.dims.filter(|&n| n > 0),
        })
    }

    /// Relative graph path for the requested precision.
    pub fn graph_rel(&self, int8: bool) -> Result<&str> {
        if int8 {
            Ok(self.onnx_graph.as_str())
        } else {
            self.onnx_graph_fp32.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "model `{}` has no onnx_graph_fp32 in model_config.json \
                     (declared int8 graph is {})",
                    self.id,
                    self.onnx_graph
                )
            })
        }
    }

    /// Absolute path of the declared graph. Hard error if the file is missing;
    /// never searches the folder for a substitute.
    pub fn graph_path(&self, model_dir: &Path, int8: bool) -> Result<PathBuf> {
        let rel = self.graph_rel(int8)?;
        let path = model_dir.join(rel);
        if !path.is_file() {
            bail!(
                "declared ONNX graph not found: {} \
                 (model_config.json onnx_graph={rel} in {})",
                path.display(),
                model_dir.display()
            );
        }
        Ok(path)
    }

    pub fn tokenizer_path(&self, model_dir: &Path) -> Result<PathBuf> {
        let path = model_dir.join(&self.tokenizer_file);
        if !path.is_file() {
            bail!(
                "declared tokenizer not found: {} \
                 (model_config.json tokenizer_file={} in {})",
                path.display(),
                self.tokenizer_file,
                model_dir.display()
            );
        }
        Ok(path)
    }
}

/// UI/doctor-only check for a complete configured model bundle.
///
/// Download/ensure code must still perform its own remote verification; this
/// helper only answers whether the declared graph and tokenizer are present
/// and large enough to be useful.
pub fn configured_model_present(model_dir: &Path, int8: bool) -> bool {
    let Ok(declared) = DeclaredModelConfig::load(model_dir) else {
        return false;
    };
    let Ok(graph) = declared.graph_path(model_dir, int8) else {
        return false;
    };
    let Ok(tokenizer) = declared.tokenizer_path(model_dir) else {
        return false;
    };
    [graph, tokenizer].iter().all(|path| {
        std::fs::metadata(path)
            .map(|metadata| metadata.len() > 1024)
            .unwrap_or(false)
    })
}

impl ModelDescriptor {
    pub fn from_declared(
        declared: DeclaredModelConfig,
        model_dir: PathBuf,
        graph_rel: String,
        dims: usize,
        kv_geometry: Option<KvGeometry>,
    ) -> Result<Self> {
        if let Some(want) = declared.dims {
            if want != dims {
                bail!(
                    "model_config.json dims={want} does not match ONNX last_hidden_state \
                     width {dims} in {}",
                    model_dir.display()
                );
            }
        }
        let has_kv_cache = kv_geometry.is_some();
        Ok(Self {
            id: declared.id,
            dims,
            max_seq_tokens: declared.max_seq_tokens,
            pooling: declared.pooling,
            normalize: declared.normalize,
            uses_semantic_prefix: declared.uses_semantic_prefix,
            query_instruction: declared.query_instruction,
            has_kv_cache,
            kv_geometry,
            onnx_graph: graph_rel,
            tokenizer_file: declared.tokenizer_file,
            window_overlap_bytes: declared.window_overlap_bytes,
            window_overlap_tokens: declared.window_overlap_tokens,
            model_dir,
        })
    }
}

fn max_seq_from_tokenizer_config(model_dir: &Path) -> Result<usize> {
    let path = model_dir.join("tokenizer_config.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    v.get("model_max_length")
        .and_then(|x| x.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .ok_or_else(|| anyhow::anyhow!("{} has no positive model_max_length", path.display()))
}

/// Write `model_config.json` only if it is not already present.
pub fn write_model_config_if_missing(model_dir: &Path, json: &str) -> Result<()> {
    let path = model_dir.join("model_config.json");
    if path.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(model_dir)
        .with_context(|| format!("creating {}", model_dir.display()))?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_cfg(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("model_config.json"), body).unwrap();
    }

    #[test]
    fn missing_pooling_is_a_hard_error() {
        let tmp = tempdir().unwrap();
        write_cfg(
            tmp.path(),
            r#"{
              "id": "x",
              "uses_semantic_prefix": false,
              "onnx_graph": "onnx/model.onnx",
              "max_seq_tokens": 512
            }"#,
        );
        let err = DeclaredModelConfig::load(tmp.path()).expect_err("pooling is required");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("pooling") && msg.contains(&tmp.path().display().to_string()),
            "hard error must name the field and directory, got: {msg}"
        );
    }

    #[test]
    fn missing_uses_semantic_prefix_is_a_hard_error() {
        let tmp = tempdir().unwrap();
        write_cfg(
            tmp.path(),
            r#"{
              "id": "x",
              "pooling": "cls",
              "onnx_graph": "onnx/model.onnx",
              "max_seq_tokens": 512
            }"#,
        );
        let err = DeclaredModelConfig::load(tmp.path()).expect_err("flag is required");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("uses_semantic_prefix"),
            "hard error must name the field, got: {msg}"
        );
    }

    #[test]
    fn declared_graph_missing_on_disk_names_the_expected_path() {
        let tmp = tempdir().unwrap();
        write_cfg(tmp.path(), BGE_SMALL_MODEL_CONFIG_JSON);
        let declared = DeclaredModelConfig::load(tmp.path()).unwrap();
        let err = declared
            .graph_path(tmp.path(), true)
            .expect_err("file is not on disk");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("onnx/model_quantized.onnx")
                && msg.contains(&tmp.path().display().to_string()),
            "must name the declared path, not search the folder, got: {msg}"
        );
    }

    #[test]
    fn qwen3_template_parses_to_the_hardcoded_identity() {
        let tmp = tempdir().unwrap();
        write_cfg(tmp.path(), QWEN3_MODEL_CONFIG_JSON);
        let d = DeclaredModelConfig::load(tmp.path()).unwrap();
        assert_eq!(d.id, "Qwen3-Embedding-0.6B");
        assert_eq!(d.pooling, PoolingStrategy::LastToken);
        assert!(d.normalize);
        assert!(d.uses_semantic_prefix);
        assert!(d.query_instruction.is_none());
        assert_eq!(d.max_seq_tokens, 2560);
        assert_eq!(d.window_overlap_bytes, 256);
        assert_eq!(d.window_overlap_tokens, 256);
        assert_eq!(d.onnx_graph, "onnx/model_int8.onnx");
        assert_eq!(d.onnx_graph_fp32.as_deref(), Some("onnx/model.onnx"));
        assert_eq!(d.dims, Some(1024));
    }

    #[test]
    fn bge_template_declares_cls_and_quantized_graph() {
        let tmp = tempdir().unwrap();
        write_cfg(tmp.path(), BGE_SMALL_MODEL_CONFIG_JSON);
        let d = DeclaredModelConfig::load(tmp.path()).unwrap();
        assert_eq!(d.pooling, PoolingStrategy::Cls);
        assert!(!d.uses_semantic_prefix);
        assert_eq!(
            d.query_instruction.as_deref(),
            Some("Represent this sentence for searching relevant passages: ")
        );
        assert_eq!(d.max_seq_tokens, 512);
        assert_eq!(d.window_overlap_tokens, 82);
        assert_eq!(d.onnx_graph, "onnx/model_quantized.onnx");
        assert_eq!(d.dims, Some(384));
        assert!(d.onnx_graph_fp32.is_none());
    }

    #[test]
    fn configured_model_present_uses_declared_files_and_size_guard() {
        let tmp = tempdir().unwrap();
        write_cfg(tmp.path(), BGE_SMALL_MODEL_CONFIG_JSON);
        let payload = vec![0xAB; 2048];
        let graph = tmp.path().join("onnx/model_quantized.onnx");
        std::fs::create_dir_all(graph.parent().unwrap()).unwrap();
        std::fs::write(&graph, &payload).unwrap();
        std::fs::write(tmp.path().join("tokenizer.json"), &payload).unwrap();

        assert!(configured_model_present(tmp.path(), true));
        std::fs::write(&graph, [0u8; 1]).unwrap();
        assert!(!configured_model_present(tmp.path(), true));
        assert!(!configured_model_present(tmp.path(), false));
    }

    #[test]
    fn missing_config_file_names_the_directory() {
        let tmp = tempdir().unwrap();
        let err = DeclaredModelConfig::load(tmp.path()).expect_err("no json");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("model_config.json") && msg.contains(&tmp.path().display().to_string()),
            "got: {msg}"
        );
    }

    #[test]
    fn semantic_prefix_and_query_instruction_are_mutually_exclusive() {
        let tmp = tempdir().unwrap();
        write_cfg(
            tmp.path(),
            r#"{
              "id": "x",
              "pooling": "last_token",
              "uses_semantic_prefix": true,
              "query_instruction": "search: ",
              "onnx_graph": "onnx/model.onnx",
              "max_seq_tokens": 512
            }"#,
        );
        let err = DeclaredModelConfig::load(tmp.path()).expect_err("configuration is forbidden");
        let msg = format!("{err:#}");
        assert!(msg.contains("query_instruction"), "got: {msg}");
    }
}
