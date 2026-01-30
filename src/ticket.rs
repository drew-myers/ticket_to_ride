use anyhow::{Context, Result};
use gray_matter::{engine::YAML, Matter};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Represents a parsed ticket from .tickets/*.md
#[derive(Debug, Clone)]
pub struct Ticket {
    /// Path to the ticket file
    pub path: PathBuf,
    /// Ticket ID (e.g., "ttr-0001")
    pub id: String,
    /// Status: open, in_progress, closed
    pub status: String,
    /// Dependencies (ticket IDs this depends on)
    pub deps: Vec<String>,
    /// Linked tickets (symmetric relationships)
    pub links: Vec<String>,
    /// Creation timestamp
    pub created: Option<String>,
    /// Type: bug, feature, task, epic, chore
    pub ticket_type: String,
    /// Priority 0-4 (0 = highest)
    pub priority: u8,
    /// Assignee name
    pub assignee: Option<String>,
    /// External reference (e.g., "gh-123")
    pub external_ref: Option<String>,
    /// Parent ticket ID
    pub parent: Option<String>,
    /// Tags for labeling
    pub tags: Vec<String>,
    /// Ticket title (from markdown heading)
    pub title: String,
    /// Full body content (excluding Notes section)
    pub body: String,
}

/// YAML frontmatter structure
#[derive(Debug, Deserialize)]
struct Frontmatter {
    id: String,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
    created: Option<String>,
    #[serde(rename = "type", default = "default_type")]
    ticket_type: String,
    #[serde(default = "default_priority")]
    priority: u8,
    assignee: Option<String>,
    #[serde(rename = "external-ref")]
    external_ref: Option<String>,
    parent: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_status() -> String {
    "open".to_string()
}

fn default_type() -> String {
    "task".to_string()
}

fn default_priority() -> u8 {
    2
}

impl Ticket {
    /// Parse a ticket from a markdown file
    pub fn parse(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read ticket: {}", path.display()))?;

        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(&content);

        let frontmatter: Frontmatter = parsed
            .data
            .ok_or_else(|| anyhow::anyhow!("No frontmatter found in {}", path.display()))?
            .deserialize()
            .with_context(|| format!("Failed to parse frontmatter in {}", path.display()))?;

        let body_content = parsed.content.trim();

        // Extract title from first # heading
        let title = body_content
            .lines()
            .find(|line| line.starts_with("# "))
            .map(|line| line.trim_start_matches("# ").to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        // Get body without the title line, and filter out Notes section
        let body = extract_body(body_content);

        Ok(Ticket {
            path: path.to_path_buf(),
            id: frontmatter.id,
            status: frontmatter.status,
            deps: frontmatter.deps,
            links: frontmatter.links,
            created: frontmatter.created,
            ticket_type: frontmatter.ticket_type,
            priority: frontmatter.priority,
            assignee: frontmatter.assignee,
            external_ref: frontmatter.external_ref,
            parent: frontmatter.parent,
            tags: frontmatter.tags,
            title,
            body,
        })
    }

    /// Load all tickets from a directory
    pub fn load_all(tickets_dir: &Path) -> Result<Vec<Self>> {
        let mut tickets = Vec::new();

        for entry in fs::read_dir(tickets_dir)
            .with_context(|| format!("Failed to read directory: {}", tickets_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "md") {
                // Skip sync.toml and other non-ticket files
                if let Some(name) = path.file_stem() {
                    if name == "sync" {
                        continue;
                    }
                }

                match Self::parse(&path) {
                    Ok(ticket) => tickets.push(ticket),
                    Err(e) => {
                        eprintln!("Warning: Failed to parse {}: {}", path.display(), e);
                    }
                }
            }
        }

        // Sort by ID for consistent ordering
        tickets.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(tickets)
    }

    /// Write or update the external-ref field in the ticket file
    pub fn write_external_ref(&mut self, external_ref: &str) -> Result<()> {
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("Failed to read ticket: {}", self.path.display()))?;

        // Check if external-ref exists in frontmatter (not in body)
        let has_external_ref_in_frontmatter = {
            let mut in_frontmatter = false;
            let mut found = false;
            for line in content.lines() {
                if line == "---" {
                    if in_frontmatter {
                        break; // End of frontmatter
                    } else {
                        in_frontmatter = true;
                        continue;
                    }
                }
                if in_frontmatter && line.starts_with("external-ref:") {
                    found = true;
                    break;
                }
            }
            found
        };

        let new_content = if has_external_ref_in_frontmatter {
            // Update existing external-ref in frontmatter only
            let mut in_frontmatter = false;
            let mut passed_frontmatter = false;
            content
                .lines()
                .map(|line| {
                    if line == "---" {
                        if in_frontmatter {
                            passed_frontmatter = true;
                        }
                        in_frontmatter = !in_frontmatter;
                        return line.to_string();
                    }
                    if in_frontmatter && !passed_frontmatter && line.starts_with("external-ref:") {
                        format!("external-ref: {}", external_ref)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            // Add external-ref before closing --- of frontmatter
            let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            let mut insert_idx = None;

            let mut in_frontmatter = false;
            for (i, line) in lines.iter().enumerate() {
                if line == "---" {
                    if in_frontmatter {
                        // End of frontmatter
                        insert_idx = Some(i);
                        break;
                    } else {
                        in_frontmatter = true;
                    }
                }
            }

            if let Some(idx) = insert_idx {
                lines.insert(idx, format!("external-ref: {}", external_ref));
            }

            lines.join("\n")
        };

        // Ensure file ends with newline
        let final_content = if new_content.ends_with('\n') {
            new_content
        } else {
            format!("{}\n", new_content)
        };

        fs::write(&self.path, final_content)
            .with_context(|| format!("Failed to write ticket: {}", self.path.display()))?;

        self.external_ref = Some(external_ref.to_string());
        Ok(())
    }

    /// Check if this ticket has been synced to GitHub
    pub fn is_synced(&self) -> bool {
        self.external_ref
            .as_ref()
            .is_some_and(|r| r.starts_with("gh-"))
    }

    /// Get the GitHub issue number if synced
    pub fn github_issue_number(&self) -> Option<u64> {
        self.external_ref.as_ref().and_then(|r| {
            r.strip_prefix("gh-")
                .and_then(|num| num.parse::<u64>().ok())
        })
    }
}

/// Extract body content, filtering out the Notes section
fn extract_body(content: &str) -> String {
    let mut result = Vec::new();
    let mut in_notes = false;

    for line in content.lines() {
        // Skip the title line
        if line.starts_with("# ") && result.is_empty() {
            continue;
        }

        // Check for Notes section
        if line.starts_with("## Notes") {
            in_notes = true;
            continue;
        }

        // Check if we've hit a new section after Notes
        if in_notes && line.starts_with("## ") {
            in_notes = false;
        }

        if !in_notes {
            result.push(line);
        }
    }

    // Trim leading/trailing empty lines
    let body = result.join("\n");
    body.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_ticket(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::with_suffix(".md").unwrap();
        write!(file, "{}", content).unwrap();
        file
    }

    #[test]
    fn test_parse_minimal_ticket() {
        let content = r#"---
id: test-001
---
# Test Ticket

This is a test.
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();

        assert_eq!(ticket.id, "test-001");
        assert_eq!(ticket.status, "open");
        assert_eq!(ticket.ticket_type, "task");
        assert_eq!(ticket.priority, 2);
        assert_eq!(ticket.title, "Test Ticket");
        assert!(ticket.body.contains("This is a test"));
    }

    #[test]
    fn test_parse_full_ticket() {
        let content = r#"---
id: ttr-0001
status: in_progress
deps: [ttr-0002, ttr-0003]
links: []
created: 2026-01-29T18:00:00Z
type: epic
priority: 0
assignee: acmyers
external-ref: gh-123
parent: parent-001
tags: [setup, core]
---
# Full Test Ticket

Description here.

## Design

Design notes.

## Acceptance Criteria

- [ ] Criterion 1

## Notes

**2026-01-29T12:00:00Z**

This note should not appear in body.
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();

        assert_eq!(ticket.id, "ttr-0001");
        assert_eq!(ticket.status, "in_progress");
        assert_eq!(ticket.deps, vec!["ttr-0002", "ttr-0003"]);
        assert_eq!(ticket.ticket_type, "epic");
        assert_eq!(ticket.priority, 0);
        assert_eq!(ticket.assignee, Some("acmyers".to_string()));
        assert_eq!(ticket.external_ref, Some("gh-123".to_string()));
        assert_eq!(ticket.parent, Some("parent-001".to_string()));
        assert_eq!(ticket.tags, vec!["setup", "core"]);
        assert_eq!(ticket.title, "Full Test Ticket");
        assert!(ticket.body.contains("Description here"));
        assert!(ticket.body.contains("Design notes"));
        assert!(ticket.body.contains("Criterion 1"));
        assert!(!ticket.body.contains("This note should not appear"));
    }

    #[test]
    fn test_is_synced() {
        let content = r#"---
id: test-001
external-ref: gh-456
---
# Test
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();

        assert!(ticket.is_synced());
        assert_eq!(ticket.github_issue_number(), Some(456));
    }

    #[test]
    fn test_not_synced() {
        let content = r#"---
id: test-001
---
# Test
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();

        assert!(!ticket.is_synced());
        assert_eq!(ticket.github_issue_number(), None);
    }

    #[test]
    fn test_write_external_ref_new() {
        let content = r#"---
id: test-001
status: open
tags: []
---
# Test
"#;
        let file = create_test_ticket(content);
        let mut ticket = Ticket::parse(file.path()).unwrap();

        ticket.write_external_ref("gh-789").unwrap();

        // Re-read and verify
        let updated = Ticket::parse(file.path()).unwrap();
        assert_eq!(updated.external_ref, Some("gh-789".to_string()));
    }

    #[test]
    fn test_write_external_ref_update() {
        let content = r#"---
id: test-001
external-ref: gh-123
---
# Test
"#;
        let file = create_test_ticket(content);
        let mut ticket = Ticket::parse(file.path()).unwrap();

        ticket.write_external_ref("gh-456").unwrap();

        let updated = Ticket::parse(file.path()).unwrap();
        assert_eq!(updated.external_ref, Some("gh-456".to_string()));
    }

    #[test]
    fn test_write_external_ref_with_code_block_example() {
        // This tests the bug where external-ref in a code block example
        // was being matched instead of adding to the real frontmatter
        let content = r#"---
id: test-001
status: open
tags: []
---
# Test Ticket

Here's an example ticket:

```yaml
---
id: example
external-ref: gh-999
---
```

More content here.
"#;
        let file = create_test_ticket(content);
        let mut ticket = Ticket::parse(file.path()).unwrap();

        // Should NOT be synced - the external-ref is in a code block, not frontmatter
        assert!(!ticket.is_synced());

        // Write external-ref should add to real frontmatter
        ticket.write_external_ref("gh-123").unwrap();

        let updated = Ticket::parse(file.path()).unwrap();
        assert_eq!(updated.external_ref, Some("gh-123".to_string()));
        assert!(updated.is_synced());

        // The code block example should still have its original value
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("external-ref: gh-999")); // in code block
        assert!(content.contains("external-ref: gh-123")); // in frontmatter
    }

    #[test]
    fn test_write_external_ref_with_body_mention() {
        // Test that mentioning external-ref in body text doesn't confuse the writer
        let content = r#"---
id: test-001
status: open
---
# Test Ticket

Make sure external-ref: gh-{number} is written back to ticket files.
"#;
        let file = create_test_ticket(content);
        let mut ticket = Ticket::parse(file.path()).unwrap();

        assert!(!ticket.is_synced());

        ticket.write_external_ref("gh-42").unwrap();

        let updated = Ticket::parse(file.path()).unwrap();
        assert_eq!(updated.external_ref, Some("gh-42".to_string()));
    }

    #[test]
    fn test_extract_body_filters_notes() {
        let content = r#"# Title

Description.

## Design

Design stuff.

## Notes

**2026-01-29**
Note 1

**2026-01-30**
Note 2
"#;
        let body = extract_body(content);
        assert!(body.contains("Description"));
        assert!(body.contains("Design stuff"));
        assert!(!body.contains("Note 1"));
        assert!(!body.contains("Note 2"));
    }

    #[test]
    fn test_extract_body_with_section_after_notes() {
        let content = r#"# Title

Intro.

## Notes

Some notes.

## References

This should be included.
"#;
        let body = extract_body(content);
        assert!(body.contains("Intro"));
        assert!(!body.contains("Some notes"));
        assert!(body.contains("This should be included"));
    }

    #[test]
    fn test_github_issue_number_parsing() {
        let content = r#"---
id: test-001
external-ref: gh-12345
---
# Test
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();
        assert_eq!(ticket.github_issue_number(), Some(12345));
    }

    #[test]
    fn test_github_issue_number_invalid() {
        let content = r#"---
id: test-001
external-ref: jira-123
---
# Test
"#;
        let file = create_test_ticket(content);
        let ticket = Ticket::parse(file.path()).unwrap();
        // Not a gh- ref, so not synced to GitHub
        assert!(!ticket.is_synced());
        assert_eq!(ticket.github_issue_number(), None);
    }
}
