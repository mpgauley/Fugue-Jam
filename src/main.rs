mod app;
mod localization;

use app::MusicApp;
use eframe::egui;
use std::{fs, process::Command};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Fugue Jam")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Fugue Jam",
        options,
        Box::new(|creation_context| {
            if let Err(error) = install_multilingual_font_fallback(&creation_context.egui_ctx) {
                eprintln!(
                    "Some foreign-language text may not display because font fallbacks could not be loaded: {error}"
                );
            }
            Ok(Box::new(MusicApp::new()))
        }),
    )
}

fn install_multilingual_font_fallback(ctx: &egui::Context) -> Result<(), String> {
    let languages = [
        ("japanese", "ja"),
        ("chinese-simplified", "zh-cn"),
        ("chinese-traditional", "zh-tw"),
        ("korean", "ko"),
        ("russian", "ru"),
    ];
    let mut definitions = egui::FontDefinitions::default();
    let mut added_fonts = Vec::new();
    for (name, language) in languages {
        let output = Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}\n", &format!(":lang={language}")])
            .output()
            .map_err(|error| format!("fontconfig lookup for {language} failed: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "fontconfig lookup for {language} exited with {}",
                output.status
            ));
        }
        let output = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
        let mut lines = output.lines();
        let path = lines
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| format!("no {name}-capable system font was found"))?;
        let index = lines
            .next()
            .and_then(|index| index.parse::<u32>().ok())
            .unwrap_or(0);
        let font = fs::read(path).map_err(|error| format!("could not read {path}: {error}"))?;
        let mut font_data = egui::FontData::from_owned(font);
        font_data.index = index;
        let font_name = format!("system-{name}-fallback");
        definitions
            .font_data
            .insert(font_name.clone(), font_data.into());
        added_fonts.push(font_name);
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .extend(added_fonts.iter().cloned());
    }
    ctx.set_fonts(definitions);
    Ok(())
}
