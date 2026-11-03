//! Tab nvims started from an older runtime ("stale"). A daemon outlives the
//! pair closing (`cmd-e` reattaches to it), so after an install it keeps the
//! Lua it loaded until it restarts. Each daemon records the runtime stamp it
//! was started with (`daemon::STAMP_ENV`), and:
//!
//! * an install never stops one: it reports how many are stale
//!   (`report_after_install`);
//! * (re)opening a tab's pair restarts a stale daemon first, unless it has
//!   unsaved buffers or pending comments: then it is kept, and the person is
//!   told to save and reopen (`refresh_on_open`'s notice: a toast from the mod
//!   and a message in nvim).

use std::{path::Path, time::Duration};

use anyhow::Result;

use crate::{
    config,
    daemon::{
        self,
        inventory::{self, Probe},
        registry,
    },
    state::tab_key,
};

/// How long the notice's `vim.notify` request may take: a daemon at a
/// prompt answers nothing.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// What to do with a tab's daemon when its pair opens.
#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    /// Current, or nothing to compare with.
    Keep,
    /// Stale and clean: restart it on the installed runtime.
    Restart,
    /// Stale with unsaved work: keep it and say so.
    KeepDirty,
}

/// Pure: whether a daemon may hold work a restart would lose. Work it
/// couldn't count is work kept.
fn has_work(look: &Probe) -> bool {
    look.unsaved != Some(0) || look.comments != Some(0)
}

/// Pure: the plan for a daemon that answered the probe, under the installed
/// runtime `current`.
pub(crate) fn plan(current: Option<&str>, look: &Probe) -> Plan {
    match current {
        Some(current) if look.stamp != current => {
            if has_work(look) {
                Plan::KeepDirty
            } else {
                Plan::Restart
            }
        }
        _ => Plan::Keep,
    }
}

/// Pure: the notice for a stale daemon that was kept.
/// What nvim itself shows (one line; the toast has `dirty_notice`).
const SHORT_NOTICE: &str =
    "herdr-nvim: kept the old version for your unsaved work; save (:wa), then reopen nvim";

pub(crate) fn dirty_notice(look: &Probe) -> String {
    let count = |n: Option<u32>, what: &str| match n {
        Some(0) => None,
        Some(n) => Some(format!("{n} {what}(s)")),
        None => Some(format!("{what}s it couldn't count")),
    };
    let work: Vec<String> = [
        count(look.unsaved, "unsaved buffer"),
        count(look.comments, "pending comment"),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!(
        "This tab's nvim still runs the herdr-nvim from before the last install, and it has {}, \
         so it was kept. Save (:wa) and send or clear the comments, then close and reopen nvim \
         (cmd-e twice) to update it.",
        work.join(" and ")
    )
}

/// Before a tab's pair opens: restart its daemon if it is stale and clean;
/// keep a stale one with unsaved work, and return the notice for the
/// caller's output (nvim shows it too, once its window attaches).
pub(crate) fn refresh_on_open(tab: &str, cwd: &Path) -> Result<Option<String>> {
    let plugin_root = daemon::plugin_root()?;
    let current = daemon::runtime_stamp(&plugin_root);
    let sidebar = config::load().sidebar;
    let key = tab_key(tab);
    let Some(look) = current
        .as_ref()
        .and_then(|_| inventory::probe(&key, &sidebar))
    else {
        return Ok(None);
    };
    match plan(current.as_deref(), &look) {
        Plan::Keep => Ok(None),
        Plan::Restart => {
            registry::all_stopped(registry::stop_keys(&[key], &sidebar))?;
            daemon::ensure_daemon(tab, &plugin_root, &config::load(), cwd)?;
            eprintln!("herdr-nvim: restarted tab {tab}'s nvim on the installed runtime");
            Ok(None)
        }
        Plan::KeepDirty => {
            let notice = dirty_notice(&look);
            // In nvim, one line that fits beside the ruler: a longer message
            // waits at a hit-enter prompt, and no request gets past that (the
            // tree's operations would fail). The app's toast says it all;
            // `g:herdr_nvim_kept` keeps the whole line.
            let expr = format!(
                "luaeval(\"(function(m) vim.g.herdr_nvim_kept = m; \
                 vim.api.nvim_create_autocmd('UIEnter', {{once = true, callback = \
                 function() vim.schedule(function() local w = vim.o.columns - 25; \
                 if vim.fn.strdisplaywidth(m) > w then m = vim.fn.strcharpart(m, 0, w) .. '…' end; \
                 vim.api.nvim_echo({{{{m, 'WarningMsg'}}}}, true, {{}}) end) end}}) end)(_A)\", {})",
                daemon::vim_string_literal(SHORT_NOTICE)
            );
            let socket = registry::socket_path(tab);
            let _ = daemon::remote_expr_within(&socket, &expr, &sidebar, ASK_WITHIN);
            Ok(Some(notice))
        }
    }
}

/// After an install: how many running nvims are stale (`inventory`'s
/// `stale`). They keep running; each restarts the next time its pair opens.
pub(crate) fn report_after_install() {
    let sidebar = config::load().sidebar;
    let keys = registry::daemon_keys().unwrap_or_default();
    let running: Vec<inventory::DaemonInfo> = inventory::describe(keys, None, &sidebar)
        .into_iter()
        .filter(|info| info.probe.is_some())
        .collect();
    let stale: Vec<&inventory::DaemonInfo> = running.iter().filter(|info| info.stale).collect();
    if stale.is_empty() {
        println!(
            "nvim: {} running, none older than this install",
            running.len()
        );
        return;
    }
    let dirty = stale
        .iter()
        .filter(|info| info.probe.as_ref().is_some_and(has_work))
        .count();
    println!(
        "nvim: {} of {} running tab nvim(s) are stale (they run the herdr-nvim from before this \
         install; left running): {}",
        stale.len(),
        running.len(),
        stale
            .iter()
            .map(|info| info.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "nvim: each restarts on the new one the next time its pair opens (cmd-e or Files, twice){}",
        if dirty > 0 {
            format!("; {dirty} with unsaved work stay(s) until saved")
        } else {
            String::new()
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(stamp: &str, unsaved: u32, comments: u32) -> Probe {
        Probe {
            pid: Some(1),
            unsaved: Some(unsaved),
            comments: Some(comments),
            tab_id: None,
            stamp: stamp.to_owned(),
        }
    }

    #[test]
    fn only_a_stale_clean_daemon_restarts() {
        assert_eq!(plan(Some("new"), &look("new", 3, 1)), Plan::Keep);
        assert_eq!(plan(Some("new"), &look("old", 0, 0)), Plan::Restart);
        assert_eq!(plan(Some("new"), &look("", 0, 0)), Plan::Restart);
        assert_eq!(plan(Some("new"), &look("old", 1, 0)), Plan::KeepDirty);
        assert_eq!(plan(Some("new"), &look("old", 0, 2)), Plan::KeepDirty);
        assert_eq!(plan(None, &look("old", 0, 0)), Plan::Keep, "a repo build");
        let uncounted = Probe {
            unsaved: None,
            ..look("old", 0, 0)
        };
        assert_eq!(plan(Some("new"), &uncounted), Plan::KeepDirty);
    }

    #[test]
    fn the_notice_counts_the_unsaved_work() {
        let text = dirty_notice(&look("old", 2, 1));
        assert!(
            text.contains("2 unsaved buffer(s) and 1 pending comment(s)"),
            "{text}"
        );
    }
}
