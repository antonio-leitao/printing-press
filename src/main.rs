use regex::Regex;
use std::env;

// Struct to represent a variable
#[derive(Debug)]
struct Variable {
    name: String,
    is_bool: bool,
    description: String,
}

fn parse_bool_variable(line: &str) -> Option<Variable> {
    let re_bool = Regex::new(r"^\\newif\\if([^%\s]*) *%?(.*)").unwrap();

    if let Some(captures) = re_bool.captures(line) {
        if let Some(name) = captures.get(1) {
            let description = captures.get(2).unwrap().as_str().trim().to_string();
            return Some(Variable {
                name: name.as_str().to_string(),
                is_bool: true,
                description,
            });
        }
    }
    None
}

fn parse_non_bool_variable(line: &str) -> Option<Variable> {
    let re_non_bool = Regex::new(r"^\\newcommand\{\\(\S*)\}\{([^}]*)} *%?(.*)").unwrap();

    if let Some(captures) = re_non_bool.captures(line) {
        if let Some(name) = captures.get(1) {
            let description = captures.get(3).unwrap().as_str().trim().to_string();
            return Some(Variable {
                name: name.as_str().to_string(),
                is_bool: false,
                description,
            });
        }
    }
    None
}

fn read_variables(file_path: &str) -> Vec<Variable> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(_) => {
            println!("Error reading variables file.");
            return Vec::new();
        }
    };

    let mut variables = Vec::new();

    for line in content.lines() {
        if line.starts_with('%') {
            continue;
        }

        if let Some(variable) = parse_bool_variable(line) {
            variables.push(variable);
        } else if let Some(variable) = parse_non_bool_variable(line) {
            variables.push(variable);
        }
    }

    variables
}


fn parse_args(args: Vec<String>) {
    // Check for the number of arguments
    if args.len() < 2 {
        println!("Usage: press <theme> [--help] [variables]");
        return;
    }

    let theme = &args[1];

    match theme.as_str() {
        "--help" | "-h" => print_general_help(),
        _ => {
            // Check if help is requested for the theme
            if args.len() > 2 && (args[2] == "--help" || args[2] == "-h") {
                // Display theme-specific help
                println!("Theme Help for '{}':", theme);
                // Assuming you have a function to fetch theme-specific variables
                let theme_variables = get_theme_variables(theme);
                print_theme_help(theme_variables);
            } else {
                // Parse variables for the theme
                let theme_variables = get_theme_variables(theme);
                parse_user_input(args, theme_variables);
            }
        }
    }
}


fn get_theme_variables(theme: &str) -> Vec<Variable> {
    // Logic to fetch theme-specific variables based on the theme
    // For demonstration purposes, let's return a default set of variables
    match theme {
        "book" => read_variables("variables.tex"),
        _ => vec![],
    }
}

// Function to parse user input and set variables accordingly
fn parse_user_input(input_args: Vec<String>, variables: Vec<Variable>) {
    for arg in input_args.iter() {
        if arg.starts_with("--") {
            let variable_name = &arg[2..];  // Remove the leading '--'
              // Check if the variable exists in the variables list
            if let Some(variable) = variables.iter().find(|v| v.name == variable_name) {
                if variable.is_bool {
                    // For boolean variables, toggle the value
                    println!("Setting {} to true", variable_name);
                    // In a real application, you'd toggle the boolean value accordingly
                } else {
                    // For non-boolean variables, set the value provided in the next argument
                    if let Some(value) = input_args.iter().position(|v| v == arg) {
                        if let Some(new_value) = input_args.get(value + 1) {
                            println!("Setting {} to {}", variable_name, new_value);
                            // In a real application, you'd update the non-boolean variable value accordingly
                        } else {
                            println!("No value provided for {}.", variable_name);
                        }
                    }
                }
            } else {
                println!("Invalid variable: {}", variable_name);
            }
        }
    }
}


fn print_general_help(){
    // Display general help
    let themes = get_theme_list();
    println!("Usage:");
    println!("  press <theme> [variables]");
    println!("  press <theme> --help");
    println!("  press --help\n");
    println!("Themes:");
    themes.into_iter().for_each(|theme|println!("  {}",theme))
}
fn get_theme_list()->Vec<String>{
    vec!["Book".to_string(),"Draft".to_string(),"Paper".to_string(),"IEEE".to_string()]
}


fn separate_variables(
    variables: Vec<Variable>,
) -> (Vec<Variable>, Vec<Variable>) {
    let mut bool_variables = Vec::new();
    let mut non_bool_variables = Vec::new();

    for variable in variables.into_iter() {
        if variable.is_bool {
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
        let description = if variable.description.is_empty() {
            "".to_string()
        } else {
            format!("{}", variable.description)
        };
        // Print with adjusted spacing based on the maximum name length
        println!(
            "  --{:width$}{}",
            variable.name,
            description,
            width = max_name_length + 3
        );
    }
}

fn print_theme_help(variables: Vec<Variable>){

    let (flags,arguments) = separate_variables(variables);
    println!("\nArguments:");
    print_variable_help(arguments);
    println!("\nFlags:");
    print_variable_help(flags);
}

fn main() {
    let input_args: Vec<String> = env::args().collect();
    parse_args(input_args);
}
