use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::env;
mod cli;
use cli::ParsedAction;
mod parser_tex;
mod settings;
mod theme;
mod writer;

#[derive(Debug)]
pub struct Variable {
    name: String,
    value: Option<String>,
    is_boolean: bool,
    comment: Option<String>,
}

fn update_variables(original: &mut Vec<Variable>, new: Vec<Variable>) {
    for variable in new {
        if let Some(index) = original.iter().position(|v| v.name == variable.name) {
            // Variable with the same name exists in the original, overwrite it
            original[index] = variable;
        } else {
            // Variable doesn't exist in the original, add it
            original.push(variable);
        }
    }
}

fn merge_and_validate_variables(
    theme_name: String,
    cli_options: Vec<Variable>,
) -> Result<Vec<Variable>> {
    // get default variables
    let mut variables = theme::theme_variables(theme_name)?;
    //get user defined variables
    let mut user_variables = theme::read_theme_variables("press_variables.tex")?;
    update_variables(&mut user_variables, cli_options);
    //validate and merge
    validate_variables(&mut variables, user_variables)?;
    Ok(variables)
}

fn validate_variables(original: &mut Vec<Variable>, new: Vec<Variable>) -> Result<()> {
    // Create a set of names from the original vector for quick lookup
    let original_names: HashSet<String> = original.iter().map(|v| v.name.clone()).collect();
    // Check if all names from the new vector are in the original vector
    for variable in &new {
        if !original_names.contains(&variable.name) {
            return Err(anyhow!("Unrecognized variable name '{}'", variable.name));
        }
    }
    // Merge the variables
    update_variables(original, new);
    Ok(())
}

fn variables_to_hashmap(variables: Vec<Variable>) -> HashMap<String, Variable> {
    let mut hm = HashMap::new();
    for variable in variables.into_iter() {
        hm.insert(variable.name.clone(), variable);
    }
    hm
}

fn main_loop(theme: String, cli_variables: Vec<Variable>) -> Result<()> {
    //THIS FUNCTION IS A BIT OF A MESS EH
    //get user variables
    let variables = merge_and_validate_variables(theme.clone(), cli_variables)?;
    let config = variables_to_hashmap(variables);
    //write content into tempdir and run latexmk
    writer::write_content(theme, config)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    match cli::parse(args) {
        ParsedAction::Help => cli::print_general_help(),
        ParsedAction::ThemeHelp { name } => cli::print_theme_help(name),
        ParsedAction::Theme { name, variables } => {
            println!("Main loop for {}", name);
            match main_loop(name, variables) {
                Ok(()) => println!("Finished succesfully"),
                Err(err) => println!("{}", err),
            }
        }
    }
}
