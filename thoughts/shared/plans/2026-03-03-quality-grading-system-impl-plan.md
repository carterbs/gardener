# Quality Grading System — Implementation Plan

## Overview

Replace the current deterministic-only quality scoring pipeline (`quality_domain_catalog.rs` → `quality_evidence.rs` → `quality_scoring.rs` → `quality_grades.rs`) with a hybrid architecture: **deterministic tools** gather hard data, an **LLM agent** interprets it and produces structured scores, and a **deterministic formula** converts scores to letter grades and renders the output document.

The system must grade any repo given only a path — no setup, no config, no prior knowledge.

```
[deterministic tools] → [agent assessment + scoring] → [deterministic grade computation + rendering]
```

---

## Current State

The existing pipeline is four Rust files called from `startup.rs:refresh_quality_report()`:

| File | What it does | Limitation |
|---|---|---|
| `quality_domain_catalog.rs` | Walks `src/**/*.rs`, pattern-matches to hardcoded domain labels | Rust-only, hardcoded domains, no discovery |
| `quality_evidence.rs` | Counts source files, test files, integration tests, instrumentation | Rust-only, string-matching heuristics |
| `quality_scoring.rs` | Applies a fixed formula: base + tested_ratio + instrumentation + bonuses | No agent judgment, no risk weighting |
| `quality_grades.rs` | Renders Markdown report with domain table and readiness dimensions | No per-domain notes, no deficiency list |

The current startup path (`refresh_quality_report`) depends on `RepoIntelligenceProfile` from `.gardener/repo-intelligence.toml`. The new system must **not** require this artifact — it must work standalone with only a repo path.

Key gaps vs. the pre-plan:
- **No multi-language support** — only walks `*.rs`
- **No LLM-driven domain discovery** — domains are a hardcoded match expression
- **No agent judgment** — scoring is purely formulaic
- **No structural deficiency detection** — no backlog task generation from gaps
- **5-level grading (A/B/C/D/F)** — pre-plan requires 9-level (A through F with +/-)

---

## Architecture

### Three-layer design

**Layer 1: Evidence Tools (Rust, deterministic)**
A set of tools that collect hard data about a repo. Each produces structured JSON. They don't interpret — the agent interprets. All tools are combined into a single CLI entry point (`gardener quality-tools collect <repo-path>`) that returns a versioned **evidence bundle** — one JSON document containing all tool outputs. Individual tool subcommands exist for debugging but the agent receives the full bundle.

**Layer 2: Assessment Agent (LLM, non-deterministic)**
An agent session that receives a prompt, the pre-collected evidence bundle, and the repo path. The agent **does not call tools** — it receives the evidence bundle as context and reasons about it. The evidence bundle includes not just file listings and metrics, but also **doc contents** (full text of steering/convention docs) and **file signatures** (first ~20 lines of each source file for understanding purpose). The agent emits a structured JSON payload with per-domain scores, repo-wide scores, and structural deficiencies.

**Layer 3: Grade Computation + Rendering (Rust, deterministic)**
A pure function maps the agent's scores to letter grades (9-level scale) and renders the Markdown output document.

### Data flow

```
startup.rs / CLI command
  └── Layer 1: collect_evidence_bundle(repo_path) → EvidenceBundle (JSON)
  └── Layer 2: run_assessment_agent(bundle, repo_path) → AssessmentPayload
  └── Layer 3: compute_grades(payload) → GradeReport
  └── Layer 3: render_document(report) → Markdown string
  └── Layer 3: emit_backlog_tasks(report.deficiencies) → BacklogStore
```

The agent does **not** invoke tools at runtime — evidence is pre-collected. This keeps the agent's job focused (interpret and score) and makes the pipeline deterministically reproducible up to the LLM call boundary.

### Large repo scaling strategy

For repos with >2,000 source files, the evidence bundle would exceed typical LLM context limits. The pipeline handles this with a **two-tier approach**:

1. **Summary tier** (always included): Language summary, aggregated metrics per top-level directory (file count, test count, assertion density, debt markers, instrumentation ratio), doc contents, CI/lint configs, coverage summary. Fixed size regardless of repo size.
2. **Detail tier** (included up to budget): Per-file listings, sorted by relevance (untested files first, then files with debt markers, then the rest). Truncated at a configurable token budget (default: 80,000 tokens). The bundle includes a `truncated: true` flag and `files_included` / `files_total` counts so the agent knows it's working with a subset.

The agent prompt explicitly handles truncation: "If the evidence bundle is truncated, assess based on what you can see. Note that your sample is biased toward untested and problematic files."

For the deterministic fallback, scaling is not an issue — the fallback receives the **full, non-truncated evidence bundle** (truncation only applies to the JSON serialization sent to the LLM agent). The `collect_evidence_bundle()` function always collects all data; a separate `truncate_for_agent()` function produces the agent-facing subset. The fallback operates on the full `EvidenceBundle` struct in memory.

### Empty repo handling

If the repo contains zero recognized source files (only config, docs, or nothing):
- Evidence bundle is still generated with all-zero metrics and empty file lists.
- The pipeline produces a valid report with a single "repository" pseudo-domain scoring 0 across all axes.
- Grade: F. Primary gap: "No source files detected."
- No backlog tasks emitted (nothing actionable).
- This is a valid, non-error outcome — the system grades what it finds.

### File exclusion policy

All tools respect a default exclusion list. These directories/patterns are skipped during walks and scans:
- `node_modules/`, `vendor/`, `third_party/`, `.git/`
- `target/`, `dist/`, `build/`, `out/`, `.build/`
- `*.min.js`, `*.min.css`, `*.bundle.js`
- Lock files: `package-lock.json`, `Cargo.lock`, `yarn.lock`, `Gemfile.lock`, `poetry.lock`
- Generated: `*.pb.go`, `*.generated.*`, `*_generated.*`

The exclusion list is a static constant in `tree_walker.rs`. Repos can override via `.gardener/quality-ignore` (gitignore syntax) but it's not required.

---

## Phase 1: Shared Types + Evidence Tools (P0-4, P0-3, P0-8, P0-9)

### 1.0 Assessment Types (extracted early for Phase 2/3 independence)

**New file: `tools/gardener/src/quality_assessment_types.rs`**

All shared types live here so Phase 3 (grade computation) can be built before Phase 2 (agent). No file moves — new files alongside existing ones. Legacy files remain untouched until everything works.

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct AssessmentPayload {
    pub domains: Vec<DomainAssessment>,
    pub repo_wide: RepoWideAssessment,
    pub deficiencies: Vec<StructuralDeficiency>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainAssessment {
    pub name: String,
    pub languages: Vec<String>,
    pub scores: DomainScores,
    pub note: String,  // one-sentence agent-written assessment (P1-6)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainScores {
    pub test_coverage: u8,       // 0-100
    pub test_quality: u8,        // 0-100 (assertion density signal)
    pub risk_exposure: u8,       // 0-100 (higher = more untested risk)
    pub convention_adherence: u8, // 0-100
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoWideAssessment {
    pub agent_steering: u8,          // 0-100
    pub mechanical_guardrails: u8,   // 0-100
    pub local_feedback_loop: u8,     // 0-100
    pub coverage_infrastructure: u8, // 0-100
    pub documentation_quality: u8,   // 0-100
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StructuralDeficiency {
    pub description: String,
    pub domain: Option<String>,  // which domain this affects, if any
    pub category: DeficiencyCategory,
    pub severity: Priority,  // reuse existing P0/P1/P2
    pub suggested_task_title: String,
    pub suggested_task_details: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum DeficiencyCategory {
    CoverageGap,
    MissingTooling,
    MissingDocumentation,
    ConventionViolation,
    ObservabilityGap,
    FeedbackLoopGap,
}

impl DeficiencyCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CoverageGap => "coverage-gap",
            Self::MissingTooling => "missing-tooling",
            Self::MissingDocumentation => "missing-documentation",
            Self::ConventionViolation => "convention-violation",
            Self::ObservabilityGap => "observability-gap",
            Self::FeedbackLoopGap => "feedback-loop-gap",
        }
    }
}

/// 9-level grade scale
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    A, BPlus, B, BMinus, CPlus, C, CMinus, D, F,
}

impl Grade {
    pub fn from_score(score: f64) -> Self {
        match score as u8 {
            93..=100 => Grade::A,
            87..=92  => Grade::BPlus,
            80..=86  => Grade::B,
            75..=79  => Grade::BMinus,
            68..=74  => Grade::CPlus,
            60..=67  => Grade::C,
            55..=59  => Grade::CMinus,
            40..=54  => Grade::D,
            _        => Grade::F,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Grade::A => "A", Grade::BPlus => "B+", Grade::B => "B",
            Grade::BMinus => "B-", Grade::CPlus => "C+", Grade::C => "C",
            Grade::CMinus => "C-", Grade::D => "D", Grade::F => "F",
        }
    }
}
```

### 1.1 Language Registry

**New file: `tools/gardener/src/quality_language_registry.rs`**

A registry of language definitions. Built-in languages: **Rust, TypeScript/JavaScript, Swift, Python, Go**. Additionally, an `Unknown` catch-all path: any file with an unrecognized extension is reported as `Unknown(<ext>)` with extension and shebang detection. Unrecognized languages are always reported in the evidence bundle so the agent can see the full picture.

```rust
pub struct LanguageDefinition {
    pub name: String,
    pub extensions: Vec<String>,
    pub source_globs: Vec<String>,
    pub test_file_indicators: Vec<TestFileIndicator>,
    pub assertion_patterns: Vec<String>,
    pub coverage_artifacts: Vec<String>,
    pub instrumentation_patterns: Vec<String>,
}

pub enum TestFileIndicator {
    ContentMatch(String),        // e.g., "#[cfg(test)]"
    PathPattern(String),         // e.g., "*_test.go"
    DirectoryConvention(String), // e.g., "__tests__/"
}

pub enum TestType {
    Unit,
    Integration,
    EndToEnd,
    Unknown,
}

/// Classification heuristics for test type:
/// - Unit: collocated test modules, files in `tests/unit/`, `*_test.rs` without integration markers
/// - Integration: files in `tests/integration/`, `tests/` root for some languages, `*.integration.*`
/// - E2E: files in `tests/e2e/`, `e2e/`, `*.e2e.*`, `cypress/`, `playwright/`
/// - Unknown: detected as test but doesn't match any type heuristic
pub fn classify_test_type(path: &Path, language: &LanguageDefinition) -> TestType { ... }

/// Returns all built-in languages + an Unknown fallback.
/// Unknown matches any extension not claimed by a built-in language.
pub fn builtin_registry() -> Vec<LanguageDefinition> { ... }

/// Identify language from file path. Returns Unknown(<ext>) for unrecognized extensions.
/// Also checks shebang lines for extensionless scripts.
pub fn identify_language(path: &Path, first_line: Option<&str>) -> String { ... }
```

### 1.2 Tree Walker Tool

**New file: `tools/gardener/src/quality_tree_walker.rs`**

Walks the repo, lists all source and test files grouped by directory and language. Respects the exclusion policy. Reports unrecognized languages explicitly. Includes a **file signature** (first ~20 lines) for each source file — this gives the agent enough context to understand each file's purpose for domain discovery and risk assessment without reading full file bodies.

Output schema:
```json
{
  "directories": [
    {
      "path": "src/agent",
      "files": [
        {
          "path": "src/agent/claude.rs",
          "language": "Rust",
          "is_test_file": false,
          "test_type": null,
          "line_count": 245,
          "signature": "use crate::agent::{AgentAdapter, AdapterContext, ...};\nuse crate::logging::append_run_log;\n\npub struct ClaudeAdapter { ... }"
        }
      ]
    }
  ],
  "language_summary": { "Rust": 42, "TypeScript": 15, "Unknown(proto)": 3 },
  "total_source_files": 57,
  "total_test_files": 23,
  "excluded_directories": ["node_modules", "target"]
}
```

The `signature` field contains the first 20 non-blank lines of the file (imports, struct/class declarations, module docstrings). For the large-repo summary tier, signatures are omitted and only directory-level aggregates are included.
```

### 1.3 Test File Detector

**New file: `tools/gardener/src/quality_test_detector.rs`**

Identifies test files by language convention patterns. Classifies each test as `unit`, `integration`, `e2e`, or `unknown` using heuristics from the language registry.

Output schema:
```json
{
  "test_files": [
    {
      "path": "src/backlog_store.rs",
      "language": "Rust",
      "test_type": "unit",
      "detection_method": "content_match",
      "pattern_matched": "#[cfg(test)]"
    },
    {
      "path": "tests/integration/worker_pool_test.rs",
      "language": "Rust",
      "test_type": "integration",
      "detection_method": "directory_convention",
      "pattern_matched": "tests/integration/"
    }
  ],
  "untested_source_files": ["src/quality_scoring.rs", "src/startup.rs"],
  "summary": { "unit": 18, "integration": 4, "e2e": 0, "unknown": 1 }
}
```

### 1.4 Assertion Counter

**New file: `tools/gardener/src/quality_assertion_counter.rs`**

Counts test cases and assertion calls per test file using language-specific patterns from the registry.

Output schema:
```json
{
  "files": [
    {
      "path": "src/backlog_store.rs",
      "language": "Rust",
      "test_count": 8,
      "assertion_count": 24,
      "assertion_density": 3.0
    }
  ],
  "totals": { "tests": 120, "assertions": 380, "avg_density": 3.17 }
}
```

### 1.5 Coverage Parser

**New file: `tools/gardener/src/quality_coverage_parser.rs`**

Searches for known coverage artifacts and parses them into a normalized format. Handles malformed artifacts gracefully — logs parse errors but doesn't fail.

**Coverage merge rules**: When multiple artifacts exist, they are merged with these semantics:
- Same file appearing in multiple artifacts: highest coverage value wins (optimistic merge)
- Path normalization: strip leading `./`, normalize to repo-relative paths
- Duplicate file entries within a single artifact: last entry wins

**Artifact precedence** (for summary stats when formats conflict): Istanbul JSON > lcov > Cobertura XML > Tarpaulin JSON. Precedence only matters for the `summary` field — `per_file` includes all merged data.

Output schema:
```json
{
  "artifacts_found": ["coverage/lcov.info"],
  "artifacts_parsed": ["coverage/lcov.info"],
  "parse_errors": [],
  "coverage_available": true,
  "summary": {
    "lines_covered": 1200,
    "lines_total": 2000,
    "line_coverage_pct": 60.0,
    "source_artifact": "coverage/lcov.info"
  },
  "per_file": [
    { "path": "src/lib.rs", "line_coverage_pct": 72.5, "source_artifact": "coverage/lcov.info" }
  ]
}
```

If no artifacts found: `{ "artifacts_found": [], "coverage_available": false, "parse_errors": [] }`.
If artifacts found but all malformed: `{ "artifacts_found": [...], "artifacts_parsed": [], "parse_errors": [...], "coverage_available": false }`.

### 1.6 Untested File Finder

**New file: `tools/gardener/src/quality_untested_finder.rs`**

For each source file, checks whether a corresponding test file exists — either collocated (inline test module) or in a conventional test directory.

Output schema:
```json
{
  "files": [
    { "path": "src/startup.rs", "language": "Rust", "has_test": false, "search_locations_checked": ["inline", "tests/startup.rs"] },
    { "path": "src/lib.rs", "language": "Rust", "has_test": true, "test_location": "inline" }
  ],
  "untested_count": 12,
  "total_count": 42
}
```

### 1.7 Debt Scanner (TODO/FIXME)

**New file: `tools/gardener/src/quality_debt_scanner.rs`**

Counts `TODO`, `FIXME`, `HACK`, `XXX`, `DEPRECATED` markers per file with surrounding context.

Output schema:
```json
{
  "markers": [
    { "path": "src/worker_pool.rs", "line": 245, "kind": "TODO", "text": "handle edge case for stale leases" }
  ],
  "per_file_counts": { "src/worker_pool.rs": 3, "src/startup.rs": 1 },
  "total": 12
}
```

### 1.8 Doc Scanner

**New file: `tools/gardener/src/quality_doc_scanner.rs`**

Checks for the presence and recency of key documentation files. Reports `last_modified_days_ago` as a raw number — interpretation of staleness is the agent's job (no deterministic thresholds baked in). **Includes full file contents** for steering and convention docs so the agent can assess convention adherence (P1-5) without needing to call tools.

Checked paths: `AGENTS.md`, `CLAUDE.md`, `CODEX.md`, `README.md`, `README`, `CONTRIBUTING.md`, `.editorconfig`, plus any files matching `docs/**/*.md`, `docs/conventions/**`.

Output schema:
```json
{
  "docs": [
    { "path": "AGENTS.md", "exists": true, "last_modified_days_ago": 2, "line_count": 18, "content": "# Gardener Runtime\n..." },
    { "path": "CLAUDE.md", "exists": true, "last_modified_days_ago": 5, "line_count": 42, "content": "# CLAUDE compatibility\n..." },
    { "path": "CONTRIBUTING.md", "exists": false }
  ],
  "steering_doc_count": 2,
  "convention_doc_count": 0,
  "total_doc_files": 5
}
```

Doc content is included in full for files under 500 lines. Larger docs are truncated to the first 500 lines with a `"truncated": true` flag.
```

### 1.9 CI/Lint Detector

**New file: `tools/gardener/src/quality_ci_lint_detector.rs`**

Checks for CI config files, linter configs, pre-commit hooks, and coverage threshold configurations.

Output schema:
```json
{
  "ci": { "found": true, "configs": [".github/workflows/ci.yml"] },
  "linters": { "found": true, "configs": ["clippy.toml", ".rustfmt.toml"] },
  "pre_commit": { "found": true, "configs": [".pre-commit-config.yaml"] },
  "coverage_thresholds": { "found": false }
}
```

### 1.10 Instrumentation Detector

**New file: `tools/gardener/src/quality_instrumentation_detector.rs`**

Scans for logging, tracing, and metrics calls using language-specific patterns from the registry.

Output schema:
```json
{
  "files_with_instrumentation": 28,
  "total_source_files": 42,
  "instrumentation_ratio": 0.67,
  "frameworks_detected": ["tracing", "log"],
  "per_file": [
    { "path": "src/worker_pool.rs", "log_calls": 8, "trace_spans": 2, "metrics_calls": 0 }
  ]
}
```

### 1.11 Evidence Bundle + CLI

**New file: `tools/gardener/src/quality_evidence_bundle.rs`**

Combines all tool outputs into a single versioned evidence bundle:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub schema_version: u32,  // starts at 1
    pub repo_path: String,
    pub collected_at: String,  // ISO 8601
    pub truncated: bool,       // true if detail tier was truncated for large repos
    pub files_included: usize, // number of files with full detail (signatures)
    pub files_total: usize,    // total source files in the repo
    pub tree: TreeWalkerOutput,
    pub tests: TestDetectorOutput,
    pub assertions: AssertionCounterOutput,
    pub coverage: CoverageParserOutput,
    pub untested: UntestedFinderOutput,
    pub debt: DebtScannerOutput,
    pub docs: DocScannerOutput,
    pub ci_lint: CiLintDetectorOutput,
    pub instrumentation: InstrumentationDetectorOutput,
    pub domain_hints: Option<DomainHints>,  // from .gardener/domains.toml if present
    pub package_manifests: Vec<PackageManifest>,  // Cargo.toml, package.json, etc.
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainHints {
    pub domains: Vec<DomainHint>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainHint {
    pub name: String,
    pub paths: Vec<String>,  // glob patterns
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageManifest {
    pub path: String,         // e.g., "packages/auth/package.json"
    pub package_name: String, // e.g., "@myorg/auth"
    pub manifest_type: String, // "cargo", "npm", "swift-package", "pyproject", "go-mod"
}

pub fn collect_evidence_bundle(repo_path: &Path) -> Result<EvidenceBundle> { ... }
```

The `domain_hints` field is populated by reading `.gardener/domains.toml` if it exists. The `package_manifests` field lists all detected package manifests — this is critical for monorepo domain discovery (both by the agent and the deterministic fallback).

**CLI subcommand** (added to `lib.rs`):

```
gardener quality-tools collect <repo-path>     # full evidence bundle (JSON to stdout)
gardener quality-tools tree-walk <repo-path>    # individual tool (for debugging)
gardener quality-tools test-detect <repo-path>  # individual tool (for debugging)
...etc
```

The `collect` command is what the assessment pipeline calls. Individual tool commands exist for debugging and development.

**CLI contract**: The `collect` output is the agent's sole evidence input. The schema is versioned (`schema_version: 1`). Breaking changes increment the version. The agent prompt references the current schema version.

### 1.12 File Organization

**No file moves during implementation.** New files are added alongside existing quality files with a `quality_` prefix:
- `quality_assessment_types.rs`
- `quality_language_registry.rs`
- `quality_tree_walker.rs`
- `quality_test_detector.rs`
- `quality_assertion_counter.rs`
- `quality_coverage_parser.rs`
- `quality_untested_finder.rs`
- `quality_debt_scanner.rs`
- `quality_doc_scanner.rs`
- `quality_ci_lint_detector.rs`
- `quality_instrumentation_detector.rs`
- `quality_evidence_bundle.rs`

Existing `quality_domain_catalog.rs`, `quality_evidence.rs`, `quality_scoring.rs`, `quality_grades.rs` remain untouched. Module consolidation into `quality/` is a separate refactor PR after the feature is stable and tested.

---

## Phase 2: Assessment Agent (P0-1, P0-2, P0-5, P1-1, P1-2, P1-4, P1-5, P1-6)

### 2.1 Assessment Prompt

**New file: `tools/gardener/src/quality_assessment_prompt.rs`**

Build the agent prompt. Structure:

1. **Role**: "You are a code quality assessor. Your job is to evaluate how well this repository supports autonomous agent work."
2. **Evidence bundle**: The full JSON evidence bundle is included inline. The agent does not call any tools — all data is pre-collected.
3. **Domain discovery instructions**: "Using the evidence bundle, identify meaningful domains in this repository. A domain is a cohesive area of functionality. Name each domain based on what it does, not on directory names. Use file signatures, package manifests, and directory structure to understand module boundaries. If `domain_hints` is present in the evidence, use it as a starting point but override based on what you actually find."
4. **Domain-file mapping requirement**: "For each domain you identify, list the source files that belong to it. Every non-test source file in the evidence bundle must be assigned to exactly one domain. Test files, config files, and docs are not included in the mapping. Output this mapping in the `domain_file_map` field."
5. **Assessment instructions**: For each domain, assess test coverage, test quality (assertion density), risk exposure (which untested files are high-risk based on what they do — use file signatures to understand their purpose, e.g., files handling auth, payments, data deletion, or external integrations are high-risk), and convention adherence (read the doc contents in the evidence to understand what conventions are expected, then assess whether file signatures suggest compliance). Weight integration/e2e tests more heavily than unit tests for domains where integration behavior matters. Factor TODO/FIXME density into your assessment. For repo-wide dimensions, assess agent steering, mechanical guardrails, local feedback loop, coverage infrastructure, and documentation quality.
6. **Primary gap selection**: "Identify the single most impactful gap — the one improvement that would most improve agent readiness. This goes in `primary_gap`."
7. **Output contract**: Emit a JSON payload between `<<GARDENER_JSON_START>>` and `<<GARDENER_JSON_END>>` markers (matching existing agent protocol).

### 2.2 Assessment Payload Schema

Uses the types from Phase 1.0 (`quality_assessment_types.rs`). Additionally adds:

```rust
// Added to AssessmentPayload:
pub struct AssessmentPayload {
    pub domains: Vec<DomainAssessment>,
    pub repo_wide: RepoWideAssessment,
    pub deficiencies: Vec<StructuralDeficiency>,
    pub domain_file_map: BTreeMap<String, Vec<String>>,  // domain name → file paths
    pub primary_gap: String,  // single most impactful improvement
    pub languages_detected: Vec<String>,
}
```

### 2.3 Assessment Agent Runner

**New file: `tools/gardener/src/quality_assessment_runner.rs`**

Orchestrates the assessment:

1. Collect evidence bundle via `collect_evidence_bundle(repo_path)`.
2. Build the assessment prompt with the serialized bundle.
3. Execute the agent via `AdapterFactory` (reusing existing agent adapter infrastructure).
4. Parse the `<<GARDENER_JSON_START>>` / `<<GARDENER_JSON_END>>` envelope.
5. Validate the payload: scores in range 0–100, domains non-empty, all **source files** (non-test, non-config) from the evidence bundle appear in `domain_file_map`, deficiency categories are valid. Test files, config files, and docs are not required to be mapped to domains.
6. Return the validated payload.

**Failure handling**:
- **Parse failure** (malformed JSON): Log the raw output, retry once with a "your previous output was malformed, here is the error" prompt appended. If retry also fails, fall back to deterministic pipeline.
- **Validation failure** (scores out of range, missing fields): Attempt lenient recovery — clamp out-of-range scores, fill missing optional fields with defaults. If recovery isn't possible, fall back.
- **Agent timeout**: Fall back to deterministic pipeline.
- **No schema versioning on the agent output** — the schema is defined by the prompt and the Rust deserializer. If deserialization fails, it's a parse failure.

Config integration: New `AppConfig` fields:
- `quality.backend: Option<AgentKind>` — defaults to `seeding.backend`
- `quality.model: Option<String>` — defaults to `seeding.model`
- `quality.max_turns: u32` — default 10 (agent doesn't call tools, just reasons)

### 2.4 Fallback Path (deterministic, no LLM)

When the agent is unavailable (test mode, no API key, agent failure after retry), the fallback produces an `AssessmentPayload` from the **full (non-truncated) evidence bundle** in memory — no dependency on `RepoIntelligenceProfile` or legacy hardcoded domains.

**Deterministic domain discovery**: The fallback uses a multi-strategy approach:
1. **Package manifests first**: If `package_manifests` is non-empty (monorepo), each package becomes a domain named after the package.
2. **Directory clustering otherwise**: Group source files by their second-level directory (e.g., `src/agent/*.rs` → "agent", `lib/auth/*.ts` → "auth"). The first directory level is skipped if it's a common source root (`src/`, `lib/`, `app/`, `pkg/`).
3. **Flat repos**: If all source files are in the root or a single directory, create a single "repository" domain.
Files that don't fit into any domain go into an "other" domain.

**Deterministic scoring**: For each domain:
- `test_coverage`: `(tested_files / total_files) * 100` from the untested finder
- `test_quality`: `min(assertion_density * 20, 100)` from the assertion counter
- `risk_exposure`: `50` (constant — deterministic pipeline can't assess risk)
- `convention_adherence`: `50` (constant — deterministic pipeline can't assess conventions)

For repo-wide:
- `agent_steering`: `min(steering_doc_count * 35, 100)` from doc scanner
- `mechanical_guardrails`: `(has_linter * 40 + has_pre_commit * 30 + has_ci * 30)` from CI/lint detector
- `local_feedback_loop`: `(has_ci * 50 + has_test_files * 50)` from CI/lint + test detector
- `coverage_infrastructure`: `(coverage_available * 70 + has_coverage_thresholds * 30)` from coverage parser + CI/lint
- `documentation_quality`: `min(total_doc_files * 20, 100)` from doc scanner

This fallback is intentionally conservative — it signals "you should run with an agent for a real assessment" by producing middle-of-the-road scores where judgment is needed.

---

## Phase 3: Grade Computation + Rendering (P0-6, P0-7)

### 3.1 Grade Formula

**New file: `tools/gardener/src/quality_grade_compute.rs`**

Pure functions. No I/O. Takes `AssessmentPayload`, returns grades.

```rust
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
    pub deficiencies: Vec<StructuralDeficiency>,  // sorted by severity then category
    pub primary_gap: String,
    pub languages_detected: Vec<String>,
}

pub fn compute_grade_report(payload: AssessmentPayload) -> GradeReport {
    let mut domain_grades: Vec<_> = payload.domains.into_iter()
        .map(|d| {
            let (score, grade) = compute_domain_grade(&d.scores);
            (d, score, grade)
        })
        .collect();
    // Sort by score ascending (worst first) for the report
    domain_grades.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let (repo_score, repo_grade) = compute_repo_grade(&payload.repo_wide);

    // Deficiency ranking: P0 first, then P1, then P2.
    // Within same priority: sort by category name for stability.
    let mut deficiencies = payload.deficiencies;
    deficiencies.sort_by(|a, b| {
        a.severity.cmp(&b.severity)
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
```

### 3.2 Report Renderer

**New file: `tools/gardener/src/quality_grade_renderer.rs`**

Render the output document per the pre-plan spec:

1. **Repo summary** — languages detected, overall readiness grade, primary gap (from `payload.primary_gap` — agent-selected, validated non-empty)
2. **Agent readiness table** — repo-wide dimension scores and grades
3. **Domain coverage table** — per-domain scores, composite score, grade, languages
4. **Structural deficiencies** — ranked list sorted by severity then category, with suggested backlog task titles
5. **Per-domain notes** — agent-written one-sentence assessments from `DomainAssessment.note`
6. Timestamp, TTL metadata, and `assessed_by: "agent" | "deterministic-fallback"` marker

### 3.3 Backlog Task Emission (P0-7, P1-3)

**New file: `tools/gardener/src/quality_backlog_emitter.rs`**

Convert `StructuralDeficiency` items into `BacklogTask` structs and upsert into `BacklogStore`. Uses the **synchronous** `upsert_task` API (matching the actual `BacklogStore` interface — no async).

```rust
pub fn emit_deficiency_tasks(
    store: &BacklogStore,
    deficiencies: &[StructuralDeficiency],
) -> Result<Vec<String>> {
    let mut task_ids = Vec::new();
    for d in deficiencies {
        let task = NewTask {
            kind: TaskKind::QualityGap,
            title: d.suggested_task_title.clone(),
            details: d.suggested_task_details.clone(),
            rationale: d.description.clone(),
            // scope_key uses domain + category for dedup.
            // NOT the agent-generated title (which varies across runs).
            scope_key: format!(
                "quality:{}:{}",
                d.domain.as_deref().unwrap_or("repo"),
                d.category.as_str(),
            ),
            priority: d.severity.clone(),
            source: "quality-grading".to_string(),
            ..Default::default()
        };
        let result = store.upsert_task(task)?;
        task_ids.push(result.task_id.clone());
    }
    Ok(task_ids)
}
```

**Dedup semantics**: The scope_key is `quality:{domain}:{category}`. This means each domain can have at most one task per deficiency category. If the agent reports multiple deficiencies in the same domain+category, they're merged into one task (the last one wins via upsert). This is intentional — it keeps the backlog clean and prevents proliferation of near-duplicate tasks. The task details capture the most recent assessment's specifics.

If a domain has both "no coverage tooling" and "low test coverage", those are different categories (`MissingTooling` vs `CoverageGap`) and produce separate tasks. But two "missing tooling" findings in the same domain collapse into one.

---

## Phase 4: Integration + Startup (P0-8)

### 4.1 Standalone Pipeline Function

**New file: `tools/gardener/src/quality_pipeline.rs`**

A self-contained pipeline function that does not depend on `RepoIntelligenceProfile`, `RuntimeScope`, or any Gardener-specific startup state:

```rust
pub fn run_quality_pipeline(
    repo_path: &Path,
    agent_factory: Option<&AdapterFactory>,
    store: Option<&BacklogStore>,
    config: &QualityConfig,
) -> Result<(String, GradeReport)> {
    // 1. Collect evidence
    let bundle = collect_evidence_bundle(repo_path)?;

    // 2. Run agent assessment (or fallback)
    let payload = if let Some(factory) = agent_factory {
        match run_assessment_agent(factory, &bundle, config) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("Agent assessment failed, using deterministic fallback: {e}");
                deterministic_fallback(&bundle)
            }
        }
    } else {
        deterministic_fallback(&bundle)
    };

    // 3. Compute grades
    let report = compute_grade_report(payload);

    // 4. Render document
    let document = render_grade_document(&report);

    // 5. Emit backlog tasks (if store available)
    if let Some(store) = store {
        emit_deficiency_tasks(store, &report.deficiencies)?;
    }

    Ok((document, report))
}
```

### 4.2 Startup Integration

**Modified file: `tools/gardener/src/startup.rs`**

Update `refresh_quality_report()` to call `run_quality_pipeline()` instead of the old `render_quality_grade_document()`. The `RepoIntelligenceProfile` is no longer required for quality grading — if it exists, it can be used as optional context but the pipeline works without it.

### 4.3 CLI Command

**Modified file: `tools/gardener/src/lib.rs`**

Add a `quality-grade` CLI command:

```
gardener quality-grade <repo-path> [--output <path>] [--no-agent] [--emit-tasks]
```

- `--no-agent`: Force the deterministic fallback
- `--emit-tasks`: Write deficiency tasks to the backlog DB
- Default behavior: print the Markdown document to stdout

---

## Phase 5: P1 Enhancements

### 5.1 Risk-Weighted Assessment (P1-1)

Handled by the agent prompt in Phase 2. The agent reads untested file names from the evidence bundle, infers what they do, and assigns `risk_exposure` scores accordingly. No additional code — this is agent behavior controlled by the prompt.

### 5.2 TODO/FIXME Density (P1-2)

Handled by the debt scanner tool (Phase 1.7) and the agent prompt. The agent sees debt density per file/domain in the evidence bundle and factors it into scoring.

### 5.3 Coverage Gap Tasks (P1-3)

Handled by the backlog emitter (Phase 3.3). The agent emits specific deficiencies for missing coverage tooling (high priority) and low-coverage domains (medium priority, targeting specific untested files).

### 5.4 Integration/E2E Test Bonus (P1-4)

The test detector (Phase 1.3) classifies tests by type (`unit`, `integration`, `e2e`). The agent prompt instructs the agent to weight integration/e2e tests more heavily for relevant domains.

### 5.5 Convention Adherence (P1-5)

The agent reads convention docs found by the doc scanner (Phase 1.8) via the evidence bundle and assesses compliance. This is inherently an LLM task.

### 5.6 Per-Domain Notes (P1-6)

Part of the `DomainAssessment.note` field. The agent writes one-sentence notes per domain as part of its structured output.

---

## Files Changed

### New files
| File | Purpose |
|---|---|
| `quality_assessment_types.rs` | Shared types: AssessmentPayload, Grade, DomainScores, etc. |
| `quality_language_registry.rs` | Multi-language definitions + Unknown fallback |
| `quality_tree_walker.rs` | File tree enumeration with exclusion policy |
| `quality_test_detector.rs` | Test file identification with type classification |
| `quality_assertion_counter.rs` | Test quality metrics |
| `quality_coverage_parser.rs` | Coverage artifact parsing with merge rules |
| `quality_untested_finder.rs` | Untested file detection |
| `quality_debt_scanner.rs` | TODO/FIXME scanning |
| `quality_doc_scanner.rs` | Documentation presence check |
| `quality_ci_lint_detector.rs` | CI/lint config detection |
| `quality_instrumentation_detector.rs` | Observability scanning |
| `quality_evidence_bundle.rs` | Evidence bundle aggregation + CLI |
| `quality_assessment_prompt.rs` | Agent prompt builder |
| `quality_assessment_runner.rs` | Agent orchestration + fallback |
| `quality_grade_compute.rs` | Grade formula (pure functions) |
| `quality_grade_renderer.rs` | Markdown report rendering |
| `quality_backlog_emitter.rs` | Deficiency → backlog task conversion |
| `quality_pipeline.rs` | Self-contained pipeline orchestrator |

All new files in `tools/gardener/src/`.

### Modified files
| File | Changes |
|---|---|
| `tools/gardener/src/lib.rs` | New module declarations, `quality-tools` + `quality-grade` CLI subcommands |
| `tools/gardener/src/startup.rs` | `refresh_quality_report()` calls new pipeline |
| `tools/gardener/src/config.rs` | New `quality` config section |

### Unchanged (legacy, removed later)
| File | Status |
|---|---|
| `quality_domain_catalog.rs` | Untouched — removed in a follow-up refactor PR |
| `quality_evidence.rs` | Untouched — removed in a follow-up refactor PR |
| `quality_scoring.rs` | Untouched — removed in a follow-up refactor PR |
| `quality_grades.rs` | Untouched — removed in a follow-up refactor PR |

---

## Testing Strategy

### Unit tests
- **Language registry**: Verify extension → language mapping for all built-in languages + Unknown fallback. Test shebang detection.
- **Each tool**: Feed a known directory structure via `tempdir`, assert correct JSON output. Include edge cases: empty repos, repos with only test files, repos with only generated code.
- **Grade formula**: Property-based tests — all scores in [0, 100] produce valid grades, boundary values map to expected grades (92 → B+, 93 → A, etc.).
- **Backlog emitter**: Use `BacklogStore` with in-memory SQLite, verify correct `TaskKind`, scope_key uniqueness, deduplication on re-upsert.
- **Evidence bundle**: Verify schema_version is present, all sub-tool outputs are populated, collected_at is valid ISO 8601.
- **Exclusion policy**: Verify `node_modules/`, `target/`, etc. are excluded from tree walks.

### Integration tests
- **Tool CLI**: Run `gardener quality-tools collect .` against the gardener repo itself, assert non-empty output with expected Rust language detection.
- **Full pipeline (no-agent)**: Run `gardener quality-grade . --no-agent`, verify the output document has all 6 required sections.
- **Full pipeline (with agent)**: Run `gardener quality-grade .` against the gardener repo, verify structured output parses and grades are reasonable.

### Validation against known repos
- **Gardener** (Rust): Should detect Rust as primary language, find existing inline tests, identify missing coverage tooling.
- **brad-os** (TypeScript + Swift): Should detect both languages, parse Istanbul coverage if present, find Swift test targets. Should report Unknown for any non-TS/Swift files.

---

## Success Criteria

1. `gardener quality-grade <any-repo-path>` produces a complete Markdown report with all 6 sections specified in the pre-plan output format.
2. The agent discovers domains without hardcoded maps — running against different repos produces different domain lists.
3. Multi-language detection works: running against a mixed-language repo correctly identifies and reports all languages, including Unknown for unrecognized ones.
4. Structural deficiencies are emitted as backlog tasks with appropriate priorities and domain-specific scope keys.
5. The deterministic fallback (`--no-agent`) produces a valid report using only evidence bundle data — no dependency on `RepoIntelligenceProfile`.
6. Grade computation is transparent — the formula is a pure function from scores to grades, documented in the output.
7. The 9-level grading scale (A through F with +/-) is used consistently.
8. No hardcoded domain names, no language-specific scoring logic in the grade formula, no repo-specific knowledge.
9. Agent failure (timeout, parse error, validation failure) gracefully degrades to the deterministic fallback with a logged warning.
10. Evidence bundle is reproducible — running `quality-tools collect` twice on the same repo state produces identical output (excluding the `collected_at` timestamp, which is exempt from comparison).

---

## Non-Goals (Deferred to P2)

- **Trend tracking** (P2-1): Storing previous grade snapshots for directional movement.
- **Cross-platform awareness** (P2-2): Per-platform test health within domains.
- **Coverage file generation**: We parse existing artifacts — we don't run coverage tools.
- **`.gardener/domains.toml` hint file**: The evidence bundle reads this file if present and passes it through to the agent, but no special handling or validation is required. The agent uses it as optional input. This is in-scope as a pass-through; what's deferred is building tooling to *create or manage* the hint file.
- **Module consolidation**: Moving `quality_*.rs` files into a `quality/` directory is a separate refactor PR after the feature is stable.

---

## Implementation Order

1. **Phase 1.0** — shared types (`quality_assessment_types.rs`). This unblocks both Phase 2 and Phase 3.
2. **Phase 1.1–1.11** — evidence tools + bundle + CLI. Start with language registry + tree walker + exclusion policy, then build remaining tools. Each tool is independently testable. Finish with the bundle aggregator and CLI.
3. **Phase 3** — grade computation + rendering + backlog emitter. Pure Rust, no LLM dependency. Test with hand-crafted `AssessmentPayload` structs.
4. **Phase 2** — assessment agent prompt + runner + fallback. Depends on Phase 1 tools (for evidence bundle) and Phase 1.0 types (for payload).
5. **Phase 4** — integration: pipeline orchestrator, startup wiring, CLI command.
6. **Phase 5** — already embedded in Phases 1–2, no separate work needed.

Estimated new Rust code: ~3,000–4,000 lines across 18 new files and 3 modified files.
