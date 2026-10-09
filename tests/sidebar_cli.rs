mod support;

#[cfg(unix)]
mod unix {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::support::{nvim_available, Daemon, Sandbox};

    #[test]
    fn sidebar_refreshes_reused_daemon_pane_identity_before_attach() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }

        // Keep the name short: the daemon socket lives under it, and macOS
        // caps socket paths at 104 bytes (nvim >= 0.12.5 refuses longer).
        let sandbox = Sandbox::new("sidebar-pane-id");
        let daemon = Daemon::spawn(&sandbox, "w1:t1");
        daemon
            .eval("setenv('HERDR_PANE_ID', 'w1:p2')")
            .expect("seed stale pane id");
        assert_eq!(daemon.eval("$HERDR_PANE_ID").as_deref(), Some("w1:p2"));
        let daemon_pid = daemon.eval("getpid()").expect("daemon pid");

        let attached_pane = sandbox.dir.join("attached-pane-id");
        let wrapper = sandbox.dir.join("bin").join("nvim-sidebar-test");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\n\
                 if [ \"$3\" = \"--remote-ui\" ]; then\n\
                   nvim --headless --server \"$2\" --remote-expr '$HERDR_PANE_ID' > '{}'\n\
                   exit $?\n\
                 fi\n\
                 exec nvim \"$@\"\n",
                attached_pane.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();

        let config_dir = sandbox.dir.join("xdg-config").join("herdr-nvim");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.toml"),
            format!(
                "[sidebar]\nnvim_bin = {:?}\n",
                wrapper.display().to_string()
            ),
        )
        .unwrap();

        let output = sandbox.run_with_env(
            &["sidebar"],
            None,
            &[("HERDR_TAB_ID", "w1:t1"), ("HERDR_PANE_ID", "w1:p3")],
        );
        assert!(
            output.status.success(),
            "sidebar failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(attached_pane).unwrap().trim(), "w1:p3");
        assert_eq!(
            daemon.eval("getpid()").as_deref(),
            Some(daemon_pid.as_str()),
            "sidebar must reuse the existing per-tab daemon"
        );
    }
}
