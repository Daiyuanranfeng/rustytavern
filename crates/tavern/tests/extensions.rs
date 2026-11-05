use rustytavern::extension::{Extensions, Theme};
use serde_json::json;

#[test]
fn an_extension_changes_only_the_theme() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let extensions = Extensions::open(data.path()).unwrap();
    let write = |value: serde_json::Value| {
        std::fs::write(source.path().join("extension.json"), value.to_string()).unwrap()
    };
    write(json!({"id":"rose","version":"1.0.0","theme":{"user_bubble":"#ff5f87"}}));
    extensions.install(source.path()).unwrap();
    assert_eq!(extensions.theme().unwrap(), Theme::default());
    extensions.set_enabled("rose", true).unwrap();
    assert_eq!(
        extensions.theme().unwrap().user_bubble.as_deref(),
        Some("#ff5f87")
    );
    write(json!({"id":"agent","version":"1.0.0","plugins":[]}));
    assert!(extensions.install(source.path()).is_err());
    extensions.uninstall("rose").unwrap();
    assert!(extensions.list().unwrap().is_empty());
}
