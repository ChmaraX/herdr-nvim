//! `herdr-nvim app-mermaid --theme <dark|light> --out <file.png> --code <text>`:
//! draws one Mermaid diagram for the app's markdown preview
//! (`mods/herdr-nvim/preview.js`), with no browser and nothing to install.
//!
//! Merman (a Rust port of Mermaid) lays the diagram out with Mermaid's own
//! layouts and themes and paints it as a PNG at twice its size, on a
//! transparent background. It measures the labels with the same system fonts
//! it paints them with, so text always fits its box (an SVG would be painted
//! by the app with fonts merman did not measure). The PNG is written beside
//! `--out` and renamed onto it, so a reader never sees half of one.

use std::{env, fs, path::Path, process};

use anyhow::{anyhow, bail, Context, Result};
use merman::svg::export::{
    NativeFontContext, NativeFontError, NativeFontLimits, NativeFontOptions, RasterOptions,
};
use merman::{
    MermaidEngine, OperationControl, PngRequest, RenderOutput, RenderRequest, Renderer,
    SourcePresentationConfig, SvgEnvironment, SvgRequest,
};

use super::cli;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Theme {
    Dark,
    Light,
}

impl Theme {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            _ => bail!("--theme is dark or light, not {name:?}"),
        }
    }

    /// Mermaid's theme of that name.
    fn mermaid(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "default",
        }
    }
}

pub fn mermaid_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let usage = "usage: herdr-nvim app-mermaid --theme <dark|light> --out <file.png> --code <text>";
    let theme = Theme::parse(cli::flag(&args, "--theme")?.context(usage)?)?;
    let out = Path::new(cli::flag(&args, "--out")?.context(usage)?);
    let code = cli::flag(&args, "--code")?.context(usage)?;
    let png = draw(code, theme)?;
    let partial = out.with_extension(format!("{}.partial", process::id()));
    fs::write(&partial, png).with_context(|| format!("cannot write {}", partial.display()))?;
    fs::rename(&partial, out).with_context(|| format!("cannot write {}", out.display()))
}

/// The diagram as PNG bytes at twice its size; an error says what is wrong
/// with the code (Mermaid's parse error) or why it could not be drawn.
pub(crate) fn draw(code: &str, theme: Theme) -> Result<Vec<u8>> {
    let control = OperationControl::new();
    let fonts = system_fonts(&control).map_err(|e| anyhow!("cannot load the system fonts: {e}"))?;
    let environment = SvgEnvironment::deterministic()
        .with_native_font_context(fonts)
        .map_err(|e| anyhow!("cannot load the system fonts: {e}"))?;
    let config =
        SourcePresentationConfig::from_value(serde_json::json!({ "theme": theme.mermaid() }));
    let renderer =
        Renderer::new().with_presentation_engine(MermaidEngine::new().with_site_config(config));
    let request = PngRequest {
        svg: SvgRequest {
            environment,
            ..SvgRequest::default()
        },
        options: RasterOptions::default().with_scale(2.0),
    };
    match renderer.render(RenderRequest::png(code, control, request)) {
        Ok(RenderOutput::Png(Some(png))) => Ok(png.bytes().to_vec()),
        Ok(_) => bail!("not a Mermaid diagram"),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// The machine's fonts, for measuring and painting alike. macOS answers from
/// Core Text (about 35 ms); elsewhere every font file is scanned.
fn system_fonts(control: &OperationControl) -> Result<NativeFontContext, NativeFontError> {
    // The default budgets are less than a Mac's: 2 GiB of font files to
    // scan, 64 MiB of fonts to use (its emoji font alone is 190 MB).
    let options = NativeFontOptions::default()
        .with_system_fonts(true)
        .with_limits(NativeFontLimits {
            max_system_scan_bytes: 16 << 30,
            max_bytes: 1 << 30,
            ..NativeFontLimits::default()
        });
    #[cfg(target_os = "macos")]
    return options.load_platform(control);
    #[cfg(not(target_os = "macos"))]
    return options.load(control);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[1..4], b"PNG", "not a PNG");
        let at = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
        (at(16), at(20))
    }

    #[test]
    fn draws_each_theme_and_reports_mermaids_parse_error() {
        let code =
            "flowchart TD\n  A[Read the issue] --> B{Reproduced?}\n  B -- yes --> C[Write the fix]";
        let dark = draw(code, Theme::Dark).unwrap();
        let light = draw(code, Theme::Light).unwrap();
        let (width, height) = png_size(&dark);
        assert!(
            width > 200 && height > 300,
            "a three-node column at 2x: {width}x{height}"
        );
        assert_eq!(
            png_size(&light),
            (width, height),
            "a theme changes colors, not layout"
        );
        assert_ne!(dark, light);

        let wider = draw(
            "flowchart LR\n  A[Read the issue] --> B{Reproduced?}",
            Theme::Dark,
        )
        .unwrap();
        assert!(
            png_size(&wider).0 > png_size(&wider).1,
            "LR lays out left to right"
        );

        let error = draw("flowchart TD\n  A[unclosed --> B", Theme::Dark).unwrap_err();
        assert!(format!("{error}").contains("parse error"), "{error}");
        assert!(draw("just some text", Theme::Light).is_err());
    }
}
