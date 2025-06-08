use anyhow::{Context, Result};
// Remove direct git2::Repository import, use AvRepo from GIT_REPO
// use git2::Repository;
use log::info;
use crate::gh::GhRepositoryDetails; // This is effectively RepositoryMeta due to the alias in meta::types
use crate::gh::GhClient;
// serde_json, std::fs, std::io::Write are no longer directly needed for metadata writing logic here
// use serde_json;
// use std::fs::{create_dir_all, File};
// use std::io::Write;
use tokio;
use crate::GLOBAL_CONFIG;
use crate::GIT_REPO; // Import the static GIT_REPO
use crate::meta::{JsonFileDb, MetadataState};

// Removed local GhRepositoryDetails struct definition
// Removed mock fetch_github_repo_details async function

fn parse_github_slug(url: &str) -> Result<(String, String)> {
    let slug_str = if url.contains("github.com:") {
        // Handle git@github.com:owner/repo.git
        url.split("github.com:")
            .nth(1)
            .context("Failed to parse SSH URL for slug")?
    } else if url.contains("github.com/") {
        // Handle https://github.com/owner/repo.git or https://github.com/owner/repo
        url.split("github.com/")
            .nth(1)
            .context("Failed to parse HTTPS URL for slug")?
    } else {
        anyhow::bail!("URL does not appear to be a GitHub repository URL: {}", url);
    };

    let slug_trimmed = slug_str.trim_end_matches(".git");

    let parts: Vec<&str> = slug_trimmed.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!(
            "Parsed slug is not in 'owner/repo' format: {}",
            slug_trimmed
        );
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

pub async fn run(directory: Option<String>) -> anyhow::Result<()> {
    let config = GLOBAL_CONFIG
        .get()
        .expect("GLOBAL_CONFIG not initialized. This is a bug.");
    // info!("GLOBAL_CONFIG.github.token (from init cmd): {:?}", config.github.token);
    // info!("GLOBAL_CONFIG.pull_request.open_browser (from init cmd): {:?}", config.pull_request.open_browser);

    info!("[COMMAND] Init - starting initialization...");

    // Get the AvRepo instance from the static variable
    let av_repo = GIT_REPO
        .get()
        .expect("GIT_REPO not initialized. This is a bug.")
        .as_ref()
        .context("`init` command requires to be run inside a Git repository or for a directory to be specified.")?;

    info!("Operating in repository with workdir: {:?}, .git dir: {:?}", av_repo.workdir, av_repo.common_dir);

    // Use AvRepo to find the remote URL
    let origin_url = av_repo.find_remote_url("origin")?;
    info!("Found origin remote URL: {}", origin_url);

    let (owner, repo_name) = parse_github_slug(origin_url)?;
    info!("Parsed GitHub slug: owner='{}', name='{}'", owner, repo_name);

    let gh_token = config
        .github
        .token
        .as_deref()
        .context("GitHub token not configured. Please set AV_GITHUB_TOKEN or GITHUB_TOKEN, or configure in av/config.toml")?;

    let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;

    info!("Fetching repository details from GitHub for {}/{}...", owner, repo_name);
    let repo_details = gh_client.get_repository_details(&owner, &repo_name).await?;
    info!("Fetched GitHub repository details: {:?}", repo_details);

    // Initialize JsonFileDb with the repository's common_dir (e.g., .git/)
    let db = JsonFileDb::new(&av_repo.common_dir);

    // Read existing state, or default if none
    // .unwrap_or_default() is used for simplicity; could also propagate error or handle specifically
    let mut current_state = db.read_state().unwrap_or_default();

    // Set the repository details in the state
    // repo_details is GhRepositoryDetails, which is type-aliased to RepositoryMeta in meta::types
    current_state.repository = Some(repo_details.clone());

    // Write the updated state back to the JSON file
    db.write_state(&current_state)
        .context("Failed to write repository metadata using JsonFileDb")?;

    info!("Successfully wrote repository metadata to {:?}", db.metadata_file_path());

    // TODO: Commit transaction (if using a transactional database) - this remains if db operations become more complex
    info!("Successfully initialized repository for use with av!");
    Ok(())
}
