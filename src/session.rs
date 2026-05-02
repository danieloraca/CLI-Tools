use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppSession {
    pub profile_id: String,
    pub account_id: String,
    pub account_name: String,
    pub app_description: String,
    pub user_id: String,
    pub redirect_url: String,
}

pub fn save_session(session: &AppSession, session_file: Option<&Path>) -> Result<()> {
    let Some(session_file) = session_file else {
        return Ok(());
    };

    if let Some(parent) = session_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let data = serde_json::to_vec_pretty(session)?;
    fs::write(session_file, data)
        .with_context(|| format!("failed to write session to {}", session_file.display()))?;

    Ok(())
}

pub fn load_session(session_file: &Path) -> Result<AppSession> {
    let data = fs::read(session_file)
        .with_context(|| format!("failed to read session from {}", session_file.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse session from {}", session_file.display()))
}

pub fn default_session_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config/gecko/session.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_session_shape() {
        let session = AppSession {
            profile_id: "p-1".to_string(),
            account_id: "281".to_string(),
            account_name: "Stage 281".to_string(),
            app_description: "Forms".to_string(),
            user_id: "2260".to_string(),
            redirect_url: "https://example.test".to_string(),
        };

        let encoded = serde_json::to_value(&session).unwrap();

        assert_eq!(encoded["account_id"], "281");
        assert_eq!(encoded["user_id"], "2260");
    }
}
