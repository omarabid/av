use super::types::{BranchMeta, MetadataState}; // Added BranchMeta
use anyhow::{Context, Result};
use log::debug;
use std::collections::HashMap; // Added for get_all_branches return type
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf}; // Added PathBuf

const METADATA_FILENAME: &str = "av_metadata.json";

pub struct JsonFileDb<'a> {
    repo_common_dir: &'a Path, // e.g., .git/
}

impl<'a> JsonFileDb<'a> {
    pub fn new(repo_common_dir: &'a Path) -> Self {
        Self { repo_common_dir }
    }

    fn av_metadata_dir(&self) -> PathBuf {
        self.repo_common_dir.join("av")
    }

    // Make this public for use in init.rs logging
    pub fn metadata_file_path(&self) -> PathBuf {
        self.av_metadata_dir().join(METADATA_FILENAME)
    }

    pub fn read_state(&self) -> Result<MetadataState> {
        let path = self.metadata_file_path();
        if !path.exists() {
            debug!("Metadata file {:?} does not exist, returning default state.", path);
            return Ok(MetadataState::default());
        }
        let file = File::open(&path)
            .with_context(|| format!("Failed to open metadata file: {:?}", path))?;
        let reader = BufReader::new(file);
        let state: MetadataState = serde_json::from_reader(reader)
            .with_context(|| format!("Failed to deserialize metadata from file: {:?}", path))?;
        debug!("Successfully read metadata state from {:?}", path);
        Ok(state)
    }

    pub fn write_state(&self, state: &MetadataState) -> Result<()> {
        let av_dir = self.av_metadata_dir();
        fs::create_dir_all(&av_dir)
            .with_context(|| format!("Failed to create directory: {:?}", av_dir))?; // Updated error message
        let path = self.metadata_file_path();
        let file = File::create(&path)
            .with_context(|| format!("Failed to create/truncate metadata file: {:?}", path))?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, state)
            .with_context(|| format!("Failed to serialize metadata to file: {:?}", path))?;
        debug!("Successfully wrote metadata state to {:?}", path);
        Ok(())
    }

    // Branch-specific operations that read-modify-write the state
    pub fn upsert_branch(&self, branch_meta_to_upsert: &BranchMeta) -> Result<()> {
        let mut state = self.read_state().context("Failed to read state for upserting branch")?;
        // Using the MetadataState's own method, which is slightly cleaner if it exists.
        // Otherwise, direct manipulation here is also fine.
        state.upsert_branch(branch_meta_to_upsert.clone());
        self.write_state(&state).context("Failed to write state after upserting branch")
    }

    pub fn get_branch(&self, branch_name: &str) -> Result<Option<BranchMeta>> {
        let state = self.read_state().context("Failed to read state for getting branch")?;
        // Using the MetadataState's own method.
        Ok(state.branch(branch_name).cloned())
    }

    pub fn delete_branch(&self, branch_name: &str) -> Result<Option<BranchMeta>> {
        let mut state = self.read_state().context("Failed to read state for deleting branch")?;
        // Using the MetadataState's own method.
        let removed_branch = state.delete_branch(branch_name);
        if removed_branch.is_some() {
            self.write_state(&state).context("Failed to write state after deleting branch")?;
        }
        Ok(removed_branch)
    }

    pub fn get_all_branches(&self) -> Result<HashMap<String, BranchMeta>> {
        let state = self.read_state().context("Failed to read state for getting all branches")?;
        Ok(state.branches) // Directly returning the HashMap from MetadataState
    }
}
