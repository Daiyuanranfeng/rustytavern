//! The `tools.web` plugin registers `web_fetch` for its granted HTTPS hosts.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::definition;
use crate::{
    Error, Result,
    agent::{AgentTool, ToolSet},
    plugin::PluginContext,
    prompt::{Contribution, Retention},
    provider::ToolDefinition,
    source::PublicFetcher,
};
use rustytavern_core::Role;

/// The provider runs the web search. This guide tells the model when to use the web tools.
const GUIDE: &str = "Tool use. You can use tools before you reply. Search the web when the user asks you to search, look up, or check something, or when the reply needs current facts. Run a separate search for each language that the user names, and write each query in that language. Call web_fetch to read a full page. Base the reply on the results and name the sources. Keep the character's voice when you report the results.";

/// The `tools.web` plugin registers `web_fetch` for its granted HTTPS hosts.
/// The plugin also adds the tool-use guide to the prompt.
pub(crate) fn activate(context: &mut PluginContext) -> Result<()> {
    let hosts = context
        .grants()
        .iter()
        .filter_map(|g| g.strip_prefix("network:https:").map(str::to_owned))
        .collect::<Vec<_>>();
    if hosts.is_empty() {
        return Err(Error::new(
            "capability_denied",
            "The web tools require a network:https grant.",
        ));
    }
    let mut fetcher = PublicFetcher::new(hosts);
    fetcher.max_bytes = 2 * 1024 * 1024;
    let tools: ToolSet = vec![Arc::new(WebFetch(fetcher))];
    context.contribute(Contribution {
        plugin_id: String::new(),
        section_id: "tool_use".into(),
        slot: "before_history".into(),
        priority: 1,
        role: Role::System,
        text: GUIDE.into(),
        provenance: Value::Null,
        retention: Retention::Optional,
    })?;
    context.register("tools.web/1", Arc::new(tools))
}

struct WebFetch(PublicFetcher);
#[async_trait]
impl AgentTool for WebFetch {
    fn definition(&self) -> ToolDefinition {
        definition(
            "web_fetch",
            "Fetch one public HTTPS page. The tool returns the page as plain text.",
            json!({"type":"object","properties":{"url":{"type":"string","description":"The HTTPS URL."}},"required":["url"],"additionalProperties":false}),
        )
    }
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String> {
        let url = arguments["url"].as_str().unwrap_or_default();
        let artifact = self.0.fetch(url, cancellation).await?;
        let media = artifact.provenance.media_type.to_ascii_lowercase();
        let body = String::from_utf8_lossy(&artifact.bytes);
        let text = if media.starts_with("text/html") {
            html_text(&body)
        } else if media.starts_with("text/") || media.contains("json") || media.contains("xml") {
            body.into_owned()
        } else {
            format!("The tool does not read the media type {media}.")
        };
        Ok(format!("URL: {}\n\n{text}", artifact.provenance.final_url))
    }
}

/// The function removes scripts, styles, and tags from HTML.
/// ASCII lowercase keeps byte offsets, so one index serves both strings.
fn html_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut text = String::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find('<').map(|i| at + i) {
        text.push_str(&html[at..start]);
        let close = if lower[start..].starts_with("<script") {
            "</script>"
        } else if lower[start..].starts_with("<style") {
            "</style>"
        } else {
            ">"
        };
        at = lower[start..]
            .find(close)
            .map_or(html.len(), |i| start + i + close.len());
        text.push(' ');
    }
    text.push_str(&html[at..]);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn html_text_removes_scripts_styles_and_tags() {
        let html = "<p>A<SCRIPT>x<y</script> <style>b{}</style>B&amp;</p>";
        assert_eq!(super::html_text(html), "A B&amp;");
    }
}
