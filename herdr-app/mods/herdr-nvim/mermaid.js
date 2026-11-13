// Mermaid diagrams in the markdown preview (preview.js). Each ```mermaid
// block is drawn by herdr-nvim itself (`app-mermaid`: Mermaid's layouts and
// themes in Rust, src/app/mermaid.rs), so there is nothing to install.
//
// A diagram is drawn once per code and theme, as a PNG at twice its size into
// the preview's image cache (named by both), so editing the text around it,
// or reloading the file, draws nothing again.

import { exec } from "herdr";
import { bin } from "./lib.js";

// The fence that opens a code block: ``` or ~~~, up to three spaces in, as
// CommonMark has it, and its language. Every fence is tracked, so a mermaid
// fence inside another one (a markdown example) stays text.
const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/;
// A link reference definition (`[label]: url`), not a footnote (`[^note]:`).
const LINK_DEFINITION = /^ {0,3}\[(?!\^)[^\]]+\]:\s*\S/;

// The markdown as parts in order: {md} text, {mermaid} a block's code; text
// without a mermaid block is one {md} part, the text as it is. An unclosed
// mermaid block (an agent still writing it) stays text. Each text part is
// rendered on its own, so it gets the document's link definitions: a
// reference resolves whichever part its definition is in.
export function splitMermaid(text) {
  const parts = [];
  const definitions = [];
  let md = [];
  let open = null; // {fence, mermaid, lines, start}
  const closes = (line) => new RegExp(`^ {0,3}${open.fence[0] === "`" ? "`" : "~"}{${open.fence.length},}\\s*$`).test(line);
  for (const line of text.split("\n")) {
    if (open) {
      if (closes(line)) {
        if (open.mermaid) {
          if (md.length) parts.push({ md });
          md = [];
          parts.push({ mermaid: open.lines.join("\n") });
        } else md.push(open.start, ...open.lines, line);
        open = null;
      } else open.lines.push(line);
      continue;
    }
    const m = FENCE.exec(line);
    if (m && !(m[1][0] === "`" && m[2].includes("`"))) open = { fence: m[1], mermaid: m[2].toLowerCase() === "mermaid", lines: [], start: line };
    else {
      md.push(line);
      if (LINK_DEFINITION.test(line)) definitions.push(line);
    }
  }
  const unclosed = open !== null;
  if (open) md.push(open.start, ...open.lines);
  if (md.length) parts.push({ md });
  if (!parts.some((p) => p.mermaid !== undefined)) return [{ md: text }];
  // Not after an unclosed fence (the last part's): they would be its code.
  const takes = (i) => definitions.length && !(unclosed && i === parts.length - 1);
  return parts.map((p, i) => {
    if (p.md === undefined) return p;
    const own = p.md.join("\n");
    return { md: takes(i) ? `${own}\n\n${definitions.join("\n")}` : own };
  });
}

// FNV-1a over a string's UTF-16 units: the cache name of a diagram.
export function hashText(s) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0") + s.length.toString(16);
}

// A PNG's size in pixels, from its IHDR chunk; null if it isn't one.
export function pngSize(bytes) {
  if (!bytes || bytes.length < 24 || bytes[1] !== 0x50 || bytes[2] !== 0x4e || bytes[3] !== 0x47) return null;
  const at = (i) => ((bytes[i] << 24) | (bytes[i + 1] << 16) | (bytes[i + 2] << 8) | bytes[i + 3]) >>> 0;
  return { width: at(16), height: at(20) };
}

// Draws one diagram as the PNG `out` (absolute; written whole or not at all):
// null, or what is wrong (Mermaid's parse error).
export async function drawMermaid(code, mode, out) {
  const r = await exec(bin(), ["app-mermaid", "--theme", mode, "--out", out, "--code", code])
    .catch((e) => ({ code: -1, stderr: String(e?.message ?? e) }));
  if (r.code === 0) return null;
  return (r.stderr ?? "").trim().replace(/^herdr-nvim: /, "") || "The diagram could not be drawn.";
}

// "dark" or "light": the diagrams' theme for a background, "#rrggbb" by its
// luminance; anything else counts as dark, Herdr's default.
export function modeOf(background) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(background ?? "");
  if (!m) return "dark";
  const [r, g, b] = m.slice(1).map((x) => parseInt(x, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5 ? "dark" : "light";
}
