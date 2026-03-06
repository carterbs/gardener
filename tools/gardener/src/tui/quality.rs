pub use super::live::quality::{draw_quality_grading_live, draw_quality_intro_live};
pub use super::views::quality::{render_quality_grading, render_quality_intro, QUALITY_DIMENSIONS};

#[cfg(test)]
use super::views::quality::{quality_activity_lines, quality_dimension_lines};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_quality_intro_lists_all_dimension_ids() {
        let frame = render_quality_intro(140, 24);
        for (name, _) in QUALITY_DIMENSIONS {
            assert!(frame.contains(name), "missing dimension {name}");
        }
        assert!(frame.contains("Quality Dimensions"));
        assert!(frame.contains("assessing 9 quality dimensions"));
    }

    #[test]
    fn render_quality_grading_shows_activity_lines_and_footer() {
        let activity = vec![
            "Scanning repository evidence".to_string(),
            "Comparing coverage infrastructure".to_string(),
            "Scoring mechanical guardrails".to_string(),
        ];
        let frame = render_quality_grading(&activity, 110, 20);
        assert!(frame.contains("Quality Grading Activity"));
        assert!(frame.contains("Scanning repository evidence"));
        assert!(frame.contains("Comparing coverage infrastructure"));
        assert!(frame.contains("Scoring mechanical guardrails"));
        assert!(frame.contains("Quality grading in progress"));
    }

    #[test]
    fn render_quality_grading_uses_waiting_state_when_empty() {
        let frame = render_quality_grading(&[], 90, 16);
        assert!(frame.contains("grading your repository"));
        assert!(frame.contains("waiting for quality grading updates"));
    }

    #[test]
    fn quality_activity_lines_preserve_order_and_fallback_copy() {
        assert_eq!(
            quality_activity_lines(&[]),
            vec!["- waiting for quality grading updates".to_string()]
        );
        assert_eq!(
            quality_activity_lines(&[
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ]),
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ]
        );
    }

    #[test]
    fn quality_dimension_lines_cover_every_dimension_description() {
        let lines = quality_dimension_lines();
        assert_eq!(lines.len(), QUALITY_DIMENSIONS.len());
        for ((name, desc), line) in QUALITY_DIMENSIONS.iter().zip(lines.iter()) {
            assert!(line.contains(name));
            assert!(line.contains(desc));
        }
    }

    #[test]
    fn live_quality_draw_wrappers_render_without_test_only_bypass_hooks() {
        draw_quality_grading_live(&["Scanning coverage".to_string()]).expect("grading draw");
        draw_quality_intro_live().expect("intro draw");
    }
}
