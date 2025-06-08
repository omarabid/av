use anyhow::{Context, Result};
use log::{info, debug};

use crate::git_ops::AvRepo;
use crate::gh::GhClient;
use crate::meta::{JsonFileDb, BranchMeta, PullRequestMeta as AvPullRequestMeta}; // AvPullRequestMeta for clarity
// Assuming GlobalConfig might be needed for default remote, etc.
// Not directly using GlobalConfig here, but CreatePullRequestOpts will carry necessary info like default_remote_name
// use crate::config::AvConfig; // Not directly used

#[derive(Debug)]
pub struct CreatePullRequestOpts<'a> {
    // Information about the repository where the PR will be created
    pub repo_owner: &'a str, // This comes from RepositoryMeta.owner.login
    pub repo_name: &'a str,  // This comes from RepositoryMeta.name
    pub repo_node_id: &'a str, // GitHub GraphQL Node ID for the repository, from RepositoryMeta.id

    // Branch information
    pub head_ref_name: &'a str, // The branch to be merged (e.g., "feature-branch")
    pub base_ref_name: &'a str, // The branch to merge into (e.g., "main")

    // PR content
    pub title: String,
    pub body: String,
    pub is_draft: bool,

    // Control flags
    pub no_push: bool,
    pub default_remote_name: &'a str,
}

pub async fn create_pull_request(
    av_repo: &AvRepo,
    gh_client: &GhClient,
    db: &JsonFileDb, // Changed from &mut to & as upsert_branch takes &self
    opts: CreatePullRequestOpts<'_>,
) -> Result<AvPullRequestMeta> {
    debug!("Action: Creating PR for head '{}' into base '{}'", opts.head_ref_name, opts.base_ref_name);

    // 1. Push branch (if not opted out)
    if !opts.no_push {
        info!("Pushing branch '{}' to remote '{}'...", opts.head_ref_name, opts.default_remote_name);
        let refspec = format!("refs/heads/{}:refs/heads/{}", opts.head_ref_name, opts.head_ref_name);
        av_repo.push(opts.default_remote_name, &[&refspec], false) // false = no force push for initial PR creation
            .with_context(|| format!("Failed to push branch '{}' before PR creation", opts.head_ref_name))?;
        info!("Branch '{}' pushed successfully.", opts.head_ref_name);
    } else {
        info!("Skipping push for branch '{}' due to --no-push flag.", opts.head_ref_name);
    }

    // 2. Create PR on GitHub
    info!("Creating PR on GitHub for {}/{} (head: {}, base: {}, title: '{}', draft: {})",
        opts.repo_owner, opts.repo_name, opts.head_ref_name, opts.base_ref_name, opts.title, opts.is_draft);

    let pr_node = gh_client.create_pull_request(
        opts.repo_node_id,
        opts.base_ref_name,
        opts.head_ref_name,
        &opts.title,
        &opts.body,
        opts.is_draft,
    ).await.with_context(|| format!("Failed to create pull request on GitHub for branch '{}'", opts.head_ref_name))?;

    info!("Successfully created PR #{} for branch '{}': {}", pr_node.number, opts.head_ref_name, pr_node.permalink);

    // 3. Update local metadata
    let mut branch_meta = db.get_branch(opts.head_ref_name)?
        .with_context(|| format!("Branch '{}' not found in av metadata after PR creation. This should not happen if it was pushed and tracked.", opts.head_ref_name))?;

    let created_pr_meta = AvPullRequestMeta {
        id: pr_node.id,
        number: pr_node.number,
        permalink: pr_node.permalink,
    };

    branch_meta.pull_request = Some(created_pr_meta.clone());
    db.upsert_branch(&branch_meta)
        .with_context(|| format!("Failed to update branch metadata for '{}' with PR info", opts.head_ref_name))?;
    info!("Branch metadata for '{}' updated with PR information.", opts.head_ref_name);

    Ok(created_pr_meta)
}
