use super::types::RuntimeFileConfig;

/// Read Grok CLI config from `~/.grok/config.toml`.
pub(super) fn read_config_file() -> Option<RuntimeFileConfig> {
    let path = grok_config_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    parse_grok_config(&raw)
}

fn grok_config_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join(".grok").join("config.toml"))
}

fn parse_grok_config(toml_str: &str) -> Option<RuntimeFileConfig> {
    let table: toml::Table = toml_str.parse().ok()?;

    // Prefer [models].default (current CLI layout). Fall back to top-level model.
    let model = table
        .get("models")
        .and_then(|value| value.as_table())
        .and_then(|models| toml_string(models, "default"))
        .or_else(|| toml_string(&table, "model"));

    let thinking_effort = table
        .get("models")
        .and_then(|value| value.as_table())
        .and_then(|models| toml_string(models, "default_reasoning_effort"));

    Some(RuntimeFileConfig {
        model,
        // Grok is a locked provider/runtime — no separate provider axis.
        provider: Some("grok".to_string()),
        thinking_effort,
        ..RuntimeFileConfig::default()
    })
}

fn toml_string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_default() {
        let cfg = parse_grok_config(
            r#"
[models]
default = "grok-4.5"
default_reasoning_effort = "medium"
"#,
        )
        .expect("valid grok toml");
        assert_eq!(cfg.model.as_deref(), Some("grok-4.5"));
        assert_eq!(cfg.provider.as_deref(), Some("grok"));
        assert_eq!(cfg.thinking_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn falls_back_to_top_level_model() {
        let cfg = parse_grok_config(r#"model = "composer-2.5""#).expect("valid");
        assert_eq!(cfg.model.as_deref(), Some("composer-2.5"));
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(parse_grok_config("{{{{not toml").is_none());
    }
}
