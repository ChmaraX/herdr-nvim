// cmd-o (`herdr-nvim.pick`, taken by the controller, sync.js): a file from
// the agent's output. `herdr-nvim pick-file` gathers the candidates (the
// agent's session log first, then git changes, the paths on its screen and
// the repo) and starts the picker in a hidden core terminal; it is shown in a
// dialog under a title bar (this mod with props {bar}). ⏎ opens the file in
// the tab's nvim (`pick-file --finish`).

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { notify, openOverlay, theme } from "herdr";
import { nvim } from "./lib.js";
import { mark } from "./timing.js";

// `at`: the tab, as `place()` captured it when the key was pressed.
export async function pick(at) {
  const t0 = Date.now();
  const out = await nvim("pick-file", [], at);
  mark(`pick-file answered in ${Date.now() - t0} ms`);
  if (out === null) return;
  const result = JSON.parse(out);
  if (result.empty) {
    notify({ title: "herdr-nvim: nothing to pick", body: result.empty, level: "warning", timeout: 8000 });
    return;
  }
  openOverlay({
    kind: "dialog",
    width: 820,
    height: 470,
    layout: { type: "column", children: [
      { type: "block", size: { px: 30 }, content: { mod: "herdr-nvim", props: { bar: true } } },
      { type: "block", content: { terminal: result.terminal } },
    ] },
  });
  mark(`dialog opened with ${result.terminal}`);
}

// The dialog's title bar.
export class PickBar {
  render() {
    const t = theme();
    return h_flex().size_full().px(12).gap(8).items_center().bg(t.chrome).border_b(1).border_color(t.border).text_size(12.5)
      .child(div().text_color(t.foreground).font_semibold().child("Open file from agent output"))
      .child(div().flex_1())
      .child(div().text_color(t.dim).child("⏎ open in nvim · esc close"));
  }
}
