// cmd-o (`herdr-nvim.pick`, taken by the controller, sync.js): a file from
// the agent's output. `herdr-nvim pick-file` gathers the candidates (the
// agent's session log first, then git changes, the paths on its screen and
// the repo) and starts the picker in a hidden core terminal; it is shown in a
// dialog under a title bar (this mod with props {bar}). ⏎ opens the file in
// the tab's nvim (`pick-file --finish`).

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { notify, openOverlay, terminals, theme } from "herdr";
import { nvim } from "./lib.js";

// `at`: the tab, as `place()` captured it when the key was pressed.
// One picker at a time: cmd-o again while one opens or is open does nothing
// (the dialog closes itself when its picker exits).
let starting = false, shown = null;

export async function pick(at) {
  if (starting || (shown && terminals().some((t) => t.address === shown && t.state === "live"))) return;
  starting = true;
  let out = null;
  try {
    out = await nvim("pick-file", [], at);
  } finally {
    starting = false;
  }
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
  shown = result.terminal;
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
