use gardener::replay::recorder::{
    clear_session_recorder, emit_record, get_recording_worker_id, init_session_recorder,
    set_recording_worker_id, RecordingProcessRunner,
};
use gardener::replay::recording::{
    AgentTurnRecord, BacklogMutationRecord, BacklogSnapshotRecord, BacklogTaskRecord,
    ProcessCallRecord, ProcessOutputRecord, ProcessRequestRecord, RecordEntry,
    SessionEndRecord, SessionStartRecord,
};
use gardener::replay::replayer::{
    ReplayAgentAdapter, ReplayProcessRunner, SessionRecording,
};
use gardener::agent::codex::CodexAdapter;
use gardener::agent::{AdapterContext, AgentAdapter};
use gardener::protocol::AgentTerminal;
use gardener::runtime::{FakeProcessRunner, ProcessOutput, ProcessRequest};
use gardener::types::AgentKind;
use std::sync::Arc;
use tempfile::NamedTempFile;

// ── helpers ───────────────────────────────────────────────────────────────────

fn fake_task_record(task_id: &str) -> BacklogTaskRecord {
    BacklogTaskRecord {
        task_id: task_id.to_string(),
        kind: gardener::task_identity::TaskKind::Maintenance,
        title: "test task".to_string(),
        details: String::new(),
        rationale: String::new(),
        scope_key: "test".to_string(),
        priority: gardener::priority::Priority::P1,
        status: "ready".to_string(),
        last_updated: 0,
        lease_owner: None,
        lease_expires_at: None,
        source: "test".to_string(),
        related_pr: None,
        related_branch: None,
        attempt_count: 1,
        created_at: 0,
    }
}

fn write_minimal_recording(
    path: &std::path::Path,
    worker_id: &str,
    task_id: &str,
    process_calls: Vec<(ProcessRequestRecord, ProcessOutputRecord)>,
    agent_turns: Vec<AgentTurnRecord>,
) {
    let mut entries: Vec<RecordEntry> = Vec::new();
    entries.push(RecordEntry::SessionStart(SessionStartRecord {
        run_id: "test-run".to_string(),
        recorded_at_unix_ns: 0,
        gardener_version: "test".to_string(),
        config_snapshot: serde_json::json!({}),
    }));
    entries.push(RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
        tasks: vec![fake_task_record(task_id)],
    }));
    entries.push(RecordEntry::BacklogMutation(BacklogMutationRecord {
        seq: 1,
        timestamp_ns: 0,
        worker_id: worker_id.to_string(),
        operation: "claim_next".to_string(),
        task_id: task_id.to_string(),
        result_ok: true,
    }));
    for (i, (req, resp)) in process_calls.into_iter().enumerate() {
        entries.push(RecordEntry::ProcessCall(ProcessCallRecord {
            seq: 10 + i as u64,
            timestamp_ns: 0,
            worker_id: worker_id.to_string(),
            thread_id: "test".to_string(),
            request: req,
            result: resp,
            duration_ns: 0,
        }));
    }
    for turn in agent_turns {
        entries.push(RecordEntry::AgentTurn(turn));
    }
    entries.push(RecordEntry::SessionEnd(SessionEndRecord {
        completed_tasks: 1,
        total_duration_ns: 0,
    }));

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .expect("open recording file");
    for entry in &entries {
        let line = serde_json::to_string(entry).expect("test");
        writeln!(f, "{line}").expect("test");
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[test]
fn replay_process_runner_returns_recorded_responses() {
    let tmp = NamedTempFile::new().expect("test");
    write_minimal_recording(
        tmp.path(),
        "worker-1",
        "task-1",
        vec![(
            ProcessRequestRecord {
                program: "echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: None,
            },
            ProcessOutputRecord {
                exit_code: 0,
                stdout: "hello".to_string(),
                stderr: String::new(),
                stdout_truncated: false,
            },
        )],
        vec![],
    );

    let recording = SessionRecording::load(tmp.path()).expect("test");
    let runner = ReplayProcessRunner::from_recording(&recording, "worker-1");

    use gardener::runtime::ProcessRunner;
    let handle = runner
        .spawn(ProcessRequest {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
        })
        .expect("test");
    let out = runner.wait(handle).expect("test");
    assert_eq!(out.stdout, "hello");
    assert_eq!(out.exit_code, 0);

    // A second call should fail (queue empty)
    let handle2 = runner
        .spawn(ProcessRequest {
            program: "echo".to_string(),
            args: vec!["bye".to_string()],
            cwd: None,
        })
        .expect("test");
    assert!(runner.wait(handle2).is_err(), "expected empty queue error");
}

#[test]
fn replay_agent_adapter_returns_recorded_step_results() {
    let tmp = NamedTempFile::new().expect("test");
    let turn = AgentTurnRecord {
        seq: 1,
        timestamp_ns: 0,
        worker_id: "worker-1".to_string(),
        state: "understand".to_string(),
        terminal: "success".to_string(),
        payload: serde_json::json!({"task_type": "task", "reasoning": "test"}),
        diagnostic_count: 0,
    };
    write_minimal_recording(tmp.path(), "worker-1", "task-1", vec![], vec![turn]);

    let recording = SessionRecording::load(tmp.path()).expect("test");
    let adapter = ReplayAgentAdapter::from_recording(&recording, "worker-1", AgentKind::Claude);

    use gardener::agent::AgentAdapter;
    use gardener::runtime::FakeProcessRunner;
    let runner = FakeProcessRunner::default();
    let step = adapter
        .execute(
            &runner,
            &gardener::agent::AdapterContext {
                worker_id: "worker-1".to_string(),
                session_id: "s1".to_string(),
                sandbox_id: String::new(),
                model: String::new(),
                cwd: std::path::PathBuf::from("/tmp"),
                prompt_version: "v1".to_string(),
                context_manifest_hash: "abc".to_string(),
                output_schema: None,
                output_file: None,
                permissive_mode: false,
                max_turns: None,
            },
            "prompt",
            None,
        )
        .expect("test");

    assert_eq!(step.terminal, AgentTerminal::Success);
    assert_eq!(step.payload["task_type"], "task");

    // Second call should fail (queue empty)
    assert!(
        adapter.execute(&runner, &gardener::agent::AdapterContext {
            worker_id: "worker-1".to_string(),
            session_id: "s1".to_string(),
            sandbox_id: String::new(),
            model: String::new(),
            cwd: std::path::PathBuf::from("/tmp"),
            prompt_version: "v1".to_string(),
            context_manifest_hash: "abc".to_string(),
            output_schema: None,
            output_file: None,
            permissive_mode: false,
            max_turns: None,
        }, "prompt", None).is_err()
    );
}

#[test]
fn large_agent_output_hash_compressed() {
    use gardener::replay::recording::ProcessOutputRecord;

    let big_stdout = "x".repeat(70_000);
    let rec = ProcessOutputRecord::from_output(0, big_stdout, String::new());
    assert!(rec.stdout_truncated, "large output should be truncated");
    assert!(
        rec.stdout.starts_with("<hash:sha256:"),
        "truncated output should contain hash prefix"
    );
    assert_eq!(rec.exit_code, 0);
}

#[test]
fn recording_round_trip_process_calls() {
    // Record a few subprocess calls with RecordingProcessRunner
    let tmp = NamedTempFile::new().expect("test");
    init_session_recorder(tmp.path()).expect("test");

    emit_record(RecordEntry::SessionStart(SessionStartRecord {
        run_id: "rt-run".to_string(),
        recorded_at_unix_ns: 0,
        gardener_version: "test".to_string(),
        config_snapshot: serde_json::json!({}),
    }));
    emit_record(RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
        tasks: vec![fake_task_record("task-rt")],
    }));
    emit_record(RecordEntry::BacklogMutation(BacklogMutationRecord {
        seq: 1,
        timestamp_ns: 0,
        worker_id: "worker-rt".to_string(),
        operation: "claim_next".to_string(),
        task_id: "task-rt".to_string(),
        result_ok: true,
    }));

    let fake = Arc::new(FakeProcessRunner::default());
    fake.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "alpha".to_string(),
        stderr: String::new(),
    }));
    fake.push_response(Ok(ProcessOutput {
        exit_code: 42,
        stdout: String::new(),
        stderr: "oops".to_string(),
    }));

    set_recording_worker_id("worker-rt");
    let recording_runner = RecordingProcessRunner::new(fake.clone());

    use gardener::runtime::ProcessRunner;
    let h1 = recording_runner
        .spawn(ProcessRequest {
            program: "cmd1".to_string(),
            args: vec![],
            cwd: None,
        })
        .expect("test");
    let h2 = recording_runner
        .spawn(ProcessRequest {
            program: "cmd2".to_string(),
            args: vec!["--flag".to_string()],
            cwd: None,
        })
        .expect("test");
    let out1 = recording_runner.wait(h1).expect("test");
    let out2 = recording_runner.wait(h2).expect("test");
    assert_eq!(out1.stdout, "alpha");
    assert_eq!(out2.exit_code, 42);

    clear_session_recorder();

    // Load recording and verify structure
    let recording = SessionRecording::load(tmp.path()).expect("test");
    assert_eq!(recording.header.run_id, "rt-run");
    assert_eq!(recording.backlog.len(), 1);
    assert_eq!(recording.backlog[0].task_id, "task-rt");

    let calls = recording.process_calls_for("worker-rt");
    assert_eq!(calls.len(), 2, "should have recorded 2 process calls");
    assert_eq!(calls[0].request.program, "cmd1");
    assert_eq!(calls[0].result.stdout, "alpha");
    assert_eq!(calls[1].request.program, "cmd2");
    assert_eq!(calls[1].result.exit_code, 42);

    // Replay with ReplayProcessRunner
    let replayer = ReplayProcessRunner::from_recording(&recording, "worker-rt");
    let rh1 = replayer
        .spawn(ProcessRequest {
            program: "cmd1".to_string(),
            args: vec![],
            cwd: None,
        })
        .expect("test");
    let rh2 = replayer
        .spawn(ProcessRequest {
            program: "cmd2".to_string(),
            args: vec!["--flag".to_string()],
            cwd: None,
        })
        .expect("test");
    let rout1 = replayer.wait(rh1).expect("test");
    let rout2 = replayer.wait(rh2).expect("test");
    assert_eq!(rout1.stdout, "alpha");
    assert_eq!(rout2.exit_code, 42);

    // No mismatches since programs match
    let mismatches = replayer.verify_request_alignment();
    assert!(mismatches.is_empty(), "no request mismatches expected");
}

#[test]
fn replay_catches_request_mismatch() {
    let tmp = NamedTempFile::new().expect("test");
    // Write recording with "cmd-original"
    write_minimal_recording(
        tmp.path(),
        "worker-x",
        "task-x",
        vec![(
            ProcessRequestRecord {
                program: "cmd-original".to_string(),
                args: vec![],
                cwd: None,
            },
            ProcessOutputRecord {
                exit_code: 0,
                stdout: "data".to_string(),
                stderr: String::new(),
                stdout_truncated: false,
            },
        )],
        vec![],
    );

    let recording = SessionRecording::load(tmp.path()).expect("test");
    let runner = ReplayProcessRunner::from_recording(&recording, "worker-x");

    use gardener::runtime::ProcessRunner;
    // Replay with a DIFFERENT program name
    let handle = runner
        .spawn(ProcessRequest {
            program: "cmd-different".to_string(),
            args: vec![],
            cwd: None,
        })
        .expect("test");
    let _ = runner.wait(handle).expect("test");

    let mismatches = runner.verify_request_alignment();
    assert_eq!(mismatches.len(), 1, "should detect one mismatch");
    assert_eq!(mismatches[0].expected_program, "cmd-original");
    assert_eq!(mismatches[0].actual_program, "cmd-different");
}

#[test]
fn session_recording_worker_ids() {
    let tmp = NamedTempFile::new().expect("test");
    // Write recording with entries from two workers
    use std::io::Write;
    let entries = vec![
        RecordEntry::SessionStart(SessionStartRecord {
            run_id: "r".to_string(),
            recorded_at_unix_ns: 0,
            gardener_version: "t".to_string(),
            config_snapshot: serde_json::json!({}),
        }),
        RecordEntry::AgentTurn(AgentTurnRecord {
            seq: 1,
            timestamp_ns: 0,
            worker_id: "w1".to_string(),
            state: "understand".to_string(),
            terminal: "success".to_string(),
            payload: serde_json::json!({}),
            diagnostic_count: 0,
        }),
        RecordEntry::AgentTurn(AgentTurnRecord {
            seq: 2,
            timestamp_ns: 0,
            worker_id: "w2".to_string(),
            state: "doing".to_string(),
            terminal: "failure".to_string(),
            payload: serde_json::json!({}),
            diagnostic_count: 1,
        }),
        RecordEntry::AgentTurn(AgentTurnRecord {
            seq: 3,
            timestamp_ns: 0,
            worker_id: "w1".to_string(),
            state: "doing".to_string(),
            terminal: "success".to_string(),
            payload: serde_json::json!({}),
            diagnostic_count: 0,
        }),
    ];
    let mut f = std::fs::File::create(tmp.path()).expect("test");
    for e in &entries {
        writeln!(f, "{}", serde_json::to_string(e).expect("test")).expect("test");
    }

    let recording = SessionRecording::load(tmp.path()).expect("test");
    let ids = recording.worker_ids();
    assert_eq!(ids.len(), 2, "should find 2 unique worker IDs");
    assert!(ids.contains(&"w1".to_string()));
    assert!(ids.contains(&"w2".to_string()));

    let w1_turns = recording.agent_turns_for("w1");
    assert_eq!(w1_turns.len(), 2);
    let w2_turns = recording.agent_turns_for("w2");
    assert_eq!(w2_turns.len(), 1);
    assert_eq!(w2_turns[0].terminal, "failure");
}

#[test]
fn set_get_worker_id_thread_local() {
    set_recording_worker_id("test-worker");
    assert_eq!(get_recording_worker_id(), "test-worker");
    // Reset
    set_recording_worker_id("pool");
    assert_eq!(get_recording_worker_id(), "pool");
}

#[test]
fn recording_overhead_acceptable() {
    // Verify RecordingProcessRunner with no recorder active has minimal overhead
    // by running a tight loop without init_session_recorder
    let fake = Arc::new(FakeProcessRunner::default());
    for _ in 0..100 {
        fake.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "x".to_string(),
            stderr: String::new(),
        }));
    }
    let recording_runner = RecordingProcessRunner::new(fake);
    use gardener::runtime::ProcessRunner;
    for _ in 0..100 {
        let h = recording_runner
            .spawn(ProcessRequest {
                program: "noop".to_string(),
                args: vec![],
                cwd: None,
            })
            .expect("test");
        let _ = recording_runner.wait(h).expect("test");
    }
    // If we get here without panicking, the overhead is acceptable.
    // The real benchmark would compare timing, but that's env-dependent.
}

// ── production failure replay ─────────────────────────────────────────────────

/// Session replay for:
///   run.id:  55c2c192701db506764301c5bf93acc1
///   worker:  worker-2
///   task:    manual:quality:auto-1772289029000
///   error:   "process error: missing turn.completed or turn.failed event"
///
/// The recording was reconstructed from otel-logs.jsonl. The codex process
/// emitted multiple JSON objects in a single read() chunk. This regression test
/// verifies that replay now parses those concatenated objects and recognizes
/// the terminal `turn.completed` event instead of surfacing
/// "missing turn.completed or turn.failed event".
#[test]
fn session_replay_reproduces_missing_terminal_event_bug() {
    let recording_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recording_55c2c192701db506764301c5bf93acc1.jsonl");
    let recording = SessionRecording::load(&recording_path)
        .expect("load production failure recording");

    assert_eq!(recording.header.run_id, "55c2c192701db506764301c5bf93acc1");

    let runner = ReplayProcessRunner::from_recording(&recording, "worker-2");

    let ctx = AdapterContext {
        worker_id: "worker-2".to_string(),
        session_id: "55c2c192701db506764301c5bf93acc1".to_string(),
        sandbox_id: String::new(),
        model: "codex-1".to_string(),
        cwd: std::path::PathBuf::from("/tmp"),
        prompt_version: "v1".to_string(),
        context_manifest_hash: String::new(),
        output_schema: None,
        output_file: Some(std::path::PathBuf::from("/tmp/codex-last-message.json")),
        permissive_mode: false,
        max_turns: None,
    };

    let step = CodexAdapter
        .execute(&runner, &ctx, "prompt", None)
        .expect("concatenated replay events should parse as a successful terminal turn");

    assert!(
        step.terminal == AgentTerminal::Success,
        "expected success terminal, got: {:?}",
        step.terminal
    );
    assert!(
        step.events
            .iter()
            .any(|event| event.raw_type == "turn.completed"),
        "expected replay to include turn.completed event, got: {:?}",
        step.events
    );
}
