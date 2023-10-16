use crate::reader::{ContentFile, ContentIndex};
use crate::Variable;
use crate::{parser_txt, settings};
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
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

fn copy_content_to_temp_dir(temp_dir: &TempDir, files: Vec<ContentFile>, dir: &str) -> Result<()> {
    for file in files {
        let source_path = PathBuf::from(dir).join(&file.origin);
        let destination_path = temp_dir.path().join("content").join(&file.destination);
        // Ensure the destination directory exists
        fs::create_dir_all(destination_path.parent().unwrap())?;
        // Read the content of the file
        let content = fs::read_to_string(&source_path)?;
        // Transform the content
        let transformed_content = parser_txt::parse_markdown(&content);
        // Write the transformed content to the destination file
        let mut destination_file = fs::File::create(&destination_path)?;
        destination_file.write_all(transformed_content.as_bytes())?;
        println!(
            "Copied and transformed: {} -> {}",
            source_path.display(),
            destination_path.display()
        );
    }
    Ok(())
}

fn write_content_index_to_file(temp_dir: &TempDir, index: &ContentIndex) -> Result<()> {
    let content_index_path = temp_dir.path().join("content").join("content_index.tex");
    // Ensure the destination directory exists
    fs::create_dir_all(content_index_path.parent().unwrap())?;
    let mut file = File::create(&content_index_path)?;
    write!(file, "{}", index.write_latex())?;
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

pub fn write_content(
    index: ContentIndex,
    files: Vec<ContentFile>,
    input_dir: &str,
    theme: String,
    variables: HashMap<String, Variable>,
) -> Result<()> {
    let temp_dir = TempDir::new("temp_dir_example")?;
    copy_theme_dir_to_temp(&temp_dir, theme)?;
    copy_content_to_temp_dir(&temp_dir, files, input_dir)?;
    write_content_index_to_file(&temp_dir, &index)?;
    write_variables(&temp_dir, variables)?;
    println!("It got here");
    run_latexmk(&temp_dir)?;
    move_main_pdf_to_working_dir(&temp_dir)?;
    temp_dir.close()?;
    Ok(())
}
