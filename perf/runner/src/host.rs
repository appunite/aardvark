use super::scenarios::*;
use super::*;
use std::fs::File;
use std::io::Read;

const MAX_HOST_LOCKFILE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn bench_host(
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

pub(super) fn host_modes_for_scenario(scenario: Scenario) -> &'static [Mode] {
    match scenario {
        // Pyodide 0.29.4 pins matplotlib 3.8.4 for Python 3.13.2. That exact
        // native CPython package set is not generally wheel-installable, so the
        // default full matrix avoids producing a misleading or host-dependent row.
        Scenario::Matplotlib | Scenario::NumpyMatmul | Scenario::ScipySgemm => &[],
        _ => Mode::host_modes(),
    }
}

pub(super) fn aardvark_modes_for_scenario(scenario: Scenario) -> &'static [Mode] {
    match scenario {
        Scenario::Tensor => Mode::aardvark_modes(),
        _ => Mode::aardvark_modes(),
    }
}

fn host_runtime_plan(scenario: Scenario) -> Result<HostRuntimePlan> {
    let lock_path = pyodide_lock_path()?;
    let lock_text =
        read_text_file_limited(&lock_path, MAX_HOST_LOCKFILE_BYTES, "Pyodide lockfile")?;
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

fn read_text_file_limited(path: &Path, limit: u64, kind: &str) -> Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    if len > limit {
        anyhow::bail!(
            "refusing to read {kind} {}: {} bytes exceeds the {} byte limit",
            path.display(),
            len,
            limit
        );
    }
    let mut bytes = Vec::with_capacity(len.min(8 * 1024 * 1024) as usize);
    let mut limited = file.take(limit.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.len() as u64 > limit {
        anyhow::bail!(
            "{kind} {} exceeded the {} byte limit while reading",
            path.display(),
            limit
        );
    }
    String::from_utf8(bytes).with_context(|| format!("read {} as UTF-8", path.display()))
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

    let package = lock_packages.get(package_key).ok_or_else(|| {
        anyhow!(
            "{} resolved Pyodide package '{}' but metadata was missing",
            lock_path.display(),
            package_key
        )
    })?;
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
        .join(default_pyodide_dist_dir_name())
        .join("pyodide-lock.json");
    if default_path.exists() {
        return Ok(default_path);
    }

    Err(anyhow!(
        "host Python benchmarks require a staged Pyodide distribution so Python and package versions can be pinned; set AARDVARK_PYODIDE_DIST_DIR or PYODIDE_DIST_DIR"
    ))
}

fn default_pyodide_dist_dir_name() -> String {
    format!(
        "aardvark-{}-pyodide-v{}-full",
        aardvark_core::AARDVARK_VERSION,
        aardvark_core::pyodide::PYODIDE_VERSION
    )
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
