use anyhow::{Context, Result, anyhow};
use clap::Parser;
use log::{info, debug, warn};

use crate::git_ops::AvRepo;
use crate::meta::JsonFileDb;
use crate::GIT_REPO;

#[derive(Parser, Debug)]
#[clap(about = "Record changes to the repository, updating av metadata")]
pub struct CommitOpts {
    #[clap(short, long, help = "Commit message")]
    pub message: Option<String>,

    #[clap(long, help = "Amend the previous commit")]
    pub amend: bool,

    // TODO: #[clap(short, long, help = "Stage all modified/deleted files")]
    // TODO: pub all: bool,
}

pub async fn run_commit_cmd(opts: CommitOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("Commit command requires to be run inside a Git repository")?;

    if !opts.amend && opts.message.is_none() {
        // In a real CLI, you might shell out to `git commit` here to open an editor.
        // For this non-interactive version, require -m if not amending.
        return Err(anyhow!("Commit message is required via -m unless using --amend."));
    }

    let signature = av_repo.git2_repo.signature().context("Failed to get git signature for commit author/committer. Configure user.name and user.email in your Git config.")?;

    let new_commit_oid: git2::Oid;

    if opts.amend {
        info!("Amending previous commit...");
        let head_ref = av_repo.git2_repo.head().context("Failed to get HEAD reference for amend.")?;
        let head_commit_obj = head_ref.peel_to_commit()
            .context("HEAD is not a commit or cannot be peeled to a commit, cannot amend.")?;

        // For amend, message can be optional if user wants to keep old one (git opens editor).
        // Here, if no -m, we use the existing commit message.
        let message_to_use = match &opts.message {
            Some(m) => m.as_str(),
            None => head_commit_obj.message().unwrap_or(""), // Fallback to empty if somehow message is not UTF-8
        };

        debug!("Amending with message: '{}'", message_to_use);

        // Use existing author/committer unless specified otherwise (not supported yet by this command)
        let author_sig = head_commit_obj.author();
        let committer_sig = signature; // Use current user as committer for amend

        // For amend, the tree is usually taken from the index unless specified.
        // If the index is empty or unchanged, it might re-use the old tree.
        // To ensure what's staged is used:
        let mut index = av_repo.git2_repo.index().context("Failed to get repository index for amend.")?;
        let tree_oid = index.write_tree().context("Failed to write index to tree for amend. Any files staged?")?;
        let tree = av_repo.git2_repo.find_tree(tree_oid).context("Failed to find newly written tree for amend")?;


        new_commit_oid = head_commit_obj.amend(
            None, // update_ref: None means HEAD will be updated by default
            Some(&author_sig),
            Some(&committer_sig), // Current user is the committer of the amend action
            None, // message_encoding
            Some(message_to_use),
            Some(&tree) // Use tree from current index state
        ).context("Failed to amend commit using git2. Ensure files are staged if you intended to change content.")?;
        info!("Successfully amended commit. New HEAD: {}", new_commit_oid.to_string().chars().take(7).collect::<String>());

    } else {
        // New commit
        let message = opts.message.expect("Message should be Some if not amending, checked earlier.");
        info!("Creating new commit with message: {}...", message);

        let mut index = av_repo.git2_repo.index().context("Failed to get repository index. Corrupted?")?;
        let tree_oid = index.write_tree().context("Failed to write index to tree. Any files staged? `git commit` normally fails if staging area is empty.")?;
        let tree = av_repo.git2_repo.find_tree(tree_oid).context("Failed to find newly written tree")?;

        let head_ref = av_repo.git2_repo.head().context("Failed to get HEAD reference for new commit.")?;
        let head_commit_obj = head_ref.peel_to_commit()
            .context("HEAD is not a commit or cannot be peeled to a commit. Cannot create new commit on top of it.")?;
        let parent_commits = [&head_commit_obj];

        new_commit_oid = av_repo.git2_repo.commit(
            Some("HEAD"), // update_ref: Update HEAD to point to the new commit
            &signature,   // author
            &signature,   // committer (current user is both author and committer for new commit)
            &message,
            &tree,
            &parent_commits,
        ).context("Failed to create commit using git2. Ensure files are staged if you intended to make changes.")?;
        info!("Successfully created new commit. HEAD: {}", new_commit_oid.to_string().chars().take(7).collect::<String>());
    }

    // Update metadata
    let current_branch_name = av_repo.current_branch_name()?;
    let db = JsonFileDb::new(&av_repo.common_dir);

    if let Some(mut branch_meta) = db.get_branch(&current_branch_name)? {
        let old_head = branch_meta.head_commit.clone();
        branch_meta.head_commit = new_commit_oid.to_string();
        db.upsert_branch(&branch_meta)
            .with_context(|| format!("Failed to update metadata for branch '{}' after commit", current_branch_name))?;
        info!("Updated metadata for branch '{}': head moved from {} to {}.",
              current_branch_name,
              old_head.chars().take(7).collect::<String>(),
              new_commit_oid.to_string().chars().take(7).collect::<String>()
            );
    } else {
        warn!("Current branch '{}' is not tracked by av. Metadata not updated.", current_branch_name);
        // TODO: Implement auto-branch creation if on a trunk branch.
        // This would involve:
        // 1. Checking if current_branch_name is a trunk (e.g., using av_repo.is_trunk_branch()).
        // 2. If it is, prompting for a new branch name (or auto-generating one).
        // 3. Creating the new branch in Git (pointing to new_commit_oid).
        // 4. Checking out the new branch.
        // 5. Creating metadata for this new branch, with current_branch_name as its parent.
    }

    Ok(())
}
