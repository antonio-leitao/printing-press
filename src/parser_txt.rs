use nom::{
    branch::alt,
    bytes::complete::{tag, take_until},
    character::complete::none_of,
    combinator::{map, peek},
    multi::many1,
    sequence::delimited,
    IResult,
};

#[derive(Debug)]
enum ParsedElement {
    Bold(String),
    Italics(String),
    Quote(String),
}

fn parse_bold(input: &str) -> IResult<&str, ParsedElement> {
    map(
        delimited(tag("*"), many1(none_of("*")), tag("*")),
        |content: Vec<char>| ParsedElement::Bold(content.into_iter().collect()),
    )(input)
}

fn parse_italics(input: &str) -> IResult<&str, ParsedElement> {
    map(
        delimited(tag("_"), many1(none_of("_")), tag("_")),
        |content: Vec<char>| ParsedElement::Italics(content.into_iter().collect()),
    )(input)
}

fn parse_quote(input: &str) -> IResult<&str, ParsedElement> {
    map(
        delimited(tag("\n\n\t"), take_until("\n\n"), peek(tag("\n\n"))),
        |content: &str| ParsedElement::Quote(content.replace("\t", "")),
    )(input)
}

fn parse_element(input: &str) -> IResult<&str, ParsedElement> {
    alt((parse_bold, parse_italics, parse_quote))(input)
}

fn transform_element(element: ParsedElement) -> String {
    match element {
        ParsedElement::Bold(content) => format!("\\textbf{{{}}}", content),
        ParsedElement::Italics(content) => format!("\\emph{{{}}}", content),
        ParsedElement::Quote(content) => format!("\\quote{{{}}}", content),
    }
}

pub fn parse_markdown(input: &str) -> String {
    let mut transformed_output = String::new();

    let mut remaining_input = input;
    while !remaining_input.is_empty() {
        if let Ok((next_remaining, parsed)) = parse_element(remaining_input) {
            transformed_output.push_str(&transform_element(parsed));
            remaining_input = next_remaining;
        } else {
            transformed_output.push_str(&remaining_input[..1]);
            remaining_input = &remaining_input[1..];
        }
    }

    transformed_output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parser() {
        let markdown_input = "This is *bold* and _italics_ text.\n\n\tThis is a quoted block.\n\tThat spans multiple lines.\n\n\tHello there\n\n";
        let latex_output = parse_markdown(markdown_input);
        let expected_output = "This is \\textbf{bold} and \\emph{italics} text.\\quote{This is a quoted block.\nThat spans multiple lines.}\\quote{Hello there}\n\n";
        assert_eq!(latex_output, expected_output);
    }
}
