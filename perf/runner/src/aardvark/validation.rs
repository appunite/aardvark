use super::*;

pub(super) fn validate_aardvark_outcome(
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
        Scenario::Tensor => {
            return Err(anyhow!(
                "tensor scenario does not produce a JSON validation payload"
            ));
        }
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
    let count = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| anyhow!("{label} count prefix was malformed"))?,
    ) as usize;
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
            let total = f64::from_le_bytes(
                bytes[0..8]
                    .try_into()
                    .map_err(|_| anyhow!("numpy rawctx payload was malformed"))?,
            );
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
            let total = f64::from_le_bytes(
                bytes[0..8]
                    .try_into()
                    .map_err(|_| anyhow!("matrix rawctx payload was malformed"))?,
            );
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
            let byte_count = u64::from_le_bytes(
                bytes[0..8]
                    .try_into()
                    .map_err(|_| anyhow!("matplotlib rawctx payload was malformed"))?,
            );
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
