use anyhow::Result;
use shellexpand::tilde;
use std::fs;
// Add editor
// pub const EDITOR: &str = "vim";
pub fn themes_dir() -> Result<String> {
    let dir = tilde("~/.press/themes").to_string();
    fs::create_dir_all(&dir)?;
    Ok(dir)
}
pub fn theme_dir(name: &str) -> Result<String> {
    let mut themes = themes_dir()?;
    themes.push_str(&format!("/{}", name));
    Ok(themes)
}
