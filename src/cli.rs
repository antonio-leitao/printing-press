use crate::theme;
use crate::Variable;
use anyhow::Error;
use nom::{
    bytes::complete::{tag, take_while1},
    sequence::preceded,
    IResult,
};
use std::collections::HashMap;

pub enum ParsedAction {
    Help,
    ThemeHelp {
        name: String,
    },
    Theme {
        name: String,
        variables: Vec<Variable>,
    },
}

fn is_valid_argument_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn argument_parser(input: &str) -> IResult<&str, &str> {
    preceded(tag("--"), take_while1(is_valid_argument_char))(input)
}

fn separate_args(input_args: Vec<String>) -> HashMap<String, Option<String>> {
    let mut options = HashMap::new();
    let mut prev_key = String::new();

    for arg in input_args.iter() {
        if let Ok((_, arg_name)) = argument_parser(arg) {
            // This is a key
            prev_key = arg_name.to_string();
            options.insert(prev_key.clone(), None);
        } else {
            // This is a value for the previous key
            options.insert(prev_key.clone(), Some(arg.clone()));
        }
    }

    options
}

fn is_general_help(args: Vec<String>) -> bool {
    args.len() < 2 || &args[1] == "--help" || &args[1] == "-h"
}

fn theme_and_options(args: Vec<String>) -> (String, HashMap<String, Option<String>>) {
    let theme = &args[1];
    let options = separate_args(args[2..].to_vec());
    (theme.to_string(), options)
}

fn separate_variables(variables: Vec<Variable>) -> (Vec<Variable>, Vec<Variable>) {
    let mut bool_variables = Vec::new();
    let mut non_bool_variables = Vec::new();

    for variable in variables.into_iter() {
        if variable.is_boolean {
            bool_variables.push(variable);
        } else {
            non_bool_variables.push(variable);
        }
    }

    (bool_variables, non_bool_variables)
}

fn print_variable_help(variables: Vec<Variable>) {
    // Calculate the maximum variable name length
    let max_name_length = variables
        .iter()
        .map(|variable| variable.name.len())
        .max()
        .unwrap_or(0);

    for variable in variables {
        let description = variable.comment.unwrap_or("".to_string());
        // Print with adjusted spacing based on the maximum name length
        println!(
            "  --{:width$}{}",
            variable.name,
            description,
            width = max_name_length + 3
        );
    }
}

fn print_error(err: Error) {
    println!("{}\n", err);
    println!("Usage:");
    println!("  press <theme> [variables]");
    println!("  press <theme> --help");
    println!("  press --help\n");
}

fn variables_from_options(options: HashMap<String, Option<String>>) -> Vec<Variable> {
    let mut variables = Vec::new();
    for (key, value) in options.into_iter() {
        variables.push(Variable {
            name: key,
            value: value.clone(),
            is_boolean: !value.is_some(),
            comment: None,
        })
    }
    variables
}
/// COMPOSITE FUNCTIONS

pub fn print_general_help() {
    // Display general help
    match theme::all_themes() {
        Ok(themes) => {
            println!("Usage:");
            println!("  press <theme> [variables]");
            println!("  press <theme> --help");
            println!("  press --help\n");
            println!("Themes:");
            themes.into_iter().for_each(|theme| println!("  {}", theme))
        }
        Err(err) => print_error(err),
    }
}

pub fn print_theme_help(name: String) {
    // Get theme variables
    println!("Applies the {} template", name);
    match theme::theme_variables(name) {
        Ok(variables) => {
            let (flags, arguments) = separate_variables(variables);
            println!("\nArguments:");
            print_variable_help(arguments);
            if flags.len() > 0 {
                println!("\nFlags:");
                print_variable_help(flags);
            }
        }
        Err(err) => println!("{}", err),
    };
}

pub fn parse(input_args: Vec<String>) -> ParsedAction {
    //cargo run -- --author "me and myself" --chapter --title "Hello"
    if is_general_help(input_args.clone()) {
        return ParsedAction::Help;
    }
    let (name, options) = theme_and_options(input_args);
    if options.contains_key("help") {
        return ParsedAction::ThemeHelp { name };
    }
    let variables = variables_from_options(options);
    ParsedAction::Theme { name, variables }
}
