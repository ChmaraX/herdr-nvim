// winprobe: a throwaway mod that asks the real app the Windows facts
// herdr-nvim's installer and launcher depend on (doc/windows-facts.md).
// Setup and use: herdr-app/e2e/WINDOWS-CHECKLIST.md, step 1.
//
// Run all (or `herdr-app do winprobe.run`) runs every step below once.
// Each run writes <appHome>/winprobe-out/<run>/results.json and the probe's
// log.jsonl. Not under mods/: every file saved there reloads the mod mid-run.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import {
  appHome, checking, exec, focused, hc, home, keys, nextCommand, panes, readTextFile,
  run, setShared, shared, spaces, theme, updates, watchFolder, window, writeTextFile,
} from "herdr";

// Arguments that break naive quoting (cmd.exe, MSVC argv rules, JSON).
const NASTY = [
  'say "hi"', "%PATH%", "a&b", "x^y", "wow!", "'q'", "two words", "é", "漢",
  JSON.stringify({ k: 'v "q"', list: [1, "two", { back: "\\" }] }), "", "back\\", "C:\\dir with space\\",
];
const LAST_RUN = "winprobe.run";

const modDir = () => `${appHome()}/mods/winprobe`;
const bin = () => `${modDir()}/bin/winprobe`;
const probeExe = () => (window()?.platform === "windows" ? `${bin()}.exe` : bin());

// One run's files and results.
class Run {
  constructor(id) {
    this.id = id;
    this.out = `${appHome()}/winprobe-out/${id}`;
    this.log = `${this.out}/log.jsonl`;
    this.file = `${this.out}/results.json`;
    this.data = { run: id, steps: {} };
    this.written = null;
  }

  static async load(id) {
    const r = new Run(id);
    const text = await readTextFile(r.file).catch(() => null);
    if (text) {
      try { r.data = JSON.parse(text); r.written = text; } catch {}
    }
    return r;
  }

  // The probe's own line for this call, plus what exec saw.
  async probe(program, sub, args, tag) {
    const started = Date.now();
    try {
      const x = await exec(program, ["--log", this.log, "--tag", tag, sub, ...args], { timeout: 60000 });
      let record = null;
      try { record = JSON.parse(x.stdout.trim().split("\n").pop()); } catch {}
      return { program, code: x.code, stderr: x.stderr, ms: Date.now() - started, record };
    } catch (e) {
      return { program, rejected: String(e), ms: Date.now() - started, record: null };
    }
  }

  async records() {
    const text = await readTextFile(this.log).catch(() => null);
    return (text ?? "").split("\n").filter(Boolean).flatMap((line) => {
      try { return [JSON.parse(line)]; } catch { return []; }
    });
  }

  // A copy of the probe outside the mod folder, so renaming the folder
  // isn't refused only because the probe runs from it.
  async outside() {
    if (!this.data.outside) {
      const copy = await this.probe(probeExe(), "copy-self", [], "copy-self");
      this.data.outside = copy.record?.result?.path ?? null;
    }
    return this.data.outside;
  }

  // Steps the mod didn't see finish (it was reloaded, e.g. by its folder's
  // rename): their probe line is in the log under the step's tag.
  async reconcile() {
    const recs = await this.records();
    for (const [id, step] of Object.entries(this.data.steps)) {
      if (step.status !== "running") continue;
      const rec = [...recs].reverse().find((x) => x.tag === id);
      if (rec) this.data.steps[id] = { status: "done", result: { record: rec, from_log: true } };
    }
    const survive = recs.filter((x) => x.tag === "survive" && x.sub === "tick");
    if (survive.length) {
      this.data.survive = {
        ...this.data.survive,
        ticks: survive.length,
        ticks_without_app: survive.filter((x) => x.app_running === false).length,
        last_tick_age_s: Math.round((Date.now() - survive[survive.length - 1].ts_ms) / 1000),
      };
    }
  }

  async save() {
    const text = JSON.stringify(this.data, null, 2);
    try {
      await writeTextFile(this.file, text, this.written);
      this.written = text;
    } catch {
      // The run folder appears with the probe's first line; a stale
      // snapshot is replaced by what is on disk now.
      this.written = await readTextFile(this.file).catch(() => null);
    }
  }
}

const argsCheck = (got) => {
  if (!got) return { match: false, why: "no record" };
  const at = NASTY.findIndex((a, i) => got[i] !== a);
  if (at < 0 && got.length === NASTY.length) return { match: true };
  const i = at < 0 ? NASTY.length : at;
  return { match: false, at: i, sent: NASTY[i] ?? null, got: got[i] ?? null, count: got.length };
};
const argsWord = (c) => (c.match ? "args intact" : c.why ?? `arg ${c.at} changed`);
const execWord = (p) => (p.rejected ? "rejected" : argsWord(argsCheck(p.record?.args)));

async function waitFor(r, cx, test, seconds) {
  for (let waited = 0; waited < seconds; waited += 2) {
    if ((await r.records()).some(test)) return true;
    await cx.sleep(2000);
  }
  return false;
}

const renameWord = (p) => {
  const res = p.record?.result;
  if (!res) return p.rejected ?? "no record";
  const first = res.attempts?.find((a) => a);
  if (!res.moved) return `refused: os ${first?.os} ${first?.kind}`;
  return res.restored ? "renamed and back" : "RENAMED, NOT BACK: see results.json";
};

const STEPS = [
  {
    id: "where", about: "herdr-app.exe and hc.exe; System32 curl and tar",
    run: ({ r }) => r.probe(probeExe(), "where", [], "where"),
    sum: (p) => {
      const w = p.record?.result;
      if (!w) return p.rejected ?? "no record";
      const beside = w.beside_running_app.map((b) => (b.hc_exists ? "hc beside app" : "no hc beside app")).join(", ") || "no running app found";
      const path = Object.entries(w.on_path).map(([k, v]) => `${k} on PATH ${v.length}`).join(", ");
      return `${beside} · ${path} · System32 ${JSON.stringify(w.system32)}`;
    },
  },
  {
    id: "app", about: "platform, app home, default keys, panes' and spaces' folders",
    run: async () => ({
      platform: window()?.platform,
      app_home: appHome(),
      home: home(),
      version: updates()?.app?.version ?? null,
      focused: focused(),
      panes: panes().map(({ pane, terminal, tab, mod, title, cwd }) => ({ pane, terminal, tab, mod, title, cwd })),
      spaces: spaces().map(({ id, name, folder, machine }) => ({ id, name, folder, machine })),
      keys: keys(),
    }),
    sum: (a) => `${a.platform} · ${a.keys.length} keys · cwd e.g. ${a.panes.find((p) => p.cwd)?.cwd ?? "none"}`,
  },
  {
    id: "exec-args", about: "exec of bin/winprobe and bin/winprobe.exe with nasty args",
    run: async ({ r }) => ({
      extensionless: await r.probe(bin(), "echo", NASTY, "exec-extensionless"),
      exe: await r.probe(`${bin()}.exe`, "echo", NASTY, "exec-exe"),
    }),
    sum: (x) => `extensionless: ${execWord(x.extensionless)} · .exe: ${execWord(x.exe)}`,
  },
  {
    id: "std-command", about: "Rust std Command starting the extensionless path",
    run: ({ r }) => r.probe(probeExe(), "spawn", [bin(), "--log", r.log, "--tag", "std-extensionless", "echo", ...NASTY], "std-command"),
    sum: (p) => {
      const res = p.record?.result;
      if (!res) return p.rejected ?? "no record";
      if (res.error) return `failed: ${res.error.message}`;
      return `ran ${res.child?.image ?? "?"} · ${argsWord(argsCheck(res.child?.args))}`;
    },
  },
  {
    id: "terminals", about: "pane.split {argv}, {argv, env}, terminal.open, hc run",
    run: async ({ r, cx, pane }) => {
      const argv = (tag) => [probeExe(), "--log", r.log, "--tag", tag, "echo", ...NASTY];
      const sent = {};
      const send = (tag, command, args) => {
        try { run(command, args); sent[tag] = "sent"; } catch (e) { sent[tag] = String(e); }
      };
      send("pane-split", "pane.split", { direction: "right", pane, argv: argv("pane-split") });
      send("pane-split-env", "pane.split", {
        direction: "down", pane, argv: argv("pane-split-env"),
        env: { WINPROBE_MARK: "pane-split-env", HERDR_APP_HOME: appHome() },
      });
      send("terminal-open", "terminal.open", { argv: argv("terminal-open"), place: "right" });
      const hcVersion = await hc(["--version"]).catch((e) => ({ rejected: String(e) }));
      sent["hc-run"] = await hc(["run", "--bg", "--no-parent", "--rm", "--json", "--", ...argv("hc-run")])
        .catch((e) => ({ rejected: String(e) }));
      await cx.sleep(5000);
      const recs = await r.records();
      const tags = ["pane-split", "pane-split-env", "terminal-open", "hc-run"];
      const got = Object.fromEntries(tags.map((tag) => {
        const rec = recs.find((x) => x.tag === tag) ?? null;
        return [tag, { sent: sent[tag], args: argsCheck(rec?.args), mark: rec?.env?.WINPROBE_MARK ?? null, record: rec }];
      }));
      return { pane, hc_version: hcVersion, ...got };
    },
    sum: (x) => ["pane-split", "pane-split-env", "terminal-open", "hc-run"].map((tag) => {
      const t = x[tag];
      if (!t.record) return `${tag}: no record${tag.endsWith("env") ? " (env refused? macOS 0.1.16 refuses it)" : ""}`;
      return `${tag}: ${argsWord(t.args)}${tag.endsWith("env") ? `, env ${t.mark ? "arrived" : "dropped"}` : ""}`;
    }).join(" · "),
  },
  {
    id: "watch", about: "watchFolder's path form for a file written in the run folder",
    run: async ({ r, cx }) => {
      const touched = `${r.out}/touch.txt`;
      let watch = watchFolder(r.out);
      // Earlier writes (results.json) may still arrive first.
      await cx.sleep(1000);
      await writeTextFile(touched, "touch\n", null);
      const batches = [];
      const deadline = Date.now() + 6000;
      for (;;) {
        const got = await Promise.race([watch, cx.sleep(Math.max(0, deadline - Date.now())).then(() => null)]);
        watch.cancel();
        if (!got) break;
        batches.push(got);
        if (got.some((p) => /touch\.txt$/.test(p))) break;
        watch = watchFolder(r.out);
      }
      const hit = batches.flat().find((p) => /touch\.txt$/.test(p)) ?? null;
      return { watched: r.out, touched, hit, same_form: hit === touched, batches };
    },
    sum: (w) => (w.hit ? `${w.same_form ? "same form" : "other form"}: ${w.hit}` : `touch.txt not seen in 6 s (${w.batches.length} batches)`),
  },
  {
    id: "motw", about: "Mark of the Web on the probe exe",
    run: ({ r }) => r.probe(probeExe(), "motw", [probeExe()], "motw"),
    sum: (p) => (p.record?.result?.zone_identifier ? `marked: ${p.record.result.zone_identifier.replace(/\s+/g, " ").trim()}` : "no Zone.Identifier"),
  },
  {
    id: "detached", about: "a daemon started the way herdr-nvim starts one outlives the exec",
    run: async ({ r, cx }) => {
      const spawned = await r.probe(probeExe(), "spawn-detached", ["20"], "detached");
      const returned = Date.now();
      await cx.sleep(7000);
      const mine = (await r.records()).filter((x) => x.tag === "detached");
      const start = mine.find((x) => x.sub === "tick-start") ?? null;
      const after = mine.filter((x) => x.sub === "tick" && x.ts_ms > returned).length;
      return { spawned, ticks_after_exec_returned: after, start };
    },
    sum: (d) => {
      const flags = d.spawned.record?.result?.flags;
      return `exec took ${d.spawned.ms} ms · ${d.ticks_after_exec_returned} ticks after it · breakaway ${flags?.breakaway ?? "n/a"} · child in job ${d.start?.job?.in_job ?? "n/a"}`;
    },
  },
  {
    id: "rename-held", about: "rename the mod folder while an exe in it runs",
    run: async ({ r }) => {
      const outside = await r.outside();
      const running = !(await r.records()).some((x) => x.tag === "detached" && x.sub === "tick-end");
      return { exe_running_in_folder: running, ...(await r.probe(outside, "rename-dir", [modDir()], "rename-held")) };
    },
    sum: (p) => `${p.exe_running_in_folder === false ? "(exe had ended) " : ""}${renameWord(p)}`,
  },
  {
    id: "rename-free", about: "rename the loaded mod folder with nothing running from it",
    run: async ({ r, cx }) => {
      const ended = await waitFor(r, cx, (x) => x.tag === "detached" && x.sub === "tick-end", 30);
      const outside = await r.outside();
      return { detached_ended: ended, ...(await r.probe(outside, "rename-dir", [modDir()], "rename-free")) };
    },
    sum: renameWord,
  },
];

export default class WinProbe extends View {
  init(_props, cx) {
    this.run = null;
    this.busy = null;
    if (checking()) return;
    cx.spawn(async (cx) => {
      const id = shared(LAST_RUN);
      if (id) {
        this.run = await Run.load(id);
        await this.run.reconcile();
        await this.run.save();
        cx.notify();
      }
    });
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "run") this.runAll(cx);
        else if (command === "survive") this.survive(cx);
        else if (command === "read") this.read(cx);
      }
    });
  }

  runAll(cx) {
    if (this.busy) return;
    const f = focused();
    const pane = f?.pane ?? panes().find((p) => p.terminal)?.pane ?? null;
    this.busy = "starting";
    cx.spawn(async (cx) => {
      const r = new Run(new Date().toISOString().replace(/[:.]/g, "-"));
      setShared(LAST_RUN, r.id);
      this.run = r;
      for (const step of STEPS) {
        this.busy = step.id;
        cx.notify();
        r.data.steps[step.id] = { status: "running" };
        await r.save();
        try {
          r.data.steps[step.id] = { status: "done", result: await step.run({ r, cx, pane }) };
        } catch (e) {
          r.data.steps[step.id] = { status: "failed", error: String(e?.stack ?? e) };
        }
        await r.save();
      }
      this.busy = null;
      cx.notify();
    });
  }

  // For the app-quit check: a 5-minute daemon whose ticks say whether the
  // app was running (WINDOWS-CHECKLIST.md, step 1).
  survive(cx) {
    cx.spawn(async (cx) => {
      const r = this.run ?? new Run(new Date().toISOString().replace(/[:.]/g, "-"));
      setShared(LAST_RUN, r.id);
      this.run = r;
      const spawned = await r.probe(probeExe(), "spawn-detached", ["300"], "survive");
      r.data.survive = { started_ms: Date.now(), spawned };
      await r.save();
      cx.notify();
    });
  }

  read(cx) {
    cx.spawn(async (cx) => {
      if (!this.run) return;
      await this.run.reconcile();
      await this.run.save();
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    const button = (id, label, onClick) => h_flex().id(id).h(28).px(12).items_center().rounded(6)
      .border_1().border_color(t.input).bg(t.raised).text_size(12).cursor_pointer()
      .hover((s) => s.opacity(0.88)).on_click((_e, cx) => onClick(cx)).child(label);
    const steps = this.run?.data.steps ?? {};
    const line = (step) => {
      const s = steps[step.id];
      let text = step.about;
      if (s?.status === "done") {
        try { text = step.sum(s.result); } catch (e) { text = `summary failed: ${e}`; }
      } else if (s?.status === "failed") text = s.error.split("\n")[0];
      const mark = { done: "✓", failed: "✗", running: "…" }[s?.status] ?? "·";
      return h_flex().gap(8).items_start()
        .child(div().w(14).flex_none().text_color(s?.status === "failed" ? t.blocked : t.muted).child(mark))
        .child(div().w(96).flex_none().text_color(t.foreground).child(step.id))
        .child(div().flex_1().min_w(0).text_color(t.muted).child(text));
    };
    const survive = this.run?.data.survive;
    return v_flex().size_full().p(14).gap(10).bg(t.surface).text_color(t.foreground).text_size(12.5)
      .child(div().text_size(15).font_semibold().child(`winprobe · ${window()?.platform}`))
      .child(h_flex().gap(8)
        .child(button("wp-run", this.busy ? `Running ${this.busy}…` : "Run all", (cx) => this.runAll(cx)))
        .child(button("wp-survive", "Survive app quit", (cx) => this.survive(cx)))
        .child(button("wp-read", "Read log", (cx) => this.read(cx))))
      .child(v_flex().gap(6).children(STEPS.map(line)))
      .child(div().text_color(t.muted).child(survive
        ? `survive: ${survive.ticks ?? 0} ticks, ${survive.ticks_without_app ?? 0} after the app that started it quit, last ${survive.last_tick_age_s ?? "?"} s ago`
        : "survive: not started"))
      .child(div().text_size(11).text_color(t.dim).child(this.run ? this.run.file : "No run yet."));
  }
}
