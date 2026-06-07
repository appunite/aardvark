use crate::bundle_manifest::ManifestNetworkResources;

const DEFAULT_NATIVE_FETCH_MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_NATIVE_FETCH_MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_NATIVE_FETCH_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub(crate) struct NetworkContactRecord {
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) https: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct NetworkDeniedRecord {
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) reason: String,
    pub(crate) https_required: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct NetworkPolicy {
    entries: Vec<HostPattern>,
    https_only: bool,
    pub(crate) fetch_limits: NetworkFetchLimits,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NetworkFetchLimits {
    pub(crate) max_request_bytes: u64,
    pub(crate) max_response_bytes: u64,
    pub(crate) timeout_ms: u64,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            https_only: true,
            fetch_limits: NetworkFetchLimits::default(),
        }
    }
}

impl NetworkPolicy {
    pub(crate) fn new(allow: &[String], https_only: bool) -> Self {
        Self::with_limits(allow, https_only, NetworkFetchLimits::default())
    }

    pub(crate) fn from_manifest(network: &ManifestNetworkResources) -> Self {
        Self::with_limits(
            network.allow.as_slice(),
            network.https_only,
            NetworkFetchLimits::from_manifest(network),
        )
    }

    fn with_limits(allow: &[String], https_only: bool, fetch_limits: NetworkFetchLimits) -> Self {
        let entries = allow
            .iter()
            .filter_map(|value| HostPattern::from_pattern(value))
            .collect();
        Self {
            entries,
            https_only,
            fetch_limits,
        }
    }

    pub(crate) fn evaluate(
        &self,
        host: &str,
        port: Option<u16>,
        is_https: bool,
    ) -> NetworkDecision {
        if self.entries.is_empty() {
            return NetworkDecision::Denied(NetworkDenyReason::NoAllowlist);
        }
        if self.https_only && !is_https {
            return NetworkDecision::Denied(NetworkDenyReason::SchemeNotAllowed);
        }
        let host_lc = host.to_ascii_lowercase();
        for pattern in &self.entries {
            if pattern.matches(&host_lc, port) {
                return NetworkDecision::Allowed;
            }
        }
        NetworkDecision::Denied(NetworkDenyReason::HostNotAllowed)
    }
}

impl Default for NetworkFetchLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: DEFAULT_NATIVE_FETCH_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_NATIVE_FETCH_MAX_RESPONSE_BYTES,
            timeout_ms: DEFAULT_NATIVE_FETCH_TIMEOUT_MS,
        }
    }
}

impl NetworkFetchLimits {
    fn from_manifest(network: &ManifestNetworkResources) -> Self {
        Self {
            max_request_bytes: network
                .max_request_bytes
                .unwrap_or(DEFAULT_NATIVE_FETCH_MAX_REQUEST_BYTES),
            max_response_bytes: network
                .max_response_bytes
                .unwrap_or(DEFAULT_NATIVE_FETCH_MAX_RESPONSE_BYTES),
            timeout_ms: network
                .timeout_ms
                .unwrap_or(DEFAULT_NATIVE_FETCH_TIMEOUT_MS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkDecision {
    Allowed,
    Denied(NetworkDenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkDenyReason {
    NoAllowlist,
    SchemeNotAllowed,
    HostNotAllowed,
}

impl NetworkDenyReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            NetworkDenyReason::NoAllowlist => "no-allowlist",
            NetworkDenyReason::SchemeNotAllowed => "scheme-not-allowed",
            NetworkDenyReason::HostNotAllowed => "host-not-allowed",
        }
    }
}

#[derive(Debug, Clone)]
struct HostPattern {
    kind: HostPatternKind,
    port: Option<u16>,
}

#[derive(Debug, Clone)]
enum HostPatternKind {
    Exact(String),
    WildcardSuffix(String),
}

impl HostPattern {
    fn from_pattern(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lowered = trimmed.to_ascii_lowercase();
        let (host_part, port) = split_host_and_port(&lowered);
        if host_part.is_empty() {
            return None;
        }
        if host_part.starts_with("*.") {
            let suffix = host_part.trim_start_matches("*.").to_owned();
            if suffix.is_empty() {
                return None;
            }
            Some(Self {
                kind: HostPatternKind::WildcardSuffix(suffix),
                port,
            })
        } else {
            Some(Self {
                kind: HostPatternKind::Exact(host_part),
                port,
            })
        }
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        let port_allowed = match (self.port, port) {
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
            _ => true,
        };
        if !port_allowed {
            return false;
        }
        match &self.kind {
            HostPatternKind::Exact(expected) => host == expected,
            HostPatternKind::WildcardSuffix(suffix) => host.ends_with(suffix),
        }
    }
}

fn split_host_and_port(value: &str) -> (String, Option<u16>) {
    if let Some(idx) = value.rfind(':') {
        let (host_part, port_part) = value.split_at(idx);
        let port_str = &port_part[1..];
        if !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(port) = port_str.parse::<u16>() {
                return (host_part.to_owned(), Some(port));
            }
        }
    }
    (value.to_owned(), None)
}
