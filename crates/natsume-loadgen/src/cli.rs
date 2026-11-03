use std::{collections::HashSet, path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu};

use crate::error::{self, Error, Result};

#[derive(Parser)]
#[command(about = "Simulated Natsume workstation fleet")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    Run(Box<Run>),
    Setup(crate::setup::Setup),
}

#[derive(Clone, Copy, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Arrival {
    Uniform,
    AllAtOnce,
}

#[derive(Args)]
pub(crate) struct Run {
    /// Scenario values override the corresponding CLI timing/fleet values.
    #[arg(long)]
    pub(crate) scenario: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) operator: crate::operator::Credentials,
    #[arg(long, default_value_t = 0)]
    pub(crate) operators: usize,
    #[arg(long, value_parser = duration, default_value = "10s")]
    pub(crate) poll_interval: Duration,
    #[arg(long, value_parser = duration, default_value = "30s")]
    pub(crate) session_interval: Duration,
    /// XLSX used by scheduled import actions unless the event specifies its own file.
    #[arg(long)]
    pub(crate) operator_roster: Option<PathBuf>,
    #[arg(skip)]
    pub(crate) events: Vec<crate::scenario::TimedEvent>,
    /// Canonical HTTPS server origin, without a trailing slash.
    #[arg(long)]
    pub(crate) server: String,
    #[arg(long)]
    pub(crate) trust_root: PathBuf,
    #[arg(long)]
    pub(crate) state_dir: PathBuf,
    /// One-column CSV, optionally headed `seat_code`; row i is device i.
    #[arg(long)]
    pub(crate) seat_plan: PathBuf,
    /// A new directory for each run; contains events.csv.gz and run.json.
    #[arg(long)]
    pub(crate) output_dir: PathBuf,
    #[arg(long, default_value_t = 600)]
    pub(crate) devices: usize,
    #[arg(long, default_value_t = 1)]
    pub(crate) seed: u64,
    #[arg(long, value_parser = duration, default_value = "0s")]
    pub(crate) ramp: Duration,
    #[arg(long, value_enum, default_value = "uniform")]
    pub(crate) arrival: Arrival,
    /// Measurement time after the last scheduled arrival.
    #[arg(long, value_parser = duration, default_value = "3m")]
    pub(crate) duration: Duration,
    /// Resource delay is uniform in [base, base + jitter].
    #[arg(long, value_parser = duration, default_value = "20ms")]
    pub(crate) resource_delay: Duration,
    #[arg(long, value_parser = duration, default_value = "0ms")]
    pub(crate) resource_jitter: Duration,
    #[arg(long, value_parser = duration)]
    pub(crate) gateway_delay: Option<Duration>,
    #[arg(long, value_parser = duration)]
    pub(crate) binding_delay: Option<Duration>,
    #[arg(long, value_parser = duration)]
    pub(crate) runtime_delay: Option<Duration>,
    #[arg(long, value_parser = duration)]
    pub(crate) session_delay: Option<Duration>,
    #[arg(long, value_parser = duration)]
    pub(crate) home_delay: Option<Duration>,
    #[arg(long, value_parser = duration)]
    pub(crate) power_delay: Option<Duration>,
    /// Zero disables logo polling.
    #[arg(long, value_parser = duration, default_value = "60s")]
    pub(crate) logo_interval: Duration,
    #[arg(long, value_parser = duration, default_value = "5s")]
    pub(crate) reconnect_initial: Duration,
    #[arg(long, value_parser = duration, default_value = "30s")]
    pub(crate) reconnect_max: Duration,
    #[arg(long, value_parser = duration, default_value = "60s")]
    pub(crate) stable_after: Duration,
    #[arg(long, value_parser = duration, default_value = "30s")]
    pub(crate) ping_interval: Duration,
    #[arg(long, value_parser = duration, default_value = "60s")]
    pub(crate) silence_timeout: Duration,
}

impl Run {
    pub(crate) fn validate(&self) -> Result<Vec<String>> {
        if !crate::target::canonical_origin(&self.server) {
            return Err(Error::Invalid {
                code: "SERVER_ORIGIN",
            });
        }
        if self.devices == 0
            || self.devices > 100_000
            || self.duration.is_zero()
            || self.reconnect_initial.is_zero()
            || self.reconnect_max < self.reconnect_initial
            || self.ping_interval.is_zero()
            || self.silence_timeout <= self.ping_interval
            || self.operators > 1000
            || self.poll_interval.is_zero()
            || self.session_interval.is_zero()
            || (matches!(self.arrival, Arrival::AllAtOnce) && !self.ramp.is_zero())
        {
            return Err(Error::Invalid {
                code: "TIMING_OR_FLEET_SIZE",
            });
        }
        if self.events.windows(2).any(|pair| pair[0].at > pair[1].at)
            || self.events.iter().any(|event| {
                event.at >= self.ramp + self.duration
                    || (event.action == crate::scenario::Action::Import
                        && event.roster.is_none()
                        && self.operator_roster.is_none())
            })
        {
            return Err(Error::Invalid {
                code: "SCENARIO_EVENTS",
            });
        }
        let encoded = std::fs::read_to_string(&self.seat_plan).context(error::IoSnafu)?;
        parse_seats(&encoded, self.devices)
    }

    pub(crate) fn arrival_offset(&self, index: usize) -> Duration {
        if self.devices <= 1 || matches!(self.arrival, Arrival::AllAtOnce) {
            Duration::ZERO
        } else {
            let nanos = self.ramp.as_nanos() * index as u128 / (self.devices - 1) as u128;
            Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
        }
    }
}

#[derive(Debug, PartialEq, Eq, Snafu)]
pub(crate) enum DurationError {
    #[snafu(display("use an integer followed by ms, s, m or h"))]
    Unit,
    #[snafu(display("duration must be an unsigned integer"))]
    Number { source: std::num::ParseIntError },
    #[snafu(display("duration must be between 0 and 7 days"))]
    Range,
}

pub(crate) fn duration(value: &str) -> std::result::Result<Duration, DurationError> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        return UnitSnafu.fail();
    };
    number
        .parse::<u64>()
        .context(NumberSnafu)?
        .checked_mul(multiplier)
        .filter(|ms| *ms <= 7 * 24 * 3_600_000)
        .map(Duration::from_millis)
        .context(RangeSnafu)
}

fn parse_seats(encoded: &str, devices: usize) -> Result<Vec<String>> {
    let mut seats = Vec::new();
    let mut unique = HashSet::new();
    for (index, line) in encoded.lines().enumerate() {
        let seat = csv_cell(line)?;
        if index == 0 && seat == "seat_code" {
            continue;
        }
        if seat.is_empty()
            || seat.len() > 64
            || seat.chars().any(char::is_control)
            || !unique.insert(seat.clone())
        {
            return Err(Error::Invalid { code: "SEAT_PLAN" });
        }
        seats.push(seat);
    }
    if seats.len() < devices {
        return Err(Error::Invalid {
            code: "SEAT_PLAN_TOO_SHORT",
        });
    }
    seats.truncate(devices);
    Ok(seats)
}

fn csv_cell(line: &str) -> Result<String> {
    let Some(quoted) = line.strip_prefix('"') else {
        if line.contains([',', '"']) {
            return Err(Error::Invalid {
                code: "SEAT_PLAN_COLUMNS",
            });
        }
        return Ok(line.to_owned());
    };
    let mut output = String::new();
    let mut chars = quoted.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '"' {
            output.push(character);
        } else if chars.peek() == Some(&'"') {
            chars.next();
            output.push('"');
        } else if chars.peek().is_none() {
            return Ok(output);
        } else {
            return Err(Error::Invalid {
                code: "SEAT_PLAN_QUOTES",
            });
        }
    }
    Err(Error::Invalid {
        code: "SEAT_PLAN_QUOTES",
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn run() -> Run {
        let cli = Cli::try_parse_from([
            "loadgen",
            "run",
            "--server",
            "https://127.0.0.1:8443",
            "--trust-root",
            "root.crt",
            "--state-dir",
            "state",
            "--seat-plan",
            "seats.csv",
            "--output-dir",
            "events",
        ])
        .unwrap_or_else(|error| panic!("valid CLI fixture: {error}"));
        let Command::Run(run) = cli.command else {
            panic!("run fixture");
        };
        *run
    }

    #[test]
    fn seat_plan_preserves_text_and_rejects_collisions() -> Result<()> {
        assert_eq!(parse_seats("seat_code\n001\n\"A,B\"\n", 2)?, ["001", "A,B"]);
        for invalid in ["A\nA", "A,B", "\nA", "\"A\",\"B\"", "\"open"] {
            assert!(parse_seats(invalid, 2).is_err());
        }
        assert!(parse_seats("A", 2).is_err());
        Ok(())
    }

    #[test]
    fn duration_requires_units_and_bounds() {
        assert_eq!(duration("3m"), Ok(Duration::from_mins(3)));
        assert!(duration("999999999999h").is_err());
        assert!(duration("30").is_err());
    }
}
