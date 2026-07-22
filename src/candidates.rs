//! The merge/dedup/order pipeline that turns three read-only sources
//! (session mining, git worktree status, terminal scrape) into the final,
//! flat, recency-ordered candidate list the picker renders. Pure: every
//! I/O-shaped input (git status/log results, existence checks) is passed in
//! already computed or as an injected closure, so this module has no I/O of
//! its own and is fully unit tested.
//!
//! There is no section split here (no EDITED/MENTIONED grouping) -- every
//! touched-this-session file is one flat list, ordered most-recently-touched
//! first. `Candidate.is_edit` flags entries that are real, currently-
//! relevant edits (used by the picker to decide whether to show a diff
//! stat), but every entry -- edit or not -- lives in the same list.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{extract, gitscan, sessions};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub path: String,
    pub line: Option<u32>,
    /// True iff this path both has a session edit event (or is a git-only
    /// dirty file the session miner never saw at all) AND currently passes
    /// `gitscan::should_keep_edited`'s dirty-or-committed-in-session-or-
    /// non-git check. A session-edited file that was rolled back (net-change
    /// demoted) still appears in the list, just with `is_edit: false` and no
    /// diff stat -- it is never dropped.
    pub is_edit: bool,
    pub newly_created: bool,
    /// Unix timestamp of the most recent touch (read OR edit) of this path,
    /// when known. Drives the list's recency ordering. `None` for entries
    /// with no known timestamp (e.g. scrape-fallback candidates); those sort
    /// after every timestamped entry.
    pub touched_unix: Option<u64>,
    /// Combined (added, removed) line counts from `git diff --numstat` for
    /// `is_edit` entries that are git-tracked and currently dirty. `None`
    /// for newly-created files (the `new` badge covers those), entries kept
    /// in the list only because they were committed during the session (now
    /// clean -- no diff to show), non-git files, and all non-edit entries.
    /// Populated by `bridge::gather_candidates` (I/O), never by this pure
    /// module.
    pub diff_stat: Option<(u32, u32)>,
}

pub struct GitOnlyEdit {
    pub path: String,
    pub mtime_unix: Option<u64>,
}

pub struct BuildInput<'a> {
    pub mined_touches: &'a [sessions::MinedTouch],
    pub session_start_unix: Option<u64>,
    pub git_dirty: &'a HashSet<String>,
    pub git_committed_in_session: &'a HashSet<String>,
    pub in_git_worktree: &'a dyn Fn(&str) -> bool,
    pub git_only_dirty_not_mined: &'a [GitOnlyEdit],
    pub scraped_mentioned: &'a [extract::ScrapedPath],
    pub exists: &'a dyn Fn(&str) -> bool,
}

/// Build the single flat, deduped, recency-ordered candidate list. Not
/// capped here -- the picker's default view caps to `max_files`, but a
/// non-empty filter query still searches this full uncapped list, so
/// capping must not happen at this layer.
pub fn build_candidates(input: BuildInput) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    for touch in input.mined_touches {
        let in_repo = (input.in_git_worktree)(&touch.path);
        let dirty = input.git_dirty.contains(&touch.path);
        let committed = input.git_committed_in_session.contains(&touch.path);
        let is_edit = touch.was_edited && gitscan::should_keep_edited(in_repo, dirty, committed);
        out.push(Candidate {
            path: touch.path.clone(),
            line: None,
            is_edit,
            newly_created: touch.newly_created,
            touched_unix: touch.last_touch_unix,
            diff_stat: None,
        });
    }

    for git_only in input.git_only_dirty_not_mined {
        // Git-only entries are, by construction, *already known dirty*
        // paths the session miner never touched at all (see
        // bridge::gather_candidates) -- e.g. a bash `sed -i` the agent ran.
        // `dirty` is therefore always true here (that's what "git-only
        // *dirty*" means), unlike the mined-touch loop above where dirtiness
        // still needs to be looked up per path.
        let in_repo = (input.in_git_worktree)(&git_only.path);
        let is_edit = gitscan::should_keep_edited(in_repo, true, false);
        out.push(Candidate {
            path: git_only.path.clone(),
            line: None,
            is_edit,
            newly_created: false,
            touched_unix: git_only.mtime_unix,
            diff_stat: None,
        });
    }

    // Scrape fallback: only used when session mining produced nothing at
    // all (no agent_session tracked, or the tracked agent has no parser) --
    // otherwise the touches above are strictly better data.
    if input.mined_touches.is_empty() {
        let mut seen: HashSet<String> = HashSet::new();
        for scraped in input.scraped_mentioned {
            if !seen.insert(scraped.path.clone()) {
                continue;
            }
            out.push(Candidate {
                path: scraped.path.clone(),
                line: scraped.line,
                is_edit: false,
                newly_created: false,
                touched_unix: None,
                diff_stat: None,
            });
        }
    }

    out.retain(|c| (input.exists)(&c.path));

    // Descending by touched_unix; `None` sorts after every `Some` (Option's
    // derived Ord puts `None` first ascending, so `b.cmp(&a)` -- descending
    // -- puts it last), and the sort is stable so entries that tie (e.g.
    // multiple `None`s) keep their original relative source order.
    out.sort_by(|a, b| b.touched_unix.cmp(&a.touched_unix));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::MinedTouch;
    use std::collections::HashSet;

    fn always_true(_: &str) -> bool {
        true
    }
    fn always_false(_: &str) -> bool {
        false
    }

    fn touch(path: &str, was_edited: bool, last_touch_unix: Option<u64>) -> MinedTouch {
        MinedTouch {
            path: path.into(),
            was_edited,
            newly_created: false,
            last_touch_unix,
        }
    }

    fn base_input<'a>(
        mined_touches: &'a [MinedTouch],
        git_dirty: &'a HashSet<String>,
        git_committed: &'a HashSet<String>,
        in_worktree: &'a dyn Fn(&str) -> bool,
    ) -> BuildInput<'a> {
        BuildInput {
            mined_touches,
            session_start_unix: Some(1000),
            git_dirty,
            git_committed_in_session: git_committed,
            in_git_worktree: in_worktree,
            git_only_dirty_not_mined: &[],
            scraped_mentioned: &[],
            exists: &always_true,
        }
    }

    #[test]
    fn dirty_edited_touch_is_marked_as_edit() {
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let out = build_candidates(base_input(&touches, &dirty, &committed, &always_true));
        assert_eq!(out.len(), 1);
        assert!(out[0].is_edit);
    }

    #[test]
    fn committed_in_session_touch_is_marked_as_edit() {
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::new();
        let committed = HashSet::from(["/repo/a.rs".to_owned()]);
        let out = build_candidates(base_input(&touches, &dirty, &committed, &always_true));
        assert!(out[0].is_edit);
    }

    #[test]
    fn clean_and_uncommitted_edit_is_not_marked_as_edit_but_still_present() {
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::new();
        let committed = HashSet::new();
        let out = build_candidates(base_input(&touches, &dirty, &committed, &always_true));
        assert_eq!(out.len(), 1, "net-change-demoted entries are never dropped");
        assert!(!out[0].is_edit);
    }

    #[test]
    fn non_git_edit_is_always_marked_as_edit() {
        let touches = [touch("/home/u/.config/foo.toml", true, Some(5))];
        let dirty = HashSet::new();
        let committed = HashSet::new();
        let out = build_candidates(base_input(&touches, &dirty, &committed, &always_false));
        assert!(out[0].is_edit);
    }

    #[test]
    fn read_only_touch_is_present_but_not_marked_as_edit() {
        let touches = [touch("/repo/a.rs", false, Some(5))];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let out = build_candidates(base_input(&touches, &dirty, &committed, &always_true));
        assert_eq!(out.len(), 1);
        assert!(
            !out[0].is_edit,
            "a read-only touch is never an edit, dirty or not"
        );
        assert_eq!(out[0].touched_unix, Some(5));
    }

    #[test]
    fn git_only_dirty_file_is_added_as_edit_with_mtime() {
        let input = BuildInput {
            mined_touches: &[],
            session_start_unix: None,
            git_dirty: &HashSet::new(),
            git_committed_in_session: &HashSet::new(),
            in_git_worktree: &always_true,
            git_only_dirty_not_mined: &[GitOnlyEdit {
                path: "/repo/sed_edited.rs".into(),
                mtime_unix: Some(42),
            }],
            scraped_mentioned: &[],
            exists: &always_true,
        };
        let out = build_candidates(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/repo/sed_edited.rs");
        assert!(out[0].is_edit);
        assert_eq!(out[0].touched_unix, Some(42));
    }

    #[test]
    fn scrape_fallback_used_only_when_no_mined_data_at_all() {
        use crate::extract::ScrapedPath;
        let scraped = [ScrapedPath {
            path: "/repo/scraped.rs".into(),
            line: None,
        }];
        let input = BuildInput {
            mined_touches: &[],
            session_start_unix: None,
            git_dirty: &HashSet::new(),
            git_committed_in_session: &HashSet::new(),
            in_git_worktree: &always_true,
            git_only_dirty_not_mined: &[],
            scraped_mentioned: &scraped,
            exists: &always_true,
        };
        let out = build_candidates(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/repo/scraped.rs");
        assert!(!out[0].is_edit);
    }

    #[test]
    fn scrape_fallback_is_not_used_when_any_touch_exists() {
        use crate::extract::ScrapedPath;
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let scraped = [ScrapedPath {
            path: "/repo/scraped.rs".into(),
            line: None,
        }];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        input.scraped_mentioned = &scraped;
        let out = build_candidates(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/repo/a.rs");
    }

    #[test]
    fn ordered_newest_first_by_touched_unix() {
        let touches = [
            touch("/repo/old.rs", true, Some(1)),
            touch("/repo/new.rs", true, Some(99)),
        ];
        let dirty = HashSet::from(["/repo/old.rs".to_owned(), "/repo/new.rs".to_owned()]);
        let out = build_candidates(base_input(&touches, &dirty, &HashSet::new(), &always_true));
        assert_eq!(out[0].path, "/repo/new.rs");
        assert_eq!(out[1].path, "/repo/old.rs");
    }

    #[test]
    fn untimed_entries_sort_last_preserving_relative_order() {
        let touches = [
            touch("/repo/no_ts_a.rs", true, None),
            touch("/repo/timed.rs", true, Some(5)),
            touch("/repo/no_ts_b.rs", true, None),
        ];
        let dirty = HashSet::from([
            "/repo/no_ts_a.rs".to_owned(),
            "/repo/timed.rs".to_owned(),
            "/repo/no_ts_b.rs".to_owned(),
        ]);
        let out = build_candidates(base_input(&touches, &dirty, &HashSet::new(), &always_true));
        assert_eq!(out[0].path, "/repo/timed.rs");
        assert_eq!(out[1].path, "/repo/no_ts_a.rs");
        assert_eq!(out[2].path, "/repo/no_ts_b.rs");
    }

    #[test]
    fn nonexistent_paths_are_filtered_out() {
        let touches = [touch("/repo/ghost.rs", true, Some(5))];
        let dirty = HashSet::from(["/repo/ghost.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        input.exists = &always_false;
        let out = build_candidates(input);
        assert!(out.is_empty());
    }
}
