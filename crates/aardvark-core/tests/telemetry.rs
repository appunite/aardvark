use aardvark_core::outcome::{
    Diagnostics, ExecutionOutcome, FailureKind, FilesystemViolation, NetworkDeniedHost,
    NetworkHostContact, OutcomeStatus, ResultPayload,
};
use aardvark_core::SandboxTelemetry;

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
    };

    let telemetry: SandboxTelemetry = diagnostics.to_telemetry();
    assert_eq!(telemetry.cpu_ms_used, Some(42));
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
