use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
pub struct GitHubConfig {
    pub token: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct PullRequestConfig {
    pub draft: bool,
    pub open_browser: bool,
    pub rebase_with_draft: Option<bool>,
    pub no_wip_detection: bool,
    pub branch_name_prefix: Option<String>,
    pub write_stack: bool,
}

impl Default for PullRequestConfig {
    fn default() -> Self {
        Self {
            draft: false,
            open_browser: true, // Default to true
            rebase_with_draft: None,
            no_wip_detection: false,
            branch_name_prefix: None,
            write_stack: false,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct AviatorConfig {
    pub api_host: Option<String>,
    pub api_token: Option<String>,
}

impl Default for AviatorConfig {
    fn default() -> Self {
        Self {
            api_host: Some("https://api.aviator.co".to_string()),
            api_token: None,
        }
    }
}

pub mod loader;
pub use loader::load_config;

#[derive(Deserialize, Debug, Default)]
pub struct AvConfig {
    pub pull_request: PullRequestConfig,
    pub github: GitHubConfig,
    pub aviator: AviatorConfig,
    #[serde(default)] // Ensures Vec defaults to empty if missing in TOML
    pub additional_trunk_branches: Vec<String>,
    pub remote: Option<String>,
}
