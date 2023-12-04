use crate::settings;
use crate::Variable;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempdir::TempDir;
use walkdir::WalkDir;

fn copy_theme_dir_to_temp(temp_dir: &TempDir, name: String) -> Result<()> {
    let source_dir = settings::theme_dir(&name)?;
    let temp_content_dir = temp_dir.path();
    // Ensure the destination directory exists
    fs::create_dir_all(&temp_content_dir)?;
    for entry in WalkDir::new(&source_dir).into_iter().filter_map(|e| e.ok()) {
        let destination_path =
            temp_content_dir.join(entry.path().strip_prefix(&source_dir).unwrap());
        if entry.file_type().is_dir() {
            // Ensure the directory exists in the temp directory
            fs::create_dir_all(&destination_path)?;
        } else {
            // Copy the file
            fs::copy(&entry.path(), &destination_path)?;
        }
    }

    Ok(())
}

fn copy_content_to_temp_dir(temp_dir: &TempDir) -> Result<()> {
    let source_dir = Path::new("content/");
    let temp_content_dir = temp_dir.path().join("content/");
    // Ensure the destination directory exists
    fs::create_dir_all(&temp_content_dir)?;
    for entry in WalkDir::new(&source_dir).into_iter().filter_map(|e| e.ok()) {
        let destination_path =
            temp_content_dir.join(entry.path().strip_prefix(&source_dir).unwrap());
        if entry.file_type().is_dir() {
            // Ensure the directory exists in the temp directory
            fs::create_dir_all(&destination_path)?;
        } else {
            // Copy the file
            fs::copy(&entry.path(), &destination_path)?;
        }
    }
    Ok(())
}

fn write_variables(temp_dir: &TempDir, variables: HashMap<String, Variable>) -> Result<()> {
    let file_path = temp_dir.path().join("variables.tex");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)?;

    for variable in variables.into_values() {
        if ["dir", "depth", "extension"].contains(&variable.name.as_str()) {
            continue;
        }
        if variable.is_boolean {
            writeln!(file, "\\{}true", variable.name)?;
        } else {
            match variable.value {
                Some(value) => {
                    writeln!(file, "\\renewcommand{{\\{}}}{{{}}}\n", variable.name, value)?;
                }
                None => continue,
            }
        }
    }

    Ok(())
}

fn run_latexmk(temp_dir: &TempDir) -> Result<()> {
    let temp_dir_path = temp_dir.path().to_str().unwrap();
    let mut latexmk_command = Command::new("latexmk");
    latexmk_command.arg("main.tex");
    latexmk_command.arg("-pdf");
    let output = latexmk_command.current_dir(temp_dir_path).output()?;
    println!("stderr: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    Ok(())
}

fn move_main_pdf_to_working_dir(temp_dir: &TempDir) -> Result<()> {
    let temp_pdf_path = temp_dir.path().join("main.pdf");
    let working_pdf_path = Path::new("main.pdf");
    // Move the file
    fs::rename(&temp_pdf_path, &working_pdf_path)?;
    Ok(())
}

pub fn write_content(theme: String, variables: HashMap<String, Variable>) -> Result<()> {
    let temp_dir = TempDir::new("temp_dir_example")?;
    copy_theme_dir_to_temp(&temp_dir, theme)?;
    copy_content_to_temp_dir(&temp_dir)?;
    write_variables(&temp_dir, variables)?;
    run_latexmk(&temp_dir)?;
    move_main_pdf_to_working_dir(&temp_dir)?;
    temp_dir.close()?;
    Ok(())
}
