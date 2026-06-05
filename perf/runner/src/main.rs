use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aardvark_core::{
    config::{PyRuntimeConfig, ResetPolicy, WarmState},
    invocation::{FieldDescriptor, InvocationDescriptor},
    outcome::ResultPayload,
    pool::{PoolConfig, PoolResetMode, PyRuntimePool},
    strategy::{
        JsonInput, JsonInvocationStrategy, RawCtxBindingBuilder, RawCtxInput,
        RawCtxInvocationStrategy, RawCtxMetadata, RawCtxPublishBuilder,
    },
    Bundle, BundleArtifact, BundlePool, BundlePoolRegistry, CleanupMode, IsolateConfig,
    PoolOptions, PyRunnerError, PyRuntime, WarmedBundleHost, WarmedBundleHostOptions,
    WarmedBundleHostRegistry, WarmedBundleHostWarmup,
};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, Table};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use which::which;

mod perf;

#[derive(Parser, Debug)]
#[command(
    name = "aardvark-perf",
    about = "Performance harness for Aardvark runtime"
)]
enum Cli {
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
enum Scenario {
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
enum LoadProfile {
    None,
    Low,
    Medium,
    High,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum InvocationKind {
    Json,
    RawCtx,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PathKind {
    Cold,
    Warm,
    ResetInPlace,
    Persistent,
    FirstCall,
    FirstLive,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CleanupKind {
    Full,
    SharedBuffersOnly,
    None,
}

impl CleanupKind {
    fn label(self) -> &'static str {
        match self {
            CleanupKind::Full => "full",
            CleanupKind::SharedBuffersOnly => "shared-buffers-only",
            CleanupKind::None => "none",
        }
    }

    fn to_cleanup_mode(self) -> CleanupMode {
        match self {
            CleanupKind::Full => CleanupMode::Full,
            CleanupKind::SharedBuffersOnly => CleanupMode::SharedBuffersOnly,
            CleanupKind::None => CleanupMode::None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum Mode {
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
    fn name(&self) -> &'static str {
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

    fn invocation_kind(self) -> Option<InvocationKind> {
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

    fn path_kind(self) -> Option<PathKind> {
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

    fn cleanup_kind(self) -> Option<CleanupKind> {
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

    fn aardvark_modes() -> &'static [Mode] {
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

    fn host_modes() -> &'static [Mode] {
        &[
            Mode::HostPythonWarm,
            Mode::HostPythonPrepareRun,
            Mode::HostPythonProcess,
        ]
    }

    fn is_aardvark(self) -> bool {
        self.invocation_kind().is_some()
    }

    fn uses_explicit_warm_call(self) -> bool {
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

    fn uses_direct_rawctx_contract(self) -> bool {
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

    fn uses_owned_rawctx_inputs(self) -> bool {
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

    fn uses_registry_pool(self) -> bool {
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

    fn uses_warmed_host(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
        )
    }

    fn uses_warmed_host_registry(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
        )
    }

    fn prepares_handler_each_call(self) -> bool {
        matches!(self, Mode::AardvarkJsonRegistryPrepareEachCallNoStdio)
    }

    fn uses_registry_cached_handler(self) -> bool {
        matches!(self, Mode::AardvarkJsonRegistryCachedHandlerNoStdio)
    }

    fn uses_registry_retained_handler(self) -> bool {
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

    fn uses_pool_wide_warmup(self) -> bool {
        matches!(
            self,
            Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    fn uses_rawctx_shared_buffer_only_success(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly
                | Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    fn collects_rawctx_output_metadata(self) -> bool {
        !matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    fn uses_rawctx_flat_input_buffers(self) -> bool {
        matches!(
            self,
            Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio
                | Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio
        )
    }

    fn captures_stdio(self) -> bool {
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

#[derive(Clone, Serialize)]
struct BenchResult {
    scenario: Scenario,
    mode: Mode,
    profile: LoadProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    invocation: Option<InvocationKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup: Option<CleanupKind>,
    iterations: usize,
    total: TimingStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepare: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rss_mib: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_total: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_prepare: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_run: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_python_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_packages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    samples: Option<TimingSamples>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_breakdown: Option<SetupBreakdownStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_breakdown_samples: Option<SetupBreakdownSamples>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_pool_desired_size: Option<usize>,
}

#[derive(Serialize, serde::Deserialize, Default, Clone)]
struct TimingStats {
    avg_ms: f64,
    min_ms: f64,
    max_ms: f64,
    std_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Clone, Serialize)]
struct TimingSamples {
    total_ms: Vec<f64>,
    prepare_ms: Vec<f64>,
    run_ms: Vec<f64>,
}

#[derive(Clone, Serialize)]
struct SetupBreakdownStats {
    registry_init: TimingStats,
    artifact_parse: TimingStats,
    pool_create: TimingStats,
    handler_prepare: TimingStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    warm_all: Option<TimingStats>,
}

#[derive(Clone, Serialize)]
struct SetupBreakdownSamples {
    registry_init_ms: Vec<f64>,
    artifact_parse_ms: Vec<f64>,
    pool_create_ms: Vec<f64>,
    handler_prepare_ms: Vec<f64>,
    warm_all_ms: Vec<f64>,
}

#[derive(Default)]
struct SetupBreakdownBuckets {
    registry_init: Vec<Duration>,
    artifact_parse: Vec<Duration>,
    pool_create: Vec<Duration>,
    handler_prepare: Vec<Duration>,
    warm_all: Vec<Duration>,
}

struct TimingBuckets<'a> {
    prepare: &'a mut Vec<Duration>,
    run: &'a mut Vec<Duration>,
    total: &'a mut Vec<Duration>,
}

#[derive(Clone, Copy)]
struct AardvarkBenchContext {
    scenario: Scenario,
    profile: LoadProfile,
    invocation: InvocationKind,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli {
        Cli::All {
            iterations,
            json,
            csv,
            profile,
            samples,
            pyodide_profile,
            warm_preimports,
            manifest_preload_imports,
            setup_pool_desired_size,
        } => {
            let mut results = Vec::new();
            let profiles: Vec<LoadProfile> = match profile {
                Some(p) => vec![p],
                None => vec![
                    LoadProfile::None,
                    LoadProfile::Low,
                    LoadProfile::Medium,
                    LoadProfile::High,
                ],
            };
            for profile in profiles {
                for scenario in [
                    Scenario::Echo,
                    Scenario::Numpy,
                    Scenario::NumpyMatmul,
                    Scenario::Pandas,
                    Scenario::ScipySgemm,
                    Scenario::Tensor,
                    Scenario::Matplotlib,
                ] {
                    for mode in aardvark_modes_for_scenario(scenario) {
                        results.push(bench_aardvark(
                            scenario,
                            *mode,
                            AardvarkBenchOptions {
                                iterations,
                                profile,
                                include_samples: samples,
                                pyodide_profile: pyodide_profile.as_deref(),
                                warm_preimports: &warm_preimports,
                                manifest_preload_imports: &manifest_preload_imports,
                                setup_pool_desired_size,
                            },
                        )?);
                    }
                    for mode in host_modes_for_scenario(scenario) {
                        results.push(bench_host(scenario, *mode, iterations, profile)?);
                    }
                }
            }
            let expanded = expand_results(&results);
            if let Some(path) = json {
                write_json(&path, &expanded)?;
            }
            if let Some(path) = csv {
                write_csv(&path, &expanded)?;
            }
            print_summary(&expanded);
        }
        Cli::Scenario {
            scenario,
            mode,
            iterations,
            profile,
            samples,
            pyodide_profile,
            warm_preimports,
            manifest_preload_imports,
            setup_pool_desired_size,
        } => {
            let profile = profile.unwrap_or(LoadProfile::None);
            let result = if mode.is_aardvark() {
                bench_aardvark(
                    scenario,
                    mode,
                    AardvarkBenchOptions {
                        iterations,
                        profile,
                        include_samples: samples,
                        pyodide_profile: pyodide_profile.as_deref(),
                        warm_preimports: &warm_preimports,
                        manifest_preload_imports: &manifest_preload_imports,
                        setup_pool_desired_size,
                    },
                )?
            } else {
                bench_host(scenario, mode, iterations, profile)?
            };
            let expanded = expand_results(std::slice::from_ref(&result));
            println!("{}", serde_json::to_string_pretty(&expanded)?);
        }
    }
    Ok(())
}

struct AardvarkBenchOptions<'a> {
    iterations: usize,
    profile: LoadProfile,
    include_samples: bool,
    pyodide_profile: Option<&'a str>,
    warm_preimports: &'a [String],
    manifest_preload_imports: &'a [String],
    setup_pool_desired_size: usize,
}

fn bench_aardvark(
    scenario: Scenario,
    mode: Mode,
    options: AardvarkBenchOptions<'_>,
) -> Result<BenchResult> {
    let iterations = options.iterations;
    let profile = options.profile;
    let include_samples = options.include_samples;
    let pyodide_profile = options.pyodide_profile;
    let warm_preimports = options.warm_preimports;
    let setup_pool_desired_size = options.setup_pool_desired_size;
    let invocation = mode
        .invocation_kind()
        .ok_or_else(|| anyhow!("mode '{}' is not an Aardvark variant", mode.name()))?;
    let bench_context = AardvarkBenchContext {
        scenario,
        profile,
        invocation,
    };
    let path = mode
        .path_kind()
        .ok_or_else(|| anyhow!("mode '{}' is missing a path kind", mode.name()))?;
    let cleanup_kind = mode.cleanup_kind();
    let mut applied_cleanup = cleanup_kind;

    let python_source = scenario_source(scenario);
    let manifest = scenario_manifest(
        scenario,
        invocation,
        options.pyodide_profile,
        options.manifest_preload_imports,
    );
    let bundle_bytes = build_bundle_bytes(&python_source, manifest.as_bytes())?;
    let bundle = Bundle::from_zip_bytes(&bundle_bytes)?;
    let descriptor = if mode.uses_direct_rawctx_contract() && mode.captures_stdio() {
        None
    } else {
        descriptor_for(scenario, invocation, profile, mode)
    };

    let json_input = json_input_for(scenario, profile);
    let raw_inputs = Arc::new(rawctx_inputs_for(scenario, profile, mode)?);

    let mut prepare = Vec::with_capacity(iterations);
    let mut run = Vec::with_capacity(iterations);
    let mut total = Vec::with_capacity(iterations);
    let mut cold_total_stats: Option<TimingStats> = None;
    let mut cold_prepare_stats: Option<TimingStats> = None;
    let mut cold_run_stats: Option<TimingStats> = None;
    let mut setup_breakdown_buckets = SetupBreakdownBuckets::default();
    let mut has_setup_breakdown = false;

    match path {
        PathKind::Cold => {
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(runtime_config_for(pyodide_profile)?)?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    bench_context,
                    &mut runtime,
                    descriptor.as_ref(),
                    &bundle,
                    json_input.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::Warm => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let mut config = runtime_config_for(pyodide_profile)?;
            config.warm_state = Some(warm_state);
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(config.clone())?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    bench_context,
                    &mut runtime,
                    descriptor.as_ref(),
                    &bundle,
                    json_input.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::ResetInPlace => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let mut runtime_config = runtime_config_for(pyodide_profile)?;
            runtime_config.warm_state = Some(warm_state);
            runtime_config.reset_policy = ResetPolicy::Manual;
            let pool = PyRuntimePool::new(PoolConfig {
                max_runtimes: 1,
                runtime_config,
                reset_mode: PoolResetMode::InPlace,
            })?;
            for _ in 0..iterations {
                let mut handle = pool.checkout()?;
                {
                    let runtime = handle.runtime();
                    let mut buckets = TimingBuckets {
                        prepare: &mut prepare,
                        run: &mut run,
                        total: &mut total,
                    };
                    execute_iteration(
                        bench_context,
                        runtime,
                        descriptor.as_ref(),
                        &bundle,
                        json_input.clone(),
                        raw_inputs.as_ref(),
                        &mut buckets,
                    )?;
                }
                drop(handle);
            }
        }
        PathKind::FirstLive => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);

            let bench_cleanup = cleanup_kind.unwrap_or(CleanupKind::SharedBuffersOnly);
            applied_cleanup = Some(bench_cleanup);
            for _ in 0..iterations {
                let mut isolate_config = IsolateConfig {
                    runtime: runtime_config_for(pyodide_profile)?,
                    ..IsolateConfig::default()
                };
                isolate_config.runtime.warm_state = Some(warm_state.clone());
                isolate_config.cleanup = bench_cleanup.to_cleanup_mode();

                if mode.uses_warmed_host_registry() {
                    has_setup_breakdown = true;
                    let setup_start = Instant::now();
                    let registry_start = Instant::now();
                    let descriptor = descriptor
                        .clone()
                        .unwrap_or_else(|| InvocationDescriptor::new("main:entrypoint"));
                    let warmup = match invocation {
                        InvocationKind::Json => {
                            WarmedBundleHostWarmup::json_input(json_input.clone())
                        }
                        InvocationKind::RawCtx => WarmedBundleHostWarmup::rawctx(
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)?,
                        ),
                    };
                    let registry = WarmedBundleHostRegistry::new(
                        WarmedBundleHostOptions::pooled(PoolOptions {
                            isolate: isolate_config,
                            desired_size: setup_pool_desired_size,
                            max_size: setup_pool_desired_size.max(1),
                            telemetry_interval: None,
                            ..PoolOptions::default()
                        })
                        .with_descriptor(descriptor)
                        .with_warmup(warmup),
                    );
                    let host = registry.host_for_bytes(&bundle_bytes)?;
                    setup_breakdown_buckets
                        .registry_init
                        .push(registry_start.elapsed());
                    setup_breakdown_buckets.artifact_parse.push(Duration::ZERO);
                    setup_breakdown_buckets.pool_create.push(Duration::ZERO);
                    setup_breakdown_buckets.handler_prepare.push(Duration::ZERO);

                    let setup_elapsed = setup_start.elapsed();
                    let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                        Some(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)
                    } else {
                        None
                    };
                    let live_start = Instant::now();
                    let outcome = match invocation {
                        InvocationKind::Json => host.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => {
                            host.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                        }
                    };
                    let live_elapsed = live_start.elapsed();

                    validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                    prepare.push(setup_elapsed);
                    run.push(live_elapsed);
                    total.push(setup_elapsed + live_elapsed);
                    continue;
                }

                if mode.uses_warmed_host() {
                    has_setup_breakdown = true;
                    let setup_start = Instant::now();

                    setup_breakdown_buckets.registry_init.push(Duration::ZERO);

                    let artifact_start = Instant::now();
                    let artifact = BundleArtifact::from_bytes(&bundle_bytes)?;
                    setup_breakdown_buckets
                        .artifact_parse
                        .push(artifact_start.elapsed());

                    let host_start = Instant::now();
                    let descriptor = descriptor
                        .clone()
                        .unwrap_or_else(|| InvocationDescriptor::new("main:entrypoint"));
                    let host_options = WarmedBundleHostOptions::pooled(PoolOptions {
                        isolate: isolate_config,
                        desired_size: setup_pool_desired_size,
                        max_size: setup_pool_desired_size.max(1),
                        telemetry_interval: None,
                        ..PoolOptions::default()
                    })
                    .with_descriptor(descriptor);
                    let host = WarmedBundleHost::from_artifact(artifact, host_options)?;
                    setup_breakdown_buckets
                        .handler_prepare
                        .push(host_start.elapsed());

                    if mode.uses_pool_wide_warmup() {
                        let warm_all_start = Instant::now();
                        let outcomes = match invocation {
                            InvocationKind::Json => host
                                .warm_all(WarmedBundleHostWarmup::json_input(json_input.clone()))?,
                            InvocationKind::RawCtx => host.warm_all(
                                WarmedBundleHostWarmup::rawctx(rawctx_inputs_for_call(
                                    scenario,
                                    profile,
                                    raw_inputs.as_ref(),
                                    mode,
                                )?),
                            )?,
                        };
                        for outcome in &outcomes {
                            validate_aardvark_outcome(scenario, profile, invocation, outcome)?;
                        }
                        setup_breakdown_buckets
                            .warm_all
                            .push(warm_all_start.elapsed());
                    }

                    let setup_elapsed = setup_start.elapsed();
                    let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                        Some(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)
                    } else {
                        None
                    };
                    let live_start = Instant::now();
                    let outcome = match invocation {
                        InvocationKind::Json => host.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => {
                            host.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                        }
                    };
                    let live_elapsed = live_start.elapsed();

                    validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                    prepare.push(setup_elapsed);
                    run.push(live_elapsed);
                    total.push(setup_elapsed + live_elapsed);
                    continue;
                }

                let options = PoolOptions {
                    isolate: isolate_config,
                    desired_size: setup_pool_desired_size,
                    max_size: setup_pool_desired_size.max(1),
                    telemetry_interval: None,
                    ..PoolOptions::default()
                };

                has_setup_breakdown = true;
                let setup_start = Instant::now();

                let registry_start = Instant::now();
                let registry = BundlePoolRegistry::new(options)?;
                setup_breakdown_buckets
                    .registry_init
                    .push(registry_start.elapsed());

                let artifact_start = Instant::now();
                let artifact = BundleArtifact::from_bytes(&bundle_bytes)?;
                setup_breakdown_buckets
                    .artifact_parse
                    .push(artifact_start.elapsed());

                let pool_start = Instant::now();
                let _pool = registry.pool_for_artifact(artifact.clone())?;
                setup_breakdown_buckets
                    .pool_create
                    .push(pool_start.elapsed());

                let handler_start = Instant::now();
                let prepared =
                    registry.prepare_handler_for_artifact(artifact, descriptor.clone())?;
                setup_breakdown_buckets
                    .handler_prepare
                    .push(handler_start.elapsed());

                if mode.uses_pool_wide_warmup() {
                    let warm_all_start = Instant::now();
                    let outcomes = match invocation {
                        InvocationKind::Json => prepared.warm_all_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => prepared.warm_all_rawctx_with(|_| {
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)
                                .map_err(|err| PyRunnerError::Validation(err.to_string()))
                        })?,
                    };
                    for outcome in &outcomes {
                        validate_aardvark_outcome(scenario, profile, invocation, outcome)?;
                    }
                    setup_breakdown_buckets
                        .warm_all
                        .push(warm_all_start.elapsed());
                }
                let setup_elapsed = setup_start.elapsed();

                let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                    Some(rawctx_inputs_for_call(
                        scenario,
                        profile,
                        raw_inputs.as_ref(),
                        mode,
                    )?)
                } else {
                    None
                };
                let live_start = Instant::now();
                let outcome = match invocation {
                    InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                    InvocationKind::RawCtx => {
                        prepared.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                    }
                };
                let live_elapsed = live_start.elapsed();

                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                prepare.push(setup_elapsed);
                run.push(live_elapsed);
                total.push(setup_elapsed + live_elapsed);
            }
        }
        PathKind::Persistent => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);

            let mut isolate_config = IsolateConfig {
                runtime: runtime_config_for(pyodide_profile)?,
                ..IsolateConfig::default()
            };
            isolate_config.runtime.warm_state = Some(warm_state);
            let bench_cleanup = cleanup_kind.unwrap_or(CleanupKind::Full);
            isolate_config.cleanup = bench_cleanup.to_cleanup_mode();
            applied_cleanup = Some(bench_cleanup);

            let options = PoolOptions {
                isolate: isolate_config,
                telemetry_interval: None,
                ..PoolOptions::default()
            };

            let registry = if mode.uses_registry_pool() {
                Some(BundlePoolRegistry::new(options.clone())?)
            } else {
                None
            };
            let pool = if let Some(registry) = &registry {
                registry.pool_for_bytes(&bundle_bytes)?
            } else {
                let artifact = BundleArtifact::from_bundle(bundle.clone())?;
                BundlePool::from_artifact(artifact, options)?
            };
            let retained_prepared = if mode.uses_registry_retained_handler() {
                Some(
                    registry
                        .as_ref()
                        .expect("registry retained handler mode requires a registry")
                        .prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?,
                )
            } else {
                None
            };
            let handler = if mode.uses_registry_cached_handler() {
                if let Some(registry) = &registry {
                    let _ =
                        registry.prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?;
                }
                None
            } else if mode.uses_registry_retained_handler() {
                None
            } else {
                Some(match descriptor.clone() {
                    Some(desc) => pool.prepare_handler(Some(desc))?,
                    None => pool.prepare_default_handler()?,
                })
            };

            if mode.uses_explicit_warm_call() {
                let outcome = if let Some(prepared) = retained_prepared.as_ref() {
                    match invocation {
                        InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => prepared.call_rawctx(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)?,
                    }
                } else {
                    let handler = handler
                        .as_ref()
                        .expect("explicit warm call modes prepare a local handler");
                    match invocation {
                        InvocationKind::Json => {
                            pool.warm_json_input(handler, json_input.clone())?
                        }
                        InvocationKind::RawCtx => pool.warm_rawctx(
                            handler,
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)?,
                        )?,
                    }
                };
                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
            }

            for _ in 0..iterations {
                let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                    Some(rawctx_inputs_for_call(
                        scenario,
                        profile,
                        raw_inputs.as_ref(),
                        mode,
                    )?)
                } else {
                    None
                };
                let start = Instant::now();
                let outcome =
                    if let Some(prepared) = retained_prepared.as_ref() {
                        match invocation {
                            InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                            InvocationKind::RawCtx => prepared
                                .call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?,
                        }
                    } else if mode.uses_registry_cached_handler() {
                        let prepared = registry
                            .as_ref()
                            .expect("registry cached handler mode requires a registry")
                            .prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?;
                        match invocation {
                            InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                            InvocationKind::RawCtx => prepared
                                .call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?,
                        }
                    } else {
                        let pool_for_call = if let Some(registry) = &registry {
                            registry.pool_for_bytes(&bundle_bytes)?
                        } else {
                            pool.clone()
                        };
                        let local_handler;
                        let handler_for_call = if mode.prepares_handler_each_call() {
                            local_handler = match descriptor.clone() {
                                Some(desc) => pool_for_call.prepare_handler(Some(desc))?,
                                None => pool_for_call.prepare_default_handler()?,
                            };
                            &local_handler
                        } else {
                            handler
                                .as_ref()
                                .expect("persistent mode should have a prepared handler")
                        };
                        match invocation {
                            InvocationKind::Json => pool_for_call
                                .call_json_input(handler_for_call, json_input.clone())?,
                            InvocationKind::RawCtx => pool_for_call.call_rawctx(
                                handler_for_call,
                                raw_inputs_for_iteration.unwrap_or_default(),
                            )?,
                        }
                    };
                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                let total_elapsed = start.elapsed();
                let prepare_ms = outcome.diagnostics.prepare_ms.unwrap_or(0);
                let cleanup_ms = outcome.diagnostics.cleanup_ms.unwrap_or(0);
                let prepare_duration = Duration::from_millis(prepare_ms);
                let cleanup_duration = Duration::from_millis(cleanup_ms);
                let mut run_duration = total_elapsed
                    .checked_sub(prepare_duration)
                    .unwrap_or_default();
                run_duration = run_duration
                    .checked_sub(cleanup_duration)
                    .unwrap_or_default();

                prepare.push(prepare_duration);
                run.push(run_duration);
                total.push(total_elapsed);
            }
        }
        PathKind::FirstCall => unreachable!("first-call path is synthesized post-run"),
    }

    Ok(BenchResult {
        scenario,
        mode,
        profile,
        invocation: Some(invocation),
        path: Some(path),
        cleanup: applied_cleanup,
        iterations,
        total: timing_stats(&total),
        prepare: Some(timing_stats(&prepare)),
        run: Some(timing_stats(&run)),
        rss_mib: current_rss_mib(),
        cold_total: cold_total_stats,
        cold_prepare: cold_prepare_stats,
        cold_run: cold_run_stats,
        host_python_version: None,
        host_packages: None,
        samples: timing_samples(include_samples, &total, &prepare, &run),
        setup_breakdown: has_setup_breakdown
            .then(|| setup_breakdown_stats(&setup_breakdown_buckets)),
        setup_breakdown_samples: (has_setup_breakdown && include_samples)
            .then(|| setup_breakdown_samples(&setup_breakdown_buckets)),
        setup_pool_desired_size: has_setup_breakdown.then_some(setup_pool_desired_size),
    })
}

fn execute_iteration(
    bench_context: AardvarkBenchContext,
    runtime: &mut PyRuntime,
    descriptor: Option<&InvocationDescriptor>,
    bundle: &Bundle,
    json_input: Option<JsonInput>,
    raw_inputs: &[RawCtxInput],
    timings: &mut TimingBuckets<'_>,
) -> Result<()> {
    let prep_start = Instant::now();
    let (session, _) = match descriptor {
        Some(desc) => {
            runtime.prepare_session_with_manifest_and_descriptor(bundle.clone(), desc.clone())?
        }
        None => runtime.prepare_session_with_manifest(bundle.clone())?,
    };
    let prep_elapsed = prep_start.elapsed();

    let run_start = Instant::now();
    let outcome = match bench_context.invocation {
        InvocationKind::Json => {
            let mut strategy = JsonInvocationStrategy::with_input(json_input);
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
        InvocationKind::RawCtx => {
            let mut strategy = RawCtxInvocationStrategy::new(raw_inputs.to_vec());
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
    };
    let run_elapsed = run_start.elapsed();

    if !outcome.is_success() {
        return Err(anyhow!(
            "handler failed: {:?}; diagnostics: {:?}",
            outcome.status,
            outcome.diagnostics
        ));
    }

    validate_aardvark_outcome(
        bench_context.scenario,
        bench_context.profile,
        bench_context.invocation,
        &outcome,
    )?;

    timings.prepare.push(prep_elapsed);
    timings.run.push(run_elapsed);
    timings.total.push(prep_elapsed + run_elapsed);

    Ok(())
}

fn validate_aardvark_outcome(
    scenario: Scenario,
    profile: LoadProfile,
    invocation: InvocationKind,
    outcome: &aardvark_core::ExecutionOutcome,
) -> Result<()> {
    if !outcome.is_success() {
        return Err(anyhow!(
            "handler failed: {:?}; diagnostics: {:?}",
            outcome.status,
            outcome.diagnostics
        ));
    }

    match invocation {
        InvocationKind::Json => validate_json_outcome(scenario, profile, outcome),
        InvocationKind::RawCtx => validate_rawctx_outcome(scenario, profile, outcome),
    }
}

fn validate_json_outcome(
    scenario: Scenario,
    profile: LoadProfile,
    outcome: &aardvark_core::ExecutionOutcome,
) -> Result<()> {
    if matches!(scenario, Scenario::Tensor) {
        return validate_tensor_buffer_outcome("tensor JSON", profile, outcome);
    }

    if matches!(scenario, Scenario::Echo) {
        let expected_len = perf::echo_payload(profile)
            .map(|payload| payload.len())
            .unwrap_or("aardvark".len());
        if let Some(ResultPayload::SharedBuffers(buffers)) = outcome.payload() {
            if buffers.len() != 1 {
                return Err(anyhow!(
                    "echo JSON shared-buffer payload returned {} buffers, expected 1",
                    buffers.len()
                ));
            }
            let bytes = buffers[0]
                .as_slice()
                .ok_or_else(|| anyhow!("echo JSON buffer did not retain bytes"))?;
            if bytes.len() != expected_len {
                return Err(anyhow!(
                    "echo JSON shared-buffer length {} did not match expected {}",
                    bytes.len(),
                    expected_len
                ));
            }
            return Ok(());
        }
    }

    let Some(ResultPayload::Json(value)) = outcome.payload() else {
        return Err(anyhow!("json run did not return a JSON payload"));
    };

    match scenario {
        Scenario::Echo => {
            let expected_len = perf::echo_payload(profile)
                .map(|payload| payload.len())
                .unwrap_or("aardvark".len());
            let Some(text) = value.as_str() else {
                return Err(anyhow!("echo JSON payload was not a string"));
            };
            if text.len() != expected_len {
                return Err(anyhow!(
                    "echo JSON payload length {} did not match expected {}",
                    text.len(),
                    expected_len
                ));
            }
        }
        Scenario::Numpy => {
            let Some(number) = value.as_f64() else {
                return Err(anyhow!("numpy JSON payload was not numeric"));
            };
            if !number.is_finite() {
                return Err(anyhow!("numpy JSON payload was not finite"));
            }
            validate_numpy_total(profile, number, "JSON")?;
        }
        Scenario::NumpyMatmul | Scenario::ScipySgemm => {
            let Some(number) = value.as_f64() else {
                return Err(anyhow!("matrix JSON payload was not numeric"));
            };
            validate_matrix_total(number, "JSON")?;
        }
        Scenario::Pandas => {
            let Some(object) = value.as_object() else {
                return Err(anyhow!("pandas JSON payload was not an object"));
            };
            if object.is_empty() {
                return Err(anyhow!("pandas JSON payload was empty"));
            }
        }
        Scenario::Tensor => unreachable!("tensor JSON is validated as a shared buffer"),
        Scenario::Matplotlib => {
            let Some(byte_count) = value.as_u64() else {
                return Err(anyhow!(
                    "matplotlib JSON payload was not an unsigned byte count"
                ));
            };
            if byte_count == 0 {
                return Err(anyhow!("matplotlib JSON payload byte count was zero"));
            }
        }
    }

    Ok(())
}

fn validate_tensor_buffer_outcome(
    label: &str,
    profile: LoadProfile,
    outcome: &aardvark_core::ExecutionOutcome,
) -> Result<()> {
    let Some(ResultPayload::SharedBuffers(buffers)) = outcome.payload() else {
        return Err(anyhow!("{label} payload was not a shared buffer"));
    };
    if buffers.len() != 1 {
        return Err(anyhow!(
            "{label} returned {} shared buffers, expected 1",
            buffers.len()
        ));
    }
    let bytes = buffers[0]
        .as_slice()
        .ok_or_else(|| anyhow!("{label} buffer did not retain bytes"))?;
    let expected_len = perf::tensor_length(profile) * std::mem::size_of::<f32>();
    if bytes.len() != expected_len {
        return Err(anyhow!(
            "{label} buffer length {} did not match expected {}",
            bytes.len(),
            expected_len
        ));
    }
    Ok(())
}

fn validate_pandas_bytes(label: &str, profile: LoadProfile, bytes: &[u8]) -> Result<()> {
    if bytes.len() < 4 {
        return Err(anyhow!("{label} payload was too short"));
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().expect("slice length checked")) as usize;
    let rows = perf::pandas_rows(profile).unwrap_or(128);
    let expected_count = usize::try_from(rows.min(128)).unwrap_or(128);
    let expected_len = 4 + expected_count * 12;
    if count != expected_count || bytes.len() != expected_len {
        return Err(anyhow!(
            "{label} payload count/len {}/{} did not match expected {}/{}",
            count,
            bytes.len(),
            expected_count,
            expected_len
        ));
    }
    Ok(())
}

fn validate_rawctx_outcome(
    scenario: Scenario,
    profile: LoadProfile,
    outcome: &aardvark_core::ExecutionOutcome,
) -> Result<()> {
    let Some(ResultPayload::SharedBuffers(buffers)) = outcome.payload() else {
        return Err(anyhow!("rawctx run did not return shared buffers"));
    };
    if buffers.len() != 1 {
        return Err(anyhow!(
            "rawctx run returned {} shared buffers, expected 1",
            buffers.len()
        ));
    }

    let buffer = &buffers[0];
    let expected_id = match scenario {
        Scenario::Echo => "echo-output",
        Scenario::Numpy => "numpy-output",
        Scenario::NumpyMatmul => "numpy-matmul-output",
        Scenario::Pandas => "pandas-output",
        Scenario::ScipySgemm => "scipy-sgemm-output",
        Scenario::Tensor => "tensor-output",
        Scenario::Matplotlib => "matplotlib-output",
    };
    if buffer.id != expected_id {
        return Err(anyhow!(
            "rawctx buffer id '{}' did not match expected '{}'",
            buffer.id,
            expected_id
        ));
    }

    let bytes = buffer
        .as_slice()
        .ok_or_else(|| anyhow!("rawctx buffer '{}' did not retain bytes", buffer.id))?;

    match scenario {
        Scenario::Echo => {
            let expected_len = perf::echo_payload(profile)
                .map(|payload| payload.len())
                .unwrap_or("aardvark".len());
            if bytes.len() != expected_len {
                return Err(anyhow!(
                    "echo rawctx payload length {} did not match expected {}",
                    bytes.len(),
                    expected_len
                ));
            }
        }
        Scenario::Numpy => {
            if bytes.len() != 8 {
                return Err(anyhow!(
                    "numpy rawctx payload length {} did not match expected 8",
                    bytes.len()
                ));
            }
            let total = f64::from_le_bytes(bytes[0..8].try_into().expect("slice length checked"));
            if !total.is_finite() {
                return Err(anyhow!("numpy rawctx payload was not finite"));
            }
            validate_numpy_total(profile, total, "rawctx")?;
        }
        Scenario::NumpyMatmul | Scenario::ScipySgemm => {
            if bytes.len() != 8 {
                return Err(anyhow!(
                    "matrix rawctx payload length {} did not match expected 8",
                    bytes.len()
                ));
            }
            let total = f64::from_le_bytes(bytes[0..8].try_into().expect("slice length checked"));
            validate_matrix_total(total, "rawctx")?;
        }
        Scenario::Pandas => {
            validate_pandas_bytes("pandas rawctx", profile, bytes)?;
        }
        Scenario::Tensor => {
            let expected_len = perf::tensor_length(profile) * std::mem::size_of::<f32>();
            if bytes.len() != expected_len {
                return Err(anyhow!(
                    "tensor rawctx payload length {} did not match expected {}",
                    bytes.len(),
                    expected_len
                ));
            }
        }
        Scenario::Matplotlib => {
            if bytes.len() != 8 {
                return Err(anyhow!(
                    "matplotlib rawctx payload length {} did not match expected 8",
                    bytes.len()
                ));
            }
            let byte_count =
                u64::from_le_bytes(bytes[0..8].try_into().expect("slice length checked"));
            if byte_count == 0 {
                return Err(anyhow!("matplotlib rawctx byte count was zero"));
            }
        }
    }

    Ok(())
}

fn validate_numpy_total(profile: LoadProfile, total: f64, invocation: &str) -> Result<()> {
    let size = perf::numpy_size(profile).unwrap_or(64) as f64;
    let lower = size * 0.25;
    let upper = size * 0.75;
    if !(lower..=upper).contains(&total) {
        return Err(anyhow!(
            "numpy {invocation} total {total} was outside expected range [{lower}, {upper}] for profile {}",
            profile.name()
        ));
    }
    Ok(())
}

fn validate_matrix_total(total: f64, invocation: &str) -> Result<()> {
    if !total.is_finite() || total <= 0.0 {
        return Err(anyhow!(
            "matrix {invocation} total {total} was not a positive finite value"
        ));
    }
    Ok(())
}

fn capture_warm_state(
    bench_context: AardvarkBenchContext,
    bundle: &Bundle,
    descriptor: Option<&InvocationDescriptor>,
    json_input: Option<JsonInput>,
    raw_inputs: &[RawCtxInput],
    pyodide_profile: Option<&str>,
    warm_preimports: &[String],
) -> Result<(WarmState, TimingStats, TimingStats, TimingStats)> {
    let mut baseline_runtime = PyRuntime::new(runtime_config_for(pyodide_profile)?)?;
    let mut cold_prepare = Vec::with_capacity(1);
    let mut cold_run = Vec::with_capacity(1);
    let mut cold_total = Vec::with_capacity(1);
    {
        let mut buckets = TimingBuckets {
            prepare: &mut cold_prepare,
            run: &mut cold_run,
            total: &mut cold_total,
        };
        execute_iteration(
            bench_context,
            &mut baseline_runtime,
            descriptor,
            bundle,
            json_input.clone(),
            raw_inputs,
            &mut buckets,
        )?;
    }
    drop(baseline_runtime);

    let mut warm_config = runtime_config_for(pyodide_profile)?;
    warm_config.snapshot.save_to = Some(PathBuf::from("target/perf/bench_warm_snapshot.bin"));
    if !warm_preimports.is_empty() {
        let scripts = warm_preimport_scripts(warm_preimports)?;
        warm_config.hooks.before_warm_snapshot = Some(Arc::new(move |runtime| {
            for script in scripts.iter() {
                runtime.js_runtime().run_python_snippet(script)?;
            }
            Ok(())
        }));
    }
    let mut runtime = PyRuntime::new(warm_config)?;
    if let Some(desc) = descriptor {
        runtime.prepare_session_with_manifest_and_descriptor(bundle.clone(), desc.clone())?;
    } else {
        runtime.prepare_session_with_manifest(bundle.clone())?;
    }
    let warm_state = runtime.capture_warm_state()?;
    Ok((
        warm_state,
        timing_stats(&cold_total),
        timing_stats(&cold_prepare),
        timing_stats(&cold_run),
    ))
}

fn warm_preimport_scripts(modules: &[String]) -> Result<Arc<Vec<String>>> {
    let mut scripts = Vec::with_capacity(modules.len());
    for module in modules {
        let module = module.trim();
        if module.is_empty() {
            continue;
        }
        let literal = serde_json::to_string(module)
            .map_err(|err| anyhow!("failed to encode warm preimport module {module}: {err}"))?;
        scripts.push(format!("__import__({literal})"));
    }
    Ok(Arc::new(scripts))
}

fn runtime_config_for(pyodide_profile: Option<&str>) -> Result<PyRuntimeConfig> {
    let mut config = PyRuntimeConfig::default();
    if let Some(profile) = pyodide_profile {
        config.set_pyodide_distribution_profile(profile)?;
    }
    Ok(config)
}

fn bench_host(
    scenario: Scenario,
    mode: Mode,
    iterations: usize,
    profile: LoadProfile,
) -> Result<BenchResult> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/run_host.py");
    let uv = which("uv")
        .context("`uv` command not found on PATH. Install from https://docs.astral.sh/uv/ or ensure it is available before running the perf suite.")?;
    let host_plan = host_runtime_plan(scenario)?;
    let mut cmd = Command::new(uv);
    cmd.arg("run");
    cmd.arg(format!("--python={}", host_plan.python_version));
    for pkg in &host_plan.packages {
        cmd.arg(format!("--with={pkg}"));
    }
    cmd.arg("python");
    cmd.arg(script);
    cmd.arg("--scenario");
    cmd.arg(scenario.name());
    cmd.arg("--iterations");
    cmd.arg(iterations.to_string());
    cmd.arg("--profile");
    cmd.arg(profile.name());
    cmd.arg("--host-mode");
    cmd.arg(host_mode_name(mode)?);

    let output = cmd
        .output()
        .with_context(|| "failed to run host python benchmark")?;
    if !output.status.success() {
        return Err(anyhow!(
            "host benchmark failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let result: HostResult = serde_json::from_slice(&output.stdout)
        .with_context(|| "failed to parse host benchmark output")?;
    Ok(BenchResult {
        scenario,
        mode,
        profile,
        invocation: None,
        path: None,
        cleanup: None,
        iterations,
        total: result.total,
        prepare: result.prepare,
        run: result.run,
        rss_mib: Some(result.rss_mib),
        cold_total: None,
        cold_prepare: None,
        cold_run: None,
        host_python_version: result.python_version.or(Some(host_plan.python_version)),
        host_packages: Some(host_plan.packages),
        samples: None,
        setup_breakdown: None,
        setup_breakdown_samples: None,
        setup_pool_desired_size: None,
    })
}

fn scenario_source(scenario: Scenario) -> String {
    match scenario {
        Scenario::Echo => perf::echo_script().to_owned(),
        Scenario::Numpy => perf::numpy_script().to_owned(),
        Scenario::NumpyMatmul => perf::numpy_matmul_script().to_owned(),
        Scenario::Pandas => perf::pandas_script().to_owned(),
        Scenario::ScipySgemm => perf::scipy_sgemm_script().to_owned(),
        Scenario::Tensor => perf::tensor_script().to_owned(),
        Scenario::Matplotlib => perf::matplotlib_script().to_owned(),
    }
}

fn scenario_manifest(
    scenario: Scenario,
    invocation: InvocationKind,
    pyodide_profile: Option<&str>,
    manifest_preload_imports: &[String],
) -> String {
    let packages = scenario_packages(scenario);
    let mut manifest = json!({
        "schemaVersion": "1.0",
        "entrypoint": "main:entrypoint",
        "packages": packages,
    });
    if let Some(profile) = pyodide_profile {
        manifest["runtime"] = json!({
            "language": "python",
            "pyodide": {"profile": profile},
        });
    }
    if !manifest_preload_imports.is_empty() {
        let runtime = manifest
            .as_object_mut()
            .expect("manifest should be a JSON object")
            .entry("runtime")
            .or_insert_with(|| json!({"language": "python"}));
        let runtime_obj = runtime
            .as_object_mut()
            .expect("runtime should be a JSON object");
        runtime_obj
            .entry("language")
            .or_insert_with(|| json!("python"));
        let pyodide = runtime_obj.entry("pyodide").or_insert_with(|| json!({}));
        pyodide
            .as_object_mut()
            .expect("pyodide should be a JSON object")
            .insert("preloadImports".to_owned(), json!(manifest_preload_imports));
    }
    if matches!(invocation, InvocationKind::RawCtx) {
        manifest["resources"] = json!({
            "hostCapabilities": ["rawctx_buffers"],
        });
    }
    manifest.to_string()
}

fn descriptor_for(
    scenario: Scenario,
    invocation: InvocationKind,
    _profile: LoadProfile,
    mode: Mode,
) -> Option<InvocationDescriptor> {
    match invocation {
        InvocationKind::Json => (!mode.captures_stdio())
            .then(|| InvocationDescriptor::new("main:entrypoint").with_capture_stdio(false)),
        InvocationKind::RawCtx => {
            let mut descriptor = InvocationDescriptor::new("main:entrypoint");
            if !mode.captures_stdio() {
                descriptor = descriptor.with_capture_stdio(false);
            }
            if mode.uses_rawctx_shared_buffer_only_success() {
                descriptor = descriptor.with_rawctx_shared_buffer_only_success(true);
            }
            if !mode.collects_rawctx_output_metadata() {
                descriptor = descriptor.with_rawctx_output_metadata(false);
            }
            if mode.uses_rawctx_flat_input_buffers() {
                descriptor = descriptor.with_rawctx_flat_input_buffers(true);
            }
            if mode.uses_direct_rawctx_contract() {
                return Some(descriptor);
            }
            let metadata = match scenario {
                Scenario::Echo => RawCtxPublishBuilder::new("echo-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "echo", "profile": _profile.name()}))
                    .build(),
                Scenario::Numpy => RawCtxPublishBuilder::new("numpy-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "numpy", "profile": _profile.name()}))
                    .build(),
                Scenario::NumpyMatmul => RawCtxPublishBuilder::new("numpy-matmul-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "numpy-matmul", "profile": _profile.name()}))
                    .build(),
                Scenario::Pandas => RawCtxPublishBuilder::new("pandas-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "i32_f64_pairs",
                        "fields": ["category", "value_mean"],
                        "profile": _profile.name(),
                    }))
                    .build(),
                Scenario::ScipySgemm => RawCtxPublishBuilder::new("scipy-sgemm-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "scipy-sgemm", "profile": _profile.name()}))
                    .build(),
                Scenario::Tensor => RawCtxPublishBuilder::new("tensor-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "f32_le",
                        "profile": _profile.name(),
                    }))
                    .build(),
                Scenario::Matplotlib => RawCtxPublishBuilder::new("matplotlib-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "u64_le",
                        "profile": _profile.name(),
                    }))
                    .build(),
            };
            descriptor.outputs.push(FieldDescriptor {
                name: "result".to_owned(),
                type_tag: None,
                metadata: Some(metadata),
            });
            if matches!(scenario, Scenario::Tensor) {
                descriptor.inputs.push(FieldDescriptor {
                    name: "tensor".to_owned(),
                    type_tag: None,
                    metadata: Some(
                        RawCtxBindingBuilder::new()
                            .raw_arg("tensor_payload")
                            .optional(true)
                            .build(),
                    ),
                });
            }
            Some(descriptor)
        }
    }
}

fn scenario_packages(scenario: Scenario) -> &'static [&'static str] {
    match scenario {
        Scenario::Echo => &[],
        Scenario::Numpy => &["numpy"],
        Scenario::NumpyMatmul => &["numpy"],
        Scenario::Pandas => &["numpy", "pandas"],
        Scenario::ScipySgemm => &["scipy"],
        Scenario::Tensor => &["numpy"],
        Scenario::Matplotlib => &["numpy", "matplotlib"],
    }
}

fn build_bundle_bytes(source: &str, manifest: &[u8]) -> Result<Vec<u8>> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("main.py", options)?;
        writer.write_all(source.as_bytes())?;
        writer.start_file("aardvark.manifest.json", options)?;
        writer.write_all(manifest)?;
        writer.finish()?;
    }
    Ok(buffer)
}

fn timing_stats(samples: &[Duration]) -> TimingStats {
    if samples.is_empty() {
        return TimingStats::default();
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    let avg_ms = ms.iter().sum::<f64>() / ms.len() as f64;
    let min_ms = ms.iter().fold(f64::INFINITY, |acc, &val| acc.min(val));
    let max_ms = ms.iter().fold(f64::NEG_INFINITY, |acc, &val| acc.max(val));
    let variance = ms
        .iter()
        .map(|value| {
            let diff = value - avg_ms;
            diff * diff
        })
        .sum::<f64>()
        / ms.len() as f64;
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let percentile = |pct: f64| -> f64 {
        if ms.len() == 1 {
            return ms[0];
        }
        let position = pct * (ms.len() as f64 - 1.0);
        let lower = position.floor() as usize;
        let upper = position.ceil() as usize;
        if lower == upper {
            ms[lower]
        } else {
            let weight = position - lower as f64;
            ms[lower] * (1.0 - weight) + ms[upper] * weight
        }
    };

    TimingStats {
        avg_ms,
        min_ms,
        max_ms,
        std_ms: variance.sqrt(),
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
    }
}

fn timing_samples(
    include_samples: bool,
    total: &[Duration],
    prepare: &[Duration],
    run: &[Duration],
) -> Option<TimingSamples> {
    include_samples.then(|| TimingSamples {
        total_ms: durations_ms(total),
        prepare_ms: durations_ms(prepare),
        run_ms: durations_ms(run),
    })
}

fn setup_breakdown_stats(buckets: &SetupBreakdownBuckets) -> SetupBreakdownStats {
    SetupBreakdownStats {
        registry_init: timing_stats(&buckets.registry_init),
        artifact_parse: timing_stats(&buckets.artifact_parse),
        pool_create: timing_stats(&buckets.pool_create),
        handler_prepare: timing_stats(&buckets.handler_prepare),
        warm_all: (!buckets.warm_all.is_empty()).then(|| timing_stats(&buckets.warm_all)),
    }
}

fn setup_breakdown_samples(buckets: &SetupBreakdownBuckets) -> SetupBreakdownSamples {
    SetupBreakdownSamples {
        registry_init_ms: durations_ms(&buckets.registry_init),
        artifact_parse_ms: durations_ms(&buckets.artifact_parse),
        pool_create_ms: durations_ms(&buckets.pool_create),
        handler_prepare_ms: durations_ms(&buckets.handler_prepare),
        warm_all_ms: durations_ms(&buckets.warm_all),
    }
}

fn durations_ms(samples: &[Duration]) -> Vec<f64> {
    samples
        .iter()
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .collect()
}

fn expand_results(results: &[BenchResult]) -> Vec<BenchResult> {
    let mut expanded = Vec::new();
    for result in results {
        if let (Some(cold_total), Some(cold_prepare), Some(cold_run)) =
            (&result.cold_total, &result.cold_prepare, &result.cold_run)
        {
            let mut first = result.clone();
            first.path = Some(PathKind::FirstCall);
            first.iterations = 1;
            first.total = cold_total.clone();
            first.prepare = Some(cold_prepare.clone());
            first.run = Some(cold_run.clone());
            first.cold_total = None;
            first.cold_prepare = None;
            first.cold_run = None;
            first.samples = None;
            first.setup_breakdown = None;
            first.setup_breakdown_samples = None;
            expanded.push(first);
        }

        let mut steady = result.clone();
        steady.cold_total = None;
        steady.cold_prepare = None;
        steady.cold_run = None;
        expanded.push(steady);
    }
    expanded
}

fn write_json(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(serde_json::to_string_pretty(results)?.as_bytes())?;
    Ok(())
}

fn write_csv(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    writeln!(
        file,
        "scenario,profile,mode,invocation,path,cleanup,iterations,avg_ms,min_ms,max_ms,std_ms,p50_ms,p95_ms,p99_ms,rss_mib,prepare_avg_ms,run_avg_ms"
    )?;
    for result in results {
        let prepare_avg = result
            .prepare
            .as_ref()
            .map(|s| format!("{:.2}", s.avg_ms))
            .unwrap_or_default();
        let run_avg = result
            .run
            .as_ref()
            .map(|s| format!("{:.2}", s.avg_ms))
            .unwrap_or_default();
        let path = result
            .path
            .map(|mode| match mode {
                PathKind::Cold => "cold",
                PathKind::Warm => "warm",
                PathKind::ResetInPlace => "reset-in-place",
                PathKind::Persistent => "persistent",
                PathKind::FirstCall => "first-call",
                PathKind::FirstLive => "first-live",
            })
            .unwrap_or("-");
        let cleanup = result.cleanup.map(|kind| kind.label()).unwrap_or("-");
        writeln!(
            file,
            "{},{},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{}",
            result.scenario.name(),
            result.profile.name(),
            result.mode.name(),
            result
                .invocation
                .map(|kind| match kind {
                    InvocationKind::Json => "json",
                    InvocationKind::RawCtx => "rawctx",
                })
                .unwrap_or("-"),
            path,
            cleanup,
            result.iterations,
            result.total.avg_ms,
            result.total.min_ms,
            result.total.max_ms,
            result.total.std_ms,
            result.total.p50_ms,
            result.total.p95_ms,
            result.total.p99_ms,
            result.rss_mib.unwrap_or_default(),
            prepare_avg,
            run_avg,
        )?;
    }
    Ok(())
}

fn print_summary(results: &[BenchResult]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header([
        "Scenario",
        "Profile",
        "Mode",
        "Invocation",
        "Path",
        "Cleanup",
        "Iter",
        "Avg ms",
        "Min ms",
        "Max ms",
        "Std ms",
        "P50 ms",
        "P95 ms",
        "P99 ms",
        "RSS (MiB)",
    ]);

    for r in results {
        let invocation = r
            .invocation
            .map(|kind| match kind {
                InvocationKind::Json => "json",
                InvocationKind::RawCtx => "rawctx",
            })
            .unwrap_or("-");
        let path = r
            .path
            .map(|kind| match kind {
                PathKind::Cold => "cold",
                PathKind::Warm => "warm",
                PathKind::ResetInPlace => "reset-in-place",
                PathKind::Persistent => "persistent",
                PathKind::FirstCall => "first-call",
                PathKind::FirstLive => "first-live",
            })
            .unwrap_or("-");
        let cleanup = r
            .cleanup
            .map(|kind| kind.label().to_string())
            .unwrap_or_else(|| "-".to_string());

        let rss = r
            .rss_mib
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());

        table.add_row(vec![
            Cell::new(r.scenario.name()),
            Cell::new(r.profile.name()),
            Cell::new(r.mode.name()),
            Cell::new(invocation),
            Cell::new(path),
            Cell::new(cleanup),
            Cell::new(r.iterations.to_string()),
            Cell::new(format!("{:.2}", r.total.avg_ms)),
            Cell::new(format!("{:.2}", r.total.min_ms)),
            Cell::new(format!("{:.2}", r.total.max_ms)),
            Cell::new(format!("{:.2}", r.total.std_ms)),
            Cell::new(format!("{:.2}", r.total.p50_ms)),
            Cell::new(format!("{:.2}", r.total.p95_ms)),
            Cell::new(format!("{:.2}", r.total.p99_ms)),
            Cell::new(rss.clone()),
        ]);
    }

    println!("{}", table);
}

#[derive(Serialize, serde::Deserialize)]
struct HostResult {
    total: TimingStats,
    #[serde(default)]
    prepare: Option<TimingStats>,
    #[serde(default)]
    run: Option<TimingStats>,
    rss_mib: f64,
    #[serde(default)]
    python_version: Option<String>,
}

struct HostRuntimePlan {
    python_version: String,
    packages: Vec<String>,
}

fn host_mode_name(mode: Mode) -> Result<&'static str> {
    match mode {
        Mode::HostPythonWarm => Ok("warm-handler"),
        Mode::HostPythonPrepareRun => Ok("prepare-run"),
        Mode::HostPythonProcess => Ok("process"),
        other => Err(anyhow!(
            "mode '{}' is not a host Python variant",
            other.name()
        )),
    }
}

fn host_modes_for_scenario(scenario: Scenario) -> &'static [Mode] {
    match scenario {
        // Pyodide 0.29.4 pins matplotlib 3.8.4 for Python 3.13.2. That exact
        // native CPython package set is not generally wheel-installable, so the
        // default full matrix avoids producing a misleading or host-dependent row.
        Scenario::Matplotlib | Scenario::NumpyMatmul | Scenario::ScipySgemm => &[],
        _ => Mode::host_modes(),
    }
}

fn aardvark_modes_for_scenario(scenario: Scenario) -> &'static [Mode] {
    match scenario {
        Scenario::Tensor => Mode::aardvark_modes(),
        _ => Mode::aardvark_modes(),
    }
}

fn host_runtime_plan(scenario: Scenario) -> Result<HostRuntimePlan> {
    let lock_path = pyodide_lock_path()?;
    let lock_text = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;
    let lock: JsonValue = serde_json::from_str(&lock_text)
        .with_context(|| format!("failed to parse {}", lock_path.display()))?;
    let python_version = lock
        .get("info")
        .and_then(|info| info.get("python"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{} does not declare info.python", lock_path.display()))?;

    let lock_packages = lock
        .get("packages")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| anyhow!("{} does not declare packages", lock_path.display()))?;
    let mut visited = BTreeSet::new();
    let mut packages = Vec::new();
    for name in scenario_packages(scenario) {
        collect_host_package(name, lock_packages, &mut visited, &mut packages, &lock_path)?;
    }

    Ok(HostRuntimePlan {
        python_version,
        packages,
    })
}

fn collect_host_package(
    name: &str,
    lock_packages: &serde_json::Map<String, JsonValue>,
    visited: &mut BTreeSet<String>,
    packages: &mut Vec<String>,
    lock_path: &Path,
) -> Result<()> {
    let package_key = pyodide_package_key(lock_packages, name).ok_or_else(|| {
        anyhow!(
            "{} does not declare Pyodide package '{}'",
            lock_path.display(),
            name
        )
    })?;
    let canonical = canonical_package_name(package_key);
    if !visited.insert(canonical) {
        return Ok(());
    }

    let package = lock_packages
        .get(package_key)
        .expect("package key must come from lock package map");
    let package_name = package
        .get("name")
        .and_then(JsonValue::as_str)
        .unwrap_or(package_key);
    let version = package
        .get("version")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            anyhow!(
                "{} does not declare a version for Pyodide package '{}'",
                lock_path.display(),
                package_name
            )
        })?;
    packages.push(format!("{package_name}=={version}"));

    let depends = package
        .get("depends")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten();
    for dependency in depends {
        let dependency = dependency.as_str().ok_or_else(|| {
            anyhow!(
                "{} declares a non-string dependency for Pyodide package '{}'",
                lock_path.display(),
                package_name
            )
        })?;
        collect_host_package(dependency, lock_packages, visited, packages, lock_path)?;
    }

    Ok(())
}

fn pyodide_package_key<'a>(
    lock_packages: &'a serde_json::Map<String, JsonValue>,
    requested: &str,
) -> Option<&'a str> {
    if let Some((key, _)) = lock_packages.get_key_value(requested) {
        return Some(key.as_str());
    }

    let requested = canonical_package_name(requested);
    lock_packages
        .iter()
        .find(|(key, package)| {
            canonical_package_name(key) == requested
                || package
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .map(canonical_package_name)
                    .is_some_and(|name| name == requested)
        })
        .map(|(key, _)| key.as_str())
}

fn canonical_package_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut last_was_separator = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !last_was_separator {
                normalized.push('-');
                last_was_separator = true;
            }
        } else {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        }
    }
    normalized
}

fn pyodide_lock_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("AARDVARK_PYODIDE_DIST_DIR") {
        return Ok(PathBuf::from(dir).join("pyodide-lock.json"));
    }
    if let Some(dir) = std::env::var_os("PYODIDE_DIST_DIR") {
        return Ok(PathBuf::from(dir).join("pyodide-lock.json"));
    }

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("failed to resolve workspace root"))?;
    let default_path = workspace_root
        .join(".aardvark/pyodide-distributions")
        .join("aardvark-0.1.1-pyodide-v0.29.4-full")
        .join("pyodide-lock.json");
    if default_path.exists() {
        return Ok(default_path);
    }

    Err(anyhow!(
        "host Python benchmarks require a staged Pyodide distribution so Python and package versions can be pinned; set AARDVARK_PYODIDE_DIST_DIR or PYODIDE_DIST_DIR"
    ))
}

trait ScenarioExt {
    fn name(&self) -> &'static str;
}

impl ScenarioExt for Scenario {
    fn name(&self) -> &'static str {
        match self {
            Scenario::Echo => "echo",
            Scenario::Numpy => "numpy",
            Scenario::NumpyMatmul => "numpy-matmul",
            Scenario::Pandas => "pandas",
            Scenario::ScipySgemm => "scipy-sgemm",
            Scenario::Tensor => "tensor",
            Scenario::Matplotlib => "matplotlib",
        }
    }
}

impl LoadProfile {
    fn name(&self) -> &'static str {
        match self {
            LoadProfile::None => "none",
            LoadProfile::Low => "low",
            LoadProfile::Medium => "medium",
            LoadProfile::High => "high",
        }
    }
}

fn json_input_for(scenario: Scenario, profile: LoadProfile) -> Option<JsonInput> {
    match scenario {
        Scenario::Echo => perf::echo_payload(profile)
            .map(|bytes| JsonInput::Utf8Bytes(Bytes::copy_from_slice(bytes))),
        Scenario::Numpy => perf::numpy_size(profile).map(|size| JsonInput::SingleI64Object {
            key: "size".to_owned(),
            value: i64::try_from(size).expect("numpy size should fit i64"),
        }),
        Scenario::NumpyMatmul | Scenario::ScipySgemm => {
            perf::matrix_size(profile).map(|size| JsonInput::SingleI64Object {
                key: "size".to_owned(),
                value: i64::try_from(size).expect("matrix size should fit i64"),
            })
        }
        Scenario::Pandas => perf::pandas_rows(profile).map(|rows| JsonInput::SingleI64Object {
            key: "rows".to_owned(),
            value: i64::try_from(rows).expect("pandas rows should fit i64"),
        }),
        Scenario::Tensor => {
            let bytes = perf::tensor_bytes(profile);
            if bytes.is_empty() {
                None
            } else {
                Some(JsonInput::F32LeBytes(Bytes::from(bytes)))
            }
        }
        Scenario::Matplotlib => {
            perf::matplotlib_points(profile).map(|points| JsonInput::SingleI64Object {
                key: "points".to_owned(),
                value: i64::try_from(points).expect("matplotlib points should fit i64"),
            })
        }
    }
}

fn rawctx_inputs_for(
    scenario: Scenario,
    profile: LoadProfile,
    mode: Mode,
) -> Result<Vec<RawCtxInput>> {
    rawctx_inputs_for_with_options(
        scenario,
        profile,
        false,
        !mode.uses_direct_rawctx_contract(),
    )
}

fn rawctx_inputs_for_call(
    scenario: Scenario,
    profile: LoadProfile,
    template: &[RawCtxInput],
    mode: Mode,
) -> Result<Vec<RawCtxInput>> {
    if mode.uses_owned_rawctx_inputs() {
        rawctx_inputs_for_with_options(scenario, profile, true, !mode.uses_direct_rawctx_contract())
    } else {
        Ok(template.to_vec())
    }
}

fn rawctx_inputs_for_with_options(
    scenario: Scenario,
    profile: LoadProfile,
    force_owned: bool,
    include_metadata: bool,
) -> Result<Vec<RawCtxInput>> {
    match scenario {
        Scenario::Echo => {
            let Some(bytes) = perf::echo_payload(profile) else {
                return Ok(Vec::new());
            };
            let metadata = include_metadata.then(|| RawCtxMetadata::new("binary"));
            let data = if force_owned {
                return Ok(vec![RawCtxInput::from_vec(
                    "payload",
                    bytes.to_vec(),
                    metadata,
                )?]);
            } else {
                Bytes::from_static(bytes)
            };
            Ok(vec![RawCtxInput::new("payload", data, metadata)?])
        }
        Scenario::Numpy => {
            let Some(size) = perf::numpy_size(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&size.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
        Scenario::NumpyMatmul | Scenario::ScipySgemm => {
            let Some(size) = perf::matrix_size(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&size.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
        Scenario::Pandas => {
            let Some(rows) = perf::pandas_rows(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&rows.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
        Scenario::Tensor => {
            let bytes = perf::tensor_bytes(profile);
            if bytes.is_empty() {
                return Ok(Vec::new());
            }
            let length = bytes.len() / std::mem::size_of::<f32>();
            let metadata = if include_metadata {
                Some(
                    RawCtxMetadata::new("binary")
                        .with_shape(vec![length])
                        .with_extra(json!({"format": "f32_le"}))?,
                )
            } else {
                None
            };
            Ok(vec![RawCtxInput::from_vec("tensor", bytes, metadata)?])
        }
        Scenario::Matplotlib => {
            let Some(points) = perf::matplotlib_points(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&points.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
    }
}

impl std::str::FromStr for Scenario {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "echo" => Ok(Scenario::Echo),
            "numpy" => Ok(Scenario::Numpy),
            "numpy-matmul" | "numpymatmul" => Ok(Scenario::NumpyMatmul),
            "pandas" => Ok(Scenario::Pandas),
            "scipy-sgemm" | "scipysgemm" => Ok(Scenario::ScipySgemm),
            "tensor" => Ok(Scenario::Tensor),
            "matplotlib" => Ok(Scenario::Matplotlib),
            other => Err(format!("unknown scenario '{other}'")),
        }
    }
}

impl std::str::FromStr for LoadProfile {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Ok(LoadProfile::None),
            "low" => Ok(LoadProfile::Low),
            "medium" => Ok(LoadProfile::Medium),
            "high" => Ok(LoadProfile::High),
            other => Err(format!("unknown profile '{other}'")),
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "aardvark-json-cold" => Ok(Mode::AardvarkJsonCold),
            "aardvark-json-warm" => Ok(Mode::AardvarkJsonWarm),
            "aardvark-json-reset-in-place" => Ok(Mode::AardvarkJsonResetInPlace),
            "aardvark-json-persistent" => Ok(Mode::AardvarkJsonPersistent),
            "aardvark-json-persistent-warm-call" => Ok(Mode::AardvarkJsonPersistentWarmCall),
            "aardvark-json-persistent-no-stdio" => Ok(Mode::AardvarkJsonPersistentNoStdio),
            "aardvark-json-persistent-warm-call-no-stdio" => {
                Ok(Mode::AardvarkJsonPersistentWarmCallNoStdio)
            }
            "aardvark-json-registry-persistent-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryPersistentNoStdio)
            }
            "aardvark-json-registry-prepare-each-call-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryPrepareEachCallNoStdio)
            }
            "aardvark-json-registry-cached-handler-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryCachedHandlerNoStdio)
            }
            "aardvark-json-registry-retained-handler-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryRetainedHandlerNoStdio)
            }
            "aardvark-json-registry-retained-first-live-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryRetainedFirstLiveNoStdio)
            }
            "aardvark-json-registry-retained-warm-all-first-live-no-stdio" => {
                Ok(Mode::AardvarkJsonRegistryRetainedWarmAllFirstLiveNoStdio)
            }
            "aardvark-json-warmed-host-pooled-warm-all-first-live-no-stdio" => {
                Ok(Mode::AardvarkJsonWarmedHostPooledWarmAllFirstLiveNoStdio)
            }
            "aardvark-json-warmed-host-registry-pooled-warm-all-first-live-no-stdio" => Ok(
                Mode::AardvarkJsonWarmedHostRegistryPooledWarmAllFirstLiveNoStdio,
            ),
            "aardvark-json-persistent-full" => Ok(Mode::AardvarkJsonPersistentFull),
            "aardvark-json-persistent-shared" => Ok(Mode::AardvarkJsonPersistentShared),
            "aardvark-json-persistent-none" => Ok(Mode::AardvarkJsonPersistentNone),
            "aardvark-rawctx-cold" => Ok(Mode::AardvarkRawCtxCold),
            "aardvark-rawctx-warm" => Ok(Mode::AardvarkRawCtxWarm),
            "aardvark-rawctx-reset-in-place" => Ok(Mode::AardvarkRawCtxResetInPlace),
            "aardvark-rawctx-persistent" => Ok(Mode::AardvarkRawCtxPersistent),
            "aardvark-rawctx-persistent-no-stdio" => Ok(Mode::AardvarkRawCtxPersistentNoStdio),
            "aardvark-rawctx-direct-persistent" => Ok(Mode::AardvarkRawCtxDirectPersistent),
            "aardvark-rawctx-direct-owned-persistent" => {
                Ok(Mode::AardvarkRawCtxDirectOwnedPersistent)
            }
            "aardvark-rawctx-direct-persistent-warm-call" => {
                Ok(Mode::AardvarkRawCtxDirectPersistentWarmCall)
            }
            "aardvark-rawctx-direct-owned-persistent-warm-call" => {
                Ok(Mode::AardvarkRawCtxDirectOwnedPersistentWarmCall)
            }
            "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio" => {
                Ok(Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdio)
            }
            "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only" => {
                Ok(Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnly)
            }
            "aardvark-rawctx-direct-owned-persistent-warm-call-no-stdio-shared-buffer-only-no-output-metadata" => {
                Ok(Mode::AardvarkRawCtxDirectOwnedPersistentWarmCallNoStdioSharedBufferOnlyNoOutputMetadata)
            }
            "aardvark-rawctx-registry-retained-direct-owned-warm-call-no-stdio" => {
                Ok(Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmCallNoStdio)
            }
            "aardvark-rawctx-registry-retained-direct-owned-first-live-no-stdio" => {
                Ok(Mode::AardvarkRawCtxRegistryRetainedDirectOwnedFirstLiveNoStdio)
            }
            "aardvark-rawctx-registry-retained-direct-owned-warm-all-first-live-no-stdio" => {
                Ok(Mode::AardvarkRawCtxRegistryRetainedDirectOwnedWarmAllFirstLiveNoStdio)
            }
            "aardvark-rawctx-persistent-warm-call" => Ok(Mode::AardvarkRawCtxPersistentWarmCall),
            "aardvark-rawctx-persistent-full" => Ok(Mode::AardvarkRawCtxPersistentFull),
            "aardvark-rawctx-persistent-shared" => Ok(Mode::AardvarkRawCtxPersistentShared),
            "aardvark-rawctx-persistent-none" => Ok(Mode::AardvarkRawCtxPersistentNone),
            "host-python" | "host-python-warm" | "host" | "python" => Ok(Mode::HostPythonWarm),
            "host-python-prepare-run" => Ok(Mode::HostPythonPrepareRun),
            "host-python-process" => Ok(Mode::HostPythonProcess),
            other => Err(format!("unknown mode '{other}'")),
        }
    }
}

#[cfg(unix)]
#[cfg(target_os = "macos")]
fn current_rss_mib() -> Option<f64> {
    unsafe {
        unsafe extern "C" {
            #[link_name = "mach_task_self_"]
            static MACH_TASK_SELF: libc::mach_port_t;
        }
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        let result = libc::task_info(
            MACH_TASK_SELF,
            libc::MACH_TASK_BASIC_INFO,
            (&mut info as *mut _) as *mut libc::integer_t,
            &mut count,
        );
        if result == libc::KERN_SUCCESS {
            Some(info.resident_size as f64 / (1024.0 * 1024.0))
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn current_rss_mib() -> Option<f64> {
    let contents = fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: f64 = contents.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    Some(resident_pages * page_size / (1024.0 * 1024.0))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_rss_mib() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_package_closure_includes_transitive_lock_dependencies() {
        let mut lock_packages = serde_json::Map::new();
        lock_packages.insert(
            "root-pkg".to_owned(),
            json!({
                "name": "root-pkg",
                "version": "1.0.0",
                "depends": ["mid_pkg"],
            }),
        );
        lock_packages.insert(
            "mid-pkg".to_owned(),
            json!({
                "name": "mid-pkg",
                "version": "2.0.0",
                "depends": ["leaf.pkg"],
            }),
        );
        lock_packages.insert(
            "leaf-pkg".to_owned(),
            json!({
                "name": "leaf-pkg",
                "version": "3.0.0",
                "depends": [],
            }),
        );

        let mut visited = BTreeSet::new();
        let mut packages = Vec::new();
        collect_host_package(
            "root_pkg",
            &lock_packages,
            &mut visited,
            &mut packages,
            Path::new("pyodide-lock.json"),
        )
        .unwrap();

        assert_eq!(
            packages,
            vec!["root-pkg==1.0.0", "mid-pkg==2.0.0", "leaf-pkg==3.0.0"]
        );
    }

    #[test]
    fn full_matrix_omits_default_matplotlib_host_rows() {
        assert!(host_modes_for_scenario(Scenario::Matplotlib).is_empty());
        assert_eq!(host_modes_for_scenario(Scenario::Pandas).len(), 3);
        assert_eq!(
            aardvark_modes_for_scenario(Scenario::Tensor),
            Mode::aardvark_modes()
        );
    }
}
