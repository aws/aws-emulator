// SPDX-License-Identifier: CC0-1.0

//! The SPARK runtime a caller-supplied Python program finds on its import path.
//!
//! Every service of this workspace whose data plane RUNS a Spark program — a submitted
//! application, the script of an extract-transform-load job — runs that program in the
//! contained Python interpreter this workspace embeds, and a program built on Spark resolves
//! its engine through the `pyspark` package. These are the modules this emulator supplies
//! for it, in ONE place: ordinary Python source, handed to the interpreter as the modules on
//! the program's path, exactly as the AWS SDK modules a handler imports are handed to it.
//!
//! # What they are, and what they are not
//!
//! They are an ENGINE, not a stub and not a transcript: every transformation and every
//! action is COMPUTED, in this process, by running the program's own functions over the
//! program's own data ([`dataset`]). `parallelize` slices a collection into partitions,
//! `map` calls the program's function on each element, `reduceByKey` folds the program's
//! operator over the values of each key, and `collect` answers what those computations
//! produced. Nothing is read off the program's source text and no result is synthesised: a
//! program that computes a sum gets the sum of its own data, and one whose function raises
//! gets that exception.
//!
//! Beside the package, this module holds the ENTRY module a submitted program is run FROM
//! ([`submitted_program_modules`]): running one as `spark-submit` runs it — as `__main__`,
//! with its own path and arguments on `sys.argv` — and reporting the status it exited with,
//! its two console streams and the exception it ended on is the same for every service whose
//! data plane submits a program, so it is spelled here once as well.
//!
//! What they are not is a distributed Spark. The partitions are lists in this process, there
//! is no cluster to schedule them on, and the surface is the one this emulator can compute —
//! the resilient distributed dataset, the session and context that hand one out, the dataset of
//! rows with its column expressions ([`sql`], [`column`], [`functions`]), the columnar format
//! it reads and writes ([`parquet`]), and the Hadoop filesystem a Spark program reads and
//! writes through. A program that reaches for anything else (a streaming source, the
//! machine-learning library) raises the way a missing attribute raises, so the work it was
//! submitted by FAILS carrying that error rather than reporting a success nothing computed.
//!
//! # Where its filesystem reads and writes land
//!
//! A path a program names in Amazon S3 is a real object of this emulator's own object store:
//! [`filesystem`] performs each read and write with the AWS SDK the runtime supplies the
//! guest (`boto3`), whose calls travel over the emulator's in-process callback and are
//! signed as the identity the work runs as. So a program that writes its output to `s3://…`
//! leaves an object a later `GetObject` really reads, and one that reads from `s3://…` sees
//! what was really put there. A path in any other filesystem — a node's own disk, HDFS — is
//! one this emulator holds nothing for, and the filesystem says so the way Hadoop says it,
//! rather than pretending to have written it.
//!
//! # Bounds and containment
//!
//! Neither is here: both are the interpreter's, in host code the program cannot reach (the
//! deadline, the step budget, the memory ceiling, the recursion limit and the output cap the
//! calling service gives one run, and the native-module allow-list of the engine itself).
//! These modules add no host surface of their own: they reach the object store through the
//! same SDK seam any guest call uses, and nothing else.

mod catalog;
mod column;
mod context;
mod dataset;
mod expression;
mod filesystem;
mod functions;
mod parquet;
mod sql;
mod statements;
mod types;

/// The module the INTERPRETER registers a Spark session's engine on, under which a program's
/// statements are planned and run ([`SessionSql`]).
///
/// Named here, beside the runtime source that imports it ([`statements`]), because it is this
/// runtime's seam rather than any one interpreter's: the embedded interpreter registers the
/// module under this name for a run whose caller supplied an engine, exactly as it registers
/// the rest of a guest's host surface.
pub const HOST_MODULE: &str = "_spark_host";

/// The one function on that module: the submitted statement in, what the engine answered out.
pub const HOST_QUERY_FUNCTION: &str = "query";

/// The class a statement's ANALYSIS failure is raised as — a name the catalog holds nothing
/// under, say — which is the class Spark's own driver raises such a failure as.
pub const ANALYSIS_FAILURE_CLASS: &str = "AnalysisException";

/// The class every OTHER failure of the engine is raised as, carrying the engine's own
/// message: the failure class a Spark driver reports a failure of the engine itself as.
pub const ENGINE_FAILURE_CLASS: &str = "SparkException";

/// Where the SQL a Spark session is asked to run is really PLANNED AND RUN: the engine the
/// calling service holds, over its own catalog and the data behind it.
///
/// A Spark driver resolves the names a statement carries in the session's catalog, reads the
/// data those tables point at and runs the statement over it — none of which the Python client
/// does. So the service whose data plane submits the program supplies that engine, and this is
/// the seam it is reached through: one submitted statement in, what the engine answered out,
/// both as JSON documents, for the reason every other guest-to-host seam of this workspace
/// carries documents rather than objects.
///
/// What arrives: `{"query": "<the statement>", "database": "<the session's current
/// database>"}`.
///
/// What goes back, either the answer
/// `{"columns": [{"name": "<name>", "type": "<the Spark type it holds, where the engine states
/// one>"}], "rows": [[<value>, …]]}` — a statement that CHANGED the catalog rather than reading
/// answers no columns and no rows — or the refusal `{"error": {"class": "<the class the driver
/// raises it as>", "message": "<the engine's own message>"}}`.
pub trait SessionSql: ::std::fmt::Debug + Send + Sync {
    /// What the engine answered for the statement `document` carries.
    fn answered(&self, document: &str) -> String;
}

/// What the environment one Spark program runs IN states about itself — the two things that
/// are not the same for every caller of this runtime.
///
/// Each is a real setting of the environment, not a choice of this module: the Spark release
/// it ships, and whether its Hadoop output committer leaves the success marker a completed
/// write is marked by.
#[derive(Debug, Clone, Copy)]
pub struct SparkEnvironment<'a> {
    /// The version of Spark the environment ships, which is what the program is told it is
    /// running on — `None` for an environment that bundles no Spark, where nothing is known
    /// to tell it.
    pub spark_version: Option<&'a str>,
    /// Whether a write leaves the `_SUCCESS` marker object beside its part files, which is
    /// the environment's `mapreduce.fileoutputcommitter.marksuccessfuljobs` setting.
    pub marks_successful_jobs: bool,
    /// The Spark properties the environment's own configuration SETS, which a submitted
    /// program's configuration starts with — the `spark-defaults` of the environment, as
    /// `spark-submit` loads them for the program it submits.
    pub settings: &'a [(String, String)],
}

impl<'a> SparkEnvironment<'a> {
    /// An environment shipping `spark_version` whose committer leaves the success marker,
    /// which is the Hadoop default, and whose own configuration sets nothing.
    #[must_use]
    pub fn shipping(spark_version: Option<&'a str>) -> Self {
        Self {
            spark_version,
            marks_successful_jobs: true,
            settings: &[],
        }
    }

    /// The same environment, with the marker switched off — for one whose own committer
    /// leaves none.
    #[must_use]
    pub fn leaving_no_success_marker(self) -> Self {
        Self {
            marks_successful_jobs: false,
            ..self
        }
    }

    /// The same environment, whose own configuration sets these Spark properties.
    #[must_use]
    pub fn setting(self, settings: &'a [(String, String)]) -> Self {
        Self { settings, ..self }
    }
}

/// The `pyspark` package this emulator puts on a Spark program's import path, as
/// `(module path, source)` pairs — the form every embedded-interpreter caller hands its
/// modules over in.
///
/// `spark_version` is the version of Spark the environment the program runs in ships, which
/// is what the program is told it is running on ([`release_source`]) — `None` for an
/// environment that bundles no Spark, where nothing is known to tell it. The committer of
/// such an environment leaves the success marker; one whose own leaves none is supplied
/// through [`pyspark_modules_of`].
#[must_use]
pub fn pyspark_modules(spark_version: Option<&str>) -> Vec<(String, String)> {
    pyspark_modules_of(SparkEnvironment::shipping(spark_version))
}

/// The same package, for an environment that states what [`SparkEnvironment`] carries.
#[must_use]
pub fn pyspark_modules_of(environment: SparkEnvironment<'_>) -> Vec<(String, String)> {
    vec![
        ("pyspark/__init__.py".to_string(), PYSPARK_SOURCE.to_string()),
        ("pyspark/release.py".to_string(), release_source(environment)),
        (
            "pyspark/defaults.py".to_string(),
            defaults_source(environment.settings),
        ),
        ("pyspark/conf.py".to_string(), CONF_SOURCE.to_string()),
        (
            "pyspark/catalog.py".to_string(),
            catalog::CATALOG_SOURCE.to_string(),
        ),
        (
            "pyspark/filesystem.py".to_string(),
            filesystem::FILESYSTEM_SOURCE.to_string(),
        ),
        ("pyspark/rdd.py".to_string(), dataset::RDD_SOURCE.to_string()),
        (
            "pyspark/context.py".to_string(),
            context::CONTEXT_SOURCE.to_string(),
        ),
        ("pyspark/jvm.py".to_string(), context::JVM_SOURCE.to_string()),
        (
            "pyspark/parquet.py".to_string(),
            parquet::PARQUET_SOURCE.to_string(),
        ),
        ("pyspark/sql/__init__.py".to_string(), sql::SQL_SOURCE.to_string()),
        (
            "pyspark/sql/types.py".to_string(),
            types::TYPES_SOURCE.to_string(),
        ),
        (
            "pyspark/sql/column.py".to_string(),
            column::COLUMN_SOURCE.to_string(),
        ),
        (
            "pyspark/sql/functions.py".to_string(),
            functions::FUNCTIONS_SOURCE.to_string(),
        ),
        (
            "pyspark/sql/expression.py".to_string(),
            expression::EXPRESSION_SOURCE.to_string(),
        ),
        (
            "pyspark/sql/statements.py".to_string(),
            statements_source(),
        ),
    ]
}

/// `pyspark.sql.statements`: the client half of a session's statements, with the seam it
/// reaches the engine through written into it — so the module imports the module the
/// interpreter really registers ([`HOST_MODULE`]) rather than a name spelled twice.
fn statements_source() -> String {
    statements::STATEMENTS_SOURCE
        .replace("@@HOST_MODULE@@", HOST_MODULE)
        .replace("@@HOST_FUNCTION@@", HOST_QUERY_FUNCTION)
        .replace("@@ANALYSIS_CLASS@@", ANALYSIS_FAILURE_CLASS)
        .replace("@@ENGINE_CLASS@@", ENGINE_FAILURE_CLASS)
}

/// `pyspark.release`: what this ENVIRONMENT states about itself — the version of Spark it
/// ships, the Apache Parquet writer that release ships, and whether its output committer
/// leaves the success marker.
///
/// The version is the one that environment's own catalog publishes for Spark, so a program on
/// an older release is told that release's version rather than one spelled here. An
/// environment that bundles no Spark tells the program nothing about a version it has none of.
fn release_source(environment: SparkEnvironment<'_>) -> String {
    let version = match environment.spark_version {
        // Rendered through the JSON encoder, so the catalog's value reaches the module as a
        // string literal whatever it holds.
        Some(version) => ::serde_json::Value::String(version.to_string()).to_string(),
        None => "None".to_string(),
    };
    let writer = match parquet_writer(environment.spark_version) {
        Some(stamped) => ::serde_json::Value::String(stamped).to_string(),
        None => "None".to_string(),
    };
    format!(
        "{RELEASE_SOURCE}\n# The version of Spark this environment's own release ships.\nVERSION \
         = {version}\n# The Apache Parquet writer that release ships, as it stamps the footer \
         of a\n# file it wrote.\nPARQUET_WRITER = {writer}\n# Whether this environment's output \
         committer leaves the marker object a completed\n# write is marked by, which is its \
         own\n# `mapreduce.fileoutputcommitter.marksuccessfuljobs` \
         setting.\nMARKS_SUCCESSFUL_JOBS = {marker}\n",
        marker = if environment.marks_successful_jobs {
            "True"
        } else {
            "False"
        }
    )
}

/// The Apache Parquet writer an environment shipping `spark_version` writes its Parquet
/// output with, as that writer stamps its own name and version into a file's footer
/// (`FileMetaData.created_by`).
///
/// SOURCE: the `parquet-mr` release each Apache Spark release ships — the library Spark's
/// Parquet writer IS, whose version is what a reader of the footer finds in `created_by` and
/// reads a file's statistics against. A Spark release outside this table is one nothing is
/// known about, so a file it writes is stamped with nothing rather than with a version this
/// emulator made up.
fn parquet_writer(spark_version: Option<&str>) -> Option<String> {
    let version = spark_version?;
    let mut parts = version.trim().split('.');
    let release = (parts.next()?, parts.next()?);
    let shipped = match release {
        ("2", "2" | "3") => "1.8.2",
        ("2", "4") | ("3", "0" | "1") => "1.10.1",
        ("3", "2") => "1.12.1",
        ("3", "3") => "1.12.2",
        ("3", "4") => "1.12.3",
        ("3", "5") => "1.13.1",
        _ => return None,
    };
    Some(format!("parquet-mr version {shipped}"))
}

/// The doc comment [`release_source`] builds `pyspark/release.py` from.
const RELEASE_SOURCE: &str = r#""""The Spark this environment's release ships, as its program is told it."""
"#;

/// `pyspark.defaults`: the Spark properties the ENVIRONMENT's own configuration sets, which
/// are the defaults a submitted program's configuration starts with.
///
/// `spark-submit` loads the environment's `spark-defaults` and hands them to the application
/// it submits, so a property the environment sets is one the program reads back off its own
/// configuration — and one every part of this runtime that is governed by a property reads
/// ([`sql`]). Rendered through the JSON encoder, so a name or a value reaches the module as
/// the literal it is whatever it holds.
fn defaults_source(settings: &[(String, String)]) -> String {
    let mut rendered = String::from(DEFAULTS_SOURCE);
    rendered.push_str("SETTINGS = {\n");
    for (name, value) in settings {
        rendered.push_str(&format!(
            "    {}: {},\n",
            ::serde_json::Value::String(name.clone()),
            ::serde_json::Value::String(value.clone())
        ));
    }
    rendered.push_str("}\n");
    rendered
}

/// The doc comment [`defaults_source`] builds `pyspark/defaults.py` from.
const DEFAULTS_SOURCE: &str = r#""""The Spark properties this environment's own configuration sets."""

# The properties `spark-submit` loaded out of the environment's own configuration, which are
# the defaults a submitted program's `SparkConf` starts with.
"#;

/// `pyspark`: the names a program imports from the package itself.
const PYSPARK_SOURCE: &str = r#"
"""The `pyspark` package, as this emulator supplies it to a submitted program."""

from pyspark.conf import SparkConf
from pyspark.context import SparkContext
from pyspark.rdd import RDD

__all__ = ["SparkConf", "SparkContext", "RDD"]
"#;

/// `pyspark.conf`: the configuration a program is submitted with, which it sets and reads
/// back.
const CONF_SOURCE: &str = r#"
"""The configuration of one Spark application."""

# The properties the ENVIRONMENT the program runs in sets, which a configuration that loads
# the defaults starts with ([`pyspark.defaults`]).
from pyspark.defaults import SETTINGS as _ENVIRONMENT_SETTINGS


class SparkConf(object):
    """The key/value configuration `SparkConf` holds, as set and read back."""

    def __init__(self, loadDefaults=True, _jvm=None, _jconf=None):
        # "if loadDefaults is true, also load values from the environment": the properties
        # the environment's own configuration sets are what the program then reads back,
        # and a setting of its own takes precedence over them.
        self._settings = dict(_ENVIRONMENT_SETTINGS) if loadDefaults else {}

    def set(self, key, value):
        self._settings[str(key)] = str(value)
        return self

    def setAll(self, pairs):
        for key, value in pairs:
            self.set(key, value)
        return self

    def setIfMissing(self, key, value):
        if str(key) not in self._settings:
            self.set(key, value)
        return self

    def setAppName(self, value):
        return self.set("spark.app.name", value)

    def setMaster(self, value):
        return self.set("spark.master", value)

    def setSparkHome(self, value):
        return self.set("spark.home", value)

    def get(self, key, defaultValue=None):
        return self._settings.get(str(key), defaultValue)

    def getAll(self):
        return list(self._settings.items())

    def contains(self, key):
        return str(key) in self._settings

    def toDebugString(self):
        return "\n".join("%s=%s" % (key, value) for key, value in self.getAll())
"#;

/// The module every service of this workspace RUNS a submitted Spark program FROM, and the
/// function in it the interpreter calls.
///
/// A `spark-submit` runs its program as the `__main__` module, with the program's own path and
/// arguments on `sys.argv`, and reports what the run came to; that is the same for every
/// service whose data plane submits one, so it is spelled here rather than per service
/// ([`submitted_program_source`]).
pub const SUBMITTED_PROGRAM_MODULE: &str = "__spark_submit_program";

/// See [`SUBMITTED_PROGRAM_MODULE`].
pub const SUBMITTED_PROGRAM_FUNCTION: &str = "run";

/// The member a run's event carries to say the program is a CELL of a live session rather
/// than a standalone submit: the engine of such a session is already started, so the session
/// is bound for the program as `spark` and its context as `sc` before the first line runs.
///
/// Named here because the entry module below reads it: a caller that runs a program inside a
/// session it already started sets it, and a caller that submits a standalone program leaves
/// it out — such a program starts its own session, which is what `getOrCreate` is for.
pub const SESSION_BOUND_EVENT_MEMBER: &str = "sessionBound";

/// The handler the interpreter resolves for it: `<module>.<function>`.
#[must_use]
pub fn submitted_program_handler() -> String {
    format!("{SUBMITTED_PROGRAM_MODULE}.{SUBMITTED_PROGRAM_FUNCTION}")
}

/// The modules a submitted Spark program finds on its import path, as `(module path, source)`
/// pairs: the `pyspark` package ([`pyspark_modules`]) and the entry module that RUNS the
/// program ([`SUBMITTED_PROGRAM_MODULE`]).
///
/// This is what a calling service hands the interpreter. `spark_version` is the version of
/// Spark the environment the program runs in ships, as [`pyspark_modules`] states it.
#[must_use]
pub fn submitted_program_modules(spark_version: Option<&str>) -> Vec<(String, String)> {
    submitted_program_modules_of(SparkEnvironment::shipping(spark_version))
}

/// The same modules, for an environment that states what [`SparkEnvironment`] carries — the
/// Spark release it ships and the properties its own configuration sets.
#[must_use]
pub fn submitted_program_modules_of(environment: SparkEnvironment<'_>) -> Vec<(String, String)> {
    ::std::iter::once((
        format!("{SUBMITTED_PROGRAM_MODULE}.py"),
        SUBMITTED_PROGRAM_SOURCE.to_string(),
    ))
    .chain(pyspark_modules_of(environment))
    .collect()
}

/// The entry module's own source, for a reader of this module.
#[must_use]
pub fn submitted_program_source() -> &'static str {
    SUBMITTED_PROGRAM_SOURCE
}

/// The entry module: it runs the submitted program the way `spark-submit` runs one — as the
/// `__main__` module, with the program's path and its own arguments on `sys.argv` — and
/// reports what the run came to: the status it exited with, the two console streams it wrote,
/// and the exception it ended on when it ended on one.
///
/// The streams are reported SEPARATELY because that is what a client-mode `spark-submit`
/// leaves: the driver's own output on standard output, and the engine's logging and any
/// traceback on standard error. What each keeps is bounded by the `streamCharacters` the
/// calling service passes in the run's event, so a chatty program cannot grow the answer
/// without limit.
///
/// A run whose event says it is a CELL of a live session ([`SESSION_BOUND_EVENT_MEMBER`])
/// finds that session already bound, as an interactive engine leaves it: `spark` for the
/// session and `sc` for its context. A standalone submit binds neither, because its program
/// starts its own.
///
/// Reporting rather than raising is how the run's outcome reaches the calling service: the
/// engine's own bounds are unaffected, because a run whose budget was spent is reported as
/// stopped whatever the program answered.
const SUBMITTED_PROGRAM_SOURCE: &str = r#"
"""Runs a submitted Spark program, as the Python program it is."""

import sys
import traceback


class _Console:
    """One console stream of the program, bounded by what one run keeps of it."""

    def __init__(self, limit):
        self._parts = []
        self._kept = 0
        self._limit = limit

    def write(self, text):
        if not isinstance(text, str):
            text = str(text)
        room = self._limit - self._kept
        if room > 0:
            kept = text[:room]
            self._parts.append(kept)
            self._kept += len(kept)
        return len(text)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        return None

    def isatty(self):
        return False

    def text(self):
        return "".join(self._parts)


def run(event, context):
    out = _Console(event["streamCharacters"])
    err = _Console(event["streamCharacters"])
    held = (sys.stdout, sys.stderr)
    sys.stdout = out
    sys.stderr = err
    status = 0
    error_type = ""
    error_message = ""
    try:
        # The command line a Spark program resolves its arguments off: its own path followed
        # by the arguments the submit passed it, which is what spark-submit hands it.
        sys.argv = [event["path"]] + list(event["arguments"])
        globals_of_main = {"__name__": "__main__", "__file__": event["path"]}
        # A cell of a LIVE session finds the session's engine already started: an interactive
        # runtime starts the session before it takes the first submission and leaves it bound
        # as `spark`, with its context as `sc`. A standalone submit binds neither — its
        # program starts its own session — so the two are bound only for a run that says it
        # is a session's.
        if event.get("sessionBound"):
            from pyspark.sql import SparkSession

            bound = SparkSession.builder.getOrCreate()
            globals_of_main["spark"] = bound
            globals_of_main["sc"] = bound.sparkContext
        exec(compile(event["source"], event["path"], "exec"), globals_of_main)
    except SystemExit as ended:
        # sys.exit() is how a program names the status it exits with: no argument and None are
        # a clean exit, an integer is that status — masked to the eight bits a process exit
        # carries, which is what a submit observes of a real driver — and anything else is
        # printed on standard error and exits non-zero, exactly as the interpreter does with it.
        code = ended.code
        if code is None or code is False:
            status = 0
        elif code is True:
            status = 1
        elif isinstance(code, int):
            status = int(code) & 0xFF
        else:
            status = 1
            err.write("%s\n" % (code,))
    except BaseException as raised:
        # spark-submit writes the driver's traceback on standard error and exits non-zero.
        status = 1
        error_type = type(raised).__name__
        error_message = str(raised)
        trace = sys.exc_info()[2]
        # This module's own frame is not part of the program's traceback.
        if trace is not None and trace.tb_next is not None:
            trace = trace.tb_next
        err.write("".join(traceback.format_exception(type(raised), raised, trace)))
    finally:
        sys.stdout, sys.stderr = held
    return {
        "exitStatus": status,
        "stdout": out.text(),
        "stderr": err.text(),
        "errorType": error_type,
        "errorMessage": error_message,
    }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every module the runtime supplies is named once and carries source, and what the
    /// ENVIRONMENT states — the Spark version it ships, and whether its committer leaves the
    /// success marker — is what the caller named.
    #[test]
    fn the_runtime_names_each_module_once_and_carries_what_the_environment_states() {
        // (case, the environment, what its release module must state)
        let cases: [(&str, SparkEnvironment<'_>, [&str; 3]); 4] = [
            (
                "an environment shipping Spark 3.3",
                SparkEnvironment::shipping(Some("3.3.0")),
                [
                    "VERSION = \"3.3.0\"",
                    "PARQUET_WRITER = \"parquet-mr version 1.12.2\"",
                    "MARKS_SUCCESSFUL_JOBS = True",
                ],
            ),
            (
                "an environment shipping Spark 3.5",
                SparkEnvironment::shipping(Some("3.5.4")),
                [
                    "VERSION = \"3.5.4\"",
                    "PARQUET_WRITER = \"parquet-mr version 1.13.1\"",
                    "MARKS_SUCCESSFUL_JOBS = True",
                ],
            ),
            (
                "an environment bundling no Spark",
                SparkEnvironment::shipping(None),
                [
                    "VERSION = None",
                    "PARQUET_WRITER = None",
                    "MARKS_SUCCESSFUL_JOBS = True",
                ],
            ),
            (
                "an environment whose committer leaves no marker",
                SparkEnvironment::shipping(Some("2.4.3")).leaving_no_success_marker(),
                [
                    "VERSION = \"2.4.3\"",
                    "PARQUET_WRITER = \"parquet-mr version 1.10.1\"",
                    "MARKS_SUCCESSFUL_JOBS = False",
                ],
            ),
        ];
        for (case, environment, stated) in cases {
            let modules = pyspark_modules_of(environment);
            let mut paths: Vec<&str> = modules.iter().map(|(path, _)| path.as_str()).collect();
            let listed = paths.len();
            paths.sort_unstable();
            paths.dedup();
            assert_eq!(
                paths.len(),
                listed,
                "case {case}: a module is supplied twice, got {paths:?}"
            );
            for (path, source) in &modules {
                assert!(
                    !source.trim().is_empty(),
                    "case {case}: {path} carries no source"
                );
            }
            let release = modules
                .iter()
                .find(|(path, _)| path == "pyspark/release.py")
                .map(|(_, source)| source.clone())
                .unwrap_or_default();
            for held in stated {
                assert!(
                    release.contains(held),
                    "case {case}: expected the release module to state {held:?}, got \
                     {release:?}"
                );
            }
        }
    }

    /// The entry module reads the SAME event member the caller sets to say its program is a
    /// cell of a live session, and binds both names such a cell finds bound.
    #[test]
    fn the_entry_module_binds_the_session_the_event_member_names() {
        for held in [
            format!("event.get(\"{SESSION_BOUND_EVENT_MEMBER}\")"),
            "globals_of_main[\"spark\"]".to_string(),
            "globals_of_main[\"sc\"]".to_string(),
        ] {
            assert!(
                submitted_program_source().contains(&held),
                "case {held:?}: the entry module must carry it, got {:?}",
                submitted_program_source()
            );
        }
    }
}
