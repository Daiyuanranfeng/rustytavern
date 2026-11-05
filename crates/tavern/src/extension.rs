//! An extension changes how an application looks, for example the message bubbles.
//! An extension cannot change agent behavior or agent context. A plugin does that.
//! Each extension is one `extension.json` file in `DATA_DIR/extensions/ID/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};
use rustytavern_core::{bounded_read, valid_id};

const MANIFEST: &str = "extension.json";
const ENABLED: &str = "enabled";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub theme: Theme,
}

/// Each color is a name, such as `magenta`, or a hex value, such as `#5f87ff`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub user_bubble: Option<String>,
    pub assistant_bubble: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub enabled: bool,
}

pub struct Extensions {
    root: PathBuf,
}
impl Extensions {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("extensions");
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    /// The method copies `extension.json` from a directory or a file path.
    pub fn install(&self, source: &Path) -> Result<ExtensionManifest> {
        let file = if source.is_dir() {
            source.join(MANIFEST)
        } else {
            source.to_owned()
        };
        let manifest = read(&file)?;
        let directory = self.root.join(&manifest.id);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(manifest)
    }
    pub fn list(&self) -> Result<Vec<InstalledExtension>> {
        let mut extensions = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let directory = entry?.path();
            if directory.join(MANIFEST).is_file() {
                extensions.push(InstalledExtension {
                    manifest: read(&directory.join(MANIFEST))?,
                    enabled: directory.join(ENABLED).exists(),
                });
            }
        }
        extensions.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(extensions)
    }
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let marker = self.directory(id)?.join(ENABLED);
        if enabled {
            std::fs::write(marker, b"")?;
        } else if marker.exists() {
            std::fs::remove_file(marker)?;
        }
        Ok(())
    }
    pub fn uninstall(&self, id: &str) -> Result<()> {
        Ok(std::fs::remove_dir_all(self.directory(id)?)?)
    }
    /// The method merges the enabled themes. A later ID overrides an earlier ID.
    pub fn theme(&self) -> Result<Theme> {
        let mut theme = Theme::default();
        for extension in self.list()?.into_iter().filter(|e| e.enabled) {
            let next = extension.manifest.theme;
            theme.user_bubble = next.user_bubble.or(theme.user_bubble);
            theme.assistant_bubble = next.assistant_bubble.or(theme.assistant_bubble);
        }
        Ok(theme)
    }
    fn directory(&self, id: &str) -> Result<PathBuf> {
        let directory = self.root.join(id);
        if !valid_id(id) || !directory.join(MANIFEST).is_file() {
            return Err(Error::new("not_found", "The extension is not installed."));
        }
        Ok(directory)
    }
}

fn read(file: &Path) -> Result<ExtensionManifest> {
    let manifest: ExtensionManifest = serde_json::from_slice(&bounded_read(file, 65_536)?)?;
    if !valid_id(&manifest.id) || semver::Version::parse(&manifest.version).is_err() {
        return Err(Error::new(
            "invalid_extension_manifest",
            "The extension requires a valid ID and a semantic version.",
        ));
    }
    Ok(manifest)
}
