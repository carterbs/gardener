#[path = "commands/understand.rs"]
mod understand_cmd;
#[path = "commands/plan.rs"]
mod plan_cmd;
#[path = "commands/do_task.rs"]
mod do_task_cmd;
#[path = "commands/git_push.rs"]
mod git_push_cmd;
#[path = "commands/review_pr.rs"]
mod review_pr_cmd;
#[path = "commands/merge_pr.rs"]
mod merge_pr_cmd;
#[path = "commands/friction_analysis.rs"]
mod friction_analysis_cmd;
#[path = "commands/otel_logs.rs"]
mod otel_logs_cmd;
#[path = "commands/backlog_db.rs"]
mod backlog_db_cmd;
#[path = "commands/seed_backlog.rs"]
mod seed_backlog_cmd;
#[path = "commands/tui_live_smoke.rs"]
mod tui_live_smoke_cmd;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let command = args[1].clone();
        if !command.starts_with('-') {
            let subcommand_args = args.split_off(1);
            let exit_code = match command.as_str() {
                "understand" => understand_cmd::run_with_args(&subcommand_args),
                "plan" => plan_cmd::run_with_args(&subcommand_args),
                "do-task" => do_task_cmd::run_with_args(&subcommand_args),
                "git-push" => git_push_cmd::run_with_args(&subcommand_args),
                "review-pr" => review_pr_cmd::run_with_args(&subcommand_args),
                "merge-pr" => merge_pr_cmd::run_with_args(&subcommand_args),
                "friction-analysis" => friction_analysis_cmd::run_with_args(&subcommand_args),
                "otel-logs" => otel_logs_cmd::run_with_args(&subcommand_args),
                "backlog-db" => backlog_db_cmd::run_with_args(&subcommand_args),
                "seed-backlog" => seed_backlog_cmd::run_with_args(&subcommand_args),
                "tui-live-smoke" => tui_live_smoke_cmd::run_with_args(&subcommand_args),
                _ => gardener::run(),
            };

            match exit_code {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
    }

    match gardener::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
