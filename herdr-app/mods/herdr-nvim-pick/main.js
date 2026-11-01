// herdr-nvim's file picker for the Herdr app: the files the focused tab's
// agent mentioned or changed (and, as you type, the whole repo); Enter opens
// the file in the tab's nvim sidebar at the line.
//
// Two roles, one mod:
// - `mod.open:herdr-nvim-pick` (a key): gathers the candidates, starts the
//   picker in a hidden herdr-core terminal and shows it in a dialog, which
//   takes the keyboard (Esc goes to the picker) and closes when it exits.
// - `props.bar`: the dialog's title bar.

import { View, div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { props, theme, notify, openOverlay, closeOverlay } from "herdr";
import { nvim } from "./lib.js";

export default class HerdrNvimPick extends View {
  init(_props, cx) {
    const { overlay, bar } = props() ?? {};
    if (bar || !overlay) return;
    cx.spawn(async () => {
      const out = await nvim("pick-file");
      closeOverlay(overlay);
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
          { type: "block", size: { px: 30 }, content: { mod: "herdr-nvim-pick", props: { bar: true } } },
          { type: "block", content: { terminal: result.terminal } },
        ] },
      });
    });
  }

  render() {
    if (!props()?.bar) return div();
    const t = theme();
    return h_flex().size_full().px(12).gap(8).items_center().bg(t.chrome).border_b(1).border_color(t.border).text_size(12.5)
      .child(div().text_color(t.foreground).font_semibold().child("Open file from agent output"))
      .child(div().flex_1())
      .child(div().text_color(t.dim).child("⏎ open in nvim · esc close"));
  }
}
