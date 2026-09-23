//! SPIKE ONLY — this branch never merges.
//! Child webviews + in-process (no debug port) driving of the child's page.
//!
//! Threading: every command that touches WebView2 COM (`webview_call`) is an
//! `async fn`, so it runs on a tokio worker, never on the main thread.
//! `with_webview` from a non-main thread posts the closure to the event loop;
//! the closure runs on the main (UI) thread — where the ICoreWebView2 was
//! created — fires the async COM call, and the completion handler sends the
//! result over an mpsc channel that the tokio worker blocks on. Blocking the
//! main thread instead would deadlock (the completion needs the main pump).

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, Wry};
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{
  CallDevToolsProtocolMethodCompletedHandler, ExecuteScriptCompletedHandler,
};
use windows::core::HSTRING;

pub const CHILD_A: &str = "spike-child";
pub const CHILD_B: &str = "spike-child-b";
const PAGE1: &str = "http://127.0.0.1:8123/page1.html";

/// Worktree `spike/` directory (compile-time path; built inside this worktree).
fn spike_dir() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("spike")
}

/// Last command id we handed to the frontend poller.
pub struct SpikeCmdLast(pub std::sync::Mutex<i64>);

/// Called from `setup` on the main thread before the event loop runs:
/// `add_child` executes inline (same thread), wry pumps while WebView2 boots.
pub fn setup(app: &mut tauri::App) -> tauri::Result<()> {
  let _ = std::fs::create_dir_all(spike_dir());
  let window = app.get_window("main").expect("main window");

  let child_a = tauri::webview::WebviewBuilder::new(
    CHILD_A,
    WebviewUrl::External(PAGE1.parse().expect("page1 url")),
  );
  window.add_child(
    child_a,
    LogicalPosition::new(660.0, 60.0),
    LogicalSize::new(560.0, 360.0),
  )?;

  // (d): a second child with its OWN data_directory, only when asked, so the
  // (a) launches measure exactly one child.
  if std::env::var("SPIKE_CHILD_B").is_ok() {
    let dir = app
      .path()
      .app_local_data_dir()?
      .join("spike-child-b-profile");
    let child_b = tauri::webview::WebviewBuilder::new(
      CHILD_B,
      WebviewUrl::External(PAGE1.parse().expect("page1 url")),
    )
    .data_directory(dir);
    window.add_child(
      child_b,
      LogicalPosition::new(660.0, 450.0),
      LogicalSize::new(560.0, 290.0),
    )?;
  }
  Ok(())
}

fn child<'a>(app: &'a AppHandle, label: &str) -> Result<Webview<Wry>, String> {
  app
    .get_webview(label)
    .ok_or_else(|| format!("no webview labelled {label}"))
}

/// Run `f(core, tx)` on the UI thread against the child's ICoreWebView2 and
/// wait (off the main thread) for the COM completion result.
async fn webview_call<F>(app: &AppHandle, label: &str, f: F) -> Result<String, String>
where
  F: FnOnce(ICoreWebView2, mpsc::Sender<Result<String, String>>) -> Result<(), String>
    + Send
    + 'static,
{
  let webview = child(app, label)?;
  let (tx, rx) = mpsc::channel();
  let err_tx = tx.clone();
  webview
    .with_webview(move |pw| {
      let outcome = (|| -> Result<(), String> {
        let controller = pw.controller();
        let core = unsafe { controller.CoreWebView2() }
          .map_err(|e| format!("CoreWebView2: {e}"))?;
        f(core, tx)
      })();
      if let Err(e) = outcome {
        let _ = err_tx.send(Err(e));
      }
    })
    .map_err(|e| format!("with_webview: {e}"))?;
  // Blocks a tokio worker, never the main thread. The completion handler is
  // delivered by the main thread's message pump while we wait here.
  match rx.recv_timeout(Duration::from_secs(15)) {
    Ok(r) => r,
    Err(_) => Err(format!("timeout waiting for WebView2 completion in `{label}`")),
  }
}

async fn exec_on(app: &AppHandle, label: &str, script: String) -> Result<String, String> {
  webview_call(app, label, move |core, tx| unsafe {
    let hs = HSTRING::from(script);
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, json| {
      let _ = tx.send(match hr {
        Ok(()) => Ok(json),
        Err(e) => Err(format!("ExecuteScript failed: {e}")),
      });
      Ok(())
    }));
    core
      .ExecuteScript(&hs, &handler)
      .map_err(|e| format!("ExecuteScript call: {e}"))?;
    Ok(())
  })
  .await
}

async fn cdp_on(
  app: &AppHandle,
  label: &str,
  method: String,
  params: String,
) -> Result<String, String> {
  webview_call(app, label, move |core, tx| unsafe {
    let m = HSTRING::from(method);
    let p = HSTRING::from(params);
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, json| {
      let _ = tx.send(match hr {
        Ok(()) => Ok(json),
        Err(e) => Err(format!("CallDevToolsProtocolMethod failed: {e}")),
      });
      Ok(())
    }));
    core
      .CallDevToolsProtocolMethod(&m, &p, &handler)
      .map_err(|e| format!("CallDevToolsProtocolMethod call: {e}"))?;
    Ok(())
  })
  .await
}

// ---- commands -------------------------------------------------------------

/// Set the child's bounds in LOGICAL coordinates from the main page's
/// `getBoundingClientRect()`.
#[tauri::command]
pub fn spike_set_bounds(
  app: AppHandle,
  x: f64,
  y: f64,
  w: f64,
  h: f64,
) -> Result<(), String> {
  let wv = child(&app, CHILD_A)?;
  wv.set_position(LogicalPosition::new(x, y))
    .map_err(|e| e.to_string())?;
  wv.set_size(LogicalSize::new(w, h))
    .map_err(|e| e.to_string())
}

/// Raw bounds as Tauri reports them, for every label that exists.
#[tauri::command]
pub fn spike_bounds(app: AppHandle) -> Result<serde_json::Value, String> {
  let mut out = serde_json::Map::new();
  for label in [CHILD_A, CHILD_B] {
    if let Some(wv) = app.get_webview(label) {
      let b = wv.bounds().map_err(|e| e.to_string())?;
      out.insert(label.to_string(), serde_json::to_value(b).map_err(|e| e.to_string())?);
    }
  }
  Ok(serde_json::Value::Object(out))
}

#[tauri::command]
pub fn spike_hide(app: AppHandle) -> Result<(), String> {
  child(&app, CHILD_A)?.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn spike_show(app: AppHandle) -> Result<(), String> {
  child(&app, CHILD_A)?.show().map_err(|e| e.to_string())
}

/// Window geometry + scale, physical pixels unless noted.
#[tauri::command]
pub fn spike_window_info(app: AppHandle) -> Result<serde_json::Value, String> {
  let w = app.get_window("main").ok_or("no main window")?;
  let inner_pos = w.inner_position().map_err(|e| e.to_string())?;
  let outer_pos = w.outer_position().map_err(|e| e.to_string())?;
  let inner_size = w.inner_size().map_err(|e| e.to_string())?;
  let outer_size = w.outer_size().map_err(|e| e.to_string())?;
  let scale = w.scale_factor().map_err(|e| e.to_string())?;
  Ok(serde_json::json!({
    "inner_pos": {"x": inner_pos.x, "y": inner_pos.y},
    "outer_pos": {"x": outer_pos.x, "y": outer_pos.y},
    "inner_size": {"w": inner_size.width, "h": inner_size.height},
    "outer_size": {"w": outer_size.width, "h": outer_size.height},
    "scale_factor": scale,
  }))
}

#[tauri::command]
pub fn spike_resize_main(app: AppHandle, w: f64, h: f64) -> Result<(), String> {
  let win = app.get_window("main").ok_or("no main window")?;
  win.set_size(LogicalSize::new(w, h))
    .map_err(|e| e.to_string())
}

/// Bring our window to the front and pin it on top so screen captures are
/// not occluded by other windows. `top: false` unpins after the capture.
#[tauri::command]
pub fn spike_focus(app: AppHandle, top: bool) -> Result<(), String> {
  let win = app.get_window("main").ok_or("no main window")?;
  win.set_always_on_top(top).map_err(|e| e.to_string())?;
  win.set_focus().map_err(|e| e.to_string())
}

/// ExecuteScript on child A — result comes back through the COM completion.
#[tauri::command]
pub async fn spike_exec(app: AppHandle, script: String) -> Result<String, String> {
  exec_on(&app, CHILD_A, script).await
}

/// ExecuteScript on child B (only exists when SPIKE_CHILD_B was set).
#[tauri::command]
pub async fn spike_exec_b(app: AppHandle, script: String) -> Result<String, String> {
  exec_on(&app, CHILD_B, script).await
}

/// CallDevToolsProtocolMethod on child A (e.g. Page.navigate,
/// Input.dispatchMouseEvent, Input.insertText, Page.captureScreenshot).
#[tauri::command]
pub async fn spike_cdp(
  app: AppHandle,
  method: String,
  params: String,
) -> Result<String, String> {
  cdp_on(&app, CHILD_A, method, params).await
}

#[tauri::command]
pub async fn spike_cdp_b(
  app: AppHandle,
  method: String,
  params: String,
) -> Result<String, String> {
  cdp_on(&app, CHILD_B, method, params).await
}

/// Page.captureScreenshot over in-process CDP -> decode -> save PNG.
#[tauri::command]
pub async fn spike_screenshot(app: AppHandle, path: String) -> Result<usize, String> {
  let res = cdp_on(&app, CHILD_A, "Page.captureScreenshot".into(), "{\"format\":\"png\"}".into())
    .await?;
  let v: serde_json::Value =
    serde_json::from_str(&res).map_err(|e| format!("screenshot JSON: {e} ({res})"))?;
  let b64 = v
    .get("data")
    .and_then(|d| d.as_str())
    .ok_or_else(|| format!("no data field in screenshot response: {res}"))?;
  let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
    .map_err(|e| format!("base64: {e}"))?;
  std::fs::write(&path, &bytes).map_err(|e| format!("write {path}: {e}"))?;
  Ok(bytes.len())
}

/// Build child B (own data_directory) — only when it does not exist yet.
/// Async: `add_child` must not run in a sync command (WebView2 deadlock).
#[tauri::command]
pub async fn spike_create_child_b(app: AppHandle) -> Result<String, String> {
  if app.get_webview(CHILD_B).is_some() {
    return Ok("already exists".into());
  }
  let window = app.get_window("main").ok_or("no main window")?;
  let dir = app
    .path()
    .app_local_data_dir()
    .map_err(|e| e.to_string())?
    .join("spike-child-b-profile");
  let builder = tauri::webview::WebviewBuilder::new(
    CHILD_B,
    WebviewUrl::External(PAGE1.parse::<tauri::Url>().map_err(|e| e.to_string())?),
  )
  .data_directory(dir.clone());
  window
    .add_child(
      builder,
      LogicalPosition::new(660.0, 450.0),
      LogicalSize::new(560.0, 290.0),
    )
    .map_err(|e| e.to_string())?;
  Ok(format!("created with data_directory {}", dir.display()))
}

// ---- file-based RPC with the main-page harness (no debug port) ------------

/// Returns the contents of spike/cmd.json once per distinct `id`.
#[tauri::command]
pub fn spike_cmd_poll(state: tauri::State<'_, SpikeCmdLast>) -> Option<String> {
  let txt = std::fs::read_to_string(spike_dir().join("cmd.json")).ok()?;
  let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
  let id = v.get("id")?.as_i64()?;
  let mut last = state.0.lock().ok()?;
  if id <= *last {
    return None;
  }
  *last = id;
  Some(txt)
}

/// Append one JSON line to spike/out.ndjson.
#[tauri::command]
pub fn spike_log(line: String) -> Result<(), String> {
  let path = spike_dir().join("out.ndjson");
  let mut f = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&path)
    .map_err(|e| format!("open {}: {e}", path.display()))?;
  writeln!(f, "{line}").map_err(|e| format!("write: {e}"))
}
