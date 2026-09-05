# Plugins

Devboule runs code it did not build. This module is the boundary that makes that
acceptable: the frame a plugin lives in, the single channel it may speak through, and the
rules deciding what it is allowed to ask for.

It is the largest module in the frontend and the one where a mistake is most expensive,
so the reasoning is recorded here rather than left to be re-derived.

## The frame

`PluginSurface` loads the plugin from `http://plugin.localhost`, a registered scheme the
app serves, into an `<iframe sandbox="allow-scripts allow-same-origin">`.

**That attribute pair is usually a warning sign, and here it is not.** The familiar
objection — a framed document can delete its own `sandbox` attribute and escape — holds
only when the frame is same-origin **with the embedder**. It is not: the embedder is
`http://localhost:1420` in development or `http://tauri.localhost` when packaged, while
the frame is `http://plugin.localhost`. Across those origins the frame cannot reach the
parent document, and Tauri denies it IPC (GHSA-57fm-592m-34r7, patched in
`>= 2.0.0-beta.20`; this app is on 2.11.5).

`allow-same-origin` here means _keep your own origin_, which the plugin needs for its own
storage and for `postMessage` replies to be attributable at all. The sandbox still denies
top-level navigation, popups, form submission and downloads.

Do not simplify this to "same-origin is fine" or "sandbox pairs are dangerous". Both
readings are wrong; the deployment is what decides.

The frame's `key` includes the plugin id, origin, capabilities and a reload token, so a
bridge rebuild always remounts the document. A document that outlived its bridge would
hold a `sessions.watch` subscription the host had forgotten and would never learn to
resubscribe.

## The bridge

`pluginBridge.ts` is the only channel a plugin gets to ask the host for work.

Incoming messages are checked by **both origin and identity** — the source window must be
this frame's `contentWindow` _and_ the origin must match. Either check alone can admit a
message from a different document, so neither is sufficient. Messages are versioned
(`v: 1`), carry a correlation id, and are capped at 1 MiB; every request the host issues
into the frame has a timeout, because a plugin that never replies must not be able to
leak a pending promise per call.

## Capabilities are granted twice

A manifest **requesting** a capability is necessary and not sufficient. The host keeps its
own allowlist, `HOST_SERVED_CAPABILITIES`, and a method is routed only when it appears in
both. Today that list is `sessions.watch` and `oracle.search`.

This is deliberate: a manifest is written by the plugin author, so treating it as the sole
authority would let the plugin decide its own permissions. The second key is held by code
the author does not write.

## Backends

`pluginBackend.ts` hands out a lease per surface. The backend starts on first acquire and
stops when the last lease is released, so a plugin the user is not looking at is not a
process left running.

## Tests

`pluginBridge.test.ts` is roughly twice the size of the module it covers. That ratio is
correct for this file: most of its assertions are about messages that must be _rejected_ —
wrong origin, wrong window, oversized payload, ungranted capability, absent reply — and
rejection paths are exactly what a refactor silently loosens.
