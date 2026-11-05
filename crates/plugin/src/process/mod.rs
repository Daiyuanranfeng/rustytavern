use rustytavern_card as card;
use rustytavern_core::{Error, Result, digest, new_id, now_ms};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;

use super::{PluginContext, PluginManifest, RuntimePlugin};
use rustytavern_adapters::source::{
    Artifact, ArtifactFetcher, CardSource, Provenance, PublicFetcher,
};

const MAX_FRAME: usize = 1024 * 1024;
const MAX_CALL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessDefinition {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub accepted_hosts: BTreeSet<String>,
    pub timeout_ms: u64,
}
impl Default for ProcessDefinition {
    fn default() -> Self {
        Self {
            executable: PathBuf::new(),
            args: Vec::new(),
            accepted_hosts: BTreeSet::new(),
            timeout_ms: 30_000,
        }
    }
}

struct RpcProcess {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}
impl RpcProcess {
    async fn stop(mut self, pending: Option<(u64, &str)>) {
        let graceful = async {
            if let Some((id, reason)) = pending {
                write_frame(
                    &mut self.input,
                    &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id,"reason":reason}}),
                    MAX_CALL_BYTES,
                )
                .await?;
            }
            write_frame(
                &mut self.input,
                &json!({"jsonrpc":"2.0","id":self.next_id,"method":"shutdown","params":{}}),
                MAX_CALL_BYTES,
            )
            .await?;
            drop(self.input);
            self.child.wait().await?;
            Ok::<(), Error>(())
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), graceful).await,
            Ok(Ok(()))
        ) {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        }
    }
}
struct RpcClient {
    process: Mutex<Option<RpcProcess>>,
    definition: ProcessDefinition,
    plugin_id: String,
    grants: BTreeSet<String>,
    fetcher: Arc<dyn ArtifactFetcher>,
    slots: Semaphore,
    queued_bytes: Semaphore,
}

impl RpcClient {
    async fn start(
        definition: ProcessDefinition,
        plugin_id: String,
        grants: BTreeSet<String>,
        fetcher: Arc<dyn ArtifactFetcher>,
    ) -> Result<Arc<Self>> {
        if definition.executable.as_os_str().is_empty()
            || definition.timeout_ms == 0
            || definition.timeout_ms > 120_000
        {
            return Err(Error::new(
                "invalid_plugin_process",
                "The executable or deadline is invalid.",
            ));
        }
        let mut command = Command::new(&definition.executable);
        command
            .args(&definition.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command
            .spawn()
            .map_err(Error::context("plugin_start", "Plugin did not start."))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::new("plugin_start", "The plugin input pipe is unavailable."))?;
        let output =
            BufReader::new(child.stdout.take().ok_or_else(|| {
                Error::new("plugin_start", "The plugin output pipe is unavailable.")
            })?);
        let client = Arc::new(Self {
            process: Mutex::new(Some(RpcProcess {
                child,
                input,
                output,
                next_id: 1,
            })),
            definition,
            plugin_id,
            grants,
            fetcher,
            slots: Semaphore::new(64),
            queued_bytes: Semaphore::new(MAX_CALL_BYTES),
        });
        let reply=client.invoke("initialize",json!({"host_protocol":"1","plugin_id":client.plugin_id,"service_families":["card.source/1"]}),CancellationToken::new()).await?;
        if reply.result["host_protocol"] != "1"
            || reply.result["plugin_id"] != client.plugin_id
            || !reply.result["service_families"]
                .as_array()
                .is_some_and(|families| families.iter().any(|v| v == "card.source/1"))
        {
            client.shutdown().await;
            return Err(Error::new(
                "incompatible_plugin_protocol",
                "The external plugin handshake does not match the selected host and family.",
            ));
        }
        Ok(client)
    }
    async fn invoke(
        &self,
        method: &str,
        mut params: Value,
        cancellation: CancellationToken,
    ) -> Result<RpcReply> {
        params["deadline_ms"] = json!(now_ms().saturating_add(self.definition.timeout_ms as i64));
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.definition.timeout_ms);
        let size = serde_json::to_vec(&params)?.len();
        if size > MAX_FRAME {
            return Err(Error::new(
                "invalid_plugin_frame",
                "The invocation parameters exceed the frame byte limit.",
            ));
        }
        let _bytes = self
            .queued_bytes
            .try_acquire_many(size as u32)
            .map_err(Error::context(
                "plugin_queue_limit",
                "The plugin invocation queue exceeds its byte budget.",
            ))?;
        let _slot = self.slots.try_acquire().map_err(Error::context(
            "plugin_queue_limit",
            "The plugin invocation queue is full.",
        ))?;
        let mut guard = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the plugin invocation.")),
            _ = tokio::time::sleep_until(deadline) => return Err(Error::new("plugin_timeout", "The plugin invocation exceeded its deadline.")),
            guard = self.process.lock() => guard,
        };
        let process = guard.as_mut().ok_or_else(|| {
            Error::new(
                "plugin_not_active",
                "The external plugin process is unavailable.",
            )
        })?;
        let id = process.next_id;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Error::new("cancelled", "The caller cancelled the plugin invocation.")),
            result = tokio::time::timeout_at(deadline, self.exchange(process, method, params, &cancellation)) => result.unwrap_or_else(|_| Err(Error::new("plugin_timeout", "The plugin invocation exceeded its deadline."))),
        };
        if let Err(error) = &result {
            if let Some(process) = guard.take() {
                drop(guard);
                process.stop(Some((id, &error.code))).await;
            }
        }
        result
    }
    async fn exchange(
        &self,
        process: &mut RpcProcess,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<RpcReply> {
        let id = process.next_id;
        process.next_id = process.next_id.checked_add(1).ok_or_else(|| {
            Error::new(
                "plugin_protocol",
                "The plugin request ID range is exhausted.",
            )
        })?;
        let mut remaining = MAX_CALL_BYTES;
        remaining -= write_frame(
            &mut process.input,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            remaining,
        )
        .await?;
        let mut artifacts = BTreeMap::new();
        let mut total = 0usize;
        let mut callbacks = 0usize;
        loop {
            let (frame, size) = read_frame(&mut process.output).await?;
            total = total.saturating_add(size);
            if total > MAX_CALL_BYTES {
                return Err(Error::new(
                    "plugin_queue_limit",
                    "The plugin invocation exceeds its message budget.",
                ));
            }
            if frame["jsonrpc"] != "2.0" {
                return Err(Error::new(
                    "invalid_plugin_frame",
                    "The plugin message has no supported JSON-RPC version.",
                ));
            }
            if frame.get("method").is_some() {
                if method == "initialize" {
                    return Err(Error::new(
                        "invalid_plugin_frame",
                        "The plugin cannot call host services before initialization.",
                    ));
                }
                callbacks += 1;
                if callbacks > 64 {
                    return Err(Error::new(
                        "plugin_queue_limit",
                        "The plugin invocation exceeds its callback limit.",
                    ));
                }
                let callback_id = frame
                    .get("id")
                    .filter(|v| v.is_string() || v.is_u64())
                    .cloned()
                    .ok_or_else(|| {
                        Error::new("invalid_plugin_frame", "The host callback requires an ID.")
                    })?;
                let result = self.callback(&frame, &mut artifacts, cancellation).await;
                let reply = match result {
                    Ok(result) => json!({"jsonrpc":"2.0","id":callback_id,"result":result}),
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":callback_id,"error":{"code":-32000,"message":error.message,"data":{"code":error.code}}})
                    }
                };
                remaining -= write_frame(&mut process.input, &reply, remaining).await?;
                continue;
            }
            if frame["id"] != json!(id) {
                return Err(Error::new(
                    "invalid_plugin_frame",
                    "The plugin response has another request ID.",
                ));
            }
            if frame.get("error").is_some() {
                return Err(Error::new(
                    "plugin_error",
                    "The external plugin reported an invocation error.",
                ));
            }
            let result = frame.get("result").cloned().ok_or_else(|| {
                Error::new("invalid_plugin_frame", "The plugin response has no result.")
            })?;
            return Ok(RpcReply { result, artifacts });
        }
    }
    async fn callback(
        &self,
        frame: &Value,
        artifacts: &mut BTreeMap<String, Artifact>,
        cancellation: &CancellationToken,
    ) -> Result<Value> {
        if frame["method"] != "host.fetch" {
            return Err(Error::new(
                "unsupported_plugin_service",
                "The plugin called an unsupported host service.",
            ));
        }
        let url = frame["params"]["url"].as_str().ok_or_else(|| {
            Error::new("invalid_plugin_frame", "The fetch callback requires a URL.")
        })?;
        let hosts = self
            .grants
            .iter()
            .filter_map(|g| g.strip_prefix("network:https:").map(str::to_owned));
        PublicFetcher::new(hosts).check_url(url)?;
        if artifacts.len() >= 8 {
            return Err(Error::new(
                "plugin_queue_limit",
                "The invocation holds too many artifacts.",
            ));
        }
        let artifact = self.fetcher.fetch(url, cancellation.clone()).await?;
        let held = artifacts.values().map(|a| a.bytes.len()).sum::<usize>();
        if held.saturating_add(artifact.bytes.len()) > card::MAX_INPUT_BYTES {
            return Err(Error::new(
                "source_size_limit",
                "The invocation exceeds its artifact byte budget.",
            ));
        }
        let id = new_id("artifact");
        let reply = json!({"artifact_id":id,"sha256":artifact.provenance.content_hash,"bytes":artifact.bytes.len(),"media_type":artifact.provenance.media_type});
        artifacts.insert(id, artifact);
        Ok(reply)
    }
    async fn shutdown(&self) {
        let process = self.process.lock().await.take();
        if let Some(process) = process {
            process.stop(None).await;
        }
    }
}
struct RpcReply {
    result: Value,
    artifacts: BTreeMap<String, Artifact>,
}

async fn write_frame(input: &mut ChildStdin, value: &Value, remaining: usize) -> Result<usize> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME {
        return Err(Error::new(
            "invalid_plugin_frame",
            "The host message exceeds the frame byte limit.",
        ));
    }
    if bytes.len() > remaining {
        return Err(Error::new(
            "plugin_queue_limit",
            "The invocation exceeds its host message byte budget.",
        ));
    }
    input
        .write_all(&bytes)
        .await
        .map_err(Error::context("plugin_process", "Plugin pipe failed."))?;
    input
        .flush()
        .await
        .map(|_| bytes.len())
        .map_err(Error::context("plugin_process", "Plugin pipe failed."))
}
async fn read_frame(output: &mut BufReader<ChildStdout>) -> Result<(Value, usize)> {
    let mut bytes = Vec::new();
    loop {
        let buffer = output
            .fill_buf()
            .await
            .map_err(Error::context("plugin_process", "Plugin pipe failed."))?;
        if buffer.is_empty() {
            return Err(Error::new(
                "plugin_process",
                "The plugin closed its output before a response.",
            ));
        }
        let length = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(buffer.len());
        if bytes.len().saturating_add(length) > MAX_FRAME {
            return Err(Error::new(
                "invalid_plugin_frame",
                "The plugin message exceeds the frame byte limit.",
            ));
        }
        bytes.extend_from_slice(&buffer[..length]);
        output.consume(length);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    let frame = serde_json::from_slice(&bytes)
        .map_err(Error::context("invalid_plugin_frame", "RPC is invalid."))?;
    Ok((frame, bytes.len()))
}

struct ExternalSource {
    client: Arc<RpcClient>,
    source_id: String,
}
#[async_trait]
impl CardSource for ExternalSource {
    fn id(&self) -> &str {
        &self.source_id
    }
    fn recognize(&self, url: &str) -> bool {
        url::Url::parse(url).ok().is_some_and(|u| {
            u.scheme() == "https"
                && u.username().is_empty()
                && u.password().is_none()
                && u.host_str().is_some_and(|host| {
                    self.client.definition.accepted_hosts.is_empty()
                        || self.client.definition.accepted_hosts.contains(host)
                })
        })
    }
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact> {
        if !self.recognize(url) {
            return Err(Error::new(
                "invalid_source_url",
                "The external source does not accept this URL.",
            ));
        }
        let mut reply = self
            .client
            .invoke(
                "card.source.fetch",
                json!({"url":url,"scope":new_id("invocation")}),
                cancellation,
            )
            .await?;
        let mut artifact = if let Some(id) = reply.result["artifact_id"].as_str() {
            reply.artifacts.remove(id).ok_or_else(|| {
                Error::new(
                    "invalid_plugin_artifact",
                    "The plugin returned an artifact outside this invocation.",
                )
            })?
        } else if let Some(card) = reply.result.get("card") {
            let bytes = serde_json::to_vec(card)?;
            Artifact {
                provenance: Provenance {
                    source: self.source_id.clone(),
                    requested_url: url.into(),
                    final_url: url.into(),
                    fetched_at_ms: now_ms(),
                    media_type: "application/json".into(),
                    content_hash: digest(&bytes),
                    creator: card["data"]["creator"].as_str().map(str::to_owned),
                    license: None,
                },
                bytes,
            }
        } else {
            return Err(Error::new(
                "invalid_plugin_artifact",
                "The source result requires a scoped artifact handle or inline card.",
            ));
        };
        artifact.provenance.source = self.source_id.clone();
        artifact.provenance.requested_url = url.into();
        Ok(artifact)
    }
}

pub struct ExternalSourcePlugin {
    manifest: PluginManifest,
    definition: ProcessDefinition,
    client: Mutex<Option<Arc<RpcClient>>>,
}
impl ExternalSourcePlugin {
    pub fn new(manifest: PluginManifest, definition: ProcessDefinition) -> Self {
        Self {
            manifest,
            definition,
            client: Mutex::new(None),
        }
    }
}
#[async_trait]
impl RuntimePlugin for ExternalSourcePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        if self
            .manifest
            .provides
            .iter()
            .any(|s| s != "card.source/1" && !(s.starts_with("card.source.") && s.ends_with("/1")))
        {
            return Err(Error::new(
                "unsupported_plugin_service",
                "The external host supports card.source/1 only.",
            ));
        }
        let grants = context.grants().clone();
        let fetcher: Arc<dyn ArtifactFetcher> =
            if self.manifest.requires.iter().any(|r| r == "host.fetch/1") {
                context
                    .dependency::<Arc<dyn ArtifactFetcher>>("host.fetch/1")?
                    .as_ref()
                    .clone()
            } else {
                Arc::new(PublicFetcher::new(grants.iter().filter_map(|g| {
                    g.strip_prefix("network:https:").map(str::to_owned)
                })))
            };
        let client = RpcClient::start(
            self.definition.clone(),
            self.manifest.id.clone(),
            grants,
            fetcher,
        )
        .await?;
        let source: Arc<dyn CardSource> = Arc::new(ExternalSource {
            client: client.clone(),
            source_id: self.manifest.id.clone(),
        });
        for service in &self.manifest.provides {
            context.register(service, Arc::new(source.clone()))?;
        }
        *self.client.lock().await = Some(client);
        Ok(())
    }
    async fn deactivate(&self) -> Result<()> {
        if let Some(client) = self.client.lock().await.take() {
            client.shutdown().await;
        }
        Ok(())
    }
}
