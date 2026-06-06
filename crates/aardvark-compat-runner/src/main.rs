use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use aardvark_core::{
    BundleArtifact, ExecutionOutput, InlinePythonOptions, JsonInvocationStrategy,
    ManifestFilesystemMode, ManifestFilesystemResources, ManifestResources, OutcomeStatus,
    PyRuntime, PyRuntimeConfig, PySession, ResultPayload,
};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const JS_TEST_HELPERS: &str = r#"
(() => {
  if (typeof globalThis.sleep !== "function") {
    globalThis.sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  }
  globalThis.assert = (condition, message = "assertion failed") => {
    const passed = typeof condition === "function" ? condition() : condition;
    if (!passed) {
      throw new Error(message);
    }
  };
  globalThis.assert.equal = (actual, expected, message = "assertion failed") => {
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
  globalThis.assertThrows = (fn, expectedName = "", expectedMessage = "") => {
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
  globalThis.assertThrowsAsync = async (fn, expectedName = "", expectedMessage = "") => {
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
})();
"#;

const COMPAT_MODULE: &str = r#"
import ast
import asyncio
import builtins
import __main__
import json
import traceback

_GLOBALS = __main__.__dict__
_GLOBALS.setdefault("__name__", "__main__")
_GLOBALS.setdefault("__builtins__", builtins)

def _jsonable(value):
    if value is None or isinstance(value, (str, int, float, bool)):
        return {"kind": "json", "value": value}
    if isinstance(value, (list, tuple)):
        return {"kind": "json", "value": [_jsonable(item)["value"] for item in value]}
    if isinstance(value, dict):
        return {
            "kind": "json",
            "value": {str(key): _jsonable(item)["value"] for key, item in value.items()},
        }
    return {
        "kind": "repr",
        "type": type(value).__name__,
        "value": repr(value),
    }

def _compile_exec_and_last_expr(code, filename="<aardvark-compat>"):
    module = ast.parse(code, filename=filename, mode="exec")
    body = list(module.body)
    last_expr = body.pop() if body and isinstance(body[-1], ast.Expr) else None
    if body:
        exec_module = ast.Module(body=body, type_ignores=[])
        ast.fix_missing_locations(exec_module)
        exec(compile(exec_module, filename, "exec"), _GLOBALS)
    if last_expr is None:
        return None
    expr = ast.Expression(last_expr.value)
    ast.fix_missing_locations(expr)
    return eval(compile(expr, filename, "eval"), _GLOBALS)

async def _run_async_code(code, filename="<aardvark-compat-async>"):
    module = ast.parse(code, filename=filename, mode="exec")
    body = list(module.body)
    if body and isinstance(body[-1], ast.Expr):
        body[-1] = ast.Return(value=body[-1].value)
    elif not body or not isinstance(body[-1], ast.Return):
        body.append(ast.Return(value=ast.Constant(value=None)))
    func = ast.AsyncFunctionDef(
        name="__aardvark_compat_async",
        args=ast.arguments(
            posonlyargs=[],
            args=[],
            kwonlyargs=[],
            kw_defaults=[],
            defaults=[],
        ),
        body=body,
        decorator_list=[],
    )
    wrapper = ast.Module(body=[func], type_ignores=[])
    ast.fix_missing_locations(wrapper)
    exec(compile(wrapper, filename, "exec"), _GLOBALS)
    return await _GLOBALS["__aardvark_compat_async"]()

def _run_js(code):
    from pyodide.code import run_js

    return run_js(code)

def handler():
    payload = getattr(builtins, "__aardvark_input", None) or {}
    op = payload.get("op", "runPython")
    code = payload.get("code", "")
    try:
        if op == "runPython":
            value = _compile_exec_and_last_expr(code)
        elif op == "runPythonAsync":
            value = asyncio.run(_run_async_code(code))
        elif op == "runJs":
            value = _run_js(code)
        else:
            raise ValueError(f"unsupported compat op: {op}")
        return {
            "ok": True,
            "result": _jsonable(value),
        }
    except Exception as exc:
        return {
            "ok": False,
            "exceptionType": type(exc).__name__,
            "exceptionValue": repr(exc),
            "traceback": traceback.format_exc(),
        }
"#;

#[derive(Debug)]
struct Args {
    dist_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Request {
    Ping,
    LoadPackage { packages: Vec<String> },
    RunPython { code: String },
    RunPythonAsync { code: String },
    RunJs { code: String },
    Logs,
    Reset,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct CompatRunner {
    dist_dir: Option<PathBuf>,
    inner: Option<CompatRunnerInner>,
}

struct CompatRunnerInner {
    runtime: PyRuntime,
    session: PySession,
    logs: String,
}

impl CompatRunner {
    fn new(dist_dir: Option<PathBuf>) -> Result<Self> {
        let inner = Self::build_inner(dist_dir.clone())?;
        Ok(Self {
            dist_dir,
            inner: Some(inner),
        })
    }

    fn build_inner(dist_dir: Option<PathBuf>) -> Result<CompatRunnerInner> {
        let mut config = PyRuntimeConfig::default();
        if let Some(dir) = dist_dir.clone() {
            config = config.with_pyodide_dist_dir(dir);
        }

        let artifact = BundleArtifact::from_inline_python(
            COMPAT_MODULE,
            InlinePythonOptions {
                entrypoint: Some("aardvark_compat:handler".to_string()),
                resources: Some(ManifestResources {
                    filesystem: Some(ManifestFilesystemResources {
                        mode: Some(ManifestFilesystemMode::ReadWrite),
                        quota_bytes: Some(64 * 1024 * 1024),
                    }),
                    ..ManifestResources::default()
                }),
                ..InlinePythonOptions::default()
            },
        )
        .context("build compatibility inline bundle")?;

        let bundle = artifact.bundle();
        let mut runtime = PyRuntime::new_for_bundle(config, &bundle).context("create runtime")?;
        let descriptor = artifact.default_descriptor();
        let (session, _) = runtime
            .prepare_session_with_manifest_and_descriptor(bundle, descriptor)
            .context("prepare compatibility session")?;
        runtime
            .js_runtime()
            .execute_script("aardvark-compat-test-helpers.js", JS_TEST_HELPERS)
            .context("install JavaScript test helpers")?;

        Ok(CompatRunnerInner {
            runtime,
            session,
            logs: String::new(),
        })
    }

    fn reset(&mut self) -> Result<()> {
        drop(self.inner.take());
        self.inner = Some(Self::build_inner(self.dist_dir.clone())?);
        Ok(())
    }

    fn inner(&self) -> Result<&CompatRunnerInner> {
        self.inner
            .as_ref()
            .ok_or_else(|| anyhow!("compat runner runtime is unavailable"))
    }

    fn inner_mut(&mut self) -> Result<&mut CompatRunnerInner> {
        self.inner
            .as_mut()
            .ok_or_else(|| anyhow!("compat runner runtime is unavailable"))
    }

    fn handle(&mut self, request: Request) -> Result<Response> {
        match request {
            Request::Ping => Ok(Response {
                ok: true,
                result: Some(json!({"runner": "aardvark-compat-runner"})),
                diagnostics: None,
                error: None,
            }),
            Request::LoadPackage { packages } => {
                if packages.is_empty() {
                    return Ok(Response {
                        ok: true,
                        result: Some(json!({"loaded": []})),
                        diagnostics: None,
                        error: None,
                    });
                }
                self.inner_mut()?
                    .runtime
                    .js_runtime()
                    .load_packages(&packages)
                    .with_context(|| format!("load packages: {}", packages.join(", ")))?;
                Ok(Response {
                    ok: true,
                    result: Some(json!({"loaded": packages})),
                    diagnostics: None,
                    error: None,
                })
            }
            Request::RunPython { code } => self.run_code("runPython", code),
            Request::RunPythonAsync { code } => self.run_python_async(code),
            Request::RunJs { code } => self.run_js(code),
            Request::Logs => Ok(Response {
                ok: true,
                result: Some(json!({"logs": self.inner()?.logs})),
                diagnostics: None,
                error: None,
            }),
            Request::Reset => {
                self.reset()?;
                Ok(Response {
                    ok: true,
                    result: Some(json!({"reset": true})),
                    diagnostics: None,
                    error: None,
                })
            }
        }
    }

    fn response_from_execution(&mut self, execution: ExecutionOutput) -> Result<Response> {
        {
            let inner = self.inner_mut()?;
            inner.logs.push_str(&execution.stdout);
            inner.logs.push_str(&execution.stderr);
        }

        let diagnostics = json!({
            "stdout": execution.stdout,
            "stderr": execution.stderr,
            "exception": execution.exception_type.as_ref().map(|kind| {
                json!({
                    "kind": kind,
                    "message": execution.exception_value.clone().unwrap_or_default(),
                    "traceback": execution.traceback,
                })
            }),
        });

        if let Some(kind) = execution.exception_type {
            return Ok(Response {
                ok: false,
                result: Some(json!({
                    "ok": false,
                    "exceptionType": kind,
                    "exceptionValue": execution.exception_value,
                    "traceback": execution.traceback,
                })),
                diagnostics: Some(diagnostics),
                error: execution.exception_value,
            });
        }

        let result = match execution.json {
            Some(value) => json!({"kind": "json", "value": value}),
            None => match execution.result {
                Some(value) => json!({"kind": "repr", "type": "str", "value": value}),
                None => json!({"kind": "json", "value": null}),
            },
        };

        Ok(Response {
            ok: true,
            result: Some(json!({
                "ok": true,
                "result": result,
            })),
            diagnostics: Some(diagnostics),
            error: None,
        })
    }

    fn run_code(&mut self, op: &str, code: String) -> Result<Response> {
        let input = json!({ "op": op, "code": code });
        let mut strategy = JsonInvocationStrategy::new(Some(input));
        let outcome = {
            let inner = self.inner_mut()?;
            inner
                .runtime
                .run_session_with_strategy(&inner.session, &mut strategy)
        }
        .with_context(|| format!("run {op}"))?;

        {
            let inner = self.inner_mut()?;
            inner.logs.push_str(&outcome.diagnostics.stdout);
            inner.logs.push_str(&outcome.diagnostics.stderr);
        }

        let diagnostics = serde_json::to_value(&outcome.diagnostics)
            .context("serialize execution diagnostics")?;

        match outcome.status {
            OutcomeStatus::Success(ResultPayload::Json(value)) => {
                let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
                Ok(Response {
                    ok,
                    result: Some(value),
                    diagnostics: Some(diagnostics),
                    error: None,
                })
            }
            OutcomeStatus::Success(payload) => Ok(Response {
                ok: true,
                result: Some(serde_json::to_value(payload).context("serialize payload")?),
                diagnostics: Some(diagnostics),
                error: None,
            }),
            OutcomeStatus::Failure(kind) => Ok(Response {
                ok: false,
                result: None,
                diagnostics: Some(diagnostics),
                error: Some(format!("{kind:?}")),
            }),
        }
    }

    fn run_python_async(&mut self, code: String) -> Result<Response> {
        let execution = self
            .inner_mut()?
            .runtime
            .js_runtime()
            .run_python_async_snippet(&code)
            .context("run Python async snippet")?;

        self.response_from_execution(execution)
    }

    fn run_js(&mut self, code: String) -> Result<Response> {
        let execution = self
            .inner_mut()?
            .runtime
            .js_runtime()
            .run_js_snippet(&code)
            .context("run JavaScript snippet")?;

        self.response_from_execution(execution)
    }
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let mut runner = CompatRunner::new(args.dist_dir)?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line.context("read stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => runner.handle(request).unwrap_or_else(error_response),
            Err(err) => error_response(err.into()),
        };
        serde_json::to_writer(&mut stdout, &response).context("write response")?;
        stdout.write_all(b"\n").context("write newline")?;
        stdout.flush().context("flush response")?;
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut dist_dir = env::var_os("AARDVARK_PYODIDE_DIST_DIR").map(PathBuf::from);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dist-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--dist-dir requires a path"))?;
                dist_dir = Some(PathBuf::from(value));
            }
            "--default-dist-dir" => {
                dist_dir = Some(default_dist_dir());
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { dist_dir })
}

fn default_dist_dir() -> PathBuf {
    PathBuf::from(".aardvark/pyodide-distributions").join(format!(
        "aardvark-{}-pyodide-v{}-full",
        env!("CARGO_PKG_VERSION"),
        aardvark_core::pyodide::PYODIDE_VERSION
    ))
}

fn error_response(err: anyhow::Error) -> Response {
    Response {
        ok: false,
        result: None,
        diagnostics: None,
        error: Some(err.to_string()),
    }
}

fn print_help() {
    println!(
        "Usage: aardvark-compat-runner [--dist-dir PATH | --default-dist-dir]\n\
         Reads JSON-line commands from stdin and writes JSON-line responses."
    );
}
