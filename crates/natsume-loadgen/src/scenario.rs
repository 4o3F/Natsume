use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::{
    cli::{Arrival, Run},
    error::{self, Result},
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Scenario {
    devices: Option<usize>,
    arrival: Option<Arrival>,
    ramp: Option<String>,
    duration: Option<String>,
    resource_delay: Option<String>,
    resource_jitter: Option<String>,
    gateway_delay: Option<String>,
    binding_delay: Option<String>,
    runtime_delay: Option<String>,
    session_delay: Option<String>,
    home_delay: Option<String>,
    power_delay: Option<String>,
    logo_interval: Option<String>,
    reconnect_initial: Option<String>,
    reconnect_max: Option<String>,
    stable_after: Option<String>,
    ping_interval: Option<String>,
    silence_timeout: Option<String>,
    operators: Option<usize>,
    poll_interval: Option<String>,
    session_interval: Option<String>,
    panel_rates: Option<std::collections::BTreeMap<String, f64>>,
    pub(crate) schools: Option<usize>,
    pub(crate) logo_bytes: Option<usize>,
    pub(crate) logo_dimension: Option<u32>,
    pub(crate) events: Vec<TimedEvent>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimedEvent {
    #[serde(with = "duration_string")]
    pub(crate) at: Duration,
    pub(crate) action: Action,
    pub(crate) roster: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Waiting,
    Contest,
    Terminate,
    ResetHome,
    Export,
    Import,
}

impl Action {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Waiting => "WAITING",
            Self::Contest => "CONTEST",
            Self::Terminate => "TERMINATE",
            Self::ResetHome => "RESET_HOME",
            Self::Export => "EXPORT",
            Self::Import => "IMPORT",
        }
    }
    pub(crate) const fn is_target(self) -> bool {
        !matches!(self, Self::Export | Self::Import)
    }
}

impl Scenario {
    pub(crate) const fn device_count(&self) -> Option<usize> {
        self.devices
    }
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let mut scenario: Self =
            toml::from_str(&std::fs::read_to_string(path).context(error::IoSnafu)?)
                .context(error::TomlSnafu)?;
        for event in &mut scenario.events {
            if let Some(roster) = &mut event.roster {
                *roster = path.parent().unwrap_or(Path::new(".")).join(&*roster);
            }
        }
        Ok(scenario)
    }

    pub(crate) fn apply(self, run: &mut Run) -> Result<()> {
        macro_rules! value { ($($field:ident),*) => { $(if let Some(value) = self.$field { run.$field = value; })* }; }
        macro_rules! timing { ($($field:ident),*) => { $(if let Some(value) = self.$field { run.$field = parse_duration(&value)?; })* }; }
        macro_rules! optional_timing { ($($field:ident),*) => { $(if let Some(value) = self.$field { run.$field = Some(parse_duration(&value)?); })* }; }
        value!(devices, arrival, operators);
        timing!(
            ramp,
            duration,
            resource_delay,
            resource_jitter,
            logo_interval,
            reconnect_initial,
            reconnect_max,
            stable_after,
            ping_interval,
            silence_timeout,
            poll_interval,
            session_interval
        );
        optional_timing!(
            gateway_delay,
            binding_delay,
            runtime_delay,
            session_delay,
            home_delay,
            power_delay
        );
        run.panel_rates = self.panel_rates;
        run.events = self.events;
        Ok(())
    }
}

pub(crate) fn parse_duration(value: &str) -> Result<Duration> {
    crate::cli::duration(value).context(error::DurationSnafu)
}

mod duration_string {
    use super::{Deserialize, Duration};
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        de: D,
    ) -> std::result::Result<Duration, D::Error> {
        crate::cli::duration(&String::deserialize(de)?).map_err(serde::de::Error::custom)
    }
    pub(super) fn serialize<S: serde::Serializer>(
        duration: &Duration,
        ser: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        ser.serialize_str(&format!("{}ms", duration.as_millis()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenarios_parse_and_reject_unknown_fields() -> Result<()> {
        for file in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios"))
            .context(error::IoSnafu)?
        {
            let scenario = Scenario::load(&file.context(error::IoSnafu)?.path())?;
            let mut run = crate::cli::tests::run();
            scenario.apply(&mut run)?;
            assert!(run.events.windows(2).all(|pair| pair[0].at <= pair[1].at));
        }
        assert!(toml::from_str::<Scenario>("devicez = 100").is_err());
        assert!(toml::from_str::<Scenario>("[[events]]\nat='x'\naction='contest'").is_err());
        Ok(())
    }
}
