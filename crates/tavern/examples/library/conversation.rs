//! This example sends a caller-selected message through the public library.
use clap::Parser;
use rustytavern::{
    Error, Result,
    app::{ExtensionManager, default_registry, runtime},
    config::Application,
    session::GenerationStatus,
};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long)]
    session: String,
    #[arg(long)]
    message: String,
    #[arg(long)]
    idempotency_key: Option<String>,
    #[arg(long)]
    adopt: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = converse(Arguments::parse()).await {
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}

async fn converse(arguments: Arguments) -> Result<()> {
    let app = Application::open(&arguments.data_dir)?;
    let registry = default_registry(app.store.clone()).await?;
    let mut extensions = ExtensionManager::open(&app.data_dir, app.store, registry)?;
    extensions.restore().await?;
    let runtime = runtime(&extensions).await?;
    let session = runtime.store.session(&arguments.session)?;
    let provider = session.configuration()?.provider_config;
    if arguments.adopt {
        runtime
            .configure_session(&session, provider.clone())
            .await?;
    }
    let mut handle = runtime
        .start(
            &arguments.session,
            &arguments.message,
            arguments.idempotency_key.as_deref(),
            provider,
        )
        .await?;
    let result = loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => { signal?; handle.cancel(); break handle.wait().await?; }
            event = handle.events.recv() => {
                if event.is_none() { break handle.wait().await?; }
            }
        }
    };
    if result.status != GenerationStatus::Completed {
        return Err(result.error.unwrap_or_else(|| {
            Error::new("incomplete_stream", "The generation does not complete.")
        }));
    }
    println!(
        "{}",
        json!({"generation_id":result.id,"reply":result.draft,"usage":result.usage})
    );
    Ok(())
}
