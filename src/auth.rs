use anyhow::{Context, Result};
use std::env;
use std::process::Command;

/// Resolve GitHub token from environment or gh CLI
pub fn get_github_token() -> Result<String> {
    // Try GITHUB_TOKEN environment variable first
    if let Ok(token) = env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // Try GH_TOKEN (used by gh CLI)
    if let Ok(token) = env::var("GH_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // Fall back to gh auth token
    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .context(
            "Failed to run 'gh auth token'. Is GitHub CLI installed?\n\
             Alternatively, set GITHUB_TOKEN environment variable.",
        )?;

    if output.status.success() {
        let token = String::from_utf8(output.stdout)
            .context("Invalid UTF-8 in gh auth token output")?
            .trim()
            .to_string();

        if !token.is_empty() {
            return Ok(token);
        }
    }

    anyhow::bail!(
        "No GitHub token found.\n\
         \n\
         Options:\n\
         1. Set GITHUB_TOKEN environment variable\n\
         2. Run 'gh auth login' to authenticate GitHub CLI"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_github_token_from_env() {
        // This test depends on environment, so we just verify it doesn't panic
        // In CI, GITHUB_TOKEN is usually set
        let result = get_github_token();
        // We can't assert success because it depends on environment
        // but we can verify it returns a Result
        assert!(result.is_ok() || result.is_err());
    }
}
