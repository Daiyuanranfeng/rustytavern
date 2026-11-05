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
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
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
    Extension {
        #[command(subcommand)]
        command: ExtensionCommand,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    #[command(external_subcommand)]
    ExtensionAlias(Vec<String>),
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
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long, default_value_t = 16384)]
        context_limit: usize,
        #[arg(long, default_value_t = 1024)]
        output_limit: usize,
        #[arg(long, default_value = "{}")]
        parameters: String,
        #[arg(long)]
        routing: Option<String>,
        #[arg(long)]
        merge_system_messages: bool,
    },
}
#[derive(Subcommand)]
enum ExtensionCommand {
    Install {
        #[arg(conflicts_with = "builtin")]
        path: Option<PathBuf>,
        #[arg(long)]
        builtin: Option<String>,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Inspect {
        extension: String,
        #[arg(long)]
        json: bool,
    },
    Enable {
        #[arg(required=true,num_args=1..)]
        extensions: Vec<String>,
        #[arg(long)]
        grant: Vec<String>,
        #[arg(long)]
        trust_processes: bool,
    },
    Disable {
        extension: String,
    },
    Uninstall {
        extension: String,
    },
    Asset {
        extension: String,
        path: String,
        #[arg(long)]
        output: PathBuf,
    },
    Run {
        command: String,
        #[command(flatten)]
        input: CommandInput,
    },
}
#[derive(Subcommand)]
enum PluginCommand {
    Inspect {
        plugin: Option<String>,
        #[arg(long)]
        json: bool,
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
        terminal::run(cli.data_dir).await
    };
    if let Err(error) = result {
        eprintln!(
            "{}",
            serde_json::to_string(&error).unwrap_or_else(|_| "The operation failed.".into())
        );
        std::process::exit(error.exit_code());
    }
}
