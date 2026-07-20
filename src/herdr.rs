use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub pane_id: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Right,
    Down,
}

impl Dir {
    fn as_cli_arg(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

pub trait Herdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>>;
    fn tab_of_pane(&mut self, pane: &str) -> Result<String>;
    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)>;
    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()>;
    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String>;
    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()>;
    fn close_pane(&mut self, pane: &str) -> Result<()>;
    fn pane_alive(&mut self, pane: &str) -> Result<bool>;
    fn list_workspaces(&mut self) -> Result<Vec<String>>;
}

pub struct CliHerdr;

impl CliHerdr {
    fn run(args: &[String]) -> Result<Value> {
        let command = format!("herdr {}", args.join(" "));
        let output = Command::new("herdr")
            .args(args)
            .output()
            .with_context(|| format!("failed to run {command}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{command} failed: {}", stderr.trim());
        }

        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse JSON from {command}"))
    }

    fn panes() -> Result<Vec<(String, String)>> {
        let value = Self::run(&args(&["pane", "list"]))?;
        let panes = value
            .pointer("/result/panes")
            .and_then(Value::as_array)
            .context("herdr pane list response missing result.panes array")?;

        panes
            .iter()
            .map(|pane| {
                Ok((
                    string_at(pane, "/pane_id")?.to_owned(),
                    string_at(pane, "/tab_id")?.to_owned(),
                ))
            })
            .collect()
    }
}

impl Herdr for CliHerdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>> {
        let pane = Self::panes()?
            .into_iter()
            .find_map(|(pane, pane_tab)| (pane_tab == tab).then_some(pane))
            .with_context(|| format!("tab {tab} has no live panes"))?;
        let value = Self::run(&args(&["pane", "layout", "--pane", &pane]))?;
        parse_pane_rects_value(&value)
    }

    fn tab_of_pane(&mut self, pane: &str) -> Result<String> {
        let value = Self::run(&args(&["pane", "get", pane]))?;
        Ok(string_at(&value, "/result/pane/tab_id")?.to_owned())
    }

    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)> {
        let value = Self::run(&args(&[
            "tab",
            "create",
            "--workspace",
            workspace,
            "--no-focus",
        ]))?;
        Ok((
            string_at(&value, "/result/tab/tab_id")?.to_owned(),
            string_at(&value, "/result/root_pane/pane_id")?.to_owned(),
        ))
    }

    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()> {
        let mut command = args(&[
            "pane",
            "move",
            pane,
            "--tab",
            tab,
            "--split",
            dir.as_cli_arg(),
        ]);
        if let Some(target) = target {
            command.extend(args(&["--target-pane", target]));
        }
        if let Some(ratio) = ratio {
            command.extend(args(&["--ratio", &ratio.to_string()]));
        }
        command.push(if focus { "--focus" } else { "--no-focus" }.to_owned());
        Self::run(&command)?;
        Ok(())
    }

    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String> {
        let value = Self::run(&args(&[
            "pane",
            "split",
            pane,
            "--direction",
            dir.as_cli_arg(),
            "--ratio",
            &ratio.to_string(),
            if focus { "--focus" } else { "--no-focus" },
        ]))?;
        Ok(string_at(&value, "/result/pane/pane_id")?.to_owned())
    }

    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()> {
        Self::run(&args(&["pane", "run", pane, cmd]))?;
        Ok(())
    }

    fn close_pane(&mut self, pane: &str) -> Result<()> {
        Self::run(&args(&["pane", "close", pane]))?;
        Ok(())
    }

    fn pane_alive(&mut self, pane: &str) -> Result<bool> {
        Ok(Self::panes()?
            .iter()
            .any(|(pane_id, _tab_id)| pane_id == pane))
    }

    fn list_workspaces(&mut self) -> Result<Vec<String>> {
        let value = Self::run(&args(&["workspace", "list"]))?;
        let workspaces = value
            .pointer("/result/workspaces")
            .and_then(Value::as_array)
            .context("herdr workspace list response missing result.workspaces array")?;
        workspaces
            .iter()
            .map(|workspace| Ok(string_at(workspace, "/workspace_id")?.to_owned()))
            .collect()
    }
}

pub fn parse_pane_rects(json: &str) -> Result<Vec<PaneRect>> {
    let value: Value = serde_json::from_str(json).context("invalid pane layout JSON")?;
    parse_pane_rects_value(&value)
}

fn parse_pane_rects_value(value: &Value) -> Result<Vec<PaneRect>> {
    let layout = value
        .pointer("/result/layout")
        .context("pane layout response missing result.layout")?;
    let origin_x = u32_at(layout, "/area/x")?;
    let origin_y = u32_at(layout, "/area/y")?;
    let panes = layout
        .pointer("/panes")
        .and_then(Value::as_array)
        .context("pane layout response missing result.layout.panes array")?;

    panes
        .iter()
        .map(|pane| {
            let x = u32_at(pane, "/rect/x")?;
            let y = u32_at(pane, "/rect/y")?;
            Ok(PaneRect {
                pane_id: string_at(pane, "/pane_id")?.to_owned(),
                x: x.checked_sub(origin_x)
                    .context("pane rect x is outside the layout area")?,
                y: y.checked_sub(origin_y)
                    .context("pane rect y is outside the layout area")?,
                w: u32_at(pane, "/rect/width")?,
                h: u32_at(pane, "/rect/height")?,
            })
        })
        .collect()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("JSON response missing string at {pointer}"))
}

fn u32_at(value: &Value, pointer: &str) -> Result<u32> {
    let number = value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("JSON response missing unsigned integer at {pointer}"))?;
    number
        .try_into()
        .with_context(|| format!("integer at {pointer} does not fit in u32"))
}

#[cfg(test)]
use std::collections::VecDeque;

#[cfg(test)]
#[derive(Default)]
pub struct MockHerdr {
    pub ops: Vec<String>,
    pub pane_rects_results: VecDeque<Result<Vec<PaneRect>>>,
    pub tab_of_pane_results: VecDeque<Result<String>>,
    pub create_tab_results: VecDeque<Result<(String, String)>>,
    pub split_pane_results: VecDeque<Result<String>>,
    pub pane_alive_results: VecDeque<Result<bool>>,
    pub list_workspaces_results: VecDeque<Result<Vec<String>>>,
}

#[cfg(test)]
impl MockHerdr {
    fn next<T>(queue: &mut VecDeque<Result<T>>, operation: &str) -> Result<T> {
        queue
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("no scripted response for {operation}")))
    }
}

#[cfg(test)]
impl Herdr for MockHerdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>> {
        self.ops.push(format!("rects {tab}"));
        Self::next(&mut self.pane_rects_results, "pane_rects")
    }

    fn tab_of_pane(&mut self, pane: &str) -> Result<String> {
        self.ops.push(format!("tab_of {pane}"));
        Self::next(&mut self.tab_of_pane_results, "tab_of_pane")
    }

    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)> {
        self.ops.push(format!("create_tab {workspace}"));
        Self::next(&mut self.create_tab_results, "create_tab")
    }

    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()> {
        self.ops.push(format!(
            "move {pane} -> tab:{tab} dir:{dir:?} target:{} ratio:{} focus:{focus}",
            target.unwrap_or("-"),
            ratio.map_or_else(|| "-".to_owned(), |ratio| ratio.to_string())
        ));
        Ok(())
    }

    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String> {
        self.ops.push(format!(
            "split {pane} dir:{dir:?} ratio:{ratio} focus:{focus}"
        ));
        Self::next(&mut self.split_pane_results, "split_pane")
    }

    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()> {
        self.ops.push(format!("run {pane} {cmd}"));
        Ok(())
    }

    fn close_pane(&mut self, pane: &str) -> Result<()> {
        self.ops.push(format!("close {pane}"));
        Ok(())
    }

    fn pane_alive(&mut self, pane: &str) -> Result<bool> {
        self.ops.push(format!("alive {pane}"));
        Self::next(&mut self.pane_alive_results, "pane_alive")
    }

    fn list_workspaces(&mut self) -> Result<Vec<String>> {
        self.ops.push("list_workspaces".to_owned());
        Self::next(&mut self.list_workspaces_results, "list_workspaces")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pane_rects_from_layout_fixture() {
        let json = include_str!("../tests/fixtures/layout_3pane.json");
        let rects = parse_pane_rects(json).unwrap();

        assert_eq!(rects.len(), 3);
        assert_eq!(
            rects
                .iter()
                .filter(|rect| rect.x == 0 && rect.y == 0)
                .count(),
            1
        );
        assert_eq!(
            rects[0],
            PaneRect {
                pane_id: "w0:p1".to_owned(),
                x: 0,
                y: 0,
                w: 72,
                h: 52,
            }
        );
    }

    #[test]
    fn single_pane_layout_parses() {
        let json = include_str!("../tests/fixtures/layout_1pane.json");
        assert_eq!(parse_pane_rects(json).unwrap().len(), 1);
    }

    #[test]
    fn mock_records_canonical_move_operation() {
        let mut herdr = MockHerdr::default();
        herdr
            .move_pane("p2", "t9", Dir::Right, Some("p1"), Some(0.4), false)
            .unwrap();

        assert_eq!(
            herdr.ops,
            ["move p2 -> tab:t9 dir:Right target:p1 ratio:0.4 focus:false"]
        );
    }
}
