use super::*;

pub(super) fn timing_stats(samples: &[Duration]) -> TimingStats {
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
    ms.sort_by(f64::total_cmp);

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

pub(super) fn timing_samples(
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

pub(super) fn setup_breakdown_stats(buckets: &SetupBreakdownBuckets) -> SetupBreakdownStats {
    SetupBreakdownStats {
        registry_init: timing_stats(&buckets.registry_init),
        artifact_parse: timing_stats(&buckets.artifact_parse),
        pool_create: timing_stats(&buckets.pool_create),
        handler_prepare: timing_stats(&buckets.handler_prepare),
        warm_all: (!buckets.warm_all.is_empty()).then(|| timing_stats(&buckets.warm_all)),
    }
}

pub(super) fn setup_breakdown_samples(buckets: &SetupBreakdownBuckets) -> SetupBreakdownSamples {
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

pub(super) fn expand_results(results: &[BenchResult]) -> Vec<BenchResult> {
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

pub(super) fn write_json(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(serde_json::to_string_pretty(results)?.as_bytes())?;
    Ok(())
}

pub(super) fn write_csv(path: &Path, results: &[BenchResult]) -> Result<()> {
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

pub(super) fn print_summary(results: &[BenchResult]) {
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
