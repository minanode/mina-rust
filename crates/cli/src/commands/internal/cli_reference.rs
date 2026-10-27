//! The CLI reference, generated from the clap definitions: every visible
//! command and option with its help, default, environment variable and
//! possible values. `docs/cli-reference.generated.md` and the site's
//! `content/cli-reference.json` are its committed output; a test fails when
//! they differ from what this module generates.

use clap::{builder::PossibleValue, Arg, ArgAction, Command};
use serde::Serialize;

/// The committed Markdown reference, from the repository root.
pub const MARKDOWN_PATH: &str = "docs/cli-reference.generated.md";
/// The committed JSON reference the site renders, from the repository root.
pub const JSON_PATH: &str = "ui/minanode-site/content/cli-reference.json";

/// Writes the CLI reference to standard output.
#[derive(Debug, clap::Args)]
pub struct CliReference {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    pub format: Format,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Format {
    Markdown,
    Json,
}

impl CliReference {
    pub fn run(self) -> anyhow::Result<()> {
        let reference = Reference::of_mina();
        let text = match self.format {
            Format::Markdown => reference.markdown(),
            Format::Json => reference.json()?,
        };
        print!("{text}");
        Ok(())
    }
}

/// Every visible command, depth first, in the order `--help` lists them.
#[derive(Debug, Serialize)]
pub struct Reference {
    pub commands: Vec<CommandRef>,
}

#[derive(Debug, Serialize)]
pub struct CommandRef {
    /// The words after `mina`; empty for `mina` itself.
    pub path: Vec<String>,
    /// The anchor of the command: its path joined with `-`, `mina` for the
    /// top level.
    pub id: String,
    pub about: String,
    /// Paragraphs after the first line of help, if any.
    pub details: Vec<String>,
    pub usage: String,
    pub subcommands: Vec<SubcommandRef>,
    /// Options and arguments in groups, as `--help` shows them.
    pub groups: Vec<GroupRef>,
}

#[derive(Debug, Serialize)]
pub struct SubcommandRef {
    pub name: String,
    pub id: String,
    pub about: String,
}

#[derive(Debug, Serialize)]
pub struct GroupRef {
    pub title: String,
    pub args: Vec<ArgRef>,
}

#[derive(Debug, Serialize)]
pub struct ArgRef {
    /// The anchor of the option: the command's id, `--` and its name.
    pub id: String,
    /// How it is written: `-p, --port <PORT>` or `<KIND>`.
    pub syntax: String,
    pub aliases: Vec<String>,
    pub help: Vec<String>,
    pub default: Option<String>,
    pub env: Option<String>,
    pub possible_values: Vec<ValueRef>,
    pub required: bool,
    pub repeatable: bool,
    /// Accepted by every subcommand as well.
    pub global: bool,
}

#[derive(Debug, Serialize)]
pub struct ValueRef {
    pub name: String,
    pub help: Option<String>,
}

impl Reference {
    /// The reference of the `mina` binary.
    pub fn of_mina() -> Self {
        Self::of(<super::super::MinaCli as clap::CommandFactory>::command())
    }

    pub fn of(mut root: Command) -> Self {
        root.build();
        let mut commands = Vec::new();
        visit(&root, &[], &mut commands);
        Self { commands }
    }

    pub fn json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)? + "\n")
    }

    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# mina command-line reference\n\n");
        out.push_str(
            "Generated from the command definitions by `mina internal cli-reference \
             --format markdown`. Do not edit: run that command, or the `mina-cli` \
             test `cli_reference_is_current` with `MINA_UPDATE_CLI_REFERENCE=1`, \
             to update it.\n\n",
        );
        for command in &self.commands {
            command.markdown(&mut out);
        }
        out.truncate(out.trim_end().len());
        out.push('\n');
        out
    }
}

impl CommandRef {
    fn title(&self) -> String {
        std::iter::once("mina")
            .chain(self.path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn markdown(&self, out: &mut String) {
        out.push_str(&format!("## {}\n\n", self.title()));
        if !self.about.is_empty() {
            out.push_str(&format!("{}\n\n", self.about));
        }
        for paragraph in &self.details {
            out.push_str(&format!("{paragraph}\n\n"));
        }
        out.push_str(&format!("```text\n{}\n```\n\n", self.usage));
        if !self.subcommands.is_empty() {
            out.push_str("### Commands\n\n");
            for sub in &self.subcommands {
                out.push_str(&format!("- `{}`: {}\n", sub.name, sub.about));
            }
            out.push('\n');
        }
        for group in &self.groups {
            out.push_str(&format!("### {}\n\n", group.title));
            for arg in &group.args {
                arg.markdown(out);
            }
        }
    }
}

impl ArgRef {
    fn markdown(&self, out: &mut String) {
        out.push_str(&format!("#### `{}`\n\n", self.syntax));
        for paragraph in &self.help {
            out.push_str(&format!("{paragraph}\n\n"));
        }
        let facts = self.facts();
        if !facts.is_empty() {
            out.push_str(&format!("{}\n\n", facts.join(" ")));
        }
        if !self.possible_values.is_empty() {
            out.push_str("Values:\n\n");
            for value in &self.possible_values {
                match &value.help {
                    Some(help) => out.push_str(&format!("- `{}`: {help}\n", value.name)),
                    None => out.push_str(&format!("- `{}`\n", value.name)),
                }
            }
            out.push('\n');
        }
    }

    /// One sentence per fact: required, global, alias, default, variable.
    fn facts(&self) -> Vec<String> {
        let mut facts = Vec::new();
        if self.required {
            facts.push("Required.".to_owned());
        }
        if self.global {
            facts.push("Accepted by every command.".to_owned());
        }
        if !self.aliases.is_empty() {
            let aliases: Vec<String> = self.aliases.iter().map(|a| format!("`{a}`")).collect();
            facts.push(format!("Alias: {}.", aliases.join(", ")));
        }
        if let Some(default) = &self.default {
            facts.push(format!("Default: `{default}`."));
        }
        if let Some(env) = &self.env {
            facts.push(format!("Environment: `{env}`."));
        }
        facts
    }
}

fn visit(command: &Command, path: &[String], commands: &mut Vec<CommandRef>) {
    let id = if path.is_empty() {
        "mina".to_owned()
    } else {
        path.join("-")
    };
    let (about, details) = about_of(command);
    let subcommands: Vec<&Command> = visible_subcommands(command).collect();
    commands.push(CommandRef {
        path: path.to_vec(),
        id: id.clone(),
        about,
        details,
        usage: usage(command, path),
        subcommands: subcommands
            .iter()
            .map(|sub| SubcommandRef {
                name: sub.get_name().to_owned(),
                id: path
                    .iter()
                    .map(String::as_str)
                    .chain([sub.get_name()])
                    .collect::<Vec<_>>()
                    .join("-"),
                about: about_of(sub).0,
            })
            .collect(),
        groups: groups(command, &id, path.is_empty()),
    });
    for sub in subcommands {
        let mut path = path.to_vec();
        path.push(sub.get_name().to_owned());
        visit(sub, &path, commands);
    }
}

fn visible_subcommands(command: &Command) -> impl Iterator<Item = &Command> {
    command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
}

/// The first line of the command's help, and the paragraphs after it.
fn about_of(command: &Command) -> (String, Vec<String>) {
    let short = command
        .get_about()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let long = command
        .get_long_about()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut paragraphs = paragraphs(if long.is_empty() { &short } else { &long });
    if paragraphs.is_empty() {
        return (String::new(), Vec::new());
    }
    let first = paragraphs.remove(0);
    (first, paragraphs)
}

/// The paragraphs of a help text, each a sentence ending in a full stop:
/// clap drops the full stop of a one-paragraph help.
fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|paragraph| {
            let mut paragraph = paragraph
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned();
            if !paragraph.is_empty() && !paragraph.ends_with(['.', ':', '`']) {
                paragraph.push('.');
            }
            paragraph
        })
        .filter(|paragraph| !paragraph.is_empty())
        .collect()
}

fn usage(command: &Command, path: &[String]) -> String {
    let mut command = command.clone();
    let usage = command.render_usage().to_string();
    let usage = usage.trim().trim_start_matches("Usage:").trim();
    // A subcommand's usage starts with its own name; give the full path.
    match path.split_last() {
        Some((last, parents)) if usage.starts_with(last.as_str()) => std::iter::once("mina")
            .chain(parents.iter().map(String::as_str))
            .chain([usage])
            .collect::<Vec<_>>()
            .join(" "),
        _ => usage.to_owned(),
    }
}

/// Visible arguments, positionals first, then each help heading in the
/// order its first option is declared, `Options` for those without one.
fn groups(command: &Command, command_id: &str, top_level: bool) -> Vec<GroupRef> {
    let mut groups: Vec<GroupRef> = Vec::new();
    let mut add = |title: &str, arg: ArgRef| match groups.iter_mut().find(|g| g.title == title) {
        Some(group) => group.args.push(arg),
        None => groups.push(GroupRef {
            title: title.to_owned(),
            args: vec![arg],
        }),
    };
    let visible = command.get_arguments().filter(|arg| {
        !arg.is_hide_set()
            && !matches!(arg.get_action(), ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version)
            // A global option is listed once, on the command that defines it.
            && (top_level || !arg.is_global_set())
    });
    let (positionals, options): (Vec<&Arg>, Vec<&Arg>) =
        visible.partition(|arg| arg.is_positional());
    for arg in positionals {
        add("Arguments", arg_ref(arg, command_id));
    }
    for arg in options {
        add(
            arg.get_help_heading().unwrap_or("Options"),
            arg_ref(arg, command_id),
        );
    }
    groups
}

fn value_names(arg: &Arg) -> Vec<String> {
    match arg.get_value_names() {
        Some(names) => names.iter().map(|name| name.to_string()).collect(),
        None => vec![arg.get_id().as_str().to_uppercase()],
    }
}

fn arg_ref(arg: &Arg, command_id: &str) -> ArgRef {
    let takes_value = arg.get_action().takes_values();
    let values = value_names(arg)
        .iter()
        .map(|name| format!("<{name}>"))
        .collect::<Vec<_>>()
        .join(" ");
    let optional_value = takes_value
        && arg
            .get_num_args()
            .is_some_and(|range| range.min_values() == 0);
    let syntax = if arg.is_positional() {
        values
    } else {
        let mut names = Vec::new();
        if let Some(short) = arg.get_short() {
            names.push(format!("-{short}"));
        }
        if let Some(long) = arg.get_long() {
            names.push(format!("--{long}"));
        }
        let names = names.join(", ");
        match (takes_value, optional_value) {
            (false, _) => names,
            (true, true) => format!("{names}[=<{}>]", value_names(arg).join(" ")),
            (true, false) => format!("{names} {values}"),
        }
    };
    let name = arg
        .get_long()
        .map(str::to_owned)
        .unwrap_or_else(|| arg.get_id().as_str().to_owned());
    let help = arg
        .get_long_help()
        .or(arg.get_help())
        .map(|help| paragraphs(&help.to_string()))
        .unwrap_or_default();
    let mut aliases: Vec<String> = arg
        .get_visible_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(|alias| format!("--{alias}"))
        .collect();
    aliases.extend(
        arg.get_visible_short_aliases()
            .unwrap_or_default()
            .into_iter()
            .map(|alias| format!("-{alias}")),
    );
    let default = (takes_value && !arg.is_hide_default_value_set())
        .then(|| {
            arg.get_default_values()
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|default| !default.is_empty());
    let env = arg.get_env().map(|env| env.to_string_lossy().into_owned());
    let possible_values = if arg.is_hide_possible_values_set() || !takes_value {
        Vec::new()
    } else {
        arg.get_possible_values()
            .iter()
            .filter(|value| !value.is_hide_set())
            .map(value_ref)
            .collect()
    };
    ArgRef {
        id: format!("{command_id}--{name}"),
        syntax,
        aliases,
        help,
        default,
        env,
        possible_values,
        required: arg.is_required_set(),
        repeatable: matches!(arg.get_action(), ArgAction::Append | ArgAction::Count),
        global: arg.is_global_set(),
    }
}

fn value_ref(value: &PossibleValue) -> ValueRef {
    ValueRef {
        name: value.get_name().to_owned(),
        help: value
            .get_help()
            .and_then(|help| paragraphs(&help.to_string()).into_iter().next()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_file(path: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    }

    // The committed reference is the binary's: regenerate it after changing a
    // command or option, with MINA_UPDATE_CLI_REFERENCE=1 on this test.
    #[test]
    fn cli_reference_is_current() {
        let reference = Reference::of_mina();
        let expected = [
            (MARKDOWN_PATH, reference.markdown()),
            (JSON_PATH, reference.json().unwrap()),
        ];
        if std::env::var_os("MINA_UPDATE_CLI_REFERENCE").is_some() {
            for (path, text) in &expected {
                std::fs::write(repo_file(path), text).unwrap();
            }
        }
        for (path, text) in expected {
            let committed = std::fs::read_to_string(repo_file(path)).unwrap_or_default();
            assert!(
                committed == text,
                "{path} differs from the command definitions. Regenerate it with \
                 `MINA_UPDATE_CLI_REFERENCE=1 cargo test -p mina-cli --lib \
                 cli_reference_is_current` and commit the result."
            );
        }
    }

    #[test]
    fn hidden_options_and_commands_are_left_out() {
        let reference = Reference::of_mina();
        let node = reference
            .commands
            .iter()
            .find(|command| command.id == "node")
            .unwrap();
        let syntax: Vec<&str> = node
            .groups
            .iter()
            .flat_map(|group| &group.args)
            .map(|arg| arg.syntax.as_str())
            .collect();
        assert!(syntax.iter().any(|s| s.starts_with("-p, --port")));
        assert!(!syntax.iter().any(|s| s.contains("--record")));
        assert!(!syntax.iter().any(|s| s.contains("--network")));
        assert!(!reference
            .commands
            .iter()
            .any(|command| command.path.first().is_some_and(|word| word == "internal")));
    }

    #[test]
    fn the_reference_has_no_trailing_whitespace() {
        let reference = Reference::of_mina();
        for text in [reference.markdown(), reference.json().unwrap()] {
            assert!(text.lines().all(|line| line == line.trim_end()));
            assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
        }
    }
}
