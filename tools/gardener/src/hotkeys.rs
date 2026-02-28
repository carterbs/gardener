#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub key: char,
    pub action: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Quit,
    Retry,
    ReleaseLease,
    ParkEscalate,
    ScrollDown,
    ScrollUp,
    ViewReport,
    RegenerateReport,
    Back,
}

pub const DASHBOARD_BINDINGS: [HotkeyBinding; 5] = [
    HotkeyBinding {
        key: 'q',
        action: "quit",
    },
    HotkeyBinding {
        key: 'j',
        action: "scroll down",
    },
    HotkeyBinding {
        key: 'k',
        action: "scroll up",
    },
    HotkeyBinding {
        key: 'v',
        action: "view report",
    },
    HotkeyBinding {
        key: 'g',
        action: "regenerate",
    },
];

pub const OPERATOR_BINDINGS: [HotkeyBinding; 3] = [
    HotkeyBinding {
        key: 'r',
        action: "retry stuck leases",
    },
    HotkeyBinding {
        key: 'l',
        action: "force release leases",
    },
    HotkeyBinding {
        key: 'p',
        action: "escalate to P0",
    },
];

pub const REPORT_BINDINGS: [HotkeyBinding; 2] = [
    HotkeyBinding {
        key: 'b',
        action: "back",
    },
    HotkeyBinding {
        key: 'g',
        action: "regenerate report",
    },
];

pub fn dashboard_controls_legend() -> String {
    dashboard_controls_legend_for_mode(operator_hotkeys_enabled())
}

pub fn dashboard_controls_legend_for_mode(operator_hotkeys: bool) -> String {
    let mut bindings = DASHBOARD_BINDINGS.to_vec();
    if operator_hotkeys {
        bindings.extend(OPERATOR_BINDINGS);
    }
    format_bindings("Keys: ", &bindings)
}

pub fn report_controls_legend() -> String {
    format_bindings("Keys: ", &REPORT_BINDINGS)
}

pub fn action_for_key(key: char) -> Option<HotkeyAction> {
    action_for_key_with_mode(key, operator_hotkeys_enabled())
}

pub fn action_for_key_with_mode(key: char, operator_hotkeys: bool) -> Option<HotkeyAction> {
    match key {
        'q' => Some(HotkeyAction::Quit),
        'j' => Some(HotkeyAction::ScrollDown),
        'k' => Some(HotkeyAction::ScrollUp),
        'r' if operator_hotkeys => Some(HotkeyAction::Retry),
        'l' if operator_hotkeys => Some(HotkeyAction::ReleaseLease),
        'p' if operator_hotkeys => Some(HotkeyAction::ParkEscalate),
        'v' => Some(HotkeyAction::ViewReport),
        'g' => Some(HotkeyAction::RegenerateReport),
        'b' => Some(HotkeyAction::Back),
        _ => None,
    }
}

pub fn operator_hotkeys_enabled() -> bool {
    std::env::var("GARDENER_OPERATOR_HOTKEYS")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(false)
}

fn format_bindings(prefix: &str, bindings: &[HotkeyBinding]) -> String {
    let parts = bindings
        .iter()
        .map(|binding| format!("{} {}", binding.key, binding.action))
        .collect::<Vec<_>>();
    format!("{prefix}{}", parts.join("  "))
}
