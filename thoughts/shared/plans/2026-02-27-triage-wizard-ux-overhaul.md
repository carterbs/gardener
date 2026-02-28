# Triage Wizard UX Overhaul

## Overview

Redesign `run_repo_health_wizard` to match the visual quality of the main dashboard. The current wizard is plain-text with no cursor, no color, and no visual distinction between labels and editable values — users don't realize they're supposed to type.

## Current State

**File**: `tools/gardener/src/tui.rs:1740-1870`

The wizard renders 4 steps in a loop. Each step dumps a plain `format!()` string into a single `Paragraph` widget with `Block::default().borders(Borders::ALL).title("Input")`. Problems:

1. **No cursor** — pre-filled values sit there with no visual indicator of an insertion point
2. **No color** — labels, values, and help text are all the same default white
3. **No step progress** — just "Step 1/4" text in the footer
4. **Generic borders** — `Borders::ALL` with default styling vs the dashboard's muted `Rgb(82, 88, 126)`
5. **No hierarchy** — everything is the same visual weight

## Desired End State

A wizard that looks like it belongs in the same app as the dashboard:

```
┌─────────────────────────────────────────────────────┐
│ GARDENER  setup wizard          Esc = keep defaults │
├─────────────────────────────────────────────────────┤
│                                                     │
│  ● Parallelism  ○ Validation  ○ Docs  ○ Notes      │
│                                                     │
│  Worker parallelism                                 │
│                                                     │
│  How many parallel workers should gardener run?     │
│                                                     │
│  > 3█                                               │
│                                                     │
│  Range: 1-32. Type a number, Enter to continue.     │
│                                                     │
├─────────────────────────────────────────────────────┤
│ Step 1 of 4                               Enter →   │
└─────────────────────────────────────────────────────┘
```

Key visual properties:
- Brand-blue GARDENER wordmark + amber "setup wizard" subtitle (matching dashboard header)
- Step dots across the top showing done (●)/current (●)/future (○)
- Field label in bold white
- Help text in dim gray
- Editable value on its own line with a `>` prompt and a block cursor `█`
- Muted borders matching the dashboard palette
- Footer shows step count + "Enter →" hint

## What We're NOT Doing

- No animation/spinners (the wizard blocks on input, no tick loop)
- No restructuring the 4-step flow or changing what data is collected
- No refactoring the event handling logic (key dispatch stays the same)
- No extracting color constants (the dashboard doesn't use them either — inline RGB is the pattern here)

## Key Discoveries

- Dashboard colors are all inline `Color::Rgb(...)` — no constants: `tui.rs:784-892`
- Brand blue: `Rgb(85, 198, 255)`, muted border: `Rgb(82, 88, 126)`, amber: `Rgb(245, 196, 95)`, green: `Rgb(126, 231, 135)`, subtitle: `Rgb(170, 178, 210)`
- `StageState` enum (Done/Current/Future) already exists at `tui.rs:96-101` — reuse for step dots
- `triage_stages_with_state` pattern at `tui.rs:341-356` is the exact model for step progress
- The triage stage data is built but **never rendered visually** in `draw_triage_frame_from_state` — it's populated but unused
- Dashboard uses `Paragraph::new(vec![Line::from(vec![Span::styled(...)])])` pattern everywhere for rich text

## Implementation Approach

Single phase — all changes are in one function (`run_repo_health_wizard`) plus a small helper for the step indicator. No new dependencies needed.

## Phase 1: Redesign the wizard draw closure

### Changes required

**File: `tools/gardener/src/tui.rs`**

#### 1a. Add wizard step labels constant (near line 66, next to `TRIAGE_STAGE_LABELS`)

```rust
const WIZARD_STEP_LABELS: [&str; 4] = [
    "Parallelism",
    "Validation",
    "Docs",
    "Notes",
];
```

#### 1b. Add a helper to build the step indicator line (near line 356, after `triage_stages_with_state`)

```rust
fn wizard_step_indicator(current_step: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, label) in WIZARD_STEP_LABELS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        let (dot, style) = if i < current_step {
            ("● ", Style::default().fg(Color::Rgb(126, 231, 135)))  // done: green
        } else if i == current_step {
            ("● ", Style::default().fg(Color::Rgb(85, 198, 255)).add_modifier(Modifier::BOLD))  // current: brand blue bold
        } else {
            ("○ ", Style::default().fg(Color::Rgb(82, 88, 126)))  // future: muted
        };
        spans.push(Span::styled(dot, style));
        spans.push(Span::styled(
            *label,
            if i == current_step {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else if i < current_step {
                Style::default().fg(Color::Rgb(126, 231, 135))
            } else {
                Style::default().fg(Color::Rgb(82, 88, 126))
            },
        ));
    }
    Line::from(spans)
}
```

#### 1c. Rewrite the draw closure in `run_repo_health_wizard` (lines 1757-1801)

Replace the entire `terminal.draw(|frame| { ... })` block. New layout:

```
Vertical layout:
  [Length(3)]  — Header: "GARDENER  setup wizard" + "Esc = keep defaults"
  [Length(1)]  — Step indicator dots
  [Min(6)]    — Body: field label, help text, input line with cursor
  [Length(2)]  — Footer: "Step N of 4" + "Enter →"
```

**Header** — matches dashboard pattern from line 782:
```rust
let header = Paragraph::new(Line::from(vec![
    Span::styled("GARDENER ", Style::default()
        .fg(Color::Rgb(85, 198, 255))
        .add_modifier(Modifier::BOLD)),
    Span::styled("setup wizard", Style::default()
        .fg(Color::Rgb(245, 196, 95))
        .add_modifier(Modifier::BOLD)),
]))
.block(Block::default()
    .borders(Borders::BOTTOM)
    .border_style(Style::default().fg(Color::Rgb(82, 88, 126))));
```

**Step indicator** — uses the helper from 1b:
```rust
let steps = Paragraph::new(wizard_step_indicator(step));
```

**Body** — different per step, but all follow this structure:
```rust
// Each step renders as a multi-line Paragraph:
//   Line 1: field label (bold white)
//   Line 2: empty
//   Line 3: help/description text (dim gray)
//   Line 4: empty
//   Line 5: "> {value}█" — input line with cursor

let (label, help, value_display) = match step {
    0 => (
        "Worker parallelism",
        "How many parallel workers? Range: 1-32.",
        format!("> {}█", parallelism_input),
    ),
    1 => (
        "Validation command",
        "Command to verify code changes. Edit or keep the default.",
        format!("> {}█", validation),
    ),
    2 => (
        "Architecture docs available?",
        "Are architecture/quality docs accessible in the repo? Press y/n.",
        format!("> {}", if docs_accessible { "yes" } else { "no" }),
    ),
    _ => (
        "Additional constraints (optional)",
        "Any extra context for workers? Leave empty to skip.",
        format!("> {}█", notes),
    ),
};

let body = Paragraph::new(vec![
    Line::from(Span::styled(label, Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD))),
    Line::from(""),
    Line::from(Span::styled(help, Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::DIM))),
    Line::from(""),
    Line::from(Span::styled(value_display, Style::default()
        .fg(Color::Rgb(85, 198, 255)))),
])
.block(Block::default()
    .borders(Borders::ALL)
    .border_style(Style::default().fg(Color::Rgb(82, 88, 126))));
```

Note: step 2 (y/n toggle) shows no `█` cursor since it's not a text field.

**Footer**:
```rust
let footer = Paragraph::new(Line::from(vec![
    Span::styled(
        format!("Step {} of 4", step + 1),
        Style::default().fg(Color::Gray)),
    Span::raw("  "),
    Span::styled(
        if step < 3 { "Enter →" } else { "Enter to finish" },
        Style::default().fg(Color::Rgb(170, 178, 210))),
]))
.block(Block::default()
    .borders(Borders::TOP)
    .border_style(Style::default().fg(Color::Rgb(82, 88, 126))));
```

### Success Criteria

**Automated:**
- `cargo test -p gardener` passes (no existing wizard tests to break, but ensures no compile errors)
- `cargo clippy -p gardener` clean

**Manual:**
- Run `gardener triage` on a repo — wizard renders with:
  - [ ] GARDENER wordmark in blue, "setup wizard" in amber
  - [ ] Step dots showing current step highlighted, future steps dimmed
  - [ ] Block cursor `█` visible after pre-filled text values
  - [ ] Input value rendered in brand blue
  - [ ] Help text is dim gray, visually recessed
  - [ ] Borders are muted purple-gray matching dashboard
  - [ ] Pressing Esc still works (keeps defaults)
  - [ ] All 4 steps navigate correctly with Enter
  - [ ] Y/N toggle on step 3 still works
  - [ ] Backspace still edits text fields

## Testing Strategy

- Compile + existing test suite (no wizard-specific tests exist today)
- Manual run-through of all 4 wizard steps
- Verify Esc-to-skip still works
- Verify narrow terminal (< 80 cols) doesn't panic

## References

- Dashboard header styling: `tui.rs:782-824`
- Now section pattern: `tui.rs:827-896`
- Stage progress pattern: `tui.rs:341-356`
- Current wizard code: `tui.rs:1740-1870`
