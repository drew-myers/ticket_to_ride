use crate::config::Config;
use crate::github::client::GitHubClient;
use crate::github::issues::{ExistingIssue, IssueCreate, IssueUpdate};
use crate::github::subissues::SubIssueLink;
use crate::ticket::Ticket;
use anyhow::Result;
use std::collections::HashMap;

/// Result of syncing a single ticket
#[derive(Debug, Clone)]
pub enum SyncResult {
    Created { issue_id: String, issue_number: u64, url: String },
    Updated { issue_number: u64 },
    Skipped { reason: String },
    Failed { error: String },
}

/// Pending create for batch processing
struct PendingCreate {
    ticket_idx: usize,
    title: String,
    body: String,
    label_ids: Vec<String>,
}

/// Pending update for batch processing
struct PendingUpdate {
    ticket_idx: usize,
    issue_id: String,
    issue_number: u64,
    title: String,
    body: String,
    needs_close: bool,
    needs_reopen: bool,
}

/// Result of checking if an update is needed
enum UpdateCheck {
    NoChanges,
    Conflict(String),
    Error(String),
    NeedsUpdate {
        issue_id: String,
        issue_number: u64,
        title: String,
        body: String,
        needs_close: bool,
        needs_reopen: bool,
    },
}

/// Summary of a sync operation
#[derive(Debug, Default)]
pub struct SyncSummary {
    pub created: u32,
    pub updated: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Orchestrates syncing tickets to GitHub
pub struct SyncEngine {
    client: GitHubClient,
    config: Config,
    repo_id: String,
    owner: String,
    repo_name: String,
    assignee_id: Option<String>,
    label_cache: HashMap<String, String>,  // label name -> label ID
    ticket_to_issue: HashMap<String, u64>, // ticket ID -> GitHub issue number
}

impl SyncEngine {
    /// Create a new sync engine
    pub async fn new(client: GitHubClient, config: Config) -> Result<Self> {
        let (owner, repo_name) = config.github.repo_parts()?;
        let owner = owner.to_string();
        let repo_name = repo_name.to_string();

        // Get repository ID
        let repo_id = client.get_repository_id(&owner, &repo_name).await?;

        // Get assignee ID if configured
        let assignee_id = if let Some(ref username) = config.github.assignee {
            Some(client.get_user_id(username).await?)
        } else {
            None
        };

        // Pre-fetch labels
        let labels = client.get_labels(&owner, &repo_name).await?;
        let label_cache: HashMap<String, String> = labels
            .into_iter()
            .map(|l| (l.name.to_lowercase(), l.id))
            .collect();

        Ok(Self {
            client,
            config,
            repo_id,
            owner,
            repo_name,
            assignee_id,
            label_cache,
            ticket_to_issue: HashMap::new(), // Will be populated during sync
        })
    }

    /// Sync a list of tickets
    /// 
    /// `tickets` are the tickets to sync, `all_tickets` is used to build the
    /// dependency lookup (for rendering "Depends on" references).
    pub async fn sync(&mut self, tickets: &mut [Ticket], all_tickets: &[Ticket]) -> Result<SyncSummary> {
        let mut summary = SyncSummary::default();
        let mut results: Vec<(usize, SyncResult)> = Vec::new();

        // Build ticket ID → issue number lookup for dependency resolution
        // Use all_tickets so deps resolve even when pushing a subset
        self.ticket_to_issue = all_tickets
            .iter()
            .filter_map(|t| t.github_issue_number().map(|n| (t.id.clone(), n)))
            .collect();

        // Batch fetch all existing issues upfront
        // Include both tickets being synced AND their parents (for sub-issue linking)
        let mut issue_numbers: Vec<u64> = tickets
            .iter()
            .filter_map(|t| t.github_issue_number())
            .collect();

        // Also fetch parent issues (need their node IDs for sub-issue linking)
        for ticket in tickets.iter() {
            if let Some(ref parent_id) = ticket.parent {
                if let Some(parent_num) = self.ticket_to_issue.get(parent_id) {
                    if !issue_numbers.contains(parent_num) {
                        issue_numbers.push(*parent_num);
                    }
                }
            }
        }

        let existing_issues = if !issue_numbers.is_empty() {
            self.client
                .get_issues_batch(&self.owner, &self.repo_name, &issue_numbers)
                .await
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Phase 1: Categorize tickets
        let mut pending_creates: Vec<PendingCreate> = Vec::new();
        let mut pending_updates: Vec<PendingUpdate> = Vec::new();

        for (idx, ticket) in tickets.iter().enumerate() {
            if ticket.is_synced() {
                // Check if update is needed
                match self.check_update_needed(ticket, &existing_issues) {
                    UpdateCheck::NoChanges => {
                        results.push((idx, SyncResult::Skipped { reason: "no changes".to_string() }));
                    }
                    UpdateCheck::Conflict(reason) => {
                        results.push((idx, SyncResult::Skipped { reason }));
                    }
                    UpdateCheck::Error(e) => {
                        results.push((idx, SyncResult::Failed { error: e }));
                    }
                    UpdateCheck::NeedsUpdate { issue_id, issue_number, title, body, needs_close, needs_reopen } => {
                        pending_updates.push(PendingUpdate {
                            ticket_idx: idx,
                            issue_id,
                            issue_number,
                            title,
                            body,
                            needs_close,
                            needs_reopen,
                        });
                    }
                }
            } else {
                // Collect creates for batching
                let label_ids = self.resolve_label_ids(&ticket.tags).await;
                pending_creates.push(PendingCreate {
                    ticket_idx: idx,
                    title: ticket.title.clone(),
                    body: self.format_issue_body(ticket),
                    label_ids,
                });
            }
        }

        // Phase 2: Batch create issues
        if !pending_creates.is_empty() {
            let create_results = self.batch_create(&pending_creates).await;
            for (pending, result) in pending_creates.iter().zip(create_results) {
                // Write external-ref back to ticket file on success
                if let SyncResult::Created { issue_number, .. } = &result {
                    let ticket = &mut tickets[pending.ticket_idx];
                    let external_ref = format!("gh-{}", issue_number);
                    if let Err(e) = ticket.write_external_ref(&external_ref) {
                        results.push((pending.ticket_idx, SyncResult::Failed {
                            error: format!("Created #{} but failed to write external-ref: {}", issue_number, e),
                        }));
                        continue;
                    }
                }
                results.push((pending.ticket_idx, result));
            }
        }

        // Phase 3: Batch update issues
        if !pending_updates.is_empty() {
            let update_results = self.batch_update(&pending_updates).await;
            for (pending, result) in pending_updates.iter().zip(update_results) {
                results.push((pending.ticket_idx, result));
            }
        }

        // Sort by original index and print results
        results.sort_by_key(|(idx, _)| *idx);

        for (idx, result) in &results {
            let ticket = &tickets[*idx];
            match result {
                SyncResult::Created { issue_number, url, .. } => {
                    println!(
                        "CREATE  {} → #{}  {}",
                        ticket.id, issue_number, ticket.title
                    );
                    println!("  └─ {}", url);
                    summary.created += 1;
                }
                SyncResult::Updated { issue_number } => {
                    println!(
                        "UPDATE  {} → #{}  {}",
                        ticket.id, issue_number, ticket.title
                    );
                    summary.updated += 1;
                }
                SyncResult::Skipped { reason } => {
                    println!("SKIP    {}  ({})", ticket.id, reason);
                    summary.skipped += 1;
                }
                SyncResult::Failed { error } => {
                    println!("FAIL    {}  {}", ticket.id, error);
                    summary.failed += 1;
                }
            }
        }

        // Phase 4: Link sub-issues (parent/child relationships)
        self.link_sub_issues(tickets, all_tickets, &results, &existing_issues).await;

        Ok(summary)
    }

    /// Check if a ticket needs updating, returns update details if so
    fn check_update_needed(
        &self,
        ticket: &Ticket,
        existing_issues: &HashMap<u64, ExistingIssue>,
    ) -> UpdateCheck {
        let issue_number = match ticket.github_issue_number() {
            Some(n) => n,
            None => return UpdateCheck::Conflict("invalid external-ref".to_string()),
        };

        let existing = match existing_issues.get(&issue_number) {
            Some(issue) => issue,
            None => return UpdateCheck::Error(format!("Issue #{} not found", issue_number)),
        };

        // Check for our marker
        let marker = format!("<!-- ticket:{} -->", ticket.id);
        if !existing.body.contains(&marker) {
            return UpdateCheck::Conflict("issue modified outside ttr".to_string());
        }

        // Format new body
        let new_body = self.format_issue_body(ticket);

        // Check if update is needed
        let title_changed = existing.title != ticket.title;
        let body_changed = existing.body != new_body;
        let state_should_be_closed = ticket.status == "closed";
        let state_is_closed = existing.state == "CLOSED";
        let state_changed = state_should_be_closed != state_is_closed;

        if !title_changed && !body_changed && !state_changed {
            return UpdateCheck::NoChanges;
        }

        UpdateCheck::NeedsUpdate {
            issue_id: existing.id.clone(),
            issue_number,
            title: ticket.title.clone(),
            body: new_body,
            needs_close: state_changed && state_should_be_closed,
            needs_reopen: state_changed && !state_should_be_closed,
        }
    }

    /// Batch update multiple issues
    async fn batch_update(&self, pending: &[PendingUpdate]) -> Vec<SyncResult> {
        let mut results = vec![SyncResult::Failed { error: "Not processed".to_string() }; pending.len()];

        // Collect content updates (title/body changes)
        let updates: Vec<IssueUpdate> = pending
            .iter()
            .map(|p| IssueUpdate {
                issue_id: p.issue_id.clone(),
                title: p.title.clone(),
                body: p.body.clone(),
            })
            .collect();

        // Batch update content
        if !updates.is_empty() {
            match self.client.update_issues_batch(&updates).await {
                Ok(update_results) => {
                    for (i, p) in pending.iter().enumerate() {
                        if let Some(result) = update_results.get(&p.issue_id) {
                            match result {
                                Ok(_) => {
                                    results[i] = SyncResult::Updated { issue_number: p.issue_number };
                                }
                                Err(e) => {
                                    results[i] = SyncResult::Failed { error: e.clone() };
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // All updates failed
                    for i in 0..pending.len() {
                        results[i] = SyncResult::Failed { error: e.to_string() };
                    }
                    return results;
                }
            }
        }

        // Batch close issues
        let to_close: Vec<String> = pending
            .iter()
            .filter(|p| p.needs_close)
            .map(|p| p.issue_id.clone())
            .collect();

        if !to_close.is_empty() {
            if let Err(e) = self.client.close_issues_batch(&to_close).await {
                // Mark close failures
                for (i, p) in pending.iter().enumerate() {
                    if p.needs_close {
                        results[i] = SyncResult::Failed {
                            error: format!("Failed to close: {}", e),
                        };
                    }
                }
            }
        }

        // Batch reopen issues
        let to_reopen: Vec<String> = pending
            .iter()
            .filter(|p| p.needs_reopen)
            .map(|p| p.issue_id.clone())
            .collect();

        if !to_reopen.is_empty() {
            if let Err(e) = self.client.reopen_issues_batch(&to_reopen).await {
                // Mark reopen failures
                for (i, p) in pending.iter().enumerate() {
                    if p.needs_reopen {
                        results[i] = SyncResult::Failed {
                            error: format!("Failed to reopen: {}", e),
                        };
                    }
                }
            }
        }

        results
    }

    /// Batch create multiple issues
    async fn batch_create(&self, pending: &[PendingCreate]) -> Vec<SyncResult> {
        if pending.is_empty() {
            return Vec::new();
        }

        let creates: Vec<IssueCreate> = pending
            .iter()
            .map(|p| IssueCreate {
                title: p.title.clone(),
                body: p.body.clone(),
                label_ids: p.label_ids.clone(),
            })
            .collect();

        let assignee_ids: Option<Vec<String>> = self.assignee_id.clone().map(|id| vec![id]);
        let assignee_slice = assignee_ids.as_deref();

        match self.client.create_issues_batch(&self.repo_id, &creates, assignee_slice).await {
            Ok(create_results) => {
                create_results
                    .into_iter()
                    .map(|result| match result {
                        Ok(info) => SyncResult::Created {
                            issue_id: info.id,
                            issue_number: info.number,
                            url: info.url,
                        },
                        Err(e) => SyncResult::Failed { error: e },
                    })
                    .collect()
            }
            Err(e) => {
                // All creates failed
                vec![SyncResult::Failed { error: e.to_string() }; pending.len()]
            }
        }
    }

    /// Resolve tag names to label IDs, creating labels if needed
    async fn resolve_label_ids(&mut self, tags: &[String]) -> Vec<String> {
        if !self.config.labels.sync_tags {
            return Vec::new();
        }

        let mut label_ids = Vec::new();

        for tag in tags {
            let tag_lower = tag.to_lowercase();

            // Check cache first
            if let Some(id) = self.label_cache.get(&tag_lower) {
                label_ids.push(id.clone());
                continue;
            }

            // Try to get or create the label
            if let Ok(Some(id)) = self
                .client
                .get_or_create_label(
                    &self.owner,
                    &self.repo_name,
                    &self.repo_id,
                    tag,
                    self.config.labels.create_missing,
                )
                .await
            {
                self.label_cache.insert(tag_lower, id.clone());
                label_ids.push(id);
            }
        }

        label_ids
    }

    /// Format the issue body with marker, content, and dependencies
    fn format_issue_body(&self, ticket: &Ticket) -> String {
        format_issue_body_with_deps(&ticket.id, &ticket.body, &ticket.deps, &self.ticket_to_issue)
    }

    /// Link sub-issues based on ticket parent relationships
    /// 
    /// This runs after all creates/updates, using a two-pass approach:
    /// 1. Build a map of ticket_id → issue_node_id from existing issues and newly created ones
    /// 2. For each ticket with a parent, link child to parent as a sub-issue
    async fn link_sub_issues(
        &self,
        tickets: &[Ticket],
        all_tickets: &[Ticket],
        results: &[(usize, SyncResult)],
        existing_issues: &HashMap<u64, ExistingIssue>,
    ) {
        // Build ticket_id → issue_node_id map
        let mut ticket_to_node_id: HashMap<String, String> = HashMap::new();

        // Add from existing issues (looked up at start of sync)
        for ticket in all_tickets {
            if let Some(issue_num) = ticket.github_issue_number() {
                if let Some(existing) = existing_issues.get(&issue_num) {
                    ticket_to_node_id.insert(ticket.id.clone(), existing.id.clone());
                }
            }
        }

        // Add from newly created issues in this sync
        for (idx, result) in results {
            if let SyncResult::Created { issue_id, .. } = result {
                let ticket = &tickets[*idx];
                ticket_to_node_id.insert(ticket.id.clone(), issue_id.clone());
            }
        }

        // Collect sub-issue links to create
        let mut links: Vec<(String, SubIssueLink)> = Vec::new(); // (child_ticket_id, link)

        for ticket in tickets {
            if let Some(ref parent_id) = ticket.parent {
                // Look up both parent and child node IDs
                let parent_node_id = ticket_to_node_id.get(parent_id);
                let child_node_id = ticket_to_node_id.get(&ticket.id);

                match (parent_node_id, child_node_id) {
                    (Some(parent_id), Some(child_id)) => {
                        links.push((
                            ticket.id.clone(),
                            SubIssueLink {
                                parent_issue_id: parent_id.clone(),
                                child_issue_id: child_id.clone(),
                            },
                        ));
                    }
                    (None, _) => {
                        // Parent not synced - skip silently, it will link on next push
                    }
                    (_, None) => {
                        // Child not synced - shouldn't happen since we just synced it
                    }
                }
            }
        }

        if links.is_empty() {
            return;
        }

        // Batch link sub-issues (single GraphQL mutation)
        let sub_issue_links: Vec<SubIssueLink> = links.iter().map(|(_, link)| link.clone()).collect();
        
        match self.client.add_sub_issues_batch(&sub_issue_links).await {
            Ok(results) => {
                println!();
                for ((child_id, link), result) in links.iter().zip(results) {
                    // Find parent ticket ID for display
                    let parent_ticket_id = all_tickets
                        .iter()
                        .find(|t| ticket_to_node_id.get(&t.id) == Some(&link.parent_issue_id))
                        .map(|t| t.id.as_str())
                        .unwrap_or("?");

                    match result {
                        Ok(()) => {
                            println!("LINK    {} → {} (sub-issue)", child_id, parent_ticket_id);
                        }
                        Err(e) => {
                            eprintln!("WARN    {} sub-issue link failed: {}", child_id, e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("\nWARN    sub-issue batch link failed: {}", e);
            }
        }
    }
}

/// Format the issue body with marker and content (public for testing)
pub fn format_issue_body(ticket_id: &str, ticket_body: &str) -> String {
    format_issue_body_with_deps(ticket_id, ticket_body, &[], &HashMap::new())
}

/// Format the issue body with marker, content, and dependency references
pub fn format_issue_body_with_deps(
    ticket_id: &str,
    ticket_body: &str,
    deps: &[String],
    ticket_to_issue: &HashMap<String, u64>,
) -> String {
    let mut body = format!("<!-- ticket:{} -->\n\n", ticket_id);
    body.push_str(ticket_body);

    // Add dependencies section if there are any
    if !deps.is_empty() {
        body.push_str("\n\n---\n");
        body.push_str(&format_dependencies_section(deps, ticket_to_issue));
    }

    body.push_str("\n\n---\n");
    body.push_str(&format!("<sub>Synced from ticket `{}`</sub>", ticket_id));
    body
}

/// Format the dependencies section for the issue body
fn format_dependencies_section(deps: &[String], ticket_to_issue: &HashMap<String, u64>) -> String {
    let refs: Vec<String> = deps
        .iter()
        .map(|dep_id| {
            if let Some(issue_num) = ticket_to_issue.get(dep_id) {
                format!("#{}", issue_num)
            } else {
                format!("`{}` (not synced)", dep_id)
            }
        })
        .collect();

    format!("**Depends on:** {}", refs.join(", "))
}

/// Extract ticket ID from issue body marker
pub fn extract_ticket_marker(body: &str) -> Option<&str> {
    let start = body.find("<!-- ticket:")?;
    let after_start = &body[start + 12..];
    let end = after_start.find(" -->")?;
    Some(&after_start[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_issue_body() {
        let body = format_issue_body("ttr-0001", "This is the description.\n\n## Design\n\nSome design notes.");
        
        assert!(body.starts_with("<!-- ticket:ttr-0001 -->"));
        assert!(body.contains("This is the description."));
        assert!(body.contains("## Design"));
        assert!(body.contains("Some design notes."));
        assert!(body.contains("<sub>Synced from ticket `ttr-0001`</sub>"));
    }

    #[test]
    fn test_format_issue_body_marker_at_start() {
        let body = format_issue_body("test-123", "Content");
        
        // Marker must be at the very start for conflict detection
        assert!(body.starts_with("<!-- ticket:test-123 -->"));
    }

    #[test]
    fn test_extract_ticket_marker() {
        let body = "<!-- ticket:ttr-0001 -->\n\nSome content";
        assert_eq!(extract_ticket_marker(body), Some("ttr-0001"));
    }

    #[test]
    fn test_extract_ticket_marker_missing() {
        let body = "Some content without marker";
        assert_eq!(extract_ticket_marker(body), None);
    }

    #[test]
    fn test_extract_ticket_marker_roundtrip() {
        let original_id = "my-ticket-42";
        let body = format_issue_body(original_id, "Content here");
        let extracted = extract_ticket_marker(&body);
        assert_eq!(extracted, Some(original_id));
    }

    #[test]
    fn test_format_issue_body_with_deps_all_synced() {
        let mut lookup = HashMap::new();
        lookup.insert("ttr-0002".to_string(), 45);
        lookup.insert("ttr-0003".to_string(), 67);

        let deps = vec!["ttr-0002".to_string(), "ttr-0003".to_string()];
        let body = format_issue_body_with_deps("ttr-0001", "Description", &deps, &lookup);

        assert!(body.contains("**Depends on:** #45, #67"));
        assert!(body.contains("<sub>Synced from ticket `ttr-0001`</sub>"));
    }

    #[test]
    fn test_format_issue_body_with_deps_none_synced() {
        let lookup = HashMap::new();
        let deps = vec!["ttr-0002".to_string(), "ttr-0003".to_string()];
        let body = format_issue_body_with_deps("ttr-0001", "Description", &deps, &lookup);

        assert!(body.contains("**Depends on:** `ttr-0002` (not synced), `ttr-0003` (not synced)"));
    }

    #[test]
    fn test_format_issue_body_with_deps_mixed() {
        let mut lookup = HashMap::new();
        lookup.insert("ttr-0002".to_string(), 45);
        // ttr-0003 not in lookup (not synced)

        let deps = vec!["ttr-0002".to_string(), "ttr-0003".to_string()];
        let body = format_issue_body_with_deps("ttr-0001", "Description", &deps, &lookup);

        assert!(body.contains("**Depends on:** #45, `ttr-0003` (not synced)"));
    }

    #[test]
    fn test_format_issue_body_with_no_deps() {
        let lookup = HashMap::new();
        let deps: Vec<String> = vec![];
        let body = format_issue_body_with_deps("ttr-0001", "Description", &deps, &lookup);

        // Should not contain "Depends on" section
        assert!(!body.contains("Depends on"));
        // But still has the footer
        assert!(body.contains("<sub>Synced from ticket `ttr-0001`</sub>"));
    }

    #[test]
    fn test_format_dependencies_section() {
        let mut lookup = HashMap::new();
        lookup.insert("dep-1".to_string(), 10);
        lookup.insert("dep-2".to_string(), 20);

        let deps = vec!["dep-1".to_string(), "dep-2".to_string(), "dep-3".to_string()];
        let section = format_dependencies_section(&deps, &lookup);

        assert_eq!(section, "**Depends on:** #10, #20, `dep-3` (not synced)");
    }
}
