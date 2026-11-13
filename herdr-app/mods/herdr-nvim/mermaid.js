// Mermaid diagrams in the markdown preview (preview.js). Optional: they are
// drawn by mermaid-cli (`mmdc`) when it is on the PATH; without it, a
// ```mermaid block stays a code block, as in any other markdown.
//
// A diagram is drawn once per code and theme, as a PNG at twice its size into
// the preview's image cache (named by both), so editing the text around it,
// or reloading the file, draws nothing again.

import { exec, home } from "herdr";
import * as fs from "fs/promises";

// The fence that opens a mermaid block: ```mermaid or ~~~mermaid, up to three
// spaces in, as CommonMark has it. Other fences are tracked so a mermaid
// fence inside one (a markdown example) stays text.
const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/;

// The markdown as parts in order: {md} text, {mermaid} a block's code. An
// unclosed mermaid block (an agent still writing it) stays text.
export function splitMermaid(text) {
  const parts = [];
  let md = [];
  let open = null; // {fence, mermaid, lines, start}
  const closes = (line) => new RegExp(`^ {0,3}${open.fence[0] === "`" ? "`" : "~"}{${open.fence.length},}\\s*$`).test(line);
  for (const line of text.split("\n")) {
    if (open) {
      if (closes(line)) {
        if (open.mermaid) {
          if (md.length) parts.push({ md: md.join("\n") });
          md = [];
          parts.push({ mermaid: open.lines.join("\n") });
        } else md.push(open.start, ...open.lines, line);
        open = null;
      } else open.lines.push(line);
      continue;
    }
    const m = FENCE.exec(line);
    if (m && !(m[1][0] === "`" && m[2].includes("`"))) open = { fence: m[1], mermaid: m[2].toLowerCase() === "mermaid", lines: [], start: line };
    else md.push(line);
  }
  if (open) md.push(open.start, ...open.lines);
  if (md.length) parts.push({ md: md.join("\n") });
  return parts;
}

export const hasMermaid = (parts) => parts.some((p) => p.mermaid !== undefined);

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

// What mmdc's output says went wrong, without its stack: the parse error's
// lines, else the first line.
export function mmdcError(stderr) {
  const lines = (stderr ?? "").split("\n").map((l) => l.trimEnd()).filter((l) => l && !/^\s+at /.test(l));
  const parse = lines.findIndex((l) => /error/i.test(l));
  return (parse >= 0 ? lines.slice(parse, parse + 4) : lines.slice(0, 1)).join("\n") || "mmdc failed";
}

// Draws one diagram into `dir` (absolute, the cache) as `name`:
// {drawn: true}, {missing: true} (no mmdc), or {error}.
// mmdc runs in the home folder, where puppeteer looks for its config
// (~/.puppeteerrc.cjs, e.g. one pointing it at an installed Chrome).
export async function drawMermaid(code, dark, dir, name) {
  const input = `${dir}/.${name}.mmd`;
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(input, code);
  try {
    const args = ["-i", input, "-o", `${dir}/${name}`, "-t", dark ? "dark" : "default", "-b", "transparent", "-s", "2", "-q"];
    let r;
    try {
      r = await exec("mmdc", args, { cwd: home(), timeout: 60000 });
    } catch {
      return { missing: true };
    }
    if (r.code === 127 || /command not found|No such file or directory \(os error 2\)/.test(r.stderr ?? "")) return { missing: true };
    if (r.code !== 0) return { error: mmdcError(r.stderr || r.stdout) };
    return { drawn: true };
  } finally {
    await fs.unlink(input).catch(() => {});
  }
}

// Whether a theme's background is dark: "#rrggbb" by its luminance; anything
// else counts as dark, Herdr's default.
export function isDark(background) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(background ?? "");
  if (!m) return true;
  const [r, g, b] = m.slice(1).map((x) => parseInt(x, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
}
