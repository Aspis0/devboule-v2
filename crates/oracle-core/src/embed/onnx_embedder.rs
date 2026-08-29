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
    special_token_count: usize,
}

/// Extra headroom used when a token window is cut from a larger encoding.
/// Re-encoding a slice can produce a few boundary tokens that were merged in
/// the full encoding, so leave room for those tokens before the model limit.
const TOKEN_WINDOW_MARGIN: usize = 8;

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
        let special_token_count = tokenizer
            .encode("", true)
            .map_err(|e| anyhow::anyhow!("failed to measure tokenizer special tokens: {e}"))?
            .get_ids()
            .len();
        let max_seq = max_seq_tokens_for(&descriptor);
        if special_token_count >= max_seq {
            bail!(
                "model {} has {} special tokens but max_seq_tokens is {}",
                descriptor.id,
                special_token_count,
                max_seq
            );
        }

        let embedder = OnnxEmbedder {
            session,
            tokenizer,
            descriptor,
            special_token_count,
        };
        Ok((embedder, start.elapsed().as_millis()))
    }

    pub fn descriptor(&self) -> &ModelDescriptor {
        &self.descriptor
    }

    /// Exact number of special tokens added by this model tokenizer when
    /// `add_special_tokens` is enabled.
    pub fn special_token_count(&self) -> usize {
        self.special_token_count
    }

    /// Effective sequence cap, including the process-wide override.
    pub fn max_seq_tokens(&self) -> usize {
        max_seq_tokens_for(&self.descriptor)
    }

    /// Embed `texts` in chunks of `batch_size`.
    ///
    /// Pooling is last-real-token (right padding), matching the candle path,
    /// and each vector is L2-normalized.
    ///
    /// Long texts are split into overlapping token windows using the loaded
    /// tokenizer's offsets, each embedded, then mean-pooled so the public API
    /// stays 1:1 with no text dropped. Tokenizer truncation remains a safety
    /// net, but the token planner leaves an eight-token margin so it should
    /// never fire for a well-formed window. Forward passes are split so
    /// `sub_batch.len() × seq_len²` never exceeds the attention budget; packing
    /// uses the real token counts, not byte lengths.
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
        // `Tokenizer` keeps truncation configuration between calls. Clear the
        // forward-pass safety cap before planning, otherwise the second and
        // later embed_batched calls could hide long texts from this planner.
        self.tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow::anyhow!("failed to clear tokenizer truncation: {e}"))?;
        let (windows, counts, window_token_lens) =
            self.expand_texts_to_token_windows(texts, max_seq)?;
        let groups = crate::embed::pack_windows_for_attention(&window_token_lens, budget);

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

    /// Return the planned window count and the number whose re-encoded form
    /// would still exceed `max_seq`. This is intentionally public for the
    /// benchmark/reporting path; production embedding uses the same planner.
    pub fn token_window_stats(&mut self, texts: &[String]) -> Result<(usize, usize)> {
        self.tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow::anyhow!("failed to clear tokenizer truncation: {e}"))?;
        let max_seq = max_seq_tokens_for(&self.descriptor);
        let (windows, _counts, token_lens) = self.expand_texts_to_token_windows(texts, max_seq)?;
        Ok((
            windows.len(),
            token_lens.iter().filter(|&&n| n >= max_seq).count(),
        ))
    }

    /// Tokenize once without special tokens, then derive UTF-8-safe text
    /// slices from the tokenizer offsets. Short texts are kept byte-for-byte
    /// intact so the common one-window path is unchanged.
    fn expand_texts_to_token_windows(
        &self,
        texts: &[String],
        max_seq: usize,
    ) -> Result<(Vec<String>, Vec<usize>, Vec<usize>)> {
        let mut windows = Vec::new();
        let mut counts = Vec::with_capacity(texts.len());
        let mut token_lens = Vec::new();
        let overlap = crate::embed::resolve_embed_window_overlap_tokens(
            self.descriptor.window_overlap_tokens,
        );

        for text in texts {
            let encoding = self
                .tokenizer
                .encode(text.as_str(), false)
                .map_err(|e| anyhow::anyhow!("failed to tokenize text for windowing: {e}"))?;
            let n_tokens = encoding.get_ids().len();
            // Leave equality to the split path: a sequence that exactly
            // touches max_seq is treated as truncation-risk by the benchmark
            // contract, even though the tokenizer's hard cap would retain it.
            if n_tokens + self.special_token_count < max_seq {
                windows.push(text.clone());
                counts.push(1);
                token_lens.push((n_tokens + self.special_token_count).max(1));
                continue;
            }

            let capacity = max_seq
                .saturating_sub(self.special_token_count)
                .saturating_sub(TOKEN_WINDOW_MARGIN)
                .max(1);
            let overlap = overlap.min(capacity.saturating_sub(1));
            let offsets = encoding.get_offsets();
            if offsets.len() != n_tokens {
                bail!(
                    "tokenizer returned {} offsets for {} tokens while windowing model {}",
                    offsets.len(),
                    n_tokens,
                    self.descriptor.id
                );
            }
            let mut start = 0usize;
            let mut per_text = 0usize;
            while start < n_tokens {
                let mut end = (start + capacity).min(n_tokens);
                let (window_text, actual_tokens) = loop {
                    let start_byte = offsets[start].0;
                    let end_byte = offsets[end - 1].1;
                    if start_byte >= end_byte
                        || end_byte > text.len()
                        || !text.is_char_boundary(start_byte)
                        || !text.is_char_boundary(end_byte)
                    {
                        bail!(
                            "tokenizer returned invalid UTF-8 offsets [{start_byte}, {end_byte}) for model {}",
                            self.descriptor.id
                        );
                    }
                    let window_text = text[start_byte..end_byte].to_string();
                    let actual_tokens = self
                        .tokenizer
                        .encode(window_text.as_str(), true)
                        .map_err(|e| anyhow::anyhow!("failed to validate token window: {e}"))?
                        .get_ids()
                        .len();
                    if actual_tokens < max_seq || end == start + 1 {
                        break (window_text, actual_tokens);
                    }
                    // BPE/WordPiece can expose a few extra boundary tokens
                    // after slicing. Skip the number of token positions that
                    // are known to be over the limit; the normal path still
                    // uses the nominal eight-token margin.
                    let excess = actual_tokens.saturating_sub(max_seq).saturating_add(1);
                    end = end.saturating_sub(excess).max(start + 1);
                };
                if actual_tokens >= max_seq {
                    bail!(
                        "cannot fit a token window below max_seq_tokens={} for model {}",
                        max_seq,
                        self.descriptor.id
                    );
                }
                windows.push(window_text);
                token_lens.push(actual_tokens.max(1));
                per_text += 1;
                if end == n_tokens {
                    break;
                }
                start = end.saturating_sub(overlap).max(start + 1);
            }
            counts.push(per_text.max(1));
        }

        Ok((windows, counts, token_lens))
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
