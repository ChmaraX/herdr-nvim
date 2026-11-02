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

use std::{path::Path, sync::Mutex, time::Duration};

use anyhow::Result;

use crate::{
    config,
    daemon::{
        self,
        inventory::COMMENTS_EXPR,
        registry::{self, UNSAVED_BUFFERS_EXPR},
        STAMP_ENV,
    },
    state::tab_key,
};

/// How long a daemon gets to answer: one at a prompt answers nothing.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// What a running daemon says: its stamp, unsaved buffers, pending comments.
#[derive(Debug, PartialEq)]
pub(crate) struct Look {
    pub stamp: String,
    pub unsaved: u32,
    pub comments: u32,
}

/// Pure: parse `stamp|unsaved|comments`.
pub(crate) fn parse_look(out: &str) -> Option<Look> {
    let mut parts = out.trim().split('|');
    Some(Look {
        stamp: parts.next()?.to_owned(),
        unsaved: parts.next()?.trim().parse().ok()?,
        comments: parts.next()?.trim().parse().ok()?,
    })
}

fn look(socket: &Path) -> Option<Look> {
    let expr = format!("${STAMP_ENV} . '|' . {UNSAVED_BUFFERS_EXPR} . '|' . {COMMENTS_EXPR}");
    let sidebar = config::load().sidebar;
    parse_look(&daemon::remote_expr_within(
        socket, &expr, &sidebar, ASK_WITHIN,
    )?)
}

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

/// Pure: the plan for a daemon that answered `look`, under the installed
/// runtime `current`.
pub(crate) fn plan(current: Option<&str>, look: &Look) -> Plan {
    match current {
        Some(current) if look.stamp != current => {
            if look.unsaved == 0 && look.comments == 0 {
                Plan::Restart
            } else {
                Plan::KeepDirty
            }
        }
        _ => Plan::Keep,
    }
}

/// Pure: the notice for a stale daemon that was kept.
pub(crate) fn dirty_notice(look: &Look) -> String {
    let mut work = Vec::new();
    if look.unsaved > 0 {
        work.push(format!("{} unsaved buffer(s)", look.unsaved));
    }
    if look.comments > 0 {
        work.push(format!("{} pending comment(s)", look.comments));
    }
    format!(
        "This tab's nvim still runs the herdr-nvim from before the last install, and it has {}, \
         so it was kept. Save (:wa) and send or clear the comments, then close and reopen nvim \
         (cmd-e twice) to update it.",
        work.join(" and ")
    )
}

static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// The notice `refresh_on_open` left for this command's output, if any.
pub(crate) fn take_notice() -> Option<String> {
    NOTICE.lock().ok()?.take()
}

/// Before a tab's pair opens: restart its daemon if it is stale and clean;
/// keep a stale one with unsaved work, leaving a notice for the caller's
/// output and for nvim to show once its window attaches.
pub(crate) fn refresh_on_open(tab: &str, cwd: &Path) -> Result<()> {
    let plugin_root = daemon::plugin_root()?;
    let current = daemon::runtime_stamp(&plugin_root);
    let socket = registry::socket_path(tab);
    let Some(look) = current.as_ref().and_then(|_| look(&socket)) else {
        return Ok(());
    };
    match plan(current.as_deref(), &look) {
        Plan::Keep => Ok(()),
        Plan::Restart => {
            let sidebar = config::load().sidebar;
            registry::all_stopped(registry::stop_keys(&[tab_key(tab)], &sidebar))?;
            daemon::ensure_daemon(tab, &plugin_root, &config::load(), cwd)?;
            eprintln!("herdr-nvim: restarted tab {tab}'s nvim on the installed runtime");
            Ok(())
        }
        Plan::KeepDirty => {
            let notice = dirty_notice(&look);
            let expr = format!(
                "luaeval(\"vim.api.nvim_create_autocmd('UIEnter', {{once = true, callback = \
                 function() vim.schedule(function() vim.notify(_A, vim.log.levels.WARN) end) end}})\", {})",
                daemon::vim_string_literal(&notice)
            );
            let _ = daemon::remote_expr_within(&socket, &expr, &config::load().sidebar, ASK_WITHIN);
            if let Ok(mut slot) = NOTICE.lock() {
                *slot = Some(notice);
            }
            Ok(())
        }
    }
}

/// After an install: how many running nvims are stale. They keep running;
/// each restarts the next time its pair opens.
pub(crate) fn report_after_install() {
    let Some(current) = daemon::plugin_root()
        .ok()
        .and_then(|root| daemon::runtime_stamp(&root))
    else {
        return;
    };
    let keys = registry::daemon_keys().unwrap_or_default();
    let looks: Vec<(String, Look)> = keys
        .into_iter()
        .filter_map(|key| Some((key.clone(), look(&registry::socket_path_for_key(&key))?)))
        .collect();
    let stale: Vec<&(String, Look)> = looks.iter().filter(|(_, l)| l.stamp != current).collect();
    if stale.is_empty() {
        println!(
            "nvim: {} running, none older than this install",
            looks.len()
        );
        return;
    }
    let dirty = stale
        .iter()
        .filter(|(_, l)| l.unsaved > 0 || l.comments > 0)
        .count();
    println!(
        "nvim: {} of {} running tab nvim(s) are stale (they run the herdr-nvim from before this \
         install; left running): {}",
        stale.len(),
        looks.len(),
        stale
            .iter()
            .map(|(key, _)| key.as_str())
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

    fn look(stamp: &str, unsaved: u32, comments: u32) -> Look {
        Look {
            stamp: stamp.to_owned(),
            unsaved,
            comments,
        }
    }

    #[test]
    fn parses_what_the_daemon_answers_even_without_a_stamp() {
        assert_eq!(parse_look("ab12|0|2\n"), Some(look("ab12", 0, 2)));
        assert_eq!(
            parse_look("|1|0"),
            Some(look("", 1, 0)),
            "older than stamps"
        );
        assert_eq!(parse_look("garbage"), None);
    }

    #[test]
    fn only_a_stale_clean_daemon_restarts() {
        assert_eq!(plan(Some("new"), &look("new", 3, 1)), Plan::Keep);
        assert_eq!(plan(Some("new"), &look("old", 0, 0)), Plan::Restart);
        assert_eq!(plan(Some("new"), &look("", 0, 0)), Plan::Restart);
        assert_eq!(plan(Some("new"), &look("old", 1, 0)), Plan::KeepDirty);
        assert_eq!(plan(Some("new"), &look("old", 0, 2)), Plan::KeepDirty);
        assert_eq!(plan(None, &look("old", 0, 0)), Plan::Keep, "a repo build");
    }

    #[test]
    fn the_notice_says_what_to_save() {
        let text = dirty_notice(&look("old", 2, 1));
        assert!(
            text.contains("2 unsaved buffer(s) and 1 pending comment(s)"),
            "{text}"
        );
        assert!(text.contains("cmd-e twice"), "{text}");
    }
}
