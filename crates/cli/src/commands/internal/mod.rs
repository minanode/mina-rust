pub mod cli_reference;
pub mod graphql;

#[derive(Debug, clap::Args)]
pub struct Internal {
    #[command(subcommand)]
    pub command: InternalCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum InternalCommand {
    /// GraphQL endpoint introspection and management.
    Graphql(graphql::Graphql),
    /// Print the command-line reference of every visible command.
    #[command(hide = true)]
    CliReference(cli_reference::CliReference),
}

impl Internal {
    pub fn run(self) -> anyhow::Result<()> {
        match self.command {
            InternalCommand::Graphql(v) => v.run(),
            InternalCommand::CliReference(v) => v.run(),
        }
    }
}
