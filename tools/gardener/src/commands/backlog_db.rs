#![deny(
    clippy::manual_strip,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone
)]

use clap::{Args, Parser, Subcommand};
use gardener::backlog_cli::{
    default_manual_task_id, ensure_db_exists, format_list_row, format_task, parse_kind,
    parse_priority, parse_status, patch_has_changes, render_json, resolve_db_path, runbook_path,
    validate_retire_status, validate_update_status, MutationJson, TaskJson,
};
use gardener::backlog_store::{
    system_time_unix, BacklogStore, ManualTaskInput, TaskStatus, TaskUpdatePatch,
};
use gardener::errors::GardenerError;
use gardener::logging::structured_fallback_line;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "backlog-db")]
#[command(
    about = "Inspect and edit Gardener backlog SQLite rows",
    after_help = "Examples:\n  backlog-db list --db /tmp/backlog.sqlite\n  backlog-db add --title \"Task\" --details \"Why\" --scope runtime\n  backlog-db show --id manual:runtime:auto-123 --json\n  backlog-db update --id manual:runtime:auto-123 --status complete --rationale \"done\" --clear-lease\n\nEnvironment:\n  GARDENER_DB_PATH sets the default manual backlog DB path.\n  If unset, backlog-db defaults to ~/.gardener/backlog.sqlite."
)]
struct Cli {
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    List(ListArgs),
    Add(AddArgs),
    Runbook,
    Show(ShowArgs),
    Update(UpdateArgs),
    Retire(RetireArgs),
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct AddArgs {
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    details: Option<String>,
    #[arg(long, default_value = "P1")]
    priority: String,
    #[arg(long, default_value = "runtime")]
    scope: String,
    #[arg(long, default_value = "ready")]
    status: String,
    #[arg(long, default_value = "feature")]
    kind: String,
    #[arg(long, default_value = "manual")]
    source: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowArgs {
    #[arg(long)]
    id: String,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    rationale: Option<String>,
    #[arg(long)]
    related_pr: Option<i64>,
    #[arg(long)]
    related_branch: Option<String>,
    #[arg(long, default_value_t = false)]
    clear_lease: bool,
    #[arg(long, default_value_t = false)]
    dry_run: bool,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct RetireArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    status: String,
    #[arg(long)]
    rationale: String,
    #[arg(long)]
    related_pr: Option<i64>,
    #[arg(long)]
    related_branch: Option<String>,
    #[arg(long, default_value_t = false)]
    clear_lease: bool,
    #[arg(long, default_value_t = false)]
    dry_run: bool,
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub fn run_with_args(args: &[String]) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "main", "started");
    let _ = structured_fallback_line("backlog_db", "run", "dispatching");
    let cli = Cli::parse_from(args);
    let db_path = resolve_db_path(cli.db)?;

    match cli.command {
        Commands::List(args) => run_list(&db_path, args),
        Commands::Add(args) => run_add(&db_path, args),
        Commands::Runbook => run_runbook(),
        Commands::Show(args) => run_show(&db_path, args),
        Commands::Update(args) => run_update(&db_path, args),
        Commands::Retire(args) => run_retire(&db_path, args),
    }
}

fn open_store(db_path: &std::path::Path) -> Result<BacklogStore, GardenerError> {
    ensure_db_exists(db_path)?;
    BacklogStore::open(db_path)
}

fn run_list(db_path: &std::path::Path, args: ListArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_list", "started");
    let store = open_store(db_path)?;
    let tasks = store.list_recent_tasks(50)?;

    if args.json {
        let json_rows = tasks.iter().map(TaskJson::from).collect::<Vec<_>>();
        println!("{}", render_json(&json_rows)?);
        return Ok(0);
    }

    for task in tasks {
        println!("{}", format_list_row(&task));
    }
    Ok(0)
}

fn run_add(db_path: &std::path::Path, args: AddArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_add", "started");
    ensure_db_exists(db_path)?;
    let Some(title) = args.title else {
        return Err(GardenerError::Cli(
            "--title and --details are required for add".to_string(),
        ));
    };
    let Some(details) = args.details else {
        return Err(GardenerError::Cli(
            "--title and --details are required for add".to_string(),
        ));
    };

    let now = system_time_unix();
    let task = ManualTaskInput {
        task_id: args
            .id
            .unwrap_or_else(|| default_manual_task_id(&args.scope, now)),
        kind: parse_kind(&args.kind)?,
        title,
        details,
        rationale: String::new(),
        scope_key: args.scope,
        priority: parse_priority(&args.priority)?,
        status: parse_status(&args.status)?,
        source: args.source,
        related_pr: None,
        related_branch: None,
    };
    let store = BacklogStore::open(db_path)?;
    let created = store.insert_manual_task(task)?;

    if args.json {
        println!("{}", render_json(&TaskJson::from(&created))?);
    } else {
        println!("created: {}", created.task_id);
    }

    Ok(0)
}

fn run_runbook() -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_runbook", "started");
    let path = runbook_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|error| GardenerError::Io(format!("{}: {error}", path.display())))?;
    print!("{text}");
    Ok(0)
}

fn run_show(db_path: &std::path::Path, args: ShowArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_show", "started");
    let store = open_store(db_path)?;
    let task = store
        .get_task(&args.id)?
        .ok_or_else(|| GardenerError::Cli(format!("backlog task not found: {}", args.id)))?;

    if args.json {
        println!("{}", render_json(&TaskJson::from(&task))?);
    } else {
        println!("{}", format_task(&task));
    }
    Ok(0)
}

fn run_update(db_path: &std::path::Path, args: UpdateArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_update", "started");
    let store = open_store(db_path)?;
    let before = store
        .get_task(&args.id)?
        .ok_or_else(|| GardenerError::Cli(format!("backlog task not found: {}", args.id)))?;
    let status = args.status.as_deref().map(parse_status).transpose()?;
    if let Some(status) = status {
        validate_update_status(&before, status)?;
    }

    let patch = TaskUpdatePatch {
        status,
        rationale: args.rationale,
        related_pr: args.related_pr,
        related_branch: args.related_branch,
        clear_lease: args.clear_lease,
    };

    if !patch_has_changes(&patch) {
        return Err(GardenerError::Cli(
            "update requires at least one of --status, --rationale, --related-pr, --related-branch, or --clear-lease".to_string(),
        ));
    }

    if args.dry_run {
        let mutation = gardener::backlog_store::TaskMutation {
            before: before.clone(),
            after: preview_after(before, patch),
            changed: true,
        };
        return print_mutation(mutation, args.json);
    }

    let mutation = store.update_task_metadata(&args.id, patch)?;
    print_mutation(mutation, args.json)
}

fn run_retire(db_path: &std::path::Path, args: RetireArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "run_retire", "started");
    let store = open_store(db_path)?;
    let status = parse_status(&args.status)?;
    validate_retire_status(status)?;
    let existing = store
        .get_task(&args.id)?
        .ok_or_else(|| GardenerError::Cli(format!("backlog task not found: {}", args.id)))?;

    let patch = TaskUpdatePatch {
        status: Some(status),
        rationale: Some(args.rationale),
        related_pr: args.related_pr,
        related_branch: args.related_branch,
        clear_lease: args.clear_lease || existing.lease_owner.is_some(),
    };

    if args.dry_run {
        let mutation = gardener::backlog_store::TaskMutation {
            before: existing.clone(),
            after: preview_after(existing, patch),
            changed: true,
        };
        return print_mutation(mutation, args.json);
    }

    let mutation = store.retire_task(
        &args.id,
        status,
        patch.rationale.clone().unwrap_or_default(),
        patch.related_pr,
        patch.related_branch.clone(),
        patch.clear_lease,
    )?;
    print_mutation(mutation, args.json)
}

fn preview_after(
    mut task: gardener::backlog_store::BacklogTask,
    patch: TaskUpdatePatch,
) -> gardener::backlog_store::BacklogTask {
    if let Some(status) = patch.status {
        task.status = status;
        if !matches!(status, TaskStatus::Leased | TaskStatus::InProgress) {
            task.lease_owner = None;
            task.lease_expires_at = None;
        }
    }
    if let Some(rationale) = patch.rationale {
        task.rationale = rationale;
    }
    if let Some(related_pr) = patch.related_pr {
        task.related_pr = Some(related_pr);
    }
    if let Some(related_branch) = patch.related_branch {
        task.related_branch = Some(related_branch);
    }
    if patch.clear_lease {
        task.lease_owner = None;
        task.lease_expires_at = None;
    }
    task.last_updated = system_time_unix();
    task
}

fn print_mutation(
    mutation: gardener::backlog_store::TaskMutation,
    json: bool,
) -> Result<i32, GardenerError> {
    if json {
        println!("{}", render_json(&MutationJson::from(&mutation))?);
    } else {
        println!("before:\n{}\n", format_task(&mutation.before));
        println!("after:\n{}\n", format_task(&mutation.after));
        println!("changed: {}", mutation.changed);
    }
    Ok(0)
}
