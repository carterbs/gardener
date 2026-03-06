pub(super) struct ShutdownSummary {
    pub(super) completed: usize,
    pub(super) target: usize,
    pub(super) merged: usize,
    pub(super) failed: usize,
    pub(super) total_runtime_secs: u64,
}

impl ShutdownSummary {
    pub(super) fn format_message(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Tasks completed: {} / {}",
            self.completed, self.target
        ));
        lines.push(format!("Tasks merged (PRs landed): {}", self.merged));
        if self.failed > 0 {
            lines.push(format!("Tasks failed / unresolved: {}", self.failed));
        }
        let mins = self.total_runtime_secs / 60;
        let secs = self.total_runtime_secs % 60;
        if mins > 0 {
            lines.push(format!("Total runtime: {}m {}s", mins, secs));
        } else {
            lines.push(format!("Total runtime: {}s", secs));
        }
        lines.join("\n")
    }
}

pub(super) fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn now_hhmmss() -> String {
    let timestamp = now_unix_millis().rem_euclid(86_400_000);
    let secs = (timestamp / 1000) as u64;
    let in_day = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        in_day / 3600,
        (in_day % 3600) / 60,
        in_day % 60
    )
}
