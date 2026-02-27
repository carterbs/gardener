use gardener::hotkeys::{
    action_for_key_with_mode, dashboard_controls_legend_for_mode, report_controls_legend,
    DASHBOARD_BINDINGS, OPERATOR_BINDINGS, REPORT_BINDINGS,
};

fn legend_keys(legend: &str) -> Vec<char> {
    legend
        .split_whitespace()
        .filter_map(|token| {
            if token.len() == 1 {
                token.chars().next()
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn linter_hotkeys_advertised_in_ui_must_have_behavior() {
    let dashboard = dashboard_controls_legend_for_mode(false);
    let report = report_controls_legend();
    let mut advertised = legend_keys(&dashboard);
    advertised.extend(legend_keys(&report));

    for binding in DASHBOARD_BINDINGS {
        assert!(advertised.contains(&binding.key));
    }
    for binding in REPORT_BINDINGS {
        assert!(advertised.contains(&binding.key));
    }

    let mut unique_advertised = advertised;
    unique_advertised.sort_unstable();
    unique_advertised.dedup();
    for key in unique_advertised {
        assert!(
            action_for_key_with_mode(key, false).is_some(),
            "advertised hotkey `{key}` has no application behavior"
        );
    }

    let contract_source = include_str!("../src/worker_pool.rs");
    for binding in DASHBOARD_BINDINGS {
        let key = binding.key;
        assert!(
            contract_source.contains(&format!("hotkey:{key}")),
            "missing worker pool contract-test marker for hotkey `{key}`"
        );
    }
    for binding in REPORT_BINDINGS {
        let key = binding.key;
        assert!(
            contract_source.contains(&format!("hotkey:{key}")),
            "missing worker pool contract-test marker for hotkey `{key}`"
        );
    }
}

#[test]
fn linter_operator_hotkeys_are_hidden_by_default_but_supported_when_enabled() {
    let dashboard = dashboard_controls_legend_for_mode(false);
    for binding in OPERATOR_BINDINGS {
        assert!(!legend_keys(&dashboard).contains(&binding.key));
        assert!(action_for_key_with_mode(binding.key, false).is_none());
        assert!(action_for_key_with_mode(binding.key, true).is_some());
    }
}
