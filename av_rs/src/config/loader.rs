use figment::{
    providers::{Env, Format, Json, Serialized, Toml, Yaml},
    Error as FigmentError, Figment,
};
use crate::config::AvConfig; // This assumes AvConfig is in src/config/mod.rs
use std::path::Path;
use xdg::BaseDirectories;

pub fn load_config(repo_config_path: Option<&Path>) -> Result<AvConfig, FigmentError> {
    let xdg_dirs = BaseDirectories::new().expect("Failed to initialize XDG BaseDirectories");

    let mut figment = Figment::new().join(Serialized::defaults(AvConfig::default()));

    // Helper closure to merge a config file if it exists
    let mut merge_file = |path: std::path::PathBuf, format: &str| {
        if path.exists() {
            match format {
                "toml" => figment = figment.join(Toml::file(path)),
                "yaml" => figment = figment.join(Yaml::file(path)),
                "json" => figment = figment.join(Json::file(path)),
                _ => {} // Do nothing for unsupported formats
            }
        }
    };

    // User-level global config files (e.g., ~/.config/av/config.toml)
    let config_home_av = xdg_dirs.get_config_home().join("av");
    merge_file(config_home_av.join("config.toml"), "toml");
    merge_file(config_home_av.join("config.yaml"), "yaml");
    merge_file(config_home_av.join("config.json"), "json");

    // Other XDG config directories (e.g., /etc/xdg/av/config.toml)
    for config_dir in xdg_dirs.get_config_dirs() {
        let av_dir = config_dir.join("av");
        merge_file(av_dir.join("config.toml"), "toml");
        merge_file(av_dir.join("config.yaml"), "yaml");
        merge_file(av_dir.join("config.json"), "json");
    }

    // Repository-specific config (e.g., .git/av/config.toml or repo_root/.av/config.toml)
    // The plan specified repo_config_dir, which could be .git/av or .av in repo root.
    if let Some(dir) = repo_config_path {
        merge_file(dir.join("config.toml"), "toml");
        merge_file(dir.join("config.yaml"), "yaml");
        merge_file(dir.join("config.json"), "json");
    }

    // Environment variables
    // Specific mappings take precedence.
    // GITHUB_TOKEN is often a general token, AV_GITHUB_TOKEN is specific to av.
    // Let AV_GITHUB_TOKEN override GITHUB_TOKEN if both are set.
    let mut env_figment = Figment::new();
    if std::env::var("AV_GITHUB_TOKEN").is_ok() {
        env_figment = env_figment.join(Env::raw().map(|key| if key == "AV_GITHUB_TOKEN" { Some("github.token".into()) } else { None }));
    } else {
        env_figment = env_figment.join(Env::raw().map(|key| if key == "GITHUB_TOKEN" { Some("github.token".into()) } else { None }));
    }
     env_figment = env_figment.join(Env::raw().map(|key| {
        match key.as_str() {
            "AV_API_TOKEN" => Some("aviator.api_token".into()),
            "AV_API_HOST" => Some("aviator.api_host".into()),
            // Add other specific one-to-one mappings here if necessary
            _ => None,
        }
    }));

    // General prefixed environment variables (e.g., AV_PULL_REQUEST__DRAFT=true becomes pull_request.draft=true)
    // The double underscore `__` is a common convention for nesting.
    env_figment = env_figment.join(Env::prefixed("AV_").map(|key| key.as_str().replace("__", ".").into()).ignore_empty(true));

    figment = figment.join(env_figment);

    figment.extract()
}
