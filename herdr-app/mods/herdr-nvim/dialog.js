// The tree's small dialogs: a name to type (new, rename, copy to) or a yes/no
// (delete, move, anything outside the tab's folder). Opened by the tree as a
// dialog of this mod with props {dialog}; it runs the operation itself, then
// tells the tree what to show (shared "herdr-nvim.reveal").

import { div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { closeOverlay, openOverlay, props, setShared, shared, theme } from "herdr";
import { fsOp, inside, pathsOf, requestFor } from "./ops.js";

export function openDialog(spec) {
  openOverlay({ mod: "herdr-nvim", kind: "dialog", width: 460, props: { dialog: spec } });
}

export class Dialog {
  constructor(cx) {
    const d = props().dialog;
    this.handle = cx.focus_handle();
    if (d.kind === "input") {
      this.input = InputState.new({ value: d.value ?? "" });
      this.input.on("submit", (_e, cx) => this.submit(cx));
      this.input.focus();
    } else {
      this.handle.focus();
    }
  }

  close() {
    closeOverlay(props().overlay);
  }

  submit(cx) {
    const d = props().dialog;
    const value = this.input ? this.input.value() : "";
    if (this.input && !value.trim()) return this.close();
    const request = requestFor(d.action, value);
    this.close();
    const outside = pathsOf(request).filter((p) => !inside(p, d.place.cwd));
    if (outside.length && !request.outside) {
      return openDialog({ kind: "confirm", place: d.place, title: "Outside this tab's folder",
        detail: `${outside.join(", ")} is outside ${d.place.cwd}. Do it anyway?`,
        action: { type: "request", request: { ...request, outside: true } } });
    }
    cx.spawn(async () => {
      const result = await fsOp(d.place, request);
      if (!result) return;
      // Outside the tab's folder as the file system resolves it (a symlink
      // in it to elsewhere, which the path alone doesn't show): ask first.
      if (result.outside) {
        return openDialog({ kind: "confirm", place: d.place, title: "Outside this tab's folder",
          detail: `${result.error}. Do it anyway?`,
          action: { type: "request", request: { ...request, outside: true } } });
      }
      const show = result.path ?? result.paths?.[0] ?? request.dir;
      const n = (shared("herdr-nvim.reveal")?.n ?? 0) + 1;
      setShared("herdr-nvim.reveal", { tab: d.place.tab, path: show ?? null, n });
    });
  }

  render(cx) {
    const d = props().dialog, t = theme();
    const button = (id, label, primary, onClick) => h_flex().id(id).h(32).px(14).items_center().rounded(8)
      .border_1().border_color(primary ? t.solid : t.input).bg(primary ? t.solid : t.raised)
      .text_color(primary ? t.solid_foreground : t.foreground).text_size(12.5).cursor_pointer()
      .hover((s) => s.opacity(0.88)).on_click(onClick).child(label);
    return v_flex().id("herdr-nvim-dialog").track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape") this.close();
        else if (e.key === "enter" && !this.input) this.submit(cx);
        else return;
        cx.stop_propagation();
      })
      .w_full().p(18).gap(10).bg(t.popover).text_color(t.foreground).text_size(13)
      .child(h_flex().items_center().child(div().flex_1().text_size(15).font_semibold().child(d.title))
        .child(div().text_size(11).text_color(t.dim).child("Esc")))
      .when(Boolean(d.detail), (el) => el.child(div().text_color(t.muted).child(d.detail)))
      .when(Boolean(this.input), (el) => el.child(Input.new(this.input).h(36).px(10).rounded(9).border_1()
        .border_color(t.accent).bg(t.background).text_size(12.8).text_color(t.foreground)))
      .when(Boolean(d.hint), (el) => el.child(div().text_size(11.5).text_color(t.dim).child(d.hint)))
      .child(h_flex().pt(8).gap(8).justify_end()
        .child(button("dlg-cancel", "Cancel", false, () => this.close()))
        .child(button("dlg-ok", d.kind === "input" ? "Save" : "Confirm", true, (_e, cx) => this.submit(cx))));
  }
}
