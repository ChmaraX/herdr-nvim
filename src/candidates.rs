//! The merge/dedup/order pipeline that turns three read-only sources
//! (session mining, git worktree status, terminal scrape) into the final
//! sectioned candidate list the picker renders. Pure: every I/O-shaped
//! input (git status/log results, existence checks) is passed in already
//! computed or as an injected closure, so this module has no I/O of its own
//! and is fully unit tested.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{extract, gitscan, sessions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    Edited,
    Mentioned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub path: String,
    pub line: Option<u32>,
    pub section: Section,
    pub newly_created: bool,
    pub last_edit_unix: Option<u64>,
}

pub struct GitOnlyEdit {
    pub path: String,
    pub mtime_unix: Option<u64>,
}

pub struct BuildInput<'a> {
    pub mined_edits: &'a [sessions::MinedEdit],
    pub mined_reads: &'a [String],
    pub session_start_unix: Option<u64>,
    pub git_dirty: &'a HashSet<String>,
    pub git_committed_in_session: &'a HashSet<String>,
    pub in_git_worktree: &'a dyn Fn(&str) -> bool,
    pub git_only_dirty_not_mined: &'a [GitOnlyEdit],
    pub scraped_mentioned: &'a [extract::ScrapedPath],
    pub exists: &'a dyn Fn(&str) -> bool,
}

pub fn build_candidates(input: BuildInput) -> Vec<Candidate> {
    let mut edited: Vec<Candidate> = Vec::new();
    let mut demoted_paths: HashSet<String> = HashSet::new();

    for edit in input.mined_edits {
        let in_repo = (input.in_git_worktree)(&edit.path);
        let dirty = input.git_dirty.contains(&edit.path);
        let committed = input.git_committed_in_session.contains(&edit.path);
        if gitscan::should_keep_edited(in_repo, dirty, committed) {
            edited.push(Candidate {
                path: edit.path.clone(),
                line: None,
                section: Section::Edited,
                newly_created: edit.newly_created,
                last_edit_unix: edit.last_edit_unix,
            });
        } else {
            demoted_paths.insert(edit.path.clone());
        }
    }

    for git_only in input.git_only_dirty_not_mined {
        edited.push(Candidate {
            path: git_only.path.clone(),
            line: None,
            section: Section::Edited,
            newly_created: false,
            last_edit_unix: git_only.mtime_unix,
        });
    }

    let edited_paths: HashSet<String> = edited.iter().map(|c| c.path.clone()).collect();

    let mut mentioned: Vec<Candidate> = Vec::new();
    let mut mentioned_seen: HashSet<String> = HashSet::new();

    let use_scrape = input.mined_edits.is_empty() && input.mined_reads.is_empty();
    if use_scrape {
        for scraped in input.scraped_mentioned {
            if edited_paths.contains(&scraped.path) || !mentioned_seen.insert(scraped.path.clone())
            {
                continue;
            }
            mentioned.push(Candidate {
                path: scraped.path.clone(),
                line: scraped.line,
                section: Section::Mentioned,
                newly_created: false,
                last_edit_unix: None,
            });
        }
    } else {
        for path in demoted_paths {
            if edited_paths.contains(&path) || !mentioned_seen.insert(path.clone()) {
                continue;
            }
            mentioned.push(Candidate {
                path,
                line: None,
                section: Section::Mentioned,
                newly_created: false,
                last_edit_unix: None,
            });
        }
        for path in input.mined_reads {
            if edited_paths.contains(path) || !mentioned_seen.insert(path.clone()) {
                continue;
            }
            mentioned.push(Candidate {
                path: path.clone(),
                line: None,
                section: Section::Mentioned,
                newly_created: false,
                last_edit_unix: None,
            });
        }
    }

    edited.sort_by(|a, b| b.last_edit_unix.cmp(&a.last_edit_unix));

    edited.retain(|c| (input.exists)(&c.path));
    mentioned.retain(|c| (input.exists)(&c.path));

    edited.into_iter().chain(mentioned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::MinedEdit;
    use std::collections::HashSet;

    fn always_true(_: &str) -> bool {
        true
    }
    fn always_false(_: &str) -> bool {
        false
    }

    fn base_input<'a>(
        mined_edits: &'a [MinedEdit],
        git_dirty: &'a HashSet<String>,
        git_committed: &'a HashSet<String>,
        in_worktree: &'a dyn Fn(&str) -> bool,
    ) -> BuildInput<'a> {
        BuildInput {
            mined_edits,
            mined_reads: &[],
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
    fn dirty_session_edit_stays_in_edited() {
        let edits = [MinedEdit {
            path: "/repo/a.rs".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let out = build_candidates(base_input(&edits, &dirty, &committed, &always_true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].section, Section::Edited);
    }

    #[test]
    fn committed_in_session_edit_stays_in_edited() {
        let edits = [MinedEdit {
            path: "/repo/a.rs".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::new();
        let committed = HashSet::from(["/repo/a.rs".to_owned()]);
        let out = build_candidates(base_input(&edits, &dirty, &committed, &always_true));
        assert_eq!(out[0].section, Section::Edited);
    }

    #[test]
    fn clean_and_uncommitted_edit_is_demoted_to_mentioned() {
        let edits = [MinedEdit {
            path: "/repo/a.rs".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::new();
        let committed = HashSet::new();
        let out = build_candidates(base_input(&edits, &dirty, &committed, &always_true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].section, Section::Mentioned);
    }

    #[test]
    fn non_git_edit_always_stays_in_edited() {
        let edits = [MinedEdit {
            path: "/home/u/.config/foo.toml".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::new();
        let committed = HashSet::new();
        let out = build_candidates(base_input(&edits, &dirty, &committed, &always_false));
        assert_eq!(out[0].section, Section::Edited);
    }

    #[test]
    fn git_only_dirty_file_is_added_to_edited_with_mtime() {
        let input = BuildInput {
            mined_edits: &[],
            mined_reads: &[],
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
        assert_eq!(out[0].section, Section::Edited);
        assert_eq!(out[0].last_edit_unix, Some(42));
    }

    #[test]
    fn mentioned_excludes_anything_already_in_edited() {
        let edits = [MinedEdit {
            path: "/repo/a.rs".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::from(["/repo/a.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&edits, &dirty, &committed, &always_true);
        let reads = ["/repo/a.rs".to_owned(), "/repo/b.rs".to_owned()];
        input.mined_reads = &reads;
        let out = build_candidates(input);
        assert_eq!(
            out.iter().filter(|c| c.path == "/repo/a.rs").count(),
            1,
            "no duplicate across sections"
        );
        assert!(out
            .iter()
            .any(|c| c.path == "/repo/b.rs" && c.section == Section::Mentioned));
    }

    #[test]
    fn scrape_fallback_used_only_when_no_mined_data_at_all() {
        use crate::extract::ScrapedPath;
        let scraped = [ScrapedPath {
            path: "/repo/scraped.rs".into(),
            line: None,
        }];
        let input = BuildInput {
            mined_edits: &[],
            mined_reads: &[],
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
        assert_eq!(out[0].section, Section::Mentioned);
    }

    #[test]
    fn edited_ordered_newest_first_by_last_edit_unix() {
        let edits = [
            MinedEdit {
                path: "/repo/old.rs".into(),
                newly_created: false,
                last_edit_unix: Some(1),
            },
            MinedEdit {
                path: "/repo/new.rs".into(),
                newly_created: false,
                last_edit_unix: Some(99),
            },
        ];
        let dirty = HashSet::from(["/repo/old.rs".to_owned(), "/repo/new.rs".to_owned()]);
        let out = build_candidates(base_input(&edits, &dirty, &HashSet::new(), &always_true));
        assert_eq!(out[0].path, "/repo/new.rs");
        assert_eq!(out[1].path, "/repo/old.rs");
    }

    #[test]
    fn nonexistent_paths_are_filtered_out() {
        let edits = [MinedEdit {
            path: "/repo/ghost.rs".into(),
            newly_created: false,
            last_edit_unix: Some(5),
        }];
        let dirty = HashSet::from(["/repo/ghost.rs".to_owned()]);
        let committed = HashSet::new();
        let mut input = base_input(&edits, &dirty, &committed, &always_true);
        input.exists = &always_false;
        let out = build_candidates(input);
        assert!(out.is_empty());
    }
}
