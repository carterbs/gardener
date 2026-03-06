pub use super::live::terminal::{
    close_live_terminal, draw_dashboard_live, draw_report_live, draw_seeding_live,
    draw_shutdown_screen_live, draw_triage_live, reset_workers_scroll, scroll_workers_down,
    scroll_workers_up,
};
pub use super::views::terminal::render_seeding;

pub(super) use super::live::terminal::{
    clamped_selected_worker, selected_worker_state, set_worker_viewport, teardown_terminal,
    worker_offset_for_selection,
};
#[cfg(test)]
pub(crate) use super::views::terminal::render_shutdown_screen;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_frame_renders_empty_and_populated_activity_states() {
        let empty = render_seeding(&[], 90, 18);
        assert!(empty.contains("seeding your backlog"));
        assert!(empty.contains("waiting for seeding updates"));
        assert!(empty.contains("Seeding in progress"));

        let populated = render_seeding(&["scanning repo".to_string(), "indexing docs".to_string()], 90, 18);
        assert!(populated.contains("scanning repo"));
        assert!(populated.contains("indexing docs"));
        assert!(!populated.contains("waiting for seeding updates"));
    }

    #[test]
    fn shutdown_frame_renders_success_and_error_copy_variants() {
        let success = render_shutdown_screen(
            "Complete",
            "Tasks completed: 4\nTasks merged: 3\nTotal runtime: 2m\n",
            90,
            18,
        );
        assert!(success.contains("Complete"));
        assert!(success.contains("Tasks completed: 4"));
        assert!(success.contains("Tasks merged: 3"));
        assert!(success.contains("Total runtime: 2m"));
        assert!(success.contains("Press any key to exit"));

        let error = render_shutdown_screen("Failed", "Tasks failed: 1\nboom", 90, 18);
        assert!(error.contains("Failed"));
        assert!(error.contains("Tasks failed: 1"));
        assert!(error.contains("boom"));
        assert!(error.contains("Press Ctrl+C or c to copy the error message"));
    }

    #[test]
    fn shutdown_frame_treats_blank_lines_as_empty_rows() {
        let frame = render_shutdown_screen("Error", "Tasks failed: 1\n\nsecond line", 90, 18);
        assert!(frame.contains("Tasks failed: 1"));
        assert!(frame.contains("second line"));
    }

    #[test]
    fn close_live_terminal_resets_live_size_without_touching_worker_scroll_state() {
        set_worker_viewport(4, 9);
        assert!(scroll_workers_down());
        assert!(scroll_workers_down());

        close_live_terminal().expect("close should succeed when no terminal is initialized");

        assert_eq!(selected_worker_state(), 2);
    }

    #[test]
    fn scroll_workers_down_and_up_respect_capacity_and_bounds() {
        reset_workers_scroll();
        set_worker_viewport(3, 6);

        assert!(scroll_workers_down());
        assert!(scroll_workers_down());
        assert!(scroll_workers_down());
        assert_eq!(selected_worker_state(), 3);
        assert_eq!(worker_offset_for_selection(3, 3, 6), 1);

        assert!(scroll_workers_down());
        assert_eq!(selected_worker_state(), 4);
        assert_eq!(worker_offset_for_selection(4, 3, 6), 2);

        assert!(scroll_workers_down());
        assert_eq!(selected_worker_state(), 5);
        assert_eq!(worker_offset_for_selection(5, 3, 6), 3);
        assert!(!scroll_workers_down());

        assert!(scroll_workers_up());
        assert_eq!(selected_worker_state(), 4);
        assert_eq!(worker_offset_for_selection(4, 3, 6), 3);
        assert!(scroll_workers_up());
        assert_eq!(selected_worker_state(), 3);
        assert_eq!(worker_offset_for_selection(3, 3, 6), 3);
    }

    #[test]
    fn scroll_workers_is_noop_without_workers() {
        reset_workers_scroll();
        assert!(!scroll_workers_down());
        assert!(!scroll_workers_up());
        assert_eq!(selected_worker_state(), 0);
    }

    #[test]
    fn clamped_selection_and_offset_adjust_to_visible_bounds() {
        reset_workers_scroll();
        set_worker_viewport(2, 5);
        assert!(scroll_workers_down());
        assert!(scroll_workers_down());
        assert!(scroll_workers_down());
        assert_eq!(selected_worker_state(), 3);
        assert_eq!(worker_offset_for_selection(3, 2, 5), 2);

        assert_eq!(clamped_selected_worker(2), 1);
        assert_eq!(selected_worker_state(), 1);
        assert_eq!(worker_offset_for_selection(1, 2, 2), 0);

        assert_eq!(clamped_selected_worker(0), 0);
        assert_eq!(selected_worker_state(), 0);
    }

    #[test]
    fn live_draw_wrappers_render_without_test_only_bypass_hooks() {
        draw_report_live("/tmp/report.md", "grade: A").expect("report draw");
        draw_seeding_live(&["scan repo".to_string()]).expect("seeding draw");
        draw_triage_live(&["investigate".to_string()], &["artifact.txt".to_string()])
            .expect("triage draw");
        draw_shutdown_screen_live("Complete", "Tasks completed: 1").expect("shutdown draw");
    }
}
