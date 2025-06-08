use serde::{Serialize, Deserialize};
use xdg::BaseDirectories;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use anyhow::{Context, Result};

// Assuming UserState is defined in this file as per the plan
// If it were in config/mod.rs, it would be super::UserState or crate::config::UserState
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct UserState {
    pub notified_stack_sync_change: bool,
}

fn get_user_state_path() -> Result<PathBuf> {
    let xdg_dirs = BaseDirectories::new().context("Failed to get XDG base directories")?;
    // place_state_file will create parent directories if they don't exist when the file is written.
    xdg_dirs
        .place_state_file("av/user-state.json")
        .context("Failed to place user state file in XDG directory")
}

pub fn load_user_state() -> Result<UserState> {
    let path = get_user_state_path()?;
    if path.exists() {
        let mut file = File::open(&path)
            .with_context(|| format!("Failed to open user state file at {:?}", path))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .with_context(|| format!("Failed to read user state file at {:?}", path))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to deserialize user state from {:?}", path))
    } else {
        Ok(UserState::default())
    }
}

pub fn save_user_state(state: &UserState) -> Result<()> {
    let path = get_user_state_path()?;

    // Ensure the parent directory exists (place_state_file in get_user_state_path might not create it on its own)
    if let Some(parent_dir) = path.parent() {
        fs::create_dir_all(parent_dir)
            .with_context(|| format!("Failed to create parent directory for user state file at {:?}", parent_dir))?;
    }

    let mut file = File::create(&path)
        .with_context(|| format!("Failed to create user state file at {:?}", path))?;
    let contents = serde_json::to_string_pretty(state)
        .context("Failed to serialize user state to JSON")?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("Failed to write user state to {:?}", path))?;
    Ok(())
}
