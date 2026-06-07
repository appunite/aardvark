use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{self, Value as JsonValue};

use crate::error::{PyRunnerError, Result};
use crate::invocation::{FieldDescriptor, InvocationDescriptor};
use crate::session::PySession;

use super::builders::{validate_decoder_options, RawCtxTableSpec};

pub(super) fn cached_rawctx_spec(session: &PySession) -> Result<Option<Arc<String>>> {
    session.rawctx_spec_json(|| rawctx_spec_json_for_descriptor(session.descriptor()))
}

pub(crate) fn rawctx_spec_json_for_descriptor(
    descriptor: &InvocationDescriptor,
) -> Result<Option<Arc<String>>> {
    let spec = build_rawctx_auto_spec(descriptor)?;
    match spec {
        Some(spec) => {
            let json = serde_json::to_string(&spec).map_err(|err| {
                PyRunnerError::Execution(format!(
                    "failed to serialise rawctx auto-wrapper spec: {err}"
                ))
            })?;
            Ok(Some(Arc::new(json)))
        }
        None => Ok(None),
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RawCtxAutoSpec {
    entrypoint: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    inputs: Vec<RawCtxInputBindingSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    outputs: Vec<RawCtxOutputSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RawCtxInputBindingSpec {
    field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python_loader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    optional: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    table: Option<RawCtxTableSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RawCtxOutputSpec {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python_transform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_behavior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_none: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
}

fn build_rawctx_auto_spec(descriptor: &InvocationDescriptor) -> Result<Option<RawCtxAutoSpec>> {
    let entrypoint = descriptor.entrypoint();
    if !entrypoint.contains(':') {
        return Ok(None);
    }

    let mut inputs = Vec::new();
    for field in &descriptor.inputs {
        if let Some(binding) = parse_input_binding(field)? {
            inputs.push(binding);
        }
    }

    let mut outputs = Vec::new();
    for field in &descriptor.outputs {
        if let Some(output) = parse_output_binding(field)? {
            outputs.push(output);
        }
    }

    if inputs.is_empty() && outputs.is_empty() {
        return Ok(None);
    }

    Ok(Some(RawCtxAutoSpec {
        entrypoint: entrypoint.to_owned(),
        inputs,
        outputs,
    }))
}

fn parse_table_spec(value: &JsonValue) -> Result<RawCtxTableSpec> {
    let spec: RawCtxTableSpec = serde_json::from_value(value.clone())
        .map_err(|err| PyRunnerError::Execution(format!("invalid rawctx table spec: {err}")))?;
    spec.validate()?;
    Ok(spec)
}

fn parse_input_binding(field: &FieldDescriptor) -> Result<Option<RawCtxInputBindingSpec>> {
    let metadata = match &field.metadata {
        Some(value) => value,
        None => return Ok(None),
    };
    let rawctx = match extract_rawctx_metadata(metadata) {
        Some(value) => value,
        None => return Ok(None),
    };

    if matches!(
        rawctx
            .get("mode")
            .and_then(|value| value.as_str())
            .map(|mode| mode.eq_ignore_ascii_case("manual")
                || mode.eq_ignore_ascii_case("skip")
                || mode.eq_ignore_ascii_case("disabled")),
        Some(true)
    ) {
        return Ok(None);
    }

    let enabled = rawctx
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Ok(None);
    }

    let binding = if let Some(value) = rawctx.get("binding") {
        value.as_object().ok_or_else(|| {
            PyRunnerError::Execution("rawctx binding metadata must be a JSON object".into())
        })?
    } else {
        rawctx
    };

    let arg = match binding.get("arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx binding arg must be a string when provided".into())
            })?;
            Some(string.to_owned())
        }
        None => None,
    };
    let arg = arg.or_else(|| Some(field.name.clone()));

    let mode = binding
        .get("mode")
        .map(|value| {
            let mode = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx binding mode must be a string".into())
            })?;
            let lowered = mode.to_ascii_lowercase();
            if lowered != "keyword" && lowered != "positional" {
                return Err(PyRunnerError::Execution(format!(
                    "unsupported rawctx binding mode '{mode}' (expected 'keyword' or 'positional')"
                )));
            }
            Ok(lowered)
        })
        .transpose()?;

    if mode.as_deref() == Some("positional") && arg.is_none() {
        return Err(PyRunnerError::Execution(
            "rawctx binding cannot be positional without an argument name".into(),
        ));
    }

    let decoder = binding
        .get("decoder")
        .map(|value| {
            value
                .as_str()
                .map(|s| s.to_owned())
                .ok_or_else(|| PyRunnerError::Execution("rawctx decoder must be a string".into()))
        })
        .transpose()?;

    let options = match binding.get("options") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            if value.is_object() {
                Some(value.clone())
            } else {
                return Err(PyRunnerError::Execution(
                    "rawctx binding options must be a JSON object".into(),
                ));
            }
        }
        None => None,
    };

    validate_decoder_options(
        decoder.as_deref(),
        options.as_ref(),
        &format!("rawctx binding '{}'", field.name),
    )?;

    let metadata_arg = match binding.get("metadata_arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution(
                    "rawctx metadata_arg must be a string when provided".into(),
                )
            })?;
            Some(string.to_owned())
        }
        None => None,
    };

    let raw_arg = match binding.get("raw_arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx raw_arg must be a string when provided".into())
            })?;
            Some(string.to_owned())
        }
        None => None,
    };

    let python_loader = binding
        .get("python_loader")
        .map(|value| {
            value.as_str().map(|s| s.to_owned()).ok_or_else(|| {
                PyRunnerError::Execution("rawctx python_loader must be a string".into())
            })
        })
        .transpose()?;

    let default = binding.get("default").cloned();

    let optional = binding
        .get("optional")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| PyRunnerError::Execution("rawctx optional must be a boolean".into()))
        })
        .transpose()?;

    let mut table = match binding.get("table") {
        Some(value) => Some(parse_table_spec(value)?),
        None => None,
    };

    if table.is_none() {
        if let Some(manifest_value) = binding
            .get("table_manifest")
            .or_else(|| rawctx.get("table_manifest"))
        {
            table = Some(RawCtxTableSpec::from_manifest(manifest_value)?);
        }
    }

    if arg.is_none()
        && metadata_arg.is_none()
        && raw_arg.is_none()
        && python_loader.is_none()
        && default.is_none()
        && table.is_none()
    {
        return Err(PyRunnerError::Execution(
            "rawctx binding must project at least one argument or provide a custom loader/default"
                .into(),
        ));
    }

    Ok(Some(RawCtxInputBindingSpec {
        field: field.name.clone(),
        arg,
        mode,
        decoder,
        options,
        metadata_arg,
        raw_arg,
        python_loader,
        default,
        optional,
        table,
    }))
}

fn parse_output_binding(field: &FieldDescriptor) -> Result<Option<RawCtxOutputSpec>> {
    let metadata = match &field.metadata {
        Some(value) => value,
        None => return Ok(None),
    };
    let rawctx = match extract_rawctx_metadata(metadata) {
        Some(value) => value,
        None => return Ok(None),
    };

    if matches!(
        rawctx
            .get("mode")
            .and_then(|value| value.as_str())
            .map(|mode| mode.eq_ignore_ascii_case("manual")
                || mode.eq_ignore_ascii_case("skip")
                || mode.eq_ignore_ascii_case("disabled")),
        Some(true)
    ) {
        return Ok(None);
    }

    let publish_obj = if let Some(value) = rawctx.get("publish") {
        value.as_object().ok_or_else(|| {
            PyRunnerError::Execution("rawctx publish metadata must be a JSON object".into())
        })?
    } else {
        rawctx
    };

    let enabled = publish_obj
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Ok(None);
    }

    let mode = publish_obj
        .get("mode")
        .and_then(|value| value.as_str())
        .map(|mode| mode.to_ascii_lowercase());

    let mode_value = mode.as_deref().unwrap_or("publish-buffer");
    if mode_value != "publish-buffer" {
        return Err(PyRunnerError::Execution(format!(
            "unsupported rawctx output mode '{mode_value}'"
        )));
    }

    let id = publish_obj
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            PyRunnerError::Execution("rawctx output publish requires an 'id' field".into())
        })?
        .to_owned();

    let transform = publish_obj
        .get("transform")
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref transform_value) = transform {
        let supported = ["memoryview", "bytes", "utf8", "identity"];
        if !supported
            .iter()
            .any(|item| item.eq_ignore_ascii_case(transform_value))
        {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx output transform '{transform_value}'"
            )));
        }
    }

    let python_transform = publish_obj
        .get("python_transform")
        .and_then(|value| value.as_str())
        .map(|value| value.to_owned());

    let return_behavior = publish_obj
        .get("return")
        .or_else(|| publish_obj.get("return_behavior"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref behaviour) = return_behavior {
        let supported = ["none", "original", "buffer"];
        if !supported
            .iter()
            .any(|item| item.eq_ignore_ascii_case(behaviour))
        {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx return behaviour '{behaviour}'"
            )));
        }
    }

    let when_none = publish_obj
        .get("when_none")
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref mode) = when_none {
        let supported = ["skip", "error", "publish-empty", "propagate"];
        if !supported.iter().any(|item| item.eq_ignore_ascii_case(mode)) {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx when_none behaviour '{mode}'"
            )));
        }
    }

    let encoding = publish_obj
        .get("encoding")
        .and_then(|value| value.as_str())
        .map(|value| value.to_owned());

    Ok(Some(RawCtxOutputSpec {
        id,
        mode: mode.filter(|m| m != "publish-buffer"),
        transform,
        metadata: publish_obj.get("metadata").cloned(),
        python_transform,
        return_behavior,
        when_none,
        encoding,
    }))
}

fn extract_rawctx_metadata(value: &JsonValue) -> Option<&serde_json::Map<String, JsonValue>> {
    let object = value.as_object()?;
    if let Some(aardvark) = object.get("aardvark").and_then(|value| value.as_object()) {
        if let Some(rawctx) = aardvark.get("rawctx").and_then(|value| value.as_object()) {
            return Some(rawctx);
        }
    }
    object.get("rawctx").and_then(|value| value.as_object())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::rawctx_spec_json_for_descriptor;
    use crate::invocation::{FieldDescriptor, InvocationDescriptor};
    use crate::strategy::RawCtxPublishBuilder;

    #[test]
    fn rawctx_auto_spec_uses_outputs_list_only() {
        let mut descriptor = InvocationDescriptor::new("main:handler");
        descriptor.outputs.push(FieldDescriptor {
            name: "result".to_owned(),
            type_tag: None,
            metadata: Some(
                RawCtxPublishBuilder::new("result")
                    .transform("memoryview")
                    .build(),
            ),
        });

        let spec = rawctx_spec_json_for_descriptor(&descriptor)
            .expect("rawctx spec should serialize")
            .expect("descriptor should produce rawctx spec");
        let value: serde_json::Value =
            serde_json::from_str(&spec).expect("rawctx spec should be valid JSON");

        let mut keys = value
            .as_object()
            .expect("rawctx spec should be an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["entrypoint", "outputs"]);
        assert_eq!(
            value["outputs"],
            json!([{"id": "result", "transform": "memoryview"}])
        );
    }
}
