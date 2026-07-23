use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::{Dir, PaneRect};

#[derive(Clone, Serialize, Deserialize)]
pub struct MoveStep {
    pub pane: String,
    pub dir: Dir,
    pub target: String,
    pub ratio: f64,
}

pub struct RebuildPlan {
    pub anchor: String,
    pub steps: Vec<MoveStep>,
}

const GAP: u32 = 2;

fn bounds(rects: &[PaneRect]) -> (u32, u32, u32, u32) {
    let x0 = rects.iter().map(|rect| rect.x).min().unwrap();
    let y0 = rects.iter().map(|rect| rect.y).min().unwrap();
    let x1 = rects.iter().map(|rect| rect.x + rect.w).max().unwrap();
    let y1 = rects.iter().map(|rect| rect.y + rect.h).max().unwrap();
    (x0, y0, x1, y1)
}

/// A cut at `edge` is only valid if the two groups it produces do not
/// geometrically overlap: the rightmost extent of the "before" group must not
/// exceed the leftmost/topmost extent of the "after" group. A small tolerance
/// (GAP) allows genuine layouts near the cut to be selected as *candidates*,
/// but the overlap check itself is zero-tolerance -- two rects that overlap
/// by even one cell must never be classified onto opposite sides of the same
/// cut, or the reconstructed plan would silently drop the overlap.
fn is_clean_cut(before_max_end: u32, after_min_start: u32) -> bool {
    after_min_start >= before_max_end
}

fn x_start(rect: &PaneRect) -> u32 {
    rect.x
}
fn x_extent(rect: &PaneRect) -> u32 {
    rect.w
}
fn y_start(rect: &PaneRect) -> u32 {
    rect.y
}
fn y_extent(rect: &PaneRect) -> u32 {
    rect.h
}

/// Finds a cut position along one axis that separates rects into non-empty,
/// non-overlapping groups. `start`/`extent` extract the rect's coordinate and
/// size on that axis (x/w for a vertical cut, y/h for a horizontal one); `lo`
/// and `hi` are the axis bounds of the whole region.
fn cut(
    rects: &[PaneRect],
    lo: u32,
    hi: u32,
    start: fn(&PaneRect) -> u32,
    extent: fn(&PaneRect) -> u32,
) -> Option<u32> {
    let end = |rect: &PaneRect| start(rect) + extent(rect);
    let mut edges: Vec<u32> = rects
        .iter()
        .map(end)
        .filter(|&edge| edge > lo + GAP && edge + GAP < hi)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    edges.into_iter().find(|&cut| {
        let before_max_end = rects.iter().filter(|rect| end(rect) <= cut).map(end).max();
        let after_min_start = rects.iter().filter(|rect| end(rect) > cut).map(start).min();
        match (before_max_end, after_min_start) {
            (Some(before_max_end), Some(after_min_start)) => {
                is_clean_cut(before_max_end, after_min_start)
            }
            _ => false,
        }
    })
}

/// Returns the first pane ID of the region and the steps to build it.
fn partition(rects: &[PaneRect]) -> Result<(String, Vec<MoveStep>)> {
    if rects.len() == 1 {
        return Ok((rects[0].pane_id.clone(), vec![]));
    }

    let (x0, y0, x1, y1) = bounds(rects);
    let axes: [(Dir, u32, u32, fn(&PaneRect) -> u32, fn(&PaneRect) -> u32); 2] = [
        (Dir::Right, x0, x1, x_start, x_extent),
        (Dir::Down, y0, y1, y_start, y_extent),
    ];
    for (dir, lo, hi, start, extent) in axes {
        if let Some(cut_pos) = cut(rects, lo, hi, start, extent) {
            let (first, second): (Vec<_>, Vec<_>) = rects
                .iter()
                .cloned()
                .partition(|rect| start(rect) + extent(rect) <= cut_pos);
            let ratio = (cut_pos - lo) as f64 / (hi - lo) as f64;
            return combine(first, second, dir, ratio);
        }
    }

    bail!(
        "layout is not guillotine-partitionable ({} rects)",
        rects.len()
    )
}

fn combine(
    first: Vec<PaneRect>,
    second: Vec<PaneRect>,
    dir: Dir,
    ratio: f64,
) -> Result<(String, Vec<MoveStep>)> {
    let (first_head, first_steps) = partition(&first)?;
    let (second_head, second_steps) = partition(&second)?;
    let mut steps = vec![MoveStep {
        pane: second_head.clone(),
        dir,
        target: first_head.clone(),
        ratio,
    }];
    // Place the second branch's head first, then build both branches using
    // targets that are already present in the reconstructed layout.
    steps.extend(first_steps);
    steps.extend(second_steps);
    Ok((first_head, steps))
}

pub fn plan_rebuild(rects: &[PaneRect]) -> Result<RebuildPlan> {
    if rects.is_empty() {
        bail!("no panes");
    }

    let (anchor, steps) = partition(rects)?;
    Ok(RebuildPlan { anchor, steps })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{Dir, PaneRect};

    fn r(id: &str, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect {
            pane_id: id.into(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn single_pane_plan_is_anchor_only() {
        let p = plan_rebuild(&[r("p1", 0, 0, 100, 50)]).unwrap();
        assert_eq!(p.anchor, "p1");
        assert!(p.steps.is_empty());
    }

    #[test]
    fn two_columns() {
        // A | B at 40/60
        let p = plan_rebuild(&[r("a", 0, 0, 40, 50), r("b", 41, 0, 59, 50)]).unwrap();
        assert_eq!(p.anchor, "a");
        assert_eq!(p.steps.len(), 1);
        let s = &p.steps[0];
        assert_eq!((s.pane.as_str(), s.target.as_str()), ("b", "a"));
        assert!(matches!(s.dir, Dir::Right));
        assert!((s.ratio - 0.4).abs() < 0.03);
    }

    #[test]
    fn asymmetric_three_pane() {
        // A full-height left 40%; right column: B top 30%, C bottom 70%
        let p = plan_rebuild(&[
            r("a", 0, 0, 40, 52),
            r("b", 41, 0, 59, 15),
            r("c", 41, 16, 59, 36),
        ])
        .unwrap();
        assert_eq!(p.anchor, "a");
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].pane, "b"); // b splits a right
        assert!(matches!(p.steps[0].dir, Dir::Right));
        assert_eq!(p.steps[1].pane, "c"); // c splits b down
        assert_eq!(p.steps[1].target, "b");
        assert!(matches!(p.steps[1].dir, Dir::Down));
        assert!((p.steps[1].ratio - 0.3).abs() < 0.05);
    }

    #[test]
    fn grid_2x2() {
        let p = plan_rebuild(&[
            r("a", 0, 0, 50, 25),
            r("b", 51, 0, 49, 25),
            r("c", 0, 26, 50, 26),
            r("d", 51, 26, 49, 26),
        ])
        .unwrap();
        assert_eq!(p.steps.len(), 3);
    }

    #[test]
    fn non_partitionable_rects_error() {
        // overlapping rects — must not panic, must Err
        assert!(plan_rebuild(&[r("a", 0, 0, 60, 50), r("b", 30, 0, 70, 50)]).is_err());
    }

    #[test]
    fn one_cell_overlap_is_rejected() {
        // a covers x in [0,50); b covers x in [49,100) -- overlap of 1 cell.
        // Must be rejected, not silently split into two non-overlapping groups.
        assert!(plan_rebuild(&[r("a", 0, 0, 50, 50), r("b", 49, 0, 51, 50)]).is_err());
    }

    #[test]
    fn two_cell_overlap_is_rejected() {
        assert!(plan_rebuild(&[r("a", 0, 0, 50, 50), r("b", 48, 0, 52, 50)]).is_err());
    }

    #[test]
    fn real_fixture_plans_without_error() {
        let rects =
            crate::herdr::parse_pane_rects(include_str!("../tests/fixtures/layout_4pane.json"))
                .unwrap();
        let p = plan_rebuild(&rects).unwrap();
        assert_eq!(p.steps.len(), rects.len() - 1);
    }
}
