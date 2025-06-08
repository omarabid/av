use anyhow::{Context, Result};
use git2::Repository;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::{create_dir_all, File};
use std::io::Write;
use tokio;
// To access GLOBAL_CONFIG if it's made public in main.rs or lib.rs
// use crate::GLOBAL_CONFIG;

#[derive(Serialize, Deserialize, Debug)]
struct GhRepositoryDetails {
    id: String,
    owner: String,
    name: String,
}

async fn fetch_github_repo_details(slug: &str) -> Result<GhRepositoryDetails> {
    info!("(Mock) Fetching GitHub repository details for slug: {}", slug);
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await; // Simulate network delay
    let parts: Vec<&str> = slug.split('/').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!("Invalid slug format: {}", slug));
    }
    Ok(GhRepositoryDetails {
        id: format!("ghid_mock_{}", parts[1]),
        owner: parts[0].to_string(),
        name: parts[1].to_string(),
    })
}

fn parse_github_slug(url: &str) -> Result<String> {
    let identifier = if url.contains("github.com:") {
        // Handle git@github.com:owner/repo.git
        url.split("github.com:")
            .nth(1)
            .context("Failed to parse SSH URL")?
    } else if url.contains("github.com/") {
        // Handle https://github.com/owner/repo.git or https://github.com/owner/repo
        url.split("github.com/")
            .nth(1)
            .context("Failed to parse HTTPS URL")?
    } else {
        anyhow::bail!("URL does not appear to be a GitHub repository URL: {}", url);
    };

    let slug = identifier.trim_end_matches(".git");

    if !slug.contains('/') || slug.starts_with('/') || slug.ends_with('/') {
        anyhow::bail!("Parsed slug is not in 'owner/repo' format: {}", slug);
    }

    Ok(slug.to_string())
}

pub async fn run(directory: Option<String>) -> anyhow::Result<()> {
    // Example of how a command would access the global config:
    // let config = crate::GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug.");
    // info!("GLOBAL_CONFIG.github.token (from init cmd): {:?}", config.github.token);
    // info!("GLOBAL_CONFIG.pull_request.open_browser (from init cmd): {:?}", config.pull_request.open_browser);

    info!("[COMMAND] Init - starting initialization...");

    let repo_path = directory.as_deref().unwrap_or(".");
    let repo = Repository::open(repo_path)
        .with_context(|| format!("Failed to open git repository at '{}'", repo_path))?;
    info!("Opened repository at: {:?}", repo.path());

    let origin_remote = repo
        .find_remote("origin")
        .context("Failed to find remote 'origin'")?;
    let origin_url = origin_remote
        .url()
        .context("Origin remote URL is not valid UTF-8")?;
    info!("Found origin remote URL: {}", origin_url);

    let slug = parse_github_slug(origin_url)?;
    info!("Parsed GitHub slug: {}", slug);

    let repo_details = fetch_github_repo_details(&slug).await?;
    info!("(Mock) Fetched GitHub repository details: {:?}", repo_details);

    let metadata_dir = repo.path().join("av");
    create_dir_all(&metadata_dir)
        .with_context(|| format!("Failed to create .git/av directory at {:?}", metadata_dir))?;

    let metadata_file_path = metadata_dir.join("metadata.json");
    let file = File::create(&metadata_file_path)
        .with_context(|| format!("Failed to create metadata.json file at {:?}", metadata_file_path))?;

    serde_json::to_writer_pretty(file, &repo_details)
        .context("Failed to write metadata to JSON file")?;

    info!("Successfully wrote repository metadata to {:?}", metadata_file_path);

    // TODO: Get GitHub client (actual, not mock)
    // TODO: Commit transaction (if using a transactional database)
    info!("Successfully initialized repository for use with av!");
    Ok(())
}
