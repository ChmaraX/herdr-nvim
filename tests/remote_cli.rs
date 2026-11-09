mod support;

#[cfg(unix)]
mod unix {
    use super::support::Sandbox;
    use serde_json::{json, Value};
    use std::{fs, os::unix::fs::PermissionsExt, process::Output};

    struct Fixture {
        sandbox: Sandbox,
    }

    impl Fixture {
        fn new(name: &str, shown: bool, only_editor: bool) -> Self {
            let sandbox = Sandbox::new(name);
            let root = &sandbox.dir;
            let shell = json!({"id":"p7","content":{"terminal":"t7"}});
            let editor = json!({"id":"p9","content":{"terminal":"t9"}});
            let children = if only_editor {
                vec![editor]
            } else if shown {
                vec![shell, editor]
            } else {
                vec![shell]
            };
            let doc = json!({"version":1,"value":{"spaces":[{"id":"s1","tabs":[{"id":"t1","root":{"id":"root","children":children}}]}]}});
            let terminals = if shown {
                json!([{"address":"t9","state":"live","labels":{"herdr-nvim.remote-role":"ui"}}])
            } else {
                json!([])
            };
            fs::write(
                root.join("info.json"),
                json!({"doc":doc,"terminals":terminals,"tab_root":"/remote","running":shown})
                    .to_string(),
            )
            .unwrap();
            fs::write(root.join("state.json"), json!({"running":true,"nvim":"/remote/bin/nvim","socket":"/remote/editor.sock","modified":[]}).to_string()).unwrap();
            if shown {
                fs::write(root.join("visible"), "").unwrap();
            }
            for (name, text) in [
                (
                    "hc",
                    format!(
                        r#"#!/bin/sh
printf '%s\n' "$*" >> '{0}/hc.log'
case "$1" in
  exec)
    test "$2" = --on && test "$3" = box || exit 21
    test ! -f '{0}/offline' || {{ echo 'machine is unavailable' >&2; exit 1; }}
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -c) printf '{{"worker":"/remote/worker.lua"}}\n'; exit 0 ;;
        -l)
          shift; shift
          case "$1" in
            info) cat '{0}/info.json' ;;
            open) cat '{0}/state.json' ;;
            stop) printf '{{"running":false}}\n' ;;
            current) printf '{{"file":"/remote/file.md","watch":null,"tab_root":"/remote"}}\n' ;;
            *) exit 22 ;;
          esac
          exit 0 ;;
      esac
      shift
    done
    exit 23 ;;
  label)
    test ! -f '{0}/fail-label' || {{ echo 'label failed' >&2; exit 1; }}
    printf '{{"ok":true}}\n' ;;
  kill) rm -f '{0}/visible'; printf '{{"ok":true}}\n' ;;
  *) echo 'local core calls are forbidden in this fixture' >&2; exit 24 ;;
esac
"#,
                        root.display()
                    ),
                ),
                (
                    "herdr-app",
                    format!(
                        r#"#!/bin/sh
printf '%s\n' "$*" >> '{0}/app.log'
case "$1" in
  do)
    case "$2" in
      pane.split) touch '{0}/visible'; printf '{{"terminal":"box/t9","pane":"box/p9"}}\n' ;;
      terminal.keep) touch '{0}/visible'; printf '{{"ok":true}}\n' ;;
      terminal.park) rm -f '{0}/visible'; printf '{{"ok":true}}\n' ;;
      *) printf '{{"ok":true}}\n' ;;
    esac ;;
  status) printf '{{"space":"box/s1","tab":"box/t1","focused_pane":"box/p7","agent":"local/t0"}}\n' ;;
  ls)
    terms='[{{"terminal":"box/t7","pane":"box/p7"}}]'
    test ! -f '{0}/visible' || terms='[{{"terminal":"box/t7","pane":"box/p7"}},{{"terminal":"box/t9","pane":"box/p9"}}]'
    printf '{{"spaces":[{{"id":"box/s1","tabs":[{{"id":"box/t1","terminals":%s}}]}}]}}\n' "$terms" ;;
  *) exit 25 ;;
esac
"#,
                        root.display()
                    ),
                ),
            ] {
                let path = root.join("bin").join(name);
                fs::write(&path, text).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            Self { sandbox }
        }

        fn run(&self, mode: &str, ctx: Value, rest: &[&str]) -> Output {
            let text = ctx.to_string();
            let mut args = vec![mode];
            args.extend_from_slice(rest);
            args.extend(["--ctx", &text]);
            let app = self.sandbox.dir.join("bin/herdr-app");
            let hc = self.sandbox.dir.join("bin/hc");
            self.sandbox.run_with_env(
                &args,
                None,
                &[
                    ("HERDR_NVIM_HOST", "app"),
                    ("HERDR_APP_BIN", app.to_str().unwrap()),
                    ("HERDR_NVIM_HC", hc.to_str().unwrap()),
                ],
            )
        }

        fn ctx(&self) -> Value {
            json!({"space":"box/s1","tab":"box/t1","terminal":"box/t7","cwd":"/remote"})
        }
        fn log(&self, name: &str) -> String {
            fs::read_to_string(self.sandbox.dir.join(name)).unwrap_or_default()
        }
    }

    #[test]
    fn opens_a_remote_file_without_testing_it_on_the_desktop() {
        let f = Fixture::new("remote-open", false, false);
        let result = f.run("open", f.ctx(), &["/remote/file.md:12:3"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["terminal"],
            "box/t9"
        );
        let app = f.log("app.log");
        assert!(app.contains("\"pane\":\"box/root\""), "{app}");
        assert!(app.contains("/remote/editor.sock"), "{app}");
        let hc = f.log("hc.log");
        assert!(hc.contains("exec --on box"), "{hc}");
        assert!(
            hc.contains("\"line\":12") && hc.contains("\"col\":3"),
            "{hc}"
        );
        assert!(hc.contains("herdr-nvim.sidebar=t1"), "{hc}");
    }

    #[test]
    fn hiding_parks_only_the_machine_qualified_ui_terminal() {
        let f = Fixture::new("remote-hide", true, false);
        let result = f.run("toggle", f.ctx(), &["--state", "close"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["closed"],
            "box/t9"
        );
        let app = f.log("app.log");
        assert!(
            app.contains("terminal.park {\"terminal\":\"box/t9\"}"),
            "{app}"
        );
        assert!(!f.log("hc.log").contains("kill"));
    }

    #[test]
    fn mixed_machine_context_fails_before_any_remote_or_window_operation() {
        let f = Fixture::new("remote-wrong-machine", false, false);
        let mut ctx = f.ctx();
        ctx["space"] = json!("other/s1");
        let result = f.run("open", ctx, &["/remote/file"]);
        assert!(!result.status.success());
        assert!(f.log("hc.log").is_empty());
        assert!(f.log("app.log").is_empty());
    }

    #[test]
    fn unavailable_remote_never_falls_back_to_the_local_core() {
        let f = Fixture::new("remote-offline", false, false);
        fs::write(f.sandbox.dir.join("offline"), "").unwrap();
        let result = f.run("open", f.ctx(), &["/remote/file"]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("unavailable"));
        assert!(f.log("app.log").is_empty());
        assert!(!f.log("hc.log").contains("connect"));
    }

    #[test]
    fn cannot_hide_the_only_terminal_and_accidentally_close_its_tab() {
        let f = Fixture::new("remote-only-pane", true, true);
        let result = f.run("toggle", f.ctx(), &["--state", "close"]);
        assert!(!result.status.success());
        assert!(f.log("app.log").is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("only pane"));
    }

    #[test]
    fn reopening_a_parked_screen_waits_for_adoption_and_reuses_its_address() {
        let f = Fixture::new("remote-reopen", false, false);
        let path = f.sandbox.dir.join("info.json");
        let mut info: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        info["terminals"] =
            json!([{"address":"t9","state":"live","labels":{"herdr-nvim.remote-role":"ui"}}]);
        fs::write(path, info.to_string()).unwrap();
        let result = f.run("toggle", f.ctx(), &["--state", "open"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["terminal"],
            "box/t9"
        );
        let app = f.log("app.log");
        assert!(
            app.contains("terminal.keep {\"terminal\":\"box/t9\""),
            "{app}"
        );
        assert!(!app.contains("pane.split"), "{app}");
    }

    #[test]
    fn unsupported_remote_features_never_touch_local_files_or_window_structure() {
        let f = Fixture::new("remote-unsupported", false, false);
        for (mode, args) in [
            ("fs", vec![r#"{"op":"defaults"}"#]),
            ("pick-file", vec![]),
            ("app-preview", vec!["--file", "/remote/file"]),
            ("open-link", vec!["/remote/file"]),
            ("app-find", vec!["--root", "/remote"]),
        ] {
            let result = f.run(mode, f.ctx(), &args);
            assert!(!result.status.success());
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("not available"),
                "{mode}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(f.log("hc.log").is_empty());
        assert!(f.log("app.log").is_empty());
    }

    #[test]
    fn failed_labels_remove_only_the_new_ui_client() {
        let f = Fixture::new("remote-label-failure", false, false);
        fs::write(f.sandbox.dir.join("fail-label"), "").unwrap();
        let result = f.run("open", f.ctx(), &["/remote/file"]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("label failed"));
        assert!(f.log("hc.log").contains("kill box/t9"));
        assert!(!f.sandbox.dir.join("visible").exists());
    }

    #[test]
    fn malformed_terminal_records_are_not_silently_dropped() {
        let f = Fixture::new("remote-malformed-terminal", false, false);
        let path = f.sandbox.dir.join("info.json");
        let mut info: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        info["terminals"] = json!([{"state":"live","labels":{"herdr-nvim.remote-role":"ui"}}]);
        fs::write(path, info.to_string()).unwrap();
        assert!(!f.run("open", f.ctx(), &["/remote/file"]).status.success());
        assert!(f.log("app.log").is_empty());
    }

    #[test]
    fn a_mod_pane_is_enough_to_keep_the_tab_when_hiding_its_editor() {
        let f = Fixture::new("remote-mod-sibling", true, true);
        let path = f.sandbox.dir.join("info.json");
        let mut info: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        info["doc"]["value"]["spaces"][0]["tabs"][0]["root"]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"p-mod","content":{"mod":"board"}}));
        fs::write(path, info.to_string()).unwrap();
        assert!(f
            .run("toggle", f.ctx(), &["--state", "close"])
            .status
            .success());
        assert!(!f.sandbox.dir.join("visible").exists());
    }

    #[test]
    fn current_file_does_not_fetch_ui_state_or_change_the_window() {
        let f = Fixture::new("remote-current", false, false);
        let result = f.run("app-current", f.ctx(), &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["file"],
            "/remote/file.md"
        );
        assert!(f.log("app.log").is_empty());
        assert!(!f.log("hc.log").contains("worker.lua info"));
    }

    #[test]
    fn implicit_context_uses_the_remote_focused_pane_not_an_unrelated_agent() {
        let f = Fixture::new("remote-focused-context", false, false);
        let app = f.sandbox.dir.join("bin/herdr-app");
        let hc = f.sandbox.dir.join("bin/hc");
        let result = f.sandbox.run_with_env(
            &["open", "/remote/file"],
            None,
            &[
                ("HERDR_NVIM_HOST", "app"),
                ("HERDR_APP_BIN", app.to_str().unwrap()),
                ("HERDR_NVIM_HC", hc.to_str().unwrap()),
            ],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(f.log("hc.log").contains("\"terminal\":\"t7\""));
        assert!(!f.log("hc.log").contains("local/t0"));
    }

    #[test]
    fn remote_profiles_are_sent_to_the_machine_not_used_locally() {
        let f = Fixture::new("remote-profile", false, false);
        let config = f.sandbox.dir.join("profile.toml");
        fs::write(
            &config,
            "[remote.box]\nnvim_bin='/remote/bin/nvim'\nnvim_env=['NVIM_APPNAME=work']\n",
        )
        .unwrap();
        let app = f.sandbox.dir.join("bin/herdr-app");
        let hc = f.sandbox.dir.join("bin/hc");
        let ctx = f.ctx().to_string();
        let result = f.sandbox.run_with_env(
            &["open", "--ctx", &ctx, "/remote/file"],
            None,
            &[
                ("HERDR_NVIM_HOST", "app"),
                ("HERDR_APP_BIN", app.to_str().unwrap()),
                ("HERDR_NVIM_HC", hc.to_str().unwrap()),
                ("HERDR_NVIM_CONFIG", config.to_str().unwrap()),
            ],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(f
            .log("hc.log")
            .contains("env NVIM_APPNAME=work /remote/bin/nvim"));
        assert!(f.log("app.log").contains("NVIM_APPNAME=work"));
    }
}
