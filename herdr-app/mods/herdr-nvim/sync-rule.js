// The controller's rule (sync.js, top of the file), as a pure function, so
// it can be checked outside the app (herdr-app/e2e/js-checks.mjs).

// Pure: what the last change on the shown tab asks for, applying the rule
// at the top. `cur`/`prev`: {tab, files, other, nvim} now and at the last
// look; `kept`: the tree key hid this tab's tree, nvim kept;
// `hidByTreeKey`: the panel just closed by the tree key. Returns `panel`
// ("show"/"hide": the panel follows the tab's nvim), `nvim` (true: the tab's
// nvim follows the panel, `act`) and `kept` (true/false: set or clear that
// tab's mark).
export function wanted(cur, prev, { native, local, kept, hidByTreeKey }) {
  const followTab = (kept) => {
    if (native && cur.nvim && !cur.files && !cur.other && !kept) return "show";
    if (cur.files && (!cur.nvim || !native || kept)) return "hide";
    return null;
  };
  // The panel opened: Files or cmd-e (a tab switch never shows it, and our
  // own shows are in `prev`). Also when this look comes after a tab switch
  // (the controller was busy, or both came within a frame).
  if (prev && !prev.files && cur.files) return { kept: false, nvim: local };
  if (!prev || prev.tab !== cur.tab) return { panel: followTab(kept) };
  if (prev.files !== cur.files) {
    if (hidByTreeKey) return { kept: true };
    return { nvim: !cur.other && cur.nvim && native };
  }
  if (prev.nvim !== cur.nvim) {
    if (!cur.nvim) return { kept: false, panel: followTab(false) };
    return { panel: followTab(kept) };
  }
  return {};
}
