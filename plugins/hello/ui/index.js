const canvas = document.querySelector("#scene");
const webglReadout = document.querySelector("#webgl");
const tauriReadout = document.querySelector("#tauri");
const bridgeReadout = document.querySelector("#bridge");
const TAURI_PROBE_TIMEOUT_MS = 4000;

const isolationMeasurement = measureTauriIsolation();
void isolationMeasurement.then((isolation) => reportIsolationOutcome(isolation));
requestBridgeProbe();

const gl = canvas.getContext("webgl2", { antialias: true });
if (gl === null) {
  webglReadout.textContent = "WebGL2: unavailable — no WebGL2 context was created";
} else {
  const debugInfo = gl.getExtension("WEBGL_debug_renderer_info");
  const renderer = debugInfo
    ? gl.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL)
    : "debug extension unavailable";
  webglReadout.textContent = `WebGL2: available | renderer: ${renderer}`;
  startScene(gl);
}

async function measureTauriIsolation() {
  const internals = window.__TAURI_INTERNALS__;
  if (internals === undefined) {
    const outcome = { status: "object-absent" };
    renderIsolationOutcome(outcome);
    return outcome;
  }

  try {
    // Check the actual registered command. The injected object alone is not
    // evidence that a cross-origin frame can use Tauri IPC.
    const value = await invokePluginsListWithTimeout(internals);
    const outcome = { status: "call-succeeded", value: serializableValue(value) };
    renderIsolationOutcome(outcome);
    return outcome;
  } catch (error) {
    if (isProbeTimeout(error)) {
      const outcome = {
        status: "present-call-timed-out",
        message: errorMessage(error),
      };
      renderIsolationOutcome(outcome);
      return outcome;
    }
    const outcome = { status: "present-call-refused", message: errorMessage(error) };
    renderIsolationOutcome(outcome);
    return outcome;
  }
}

function renderIsolationOutcome(outcome) {
  tauriReadout.classList.remove("prominent");
  if (outcome.status === "object-absent") {
    tauriReadout.textContent = "Tauri IPC: object is absent — isolated";
  } else if (outcome.status === "present-call-refused") {
    tauriReadout.textContent = `Tauri IPC: object is present but the call was refused or threw — isolated in the way that matters — ${outcome.message}`;
  } else if (outcome.status === "present-call-timed-out") {
    tauriReadout.textContent = `Tauri IPC: object is present, call never answered within ${TAURI_PROBE_TIMEOUT_MS / 1000} seconds — isolated; the call hangs rather than failing`;
  } else {
    tauriReadout.classList.add("prominent");
    tauriReadout.textContent = `Tauri IPC: call SUCCEEDED and returned data — NOT isolated — ${formatValue(outcome.value)}`;
  }
}

function invokePluginsListWithTimeout(internals) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = window.setTimeout(() => {
      settled = true;
      const error = new Error(
        `plugins_list did not answer within ${TAURI_PROBE_TIMEOUT_MS / 1000} seconds; the call hangs rather than failing`,
      );
      error.name = "TauriProbeTimeoutError";
      reject(error);
    }, TAURI_PROBE_TIMEOUT_MS);

    Promise.resolve()
      .then(() => internals.invoke("plugins_list"))
      .then(
        (value) => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          resolve(value);
        },
        (error) => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          reject(error);
        },
      );
  });
}

function isProbeTimeout(error) {
  return error instanceof Error && error.name === "TauriProbeTimeoutError";
}

function requestBridgeProbe() {
  const requestId = `hello-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  let replyReceived = false;

  window.addEventListener("message", (event) => {
    const message = event.data;
    if (
      replyReceived ||
      event.source !== window.parent ||
      message === null ||
      typeof message !== "object" ||
      message.v !== 1 ||
      message.id !== requestId
    ) {
      return;
    }

    replyReceived = true;
    if (message.kind === "error") {
      const expected =
        message.message === 'The host cannot route plugin method "oracle.search" yet';
      bridgeReadout.textContent = expected
        ? `Bridge reply: refusal (expected) — ${message.message}`
        : `Bridge reply: error — ${message.message}`;
    } else if (message.kind === "result") {
      bridgeReadout.textContent = `Bridge reply: result — ${JSON.stringify(message.value)}`;
    }
  });

  window.parent.postMessage(
    {
      v: 1,
      id: requestId,
      kind: "invoke",
      method: "oracle.search",
      payload: { isolation: { status: "measurement-pending" } },
    },
    "*",
  );

  window.setTimeout(() => {
    if (!replyReceived)
      bridgeReadout.textContent = "Bridge reply: no reply (silence after 4 seconds)";
  }, 4000);
}

function reportIsolationOutcome(isolation) {
  const requestId = `hello-isolation-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  window.parent.postMessage(
    {
      v: 1,
      id: requestId,
      kind: "invoke",
      method: "oracle.search",
      payload: { isolation },
    },
    "*",
  );
}

function serializableValue(value) {
  try {
    return JSON.parse(JSON.stringify(value));
  } catch {
    return String(value);
  }
}

function formatValue(value) {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function startScene(context) {
  const vertexShader = compileShader(
    context,
    context.VERTEX_SHADER,
    `#version 300 es
      in vec2 a_position;
      in vec3 a_color;
      uniform float u_angle;
      out vec3 v_color;

      void main() {
        float sine = sin(u_angle);
        float cosine = cos(u_angle);
        mat2 rotation = mat2(cosine, -sine, sine, cosine);
        gl_Position = vec4(rotation * a_position, 0.0, 1.0);
        v_color = a_color;
      }`,
  );
  const fragmentShader = compileShader(
    context,
    context.FRAGMENT_SHADER,
    `#version 300 es
      precision highp float;
      in vec3 v_color;
      out vec4 out_color;

      void main() {
        out_color = vec4(v_color, 1.0);
      }`,
  );
  if (vertexShader === null || fragmentShader === null) {
    webglReadout.textContent = "WebGL2: context available, but the demo shader failed";
    return;
  }

  const program = context.createProgram();
  if (program === null) return;
  context.attachShader(program, vertexShader);
  context.attachShader(program, fragmentShader);
  context.linkProgram(program);
  if (!context.getProgramParameter(program, context.LINK_STATUS)) {
    webglReadout.textContent = "WebGL2: context available, but the demo could not link";
    return;
  }

  const positions = context.createBuffer();
  const colors = context.createBuffer();
  if (positions === null || colors === null) return;

  context.bindBuffer(context.ARRAY_BUFFER, positions);
  context.bufferData(
    context.ARRAY_BUFFER,
    new Float32Array([0, 0.78, -0.72, -0.58, 0.72, -0.58]),
    context.STATIC_DRAW,
  );
  context.bindBuffer(context.ARRAY_BUFFER, colors);
  context.bufferData(
    context.ARRAY_BUFFER,
    new Float32Array([0.15, 0.95, 0.9, 0.95, 0.4, 0.3, 0.8, 0.25, 1]),
    context.STATIC_DRAW,
  );

  const positionLocation = context.getAttribLocation(program, "a_position");
  const colorLocation = context.getAttribLocation(program, "a_color");
  const angleLocation = context.getUniformLocation(program, "u_angle");
  if (positionLocation < 0 || colorLocation < 0 || angleLocation === null) return;

  function resize() {
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.max(1, Math.floor(canvas.clientWidth * pixelRatio));
    const height = Math.max(1, Math.floor(canvas.clientHeight * pixelRatio));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    context.viewport(0, 0, canvas.width, canvas.height);
  }

  function draw(time) {
    resize();
    context.clearColor(0.02, 0.03, 0.08, 1);
    context.clear(context.COLOR_BUFFER_BIT);
    context.useProgram(program);
    context.bindBuffer(context.ARRAY_BUFFER, positions);
    context.enableVertexAttribArray(positionLocation);
    context.vertexAttribPointer(positionLocation, 2, context.FLOAT, false, 0, 0);
    context.bindBuffer(context.ARRAY_BUFFER, colors);
    context.enableVertexAttribArray(colorLocation);
    context.vertexAttribPointer(colorLocation, 3, context.FLOAT, false, 0, 0);
    context.uniform1f(angleLocation, time / 1800);
    context.drawArrays(context.TRIANGLES, 0, 3);
    window.requestAnimationFrame(draw);
  }

  window.requestAnimationFrame(draw);
}

function compileShader(context, type, source) {
  const shader = context.createShader(type);
  if (shader === null) return null;
  context.shaderSource(shader, source);
  context.compileShader(shader);
  if (!context.getShaderParameter(shader, context.COMPILE_STATUS)) {
    context.deleteShader(shader);
    return null;
  }
  return shader;
}
