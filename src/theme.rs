use crate::parser_tex::latex_parser;
use crate::settings;
use crate::Variable;
use anyhow::{anyhow, Result};
use std::fs;

pub fn read_theme_variables(file_path: &str) -> Result<Vec<Variable>> {
    //its ok if you dont find the file
    let content = match std::fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(_) => return Ok(Vec::new()),
    };
    let mut variables = Vec::new();
    for line in content.lines() {
        if line.starts_with('%') {
            continue;
        }
        match latex_parser(&line) {
            Ok((_, variable)) => {
                variables.push(variable);
            }
            Err(_) => continue, //maybe add error?
        }
    }
    Ok(variables)
}

pub fn all_themes() -> Result<Vec<String>> {
    let themes_dir = settings::themes_dir()?;
    let entries = fs::read_dir(themes_dir)?;
    let mut directories = Vec::new();
    for entry in entries {
        if let Ok(entry) = entry {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(folder_name) = path.file_name() {
                if let Some(name) = folder_name.to_str() {
                    directories.push(name.to_string());
                }
            }
        }
    }
    Ok(directories)
}

pub fn theme_variables(theme: String) -> Result<Vec<Variable>> {
    // get available themes
    let themes_list = all_themes()?;
    // Panic if it does not exist
    if !themes_list.contains(&theme) {
        return Err(anyhow!(
            "Theme {} not found, available themes are: {:?}",
            theme,
            themes_list
        ));
    }
    //get variables if it does exist
    let mut theme_dir = settings::theme_dir(&theme)?;
    theme_dir.push_str("/variables.tex");
    read_theme_variables(&theme_dir)
}
