use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tokenizers::{
    PaddingDirection, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams,
    TruncationStrategy,
};

use crate::embed::model_descriptor::{
    DeclaredModelConfig, KvGeometry, ModelDescriptor, PoolingStrategy,
};

/// CLI-facing execution-provider selector for the ONNX backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EpArg {
    Cpu,
    Coreml,
    Directml,
}

/// ONNX (`ort`) embedding backend: a compiled session plus a tokenizer.
///
/// The session is built once per run and reused across batches.
/// Graph-specific facts (inputs, KV geometry, pooling) come from
/// [`ModelDescriptor`] — deduced from the session, declared in
/// `model_config.json`.
pub struct OnnxEmbedder {
    session: Session,
    tokenizer: Tokenizer,
    descriptor: ModelDescriptor,
}

impl OnnxEmbedder {
    /// Load the graph + tokenizer from `model_dir` (int8 graph by default).
    ///
    /// Returns the embedder plus the wall-clock load time in milliseconds.
    pub fn load(model_dir: &Path, ep: EpArg) -> Result<(Self, u128)> {
        let int8 = std::env::var("ORACLE_RS_ONNX_VARIANT")
            .map(|v| !v.eq_ignore_ascii_case("fp32"))
            .unwrap_or(true);
        Self::load_with_precision(model_dir, ep, int8)
    }

    /// Load using the declared int8 or fp32 graph from `model_config.json`.
    pub fn load_with_precision(model_dir: &Path, ep: EpArg, int8: bool) -> Result<(Self, u128)> {
        let start = Instant::now();
        let declared = DeclaredModelConfig::load(model_dir)?;
        let model_path = declared.graph_path(model_dir, int8)?;
        let tokenizer_path = declared.tokenizer_path(model_dir)?;
        let graph_rel = declared.graph_rel(int8)?.to_string();

        let session_builder =
            Session::builder().context("failed to create ONNX session builder")?;
        let mut builder = session_builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("failed to set ONNX optimization level: {e}"))?
            .with_intra_threads(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4),
            )
            .map_err(|e| anyhow::anyhow!("failed to set ONNX intra-op threads: {e}"))?;

        #[cfg(target_os = "macos")]
        {
            use ort::ep;
            if matches!(ep, EpArg::Coreml) {
                builder = builder
                    .with_execution_providers([ep::CoreML::default()
                        .with_model_format(ep::coreml::ModelFormat::MLProgram)
                        .build()])
                    .map_err(|e| {
                        anyhow::anyhow!("failed to register CoreML execution provider: {e}")
                    })?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        if matches!(ep, EpArg::Coreml) {
            anyhow::bail!("--ep coreml is only supported on macOS builds");
        }

        // Windows GPU via DirectML (any DX12 GPU). `with_execution_providers` uses
        // ort's default soft-fallback (error_on_failure = false): if the GPU EP
        // cannot register (no driver, device busy/absent), ort logs a warning and
        // silently proceeds on CPU — so this call effectively never errors for an
        // unavailable GPU, giving us "GPU when possible, else CPU" for free.
        #[cfg(target_os = "windows")]
        {
            use ort::ep;
            if matches!(ep, EpArg::Directml) {
                builder = builder
                    .with_execution_providers([ep::DirectML::default().build()])
                    .map_err(|e| {
                        anyhow::anyhow!("failed to register DirectML execution provider: {e}")
                    })?;
            }
        }
        #[cfg(not(target_os = "windows"))]
        if matches!(ep, EpArg::Directml) {
            anyhow::bail!("--ep directml is only supported on Windows builds");
        }

        let session = builder.commit_from_file(&model_path).with_context(|| {
            format!("failed to build ONNX session from {}", model_path.display())
        })?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to load tokenizer from {}: {e}",
                tokenizer_path.display()
            )
        })?;

        let dims = dims_from_session(&session).with_context(|| {
            format!(
                "reading last_hidden_state width from {}",
                model_path.display()
            )
        })?;
        let kv_geometry = kv_geometry_from_session(&session)?;
        let descriptor = ModelDescriptor::from_declared(
            declared,
            model_dir.to_path_buf(),
            graph_rel,
            dims,
            kv_geometry,
        )?;

        let embedder = OnnxEmbedder {
            session,
            tokenizer,
            descriptor,
        };
        Ok((embedder, start.elapsed().as_millis()))
    }

    pub fn descriptor(&self) -> &ModelDescriptor {
        &self.descriptor
    }

    /// Embed `texts` in chunks of `batch_size`.
    ///
    /// Pooling is last-real-token (right padding), matching the candle path,
    /// and each vector is L2-normalized.
    ///
    /// Long texts are split into overlapping byte windows (see
    /// [`crate::embed::window_text`]), each embedded, then mean-pooled so the
    /// public API stays 1:1 with no text dropped. Tokenizer truncation is kept
    /// as a safety net at [`crate::embed::resolve_embed_max_seq_tokens`] but
    /// must never fire for well-formed windows (`n_tokens ≤ n_bytes` + specials).
    /// Forward passes are split so `sub_batch.len() × seq_len²` never exceeds
    /// the attention budget (token estimate for packing = window byte length).
    ///
    /// `cancel` is checked between groups and sub-batches (same granularity as
    /// the candle path).
    pub fn embed_batched(
        &mut self,
        texts: &[String],
        batch_size: usize,
        cancel: &crate::embed::CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let batch_size = batch_size.max(1);
        let max_seq = max_seq_tokens_for(&self.descriptor);
        let budget = crate::embed::resolve_attention_budget();
        let window_bytes = {
            let requested = std::env::var("ORACLE_EMBED_WINDOW_BYTES")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok());
            crate::embed::effective_embed_window_bytes(requested, max_seq).0
        };
        let overlap = std::env::var("ORACLE_EMBED_WINDOW_OVERLAP_BYTES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(self.descriptor.window_overlap_bytes);

        let (windows, counts) = crate::embed::expand_texts_to_windows(texts, window_bytes, overlap);
        let window_lens: Vec<usize> = windows.iter().map(|w| w.len()).collect();
        let groups = crate::embed::pack_windows_for_attention(&window_lens, budget);

        // Right-pad to the longest sequence in each batch; hard-cap sequence length
        // as a safety net (windows should already be ≤ max_seq tokens).
        self.tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            ..Default::default()
        }));
        self.tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_seq,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("failed to configure tokenizer truncation: {e}"))?;

        let mut window_out: Vec<Vec<f32>> = Vec::with_capacity(windows.len());
        for group in groups {
            if cancel.is_cancelled() {
                anyhow::bail!("embedding cancelled after {} windows", window_out.len());
            }
            let group_windows = &windows[group];
            for chunk in group_windows.chunks(batch_size) {
                if cancel.is_cancelled() {
                    anyhow::bail!("embedding cancelled after {} windows", window_out.len());
                }
                self.embed_tokenized_attention_safe(chunk, budget, cancel, &mut window_out)?;
            }
        }

        Ok(crate::embed::pool_window_vectors(&window_out, &counts))
    }

    /// Tokenize `texts`, then forward in sub-batches that satisfy the attention budget.
    ///
    /// This is the authoritative gate: caller's char-based estimate can be wrong;
    /// after tokenization the true padded `seq_len` is known and must not be
    /// violated by a forward pass.
    fn embed_tokenized_attention_safe(
        &mut self,
        texts: &[String],
        budget: usize,
        cancel: &crate::embed::CancelFlag,
        out: &mut Vec<Vec<f32>>,
    ) -> Result<()> {
        if texts.is_empty() {
            return Ok(());
        }
        if cancel.is_cancelled() {
            anyhow::bail!("embedding cancelled after {} windows", out.len());
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let seq_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0)
            .max(1);
        let max_n = crate::embed::max_batch_for_attention(seq_len, budget);

        if texts.len() <= max_n {
            self.forward_encodings(&encodings, seq_len, budget, out)?;
            return Ok(());
        }

        // Re-tokenize smaller groups: sub-batches may pad to a shorter seq_len.
        for sub in texts.chunks(max_n) {
            if cancel.is_cancelled() {
                anyhow::bail!("embedding cancelled after {} windows", out.len());
            }
            self.embed_tokenized_attention_safe(sub, budget, cancel, out)?;
        }
        Ok(())
    }

    fn forward_encodings(
        &mut self,
        encodings: &[tokenizers::Encoding],
        seq_len: usize,
        budget: usize,
        out: &mut Vec<Vec<f32>>,
    ) -> Result<()> {
        let batch = encodings.len();
        // Release-safe attention gate at the forward pass: subdivide rather
        // than assert. `batch == 1` is irreducible (still attempted; bounded
        // by the window / max_seq cap).
        if batch > 1 && crate::embed::attention_cost(batch, seq_len) > budget {
            let sizes = crate::embed::attention_sub_batch_sizes(batch, seq_len, budget);
            let mut off = 0usize;
            for size in sizes {
                let sub = &encodings[off..off + size];
                let sub_seq = sub
                    .iter()
                    .map(|e| e.get_ids().len())
                    .max()
                    .unwrap_or(0)
                    .max(1);
                self.forward_encodings(sub, sub_seq, budget, out)?;
                off += size;
            }
            return Ok(());
        }

        let mut ids_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut mask_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut pos_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);

        for enc in encodings {
            let ids = enc.get_ids();
            let attn = enc.get_attention_mask();
            for j in 0..seq_len {
                let id = if j < ids.len() { ids[j] as i64 } else { 0 };
                let m = if j < attn.len() { attn[j] as i64 } else { 0 };
                ids_vec.push(id);
                mask_vec.push(m);
                pos_vec.push(j as i64);
            }
        }

        let mut ids_vec = Some(ids_vec);
        let mut mask_vec = Some(mask_vec);
        let mut pos_vec = Some(pos_vec);
        let mut run_inputs: Vec<(
            std::borrow::Cow<'static, str>,
            ort::session::SessionInputValue<'static>,
        )> = Vec::with_capacity(self.session.inputs().len());
        for outlet in self.session.inputs() {
            let name = outlet.name();
            let value = if name == "input_ids" {
                let ids = ids_vec
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("ONNX graph has duplicate input_ids inputs"))?;
                Tensor::from_array(([batch, seq_len], ids.into_boxed_slice()))?.into()
            } else {
                self.named_input(name, batch, seq_len, &mut mask_vec, &mut pos_vec)?
            };
            run_inputs.push((name.to_string().into(), value));
        }
        let outputs = self
            .session
            .run(run_inputs)
            .context("ONNX session run failed")?;

        let (shape, data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .context("failed to extract last_hidden_state tensor")?;
        let seq = shape[1] as usize;
        let hidden = shape[2] as usize;

        for (row, encoding) in encodings.iter().enumerate().take(batch) {
            let mask = encoding.get_attention_mask();
            let vec = pool_hidden(
                self.descriptor.pooling,
                data,
                row,
                seq,
                hidden,
                mask,
                self.descriptor.normalize,
            )?;
            out.push(vec);
        }
        Ok(())
    }

    fn named_input(
        &self,
        name: &str,
        batch: usize,
        seq_len: usize,
        mask_vec: &mut Option<Vec<i64>>,
        pos_vec: &mut Option<Vec<i64>>,
    ) -> Result<ort::session::SessionInputValue<'static>> {
        if name == "attention_mask" {
            let mask = mask_vec
                .take()
                .ok_or_else(|| anyhow::anyhow!("ONNX graph has duplicate attention_mask inputs"))?;
            let t = Tensor::from_array(([batch, seq_len], mask.into_boxed_slice()))?;
            return Ok(t.into());
        }
        if name == "token_type_ids" {
            let zeros = vec![0i64; batch * seq_len];
            let t = Tensor::from_array(([batch, seq_len], zeros.into_boxed_slice()))?;
            return Ok(t.into());
        }
        if name == "position_ids" {
            let pos = pos_vec
                .take()
                .ok_or_else(|| anyhow::anyhow!("ONNX graph has duplicate position_ids inputs"))?;
            let t = Tensor::from_array(([batch, seq_len], pos.into_boxed_slice()))?;
            return Ok(t.into());
        }
        if name.contains("past_key_values") {
            let geo = self.descriptor.kv_geometry.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "graph asks for `{name}` but KV geometry was not deduced from the session"
                )
            })?;
            let empty_kv = Tensor::<f32>::new(
                self.session.allocator(),
                [
                    batch as i64,
                    geo.num_kv_heads as i64,
                    0,
                    geo.head_dim as i64,
                ],
            )
            .context("failed to allocate empty KV-cache tensor")?;
            return Ok(empty_kv.into());
        }
        bail!(
            "unhandled ONNX input `{name}` on model {} (feed-by-name: no rule for this tensor)",
            self.descriptor.id
        )
    }
}

fn max_seq_tokens_for(desc: &ModelDescriptor) -> usize {
    std::env::var("ORACLE_EMBED_MAX_SEQ_TOKENS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(desc.max_seq_tokens)
}

fn dims_from_session(session: &Session) -> Result<usize> {
    let outlet = session
        .outputs()
        .iter()
        .find(|o| o.name() == "last_hidden_state")
        .ok_or_else(|| anyhow::anyhow!("ONNX graph has no output named last_hidden_state"))?;
    let shape = outlet
        .dtype()
        .tensor_shape()
        .ok_or_else(|| anyhow::anyhow!("last_hidden_state is not a tensor"))?;
    shape
        .iter()
        .copied()
        .rev()
        .find(|&d| d > 0)
        .map(|d| d as usize)
        .ok_or_else(|| {
            anyhow::anyhow!("last_hidden_state has no static hidden dim (shape {shape:?})")
        })
}

fn kv_geometry_from_session(session: &Session) -> Result<Option<KvGeometry>> {
    let keys: Vec<_> = session
        .inputs()
        .iter()
        .filter(|o| o.name().starts_with("past_key_values.") && o.name().ends_with(".key"))
        .collect();
    if keys.is_empty() {
        return Ok(None);
    }
    let shape = keys[0]
        .dtype()
        .tensor_shape()
        .ok_or_else(|| anyhow::anyhow!("past_key_values.*.key is not a tensor"))?;
    let static_dims: Vec<i64> = shape.iter().copied().filter(|&d| d > 0).collect();
    if static_dims.len() < 2 {
        bail!(
            "cannot deduce KV geometry from {}: need two static dims (heads, head_dim), got {shape:?}",
            keys[0].name()
        );
    }
    // The KV shape is [batch, heads, past_len, head_dim]. The first static
    // dimensions can include a fixed batch from a traced export; the last two
    // always identify the cache geometry we need.
    let (num_kv_heads, head_dim) = (
        static_dims[static_dims.len() - 2],
        static_dims[static_dims.len() - 1],
    );
    Ok(Some(KvGeometry {
        num_layers: keys.len(),
        num_kv_heads: num_kv_heads as usize,
        head_dim: head_dim as usize,
    }))
}

fn pool_hidden(
    strategy: PoolingStrategy,
    data: &[f32],
    row: usize,
    seq: usize,
    hidden: usize,
    mask: &[u32],
    normalize: bool,
) -> Result<Vec<f32>> {
    let mask_sum: i64 = mask.iter().map(|&x| x as i64).sum();
    let mut vec = match strategy {
        PoolingStrategy::LastToken => {
            // With right padding the last real token is just before the pad run.
            let real_last = (mask_sum - 1) as usize;
            let base = (row * seq + real_last) * hidden;
            data.get(base..base + hidden)
                .ok_or_else(|| anyhow::anyhow!("last-token pool out of range"))?
                .to_vec()
        }
        PoolingStrategy::Cls => {
            let base = row * seq * hidden;
            data.get(base..base + hidden)
                .ok_or_else(|| anyhow::anyhow!("cls pool out of range"))?
                .to_vec()
        }
        PoolingStrategy::Mean => {
            let mut acc = vec![0.0f32; hidden];
            let mut n = 0.0f32;
            for (t, &m) in mask.iter().enumerate() {
                if m == 0 {
                    continue;
                }
                let base = (row * seq + t) * hidden;
                let token = data
                    .get(base..base + hidden)
                    .ok_or_else(|| anyhow::anyhow!("mean pool out of range at token {t}"))?;
                for (a, &x) in acc.iter_mut().zip(token) {
                    *a += x;
                }
                n += 1.0;
            }
            let denom = n.max(1.0);
            for x in acc.iter_mut() {
                *x /= denom;
            }
            acc
        }
    };
    if normalize {
        let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt() + 1e-12;
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
    Ok(vec)
}
