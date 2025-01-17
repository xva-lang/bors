//! Defines parsers for bors commands.

use std::collections::HashSet;

use crate::bors::command::BorsCommand;
use crate::github::CommitSha;

#[derive(Debug, PartialEq)]
pub enum CommandParseError<'a> {
    MissingCommand,
    UnknownCommand(&'a str),
    MissingArgValue { arg: &'a str },
    UnknownArg(&'a str),
    DuplicateArg(&'a str),
    ValidationError(String),
}

/// Part of a command, either a bare string like `try` or a key value like `parent=<sha>`.
#[derive(PartialEq)]
enum CommandPart<'a> {
    Bare(&'a str),
    KeyValue { key: &'a str, value: &'a str },
}

pub struct CommandParser {
    prefix: String,
}

impl CommandParser {
    pub fn new(prefix: String) -> Self {
        Self { prefix }
    }

    /// Parses bors commands from the given string.
    ///
    /// Assumes that each command spands at most one line and that there are not more commands on
    /// each line.
    pub fn parse_commands<'a>(
        &self,
        text: &'a str,
    ) -> Vec<Result<BorsCommand, CommandParseError<'a>>> {
        // The order of the parsers in the vector is important
        let parsers: Vec<for<'b> fn(&'b str, &[CommandPart<'b>]) -> ParseResult<'b>> =
            vec![parser_ping, parser_try_cancel, parser_try];

        text.lines()
            .filter_map(|line| match line.find(&self.prefix) {
                Some(index) => {
                    let command = &line[index + self.prefix.len()..];
                    match parse_parts(command) {
                        Ok(parts) => {
                            if parts.is_empty() {
                                Some(Err(CommandParseError::MissingCommand))
                            } else {
                                let (command, rest) = parts.split_at(1);
                                match command[0] {
                                    CommandPart::Bare(command) => {
                                        for parser in &parsers {
                                            if let Some(result) = parser(command, rest) {
                                                return Some(result);
                                            }
                                        }
                                        Some(Err(CommandParseError::UnknownCommand(command)))
                                    }
                                    CommandPart::KeyValue { .. } => {
                                        Some(Err(CommandParseError::MissingCommand))
                                    }
                                }
                            }
                        }
                        Err(error) => Some(Err(error)),
                    }
                }
                None => None,
            })
            .collect()
    }
}

type ParseResult<'a> = Option<Result<BorsCommand, CommandParseError<'a>>>;

fn parse_parts(input: &str) -> Result<Vec<CommandPart>, CommandParseError> {
    let mut parts = vec![];
    let mut seen_keys = HashSet::new();

    for item in input.split_whitespace() {
        // Stop parsing, as this is a command for another bot, such as `@rust-timer queue`.
        if item.starts_with('@') {
            break;
        }

        match item.split_once('=') {
            Some((key, value)) => {
                if value.is_empty() {
                    return Err(CommandParseError::MissingArgValue { arg: key });
                }
                if seen_keys.contains(key) {
                    return Err(CommandParseError::DuplicateArg(key));
                }
                seen_keys.insert(key);
                parts.push(CommandPart::KeyValue { key, value });
            }
            None => parts.push(CommandPart::Bare(item)),
        }
    }
    Ok(parts)
}

/// Parsers

/// Parses "@bors ping".
fn parser_ping<'a>(command: &'a str, _parts: &[CommandPart<'a>]) -> ParseResult<'a> {
    if command == "ping" {
        Some(Ok(BorsCommand::Ping))
    } else {
        None
    }
}

fn parse_sha(input: &str) -> Result<CommitSha, String> {
    if input.len() != 40 {
        return Err("SHA must have exactly 40 characters".to_string());
    }
    Ok(CommitSha(input.to_string()))
}

/// Parses "@bors try <parent=sha>".
fn parser_try<'a>(command: &'a str, parts: &[CommandPart<'a>]) -> ParseResult<'a> {
    if command != "try" {
        return None;
    }

    let mut parent = None;

    for part in parts {
        match part {
            CommandPart::Bare(key) => {
                return Some(Err(CommandParseError::UnknownArg(key)));
            }
            CommandPart::KeyValue { key, value } => {
                if *key == "parent" {
                    parent = match parse_sha(value) {
                        Ok(sha) => Some(sha),
                        Err(error) => {
                            return Some(Err(CommandParseError::ValidationError(format!(
                                "Try parent has to be a valid commit SHA: {error}"
                            ))));
                        }
                    };
                } else {
                    return Some(Err(CommandParseError::UnknownArg(key)));
                }
            }
        }
    }
    Some(Ok(BorsCommand::Try { parent }))
}

/// Parses "@bors try cancel".
fn parser_try_cancel<'a>(command: &'a str, parts: &[CommandPart<'a>]) -> ParseResult<'a> {
    if command == "try" && parts.get(0) == Some(&CommandPart::Bare("cancel")) {
        Some(Ok(BorsCommand::TryCancel))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::bors::command::parser::{CommandParseError, CommandParser};
    use crate::bors::command::BorsCommand;
    use crate::github::CommitSha;

    fn get_command_prefix() -> String {
        dotenv::dotenv().ok();
        std::env::var("CMD_PREFIX").unwrap()
    }

    #[test]
    fn no_commands() {
        let cmds = parse_commands(r#"Hi, this PR looks nice!"#);
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn missing_command() {
        let command_prefix = get_command_prefix();
        let cmds = parse_commands(&command_prefix);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Err(CommandParseError::MissingCommand)));
    }

    #[test]
    fn unknown_command() {
        let command = format!("{} foo", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(
            cmds[0],
            Err(CommandParseError::UnknownCommand("foo"))
        ));
    }

    #[test]
    fn parse_arg_no_value() {
        let command = format!("{} ping a=", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(
            cmds[0],
            Err(CommandParseError::MissingArgValue { arg: "a" })
        ));
    }

    #[test]
    fn parse_duplicate_key() {
        let command = format!("{} ping a=b a=c", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Err(CommandParseError::DuplicateArg("a"))));
    }

    #[test]
    fn parse_ping() {
        let command = format!("{} ping", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::Ping)));
    }

    #[test]
    fn parse_ping_unknown_arg() {
        let command = format!("{} ping a", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::Ping)));
    }

    #[test]

    fn parse_command_multiline() {
        let command = format!(
            r#"
line one
{} try
line two"#,
            get_command_prefix()
        );
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::Try { parent: None })));
    }

    #[test]
    fn parse_try() {
        let command = format!("{} try", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::Try { parent: None })));
    }

    #[test]
    fn parse_try_parent() {
        let command = format!(
            "{} try parent=ea9c1b050cc8b420c2c211d2177811e564a4dc60",
            get_command_prefix()
        );
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            Ok(BorsCommand::Try {
                parent: Some(CommitSha(
                    "ea9c1b050cc8b420c2c211d2177811e564a4dc60".to_string()
                ))
            })
        );
    }

    #[test]
    fn parse_try_parent_invalid() {
        let command = format!("{} try parent=foo", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        insta::assert_debug_snapshot!(cmds[0], @r###"
        Err(
            ValidationError(
                "Try parent has to be a valid commit SHA: SHA must have exactly 40 characters",
            ),
        )
        "###);
    }

    #[test]
    fn parse_try_unknown_arg() {
        let command = format!("{} try a", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Err(CommandParseError::UnknownArg("a"))));
    }

    #[test]
    fn parse_try_unknown_kv_arg() {
        let command = format!("{} try a=b", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Err(CommandParseError::UnknownArg("a"))));
    }

    #[test]
    fn parse_try_with_rust_timer() {
        let command = format!(
            r#"
{} try @rust-timer queue
        "#,
            get_command_prefix()
        );
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::Try { parent: None })));
    }

    #[test]
    fn parse_try_cancel() {
        let command = format!("{} try cancel", get_command_prefix());
        let cmds = parse_commands(&command);
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Ok(BorsCommand::TryCancel)));
    }

    fn parse_commands(text: &str) -> Vec<Result<BorsCommand, CommandParseError>> {
        CommandParser::new(get_command_prefix()).parse_commands(text)
    }
}
