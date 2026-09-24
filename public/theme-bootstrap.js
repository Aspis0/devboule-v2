// Theme before first paint. Same key and values as src/lib/theme.ts (a test
// there walks this file — the two cannot import each other): read the stored
// choice and put the resolved theme on <html>, which the stylesheet's
// [data-theme="dark"] block keys on. Loaded as a same-origin script from
// <head>, so 'self' admits it in dev and production alike and no inline hash
// is needed.
(() => {
  let pref = null;
  try {
    pref = localStorage.getItem("devboule.theme");
  } catch {
    pref = null;
  }
  const stored = pref === "light" || pref === "dark" || pref === "system" ? pref : "system";
  const systemTheme = () =>
    window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  document.documentElement.dataset.theme = stored === "system" ? systemTheme() : stored;
})();
