use anyhow::{Context, Result, anyhow};
use clap::Parser;
use log::{info, debug, warn};

use crate::git_ops::AvRepo;
use crate::meta::{JsonFileDb, BranchMeta as AvBranchMeta}; // Renamed to avoid conflict
use crate::gh::GhClient; // For PR status check
use crate::{GIT_REPO, GLOBAL_CONFIG}; // Need GLOBAL_CONFIG

#[derive(Parser, Debug)]
#[clap(about = "Record changes to the repository, updating av metadata")]
pub struct CommitOpts {
    #[clap(short, long, help = "Commit message")]
    pub message: Option<String>,

    #[clap(long, help = "Amend the previous commit")]
    pub amend: bool,

    #[clap(long, help = "Name for new branch if auto-branching (required if on trunk/shippable)")]
    pub new_branch_name: Option<String>,

    // TODO: #[clap(short, long, help = "Stage all modified/deleted files")]
    // TODO: pub all: bool,
}

pub async fn run_commit_cmd(opts: CommitOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("Commit command requires to be run inside a Git repository")?;

    let mut current_branch_name_for_commit = av_repo.current_branch_name()?;
    let mut did_auto_branch = false;
    let mut new_branch_parent_name_for_meta: Option<String> = None;
    let mut new_branch_parent_oid_for_meta: Option<String> = None;

    if !opts.amend { // Auto-branching only for new commits
        let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug.");
        let db_for_check = JsonFileDb::new(&av_repo.common_dir); // Temp DB instance for PR check
        let branch_meta_opt_for_check = db_for_check.get_branch(&current_branch_name_for_commit)?;
        let default_remote = config.remote.as_deref().unwrap_or("origin");

        let is_on_trunk = av_repo.is_trunk_branch(&current_branch_name_for_commit, default_remote)?;
        let mut is_on_shippable_pr_branch = false;

        if let Some(ref meta) = branch_meta_opt_for_check {
            if let Some(ref pr) = meta.pull_request {
                let gh_token = config.github.token.as_deref()
                    .context("GitHub token is required to check PR status for auto-branching. Please configure GITHUB_TOKEN or AV_GITHUB_TOKEN.")?;
                let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;

                let repo_info = db_for_check.read_state()?.repository
                    .with_context(|| "Repository metadata not found. Please run `av init` to initialize.")?;

                match gh_client.get_pull_request_status(&repo_info.owner.login, &repo_info.name, pr.number).await {
                    Ok(pr_status) => {
                        if matches!(pr_status.state, crate::gh::types::PullRequestState::Merged | crate::gh::types::PullRequestState::Closed) {
                            is_on_shippable_pr_branch = true;
                            info!("Current branch '{}' has a {} PR (#{}). It's considered shippable.", current_branch_name_for_commit, format!("{:?}", pr_status.state).to_uppercase(), pr.number);
                        }
                    }
                    Err(e) => warn!("Could not fetch PR status for branch '{}' (PR #{}): {}. Assuming not shippable for auto-branching.", current_branch_name_for_commit, pr.number, e),
                }
            }
        }

        if is_on_trunk || is_on_shippable_pr_branch {
            let reason = if is_on_trunk { "a trunk branch" } else { "a shippable PR branch" };
            info!("Current branch '{}' is {}. A new branch will be created for this commit.", current_branch_name_for_commit, reason);

            let new_name = opts.new_branch_name.as_deref()
                .ok_or_else(|| anyhow!("Committing on {} or shippable branch requires a new branch name. Please provide --new-branch-name <NAME>.", reason))?;

            if av_repo.branch_exists(new_name, None)? || av_repo.branch_exists(new_name, Some(default_remote))? {
                return Err(anyhow!("New branch name '{}' already exists locally or on remote '{}'. Please choose a different name.", new_name, default_remote));
            }

            let old_head_commit_oid = av_repo.find_commit("HEAD")?.context("Failed to get current HEAD commit OID before auto-branching")?.id().to_string();

            av_repo.checkout_branch(new_name, true, Some(&current_branch_name_for_commit))
                .with_context(|| format!("Failed to create and checkout new branch '{}' from '{}'", new_name, current_branch_name_for_commit))?;
            info!("Created and checked out new branch '{}' from '{}'. Commit will be applied here.", new_name, current_branch_name_for_commit);

            new_branch_parent_name_for_meta = Some(current_branch_name_for_commit.clone());
            new_branch_parent_oid_for_meta = Some(old_head_commit_oid);
            current_branch_name_for_commit = new_name.to_string(); // Subsequent operations use the new branch name
            did_auto_branch = true;
        }
    }


    if !opts.amend && opts.message.is_none() {
        return Err(anyhow!("Commit message is required via -m unless using --amend."));
    }

    let signature = av_repo.git2_repo.signature().context("Failed to get git signature for commit author/committer. Configure user.name and user.email in your Git config.")?;
    let new_commit_oid: git2::Oid;

    if opts.amend {
        info!("Amending previous commit on branch '{}'...", current_branch_name_for_commit);
        // Ensure current_branch_name_for_commit is up-to-date if auto-branching occurred
        let head_ref = av_repo.git2_repo.head().context("Failed to get HEAD reference for amend.")?;
        let head_commit_obj = head_ref.peel_to_commit()
            .context("HEAD is not a commit or cannot be peeled to a commit, cannot amend.")?;

        let message_to_use = opts.message.as_deref().unwrap_or_else(|| head_commit_obj.message().unwrap_or(""));
        debug!("Amending with message: '{}'", message_to_use);

        let author_sig = head_commit_obj.author(); // Keep original author
        let committer_sig = signature; // Current user is committer of the amend

        let mut index = av_repo.git2_repo.index().context("Failed to get repository index for amend.")?;
        let tree_oid = index.write_tree().context("Failed to write index to tree for amend. Ensure files are staged if changes are intended.")?;
        let tree = av_repo.git2_repo.find_tree(tree_oid).context("Failed to find newly written tree for amend")?;

        new_commit_oid = head_commit_obj.amend(None, Some(&author_sig), Some(&committer_sig), None, Some(message_to_use), Some(&tree))
            .context("Failed to amend commit using git2. Ensure files are staged if you intended to change content.")?;
        info!("Successfully amended commit on branch '{}'. New HEAD: {}", current_branch_name_for_commit, new_commit_oid.to_string().chars().take(7).collect::<String>());

    } else { // New commit
        let message = opts.message.expect("Message should be Some if not amending, checked earlier.");
        info!("Creating new commit on branch '{}' with message: {}...", current_branch_name_for_commit, message);

        let mut index = av_repo.git2_repo.index().context("Failed to get repository index. Corrupted?")?;
        let tree_oid = index.write_tree().context("Failed to write index to tree. Any files staged? `git commit` normally fails if staging area is empty.")?;
        let tree = av_repo.git2_repo.find_tree(tree_oid).context("Failed to find newly written tree")?;

        let head_ref = av_repo.git2_repo.head().context("Failed to get HEAD reference for new commit.")?;
        let parent_commit_obj = head_ref.peel_to_commit() // This should be the parent commit (either original branch head or new parent after auto-branch)
            .context("HEAD is not a commit or cannot be peeled. Cannot create new commit on top of it.")?;
        let parent_commits = [&parent_commit_obj];

        new_commit_oid = av_repo.git2_repo.commit(Some("HEAD"), &signature, &signature, &message, &tree, &parent_commits)
            .context("Failed to create commit using git2. Ensure files are staged if you intended to make changes.")?;
        info!("Successfully created new commit on branch '{}'. HEAD: {}", current_branch_name_for_commit, new_commit_oid.to_string().chars().take(7).collect::<String>());
    }

    // Update metadata
    let db = JsonFileDb::new(&av_repo.common_dir);
    if did_auto_branch {
        // Create metadata for the new auto-created branch
        let new_branch_meta = AvBranchMeta { // Use the renamed import
            name: current_branch_name_for_commit.clone(),
            parent_branch: new_branch_parent_name_for_meta,
            parent_commit: new_branch_parent_oid_for_meta,
            head_commit: new_commit_oid.to_string(),
            pull_request: None,
        };
        db.upsert_branch(&new_branch_meta)
            .with_context(|| format!("Failed to save metadata for new auto-created branch '{}'", current_branch_name_for_commit))?;
        info!("Saved metadata for new branch '{}'. Head: {}", current_branch_name_for_commit, new_commit_oid.to_string().chars().take(7).collect::<String>());
    } else {
        // Original logic: update existing branch's metadata if tracked
        if let Some(mut branch_meta) = db.get_branch(&current_branch_name_for_commit)? {
            let old_head = branch_meta.head_commit.clone();
            branch_meta.head_commit = new_commit_oid.to_string();
            db.upsert_branch(&branch_meta)
                .with_context(|| format!("Failed to update metadata for branch '{}' after commit", current_branch_name_for_commit))?;
            info!("Updated metadata for branch '{}': head moved from {} to {}.",
                  current_branch_name_for_commit,
                  old_head.chars().take(7).collect::<String>(),
                  new_commit_oid.to_string().chars().take(7).collect::<String>());
        } else {
            warn!("Current branch '{}' is not tracked by av. Metadata not updated.", current_branch_name_for_commit);
        }
    }

    Ok(())
}
