use std::convert::TryFrom;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use crate::network::{NetworkDecision, NetworkDenyReason, NetworkFetchLimits};

use super::package_assets::{
    guess_content_type, is_pyodide_package_asset_url, resolve_local_package_path,
    MAX_LOCAL_PACKAGE_ASSET_BYTES,
};
use super::RuntimeContext;

use tracing::{info, warn};
use url::Url;
use v8::{
    self, Array, FunctionCallbackArguments, Local, Object, PinScope, ReturnValue,
    String as V8String, Uint8Array, Value,
};

#[derive(Debug)]
struct NativeFetchInit {
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[derive(Debug)]
struct NativeFetchLimitError {
    kind: NativeFetchLimitKind,
    actual: u64,
    limit: u64,
}

#[derive(Debug)]
enum NativeFetchLimitKind {
    RequestBody,
    ResponseBody,
}

impl NativeFetchLimitError {
    fn request_body(actual: u64, limit: u64) -> Self {
        Self {
            kind: NativeFetchLimitKind::RequestBody,
            actual,
            limit,
        }
    }

    fn response_body(actual: u64, limit: u64) -> Self {
        Self {
            kind: NativeFetchLimitKind::ResponseBody,
            actual,
            limit,
        }
    }

    fn message(&self) -> String {
        let label = match self.kind {
            NativeFetchLimitKind::RequestBody => "network request body",
            NativeFetchLimitKind::ResponseBody => "network response body",
        };
        format!(
            "{label} exceeds configured limit: actual {} bytes, limit {} bytes",
            self.actual, self.limit
        )
    }
}

impl Default for NativeFetchInit {
    fn default() -> Self {
        Self {
            method: "GET".to_owned(),
            headers: Vec::new(),
            body: None,
        }
    }
}

fn native_fetch_init_from_value(
    scope: &mut PinScope<'_, '_>,
    value: Option<Local<'_, Value>>,
    max_request_bytes: u64,
) -> std::result::Result<NativeFetchInit, NativeFetchLimitError> {
    let mut init = NativeFetchInit::default();
    let Some(value) = value else {
        return Ok(init);
    };
    if value.is_null_or_undefined() {
        return Ok(init);
    }
    let Some(object) = value.to_object(scope) else {
        return Ok(init);
    };

    if let Some(method) = object_string_property(scope, object, "method") {
        let method = method.trim();
        if !method.is_empty() {
            init.method = method.to_ascii_uppercase();
        }
    }

    if let Some(headers_value) = object_property(scope, object, "headers") {
        init.headers = native_fetch_headers_from_value(scope, headers_value);
    }

    if let Some(body_value) = object_property(scope, object, "body") {
        init.body = native_fetch_body_from_value(scope, body_value, max_request_bytes)?;
    }

    Ok(init)
}

fn object_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
) -> Option<Local<'a, Value>> {
    let key = V8String::new(scope, name)?;
    object.get(scope, key.into())
}

fn object_string_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
) -> Option<String> {
    let value = object_property(scope, object, name)?;
    if value.is_null_or_undefined() {
        return None;
    }
    value
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
}

fn native_fetch_headers_from_value(
    scope: &mut PinScope<'_, '_>,
    value: Local<'_, Value>,
) -> Vec<(String, String)> {
    if value.is_null_or_undefined() {
        return Vec::new();
    }

    if let Ok(array) = Local::<Array>::try_from(value) {
        let mut headers = Vec::new();
        for index in 0..array.length() {
            let Some(entry) = array.get_index(scope, index) else {
                continue;
            };
            if let Ok(pair) = Local::<Array>::try_from(entry) {
                let Some(name_value) = pair.get_index(scope, 0) else {
                    continue;
                };
                let Some(value_value) = pair.get_index(scope, 1) else {
                    continue;
                };
                let Some(name) = name_value
                    .to_string(scope)
                    .map(|value| value.to_rust_string_lossy(scope))
                else {
                    continue;
                };
                let Some(value) = value_value
                    .to_string(scope)
                    .map(|value| value.to_rust_string_lossy(scope))
                else {
                    continue;
                };
                if !name.is_empty() {
                    headers.push((name, value));
                }
            }
        }
        return headers;
    }

    let Some(object) = value.to_object(scope) else {
        return Vec::new();
    };
    let Some(names) = object.get_own_property_names(scope, Default::default()) else {
        return Vec::new();
    };
    let mut headers = Vec::new();
    for index in 0..names.length() {
        let Some(name_value) = names.get_index(scope, index) else {
            continue;
        };
        let Some(value_value) = object.get(scope, name_value) else {
            continue;
        };
        let Some(name) = name_value
            .to_string(scope)
            .map(|value| value.to_rust_string_lossy(scope))
        else {
            continue;
        };
        let Some(value) = value_value
            .to_string(scope)
            .map(|value| value.to_rust_string_lossy(scope))
        else {
            continue;
        };
        if !name.is_empty() {
            headers.push((name, value));
        }
    }
    headers
}

fn native_fetch_body_from_value(
    scope: &mut PinScope<'_, '_>,
    value: Local<'_, Value>,
    max_request_bytes: u64,
) -> std::result::Result<Option<Vec<u8>>, NativeFetchLimitError> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }

    if let Ok(typed_array) = Local::<Uint8Array>::try_from(value) {
        let byte_len = typed_array.byte_length() as u64;
        if byte_len > max_request_bytes {
            return Err(NativeFetchLimitError::request_body(
                byte_len,
                max_request_bytes,
            ));
        }
        return Ok(uint8_array_to_vec(scope, typed_array));
    }

    let body = value
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope).into_bytes());
    if let Some(body) = body {
        let actual = body.len() as u64;
        if actual > max_request_bytes {
            return Err(NativeFetchLimitError::request_body(
                actual,
                max_request_bytes,
            ));
        }
        Ok(Some(body))
    } else {
        Ok(None)
    }
}

fn uint8_array_to_vec(
    scope: &mut PinScope<'_, '_>,
    typed_array: Local<'_, Uint8Array>,
) -> Option<Vec<u8>> {
    let byte_len = typed_array.byte_length();
    if byte_len == 0 {
        return Some(Vec::new());
    }
    let array_buffer = typed_array.buffer(scope)?;
    let backing_store = array_buffer.get_backing_store();
    let offset = typed_array.byte_offset();
    let ptr = backing_store.data()?;
    let store_size = backing_store.byte_length();
    let end = offset.checked_add(byte_len)?;
    if end > store_size {
        return None;
    }

    // SAFETY: The backing store remains alive while the V8 array buffer is
    // referenced here, and the checked offset/length range is within the store.
    unsafe {
        let data = ptr.as_ptr().add(offset) as *const u8;
        Some(std::slice::from_raw_parts(data, byte_len).to_vec())
    }
}

fn is_forbidden_xhr_request_header(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "accept-charset"
            | "accept-encoding"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "dnt"
            | "expect"
            | "host"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "user-agent"
    ) || normalized.starts_with("sec-")
        || normalized.starts_with("access-control-request-")
}

fn apply_native_fetch_options<B>(
    request: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
    limits: NetworkFetchLimits,
) -> ureq::RequestBuilder<B> {
    let timeout = Some(Duration::from_millis(limits.timeout_ms));
    let mut request = request
        .config()
        .http_status_as_error(false)
        .timeout_send_request(timeout)
        .timeout_send_body(timeout)
        .timeout_recv_response(timeout)
        .timeout_recv_body(timeout)
        .build();
    for (name, value) in headers {
        if is_forbidden_xhr_request_header(name) {
            warn!(
                target = "aardvark::sandbox",
                header = name.as_str(),
                "ignoring forbidden XMLHttpRequest request header"
            );
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    request
}

fn throw_v8_error(scope: &mut PinScope<'_, '_>, message: &str) {
    let Some(message) = v8::String::new(scope, message) else {
        return;
    };
    let exception = v8::Exception::error(scope, message);
    scope.throw_exception(exception);
}

fn read_local_package_asset(path: &Path) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_LOCAL_PACKAGE_ASSET_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "local package asset {} is {} bytes, above the {} byte limit",
                path.display(),
                len,
                MAX_LOCAL_PACKAGE_ASSET_BYTES
            ),
        ));
    }

    let mut bytes = Vec::with_capacity(len as usize);
    let mut reader = file.take(MAX_LOCAL_PACKAGE_ASSET_BYTES.saturating_add(1));
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LOCAL_PACKAGE_ASSET_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "local package asset {} exceeded the {} byte limit while reading",
                path.display(),
                MAX_LOCAL_PACKAGE_ASSET_BYTES
            ),
        ));
    }
    Ok(bytes)
}

pub(super) fn native_fetch_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let url = if args.length() > 0 {
        args.get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
    } else {
        String::new()
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        rv.set(v8::undefined(scope).into());
        return;
    }

    let context_state = scope.get_slot::<Rc<RuntimeContext>>().cloned();
    let local_package_path = context_state
        .as_ref()
        .and_then(|state| state.package_root())
        .and_then(|root| resolve_local_package_path(&root, &url));

    if let Some(local_path) = local_package_path {
        match read_local_package_asset(&local_path) {
            Ok(body) => {
                info!(
                    target = "aardvark::js",
                    %url,
                    path = %local_path.display(),
                    "serving package from local directory"
                );
                let content_type = guess_content_type(&local_path);
                let headers = [("content-type", content_type)];
                if let Some(result) = build_fetch_response(scope, 200, "OK", &url, body, &headers) {
                    rv.set(result.into());
                    return;
                }
                throw_v8_error(scope, "failed to allocate local fetch response");
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %url,
                    path = %local_path.display(),
                    error = ?err,
                    "failed to read local package asset"
                );
                if err.kind() == io::ErrorKind::InvalidData {
                    throw_v8_error(scope, &err.to_string());
                    return;
                }
            }
        }
    } else if is_pyodide_package_asset_url(&url) {
        warn!(
            target = "aardvark::packages",
            %url,
            "Pyodide package asset missing from local distribution"
        );
        throw_v8_error(
            scope,
            "Pyodide package asset is missing from AARDVARK_PYODIDE_DIST_DIR",
        );
        return;
    }

    let Some(context_state) = context_state else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    let parsed = match Url::parse(&url) {
        Ok(value) => value,
        Err(err) => {
            warn!(target = "aardvark::sandbox", %url, error = ?err, "network request rejected: invalid url");
            throw_v8_error(scope, "network access denied");
            return;
        }
    };

    let host = match parsed.host_str() {
        Some(value) if !value.is_empty() => value.to_ascii_lowercase(),
        _ => {
            warn!(target = "aardvark::sandbox", %url, "network request rejected: missing host");
            throw_v8_error(scope, "network access denied");
            return;
        }
    };
    let port = parsed.port();
    let is_https = parsed.scheme().eq_ignore_ascii_case("https");

    let policy = {
        let policy = context_state.network_policy.read();
        policy.clone()
    };
    let decision = policy.evaluate(&host, port, is_https);

    if let NetworkDecision::Denied(reason) = decision {
        context_state.record_network_denial(&host, port, reason);
        let message_text = match reason {
            NetworkDenyReason::SchemeNotAllowed => {
                if let Some(p) = port {
                    format!("network access to '{}:{}' requires https", host, p)
                } else {
                    format!("network access to '{}' requires https", host)
                }
            }
            _ => {
                if let Some(p) = port {
                    format!("network access to '{}:{}' is not permitted", host, p)
                } else {
                    format!("network access to '{}' is not permitted", host)
                }
            }
        };
        warn!(
            target = "aardvark::sandbox",
            network.allowed = false,
            %url,
            host = host.as_str(),
            port,
            reason = ?reason,
            "network request blocked"
        );
        throw_v8_error(scope, &message_text);
        return;
    }

    let fetch_init = match native_fetch_init_from_value(
        scope,
        if args.length() > 1 {
            Some(args.get(1))
        } else {
            None
        },
        policy.fetch_limits.max_request_bytes,
    ) {
        Ok(init) => init,
        Err(err) => {
            let message_text = err.message();
            warn!(
                target = "aardvark::sandbox",
                %url,
                error = message_text.as_str(),
                "network request blocked by native fetch limit"
            );
            throw_v8_error(scope, &message_text);
            return;
        }
    };

    info!(
        target = "aardvark::sandbox",
        network.allowed = true,
        method = fetch_init.method.as_str(),
        %url,
        host = host.as_str(),
        port,
        https = is_https,
        "network request allowed"
    );

    context_state.record_network_contact(&host, port, is_https);

    let body = fetch_init.body.as_deref().unwrap_or(&[]);
    let has_body = fetch_init.body.is_some();
    let response_result = match fetch_init.method.as_str() {
        "GET" => {
            let request = apply_native_fetch_options(
                ureq::get(&url),
                &fetch_init.headers,
                policy.fetch_limits,
            );
            if has_body {
                request.force_send_body().send(body)
            } else {
                request.call()
            }
        }
        "HEAD" => {
            let request = apply_native_fetch_options(
                ureq::head(&url),
                &fetch_init.headers,
                policy.fetch_limits,
            );
            request.call()
        }
        "OPTIONS" => {
            let request = apply_native_fetch_options(
                ureq::options(&url),
                &fetch_init.headers,
                policy.fetch_limits,
            );
            if has_body {
                request.force_send_body().send(body)
            } else {
                request.call()
            }
        }
        "DELETE" => {
            let request = apply_native_fetch_options(
                ureq::delete(&url),
                &fetch_init.headers,
                policy.fetch_limits,
            );
            if has_body {
                request.force_send_body().send(body)
            } else {
                request.call()
            }
        }
        "POST" => {
            apply_native_fetch_options(ureq::post(&url), &fetch_init.headers, policy.fetch_limits)
                .send(body)
        }
        "PUT" => {
            apply_native_fetch_options(ureq::put(&url), &fetch_init.headers, policy.fetch_limits)
                .send(body)
        }
        "PATCH" => {
            apply_native_fetch_options(ureq::patch(&url), &fetch_init.headers, policy.fetch_limits)
                .send(body)
        }
        other => {
            warn!(
                target = "aardvark::sandbox",
                method = other,
                %url,
                "network request rejected: unsupported http method"
            );
            throw_v8_error(scope, "unsupported HTTP method");
            return;
        }
    };

    let mut response = match response_result {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = ?err, "native fetch failed");
            rv.set(v8::undefined(scope).into());
            return;
        }
    };

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let mut headers_list = Vec::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers_list.push((name.as_str().to_ascii_lowercase(), value.to_string()));
        }
    }

    let body = match response
        .body_mut()
        .with_config()
        .limit(policy.fetch_limits.max_response_bytes)
        .read_to_vec()
    {
        Ok(body) => body,
        Err(ureq::Error::BodyExceedsLimit(limit)) => {
            let message_text =
                NativeFetchLimitError::response_body(limit.saturating_add(1), limit).message();
            warn!(
                target = "aardvark::sandbox",
                %url,
                error = message_text.as_str(),
                "network response blocked by native fetch limit"
            );
            throw_v8_error(scope, &message_text);
            return;
        }
        Err(err) => {
            tracing::warn!(%url, error = ?err, "native fetch read failed");
            rv.set(v8::undefined(scope).into());
            return;
        }
    };

    let header_refs = headers_list
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    if let Some(result) =
        build_fetch_response(scope, status as i32, &status_text, &url, body, &header_refs)
    {
        rv.set(result.into());
        return;
    }
    throw_v8_error(scope, "failed to allocate native fetch response");
}

fn build_fetch_response<'a>(
    scope: &mut PinScope<'a, '_>,
    status: i32,
    status_text: &str,
    url: &str,
    body: Vec<u8>,
    headers_list: &[(&str, &str)],
) -> Option<Local<'a, Object>> {
    let backing = v8::ArrayBuffer::new_backing_store_from_vec(body);
    let byte_length = backing.len();
    let backing_shared = backing.make_shared();
    let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &backing_shared);
    let uint8 = Uint8Array::new(scope, array_buffer, 0, byte_length)?;

    let result = v8::Object::new(scope);
    set_property(
        scope,
        result,
        "status",
        v8::Integer::new(scope, status).into(),
    )?;
    set_property(
        scope,
        result,
        "statusText",
        v8::String::new(scope, status_text)?.into(),
    )?;
    set_property(
        scope,
        result,
        "ok",
        v8::Boolean::new(scope, (200..300).contains(&status)).into(),
    )?;
    set_property(scope, result, "url", v8::String::new(scope, url)?.into())?;
    set_property(
        scope,
        result,
        "binary",
        v8::Boolean::new(scope, true).into(),
    )?;
    set_property(scope, result, "body", uint8.into())?;

    let headers_array = v8::Array::new(scope, headers_list.len() as i32);
    for (index, (name, value)) in headers_list.iter().enumerate() {
        let pair = v8::Array::new(scope, 2);
        let name_value = v8::String::new(scope, name)?;
        let value_value = v8::String::new(scope, value)?;
        pair.set_index(scope, 0, name_value.into());
        pair.set_index(scope, 1, value_value.into());
        headers_array.set_index(scope, index as u32, pair.into());
    }
    set_property(scope, result, "headers", headers_array.into())?;

    if let Some((_, value)) = headers_list
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        set_property(
            scope,
            result,
            "contentType",
            v8::String::new(scope, value)?.into(),
        )?;
    }

    Some(result)
}

fn set_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    key: &str,
    value: Local<'a, Value>,
) -> Option<()> {
    let key = v8::String::new(scope, key)?;
    object.set(scope, key.into(), value);
    Some(())
}
