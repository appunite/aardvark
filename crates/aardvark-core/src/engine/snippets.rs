use v8::{self, Local, PinScope, Value};

pub(super) fn js_snippet_wrapper(code: &str) -> String {
    let mut source = String::from(
        r#"
(async (pyodide) => {
  const sleep = globalThis.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  const assert = (condition, message = "assertion failed") => {
    const passed = typeof condition === "function" ? condition() : condition;
    if (!passed) {
      throw new Error(message);
    }
  };
  assert.equal = (actual, expected, message = "assertion failed") => {
    if (actual !== expected) {
      throw new Error(message);
    }
  };
  const errorMatches = (error, expectedName, expectedMessage) => {
    if (expectedName) {
      const actualName = String(error?.name ?? error?.constructor?.name ?? "");
      if (!actualName.includes(expectedName)) {
        return false;
      }
    }
    if (expectedMessage) {
      const actualMessage = String(error?.message ?? error ?? "");
      if (!actualMessage.includes(expectedMessage)) {
        return false;
      }
    }
    return true;
  };
  const assertThrows = (fn, expectedName = "", expectedMessage = "") => {
    try {
      fn();
    } catch (error) {
      if (errorMatches(error, expectedName, expectedMessage)) {
        return error;
      }
      throw new Error(`unexpected exception: ${error?.name ?? ""}: ${error?.message ?? error}`);
    }
    throw new Error("expected exception was not thrown");
  };
  const assertThrowsAsync = async (fn, expectedName = "", expectedMessage = "") => {
    try {
      await fn();
    } catch (error) {
      if (errorMatches(error, expectedName, expectedMessage)) {
        return error;
      }
      throw new Error(`unexpected exception: ${error?.name ?? ""}: ${error?.message ?? error}`);
    }
    throw new Error("expected exception was not thrown");
  };
"#,
    );
    source.push_str(code);
    source.push_str(
        r#"
})(globalThis.__aardvarkCompatPyodide);
"#,
    );
    source
}

pub(super) fn javascript_value_error_details<'a>(
    scope: &mut PinScope<'a, '_>,
    value: Local<'a, Value>,
    default_typ: &str,
    default_message: &str,
) -> (String, String, Option<String>) {
    let mut typ = default_typ.to_string();
    let mut message = value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| default_message.to_string());
    let mut stack: Option<String> = None;

    if let Some(object) = value.to_object(scope) {
        if let Some(name_key) = v8::String::new(scope, "name") {
            if let Some(name_value) = object.get(scope, name_key.into()) {
                if let Some(name_str) = name_value.to_string(scope) {
                    typ = name_str.to_rust_string_lossy(scope);
                }
            }
        }
        if let Some(message_key) = v8::String::new(scope, "message") {
            if let Some(message_value) = object.get(scope, message_key.into()) {
                if let Some(msg_str) = message_value.to_string(scope) {
                    message = msg_str.to_rust_string_lossy(scope);
                }
            }
        }
        if let Some(stack_key) = v8::String::new(scope, "stack") {
            if let Some(stack_value) = object.get(scope, stack_key.into()) {
                if let Some(stack_str) = stack_value.to_string(scope) {
                    stack = Some(stack_str.to_rust_string_lossy(scope));
                }
            }
        }
    }

    (typ, message, stack)
}
