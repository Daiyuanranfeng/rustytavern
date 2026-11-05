//! The source plugins register card sources for public character websites.

use std::sync::Arc;

use crate::{
    Result,
    plugin::PluginContext,
    source::{ArtifactFetcher, CardSource, ChubSource, PublicFetcher, RisuRealmSource},
};

/// The `source.chub` and `source.risurealm` plugins fetch through their granted host.
pub(crate) fn activate(context: &mut PluginContext, chub: bool) -> Result<()> {
    let (host, service) = if chub {
        ("api.chub.ai", "card.source.chub/1")
    } else {
        ("realm.risuai.net", "card.source.risurealm/1")
    };
    context.authorize(&format!("network:https:{host}"))?;
    let hosts = context
        .grants()
        .iter()
        .filter_map(|g| g.strip_prefix("network:https:").map(str::to_owned));
    let fetcher: Arc<dyn ArtifactFetcher> = Arc::new(PublicFetcher::new(hosts));
    let source: Arc<dyn CardSource> = if chub {
        Arc::new(ChubSource::new(fetcher))
    } else {
        Arc::new(RisuRealmSource::new(fetcher))
    };
    context.register(service, Arc::new(source))
}
