// cmd-o (`herdr-nvim.pick`, taken by the controller, sync.js): a file from
// the agent's output. `herdr-nvim pick-file` gathers the candidates (the
// agent's session log first, then git changes, the paths on its screen and
// the repo) and starts the picker in a hidden core terminal; it is shown in a
// dialog under a title bar (this mod with props {bar}). ⏎ opens the file in
// the tab's nvim (`pick-file --finish`).

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { hc, notify, openOverlay, terminals, theme } from "herdr";
import { nvim } from "./lib.js";

// `at`: the tab, as `place()` captured it when the key was pressed.
// One picker at a time. cmd-o again while one starts does nothing; a picker
// still running from before (its dialog gone some way) is ended first, so a
// lost one never blocks cmd-o. The dialog is modal: a click beside it
// doesn't close it while the picker runs on; Esc and ⏎ end the picker, and
// the dialog closes with it.
let starting = false, shown = null;

export async function pick(at) {
  if (starting) return;
  if (shown && terminals().some((t) => t.address === shown && t.state === "live")) {
    await hc(["kill", shown]).catch(() => null);
  }
  shown = null;
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
    modal: true,
    width: 820,
    height: 470,
    layout: { type: "column", children: [
      { type: "block", size: { px: 38 }, content: { mod: "herdr-nvim", props: { bar: true } } },
      { type: "block", content: { terminal: result.terminal } },
    ] },
  });
  shown = result.terminal;
}

// One key as a keycap: a small rounded box, as gitview draws its keys
// (herdr-gitview mods/gitview/chrome.js `keycap`), so both read alike.
function keycap(t, k) {
  const symbol = k.length === 1 && !/[a-z0-9]/i.test(k);
  return div().flex_none().min_w(18).h(18).px(5).flex().items_center().justify_center()
    .rounded(4).border_1().border_color(t.border).bg(t.raised)
    .when(!symbol, (e) => e.font_family("JetBrains Mono"))
    .text_size(symbol ? 12 : 10.5).text_color(t.foreground).child(k);
}

function keyChip(t, caps, label) {
  return h_flex().flex_none().gap(5).items_center()
    .child(h_flex().flex_none().gap(2).children(caps.map((k) => keycap(t, k))))
    .child(div().flex_none().text_size(11.5).text_color(t.dim).child(label));
}

// The dialog's title bar: its name, then the picker's keys as keycaps (the
// picker draws no hint line of its own in the app).
export class PickBar {
  render() {
    const t = theme();
    return h_flex().size_full().px(14).gap(14).items_center().bg(t.chrome).border_b(1).border_color(t.border).text_size(12.5)
      .child(div().text_color(t.foreground).font_semibold().child("Open file from agent output"))
      .child(div().flex_1())
      .child(keyChip(t, ["↑", "↓"], "move"))
      .child(keyChip(t, ["⏎"], "open in nvim"))
      .child(keyChip(t, ["^U"], "clear"))
      .child(keyChip(t, ["esc"], "close"));
  }
}
