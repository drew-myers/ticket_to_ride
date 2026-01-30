use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Main configuration structure for ttr
#[derive(Debug, Deserialize)]
pub struct Config {
    pub github: GitHubConfig,
    #[serde(default)]
    pub mapping: MappingConfig,
    #[serde(default)]
    pub labels: LabelsConfig,
}

#[derive(Debug, Deserialize)]
pub struct GitHubConfig {
    /// Repository in "owner/repo" format
    pub repo: String,
    /// Optional GitHub Project name or number
    pub project: Option<String>,
    /// Optional assignee for all created issues
    pub assignee: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MappingConfig {
    /// Project field name for ticket type (default: "Type")
    #[serde(default = "default_type_field")]
    pub type_field: String,
    /// Mapping from ticket type to project field value
    #[serde(rename = "type", default)]
    pub type_map: HashMap<String, String>,
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            type_field: default_type_field(),
            type_map: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LabelsConfig {
    /// Sync ticket tags as GitHub labels (default: true)
    #[serde(default = "default_true")]
    pub sync_tags: bool,
    /// Create labels if they don't exist (default: true)
    #[serde(default = "default_true")]
    pub create_missing: bool,
}

impl Default for LabelsConfig {
    fn default() -> Self {
        Self {
            sync_tags: true,
            create_missing: true,
        }
    }
}

fn default_type_field() -> String {
    "Type".to_string()
}

fn default_true() -> bool {
    true
}

impl GitHubConfig {
    /// Parse repo into (owner, name) tuple
    pub fn repo_parts(&self) -> Result<(&str, &str)> {
        let parts: Vec<&str> = self.repo.split('/').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid repo format '{}'. Expected 'owner/repo'", self.repo);
        }
        Ok((parts[0], parts[1]))
    }
}

impl Config {
    /// Load configuration from .tickets/sync.toml
    /// Searches current directory and parent directories
    pub fn load() -> Result<(Self, PathBuf)> {
        let tickets_dir = find_tickets_dir()?;
        let config_path = tickets_dir.join("sync.toml");

        if !config_path.exists() {
            anyhow::bail!(
                "Configuration file not found: {}\nRun 'ttr init' to create one.",
                config_path.display()
            );
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?;

        // Validate required fields
        config.github.repo_parts()?;

        Ok((config, tickets_dir))
    }
}

/// Find .tickets directory by walking up from current directory
pub fn find_tickets_dir() -> Result<PathBuf> {
    // Check TICKETS_DIR env var first
    if let Ok(dir) = env::var("TICKETS_DIR") {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Ok(path);
        }
    }

    // Walk up from current directory
    let mut dir = env::current_dir().context("Failed to get current directory")?;

    loop {
        let tickets_dir = dir.join(".tickets");
        if tickets_dir.is_dir() {
            return Ok(tickets_dir);
        }

        if !dir.pop() {
            break;
        }
    }

    // Check root
    let root_tickets = Path::new("/.tickets");
    if root_tickets.is_dir() {
        return Ok(root_tickets.to_path_buf());
    }

    anyhow::bail!(
        "No .tickets directory found (searched parent directories).\n\
         Run 'ttr init' to create one, or set TICKETS_DIR env var."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_parts() {
        let config = GitHubConfig {
            repo: "owner/repo".to_string(),
            project: None,
            assignee: None,
        };
        let (owner, name) = config.repo_parts().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_repo_parts_invalid() {
        let config = GitHubConfig {
            repo: "invalid".to_string(),
            project: None,
            assignee: None,
        };
        assert!(config.repo_parts().is_err());
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[github]
repo = "owner/repo"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.github.repo, "owner/repo");
        assert!(config.github.project.is_none());
        assert!(config.labels.sync_tags);
        assert!(config.labels.create_missing);
        assert_eq!(config.mapping.type_field, "Type");
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[github]
repo = "myorg/myrepo"
project = "Q1 Sprint"
assignee = "acmyers"

[mapping]
type_field = "Issue Type"

[mapping.type]
bug = "Bug"
feature = "Feature"
task = "Task"

[labels]
sync_tags = true
create_missing = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.github.repo, "myorg/myrepo");
        assert_eq!(config.github.project, Some("Q1 Sprint".to_string()));
        assert_eq!(config.github.assignee, Some("acmyers".to_string()));
        assert_eq!(config.mapping.type_field, "Issue Type");
        assert_eq!(config.mapping.type_map.get("bug"), Some(&"Bug".to_string()));
        assert!(config.labels.sync_tags);
        assert!(!config.labels.create_missing);
    }
}
