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
    /// True iff this path was actually touched (read or edited) during the
    /// session, or is a currently-dirty git file. Repo-wide entries added
    /// only to widen search (see `repo_files`) are `false`. The picker's
    /// default (empty-query) view shows only `session: true` entries; typing
    /// a query searches every entry, session or not.
    pub session: bool,
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
    pub diff_stat: Option<(u32, u32)>,
}

pub struct BuildInput<'a> {
    pub mined_touches: &'a [sessions::MinedTouch],
    pub first_op_unix: Option<u64>,
    /// Every currently-dirty git path (`git status`), mined or not.
    pub git_dirty: &'a HashSet<String>,
    pub git_committed_in_session: &'a HashSet<String>,
    pub in_git_worktree: &'a dyn Fn(&str) -> bool,
    /// Best-effort mtime lookup for a git-dirty path the session miner never
    /// touched (e.g. a bash `sed -i` the agent ran) -- used as that entry's
    /// `touched_unix` since it has no mined timestamp.
    pub git_mtime_unix: &'a dyn Fn(&str) -> Option<u64>,
    /// Bulk `git diff --numstat` result for the whole worktree, keyed by
    /// path, for the diff-stat eligibility pass below.
    pub diff_stats: &'a std::collections::HashMap<String, (u32, u32)>,
    pub scraped_mentioned: &'a [extract::ScrapedPath],
    /// The whole worktree's file list (`git ls-files`), used purely to widen
    /// search: every entry not already present as a session/git candidate is
    /// appended with `session: false` so it is reachable by a typed query but
    /// hidden from the default view. Empty for non-git cwds.
    pub repo_files: &'a [String],
    pub exists: &'a dyn Fn(&str) -> bool,
}

/// Build the single flat, deduped, recency-ordered candidate list. Not
/// capped here -- the picker's default view caps to `max_files`, but a
/// non-empty filter query still searches this full uncapped list, so
/// capping must not happen at this layer.
pub fn build_candidates(input: BuildInput) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut mined_paths: HashSet<&str> = HashSet::new();

    for touch in input.mined_touches {
        mined_paths.insert(touch.path.as_str());
        let in_repo = (input.in_git_worktree)(&touch.path);
        let dirty = input.git_dirty.contains(&touch.path);
        let committed = input.git_committed_in_session.contains(&touch.path);
        let is_edit = touch.was_edited && gitscan::should_keep_edited(in_repo, dirty, committed);
        out.push(Candidate {
            path: touch.path.clone(),
            line: None,
            is_edit,
            newly_created: touch.newly_created,
            session: true,
            touched_unix: touch.last_touch_unix,
            diff_stat: None,
        });
    }

    // Git-only edits: dirty paths not already covered by session mining --
    // any touch, read or edit alike, since a path we already have session
    // data for must not also get a synthesized duplicate entry (e.g. a bash
    // `sed -i` the agent ran is invisible to tool-call mining and needs
    // one). `dirty` is always true here by construction (that's what
    // "git-only *dirty*" means), unlike the mined-touch loop above where
    // dirtiness still needs to be looked up per path.
    for path in input.git_dirty {
        if mined_paths.contains(path.as_str()) {
            continue;
        }
        let in_repo = (input.in_git_worktree)(path);
        let is_edit = gitscan::should_keep_edited(in_repo, true, false);
        out.push(Candidate {
            path: path.clone(),
            line: None,
            is_edit,
            newly_created: false,
            session: true,
            touched_unix: (input.git_mtime_unix)(path),
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
                session: true,
                touched_unix: None,
                diff_stat: None,
            });
        }
    }

    // Repo-wide search pool: every worktree file not already represented as a
    // session/git candidate, appended as a non-session entry. These are hidden
    // from the default view (empty query) but reachable once the user types.
    let mut present: HashSet<String> = out.iter().map(|c| c.path.clone()).collect();
    for path in input.repo_files {
        if present.insert(path.clone()) {
            out.push(Candidate {
                path: path.clone(),
                line: None,
                is_edit: false,
                newly_created: false,
                session: false,
                touched_unix: None,
                diff_stat: None,
            });
        }
    }

    out.retain(|c| (input.exists)(&c.path));

    // Diff-stat eligibility: only is_edit, currently-dirty, non-newly-created
    // entries get a stat. Newly-created files already show the `new` badge;
    // an entry kept only because it was committed during the session is now
    // clean (no diff to show); non-git and net-change-demoted files were
    // never dirty-tracked at all.
    for candidate in &mut out {
        if candidate.is_edit
            && !candidate.newly_created
            && input.git_dirty.contains(&candidate.path)
        {
            if let Some(&(added, removed)) = input.diff_stats.get(&candidate.path) {
                if added > 0 || removed > 0 {
                    candidate.diff_stat = Some((added, removed));
                }
            }
        }
    }

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
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;

    fn always_true(_: &str) -> bool {
        true
    }
    fn always_false(_: &str) -> bool {
        false
    }
    fn no_mtime(_: &str) -> Option<u64> {
        None
    }
    fn empty_diff_stats() -> &'static HashMap<String, (u32, u32)> {
        static EMPTY: OnceLock<HashMap<String, (u32, u32)>> = OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
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
            first_op_unix: Some(1000),
            git_dirty,
            git_committed_in_session: git_committed,
            in_git_worktree: in_worktree,
            git_mtime_unix: &no_mtime,
            diff_stats: empty_diff_stats(),
            scraped_mentioned: &[],
            repo_files: &[],
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
        fn mtime_42(_: &str) -> Option<u64> {
            Some(42)
        }
        let input = BuildInput {
            mined_touches: &[],
            first_op_unix: None,
            git_dirty: &HashSet::from(["/repo/sed_edited.rs".to_owned()]),
            git_committed_in_session: &HashSet::new(),
            in_git_worktree: &always_true,
            git_mtime_unix: &mtime_42,
            diff_stats: empty_diff_stats(),
            scraped_mentioned: &[],
            repo_files: &[],
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
            first_op_unix: None,
            git_dirty: &HashSet::new(),
            git_committed_in_session: &HashSet::new(),
            in_git_worktree: &always_true,
            git_mtime_unix: &no_mtime,
            diff_stats: empty_diff_stats(),
            scraped_mentioned: &scraped,
            repo_files: &[],
            exists: &always_true,
        };
        let out = build_candidates(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/repo/scraped.rs");
        assert!(!out[0].is_edit);
    }

    #[test]
    fn repo_files_appended_as_non_session_and_deduped_against_touches() {
        // /repo/a.rs is both touched and in the repo listing -> one entry,
        // session; /repo/b.rs is repo-only -> appended, non-session.
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let repo_files = ["/repo/a.rs".to_owned(), "/repo/b.rs".to_owned()];
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        input.repo_files = &repo_files;
        let out = build_candidates(input);
        assert_eq!(out.len(), 2, "the touched+repo path must not be duplicated");
        // Touched (timestamped) entry sorts first; repo-only (untimed) last.
        assert_eq!(out[0].path, "/repo/a.rs");
        assert!(out[0].session);
        assert_eq!(out[1].path, "/repo/b.rs");
        assert!(!out[1].session, "repo-only entries are non-session");
    }

    #[test]
    fn diff_stat_set_for_eligible_dirty_edit() {
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        let diff_stats = HashMap::from([("/repo/a.rs".to_owned(), (3u32, 1u32))]);
        input.diff_stats = &diff_stats;
        let out = build_candidates(input);
        assert_eq!(out[0].diff_stat, Some((3, 1)));
    }

    #[test]
    fn diff_stat_not_set_for_newly_created_entry() {
        let mut touches = [touch("/repo/new.rs", true, Some(5))];
        touches[0].newly_created = true;
        let dirty = HashSet::from(["/repo/new.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        let diff_stats = HashMap::from([("/repo/new.rs".to_owned(), (3u32, 1u32))]);
        input.diff_stats = &diff_stats;
        let out = build_candidates(input);
        assert_eq!(
            out[0].diff_stat, None,
            "newly-created files show the `new` badge instead"
        );
    }

    #[test]
    fn diff_stat_not_set_for_non_dirty_entry() {
        // `is_edit` via committed-in-session, but no longer in `git_dirty`
        // (clean now) -- no diff to show even if a stale numstat entry
        // existed for it.
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::new();
        let committed = HashSet::from(["/repo/a.rs".to_owned()]);
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        let diff_stats = HashMap::from([("/repo/a.rs".to_owned(), (3u32, 1u32))]);
        input.diff_stats = &diff_stats;
        let out = build_candidates(input);
        assert_eq!(out[0].diff_stat, None);
    }

    #[test]
    fn diff_stat_zero_zero_is_not_set() {
        let touches = [touch("/repo/a.rs", true, Some(5))];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&touches, &dirty, &committed, &always_true);
        let diff_stats = HashMap::from([("/repo/a.rs".to_owned(), (0u32, 0u32))]);
        input.diff_stats = &diff_stats;
        let out = build_candidates(input);
        assert_eq!(out[0].diff_stat, None);
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
