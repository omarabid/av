use anyhow::{Context, Result};
use git2::{Repository as Git2Repository, ErrorCode};
use log::debug;
use std::path::{Path, PathBuf};

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
}
