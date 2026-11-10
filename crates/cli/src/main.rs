mod batch;
mod terminal;
use batch::{read_bytes, read_text};
use clap::{Args, Parser, Subcommand};
use rustytavern::{Error, Result};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rustytavern",
    version,
    about = "RustyTavern runs persistent character conversations."
)]
struct Cli {
    /// The home directory holds config.toml and rustytavern.db. The default is ~/.rustytavern.
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// The option approves every tool call that `[plugins.agent] confirm_tools` holds. Without it, a script command declines them.
    #[arg(long, global = true)]
    approve_tools: bool,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Worldbook {
        #[command(subcommand)]
        command: WorldbookCommand,
    },
    Card {
        #[command(subcommand)]
        command: CardCommand,
    },
    Persona {
        #[command(subcommand)]
        command: PersonaCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Prompt {
        #[command(subcommand)]
        command: PromptCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// The memory holds a summary of the chat and facts about the character and the persona.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Extension {
        #[command(subcommand)]
        command: ExtensionCommand,
    },
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    #[command(external_subcommand)]
    PluginAlias(Vec<String>),
    Chat {
        session: String,
        #[command(flatten)]
        input: MessageInput,
        #[arg(long)]
        events: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}
#[derive(Subcommand)]
enum ModelCommand {
    Discover {
        provider: String,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        input: Option<String>,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        parameter: Option<String>,
    },
    Select {
        provider: String,
        model: String,
    },
}
#[derive(Subcommand)]
enum WorldbookCommand {
    Import {
        path: PathBuf,
    },
    Inspect {
        worldbook: String,
    },
    List,
    Export {
        worldbook: String,
        #[arg(long)]
        output: PathBuf,
    },
    Bind {
        session: String,
        #[arg(long = "book")]
        books: Vec<String>,
        #[arg(long)]
        expected_version: Option<i64>,
    },
}
#[derive(Subcommand)]
enum CardCommand {
    Import {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Inspect {
        character: String,
        #[arg(long)]
        json: bool,
    },
    Asset {
        character: String,
        #[arg(long)]
        uri: String,
        #[arg(long)]
        output: PathBuf,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Fetch {
        url: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        json: bool,
        /// The command prints the complete card and does not save it.
        #[arg(long)]
        preview: bool,
    },
    Export {
        character: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        normalized: bool,
        #[arg(long)]
        patch: Option<PathBuf>,
    },
}
#[derive(Subcommand)]
enum PersonaCommand {
    Export {
        persona: String,
        #[arg(long)]
        output: PathBuf,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, conflicts_with = "description_file")]
        description: Option<String>,
        #[arg(long)]
        description_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Show {
        persona: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}
#[derive(Subcommand)]
enum SessionCommand {
    /// The command sets one plugins key for this session, for example plugins.agent.max_steps 4.
    Set {
        session: String,
        key: String,
        /// The value uses TOML syntax.
        value: String,
    },
    /// The command removes one session setting.
    Unset {
        session: String,
        key: String,
    },
    /// The command prints the session settings and the enabled plugins.
    Settings {
        session: String,
    },
    Configure {
        session: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        context_limit: Option<usize>,
        #[arg(long)]
        output_limit: Option<usize>,
        #[arg(long)]
        expected_version: Option<i64>,
    },
    AcceptDraft {
        session: String,
        generation: String,
        #[arg(long)]
        expected_version: Option<i64>,
    },
    New {
        #[arg(long)]
        character: String,
        #[arg(long)]
        persona: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = 0)]
        greeting: usize,
        #[arg(long)]
        accept_reduced: bool,
        #[arg(long)]
        json: bool,
    },
    Show {
        session: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Export {
        session: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        include_traces: bool,
    },
    Regenerate {
        session: String,
        #[arg(long)]
        events: bool,
    },
    Retry {
        session: String,
        #[arg(long)]
        events: bool,
    },
    Branch {
        session: String,
        #[arg(long)]
        at: String,
    },
    Edit {
        session: String,
        #[arg(long)]
        at: String,
        #[command(flatten)]
        input: MessageInput,
    },
    /// The command extends the last reply. The original reply stays as an alternative.
    Continue {
        session: String,
        #[arg(long)]
        events: bool,
    },
    /// The command writes the next user message for the user. The chat stores no message.
    Impersonate {
        session: String,
        #[arg(long)]
        events: bool,
    },
    /// The command copies the branch that ends at a message into a new chat.
    Fork {
        session: String,
        #[arg(long)]
        at: String,
    },
    /// The command sets the chat name. An empty name removes it.
    Rename {
        session: String,
        title: String,
    },
    /// The command deletes the chat and its history.
    Delete {
        session: String,
    },
    /// The command omits a message from the prompt. The chat still shows it.
    Hide {
        session: String,
        #[arg(long)]
        at: String,
        /// The flag includes the message in the prompt again.
        #[arg(long)]
        show: bool,
    },
    Rebind {
        session: String,
        #[arg(long)]
        character: Option<String>,
        #[arg(long)]
        persona: Option<String>,
        #[arg(long)]
        accept_reduced: bool,
    },
    Recover {
        session: String,
    },
}
#[derive(Subcommand)]
enum MemoryCommand {
    Show {
        session: String,
    },
    /// The command asks the chat model for a new summary and new facts.
    Update {
        session: String,
        /// The flag updates the memory only after the memory interval of new messages.
        #[arg(long)]
        if_due: bool,
    },
    /// The command replaces the summary or the facts. The facts have one fact on each line.
    Edit {
        session: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        facts: Option<String>,
    },
}
#[derive(Subcommand)]
enum PromptCommand {
    Replay {
        generation: String,
    },
    Inspect {
        session: String,
        #[command(flatten)]
        input: MessageInput,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        request: bool,
    },
}
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum ConfigCommand {
    Show,
    Capabilities,
    Provider {
        #[arg(long)]
        name: String,
        #[arg(long, value_parser=["openrouter","openai","anthropic","gemini","custom"])]
        kind: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        endpoint: Option<String>,
        /// The command stores the key in config.toml.
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value_t = 131072)]
        context_limit: usize,
        #[arg(long, default_value_t = 8192)]
        output_limit: usize,
        #[arg(long, default_value = "{}")]
        parameters: String,
        #[arg(long)]
        routing: Option<String>,
        #[arg(long)]
        merge_system_messages: bool,
    },
    /// The command reads a SillyTavern chat completion preset into the `[prompt]` table.
    /// Tavo and RisuAI read the same format.
    Preset {
        path: PathBuf,
    },
}
/// An extension changes only the application appearance.
#[derive(Subcommand)]
/// The `[extensions.NAME]` tables of config.toml define the extensions.
enum ExtensionCommand {
    List,
}
/// A plugin changes agent behavior or adds content to the agent context.
#[derive(Subcommand)]
enum PluginCommand {
    Install {
        path: PathBuf,
        #[arg(long)]
        grant: Vec<String>,
        #[arg(long)]
        trust_processes: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Inspect {
        plugin: String,
        #[arg(long)]
        json: bool,
    },
    Enable {
        plugin: String,
    },
    Disable {
        plugin: String,
    },
    Uninstall {
        plugin: String,
    },
    Run {
        command: String,
        #[command(flatten)]
        input: CommandInput,
    },
    Bind {
        service: String,
        plugin: String,
    },
}
#[derive(Subcommand)]
enum PresetCommand {
    Inspect {
        #[arg(default_value = "tavern")]
        preset: String,
        #[arg(long)]
        resolved: bool,
        #[arg(long)]
        json: bool,
    },
}
#[derive(Args)]
struct CommandInput {
    #[arg(long, conflicts_with = "args_file")]
    args: Option<String>,
    #[arg(long)]
    args_file: Option<PathBuf>,
    #[arg(long)]
    url: Option<String>,
}
impl CommandInput {
    fn read(&self) -> Result<Value> {
        let mut arguments = match (&self.args, &self.args_file) {
            (Some(value), None) => serde_json::from_str(value)?,
            (None, Some(path)) => serde_json::from_slice(&read_bytes(path, 1024 * 1024)?)?,
            _ => json!({}),
        };
        if let Some(url) = &self.url {
            if !arguments.is_object() {
                return Err(Error::new(
                    "invalid_command_arguments",
                    "The command URL requires an argument object.",
                ));
            }
            arguments["url"] = json!(url);
        }
        Ok(arguments)
    }
}
#[derive(Args)]
struct MessageInput {
    #[arg(long, conflicts_with = "message_file")]
    message: Option<String>,
    #[arg(long)]
    message_file: Option<PathBuf>,
}
impl MessageInput {
    fn read(&self) -> Result<String> {
        match (&self.message, &self.message_file) {
            (Some(text), None) => Ok(text.clone()),
            (None, Some(path)) => read_text(path),
            _ => Err(Error::new(
                "invalid_message",
                "The command requires --message or --message-file.",
            )),
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = if cli.command.is_some() {
        batch::run(cli).await
    } else {
        terminal::run(cli.home).await
    };
    if let Err(error) = result {
        eprintln!(
            "{}",
            serde_json::to_string(&error).unwrap_or_else(|_| "The operation failed.".into())
        );
        std::process::exit(error.exit_code());
    }
}
