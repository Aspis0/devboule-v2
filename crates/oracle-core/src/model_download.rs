//! Downloads ONNX embedding bundles for the `ort` backend. Oracle's portable
//! default is the BGE-small bundle (about 34 MB).
//!
//! This is the Rust engine's replacement for the Python venv + pip + warmup
//! flow: instead of installing a Python runtime that pulls the model into the
//! HF cache, we fetch the ONNX export directly into the oracle-data tree at the
//! layout `OrtEmbedder::load` expects:
//!   <oracle_data_root>/models/bge-small-en-v1.5/onnx/model_quantized.onnx
//!   <oracle_data_root>/models/bge-small-en-v1.5/tokenizer.json
//!
//! Downloads stream to a `.part` file and are atomically renamed on success, so
//! an interrupted download never leaves a truncated file that looks complete.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::embed::model_descriptor::{write_model_config_if_missing, BGE_SMALL_MODEL_CONFIG_JSON};
use crate::embed::ort_backend::OrtEmbedder;
use crate::embed::CancelFlag;

/// The model shipped as Oracle's portable default.
pub const BGE_SMALL_MODEL_ID: &str = "bge-small-en-v1.5";

/// Approximate complete package size used in UI/doctor messages. The actual
/// progress uses the server-reported Content-Length for each file.
pub const BGE_SMALL_APPROX_BYTES: u64 = 34_000_000;

/// Hard ceiling on a single downloaded file. The BGE bundle is much smaller;
/// anything unexpectedly larger means a wrong or hostile server, and we must
/// not fill the disk. This also covers servers that omit Content-Length.
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024; // 8 GiB

/// A single HEAD/GET may transfer a large model, but it must never be able to
/// keep startup blocked forever. `read_timeout` is an idle timeout between
/// body chunks; `timeout` is the cap for the whole request.
const MODEL_READ_TIMEOUT: Duration = Duration::from_secs(30);
const MODEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Progress for a single file within the bundle.
#[derive(Debug, Clone)]
pub struct FileProgress {
    /// Repo-relative path currently transferring (e.g. `onnx/model.onnx_data`).
    pub file: String,
    /// 1-based index of this file in the bundle.
    pub index: usize,
    /// Total files in the bundle.
    pub total_files: usize,
    /// Bytes written so far for this file.
    pub bytes_done: u64,
    /// Total bytes for this file (from Content-Length), or `None` if unknown.
    pub bytes_total: Option<u64>,
}

/// The on-disk model directory for the given oracle-data root.
pub fn model_dir(oracle_data_root: &Path) -> PathBuf {
    OrtEmbedder::default_model_dir(oracle_data_root, BGE_SMALL_MODEL_ID)
}

/// The on-disk directory for an explicitly configured model id.
pub fn model_dir_for(oracle_data_root: &Path, model_id: &str) -> PathBuf {
    OrtEmbedder::default_model_dir(oracle_data_root, model_id)
}

/// Effective per-file byte cap for a download. When the remote length IS
/// known (and non-zero) it is honored only up to the hard
/// [`MAX_DOWNLOAD_BYTES`] ceiling — an announced length above the ceiling is
/// clamped, so a hostile server announcing a huge Content-Length can never
/// raise the cap above the disk-safety bound. When the length is unknown
/// (HF's HEAD quirk) the cap is the hard [`MAX_DOWNLOAD_BYTES`] ceiling.
fn effective_download_cap(bytes_total: Option<u64>) -> u64 {
    bytes_total
        .filter(|&t| t > 0)
        .map(|t| t.min(MAX_DOWNLOAD_BYTES))
        .unwrap_or(MAX_DOWNLOAD_BYTES)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // Large weights on a slow link: allow a generous but finite request
        // window, while the shorter read timeout catches a stalled server.
        .timeout(MODEL_REQUEST_TIMEOUT)
        .read_timeout(MODEL_READ_TIMEOUT)
        .connect_timeout(Duration::from_secs(30))
        // Allow cross-host redirects (HF resolve URLs legitimately redirect to
        // a CDN) but refuse any non-HTTPS hop — HTTPS→HTTP downgrade would
        // enable MITM model injection / cleartext model delivery.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 10 {
                return attempt.error("too many redirects");
            }
            if attempt.url().scheme() != "https" {
                return attempt.error("refusing non-https redirect for model download");
            }
            attempt.follow()
        }))
        .build()
        .context("building HTTP client for model download")
}

/// Remote size via a HEAD request, or `None` when the server omits it.
async fn remote_len(client: &reqwest::Client, url: &str) -> Result<Option<u64>> {
    let resp = match client.head(url).send().await {
        Ok(resp) => resp,
        Err(error) if error.is_timeout() => {
            bail!(
                "checking the remote size for model file {url} timed out after {} seconds",
                MODEL_REQUEST_TIMEOUT.as_secs()
            );
        }
        Err(_) => return Ok(None),
    };
    if !resp.status().is_success() {
        return Ok(None);
    }
    // Some CDNs (HuggingFace) report Content-Length: 0 on HEAD requests for
    // redirect-to-CDN URLs. A 0 length is not a real model size — treat it as
    // "unknown" so the caller skips the exact-size guard instead of rejecting
    // a fully-downloaded file.
    Ok(resp.content_length().filter(|&len| len > 0))
}

/// Stream one file to `dest`, calling `progress` as bytes arrive. Writes to a
/// sibling `.part` file and renames on success (atomic within the same dir).
async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    bytes_total: Option<u64>,
    cancel: &CancelFlag,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("model download cancelled");
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let part = dest.with_extension(match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_string(),
    });

    let mut resp = match client.get(url).send().await {
        Ok(resp) => resp,
        Err(error) if error.is_timeout() => {
            bail!(
                "request for model file {} timed out after {} seconds: {error}",
                dest.display(),
                MODEL_REQUEST_TIMEOUT.as_secs()
            );
        }
        Err(error) => return Err(error).with_context(|| format!("GET {url}")),
    };
    if !resp.status().is_success() {
        bail!("GET {url} -> HTTP {}", resp.status());
    }

    let mut file =
        std::fs::File::create(&part).with_context(|| format!("creating {}", part.display()))?;
    let mut done: u64 = 0;
    let cap = effective_download_cap(bytes_total);
    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&part);
            bail!("model download cancelled");
        }
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                let _ = std::fs::remove_file(&part);
                if error.is_timeout() {
                    bail!(
                        "reading model download for {} timed out ({} seconds idle or {} seconds total): {error}",
                        dest.display(),
                        MODEL_READ_TIMEOUT.as_secs(),
                        MODEL_REQUEST_TIMEOUT.as_secs()
                    );
                }
                return Err(error)
                    .with_context(|| format!("reading download stream for {}", dest.display()));
            }
        };
        file.write_all(&chunk).context("writing model file")?;
        done += chunk.len() as u64;
        if done > cap {
            let _ = std::fs::remove_file(&part);
            bail!(
                "download of {} exceeded the {} byte cap (got {done} bytes) — refusing to write more",
                dest.display(),
                cap
            );
        }
        progress(done);
    }
    if cancel.is_cancelled() {
        let _ = std::fs::remove_file(&part);
        bail!("model download cancelled");
    }
    file.flush().ok();
    drop(file);

    if let Some(expected) = bytes_total {
        if expected > 0 {
            let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if got != expected {
                let _ = std::fs::remove_file(&part);
                bail!(
                    "size mismatch for {}: got {got} bytes, expected {expected}",
                    dest.display()
                );
            }
        }
    }

    std::fs::rename(&part, dest).with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

/// Clear a broken model-dir symlink so downloads can create a real directory.
/// Real directories and working symlinks are left alone.
///
/// Public so the app installer can clear a dangling link before seeding a
/// bundled ONNX tree into the writable data dir.
pub fn clear_broken_model_dir_symlink(dir: &Path) -> Result<()> {
    let meta = match dir.symlink_metadata() {
        Ok(m) => m,
        Err(_) => return Ok(()), // nothing there
    };
    if meta.file_type().is_symlink() && !dir.exists() {
        std::fs::remove_file(dir)
            .with_context(|| format!("removing broken model-dir symlink {}", dir.display()))?;
    }
    Ok(())
}

/// Download the declared files of any ONNX bundle under
/// `<oracle_data_root>/models/<model_id>/`. `hf_resolve_base` is the HuggingFace
/// resolve URL *without* a trailing slash (e.g. `https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main`).
fn ensure_bundle_onnx_with_cancel(
    oracle_data_root: &Path,
    model_id: &str,
    hf_resolve_base: &str,
    files: &[&str],
    cancel: &CancelFlag,
    progress: impl FnMut(FileProgress),
) -> Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("creating the model download runtime")?;
    runtime.block_on(ensure_bundle_onnx_async(
        oracle_data_root,
        model_id,
        hf_resolve_base,
        files,
        cancel,
        progress,
    ))
}

async fn ensure_bundle_onnx_async(
    oracle_data_root: &Path,
    model_id: &str,
    hf_resolve_base: &str,
    files: &[&str],
    cancel: &CancelFlag,
    mut progress: impl FnMut(FileProgress),
) -> Result<PathBuf> {
    let dir = OrtEmbedder::model_dir(oracle_data_root, model_id);
    clear_broken_model_dir_symlink(&dir)?;
    let client = http_client()?;
    let base = hf_resolve_base.trim_end_matches('/');

    for (i, rel) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            bail!("model download cancelled");
        }
        let url = format!("{base}/{rel}");
        let dest = dir.join(rel);
        let bytes_total = remote_len(&client, &url).await?;

        if let Some(expected) = bytes_total {
            if std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) == expected && expected > 0 {
                progress(FileProgress {
                    file: (*rel).to_string(),
                    index: i + 1,
                    total_files: files.len(),
                    bytes_done: expected,
                    bytes_total: Some(expected),
                });
                continue;
            }
        }

        let rel_owned = (*rel).to_string();
        let files_len = files.len();
        download_file(&client, &url, &dest, bytes_total, cancel, |done| {
            progress(FileProgress {
                file: rel_owned.clone(),
                index: i + 1,
                total_files: files_len,
                bytes_done: done,
                bytes_total,
            });
        })
        .await
        .with_context(|| format!("downloading {rel}"))?;
    }

    if cancel.is_cancelled() {
        bail!("model download cancelled");
    }
    Ok(dir)
}

/// HuggingFace resolve base for the Xenova bge-small quantized export.
pub const BGE_SMALL_HF_BASE: &str = "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main";

pub const BGE_SMALL_FILES: &[&str] = &["onnx/model_quantized.onnx", "tokenizer.json"];

pub fn ensure_bge_small_onnx(
    oracle_data_root: &Path,
    progress: impl FnMut(FileProgress),
) -> Result<PathBuf> {
    ensure_bge_small_onnx_with_cancel(oracle_data_root, &CancelFlag::new(), progress)
}

/// Cancellable installer for Oracle's default BGE bundle.
pub fn ensure_bge_small_onnx_with_cancel(
    oracle_data_root: &Path,
    cancel: &CancelFlag,
    progress: impl FnMut(FileProgress),
) -> Result<PathBuf> {
    let dir = ensure_bundle_onnx_with_cancel(
        oracle_data_root,
        BGE_SMALL_MODEL_ID,
        BGE_SMALL_HF_BASE,
        BGE_SMALL_FILES,
        cancel,
        progress,
    )?;
    if cancel.is_cancelled() {
        bail!("model download cancelled");
    }
    write_model_config_if_missing(&dir, BGE_SMALL_MODEL_CONFIG_JSON)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dir_layout_matches_ort_backend() {
        let root = Path::new("/tmp/oracle-data");
        assert_eq!(
            model_dir(root),
            root.join("models").join("bge-small-en-v1.5"),
            "must match OrtEmbedder::default_model_dir so the backend finds it"
        );
    }

    #[test]
    fn cancellable_ensure_stops_before_network_when_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let cancel = CancelFlag::new();
        cancel.cancel();
        let error = ensure_bge_small_onnx_with_cancel(tmp.path(), &cancel, |_| {})
            .expect_err("cancelled ensure must not start a download");
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn clear_broken_model_dir_symlink_removes_dangling_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = model_dir(tmp.path());
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/nonexistent/oracle-bge-target", &dir).unwrap();
            assert!(dir.symlink_metadata().unwrap().file_type().is_symlink());
            assert!(!dir.exists());
            clear_broken_model_dir_symlink(&dir).unwrap();
            assert!(
                !dir.symlink_metadata().is_ok()
                    || !dir.symlink_metadata().unwrap().file_type().is_symlink()
            );
            // After clear, a real dir can be created.
            std::fs::create_dir_all(&dir).unwrap();
            assert!(dir.is_dir());
        }
    }

    #[test]
    fn effective_download_cap_uses_remote_len_or_hard_ceiling() {
        assert_eq!(effective_download_cap(None), MAX_DOWNLOAD_BYTES);
        assert_eq!(effective_download_cap(Some(0)), MAX_DOWNLOAD_BYTES);
        assert_eq!(effective_download_cap(Some(1234)), 1234);
        assert_eq!(effective_download_cap(Some(u64::MAX)), MAX_DOWNLOAD_BYTES);
        assert!(effective_download_cap(Some(MAX_DOWNLOAD_BYTES + 1)) <= MAX_DOWNLOAD_BYTES);
    }
}
