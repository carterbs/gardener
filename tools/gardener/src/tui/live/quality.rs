use serde_json::json;

use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::tui::live::terminal::draw_live_frame;
use crate::tui::views::quality::{draw_quality_grading_frame, draw_quality_intro_frame};

pub fn draw_quality_grading_live(activity: &[String]) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.quality.draw_grading_live",
        json!({ "activity_lines": activity.len() }),
    );
    draw_live_frame(|frame| draw_quality_grading_frame(frame, activity))
}

pub fn draw_quality_intro_live() -> Result<(), GardenerError> {
    append_run_log("debug", "tui.quality.draw_intro_live", json!({}));
    draw_live_frame(draw_quality_intro_frame)
}
