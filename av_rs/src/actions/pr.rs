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

#[derive(Debug)]
pub struct UpdatePrBodyWithStackOpts<'a> {
    pub pr_node_id: &'a str, // GitHub GraphQL Node ID of the PR to update
    pub pr_number: i64,         // For logging
    pub pr_current_body: Option<String>,
    pub stack_branch_metas: Vec<&'a BranchMeta>, // Metadata for all branches in the stack
    // repo_owner and repo_name are not strictly needed by this action if GhClient is pre-configured,
    // but can be useful for logging or if the action were to, e.g., generate links.
    // pub repo_owner: &'a str,
    // pub repo_name: &'a str,
}

pub async fn update_pr_body_with_stack_info(
    gh_client: &GhClient,
    opts: UpdatePrBodyWithStackOpts<'_>,
) -> Result<()> {
    let stack_begin_comment = "<!-- av_stack_begin -->";
    let stack_end_comment = "<!-- av_stack_end -->";

    let mut stack_info_md = String::new();
    stack_info_md.push_str("\n\n"); // Add some spacing before the stack info
    stack_info_md.push_str(stack_begin_comment);
    stack_info_md.push_str("\n**Stack tree:**\n");

    // Iterate through the provided stack_branch_metas to build the list
    // Assumes stack_branch_metas is already ordered if a specific order is desired.
    // For simplicity, just listing them. A tree rendering would be more complex.
    if opts.stack_branch_metas.is_empty() {
        stack_info_md.push_str("- This PR is not part of a stack.\n");
    } else {
        for bm in opts.stack_branch_metas {
            // Skip the current PR itself in the list if it's part of the stack_branch_metas
            if bm.pull_request.as_ref().map_or(false, |pr| pr.number == opts.pr_number) {
                // Optionally, mark the current PR, e.g., by bolding or an arrow
                // stack_info_md.push_str(&format!("- **PR #{} (Current)** (branch `{}`): {}\n", pr.number, bm.name, pr.permalink));
                continue;
            }

            if let Some(pr) = &bm.pull_request {
                stack_info_md.push_str(&format!("- PR #{} (branch `{}`): {}\n", pr.number, bm.name, pr.permalink));
            } else {
                stack_info_md.push_str(&format!("- (No PR yet) branch `{}`\n", bm.name));
            }
        }
    }
    stack_info_md.push_str(stack_end_comment);

    let mut current_body = opts.pr_current_body.unwrap_or_default();

    // Remove old stack section if it exists
    if let Some(start_idx) = current_body.find(stack_begin_comment) {
        if let Some(end_idx) = current_body.rfind(stack_end_comment) { // rfind for robustness in case of malformed old comment
            // Ensure end_idx is after start_idx before replacing
            if end_idx + stack_end_comment.len() > start_idx {
                 current_body.replace_range(start_idx..(end_idx + stack_end_comment.len()), "");
            }
        }
    }

    // Trim whitespace that might be left after removing the old section or from an empty original body.
    let new_body = format!("{}{}", current_body.trim(), stack_info_md);

    gh_client.update_pull_request(opts.pr_node_id, None, Some(new_body), None).await
        .with_context(|| format!("Failed to update PR #{} body with stack information", opts.pr_number))?;

    info!("Updated PR #{} body with stack information.", opts.pr_number);
    Ok(())
}
