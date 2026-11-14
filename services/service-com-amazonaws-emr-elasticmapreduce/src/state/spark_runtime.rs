// SPDX-License-Identifier: CC0-1.0

//! The SPARK runtime a submitted application finds on its import path.
//!
//! A `spark-submit` of a Python application runs that application in the contained Python
//! interpreter this workspace embeds ([`super::spark_step_run`]), and an application built on
//! Spark resolves its engine through the `pyspark` package. That package is the workspace's
//! ONE in-process Spark runtime ([`::service_common::spark_runtime`]), shared with every
//! other service whose data plane runs a caller-supplied Spark program, so what an
//! application computes here is what one computes there. What this module adds is the part
//! that is this service's own: the entry module that RUNS a submitted application
//! ([`APPLICATION_ENTRY_SOURCE`]).
//!
//! What those modules are, where their filesystem reads and writes land, and why neither the
//! bounds nor the containment live in them is stated by the shared module: both are the
//! interpreter's, in host code the application cannot reach (the deadline, the step budget,
//! the memory ceiling, the recursion limit and the output cap of [`super::spark_step_run`],
//! and the native-module allow-list of the engine itself).

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;

/// The modules this emulator puts on a submitted application's import path, as the
/// interpreter takes them: the shared `pyspark` package, and the entry module that RUNS the
/// application ([`APPLICATION_ENTRY_SOURCE`]).
///
/// `spark_version` is the version of Spark the cluster's own release ships, which is what the
/// application is told it is running on — `None` for a cluster whose release bundles no
/// Spark, where nothing is known to tell it.
#[must_use]
pub fn spark_runtime_modules(spark_version: Option<&str>) -> Vec<PackageModule> {
    ::std::iter::once((
        format!("{APPLICATION_ENTRY_MODULE}.py"),
        APPLICATION_ENTRY_SOURCE.to_string(),
    ))
    .chain(::service_common::spark_runtime::pyspark_modules(spark_version))
    .map(|(path, source)| PackageModule { path, source })
    .collect()
}

/// The module this emulator runs the submitted application FROM, and the function in it the
/// engine calls.
pub const APPLICATION_ENTRY_MODULE: &str = "__emr_spark_application";
/// See [`APPLICATION_ENTRY_MODULE`].
pub const APPLICATION_ENTRY_FUNCTION: &str = "run";
/// The handler the interpreter resolves: `<module>.<function>`.
#[must_use]
pub fn application_entry_handler() -> String {
    format!("{APPLICATION_ENTRY_MODULE}.{APPLICATION_ENTRY_FUNCTION}")
}

/// The entry module: it runs the application the way `spark-submit` runs one — as the
/// `__main__` module, with the application's path and its own arguments on `sys.argv` — and
/// lets whatever the application raises travel out to the engine, which is what reports it.
const APPLICATION_ENTRY_SOURCE: &str = r#"
"""Runs a submitted Spark application, as the Python program it is."""

import sys


def run(event, context):
    # The command line a Spark application resolves its arguments off: its own path
    # followed by the arguments the submit passed it, which is what spark-submit hands it.
    sys.argv = [event["path"]] + list(event["arguments"])
    globals_of_main = {"__name__": "__main__", "__file__": event["path"]}
    exec(compile(event["source"], event["path"], "exec"), globals_of_main)
    return None
"#;
