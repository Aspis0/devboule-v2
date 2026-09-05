# Polis surface

Polis is not compiled into Devboule. It is installed as files, and this surface is what
the app shows around it: the plugin's own UI once it is installed and verified, and an
account of what is missing when it is not.

`PolisSurface` renders `PluginSurface` only when the inventory reports Polis `ready`.
Otherwise it draws two readiness sections, and the split between them is deliberate —
they answer different questions and can fail independently:

- **Can this app load code it did not build?** A one-shot transport probe checks the
  policy, the origin and the content type end to end. It runs once and is never retried,
  because the answer is a property of the build and the CSP, not of the moment.
- **What is actually installed?** A plugin is a directory with a manifest listing every
  file and its digest. Devboule reads nothing the manifest did not declare, and a plugin
  whose files no longer match is refused with a reason rather than half-loaded.

The inventory comes from the app store, not from a fetch in this component. The crescent
decides from the same answer whether to offer an install, and two fetches would be two
answers that could disagree.

See [`../plugins/README.md`](../plugins/README.md) for the frame, the bridge and the
capability model that carry any plugin, Polis included.
