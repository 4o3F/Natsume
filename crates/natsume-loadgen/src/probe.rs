use std::{path::PathBuf, time::Duration};

use clap::Args;
use snafu::ResultExt as _;

use crate::{
    error::{self, Result},
    metrics::Metrics,
    operator::{Credentials, Operator},
};

/// New operator connections, for measuring admission while existing tabs keep polling.
#[derive(Args)]
pub(crate) struct Probe {
    #[arg(long)]
    server: String,
    #[arg(long)]
    trust_root: PathBuf,
    #[command(flatten)]
    operator: Credentials,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long, default_value_t = 6)]
    attempts: usize,
}

pub(crate) async fn execute(probe: Probe) -> Result<()> {
    let (metrics, writer) = Metrics::start(&probe.output_dir)?;
    for index in 0..probe.attempts {
        let mut operator = Operator::new(
            &probe.server,
            &probe.trust_root,
            &probe.operator,
            Some((metrics.clone(), index)),
        )?;
        let result = match operator.login().await {
            Ok(()) => operator.probe_devices().await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::warn!(
                attempt = index,
                code = error.code(),
                "operator probe failed"
            );
        }
        drop(operator);
        if index + 1 < probe.attempts {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
    drop(metrics);
    writer.await.context(error::JoinSnafu)??;
    Ok(())
}

/// One export with a preauthenticated operator and an absolute request deadline.
#[derive(Args)]
pub(crate) struct Export {
    #[arg(long)]
    server: String,
    #[arg(long)]
    trust_root: PathBuf,
    #[command(flatten)]
    operator: Credentials,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long)]
    at_unix_us: u64,
}

pub(crate) async fn execute_export(export: Export) -> Result<()> {
    let (metrics, writer) = Metrics::start(&export.output_dir)?;
    let mut operator = Operator::new(
        &export.server,
        &export.trust_root,
        &export.operator,
        Some((metrics.clone(), 0)),
    )?;
    let result = async {
        operator.login().await?;
        crate::wait_until_epoch(export.at_unix_us).await?;
        operator.export().await
    }
    .await;
    drop(operator);
    drop(metrics);
    writer.await.context(error::JoinSnafu)??;
    result
}

/// Sequential raster decode timings with anonymous per-file summaries.
#[derive(Args)]
pub(crate) struct DecodeLogos {
    #[arg(long)]
    directory: PathBuf,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=100))]
    rounds: u32,
    /// Restrict synthetic directories to the 140 logos referenced by the roster.
    #[arg(long)]
    synthetic: bool,
    /// Select evenly spaced file-size ranks, without exporting names or contents.
    #[arg(long, default_value_t = 140, value_parser = clap::value_parser!(u32).range(1..=10000))]
    sample_count: u32,
    /// Emit distributions only when measuring field data.
    #[arg(long)]
    aggregate_only: bool,
}

fn decode_file(path: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(path).context(error::IoSnafu)?;
    let format = image::guess_format(&bytes).context(error::ImageSnafu)?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().context(error::ImageSnafu)?;
    let (width, height) = image::ImageDecoder::dimensions(&decoder);
    if width > 4096 || height > 4096 || u64::from(width) * u64::from(height) > 4 * 1024 * 1024 {
        return Err(crate::error::InvalidSnafu {
            code: "LOGO_DECODE_DIMENSIONS",
        }
        .build());
    }
    let raster = image::DynamicImage::from_decoder(decoder).context(error::ImageSnafu)?;
    std::hint::black_box(raster);
    Ok(())
}

fn timing_summary(values: &[u64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let quantile = |q: usize| {
        sorted
            .get((sorted.len() * q).div_ceil(100).saturating_sub(1))
            .copied()
    };
    serde_json::json!({"count":sorted.len(),"p50_us":quantile(50),"p90_us":quantile(90),"p95_us":quantile(95),"p99_us":quantile(99),"sum_us":sorted.iter().sum::<u64>()})
}

pub(crate) fn execute_decode(options: DecodeLogos) -> Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(options.directory).context(error::IoSnafu)? {
        let entry = entry.context(error::IoSnafu)?;
        if !entry.file_type().context(error::IoSnafu)?.is_file() {
            continue;
        }
        if options.synthetic
            && !entry
                .file_name()
                .to_string_lossy()
                .starts_with("Synthetic School ")
        {
            continue;
        }
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            continue;
        }
        files.push((
            entry.metadata().context(error::IoSnafu)?.len(),
            entry.path(),
        ));
    }
    files.sort();
    let available = files.len();
    let count = available.min(options.sample_count as usize);
    let selected: Vec<_> = (0..count)
        .map(|i| &files[i * available / count].1)
        .collect();
    let mut per_file = vec![Vec::new(); count];
    let mut totals = Vec::new();
    let mut failures = 0;
    for round in 0..=options.rounds {
        let mut total = 0;
        for (index, path) in selected.iter().enumerate() {
            let start = std::time::Instant::now();
            let succeeded = decode_file(path).is_ok();
            let elapsed = u64::try_from(start.elapsed().as_micros()).context(error::SizeSnafu)?;
            if round > 0 {
                if succeeded {
                    per_file[index].push(elapsed);
                    total += elapsed;
                } else {
                    failures += 1;
                }
            }
        }
        if round > 0 {
            totals.push(total);
        }
    }
    let medians: Vec<_> = per_file
        .iter_mut()
        .filter_map(|times| {
            times.sort_unstable();
            times.get(times.len() / 2).copied()
        })
        .collect();
    let mut report = serde_json::json!({"available":available,"selected":count,"rounds":options.rounds,"failures":failures,"per_file_median":timing_summary(&medians),"set_totals":timing_summary(&totals),"method":"image 0.25.10 / image-webp 0.2.4; read + bounded ImageReader + DynamicImage::from_decoder; one warmup pass"});
    if !options.aggregate_only {
        report["anonymous_per_file_us"] = serde_json::json!(per_file);
    }
    tracing::info!(result = %report, "logo decode timings");
    Ok(())
}
