use anyhow::{Context, Result, anyhow};
use git2::{BranchType, CheckoutBuilder, ErrorCode, ObjectType, Oid, Reference, ReferenceType, Repository as Git2Repository};
use log::{debug, warn};
use std::path::{Path, PathBuf};
use crate::GLOBAL_CONFIG; // For is_trunk_branch to access additional_trunk_branches

#[derive(Debug)]
pub struct AvRepo {
    pub git2_repo: Git2Repository,
    /// Absolute path to the working directory of the repository.
    pub workdir: PathBuf,
    /// Absolute path to the .git directory (or common .git dir).
    pub common_dir: PathBuf,
}

impl AvRepo {
    pub fn discover(path_hint: Option<&Path>) -> Result<Self> {
        let discovery_path = path_hint
            .or_else(|| std::env::current_dir().ok().as_deref())
            .context("Neither a path hint nor current directory was available for repository discovery")?;
        debug!("Attempting to discover repository starting from: {:?}", discovery_path);

        let repo = Git2Repository::discover(discovery_path)
            .context(format!("Failed to discover git repository at or above {:?}", discovery_path))?;

        let workdir = repo.workdir()
            .context("Repository has no working directory (bare repository?)")?
            .to_path_buf();
        let common_dir = repo.commondir().to_path_buf();
        debug!("Discovered repo with workdir: {:?}, common_dir: {:?}", workdir, common_dir);

        Ok(Self { git2_repo: repo, workdir, common_dir })
    }

    pub fn current_branch_name(&self) -> Result<String> {
        let head = self.git2_repo.head().context("Failed to get HEAD reference")?;
        if head.is_branch() {
            head.shorthand().context("HEAD branch name is not valid UTF-8")
                .map(String::from)
        } else {
            // Detached HEAD or other state
            Err(anyhow::anyhow!("HEAD is not on a branch (detached HEAD?)"))
        }
    }

    pub fn find_remote_url(&self, remote_name: &str) -> Result<String> {
        let remote = self.git2_repo.find_remote(remote_name)
            .with_context(|| format!("Failed to find remote '{}'", remote_name))?;
        remote.url().context("Remote URL is not valid UTF-8").map(String::from)
    }

    pub fn default_branch_shorthand(&self, remote_name: &str) -> Result<String> {
        let remote_head_ref_name = format!("refs/remotes/{}/HEAD", remote_name);
        let remote_head_ref = self.git2_repo.find_reference(&remote_head_ref_name)
            .with_context(|| format!("Failed to find remote HEAD reference: {}", remote_head_ref_name))?;

        if remote_head_ref.kind() != Some(ReferenceType::Symbolic) {
            return Err(anyhow!("Remote HEAD {} is not a symbolic reference", remote_head_ref_name));
        }

        let target_ref_name = remote_head_ref.symbolic_target()
            .context("Remote HEAD symbolic reference target is not valid UTF-8")?;

        // Target is like "refs/remotes/origin/main", we want "main"
        Path::new(target_ref_name).file_name()
            .and_then(|os_str| os_str.to_str())
            .map(String::from)
            .context(format!("Could not extract shorthand name from remote HEAD target: {}", target_ref_name))
    }

    pub fn branch_exists(&self, branch_name: &str, for_remote: Option<&str>) -> Result<bool> {
        let ref_name = if let Some(remote) = for_remote {
            format!("refs/remotes/{}/{}", remote, branch_name)
        } else {
            format!("refs/heads/{}", branch_name)
        };

        match self.git2_repo.find_reference(&ref_name) {
            Ok(_) => Ok(true),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(false),
            Err(e) => Err(e.into()).context(format!("Failed to check existence of reference: {}", ref_name)),
        }
    }

    pub fn is_trunk_branch(&self, branch_name: &str, default_remote_name: &str) -> Result<bool> {
        // Get default branch for the primary remote (e.g., "origin")
        match self.default_branch_shorthand(default_remote_name) {
            Ok(default_branch) if branch_name == default_branch => return Ok(true),
            Ok(default_branch) => {
                debug!("Branch '{}' is not the default branch ('{}') for remote '{}'. Checking additional trunk branches.",
                       branch_name, default_branch, default_remote_name);
            },
            Err(e) => {
                warn!("Could not determine default branch for remote '{}': {}. Checking additional trunk branches.", default_remote_name, e);
            }
        }

        let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug if called before main sets it.");
        if config.additional_trunk_branches.iter().any(|tb| tb == branch_name) {
            debug!("Branch '{}' found in additional_trunk_branches.", branch_name);
            return Ok(true);
        }

        debug!("Branch '{}' is not a default or additional trunk branch.", branch_name);
        Ok(false)
    }

    /// Checks out a branch. If `new_branch` is true, creates it from `new_head_ref_name` (or current HEAD).
    /// Returns the name of the previously checked-out branch, or an empty string if HEAD was detached.
    pub fn checkout_branch(&self, branch_name: &str, new_branch: bool, new_head_ref_name: Option<&str>) -> Result<String> {
        let previous_branch_name = self.current_branch_name().unwrap_or_default();

        if new_branch {
            let target_commit_ref = new_head_ref_name.unwrap_or("HEAD");
            let commit_obj = self.git2_repo.revparse_single(target_commit_ref)
                .with_context(|| format!("Failed to find commit for '{}'", target_commit_ref))?;

            let commit = commit_obj.as_commit()
                .with_context(|| format!("Object '{}' is not a commit", target_commit_ref))?;

            self.git2_repo.branch(branch_name, commit, false) // false = no force
                .with_context(|| format!("Failed to create new branch '{}'", branch_name))?;
            debug!("Created new branch '{}' pointing to commit {}", branch_name, commit.id());
        }

        let ref_to_checkout = format!("refs/heads/{}", branch_name);
        self.git2_repo.set_head(&ref_to_checkout)
            .with_context(|| format!("Failed to set HEAD to '{}'", ref_to_checkout))?;
        debug!("Set HEAD to '{}'", ref_to_checkout);

        // Use a CheckoutBuilder for more control, matching typical git checkout behavior.
        // .force() is like `git checkout -f`, ensures working directory changes are made.
        self.git2_repo.checkout_head(Some(CheckoutBuilder::new().force()))
            .with_context(|| format!("Failed to checkout HEAD ({})", ref_to_checkout))?;
        debug!("Checked out HEAD successfully");

        Ok(previous_branch_name)
    }

    /// Updates a Git reference.
    /// If `old_oid_str` is Some, performs a compare-and-swap. Otherwise, overwrites.
    /// `create_reflog` determines if a reflog entry should be created (via the message).
    pub fn update_ref(&self, ref_name: &str, new_oid_str: &str, old_oid_str: Option<&str>, create_reflog: bool, reason: &str) -> Result<()> {
        let new_oid = Oid::from_str(new_oid_str).context("Invalid OID string for new_oid_str")?;
        let message = if create_reflog { reason } else { "" };

        if let Some(expected_old_oid_str) = old_oid_str {
            let expected_old_oid = Oid::from_str(expected_old_oid_str)
                .context("Invalid OID string for old_oid_str")?;

            match self.git2_repo.find_reference(ref_name) {
                Ok(mut reference) => {
                    if reference.target() == Some(expected_old_oid) {
                        reference.set_target(new_oid, message)
                            .with_context(|| format!("Failed to set target for ref '{}' (conditional update)", ref_name))?;
                        debug!("Updated ref '{}' from {} to {} (conditional)", ref_name, expected_old_oid_str, new_oid_str);
                    } else {
                        let actual_target = reference.target().map(|o| o.to_string()).unwrap_or_else(|| "None".to_string());
                        return Err(anyhow!(
                            "Reference '{}' current OID ({}) does not match expected old OID ({})",
                            ref_name, actual_target, expected_old_oid_str
                        ));
                    }
                }
                Err(e) if e.code() == ErrorCode::NotFound && old_oid_str == Some("0000000000000000000000000000000000000000") => {
                    // Ref does not exist and old_oid is all zeros (expecting ref creation)
                    self.git2_repo.reference(ref_name, new_oid, false, message) // false = no overwrite, it's new
                        .with_context(|| format!("Failed to create new reference '{}'", ref_name))?;
                    debug!("Created new ref '{}' with OID {}", ref_name, new_oid_str);
                }
                Err(e) => {
                    return Err(e).context(format!("Failed to find reference '{}' for conditional update", ref_name));
                }
            }
        } else {
            // Unconditional update, overwrite if exists, create if not.
            self.git2_repo.reference(ref_name, new_oid, true, message) // true = force (overwrite)
                .with_context(|| format!("Failed to update reference '{}' (unconditional)", ref_name))?;
            debug!("Updated ref '{}' to OID {} (unconditional)", ref_name, new_oid_str);
        }
        Ok(())
    }

    /// Finds the merge base between two commits.
    pub fn merge_base(&self, commit_a_str: &str, commit_b_str: &str) -> Result<Option<Oid>> {
        let oid_a = Oid::from_str(commit_a_str)
            .with_context(|| format!("Invalid OID string for commit_a: '{}'", commit_a_str))?;
        let oid_b = Oid::from_str(commit_b_str)
            .with_context(|| format!("Invalid OID string for commit_b: '{}'", commit_b_str))?;

        match self.git2_repo.merge_base(oid_a, oid_b) {
            Ok(merge_base_oid) => {
                debug!("Merge base for {} and {} is {}", commit_a_str, commit_b_str, merge_base_oid);
                Ok(Some(merge_base_oid))
            },
            Err(e) if e.code() == ErrorCode::NotFound => {
                // This typically means no common ancestor or one/both commits not found.
                // Git CLI `merge-base` returns non-zero if commits don't exist.
                // `git2::merge_base` returns NotFound if no common ancestor.
                // Let's verify if commits exist first for better error.
                self.git2_repo.find_commit(oid_a).with_context(|| format!("Commit A '{}' not found", commit_a_str))?;
                self.git2_repo.find_commit(oid_b).with_context(|| format!("Commit B '{}' not found", commit_b_str))?;
                debug!("No common merge base found for {} and {}", commit_a_str, commit_b_str);
                Ok(None)
            }
            Err(e) => Err(e.into()).context(format!("Failed to find merge base for commits '{}' and '{}'", commit_a_str, commit_b_str)),
        }
    }
}
