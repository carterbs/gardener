use gardener::hotkeys::{
    action_for_key, dashboard_controls_legend, report_controls_legend, DASHBOARD_BINDINGS,
    REPORT_BINDINGS,
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
    let dashboard = dashboard_controls_legend();
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
            action_for_key(key).is_some(),
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
