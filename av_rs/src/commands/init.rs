use anyhow::{Context, Result};
// Remove direct git2::Repository import, use AvRepo from GIT_REPO
// use git2::Repository;
use log::info;
use crate::gh::GhRepositoryDetails;
use crate::gh::GhClient;
use serde_json;
use std::fs::{create_dir_all, File};
use std::io::Write;
use tokio;
use crate::GLOBAL_CONFIG;
use crate::GIT_REPO; // Import the static GIT_REPO

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

    // Use common_dir from AvRepo for metadata path
    let metadata_dir = av_repo.common_dir.join("av");
    create_dir_all(&metadata_dir)
        .with_context(|| format!("Failed to create directory at {:?}", metadata_dir))?;

    let metadata_file_path = metadata_dir.join("metadata.json");
    let file = File::create(&metadata_file_path)
        .with_context(|| format!("Failed to create metadata.json file at {:?}", metadata_file_path))?;

    serde_json::to_writer_pretty(file, &repo_details)
        .context("Failed to write metadata to JSON file")?;

    info!("Successfully wrote repository metadata to {:?}", metadata_file_path);

    // TODO: Commit transaction (if using a transactional database)
    info!("Successfully initialized repository for use with av!");
    Ok(())
}
