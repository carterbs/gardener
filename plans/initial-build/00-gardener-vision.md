# Gardener Vision

## The Problem

Most repositories are not agent-ready. They weren't designed to be.

They have ad-hoc shell scripts that behave differently on different machines, test suites that pass locally and fail in CI for reasons no one fully understands, implicit conventions that live in someone's head or a stale Confluence page, and a backlog that's just a vague sense of unease. Humans have been navigating this successfully for years because they can ask questions, remember context, and improvise when something breaks.

Agents can't. From an agent's perspective, anything it can't find in its context effectively doesn't exist. Ambiguity isn't a speedbump—it's a wall.

The result is that most brownfield repositories are essentially hostile to autonomous work. You can point an agent at them, but you're going to spend most of your time unblocking it, cleaning up the damage from misunderstood instructions, and wondering why it keeps making the same category of mistake. That's not a model problem. That's an environment problem.

## What Gardener Is

Gardener is a treatment for that environment problem.

You run it against a real repository—one that's been accumulating entropy for months or years—and it systematically builds the scaffolding that makes autonomous agent work possible. Not in theory. In practice. On your actual codebase, with your actual tech stack, against your actual test suite.

It doesn't ask you to rewrite the world first. It meets the repo where it is, assesses the gap between where things are and where they need to be, and starts closing it.

## What "Agent-Ready" Actually Means

The OpenAI Codex team learned this the hard way: the bottleneck wasn't model capability. It was environment legibility. Before they could ship code at scale without humans writing a line, they had to do unglamorous, systematic work to make the repository a place an agent could reason about confidently.

That means several concrete things.

**Deterministic tooling.** The build either passes or fails. The lint either triggers or it doesn't. Flaky tests get fixed or quarantined. An agent needs to be able to run a check, get a result, act on that result, and trust that re-running the same check will tell it whether the action worked. Anything less than that turns every workflow into a guessing game.

Determinism also applies to agent orchestration I/O. If a worker backend supports machine-readable streams (for example, `codex exec --json` JSONL events with explicit terminal states), Gardener should consume that protocol directly instead of scraping human-formatted text.

**Architecture that can be verified, not just described.** You can write a doc that says "keep your services out of the UI layer." What works at scale—what actually keeps an agent from drifting into bad patterns across thousands of lines of generated code—is a custom linter that catches the violation at commit time and emits an error message designed to inject remediation instructions directly into agent context. Rules that live only in docs rot. Rules encoded in tooling compound.

**Repository knowledge as the system of record.** If the architectural decision lives in a Slack thread, it doesn't exist for an agent. Quality grades, known tech debt, architectural constraints, domain boundaries, test coverage status—all of it needs to be in the repo, structured, versioned, and kept fresh. Not a massive instruction monolith that an agent can't navigate, but a map with clear pointers to the right sources of truth at the right level of detail.

**Coverage that means something.** An agent making changes in code that has no tests is operating without a feedback loop. It can't tell whether it broke something. It can't recover confidently from a regression. Coverage gates—real ones, enforced in CI, with no exceptions—are what turn automated test suites from documentation into guardrails.

**A backlog that reflects reality.** A well-structured backlog seeded from actual quality evidence, graded by domain, prioritized by impact, and expressed in task shapes agents can execute is a different thing entirely from a pile of issues that accumulated over two years. The first thing is a work queue. The second is a graveyard.

## How Gardener Builds This

The first time Gardener runs against a repository, it doesn't assume it understands the environment. It starts by listening.

Gardener runs an agent-driven discovery pass — scanning for the signals that determine whether a repository is already equipped for autonomous work: agent steering documents, architecture docs, custom linters, CI configuration, test infrastructure, and coverage gates. It forms initial hypotheses about where the repository stands against each of those dimensions. Then it asks.

A short interactive interview surfaces what the file scan can't discover on its own: the architecture document that predates the current folder structure, the linter that lives in an unusual path, the test suite that exists but isn't wired to CI yet. After collecting answers, Gardener presents its full understanding back to the operator — "here is what I've learned about this repository before I grade it" — and waits for confirmation or correction.

The result is a Repo Intelligence Profile: a versioned, committed document that maps the repository's current state against the five agent-readiness dimensions described in this vision. That profile becomes the foundation of the first quality grade, and it informs every backlog seed and quality report that follows. Gardener isn't just grading domains in isolation — it's grading the environment those agents will have to work inside.

Gardener runs as a persistent orchestrator. In subsequent runs, it audits the current state: it discovers the codebase's domains, grades each one against a consistent rubric anchored in the profile baseline, reconciles any hanging work from previous runs, and uses what it learns to seed an initial backlog of high-value tasks.

Then it works. It spawns workers, each one moving through an explicit state machine—understand the task, plan if needed, do the work, open a PR, get review, merge. Every step is deterministic. Every output is typed and validated. Workers don't freestyle their way through ambiguous state; they follow a protocol with clear failure modes and recovery paths.

While it runs, Gardener learns. Post-merge analysis captures what worked. Postmortems on failed tasks extract the pattern that caused the failure. That knowledge feeds back into how subsequent tasks are prompted—not as vague instruction inflation, but as structured evidence that influences context ranking and task selection.

The coverage floor rises. The quality grades improve. The linters catch categories of mistakes before they spread. The architecture documentation stays in sync with the code. The repo gets harder to break and easier to navigate.

## The Outcome

After a Gardener run, the repository is a different kind of place.

An agent dropped into it cold can read the AGENTS.md, follow the pointers, understand the domain structure, run the validation command, and trust what it gets back. It can pick a task from the backlog and know that the task is real, sized right, and unambiguous about success criteria. It can make a change and know that the test suite will tell it whether the change worked.

The humans who operate that repository are no longer the load-bearing wall. They're steering. The agents are executing.

That's the field Gardener is growing.
