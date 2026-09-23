// SPIKE ONLY — this branch never merges.
//
// Measurement harness for the browser-tab spike. Creates a reference <div>
// in the main page and answers command files (spike/cmd.json, written by
// spike/drive.mjs) by calling the spike_* Tauri commands, logging results to
// spike/out.ndjson. No debug port involved: the app drives its child page in
// process; the harness only sequences the calls.
import { invoke } from "@tauri-apps/api/core";

type Args = Record<string, unknown>;
type Cmd = { id: number; op: string; args?: Args };

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

// Reference slot. position:fixed → getBoundingClientRect is viewport-relative,
// which maps 1:1 onto the child webview's parent-window coordinates.
const div = document.createElement("div");
div.id = "spike-slot";
div.style.cssText = [
  "position:fixed",
  "left:660px",
  "top:60px",
  "width:560px",
  "height:360px",
  "outline:3px dashed #00e5ff",
  "pointer-events:none",
  "z-index:2147483647",
].join(";");
document.body.appendChild(div);

function rect() {
  const r = div.getBoundingClientRect();
  return { x: r.x, y: r.y, w: r.width, h: r.height };
}

async function sync() {
  const r = rect();
  const t0 = Date.now();
  await invoke("spike_set_bounds", { x: r.x, y: r.y, w: r.w, h: r.h });
  return { rect: r, invokeMs: Date.now() - t0 };
}

const ops: Record<string, (a: Args) => Promise<unknown>> = {
  sync,
  move: async (a) => {
    if (typeof a.x === "number") div.style.left = `${a.x}px`;
    if (typeof a.y === "number") div.style.top = `${a.y}px`;
    if (typeof a.w === "number") div.style.width = `${a.w}px`;
    if (typeof a.h === "number") div.style.height = `${a.h}px`;
    await sleep(50); // let layout settle before reading the rect
    return sync();
  },
  // Set child bounds WITHOUT moving the div (offset / park tests).
  raw_bounds: (a) =>
    invoke("spike_set_bounds", {
      x: Number(a.x),
      y: Number(a.y),
      w: Number(a.w),
      h: Number(a.h),
    }),
  bounds: () => invoke("spike_bounds"),
  wininfo: () => invoke("spike_window_info"),
  resize: async (a) => {
    const t0 = Date.now();
    await invoke("spike_resize_main", { w: Number(a.w), h: Number(a.h) });
    await sleep(400);
    return { resizeMs: Date.now() - t0, win: await invoke("spike_window_info") };
  },
  hide: () => invoke("spike_hide"),
  show: () => invoke("spike_show"),
  focus: (a) => invoke("spike_focus", { top: a.top !== false }),
  exec: (a) => invoke("spike_exec", { script: String(a.script) }),
  cdp: (a) =>
    invoke("spike_cdp", {
      method: String(a.method),
      params: typeof a.params === "string" ? a.params : JSON.stringify(a.params ?? {}),
    }),
  shot: (a) => invoke("spike_screenshot", { path: String(a.path) }),
  cB_create: () => invoke("spike_create_child_b"),
  cB_exec: (a) => invoke("spike_exec_b", { script: String(a.script) }),
  cB_cdp: (a) =>
    invoke("spike_cdp_b", {
      method: String(a.method),
      params: typeof a.params === "string" ? a.params : JSON.stringify(a.params ?? {}),
    }),
  divrect: async () => rect(),
  ping: async () => ({
    href: location.href,
    ua: navigator.userAgent,
    dpr: window.devicePixelRatio,
    t: Date.now(),
  }),
};

let busy = false;
async function poll() {
  if (busy) return;
  busy = true;
  try {
    const raw = await invoke<string | null>("spike_cmd_poll");
    if (raw) {
      const cmd = JSON.parse(raw) as Cmd;
      const t0 = Date.now();
      try {
        const op = ops[cmd.op];
        if (!op) throw new Error(`unknown op ${cmd.op}`);
        const result = await op(cmd.args ?? {});
        await invoke("spike_log", {
          line: JSON.stringify({ id: cmd.id, op: cmd.op, ok: true, result, t0, t1: Date.now() }),
        });
      } catch (e) {
        await invoke("spike_log", {
          line: JSON.stringify({
            id: cmd.id,
            op: cmd.op,
            ok: false,
            error: String(e),
            t0,
            t1: Date.now(),
          }),
        });
      }
    }
  } catch {
    // App not ready yet (page loading) — retry next tick.
  }
  busy = false;
}

setInterval(() => {
  void poll();
}, 150);
