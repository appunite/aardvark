use super::*;

pub(super) fn scenario_source(scenario: Scenario) -> String {
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

pub(super) fn scenario_manifest(
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
        let Some(manifest_obj) = manifest.as_object_mut() else {
            return manifest.to_string();
        };
        let runtime = manifest_obj
            .entry("runtime")
            .or_insert_with(|| json!({"language": "python"}));
        if !runtime.is_object() {
            *runtime = json!({"language": "python"});
        }
        let Some(runtime_obj) = runtime.as_object_mut() else {
            return manifest.to_string();
        };
        runtime_obj
            .entry("language")
            .or_insert_with(|| json!("python"));
        let pyodide = runtime_obj.entry("pyodide").or_insert_with(|| json!({}));
        if !pyodide.is_object() {
            *pyodide = json!({});
        }
        if let Some(pyodide_obj) = pyodide.as_object_mut() {
            pyodide_obj.insert("preloadImports".to_owned(), json!(manifest_preload_imports));
        }
    }
    if matches!(invocation, InvocationKind::RawCtx) {
        manifest["resources"] = json!({
            "hostCapabilities": ["rawctx_buffers"],
        });
    }
    manifest.to_string()
}

pub(super) fn descriptor_for(
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

pub(super) fn scenario_packages(scenario: Scenario) -> &'static [&'static str] {
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

pub(super) fn build_bundle_bytes(source: &str, manifest: &[u8]) -> Result<Vec<u8>> {
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

impl Scenario {
    pub(super) fn name(&self) -> &'static str {
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
    pub(super) fn name(&self) -> &'static str {
        match self {
            LoadProfile::None => "none",
            LoadProfile::Low => "low",
            LoadProfile::Medium => "medium",
            LoadProfile::High => "high",
        }
    }
}

pub(super) fn json_input_for(scenario: Scenario, profile: LoadProfile) -> Option<JsonInput> {
    match scenario {
        Scenario::Echo => perf::echo_payload(profile)
            .map(|bytes| JsonInput::Utf8Bytes(Bytes::copy_from_slice(bytes))),
        Scenario::Numpy => perf::numpy_size(profile).map(|size| JsonInput::SingleI64Object {
            key: "size".to_owned(),
            value: i64_from_u64(size),
        }),
        Scenario::NumpyMatmul | Scenario::ScipySgemm => {
            perf::matrix_size(profile).map(|size| JsonInput::SingleI64Object {
                key: "size".to_owned(),
                value: i64_from_u64(size),
            })
        }
        Scenario::Pandas => perf::pandas_rows(profile).map(|rows| JsonInput::SingleI64Object {
            key: "rows".to_owned(),
            value: i64_from_u64(rows),
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
                value: i64_from_u64(points),
            })
        }
    }
}

fn i64_from_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn rawctx_inputs_for(
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

pub(super) fn rawctx_inputs_for_call(
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
