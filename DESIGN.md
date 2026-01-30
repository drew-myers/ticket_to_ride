# ttr (Ticket To Ride) - Design Document

A CLI utility that syncs tickets from the [wedow/ticket](https://github.com/wedow/ticket) system into GitHub Issues, with support for GitHub Projects and sub-issues.

## Problem Statement

The `ticket` system provides a git-native, markdown-based ticket tracking system that works excellently with AI coding agents. However, many teams need visibility into work progress through GitHub Issues and Projects for stakeholders and project management.

`ttr` bridges this gap by syncing tickets to GitHub while preserving the ticket system as the source of truth for developers and agents.

## Goals

1. **One-way sync**: Tickets → GitHub Issues (ticket system is source of truth)
2. **Non-destructive**: Warn and skip on conflicts rather than overwriting manual edits
3. **Project integration**: Add issues to GitHub Projects with proper field mapping
4. **Relationship preservation**: Sync parent/child as sub-issues, dependencies as references
5. **Simple workflow**: `ttr push` after creating tickets with an agent

## Non-Goals (For Now)

- Bidirectional sync (pulling changes from GitHub back to tickets)
- Merging conflicts between ticket and issue edits
- Per-ticket assignee mapping (all issues assigned to configured user)
- Syncing ticket Notes section to GitHub

## Ticket System Overview

Tickets are markdown files with YAML frontmatter stored in `.tickets/`:

```yaml
---
id: nw-5c46
status: open                    # open | in_progress | closed
deps: [dep-1234, dep-5678]      # ticket depends on these
links: [related-abc]            # symmetric relationships
created: 2026-01-29T12:00:00Z
type: task                      # bug | feature | task | epic | chore
priority: 2                     # 0-4, 0=highest
assignee: John Doe
external-ref: gh-123            # ← Used to track synced issue number
parent: parent-ticket-id
tags: [ui, backend, urgent]
---
# Ticket Title

Description here...

## Design

Design notes...

## Acceptance Criteria

- [ ] Criterion 1
- [ ] Criterion 2
```

Key fields for sync:
- `external-ref`: Stores `gh-{issue_number}` after sync
- `parent`: Maps to GitHub sub-issues
- `deps`: Rendered as "Depends on #X, #Y" in issue body
- `tags`: Synced as GitHub labels
- `type`: Maps to GitHub Project "Type" field
- `status`: Maps to GitHub issue open/closed state

## Architecture

```
ttr/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point (clap)
│   ├── lib.rs               # Library root
│   ├── config.rs            # Parse .tickets/sync.toml
│   ├── ticket.rs            # Parse ticket markdown files
│   ├── auth.rs              # Token resolution
│   ├── sync.rs              # Core sync orchestration
│   └── github/
│       ├── mod.rs
│       ├── client.rs        # GraphQL client wrapper
│       ├── issues.rs        # Create/update issues, labels
│       ├── projects.rs      # Project field queries & updates
│       └── subissues.rs     # addSubIssue mutation
```

## Configuration

File: `.tickets/sync.toml`

```toml
[github]
repo = "owner/repo"              # Required: target repository
project = "Project Name"         # Optional: GitHub Project name or number
assignee = "username"            # Optional: assign all issues to this user

[mapping]
type_field = "Type"              # Project field name for ticket type

[mapping.type]
# ticket type -> project field option value
bug = "Bug"
feature = "Feature"
task = "Task"
epic = "Epic"
chore = "Chore"

[labels]
sync_tags = true                 # Sync ticket tags as GitHub labels
create_missing = true            # Auto-create labels that don't exist
```

## Authentication

Token resolution order:
1. `GITHUB_TOKEN` environment variable
2. `gh auth token` command (GitHub CLI)

This allows seamless use for developers who already have `gh` configured.

## Sync Algorithm

### Push Flow

```
for each ticket in .tickets/*.md:
    parse frontmatter and body
    
    if no external-ref or not gh-*:
        # CREATE new issue
        issue = create_issue(title, body_with_marker)
        apply_labels(ticket.tags)
        write_external_ref(ticket, issue.number)
        
        if project configured:
            add_to_project(issue)
            set_type_field(ticket.type)
        
        if ticket.parent has external-ref:
            add_sub_issue(parent_issue, issue)
    
    else:  # has external-ref: gh-{number}
        # UPDATE existing issue
        issue = fetch_issue(number)
        
        if issue.body missing marker or marker mismatch:
            WARN("Issue modified outside ttr, skipping")
            continue
        
        update_issue(title, body, state, labels)
```

### Issue Body Format

```markdown
<!-- ticket:nw-5c46 -->

{ticket description}

## Design

{design section if present}

## Acceptance Criteria

{acceptance criteria if present}

---
**Depends on:** #45, #67

---
<sub>Synced from ticket `nw-5c46`</sub>
```

The HTML comment marker (`<!-- ticket:nw-5c46 -->`) enables:
- Detecting if an issue was created by ttr
- Verifying the ticket-issue mapping is correct
- Safe conflict detection (skip if marker missing/mismatched)

### Conflict Detection

When updating an existing issue:

| Scenario | Action |
|----------|--------|
| Marker present, matches ticket ID | Safe to update |
| Marker present, different ticket ID | Error (mapping conflict) |
| Marker absent | Warn and skip (manual edit detected) |

### State Mapping

| Ticket Status | GitHub Issue State |
|---------------|-------------------|
| `open` | Open |
| `in_progress` | Open |
| `closed` | Closed |

### Project Schema Validation

Before setting project fields, ttr will:
1. Query the project by name/number
2. Fetch all SingleSelectField definitions
3. Find the configured type field (default: "Type")
4. Validate all mapped type values exist as options
5. Error with helpful message if validation fails

Example error:
```
Error: Project field "Type" does not have option "Epic"
Available options: Bug, Feature, Task, Chore
Configured in sync.toml: epic = "Epic"
```

## CLI Interface

```
ttr - Sync tickets to GitHub Issues

USAGE:
    ttr <COMMAND>

COMMANDS:
    init      Create .tickets/sync.toml configuration
    status    Show sync status of all tickets
    push      Sync tickets to GitHub Issues
    help      Print help information

EXAMPLES:
    ttr init                    # Interactive config setup
    ttr status                  # Show what would be synced
    ttr push                    # Sync all tickets
    ttr push nw-5c46 ab-1234    # Sync specific tickets
```

### Status Output

```
$ ttr status

Tickets: 12 total
  Unsynced:     3  (will create new issues)
  Synced:       8  (up to date)
  Modified:     1  (will update)

Unsynced:
  nw-5c46  [task]  Implement user authentication
  nw-5c47  [bug]   Fix login redirect loop
  nw-5c48  [feature] Add dark mode support

Modified:
  nw-5c40  [task]  Update API documentation (title changed)
```

### Push Output

```
$ ttr push

Validating project schema... OK
Syncing 4 tickets...

CREATE  nw-5c46 → #123  Implement user authentication
  ├─ Labels: backend, auth
  ├─ Project: Added to "Q1 Sprint"
  └─ Type: Task

CREATE  nw-5c47 → #124  Fix login redirect loop
  ├─ Labels: bug, auth
  ├─ Project: Added to "Q1 Sprint"
  ├─ Type: Bug
  └─ Parent: #123 (sub-issue)

UPDATE  nw-5c40 → #120  Update API documentation
  └─ Title updated

SKIP    nw-5c35 → #115  (modified outside ttr)

Summary: 2 created, 1 updated, 1 skipped
```

## GitHub API Usage

### GraphQL Mutations Used

| Operation | Mutation |
|-----------|----------|
| Create issue | `createIssue` |
| Update issue | `updateIssue` |
| Close issue | `closeIssue` |
| Reopen issue | `reopenIssue` |
| Add labels | `addLabelsToLabelable` |
| Create label | `createLabel` |
| Add to project | `addProjectV2ItemById` |
| Set project field | `updateProjectV2ItemFieldValue` |
| Add sub-issue | `addSubIssue` |

### GraphQL Queries Used

| Operation | Query |
|-----------|-------|
| Get repository ID | `repository(owner, name) { id }` |
| Get issue | `repository { issue(number) { id, body, state } }` |
| Get project | `repository { projectV2(number) }` or search by name |
| Get project fields | `projectV2 { fields { nodes { ... on ProjectV2SingleSelectField } } }` |
| Get labels | `repository { labels { nodes { id, name } } }` |

## Dependencies

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
gray_matter = "0.2"
thiserror = "2"
anyhow = "1"
```

## Future Considerations

### Potential Enhancements

1. **Bidirectional sync**: Pull comments, labels, or state changes from GitHub
2. **Conflict merging**: Smart merge of ticket/issue changes
3. **Per-ticket assignees**: Map ticket assignee names to GitHub usernames
4. **Milestone support**: Sync to GitHub milestones
5. **Notes sync**: Optionally sync ticket Notes as issue comments
6. **Webhook mode**: React to GitHub events in real-time

### Breaking Change Risks

- GitHub Projects API is relatively new; field types may change
- Sub-issues feature is still evolving
- GraphQL schema may deprecate mutations

## References

- [wedow/ticket](https://github.com/wedow/ticket) - The ticket system
- [GitHub GraphQL API](https://docs.github.com/en/graphql)
- [GitHub Projects API](https://docs.github.com/en/issues/planning-and-tracking-with-projects/automating-your-project/using-the-api-to-manage-projects)
- [GitHub Sub-issues](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues)
