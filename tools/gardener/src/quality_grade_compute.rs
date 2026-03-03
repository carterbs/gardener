use crate::quality_assessment_types::*;

pub fn compute_domain_grade(scores: &DomainScores) -> (f64, Grade) {
    // Weighted composite:
    // 40% test_coverage + 20% test_quality + 25% risk_exposure_inverse + 15% convention_adherence
    let composite = (scores.test_coverage as f64 * 0.40)
        + (scores.test_quality as f64 * 0.20)
        + ((100 - scores.risk_exposure) as f64 * 0.25)
        + (scores.convention_adherence as f64 * 0.15);

    let grade = Grade::from_score(composite);
    (composite, grade)
}

pub fn compute_repo_grade(repo: &RepoWideAssessment) -> (f64, Grade) {
    // Equal weight across all five dimensions
    let composite = (repo.agent_steering as f64
        + repo.mechanical_guardrails as f64
        + repo.local_feedback_loop as f64
        + repo.coverage_infrastructure as f64
        + repo.documentation_quality as f64)
        / 5.0;

    let grade = Grade::from_score(composite);
    (composite, grade)
}

pub struct GradeReport {
    pub domain_grades: Vec<(DomainAssessment, f64, Grade)>,
    pub repo_grade: (f64, Grade),
    pub deficiencies: Vec<StructuralDeficiency>,
    pub primary_gap: String,
    pub languages_detected: Vec<String>,
}

pub fn compute_grade_report(payload: AssessmentPayload) -> GradeReport {
    let mut domain_grades: Vec<_> = payload
        .domains
        .into_iter()
        .map(|d| {
            let (score, grade) = compute_domain_grade(&d.scores);
            (d, score, grade)
        })
        .collect();
    // Sort by score ascending (worst first)
    domain_grades.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let (repo_score, repo_grade) = compute_repo_grade(&payload.repo_wide);

    let mut deficiencies = payload.deficiencies;
    deficiencies.sort_by(|a, b| {
        a.severity
            .as_str()
            .cmp(b.severity.as_str())
            .then_with(|| a.category.as_str().cmp(b.category.as_str()))
    });

    GradeReport {
        domain_grades,
        repo_grade: (repo_score, repo_grade),
        deficiencies,
        primary_gap: payload.primary_gap,
        languages_detected: payload.languages_detected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::Priority;

    fn domain_scores(coverage: u8, quality: u8, risk: u8, convention: u8) -> DomainScores {
        DomainScores {
            test_coverage: coverage,
            test_quality: quality,
            risk_exposure: risk,
            convention_adherence: convention,
        }
    }

    fn repo_wide(
        steering: u8,
        guardrails: u8,
        feedback: u8,
        coverage: u8,
        docs: u8,
    ) -> RepoWideAssessment {
        RepoWideAssessment {
            agent_steering: steering,
            mechanical_guardrails: guardrails,
            local_feedback_loop: feedback,
            coverage_infrastructure: coverage,
            documentation_quality: docs,
        }
    }

    // --- Grade boundary tests ---

    #[test]
    fn boundary_92_is_b_plus() {
        // 92 falls in 87..=92 => B+
        let grade = Grade::from_score(92.0);
        assert_eq!(grade, Grade::BPlus);
    }

    #[test]
    fn boundary_93_is_a() {
        let grade = Grade::from_score(93.0);
        assert_eq!(grade, Grade::A);
    }

    #[test]
    fn boundary_87_is_b_plus() {
        let grade = Grade::from_score(87.0);
        assert_eq!(grade, Grade::BPlus);
    }

    #[test]
    fn boundary_86_is_b() {
        let grade = Grade::from_score(86.0);
        assert_eq!(grade, Grade::B);
    }

    #[test]
    fn boundary_80_is_b() {
        let grade = Grade::from_score(80.0);
        assert_eq!(grade, Grade::B);
    }

    #[test]
    fn boundary_79_is_b_minus() {
        let grade = Grade::from_score(79.0);
        assert_eq!(grade, Grade::BMinus);
    }

    #[test]
    fn boundary_75_is_b_minus() {
        let grade = Grade::from_score(75.0);
        assert_eq!(grade, Grade::BMinus);
    }

    #[test]
    fn boundary_74_is_c_plus() {
        let grade = Grade::from_score(74.0);
        assert_eq!(grade, Grade::CPlus);
    }

    #[test]
    fn boundary_60_is_c() {
        let grade = Grade::from_score(60.0);
        assert_eq!(grade, Grade::C);
    }

    #[test]
    fn boundary_59_is_c_minus() {
        let grade = Grade::from_score(59.0);
        assert_eq!(grade, Grade::CMinus);
    }

    #[test]
    fn boundary_55_is_c_minus() {
        let grade = Grade::from_score(55.0);
        assert_eq!(grade, Grade::CMinus);
    }

    #[test]
    fn boundary_54_is_d() {
        let grade = Grade::from_score(54.0);
        assert_eq!(grade, Grade::D);
    }

    #[test]
    fn boundary_40_is_d() {
        let grade = Grade::from_score(40.0);
        assert_eq!(grade, Grade::D);
    }

    #[test]
    fn boundary_39_is_f() {
        let grade = Grade::from_score(39.0);
        assert_eq!(grade, Grade::F);
    }

    // --- All-zero and all-100 tests ---

    #[test]
    fn all_zero_scores_yield_f() {
        let scores = domain_scores(0, 0, 100, 0);
        // 0*0.4 + 0*0.2 + 0*0.25 + 0*0.15 = 0
        let (composite, grade) = compute_domain_grade(&scores);
        assert_eq!(composite, 0.0);
        assert_eq!(grade, Grade::F);
    }

    #[test]
    fn all_100_scores_yield_a() {
        let scores = domain_scores(100, 100, 0, 100);
        // 100*0.4 + 100*0.2 + 100*0.25 + 100*0.15 = 100
        let (composite, grade) = compute_domain_grade(&scores);
        assert_eq!(composite, 100.0);
        assert_eq!(grade, Grade::A);
    }

    #[test]
    fn all_zero_repo_scores_yield_f() {
        let repo = repo_wide(0, 0, 0, 0, 0);
        let (composite, grade) = compute_repo_grade(&repo);
        assert_eq!(composite, 0.0);
        assert_eq!(grade, Grade::F);
    }

    #[test]
    fn all_100_repo_scores_yield_a() {
        let repo = repo_wide(100, 100, 100, 100, 100);
        let (composite, grade) = compute_repo_grade(&repo);
        assert_eq!(composite, 100.0);
        assert_eq!(grade, Grade::A);
    }

    // --- Known composite calculations ---

    #[test]
    fn known_domain_composite_calculation() {
        let scores = domain_scores(80, 70, 30, 90);
        // 80*0.4 + 70*0.2 + 70*0.25 + 90*0.15 = 32 + 14 + 17.5 + 13.5 = 77.0
        let (composite, grade) = compute_domain_grade(&scores);
        assert!((composite - 77.0).abs() < 0.001);
        assert_eq!(grade, Grade::BMinus);
    }

    #[test]
    fn known_repo_composite_calculation() {
        let repo = repo_wide(80, 90, 70, 85, 75);
        // (80 + 90 + 70 + 85 + 75) / 5 = 400 / 5 = 80.0
        let (composite, grade) = compute_repo_grade(&repo);
        assert!((composite - 80.0).abs() < 0.001);
        assert_eq!(grade, Grade::B);
    }

    // --- Grade report tests ---

    #[test]
    fn grade_report_sorts_domains_worst_first() {
        let payload = AssessmentPayload {
            domains: vec![
                DomainAssessment {
                    name: "good".to_string(),
                    languages: vec!["Rust".to_string()],
                    scores: domain_scores(100, 100, 0, 100),
                    notes: vec!["excellent".to_string()],
                },
                DomainAssessment {
                    name: "bad".to_string(),
                    languages: vec!["Python".to_string()],
                    scores: domain_scores(0, 0, 100, 0),
                    notes: vec!["terrible".to_string()],
                },
            ],
            repo_wide: repo_wide(80, 80, 80, 80, 80),
            deficiencies: vec![],
            domain_file_map: Default::default(),
            primary_gap: "coverage".to_string(),
            languages_detected: vec!["Rust".to_string()],
        };
        let report = compute_grade_report(payload);
        assert_eq!(report.domain_grades[0].0.name, "bad");
        assert_eq!(report.domain_grades[1].0.name, "good");
    }

    #[test]
    fn grade_report_sorts_deficiencies_by_severity_then_category() {
        let payload = AssessmentPayload {
            domains: vec![],
            repo_wide: repo_wide(80, 80, 80, 80, 80),
            deficiencies: vec![
                StructuralDeficiency {
                    description: "d1".to_string(),
                    domain: None,
                    category: DeficiencyCategory::MissingTooling,
                    severity: Priority::P1,
                    suggested_task_title: "t1".to_string(),
                    suggested_task_details: "details1".to_string(),
                },
                StructuralDeficiency {
                    description: "d2".to_string(),
                    domain: Some("auth".to_string()),
                    category: DeficiencyCategory::CoverageGap,
                    severity: Priority::P0,
                    suggested_task_title: "t2".to_string(),
                    suggested_task_details: "details2".to_string(),
                },
                StructuralDeficiency {
                    description: "d3".to_string(),
                    domain: None,
                    category: DeficiencyCategory::CoverageGap,
                    severity: Priority::P1,
                    suggested_task_title: "t3".to_string(),
                    suggested_task_details: "details3".to_string(),
                },
            ],
            domain_file_map: Default::default(),
            primary_gap: "coverage".to_string(),
            languages_detected: vec![],
        };
        let report = compute_grade_report(payload);
        // P0 first, then P1 sorted by category
        assert_eq!(report.deficiencies[0].severity, Priority::P0);
        assert_eq!(report.deficiencies[1].category.as_str(), "coverage-gap");
        assert_eq!(report.deficiencies[2].category.as_str(), "missing-tooling");
    }
}
