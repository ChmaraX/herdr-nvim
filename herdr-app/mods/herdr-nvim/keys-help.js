// The tree's key list (`?` in the tree, from tree-keys.js): a dialog of this
// mod with props {help: true}, grouped like Snacks' help, in two columns (Navigate | Files,
// View) so it fits a short window. ?, Esc or q closes it, and the tree
// gets the keyboard back ("focus").

import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { closeOverlay, openOverlay, props, theme } from "herdr";
import { helpGroups } from "./tree-keys.js";

// The tree key's presses as the list shows them: ["space", "e"] -> "space e".
export function keyLabel(presses) {
  return presses?.length ? presses.join(" ") : null;
}

export function openKeyHelp(treeKey) {
  openOverlay({ mod: "herdr-nvim", kind: "dialog", width: 780, props: { help: true, treeKey: keyLabel(treeKey) } });
}

export class KeyHelp {
  constructor(cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
  }

  render() {
    const t = theme(), shown = helpGroups(props()?.treeKey);
    const close = () => closeOverlay(props().overlay);
    const group = ([title, keys]) => v_flex().gap(4)
      .child(div().pt(6).text_size(11).font_semibold().text_color(t.dim).child(title.toUpperCase()))
      .children(keys.map(([key, what]) => h_flex().gap(12)
        .child(div().w(84).flex_none().text_color(t.accent).font_family("JetBrains Mono").child(key))
        .child(div().flex_1().text_color(t.foreground).child(what))));
    return v_flex().id("herdr-nvim-keys").track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (["escape", "q"].includes(e.key) || e.key_char === "?") { close(); cx.stop_propagation(); }
      })
      .w_full().p(18).gap(6).bg(t.popover).text_size(12.5)
      .child(h_flex().items_center().child(div().flex_1().text_size(15).font_semibold().text_color(t.foreground).child("Files keys"))
        .child(div().text_size(11).text_color(t.dim).child("? / Esc")))
      .child(h_flex().items_start().gap(28)
        .child(v_flex().flex_1().gap(6).children(shown.slice(0, 1).map(group)))
        .child(v_flex().flex_1().gap(6).children(shown.slice(1).map(group))));
  }
}
