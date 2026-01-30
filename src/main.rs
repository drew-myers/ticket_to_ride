use anyhow::Result;
use clap::{Parser, Subcommand};
use ticket_to_ride::{auth, config::Config, github::client::GitHubClient, sync::SyncEngine, ticket::Ticket};

#[derive(Parser)]
#[command(name = "ttr")]
#[command(about = "Sync tickets to GitHub Issues", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync tickets to GitHub Issues
    Push {
        /// Specific ticket IDs to sync (syncs all if omitted)
        ids: Vec<String>,
    },
    /// Show sync status of tickets
    Status {
        /// Quick mode: skip GitHub fetch, just show local state
        #[arg(short, long)]
        quick: bool,
    },
    /// Create .tickets/sync.toml configuration
    Init {
        /// GitHub repository (owner/repo)
        #[arg(short, long)]
        repo: Option<String>,
        /// GitHub Project name
        #[arg(short, long)]
        project: Option<String>,
        /// Default assignee username
        #[arg(short, long)]
        assignee: Option<String>,
        /// Overwrite existing config
        #[arg(short, long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Push { ids } => cmd_push(ids).await,
        Commands::Status { quick } => cmd_status(quick).await,
        Commands::Init { repo, project, assignee, force } => cmd_init(repo, project, assignee, force),
    }
}

async fn cmd_push(ids: Vec<String>) -> Result<()> {
    // Load config
    let (config, tickets_dir) = Config::load()?;

    // Get auth token
    let token = auth::get_github_token()?;

    // Create GitHub client
    let client = GitHubClient::new(token)?;

    // Load tickets
    let mut tickets = Ticket::load_all(&tickets_dir)?;

    if tickets.is_empty() {
        println!("No tickets found in {}", tickets_dir.display());
        return Ok(());
    }

    // Filter to specific IDs if provided
    if !ids.is_empty() {
        tickets.retain(|t| {
            ids.iter().any(|id| t.id == *id || t.id.contains(id))
        });

        if tickets.is_empty() {
            println!("No tickets matched the provided IDs: {:?}", ids);
            return Ok(());
        }
    }

    println!("Syncing {} ticket(s) to {}...\n", tickets.len(), config.github.repo);

    // Create sync engine and run
    let mut engine = SyncEngine::new(client, config).await?;
    let summary = engine.sync(&mut tickets).await?;

    // Print summary
    println!();
    println!(
        "Summary: {} created, {} updated, {} skipped, {} failed",
        summary.created, summary.updated, summary.skipped, summary.failed
    );

    if summary.failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Try to detect GitHub repo from git remote origin
fn detect_github_repo() -> Option<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let url = String::from_utf8(output.stdout).ok()?;
    let url = url.trim();

    // Parse various GitHub URL formats:
    // git@github.com:owner/repo.git
    // https://github.com/owner/repo.git
    // https://github.com/owner/repo

    if url.contains("github.com") {
        // SSH format: git@github.com:owner/repo.git
        if let Some(rest) = url.strip_prefix("git@github.com:") {
            let repo = rest.trim_end_matches(".git");
            return Some(repo.to_string());
        }

        // HTTPS format: https://github.com/owner/repo.git
        if let Some(rest) = url.strip_prefix("https://github.com/") {
            let repo = rest.trim_end_matches(".git");
            return Some(repo.to_string());
        }
    }

    None
}

async fn cmd_status(quick: bool) -> Result<()> {
    use ticket_to_ride::sync::format_issue_body;

    // Load config
    let (config, tickets_dir) = Config::load()?;

    // Load tickets
    let tickets = Ticket::load_all(&tickets_dir)?;

    if tickets.is_empty() {
        println!("No tickets found in {}", tickets_dir.display());
        return Ok(());
    }

    // Categorize tickets
    let mut unsynced: Vec<&Ticket> = Vec::new();
    let mut synced: Vec<&Ticket> = Vec::new();
    let mut modified: Vec<(&Ticket, &str)> = Vec::new();
    let mut conflicts: Vec<&Ticket> = Vec::new();

    // Split into synced/unsynced first
    for ticket in &tickets {
        if ticket.is_synced() {
            synced.push(ticket);
        } else {
            unsynced.push(ticket);
        }
    }

    // If quick mode or no synced tickets, skip GitHub fetch
    if !quick && !synced.is_empty() {
        // Get auth token and create client
        let token = auth::get_github_token()?;
        let client = GitHubClient::new(token)?;
        let (owner, repo_name) = config.github.repo_parts()?;

        // Batch fetch all synced issues
        let issue_numbers: Vec<u64> = synced
            .iter()
            .filter_map(|t| t.github_issue_number())
            .collect();

        let existing_issues = client
            .get_issues_batch(owner, repo_name, &issue_numbers)
            .await
            .unwrap_or_default();

        // Re-categorize synced tickets based on GitHub state
        let mut still_synced: Vec<&Ticket> = Vec::new();

        for ticket in synced {
            let issue_number = match ticket.github_issue_number() {
                Some(n) => n,
                None => {
                    conflicts.push(ticket);
                    continue;
                }
            };

            let existing = match existing_issues.get(&issue_number) {
                Some(issue) => issue,
                None => {
                    // Issue not found on GitHub
                    conflicts.push(ticket);
                    continue;
                }
            };

            // Check for our marker
            let marker = format!("<!-- ticket:{} -->", ticket.id);
            if !existing.body.contains(&marker) {
                conflicts.push(ticket);
                continue;
            }

            // Check if content matches
            let expected_body = format_issue_body(&ticket.id, &ticket.body);
            let title_changed = existing.title != ticket.title;
            let body_changed = existing.body != expected_body;
            let state_should_be_closed = ticket.status == "closed";
            let state_is_closed = existing.state == "CLOSED";
            let state_changed = state_should_be_closed != state_is_closed;

            if title_changed || body_changed || state_changed {
                let reason = if title_changed {
                    "title changed"
                } else if body_changed {
                    "body changed"
                } else {
                    "state changed"
                };
                modified.push((ticket, reason));
            } else {
                still_synced.push(ticket);
            }
        }

        synced = still_synced;
    }

    // Print results
    println!("Repository: {}", config.github.repo);
    if quick {
        println!("(quick mode - GitHub state not checked)");
    }
    println!();
    println!("Tickets: {} total", tickets.len());
    println!("  Unsynced:  {:>3}  (will create new issues)", unsynced.len());
    println!("  Synced:    {:>3}  (up to date)", synced.len());
    if !quick {
        println!("  Modified:  {:>3}  (will update)", modified.len());
        println!("  Conflicts: {:>3}  (modified outside ttr)", conflicts.len());
    }

    if !unsynced.is_empty() {
        println!();
        println!("Unsynced:");
        for ticket in &unsynced {
            println!(
                "  {:<12} [{}]  {}",
                ticket.id, ticket.ticket_type, ticket.title
            );
        }
    }

    if !modified.is_empty() {
        println!();
        println!("Modified:");
        for (ticket, reason) in &modified {
            let issue_num = ticket.github_issue_number().unwrap_or(0);
            println!(
                "  {:<12} → #{:<5}  {} ({})",
                ticket.id, issue_num, ticket.title, reason
            );
        }
    }

    if !conflicts.is_empty() {
        println!();
        println!("Conflicts:");
        for ticket in &conflicts {
            let issue_num = ticket.github_issue_number().unwrap_or(0);
            println!(
                "  {:<12} → #{:<5}  {}",
                ticket.id, issue_num, ticket.title
            );
        }
    }

    if !synced.is_empty() && (unsynced.is_empty() || quick) {
        println!();
        println!("Synced:");
        for ticket in &synced {
            let issue_num = ticket.github_issue_number().unwrap_or(0);
            println!(
                "  {:<12} → #{:<5}  {}",
                ticket.id, issue_num, ticket.title
            );
        }
    }

    Ok(())
}

fn cmd_init(
    repo: Option<String>,
    project: Option<String>,
    assignee: Option<String>,
    force: bool,
) -> Result<()> {
    use std::fs;
    use std::io::{self, BufRead, Write};
    use std::path::Path;

    let tickets_dir = Path::new(".tickets");
    let config_path = tickets_dir.join("sync.toml");

    if config_path.exists() && !force {
        anyhow::bail!(
            "Configuration already exists: {}\nUse --force to overwrite.",
            config_path.display()
        );
    }

    // Create .tickets directory if needed
    if !tickets_dir.exists() {
        fs::create_dir(tickets_dir)?;
        println!("Created {}/", tickets_dir.display());
    }

    // Determine repo - from flag, git remote, or prompt
    let repo = if let Some(r) = repo {
        r
    } else if let Some(r) = detect_github_repo() {
        println!("Detected repository: {}", r);
        r
    } else {
        // Interactive prompt
        print!("GitHub repository (owner/repo): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() {
            anyhow::bail!("Repository is required");
        }
        input.to_string()
    };

    // Validate repo format
    if !repo.contains('/') || repo.split('/').count() != 2 {
        anyhow::bail!("Invalid repository format. Expected 'owner/repo'");
    }

    // Determine project - from flag or prompt
    let project = if let Some(p) = project {
        Some(p)
    } else if atty::is(atty::Stream::Stdin) {
        print!("GitHub Project name (optional, press Enter to skip): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() { None } else { Some(input.to_string()) }
    } else {
        None
    };

    // Determine assignee - from flag or prompt
    let assignee = if let Some(a) = assignee {
        Some(a)
    } else if atty::is(atty::Stream::Stdin) {
        print!("Default assignee (optional, press Enter to skip): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() { None } else { Some(input.to_string()) }
    } else {
        None
    };

    // Build config
    let mut config = format!(
        r#"[github]
repo = "{}"
"#,
        repo
    );

    if let Some(p) = &project {
        config.push_str(&format!("project = \"{}\"\n", p));
    } else {
        config.push_str("# project = \"Project Name\"  # Optional: GitHub Project to add issues to\n");
    }

    if let Some(a) = &assignee {
        config.push_str(&format!("assignee = \"{}\"\n", a));
    } else {
        config.push_str("# assignee = \"username\"  # Optional: assign all issues to this user\n");
    }

    config.push_str(
        r#"
[mapping]
type_field = "Type"  # Project field name for ticket type

[mapping.type]
bug = "Bug"
feature = "Feature"
task = "Task"
epic = "Epic"
chore = "Chore"

[labels]
sync_tags = true  # Sync ticket tags as GitHub labels
create_missing = true  # Create labels that don't exist
"#,
    );

    fs::write(&config_path, config)?;
    println!();
    println!("Created {}", config_path.display());
    println!();
    println!("Next steps:");
    if project.is_none() || assignee.is_none() {
        println!("  1. Edit {} to customize settings", config_path.display());
        println!("  2. Run 'ttr push' to sync tickets");
    } else {
        println!("  1. Run 'ttr push' to sync tickets");
    }

    Ok(())
}
