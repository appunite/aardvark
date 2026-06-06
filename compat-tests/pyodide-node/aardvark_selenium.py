#!/usr/bin/env python3
"""Small local controller that mimics the useful part of Pyodide's selenium fixture."""

from __future__ import annotations

import json
import select
import subprocess
from pathlib import Path
from typing import Any


class AardvarkGuestError(RuntimeError):
    def __init__(self, response: dict[str, Any]):
        self.response = response
        payload = response.get("result") or {}
        message = response.get("error")
        if not message and isinstance(payload, dict):
            message = payload.get("exceptionValue") or payload.get("exceptionType")
        super().__init__(message or json.dumps(response, sort_keys=True))


class AardvarkSelenium:
    """A minimal Pyodide-test fixture adapter backed by aardvark-compat-runner."""

    JavascriptException = AardvarkGuestError

    def __init__(
        self,
        repo_root: Path,
        *,
        dist_dir: Path | None = None,
        command_timeout_seconds: float = 120.0,
        runner_command: list[str] | None = None,
    ) -> None:
        self.repo_root = repo_root
        self.browser = "node"
        self.command_timeout_seconds = command_timeout_seconds
        self._last_response: dict[str, Any] | None = None
        command = runner_command or [
            "cargo",
            "run",
            "-q",
            "-p",
            "aardvark-compat-runner",
            "--",
        ]
        if dist_dir is not None:
            command.extend(["--dist-dir", str(dist_dir)])
        self._proc = subprocess.Popen(
            command,
            cwd=repo_root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.command({"op": "ping"})

    @property
    def last_response(self) -> dict[str, Any] | None:
        return self._last_response

    @property
    def logs(self) -> str:
        response = self.command({"op": "logs"})
        result = response.get("result") or {}
        return str(result.get("logs", ""))

    def close(self) -> None:
        if self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                self._proc.wait(timeout=5)

    def command(self, payload: dict[str, Any]) -> dict[str, Any]:
        if self._proc.poll() is not None:
            stderr = self._proc.stderr.read() if self._proc.stderr else ""
            raise RuntimeError(f"aardvark-compat-runner exited early: {stderr}")
        assert self._proc.stdin is not None
        assert self._proc.stdout is not None
        self._proc.stdin.write(json.dumps(payload) + "\n")
        self._proc.stdin.flush()
        ready, _, _ = select.select(
            [self._proc.stdout], [], [], self.command_timeout_seconds
        )
        if not ready:
            op = payload.get("op", "<unknown>")
            self.close()
            raise TimeoutError(
                f"aardvark-compat-runner timed out after "
                f"{self.command_timeout_seconds:g}s waiting for {op}"
            )
        line = self._proc.stdout.readline()
        if not line:
            stderr = self._proc.stderr.read() if self._proc.stderr else ""
            raise RuntimeError(f"aardvark-compat-runner produced no response: {stderr}")
        response = json.loads(line)
        self._last_response = response
        if not response.get("ok"):
            raise AardvarkGuestError(response)
        return response

    def load_package(self, packages: str | list[str]) -> None:
        if isinstance(packages, str):
            packages = [packages]
        self.command({"op": "loadPackage", "packages": packages})

    def run(self, code: str) -> Any:
        return self._run_code("runPython", code)

    def run_async(self, code: str) -> Any:
        return self._run_code("runPythonAsync", code)

    def run_js(self, code: str) -> Any:
        return self._run_code("runJs", code)

    def reset(self) -> None:
        self.command({"op": "reset"})

    def _run_code(self, op: str, code: str) -> Any:
        response = self.command({"op": op, "code": code})
        payload = response.get("result") or {}
        if not payload.get("ok"):
            raise AardvarkGuestError(response)
        result = payload.get("result") or {}
        return result.get("value")

    def __enter__(self) -> "AardvarkSelenium":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()
