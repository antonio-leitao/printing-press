use crate::Variable;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_while},
    sequence::{delimited, preceded},
    IResult,
};

fn is_acceptable_key(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_acceptable_value(c: char) -> bool {
    c != '%' && c != '}'
}

fn comment_parser(input: &str) -> IResult<&str, &str> {
    let (input, _) = take_while(|c| c == '\t' || c == ' ')(input)?;
    let (input, _) = tag("%")(input)?; // Consume the '%'
    let (input, comment) = take_till(|c| c == '\n' || c == '\r')(input)?;
    Ok((input, comment))
}

fn variable_parser(input: &str) -> IResult<&str, Variable> {
    //ignore spaces
    let (input, _) = take_while(|c| c == '\t' || c == ' ')(input)?;
    let (input, name) = delimited(
        tag("\\newcommand{\\"),
        take_while(is_acceptable_key),
        tag("}"),
    )(input)?;
    let (input, value) = delimited(tag("{"), take_while(is_acceptable_value), tag("}"))(input)?;

    let comment = match comment_parser(input) {
        Ok((_, comment)) => Some(comment.to_string()),
        Err(_) => None,
    };
    Ok((
        input,
        Variable {
            name: name.to_string(),
            value: Some(value.to_string()),
            is_boolean: false,
            comment,
        },
    ))
}

fn boolean_parser(input: &str) -> IResult<&str, Variable> {
    let (input, _) = take_while(|c| c == '\t' || c == ' ')(input)?;
    let (input, name) = preceded(tag("\\newif\\if"), take_while(is_acceptable_key))(input)?;
    let comment = match comment_parser(input) {
        Ok((_, comment)) => Some(comment.to_string()),
        Err(_) => None,
    };

    Ok((
        input,
        Variable {
            name: name.to_string(),
            value: None,
            is_boolean: true,
            comment,
        },
    ))
}

pub fn latex_parser(input: &str) -> IResult<&str, Variable> {
    alt((variable_parser, boolean_parser))(input)
}
