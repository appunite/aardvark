use aardvark_core::{
    config::PyRuntimeConfig,
    invocation::{FieldDescriptor, InvocationDescriptor},
    outcome::{FailureKind, OutcomeStatus, ResultPayload},
    strategy::{
        JsonInvocationStrategy, RawCtxBindingBuilder, RawCtxInput, RawCtxInvocationStrategy,
        RawCtxMetadata, RawCtxPublishBuilder,
    },
    Bundle, PyRuntime, Result, RuntimeLanguage,
};
use bytes::Bytes;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn default_entrypoint() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    return { greeting: "hello" };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, manifest_opt) = runtime.prepare_session_with_manifest(bundle)?;
    assert!(manifest_opt.is_some(), "manifest should be detected");
    assert_eq!(
        session.descriptor().language,
        Some(RuntimeLanguage::JavaScript)
    );
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!({ "greeting": "hello" }));
        }
        other => panic!("expected json payload, got {:?}", other),
    }
    Ok(())
}

#[test]
fn console_diagnostics() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    console.log("hello js stdout");
    console.error("hello js stderr");
    return "ok";
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    assert!(outcome.is_success(), "expected success outcome");

    let diagnostics = &outcome.diagnostics;
    assert!(
        diagnostics.stdout.contains("hello js stdout"),
        "stdout should capture console.log output: {:?}",
        diagnostics.stdout
    );
    assert!(
        diagnostics.stderr.contains("hello js stderr"),
        "stderr should capture console.error output: {:?}",
        diagnostics.stderr
    );

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => assert_eq!(value, &json!("ok")),
        Some(ResultPayload::Text(text)) => assert_eq!(text, "ok"),
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn shared_buffers_payload() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    const data = new Uint8Array([1, 2, 3, 4]);
    globalThis.__aardvarkPublishBuffer("js-buffer", data, { dtype: "u8" });
    return null;
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "js-buffer");
            assert_eq!(handle.length, 4);
            let bytes = handle
                .as_slice()
                .expect("shared buffer should retain data for inspection");
            assert_eq!(bytes, &[1, 2, 3, 4]);
            let dtype = handle
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("dtype"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(dtype, "u8");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    let diagnostics = &outcome.diagnostics;
    assert!(
        diagnostics.stdout.is_empty(),
        "expected empty stdout, got {:?}",
        diagnostics.stdout
    );
    assert!(
        diagnostics.stderr.is_empty(),
        "expected empty stderr, got {:?}",
        diagnostics.stderr
    );

    Ok(())
}

#[test]
fn json_strategy() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    const consume = globalThis.__aardvarkConsumeJsonInput
        ? globalThis.__aardvarkConsumeJsonInput()
        : globalThis.__aardvarkGetJsonInput?.();
    if (!consume || consume.answer !== 42) {
        throw new Error("json input missing");
    }
    return { ok: true, text: consume.message };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;

    let mut strategy = JsonInvocationStrategy::new(Some(json!({
        "answer": 42,
        "message": "hello-json",
    })));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["ok"], json!(true));
            assert_eq!(value["text"], json!("hello-json"));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn rawctx_strategy() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    const buffers = globalThis.__aardvarkInputBuffers || {};
    const metadata = globalThis.__aardvarkInputMetadata || {};
    const payload = buffers["payload"];
    if (!(payload instanceof Uint8Array)) {
        throw new Error("payload buffer missing");
    }
    const meta = metadata["payload"] || {};
    if (meta.dtype !== "utf8") {
        throw new Error("unexpected dtype");
    }
    const text = new TextDecoder().decode(payload);
    if (text !== "rawctx-js") {
        throw new Error("unexpected payload contents");
    }
    globalThis.__aardvarkPublishBuffer("echo-js", payload, meta);
    return null;
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;

    let meta = RawCtxMetadata::new("utf8");
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"rawctx-js"),
        Some(meta),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "echo-js");
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"rawctx-js");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn rawctx_auto_wrapper() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:handler",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export function handler(text, extras) {
    if (text !== "hello-js") {
        throw new Error("unexpected text value");
    }
    if (!extras || typeof extras.amount !== "number" || Math.abs(extras.amount - 42.5) > 1e-6) {
        throw new Error("missing amount");
    }
    if (!extras.meta_info || extras.meta_info.dtype !== "utf8") {
        throw new Error("missing metadata");
    }
    if (!extras.payload_raw || !(extras.payload_raw.data instanceof Uint8Array)) {
        throw new Error("missing raw payload");
    }
    const decoded = new TextDecoder().decode(extras.payload_raw.data);
    return { upper: decoded.toUpperCase(), amount: extras.amount };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.language = Some(RuntimeLanguage::JavaScript);
    descriptor.inputs.push(FieldDescriptor {
        name: "payload".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("text")
                .mode("positional")
                .decoder("utf8")
                .metadata_arg("meta_info")
                .raw_arg("payload_raw")
                .build(),
        ),
    });
    descriptor.inputs.push(FieldDescriptor {
        name: "amount".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("amount")
                .decoder("float64")
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;

    let payload_meta = RawCtxMetadata::new("utf8").with_extra(json!({ "dtype": "utf8" }))?;
    let inputs = vec![
        RawCtxInput::new(
            "payload",
            Bytes::from_static(b"hello-js"),
            Some(payload_meta),
        )?,
        RawCtxInput::new(
            "amount",
            Bytes::copy_from_slice(&42.5f64.to_le_bytes()),
            None,
        )?,
    ];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["upper"], json!("HELLO-JS"));
            assert_eq!(value["amount"], json!(42.5));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn rawctx_output_transform() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:handler",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export function handler() {
    return "buffer-js";
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.language = Some(RuntimeLanguage::JavaScript);
    descriptor.outputs.push(FieldDescriptor {
        name: "result".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("js-output")
                .transform("utf8")
                .metadata(json!({ "kind": "js" }))
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let mut strategy = RawCtxInvocationStrategy::default();
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "js-output");
            let bytes = handle
                .as_slice()
                .expect("shared buffer should expose zero-copy slice");
            assert_eq!(bytes, b"buffer-js");
            let kind = handle
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(kind, "js");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn network_denies_hosts_not_in_allowlist() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" },
        "resources": {
            "network": {
                "allow": [],
                "httpsOnly": true
            }
        }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(JS_NETWORK_BLOCK_SCRIPT, manifest);
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            if let Some(message) = info.value.as_ref() {
                let lowered = message.to_lowercase();
                assert!(
                    lowered.contains("not permitted")
                        || lowered.contains("blocked")
                        || lowered == "undefined",
                    "expected network policy message, got {:?}",
                    message
                );
            }
        }
        other => panic!("expected javascript network denial, got {:?}", other),
    }
    assert_eq!(
        outcome.diagnostics.network_hosts_blocked.len(),
        1,
        "expected one blocked host in diagnostics"
    );
    let blocked = &outcome.diagnostics.network_hosts_blocked[0];
    assert_eq!(blocked.host, "blocked.example");
    assert_eq!(blocked.reason, "no-allowlist");
    assert!(
        !blocked.https_required,
        "https flag should be false for blanket denials"
    );
    Ok(())
}

#[test]
fn xmlhttprequest_sync_data_url_matches_browser_shape() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    const seen = [];
    const xhr = new XMLHttpRequest();
    xhr.onreadystatechange = () => seen.push(xhr.readyState);
    xhr.open("GET", "data:text/plain,hello%20xhr", false);
    xhr.send();
    return {
        status: xhr.status,
        statusText: xhr.statusText,
        text: xhr.responseText,
        response: xhr.response,
        header: xhr.getResponseHeader("Content-Type"),
        allHeaders: xhr.getAllResponseHeaders(),
        done: xhr.readyState === XMLHttpRequest.DONE,
        seen,
    };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], json!(200));
            assert_eq!(value["statusText"], json!("OK"));
            assert_eq!(value["text"], json!("hello xhr"));
            assert_eq!(value["response"], json!("hello xhr"));
            assert_eq!(value["header"], json!("text/plain"));
            assert_eq!(value["done"], json!(true));
            assert!(
                value["allHeaders"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("content-type: text/plain"),
                "expected content-type in all headers: {value:?}"
            );
            let seen = value["seen"].as_array().expect("seen should be an array");
            assert!(
                seen.iter().any(|state| state == &json!(4)),
                "expected DONE readyState in callback trace: {value:?}"
            );
        }
        other => panic!(
            "expected json payload, got {:?}; status {:?}",
            other, outcome.status
        ),
    }
    Ok(())
}

#[test]
fn xmlhttprequest_async_json_response_resolves_promise() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    return new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.responseType = "json";
        xhr.onload = () => resolve({
            status: xhr.status,
            ok: xhr.response.ok,
            value: xhr.response.value,
        });
        xhr.onerror = (event) => reject(event.error || new Error("xhr failed"));
        xhr.open("GET", "data:application/json,%7B%22ok%22%3Atrue%2C%22value%22%3A7%7D");
        xhr.send();
    });
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!({ "status": 200, "ok": true, "value": 7 }));
        }
        other => panic!("expected json payload, got {:?}", other),
    }
    Ok(())
}

#[test]
fn xmlhttprequest_posts_headers_and_body_through_native_fetch() -> Result<()> {
    let (port, request_rx) = spawn_one_shot_http_server();
    let manifest = format!(
        r#"{{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": {{ "language": "javascript" }},
        "resources": {{
            "network": {{
                "allow": ["127.0.0.1:{port}"],
                "httpsOnly": false
            }}
        }}
    }}"#
    );

    let bundle = bundle_with_js_main_and_manifest(
        &format!(
            r#"
export default function main() {{
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "http://127.0.0.1:{port}/submit", false);
    xhr.setRequestHeader("X-Aardvark-Test", "xhr");
    xhr.setRequestHeader("Content-Type", "text/plain");
    xhr.send("hello-body");
    return {{
        status: xhr.status,
        response: xhr.responseText,
        header: xhr.getResponseHeader("x-aardvark-test"),
    }};
}}
"#
        ),
        &manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], json!(200));
            assert_eq!(value["response"], json!("accepted"));
            assert_eq!(value["header"], json!("server"));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    let request = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server should receive request");
    let lowered = request.to_ascii_lowercase();
    assert!(
        request.starts_with("POST /submit "),
        "expected POST request, got {request:?}"
    );
    assert!(
        lowered.contains("x-aardvark-test: xhr"),
        "expected custom header, got {request:?}"
    );
    assert!(
        lowered.contains("content-type: text/plain"),
        "expected content-type header, got {request:?}"
    );
    assert!(
        request.ends_with("hello-body"),
        "expected request body, got {request:?}"
    );
    assert!(
        outcome
            .diagnostics
            .network_hosts_contacted
            .iter()
            .any(|contact| contact.host == "127.0.0.1"
                && contact.port == Some(port)
                && !contact.https),
        "expected localhost contact in diagnostics: {:?}",
        outcome.diagnostics.network_hosts_contacted
    );
    Ok(())
}

#[test]
fn xmlhttprequest_preserves_http_error_status_and_body() -> Result<()> {
    let response = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmissing".to_vec();
    let (port, request_rx) = spawn_one_shot_http_server_with_response(response);
    let manifest = format!(
        r#"{{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": {{ "language": "javascript" }},
        "resources": {{
            "network": {{
                "allow": ["127.0.0.1:{port}"],
                "httpsOnly": false
            }}
        }}
    }}"#
    );

    let bundle = bundle_with_js_main_and_manifest(
        &format!(
            r#"
export default function main() {{
    const xhr = new XMLHttpRequest();
    xhr.open("GET", "http://127.0.0.1:{port}/missing", false);
    xhr.send();
    return {{
        status: xhr.status,
        statusText: xhr.statusText,
        response: xhr.responseText,
        contentType: xhr.getResponseHeader("content-type"),
    }};
}}
"#
        ),
        &manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], json!(404));
            assert_eq!(value["statusText"], json!("Not Found"));
            assert_eq!(value["response"], json!("missing"));
            assert_eq!(value["contentType"], json!("text/plain"));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    let request = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server should receive request");
    assert!(
        request.starts_with("GET /missing "),
        "expected missing request, got {request:?}"
    );
    Ok(())
}

#[test]
fn xmlhttprequest_ignores_forbidden_request_headers() -> Result<()> {
    let (port, request_rx) = spawn_one_shot_http_server();
    let manifest = format!(
        r#"{{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": {{ "language": "javascript" }},
        "resources": {{
            "network": {{
                "allow": ["127.0.0.1:{port}"],
                "httpsOnly": false
            }}
        }}
    }}"#
    );

    let bundle = bundle_with_js_main_and_manifest(
        &format!(
            r#"
export default function main() {{
    const xhr = new XMLHttpRequest();
    xhr.open("GET", "http://127.0.0.1:{port}/headers", false);
    xhr.setRequestHeader("Host", "evil.example");
    xhr.setRequestHeader("Cookie", "session=secret");
    xhr.setRequestHeader("X-Aardvark-Test", "allowed");
    xhr.send();
    return {{
        status: xhr.status,
        response: xhr.responseText,
    }};
}}
"#
        ),
        &manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], json!(200));
            assert_eq!(value["response"], json!("accepted"));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    let request = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server should receive request");
    let lowered = request.to_ascii_lowercase();
    assert!(
        lowered.contains("x-aardvark-test: allowed"),
        "expected allowed custom header, got {request:?}"
    );
    assert!(
        !lowered.contains("host: evil.example"),
        "forbidden Host override should not be forwarded: {request:?}"
    );
    assert!(
        !lowered.contains("cookie:"),
        "forbidden Cookie header should not be forwarded: {request:?}"
    );
    Ok(())
}

#[test]
fn xmlhttprequest_denial_uses_network_policy_telemetry() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" },
        "resources": {
            "network": {
                "allow": [],
                "httpsOnly": true
            }
        }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", "https://blocked.example/resource", false);
    try {
        xhr.send();
    } catch (err) {
        return {
            name: err.name,
            message: err.message,
            status: xhr.status,
        };
    }
    return { name: "none" };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["name"], json!("NetworkError"));
            assert_eq!(value["status"], json!(0));
            assert!(
                value["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not permitted"),
                "expected policy message, got {value:?}"
            );
        }
        other => panic!("expected json payload, got {:?}", other),
    }
    assert_eq!(outcome.diagnostics.network_hosts_blocked.len(), 1);
    let blocked = &outcome.diagnostics.network_hosts_blocked[0];
    assert_eq!(blocked.host, "blocked.example");
    assert_eq!(blocked.reason, "no-allowlist");
    Ok(())
}

#[test]
fn exception_reports_failure() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(JS_THROWING_SCRIPT, manifest);
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            let typ = info.typ.clone().unwrap_or_default().to_lowercase();
            assert!(
                typ.contains("error"),
                "expected JS exception type in diagnostics, got {:?}",
                info.typ
            );
            let value = info.value.clone().unwrap_or_default();
            assert!(
                value.contains("boom"),
                "expected message to contain boom, got {:?}",
                value
            );
        }
        other => panic!("expected javascript exception failure, got {:?}", other),
    }
    assert!(
        outcome.diagnostics.stdout.contains("about to throw"),
        "stdout should include pre-throw log"
    );
    Ok(())
}

#[test]
fn rawctx_requires_capability() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(JS_RAWCTX_PUBLISH_SCRIPT, manifest);
    let mut config = PyRuntimeConfig::default();
    config.host_capabilities.clear();
    let mut runtime = PyRuntime::new(config)?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    assert!(
        matches!(outcome.status, OutcomeStatus::Failure(_)),
        "expected capability denial"
    );
    Ok(())
}

fn bundle_with_js_main_and_manifest(code: &str, manifest: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.js",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start main.js entry");
    writer
        .write_all(code.as_bytes())
        .expect("failed to write main.js");

    writer
        .start_file(
            "aardvark.manifest.json",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start manifest entry");
    writer
        .write_all(manifest.as_bytes())
        .expect("failed to write manifest");

    let cursor = writer.finish().expect("failed to finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("failed to parse bundle")
}

fn spawn_one_shot_http_server() -> (u16, mpsc::Receiver<String>) {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Aardvark-Test: server\r\nContent-Length: 8\r\nConnection: close\r\n\r\naccepted".to_vec();
    spawn_one_shot_http_server_with_response(response)
}

fn spawn_one_shot_http_server_with_response(response: Vec<u8>) -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let port = listener
        .local_addr()
        .expect("test server local addr")
        .port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut bytes = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            let read = match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => break,
            };
            bytes.extend_from_slice(&buf[..read]);
            if http_request_complete(&bytes) {
                break;
            }
        }
        let request = String::from_utf8_lossy(&bytes).to_string();
        let _ = tx.send(request);
        let _ = stream.write_all(&response);
    });
    (port, rx)
}

fn http_request_complete(bytes: &[u8]) -> bool {
    let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers_len = headers_end + 4;
    let headers = String::from_utf8_lossy(&bytes[..headers_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    bytes.len() >= headers_len + content_length
}

const JS_NETWORK_BLOCK_SCRIPT: &str = r#"
export default function main() {
    globalThis.__pyRunnerNativeFetch("https://blocked.example/resource");
    return "should-not-complete";
}
"#;

const JS_THROWING_SCRIPT: &str = r#"
export default function main() {
    console.log("about to throw");
    throw new Error("boom from js");
}
"#;

const JS_RAWCTX_PUBLISH_SCRIPT: &str = r#"
export default function main() {
    globalThis.__aardvarkPublishBuffer("js-buf", new Uint8Array([1, 2, 3]), null);
    return null;
}
"#;
