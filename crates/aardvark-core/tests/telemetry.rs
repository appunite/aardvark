use aardvark_core::outcome::{
    Diagnostics, ExecutionOutcome, FailureKind, FilesystemViolation, NetworkDeniedHost,
    NetworkHostContact, OutcomeStatus, ResultPayload,
};
use aardvark_core::persistent::PoolStats;
use aardvark_core::{PoolTelemetry, SandboxTelemetry};

#[test]
fn diagnostics_to_telemetry_maps_fields() {
    let diagnostics = Diagnostics {
        stdout: "hello".into(),
        stderr: String::new(),
        exception: None,
        cpu_ms_used: Some(42),
        filesystem_bytes_written: Some(1024),
        network_hosts_contacted: vec![NetworkHostContact {
            host: "allowed.test".into(),
            port: Some(443),
            https: true,
        }],
        network_hosts_blocked: vec![NetworkDeniedHost {
            host: "denied.test".into(),
            port: Some(80),
            https_required: true,
            reason: "scheme-not-allowed".into(),
        }],
        filesystem_violations: vec![FilesystemViolation {
            path: Some("/session/tmp.txt".into()),
            message: "quota exceeded".into(),
        }],
        reset: None,
        queue_wait_ms: Some(12),
        prepare_ms: Some(34),
        cleanup_ms: Some(56),
        py_heap_kib: Some(2048),
        rss_kib_before: Some(4096),
        rss_kib_after: Some(6144),
    };

    let telemetry: SandboxTelemetry = diagnostics.to_telemetry();
    assert_eq!(telemetry.cpu_ms_used, Some(42));
    assert_eq!(telemetry.queue_wait_ms, Some(12));
    assert_eq!(telemetry.prepare_ms, Some(34));
    assert_eq!(telemetry.cleanup_ms, Some(56));
    assert_eq!(telemetry.memory.py_heap_kib, Some(2048));
    assert_eq!(telemetry.memory.rss_kib_before, Some(4096));
    assert_eq!(telemetry.memory.rss_kib_after, Some(6144));
    assert_eq!(telemetry.filesystem.bytes_written, Some(1024));
    assert_eq!(telemetry.network.allowed[0].host, "allowed.test");
    assert_eq!(telemetry.network.blocked[0].host, "denied.test");
    assert!(telemetry.has_policy_violations());
}

#[test]
fn execution_outcome_exposes_telemetry() {
    let diagnostics = Diagnostics {
        cpu_ms_used: Some(5),
        ..Diagnostics::default()
    };
    let outcome = ExecutionOutcome::success(ResultPayload::None, diagnostics);
    let telemetry = outcome.sandbox_telemetry();
    assert_eq!(telemetry.cpu_ms_used, Some(5));
    assert!(!telemetry.has_policy_violations());

    let failure = ExecutionOutcome::failure(
        FailureKind::TimeoutExceeded { requested_ms: 10 },
        Diagnostics {
            filesystem_violations: vec![FilesystemViolation {
                path: None,
                message: "read-only".into(),
            }],
            ..Diagnostics::default()
        },
    );
    let failure_telemetry = failure.sandbox_telemetry();
    assert!(failure_telemetry.has_policy_violations());
    assert_eq!(failure_telemetry.network.allowed.len(), 0);
    match failure.status {
        OutcomeStatus::Failure(FailureKind::TimeoutExceeded { requested_ms }) => {
            assert_eq!(requested_ms, 10);
        }
        _ => panic!("unexpected outcome status"),
    }
}

#[test]
fn pool_stats_into_pool_telemetry() {
    let stats = PoolStats {
        total: 3,
        idle: 1,
        busy: 2,
        waiting: 4,
        invocations: 10,
        average_queue_wait_ms: 42.5,
        queue_wait_p50_ms: Some(30.0),
        queue_wait_p95_ms: Some(70.0),
        quarantine_events: 1,
        quarantine_heap_hits: 1,
        quarantine_rss_hits: 0,
        scaledown_events: 2,
    };

    let telemetry = PoolTelemetry::from(&stats);
    assert_eq!(telemetry.total_isolates, 3);
    assert_eq!(telemetry.busy_isolates, 2);
    assert_eq!(telemetry.waiting_calls, 4);
    assert_eq!(telemetry.queue_wait_p95_ms, Some(70.0));
    assert_eq!(telemetry.quarantine_events, 1);
    assert_eq!(telemetry.quarantine_heap_hits, 1);
    assert_eq!(telemetry.quarantine_rss_hits, 0);
    assert_eq!(telemetry.scaledown_events, 2);
}
