// SPDX-License-Identifier: CC0-1.0

//! The AWS Glue ETL library a job script finds on its import path: the `awsglue` package,
//! and the Spark runtime under it.
//!
//! "Starts a job run using a job definition" (`StartJobRun`), and a job definition names the
//! script it runs: a run of a job IS one execution of that script
//! ([`super::job_script`]). Every AWS Glue Spark job image ships the Glue ETL library, whose
//! documented entry points are `awsglue.context.GlueContext`,
//! `awsglue.dynamicframe.DynamicFrame`, `awsglue.job.Job` and
//! `awsglue.utils.getResolvedOptions` — so a script that imports them finds them here, and
//! the Spark engine they are built on is the workspace's ONE in-process `pyspark`
//! ([`::service_common::spark_runtime`]), shared with every other service that runs a
//! caller-supplied Spark program.
//!
//! # Nothing here is synthesised
//!
//! These are ordinary Python modules, handed to the contained interpreter as the modules on
//! the script's path, and what they do they really do:
//!
//! * a frame read `from_catalog` is READ — the table's metadata through the Data Catalog's
//!   own API and its data through the object store, both over the SDK the runtime supplies
//!   the guest, whose calls are signed as the role the run is performed as. So a read of a
//!   Lake-Formation-governed table the role holds no permission on is REFUSED by the catalog,
//!   and the run fails carrying that refusal; a read the catalog answers sees exactly the
//!   objects that were put at the table's location ([`table_data`]);
//! * a frame's transformations and actions are COMPUTED over those records, in this process,
//!   by the script's own functions ([`frames`]);
//! * a frame WRITTEN out leaves real objects in the object store, through the same shared
//!   filesystem a Spark program's own writes go through ([`context`]).
//!
//! A surface a script reaches for that this library does not supply raises the way a missing
//! attribute or an absent module raises, so the run FAILS carrying that error rather than
//! reporting a success nothing computed.
//!
//! # Bounds and containment
//!
//! Neither is here: both are the interpreter's, in host code no script can reach — the wall
//! clock, step, memory, recursion and output ceilings [`super::job_script`] gives one run,
//! and the native-module allow-list of the engine itself. What these modules add is the
//! bound on how much of a table ONE read takes ([`table_data`]), because that read is made
//! against this emulator.

mod context;
mod frames;
mod table_data;

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;

/// The modules this emulator puts on a job script's import path: the `awsglue` package and
/// the shared `pyspark` package under it, for a job whose `GlueVersion` is `glue_version`.
///
/// The caller adds its own entry module — the one that RUNS the script — because what runs a
/// script is the run's own concern ([`super::job_script`]).
#[must_use]
pub fn glue_etl_modules(glue_version: &str) -> Vec<PackageModule> {
    [
        ("awsglue/__init__.py", AWSGLUE_SOURCE.to_string()),
        ("awsglue/utils.py", AWSGLUE_UTILS_SOURCE.to_string()),
        ("awsglue/gluetypes.py", frames::GLUETYPES_SOURCE.to_string()),
        (
            "awsglue/_table_data.py",
            table_data::TABLE_DATA_SOURCE.to_string(),
        ),
        (
            "awsglue/dynamicframe.py",
            frames::DYNAMICFRAME_SOURCE.to_string(),
        ),
        ("awsglue/context.py", context::CONTEXT_SOURCE.to_string()),
        ("awsglue/data_sink.py", context::DATA_SINK_SOURCE.to_string()),
        (
            "awsglue/data_source.py",
            context::DATA_SOURCE_SOURCE.to_string(),
        ),
        ("awsglue/job.py", context::JOB_SOURCE.to_string()),
        (
            "awsglue/transforms.py",
            context::TRANSFORMS_SOURCE.to_string(),
        ),
    ]
    .into_iter()
    .map(|(path, source)| (path.to_string(), source))
    .chain(::service_common::spark_runtime::pyspark_modules(
        spark_version_of(glue_version),
    ))
    .map(|(path, source)| PackageModule { path, source })
    .collect()
}

/// Which Spark version a job's `GlueVersion` runs on, which is what its script reads as
/// `spark.version`.
///
/// SOURCE: the Glue versions AWS publishes for `Job$GlueVersion` — "Glue version determines
/// the versions of Apache Spark and Python that Glue supports", whose table pairs each Glue
/// version with the Spark it ships. A version outside that table is one nothing is known
/// about, and the script is told nothing rather than a version this emulator made up.
fn spark_version_of(glue_version: &str) -> Option<&'static str> {
    match glue_version.trim() {
        "0.9" => Some("2.2.1"),
        "1.0" | "2.0" => Some("2.4.3"),
        "3.0" => Some("3.1.1"),
        "4.0" => Some("3.3.0"),
        "5.0" => Some("3.5.4"),
        _ => None,
    }
}

/// `awsglue`: the package AWS Glue ships on a job script's import path.
const AWSGLUE_SOURCE: &str = r#"
"""The `awsglue` package, as this emulator supplies it on a job script's import path."""
"#;

/// `awsglue.utils`: the argument resolution a Glue job script reads its parameters through.
///
/// "`getResolvedOptions(args, options)`" takes `sys.argv` and the names the script wants,
/// and answers a dictionary of them — AWS documents it as the way "to retrieve the arguments
/// that were passed", and a job argument is named on the command line as `--name value`. A
/// name the line does not carry raises `GlueArgumentError`, because the script asked for a
/// parameter the run was not given.
const AWSGLUE_UTILS_SOURCE: &str = r#"
"""Resolving a Glue job's own arguments off the command line it was run with."""


class GlueArgumentError(Exception):
    """A requested job argument the run was not given."""


def _named_arguments(args):
    """Every `--name value` (or `--name=value`) pair of a command line, by name."""
    named = {}
    index = 0
    while index < len(args):
        token = args[index]
        index += 1
        if not token.startswith("--"):
            continue
        name = token[2:]
        if "=" in name:
            name, value = name.split("=", 1)
            named[name] = value
            continue
        if index < len(args) and not args[index].startswith("--"):
            named[name] = args[index]
            index += 1
        else:
            named[name] = ""
    return named


def getResolvedOptions(args, options):
    """The job arguments `options` names, read off the command line `args`."""
    named = _named_arguments(list(args))
    resolved = {}
    for option in options or []:
        name = option[2:] if option.startswith("--") else option
        if name not in named:
            raise GlueArgumentError("argument --%s is required" % name)
        resolved[name] = named[name]
    return resolved
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The library a script imports is supplied whole and once, with the shared Spark
    /// runtime under it, and the Spark version it is told is the one its own Glue version
    /// ships.
    #[test]
    fn the_library_is_supplied_once_with_the_spark_its_glue_version_ships() {
        // (case, the job's GlueVersion, the Spark version its script reads)
        let cases: [(&str, &str, &str); 5] = [
            ("the current Glue version", "5.0", "VERSION = \"3.5.4\""),
            ("the Glue version before it", "4.0", "VERSION = \"3.3.0\""),
            ("a Glue version from the 2.x era", "2.0", "VERSION = \"2.4.3\""),
            ("a version carrying spaces", " 3.0 ", "VERSION = \"3.1.1\""),
            ("a version no published table names", "9.9", "VERSION = None"),
        ];
        for (case, glue_version, stated) in cases {
            let modules = glue_etl_modules(glue_version);
            let mut paths: Vec<&str> = modules.iter().map(|module| module.path.as_str()).collect();
            let listed = paths.len();
            paths.sort_unstable();
            paths.dedup();
            assert_eq!(
                paths.len(),
                listed,
                "case {case}: a module is supplied twice, got {paths:?}"
            );
            for entry in ["awsglue/context.py", "awsglue/job.py", "pyspark/__init__.py"] {
                assert!(
                    paths.contains(&entry),
                    "case {case}: expected {entry} on the import path, got {paths:?}"
                );
            }
            let release = modules
                .iter()
                .find(|module| module.path == "pyspark/release.py")
                .map(|module| module.source.clone())
                .unwrap_or_default();
            assert!(
                release.contains(stated),
                "case {case}: expected the Spark version {stated:?}, got {release:?}"
            );
        }
    }
}
