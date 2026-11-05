use rustytavern::config::{Application, ConfigCredentials, Configuration};
use rustytavern::extension;
use rustytavern::provider::{CredentialResolver, ProviderConfig};

const TEXT: &str = r##"# The user keeps this comment.
default_provider = "openrouter"

[providers.openrouter]
kind = "openrouter"
model = "openai/gpt-4.1-mini"
api_key = "sk-or-test"

[plugins]
enabled = ["agent", "skills"]

[plugins.agent]
max_steps = 4 # four steps

[plugins.mcp.servers.docs]
url = "https://example.com/mcp"
env = { TOKEN = "secret" }

[extensions.rose]
user_bubble = "#ff5f87"

[extensions.sky]
assistant_bubble = "cyan"
"##;

#[test]
fn one_toml_file_holds_every_setting() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), TEXT).unwrap();
    let mut app = Application::open(home.path()).unwrap();
    assert!(home.path().join("rustytavern.db").exists());
    let config = &app.config;
    assert_eq!(config.default_provider.as_deref(), Some("openrouter"));
    assert_eq!(
        config.plugins.enabled(),
        ["agent".to_string(), "skills".into()].into()
    );
    let essential = config.plugins.essential_configuration(home.path());
    assert_eq!(essential["agent"]["max_steps"], 4);
    assert_eq!(
        essential["skills"]["paths"][0],
        home.path().join("skills").to_str().unwrap()
    );
    let theme = extension::theme(&config.extensions);
    assert_eq!(theme.user_bubble.as_deref(), Some("#ff5f87"));
    assert_eq!(theme.assistant_bubble.as_deref(), Some("cyan"));

    let provider = config.provider("openrouter").unwrap();
    assert_eq!(provider.api_key_env.as_deref(), Some("config:openrouter"));
    assert_eq!(
        ConfigCredentials::new(config)
            .resolve("config:openrouter")
            .unwrap(),
        "sk-or-test"
    );
    let redacted = config.redacted().unwrap().to_string();
    assert!(!redacted.contains("sk-or-test") && !redacted.contains("secret"));

    app.config.set_provider(
        "openrouter",
        ProviderConfig {
            model: "openai/gpt-5".into(),
            ..provider
        },
        None,
    );
    app.save_config().unwrap();
    let saved = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(saved.starts_with("# The user keeps this comment."));
    assert!(saved.contains("max_steps = 4 # four steps"));
    assert!(saved.contains("model = \"openai/gpt-5\""));
    assert!(saved.contains("api_key = \"sk-or-test\""));
    assert!(!saved.contains("api_key_env"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join("config.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn an_unknown_setting_or_extension_field_fails() {
    for text in [
        "[chat]\nprovider = \"x\"\n",
        "[extensions.rose]\nplugins = []\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        assert_eq!(
            Configuration::load(&home.path().join("config.toml"))
                .unwrap_err()
                .code,
            "configuration"
        );
    }
}
