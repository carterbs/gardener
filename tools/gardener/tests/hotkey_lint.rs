use gardener::hotkeys::{dashboard_controls_legend, report_controls_legend, WORKER_POOL_HOTKEYS};

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
fn linter_hotkeys_advertised_in_ui_must_have_worker_pool_contract_tests() {
    let dashboard = dashboard_controls_legend();
    let report = report_controls_legend();
    let mut advertised = legend_keys(&dashboard);
    advertised.extend(legend_keys(&report));

    let contract_source = include_str!("../src/worker_pool.rs");
    for key in WORKER_POOL_HOTKEYS {
        assert!(
            advertised.contains(&key),
            "hotkey `{key}` must be advertised in UI legends"
        );
        assert!(
            contract_source.contains(&format!("hotkey:{key}")),
            "missing worker pool contract-test marker for hotkey `{key}`"
        );
    }
}
