// herdr-nvim for the Herdr app: the tab's nvim (a real nvim terminal, a
// full-height pane of the tab backed by that tab's persistent daemon) and a
// native file tree beside it, opened and closed together.
//
// One mod, three roles, chosen by where it is shown:
// - the side panel's `files` slot (the tab bar's Files button shows it): the
//   file tree (tree-view.js);
// - an empty floating block, props {role: "sync"}: the controller that opens
//   and closes the tab's nvim pane with the panel (sync.js);
// - an overlay, `mod.open:herdr-nvim` (cmd-e): does exactly what the Files
//   button does (Files shown: hide the panel; else show Files), then closes;
//   the controller does the rest. Without the slot in this layout it toggles
//   nvim alone.

import { View, div } from "gpui-kit";
import { closeOverlay, layout, props } from "herdr";
import { nvim } from "./lib.js";
import { hidePanel, panelState, showFiles } from "./panel.js";
import { Sync } from "./sync.js";
import { Tree } from "./tree-view.js";

export default class HerdrNvim extends View {
  init(_props, cx) {
    const { overlay, role } = props() ?? {};
    if (overlay) {
      cx.spawn(async () => {
        const p = panelState(layout());
        if (!p) await nvim("toggle");
        else if (p.files) hidePanel(p);
        else showFiles(p);
        closeOverlay(overlay);
      });
    } else if (role === "sync") this.sync = new Sync();
    else this.tree = new Tree(cx);
  }

  render(cx) {
    if (this.tree) return this.tree.render(cx);
    this.sync?.look(cx);
    return div();
  }
}
