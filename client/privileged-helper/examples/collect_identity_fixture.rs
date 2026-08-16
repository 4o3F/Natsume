// Development-only anonymized hardware fixture collector; never packaged.
//
// G0-IN-005 field evidence must characterize the shipped collectors, so this example
// delegates to the production `hardware_identity::collect` pipeline and only owns the
// anonymized serialization. It never prints raw hardware values.

use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, Write as _},
    path::Path,
    process::ExitCode,
};

use natsume_machine_identity::{
    ANCHOR_ORDER, CollectionCompleteness, EvidenceQuality, EvidenceStatus, collection_completeness,
    evaluate_slot,
};
use natsume_privileged_helper::hardware_identity;
use serde::Serialize;

const USAGE: &str = "usage: collect_identity_fixture --namespace <uuid>";

#[derive(Serialize)]
struct FixtureSlot {
    anchor_kind: &'static str,
    status: EvidenceStatus,
    quality: EvidenceQuality,
    candidate_id: Option<String>,
}

#[derive(Serialize)]
struct FixtureRecord {
    slots: [FixtureSlot; 3],
    completeness: CollectionCompleteness,
}

fn write_usage() {
    let _write_result = writeln!(io::stderr().lock(), "{USAGE}");
}

fn write_output_error() {
    let _write_result = writeln!(
        io::stderr().lock(),
        "collect_identity_fixture: failed to write JSON output"
    );
}

fn namespace_argument() -> Option<OsString> {
    let mut arguments = env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(flag), Some(namespace), None) if flag == OsStr::new("--namespace") => Some(namespace),
        _ => None,
    }
}

fn main() -> ExitCode {
    let Some(namespace_argument) = namespace_argument() else {
        write_usage();
        return ExitCode::from(2);
    };
    let Ok(namespace_text) = namespace_argument.into_string() else {
        write_usage();
        return ExitCode::from(2);
    };
    let Ok(fleet_namespace) = namespace_text.parse() else {
        write_usage();
        return ExitCode::from(2);
    };

    let readings = hardware_identity::collect(Path::new("/"));
    let evaluations = [
        evaluate_slot(ANCHOR_ORDER[0], &readings[0], fleet_namespace),
        evaluate_slot(ANCHOR_ORDER[1], &readings[1], fleet_namespace),
        evaluate_slot(ANCHOR_ORDER[2], &readings[2], fleet_namespace),
    ];
    let statuses = [
        evaluations[0].status,
        evaluations[1].status,
        evaluations[2].status,
    ];
    let slots = std::array::from_fn(|index| FixtureSlot {
        anchor_kind: ANCHOR_ORDER[index].label(),
        status: evaluations[index].status,
        quality: evaluations[index].quality,
        candidate_id: evaluations[index]
            .candidate_id
            .map(|candidate| candidate.to_string()),
    });
    let record = FixtureRecord {
        slots,
        completeness: collection_completeness(&statuses),
    };

    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &record).is_err() || writeln!(stdout).is_err() {
        write_output_error();
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
