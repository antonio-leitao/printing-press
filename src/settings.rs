use crate::Variable;
use anyhow::{anyhow, Result};
use shellexpand::tilde;
use std::collections::HashMap;
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
fn ensure_ends_with_slash(s: &str) -> String {
    if !s.ends_with('/') {
        s.to_owned() + "/"
    } else {
        s.to_owned()
    }
}

pub fn get_clean_dir(config: &HashMap<String, Variable>, name: &str) -> Result<String> {
    match config.get(name) {
        Some(variable) => match &variable.value {
            Some(dir) => Ok(ensure_ends_with_slash(&dir)),
            None => Ok("/".to_string()),
        },
        None => Err(anyhow!("Could not find variable {} in config", name)),
    }
}
