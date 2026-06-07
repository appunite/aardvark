use std::path::PathBuf;

use aardvark_core::CleanupMode;
use clap::{Parser, ValueEnum};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(
    name = "aardvark-perf",
    about = "Performance harness for Aardvark runtime"
)]
pub(crate) enum Cli {
    /// Run benchmarks for all scenarios (Aardvark + host Python) and emit reports
    All {
        #[arg(long, default_value_t = 10)]
        iterations: usize,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        csv: Option<PathBuf>,
        #[arg(long, value_enum, ignore_case = true)]
        profile: Option<LoadProfile>,
        /// Include per-iteration timing samples in JSON output.
        #[arg(long)]
        samples: bool,
        /// Pyodide distribution profile to write into generated bundle manifests.
        #[arg(long)]
        pyodide_profile: Option<String>,
        /// Python modules to import immediately before warm snapshot capture.
        #[arg(long = "warm-preimport")]
        warm_preimports: Vec<String>,
        /// Python modules to write into runtime.pyodide.preloadImports in generated manifests.
        #[arg(long = "manifest-preload-import")]
        manifest_preload_imports: Vec<String>,
        /// Desired pool size used by first-live registry setup benchmarks.
        #[arg(long = "setup-pool-desired-size", default_value_t = 2)]
        setup_pool_desired_size: usize,
    },
    /// Run a single scenario/mode combination
    Scenario {
        #[arg(long, value_enum, ignore_case = true)]
        scenario: Scenario,
        #[arg(long, value_enum, ignore_case = true)]
        mode: Mode,
        #[arg(long, default_value_t = 10)]
        iterations: usize,
        #[arg(long, value_enum, ignore_case = true)]
        profile: Option<LoadProfile>,
        /// Include per-iteration timing samples in JSON output.
        #[arg(long)]
        samples: bool,
        /// Pyodide distribution profile to write into the generated bundle manifest.
        #[arg(long)]
        pyodide_profile: Option<String>,
        /// Python modules to import immediately before warm snapshot capture.
        #[arg(long = "warm-preimport")]
        warm_preimports: Vec<String>,
        /// Python modules to write into runtime.pyodide.preloadImports in the generated manifest.
        #[arg(long = "manifest-preload-import")]
        manifest_preload_imports: Vec<String>,
        /// Desired pool size used by first-live registry setup benchmarks.
        #[arg(long = "setup-pool-desired-size", default_value_t = 2)]
        setup_pool_desired_size: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub(crate) enum Scenario {
    Echo,
    Numpy,
    NumpyMatmul,
    Pandas,
    ScipySgemm,
    Tensor,
    Matplotlib,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub(crate) enum LoadProfile {
    None,
    Low,
    Medium,
    High,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InvocationKind {
    Json,
    RawCtx,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PathKind {
    Cold,
    Warm,
    ResetInPlace,
    Persistent,
    FirstCall,
    FirstLive,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CleanupKind {
    Full,
    SharedBuffersOnly,
    None,
}

impl CleanupKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            CleanupKind::Full => "full",
            CleanupKind::SharedBuffersOnly => "shared-buffers-only",
            CleanupKind::None => "none",
        }
    }

    pub(crate) fn to_cleanup_mode(self) -> CleanupMode {
        match self {
            CleanupKind::Full => CleanupMode::Full,
            CleanupKind::SharedBuffersOnly => CleanupMode::SharedBuffersOnly,
            CleanupKind::None => CleanupMode::None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub(crate) enum Mode {
    AardvarkJsonCold,
    AardvarkJsonWarm,
    AardvarkJsonResetInPlace,
    AardvarkJsonPersistent,
    #[value(name = "aardvark-json-persistent-warm-call")]
    AardvarkJsonPersistentWarmCall,
    #[value(name = "aardvark-json-persistent-no-stdio")]
    AardvarkJsonPersistentNoStdio,
    #[value(name = "aardvark-json-persistent-warm-call-no-stdio")]
    AardvarkJsonPersistentWarmCallNoStdio,
    #[value(name = "aardvark-json-registry-persistent-no-stdio")]
    AardvarkJsonRegistryPersistentNoStdio,
    #[value(name = "aardvark-json-registry-prepare-each-call-no-stdio")]
    AardvarkJsonRegistryPrepareEachCallNoStdio,
    #[value(name = "aardvark-json-registry-cached-handler-no-stdio")]
    AardvarkJsonRegistryCachedHandlerNoStdio,
    #[value(name = "aardvark-json-registry-retained-handler-no-stdio")]
    AardvarkJsonRegistryRetainedHandlerNoStdio,
    #[value(name = "aardvark-json-registry-retained-first-live-no-stdio")]
    AardvarkJsonRegistryRetainedFirstLiveNoStdio,
    #[value(name = "aardvark-json-registry-retained-warm-all-first-live-no-stdio")]
    AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio,
    #[value(name = "aardvark-json-warmed-host-pooled-warm-all-first-live-no-stdio")]
    AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio,
    #[value(name = "aardvark-json-warmed-host-registry-pooled-warm-all-first-live-no-stdio")]
    AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio,
    #[value(name = "aardvark-json-persistent-full")]
    AardvarkJsonPersistentFull,
    AardvarkJsonPersistentShared,
    AardvarkJsonPersistentNone,
    #[value(name = "aardvark-rawctx-cold")]
    AardvarkRawCtxCold,
    #[value(name = "aardvark-rawctx-warm")]
    AardvarkRawCtxWarm,
    #[value(name = "aardvark-rawctx-reset-in-place")]
    AardvarkRawCtxResetInPlace,
    #[value(name = "aardvark-rawctx-persistent")]
    AardvarkRawCtxPersistent,
    #[value(name = "aardvark-rawctx-persistent-no-stdio")]
    AardvarkRawCtxPersistentNoStdio,
    #[value(name = "aardvark-rawctx-direct-persistent")]
    AardvarkRawCtxDirectPersistent,
    #[value(name = "aardvark-rawctx-direct-owned-persistent")]
    AardvarkRawCtxDirectOwnedPersistent,
    #[value(name = "aardvark-rawctx-direct-persistent-warm-call")]
    AardvarkRawCtxDirectPersistentWarmCall,
    #[value(name = "aardvark-rawctx-direct-owned-persistent-warm-call")]
    AardvarkRawCtxDirectOwnedPersistentWarmCall,
    #[value(name = "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio")]
    AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio,
    #[value(
        name = "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only"
    )]
    AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly,
    #[value(
        name = "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only-no-output-metadata"
    )]
    AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata,
    #[value(name = "aardvark-rawctx-registry-retained-direct-owned-warm-call-no-stdio")]
    AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio,
    #[value(name = "aardvark-rawctx-registry-retained-direct-owned-first-live-no-stdio")]
    AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio,
    #[value(name = "aardvark-rawctx-registry-retained-direct-owned-warm-all-first-live-no-stdio")]
    AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio,
    #[value(name = "aardvark-rawctx-persistent-warm-call")]
    AardvarkRawCtxPersistentWarmCall,
    #[value(name = "aardvark-rawctx-persistent-full")]
    AardvarkRawCtxPersistentFull,
    #[value(name = "aardvark-rawctx-persistent-shared")]
    AardvarkRawCtxPersistentShared,
    #[value(name = "aardvark-rawctx-persistent-none")]
    AardvarkRawCtxPersistentNone,
    #[value(
        name = "host-python-warm",
        alias = "host-python",
        alias = "host",
        alias = "python"
    )]
    HostPythonWarm,
    #[value(name = "host-python-prepare-run")]
    HostPythonPrepareRun,
    #[value(name = "host-python-process")]
    HostPythonProcess,
}

impl Mode {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Mode::AardvarkJsonCold => "aardvark-json-cold",
            Mode::AardvarkJsonWarm => "aardvark-json-warm",
            Mode::AardvarkJsonResetInPlace => "aardvark-json-reset-in-place",
            Mode::AardvarkJsonPersistent => "aardvark-json-persistent",
            Mode::AardvarkJsonPersistentWarmCall => "aardvark-json-persistent-warm-call",
            Mode::AardvarkJsonPersistentNoStdio => "aardvark-json-persistent-no-stdio",
            Mode::AardvarkJsonPersistentWarmCallNoStdio => {
                "aardvark-json-persistent-warm-call-no-stdio"
            }
            Mode::AardvarkJsonRegistryPersistentNoStdio => {
                "aardvark-json-registry-persistent-no-stdio"
            }
            Mode::AardvarkJsonRegistryPrepareEachCallNoStdio => {
                "aardvark-json-registry-prepare-each-call-no-stdio"
            }
            Mode::AardvarkJsonRegistryCachedHandlerNoStdio => {
                "aardvark-json-registry-cached-handler-no-stdio"
            }
            Mode::AardvarkJsonRegistryRetainedHandlerNoStdio => {
                "aardvark-json-registry-retained-handler-no-stdio"
            }
            Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio => {
                "aardvark-json-registry-retained-first-live-no-stdio"
            }
            Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio => {
                "aardvark-json-registry-retained-warm-all-first-live-no-stdio"
            }
            Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio => {
                "aardvark-json-warmed-host-pooled-warm-all-first-live-no-stdio"
            }
            Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio => {
                "aardvark-json-warmed-host-registry-pooled-warm-all-first-live-no-stdio"
            }
            Mode::AardvarkJsonPersistentFull => "aardvark-json-persistent-full",
            Mode::AardvarkJsonPersistentShared => "aardvark-json-persistent-shared",
            Mode::AardvarkJsonPersistentNone => "aardvark-json-persistent-none",
            Mode::AardvarkRawCtxCold => "aardvark-rawctx-cold",
            Mode::AardvarkRawCtxWarm => "aardvark-rawctx-warm",
            Mode::AardvarkRawCtxResetInPlace => "aardvark-rawctx-reset-in-place",
            Mode::AardvarkRawCtxPersistent => "aardvark-rawctx-persistent",
            Mode::AardvarkRawCtxPersistentNoStdio => "aardvark-rawctx-persistent-no-stdio",
            Mode::AardvarkRawCtxDirectPersistent => "aardvark-rawctx-direct-persistent",
            Mode::AardvarkRawCtxDirectOwnedPersistent => "aardvark-rawctx-direct-owned-persistent",
            Mode::AardvarkRawCtxDirectPersistentWarmCall => {
                "aardvark-rawctx-direct-persistent-warm-call"
            }
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall => {
                "aardvark-rawctx-direct-owned-persistent-warm-call"
            }
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio => {
                "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio"
            }
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly => {
                "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only"
            }
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata => {
                "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only-no-output-metadata"
            }
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio => {
                "aardvark-rawctx-registry-retained-direct-owned-warm-call-no-stdio"
            }
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio => {
                "aardvark-rawctx-registry-retained-direct-owned-first-live-no-stdio"
            }
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio => {
                "aardvark-rawctx-registry-retained-direct-owned-warm-all-first-live-no-stdio"
            }
            Mode::AardvarkRawCtxPersistentWarmCall => "aardvark-rawctx-persistent-warm-call",
            Mode::AardvarkRawCtxPersistentFull => "aardvark-rawctx-persistent-full",
            Mode::AardvarkRawCtxPersistentShared => "aardvark-rawctx-persistent-shared",
            Mode::AardvarkRawCtxPersistentNone => "aardvark-rawctx-persistent-none",
            Mode::HostPythonWarm => "host-python-warm",
            Mode::HostPythonPrepareRun => "host-python-prepare-run",
            Mode::HostPythonProcess => "host-python-process",
        }
    }

    pub(crate) fn invocation_kind(self) -> Option<InvocationKind> {
        match self {
            Mode::AardvarkJsonCold | Mode::AardvarkJsonWarm | Mode::AardvarkJsonResetInPlace => {
                Some(InvocationKind::Json)
            }
            Mode::AardvarkJsonPersistent
            | Mode::AardvarkJsonPersistentWarmCall
            | Mode::AardvarkJsonPersistentNoStdio
            | Mode::AardvarkJsonPersistentWarmCallNoStdio
            | Mode::AardvarkJsonRegistryPersistentNoStdio
            | Mode::AardvarkJsonRegistryPrepareEachCallNoStdio
            | Mode::AardvarkJsonRegistryCachedHandlerNoStdio
            | Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
            | Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
            | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonPersistentFull
            | Mode::AardvarkJsonPersistentShared
            | Mode::AardvarkJsonPersistentNone => Some(InvocationKind::Json),
            Mode::AardvarkRawCtxCold
            | Mode::AardvarkRawCtxWarm
            | Mode::AardvarkRawCtxResetInPlace => Some(InvocationKind::RawCtx),
            Mode::AardvarkRawCtxPersistent
            | Mode::AardvarkRawCtxPersistentNoStdio
            | Mode::AardvarkRawCtxDirectPersistent
            | Mode::AardvarkRawCtxDirectOwnedPersistent
            | Mode::AardvarkRawCtxDirectPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
            | Mode::AardvarkRawCtxPersistentWarmCall
            | Mode::AardvarkRawCtxPersistentFull
            | Mode::AardvarkRawCtxPersistentShared
            | Mode::AardvarkRawCtxPersistentNone => Some(InvocationKind::RawCtx),
            Mode::HostPythonWarm | Mode::HostPythonPrepareRun | Mode::HostPythonProcess => None,
        }
    }

    pub(crate) fn path_kind(self) -> Option<PathKind> {
        match self {
            Mode::AardvarkJsonCold | Mode::AardvarkRawCtxCold => Some(PathKind::Cold),
            Mode::AardvarkJsonWarm | Mode::AardvarkRawCtxWarm => Some(PathKind::Warm),
            Mode::AardvarkJsonResetInPlace | Mode::AardvarkRawCtxResetInPlace => {
                Some(PathKind::ResetInPlace)
            }
            Mode::AardvarkJsonPersistent
            | Mode::AardvarkJsonPersistentWarmCall
            | Mode::AardvarkJsonPersistentNoStdio
            | Mode::AardvarkJsonPersistentWarmCallNoStdio
            | Mode::AardvarkJsonRegistryPersistentNoStdio
            | Mode::AardvarkJsonRegistryPrepareEachCallNoStdio
            | Mode::AardvarkJsonRegistryCachedHandlerNoStdio
            | Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
            | Mode::AardvarkJsonPersistentFull
            | Mode::AardvarkJsonPersistentShared
            | Mode::AardvarkJsonPersistentNone
            | Mode::AardvarkRawCtxPersistent
            | Mode::AardvarkRawCtxPersistentNoStdio
            | Mode::AardvarkRawCtxDirectPersistent
            | Mode::AardvarkRawCtxDirectOwnedPersistent
            | Mode::AardvarkRawCtxDirectPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
            | Mode::AardvarkRawCtxPersistentWarmCall
            | Mode::AardvarkRawCtxPersistentFull
            | Mode::AardvarkRawCtxPersistentShared
            | Mode::AardvarkRawCtxPersistentNone => Some(PathKind::Persistent),
            Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
            | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio => {
                Some(PathKind::FirstLive)
            }
            Mode::HostPythonWarm | Mode::HostPythonPrepareRun | Mode::HostPythonProcess => None,
        }
    }

    pub(crate) fn cleanup_kind(self) -> Option<CleanupKind> {
        match self {
            Mode::AardvarkJsonPersistent
            | Mode::AardvarkJsonPersistentWarmCall
            | Mode::AardvarkJsonPersistentNoStdio
            | Mode::AardvarkJsonPersistentWarmCallNoStdio
            | Mode::AardvarkJsonRegistryPersistentNoStdio
            | Mode::AardvarkJsonRegistryPrepareEachCallNoStdio
            | Mode::AardvarkJsonRegistryCachedHandlerNoStdio
            | Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
            | Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
            | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
            | Mode::AardvarkRawCtxPersistent
            | Mode::AardvarkRawCtxPersistentNoStdio
            | Mode::AardvarkRawCtxDirectPersistent
            | Mode::AardvarkRawCtxDirectOwnedPersistent
            | Mode::AardvarkRawCtxDirectPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
            | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
            | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
            | Mode::AardvarkRawCtxPersistentWarmCall => Some(CleanupKind::SharedBuffersOnly),
            Mode::AardvarkJsonPersistentFull | Mode::AardvarkRawCtxPersistentFull => {
                Some(CleanupKind::Full)
            }
            Mode::AardvarkJsonPersistentShared | Mode::AardvarkRawCtxPersistentShared => {
                Some(CleanupKind::SharedBuffersOnly)
            }
            Mode::AardvarkJsonPersistentNone | Mode::AardvarkRawCtxPersistentNone => {
                Some(CleanupKind::None)
            }
            _ => None,
        }
    }

    pub(crate) fn aardvark_modes() -> &'static [Mode] {
        &[
            Mode::AardvarkJsonCold,
            Mode::AardvarkJsonWarm,
            Mode::AardvarkJsonResetInPlace,
            Mode::AardvarkJsonPersistent,
            Mode::AardvarkJsonPersistentWarmCall,
            Mode::AardvarkJsonPersistentNoStdio,
            Mode::AardvarkJsonPersistentWarmCallNoStdio,
            Mode::AardvarkJsonRegistryPersistentNoStdio,
            Mode::AardvarkJsonRegistryPrepareEachCallNoStdio,
            Mode::AardvarkJsonRegistryCachedHandlerNoStdio,
            Mode::AardvarkJsonRegistryRetainedHandlerNoStdio,
            Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio,
            Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio,
            Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio,
            Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio,
            Mode::AardvarkJsonPersistentFull,
            Mode::AardvarkJsonPersistentShared,
            Mode::AardvarkJsonPersistentNone,
            Mode::AardvarkRawCtxCold,
            Mode::AardvarkRawCtxWarm,
            Mode::AardvarkRawCtxResetInPlace,
            Mode::AardvarkRawCtxPersistent,
            Mode::AardvarkRawCtxPersistentNoStdio,
            Mode::AardvarkRawCtxDirectPersistent,
            Mode::AardvarkRawCtxDirectOwnedPersistent,
            Mode::AardvarkRawCtxDirectPersistentWarmCall,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata,
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio,
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio,
            Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio,
            Mode::AardvarkRawCtxPersistentWarmCall,
            Mode::AardvarkRawCtxPersistentFull,
            Mode::AardvarkRawCtxPersistentShared,
            Mode::AardvarkRawCtxPersistentNone,
        ]
    }

    pub(crate) fn host_modes() -> &'static [Mode] {
        &[
            Mode::HostPythonWarm,
            Mode::HostPythonPrepareRun,
            Mode::HostPythonProcess,
        ]
    }

    pub(crate) fn is_aardvark(self) -> bool {
        self.invocation_kind().is_some()
    }

    pub(crate) fn uses_explicit_warm_call(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonPersistentWarmCall
                | Mode::AardvarkJsonPersistentWarmCallNoStdio
                | Mode::AardvarkRawCtxPersistentWarmCall
                | Mode::AardvarkRawCtxDirectPersistentWarmCall
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_direct_rawctx_contract(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectPersistent
                | Mode::AardvarkRawCtxDirectOwnedPersistent
                | Mode::AardvarkRawCtxDirectPersistentWarmCall
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_owned_rawctx_inputs(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistent
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_registry_pool(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonRegistryPersistentNoStdio
                | Mode::AardvarkJsonRegistryPrepareEachCallNoStdio
                | Mode::AardvarkJsonRegistryCachedHandlerNoStdio
                | Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
                | Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
                | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_warmed_host(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_warmed_host_registry(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn prepares_handler_each_call(self) -> bool {
        matches!(self, Mode::AardvarkJsonRegistryPrepareEachCallNoStdio)
    }

    pub(crate) fn uses_registry_cached_handler(self) -> bool {
        matches!(self, Mode::AardvarkJsonRegistryCachedHandlerNoStdio)
    }

    pub(crate) fn uses_registry_retained_handler(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
                | Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
                | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_pool_wide_warmup(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_rawctx_shared_buffer_only_success(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn collects_rawctx_output_metadata(self) -> bool {
        !matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn uses_rawctx_flat_input_buffers(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    pub(crate) fn captures_stdio(self) -> bool {
        !matches!(
            self,
            Mode::AardvarkJsonPersistentNoStdio
                | Mode::AardvarkJsonPersistentWarmCallNoStdio
                | Mode::AardvarkJsonRegistryPersistentNoStdio
                | Mode::AardvarkJsonRegistryPrepareEachCallNoStdio
                | Mode::AardvarkJsonRegistryCachedHandlerNoStdio
                | Mode::AardvarkJsonRegistryRetainedHandlerNoStdio
                | Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio
                | Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxPersistentNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
        )
    }
}
