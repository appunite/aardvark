use serde_json::Value as JsonValue;

use crate::error::{PyRunnerError, Result};

pub(in crate::strategy::rawctx) fn validate_decoder_options(
    decoder: Option<&str>,
    options: Option<&JsonValue>,
    context: &str,
) -> Result<()> {
    let Some(raw_decoder) = decoder else {
        if let Some(value) = options {
            if !value.is_null() && !value.is_object() {
                return Err(PyRunnerError::Execution(format!(
                    "{context} decoder options must be a JSON object"
                )));
            }
        }
        return Ok(());
    };

    let trimmed = raw_decoder.trim();
    if trimmed.is_empty() {
        return Err(PyRunnerError::Execution(format!(
            "{context} decoder cannot be empty"
        )));
    }

    let decoder = trimmed.to_ascii_lowercase();
    let Some(options_value) = options else {
        return Ok(());
    };

    let object = options_value.as_object().ok_or_else(|| {
        PyRunnerError::Execution(format!("{context} decoder options must be a JSON object"))
    })?;

    if object.is_empty() {
        return Ok(());
    }

    match decoder.as_str() {
        "utf8" | "string" | "json" => {
            if let Some(value) = object.get("encoding") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'encoding' must be a string"
                    )));
                };
                if string.trim().is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'encoding' cannot be empty"
                    )));
                }
            }
            if let Some(value) = object.get("errors") {
                if value.as_str().is_none() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'errors' must be a string"
                    )));
                }
            }
        }
        "float32" | "f32" | "float64" | "f64" | "int32" | "i32" | "uint32" | "u32" => {
            if let Some(value) = object.get("struct_format") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' must be a string"
                    )));
                };
                let trimmed = string.trim();
                if trimmed.is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' cannot be empty"
                    )));
                }

                let expected = match decoder.as_str() {
                    "float32" | "f32" => 'f',
                    "float64" | "f64" => 'd',
                    "int32" | "i32" => 'i',
                    "uint32" | "u32" => 'I',
                    other => {
                        debug_assert!(matches!(
                            other,
                            "float32"
                                | "f32"
                                | "float64"
                                | "f64"
                                | "int32"
                                | "i32"
                                | "uint32"
                                | "u32"
                        ));
                        'f'
                    }
                };
                let Some(type_char) = trimmed.chars().last() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' cannot be empty"
                    )));
                };
                if !type_char.eq_ignore_ascii_case(&expected) {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' must end with '{}'",
                        expected
                    )));
                }

                if trimmed.len() > type_char.len_utf8() {
                    let prefix = &trimmed[..trimmed.len() - type_char.len_utf8()];
                    if !prefix.is_empty() {
                        let mut chars = prefix.chars();
                        let Some(first) = chars.next() else {
                            return Err(PyRunnerError::Execution(format!(
                                "{context} decoder option 'struct_format' prefix cannot be empty"
                            )));
                        };
                        let allowed = ['<', '>', '!', '=', '@'];
                        if allowed.contains(&first) {
                            if chars.any(|c| !c.is_ascii_digit()) {
                                return Err(PyRunnerError::Execution(format!(
                                    "{context} decoder option 'struct_format' prefix must contain only digits after the byteorder flag"
                                )));
                            }
                        } else if !first.is_ascii_digit() || chars.any(|c| !c.is_ascii_digit()) {
                            return Err(PyRunnerError::Execution(format!(
                                "{context} decoder option 'struct_format' prefix must be digits or a byteorder flag"
                            )));
                        }
                    }
                }
            }
        }
        "int64" | "i64" => {
            if let Some(value) = object.get("byteorder") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be a string"
                    )));
                };
                let lowered = string.trim().to_ascii_lowercase();
                if lowered != "little" && lowered != "big" {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be 'little' or 'big'"
                    )));
                }
            }
            if let Some(value) = object.get("signed") {
                if !value.is_boolean() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'signed' must be a boolean"
                    )));
                }
            }
        }
        "bool" | "boolean" => {
            if let Some(value) = object.get("byteorder") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be a string"
                    )));
                };
                let lowered = string.trim().to_ascii_lowercase();
                if lowered != "little" && lowered != "big" {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be 'little' or 'big'"
                    )));
                }
            }
        }
        "base64" | "b64" => {
            if let Some(value) = object.get("altchars") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'altchars' must be a string"
                    )));
                };
                if string.chars().count() != 2 {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'altchars' must contain exactly two characters"
                    )));
                }
            }
            for key in ["validate", "as_memoryview", "as_bytearray"] {
                if let Some(value) = object.get(key) {
                    if !value.is_boolean() {
                        return Err(PyRunnerError::Execution(format!(
                            "{context} decoder option '{key}' must be a boolean"
                        )));
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}
