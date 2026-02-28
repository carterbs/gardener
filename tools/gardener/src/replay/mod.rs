//! Session recording and replay for deterministic regression testing.
//!
//! # Overview
//!
//! Gardener can record a live run at three boundaries:
//! 1. **ProcessRunner** – every subprocess call and its response
//! 2. **Agent turn** – the `StepResult` returned to the FSM
//! 3. **BacklogStore** – initial task snapshot + mutation log
//!
//! The recording is a JSONL file (one `RecordEntry` per line).  A replay
//! substitutes fake implementations pre-seeded from the recording, so the
//! full FSM runs deterministically in tests without any real I/O.

pub mod recording;
pub mod recorder;
pub mod replayer;
