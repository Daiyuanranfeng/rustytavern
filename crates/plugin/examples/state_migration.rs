//! This example upgrades persistent plugin state through host commands.
use async_trait::async_trait;
use clap::Parser;
use rustytavern_card::Card;
use rustytavern_conversation::Session;
use rustytavern_conversation::prompt::PromptOptions;
use rustytavern_core::{Error, Persona, Result};
use rustytavern_plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin};
use rustytavern_store::SqliteStore;
use rustytavern_store::StoreBackend;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

const ID: &str = "example.state-counter";
#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(value_parser=["seed","upgrade","fail","inspect","old-write"])]
    operation: String,
}
struct Counter {
    manifest: PluginManifest,
    store: Arc<dyn StoreBackend>,
    operation: String,
}
#[async_trait]
impl RuntimePlugin for Counter {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        match self.operation.as_str() {
            "seed" => {
                if !self
                    .store
                    .plugin_state_snapshot(ID)?
                    .entries
                    .as_object()
                    .unwrap()
                    .is_empty()
                {
                    return Err(Error::new(
                        "configuration",
                        "The example namespace already contains state.",
                    ));
                }
                context.state_write(self.store.as_ref(), ID, "counter", &json!(7))?;
                context.state_write(
                    self.store.as_ref(),
                    ID,
                    "opaque",
                    &json!({"future":["保留",false,null]}),
                )?;
                let card = Card::import(include_bytes!("lumen.json"))?;
                let persona = Persona::new("Alex".into(), "Alex charts the coast.".into())?;
                self.store.save_card(&card)?;
                self.store.save_persona(&persona)?;
                self.store.create_session(
                    &Session {
                        id: String::new(),
                        character_id: card.id,
                        persona_id: persona.id,
                        worldbook_ids: Vec::new(),
                        provider: "unused-offline".into(),
                        model: "fixture".into(),
                        head: None,
                        version: 0,
                        accept_reduced: false,
                        options: PromptOptions::default(),
                        composition: json!({"plugins":[{"manifest":self.manifest}]}),
                    },
                    None,
                )?;
            }
            "upgrade" | "fail" => {
                context.state_migrate(self.store.as_ref(), |before| {
                    if self.operation == "fail" {
                        return Err(Error::new(
                            "migration_fixture",
                            "The example migration stops before commit.",
                        ));
                    }
                    if before.schema != 1 {
                        return Err(Error::new(
                            "unsupported_plugin_state_schema",
                            "The example requires state schema 1.",
                        ));
                    }
                    let count = before.entries["counter"]["value"].as_i64().ok_or_else(|| {
                        Error::new(
                            "invalid_plugin_state",
                            "The example counter must be an integer.",
                        )
                    })?;
                    Ok(BTreeMap::from([("counter".into(), json!({"count":count}))]))
                })?;
            }
            "old-write" => {
                context.state_write(self.store.as_ref(), ID, "counter", &json!(99))?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}
async fn run(arguments: Arguments) -> Result<()> {
    std::fs::create_dir_all(&arguments.data_dir)?;
    let store = Arc::new(SqliteStore::open(
        arguments.data_dir.join("sessions.sqlite3"),
    )?);
    if arguments.operation != "inspect" {
        let capabilities =
            BTreeSet::from([format!("state.read:{ID}"), format!("state.write:{ID}")]);
        let schema = if matches!(arguments.operation.as_str(), "seed" | "old-write") {
            1
        } else {
            2
        };
        let plugin = Counter {
            manifest: PluginManifest {
                id: ID.into(),
                version: format!("{schema}.0.0"),
                state_schema: schema,
                requested_capabilities: capabilities.clone(),
                ..Default::default()
            },
            store: store.clone(),
            operation: arguments.operation,
        };
        let mut registry = PluginRegistry::default();
        registry
            .inject(
                vec![Arc::new(plugin)],
                "example",
                &BTreeMap::from([(ID.into(), capabilities)]),
            )
            .await?;
        registry.eject_owner("example").await?;
    }
    let session_ids: Vec<_> = store
        .list("session")?
        .into_iter()
        .map(|session| session["id"].clone())
        .collect();
    println!(
        "{}",
        json!({"state":store.plugin_state_snapshot(ID)?,"session_ids":session_ids,"model_requests":0})
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(Arguments::parse()).await {
        eprintln!("{}", serde_json::to_string(&error).unwrap());
        std::process::exit(error.exit_code());
    }
}
