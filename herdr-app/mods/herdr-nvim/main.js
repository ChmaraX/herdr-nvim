// herdr-nvim for the Herdr app: the tab's nvim (a real nvim terminal, a
// full-height pane of the tab backed by that tab's persistent daemon) and a
// native file tree beside it, opened and closed together.
//
// One mod, five roles, chosen by where it is shown:
// - the side panel's `files` slot (the tab bar's Files button shows it): the
//   file tree (tree-view.js), with the keyboard when `pane.go` or
//   cmd-shift-e gives it (mod.json "focusable");
// - an empty floating block, props {role: "sync"}: the controller that opens
//   and closes the tab's nvim pane with the panel (sync.js);
// - a pane with props {preview, tab}: the file rendered in the tab's nvim's
//   place (preview.js; the controller's swap puts it there, sync.js);
// - a dialog with props {dialog}: the tree's new/rename/copy/delete prompts
//   (dialog.js); with props {help}: its key list (keys-help.js); with props
//   {bar}: the title of cmd-o's picker (pick.js);
// - an overlay, `mod.open:herdr-nvim` (cmd-e): does exactly what the Files
//   button does, then closes; with `tree = "nvim"`, or without the slot in
//   this layout, it toggles the tab's nvim alone.

import { View, div } from "gpui-kit";
import { closeOverlay, layout, props } from "herdr";
import { nvim, place } from "./lib.js";
import { treeMode } from "./ops.js";
import { hidePanel, panelState, showFiles } from "./panel.js";
import { Dialog } from "./dialog.js";
import { KeyHelp } from "./keys-help.js";
import { PickBar } from "./pick.js";
import { Preview } from "./preview.js";
import { Sync } from "./sync.js";
import { Tree } from "./tree-view.js";
import { timed } from "./timing.js";

export default class HerdrNvim extends View {
  init(_props, cx) {
    const { overlay, role, dialog, help, bar, preview } = props() ?? {};
    if (preview) this.part = new Preview(props(), cx);
    else if (bar) this.part = new PickBar();
    else if (help) this.part = new KeyHelp(cx);
    else if (dialog) this.part = new Dialog(cx);
    else if (overlay) {
      cx.spawn(async () => {
        const p = panelState(layout()), at = place();
        if (!p || (await treeMode()) === "nvim") {
          if (p?.files) hidePanel(p);
          await nvim("toggle", [], at);
        } else if (p.files) hidePanel(p);
        else showFiles(p);
        closeOverlay(overlay);
      });
    } else if (role === "sync") this.sync = new Sync(cx);
    else this.part = new Tree(cx);
  }

  render(cx) {
    const name = this.sync ? "controller" : this.part?.constructor?.name ?? "overlay";
    return timed(name, () => {
      if (this.part) return this.part.render(cx);
      this.sync?.look(cx);
      return div();
    });
  }
}
