// A stand-in for the app's `herdr` module (and the two UI kits), so
// js-checks.mjs can load the mod's own files in node. Only what those files
// import, reading from `app`: each check sets what the app would report.
export const app = {
  layout: null,
  panes: [],
  terminals: [],
  focused: null,
  spaces: [],
  shared: {},
  ran: [],
  overlays: [],
  // exec(cmd, args, opts) -> {code, stdout, stderr}; a check replaces it.
  exec: async () => ({ code: 1, stdout: "", stderr: "no exec in this check" }),
};

export const exec = (...a) => app.exec(...a);
export const layout = () => app.layout;
export const panes = () => app.panes;
export const terminals = () => app.terminals;
export const focused = () => app.focused;
export const spaces = () => app.spaces;
export const shared = (k) => app.shared[k];
export const setShared = (k, v) => {
  if (v === null) throw new Error(`setShared(${k}, null): the app can't read a null back`);
  app.shared[k] = v;
  return null;
};
export const run = (name, args) => app.ran.push([name, args]);
export const notify = () => {};
export const nextCommand = () => new Promise(() => {});
export const appHome = () => "/nonexistent-app-home";
export const readTextFile = async () => { throw new Error("no files in this check"); };
export const watchFolder = () => new Promise(() => {});
export const window = () => ({ platform: "macos" });
export const hc = async () => ({ code: 1, stdout: "", stderr: "" });
export const openOverlay = (spec) => app.overlays.push(spec);
export const closeOverlay = () => {};
export const props = () => ({});
export const theme = () => ({});
// gpui-kit / gpui-base: never drawn in a check.
export const div = () => ({});
export const svg = () => ({});
export const h_flex = () => ({});
export const v_flex = () => ({});
