use gardener::hotkeys::{
    action_for_key_with_mode, dashboard_controls_legend_for_mode, report_controls_legend,
    HotkeyAction, DASHBOARD_BINDINGS, OPERATOR_BINDINGS,
};

#[test]
fn all_standard_bindings_resolved() {
    let cases = [
        ('q', HotkeyAction::Quit),
        ('j', HotkeyAction::ScrollDown),
        ('k', HotkeyAction::ScrollUp),
        ('v', HotkeyAction::ViewReport),
        ('g', HotkeyAction::RegenerateReport),
        ('b', HotkeyAction::Back),
    ];
    for (key, expected) in cases {
        let action = action_for_key_with_mode(key, false);
        assert_eq!(action, Some(expected), "key '{key}' should resolve to {expected:?}");
    }
}

#[test]
fn operator_keys_gated_without_flag() {
    for key in ['r', 'l', 'p'] {
        let action = action_for_key_with_mode(key, false);
        assert_eq!(action, None, "key '{key}' should be None without operator mode");
    }
}

#[test]
fn operator_keys_enabled_with_flag() {
    let cases = [
        ('r', HotkeyAction::Retry),
        ('l', HotkeyAction::ReleaseLease),
        ('p', HotkeyAction::ParkEscalate),
    ];
    for (key, expected) in cases {
        let action = action_for_key_with_mode(key, true);
        assert_eq!(action, Some(expected), "key '{key}' should resolve to {expected:?} with operator mode");
    }
}

#[test]
fn unknown_key_returns_none() {
    for key in ['x', 'z', '1', '!', ' '] {
        let action = action_for_key_with_mode(key, true);
        assert_eq!(action, None, "key '{key}' should return None");
    }
}

#[test]
fn back_action_available_always() {
    assert_eq!(action_for_key_with_mode('b', false), Some(HotkeyAction::Back));
    assert_eq!(action_for_key_with_mode('b', true), Some(HotkeyAction::Back));
}

#[test]
fn all_dashboard_bindings_have_matching_action() {
    for binding in &DASHBOARD_BINDINGS {
        let action = action_for_key_with_mode(binding.key, false);
        assert!(action.is_some(), "dashboard binding '{}' ({}) has no matching action", binding.key, binding.action);
    }
}

#[test]
fn all_operator_bindings_have_matching_action() {
    for binding in &OPERATOR_BINDINGS {
        let action = action_for_key_with_mode(binding.key, true);
        assert!(action.is_some(), "operator binding '{}' ({}) has no matching action with operator=true", binding.key, binding.action);
    }
}

#[test]
fn dashboard_controls_legend_includes_all_keys() {
    let legend = dashboard_controls_legend_for_mode(false);
    for binding in &DASHBOARD_BINDINGS {
        assert!(legend.contains(binding.key), "legend should contain key '{}': {legend}", binding.key);
    }
}

#[test]
fn dashboard_controls_legend_with_operator_includes_operator_keys() {
    let legend = dashboard_controls_legend_for_mode(true);
    for binding in &OPERATOR_BINDINGS {
        assert!(legend.contains(binding.key), "operator legend should contain key '{}': {legend}", binding.key);
    }
}

#[test]
fn report_controls_legend_includes_back_and_regenerate() {
    let legend = report_controls_legend();
    assert!(legend.contains('b'), "report legend should contain 'b': {legend}");
    assert!(legend.contains('g'), "report legend should contain 'g': {legend}");
}
