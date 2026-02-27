#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub key: char,
    pub action: &'static str,
}

pub const DASHBOARD_BINDINGS: [HotkeyBinding; 6] = [
    HotkeyBinding {
        key: 'q',
        action: "quit",
    },
    HotkeyBinding {
        key: 'r',
        action: "retry",
    },
    HotkeyBinding {
        key: 'l',
        action: "release-lease",
    },
    HotkeyBinding {
        key: 'p',
        action: "park/escalate",
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

pub const WORKER_POOL_HOTKEYS: [char; 3] = ['v', 'g', 'b'];

pub fn dashboard_controls_legend() -> String {
    format_bindings("Keys: ", &DASHBOARD_BINDINGS)
}

pub fn report_controls_legend() -> String {
    format_bindings("Keys: ", &REPORT_BINDINGS)
}

fn format_bindings(prefix: &str, bindings: &[HotkeyBinding]) -> String {
    let parts = bindings
        .iter()
        .map(|binding| format!("{} {}", binding.key, binding.action))
        .collect::<Vec<_>>();
    format!("{prefix}{}", parts.join("  "))
}
