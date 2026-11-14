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
//! What they are not is a distributed Spark. The partitions are lists in this process, there
//! is no cluster to schedule them on, and the surface is the one this emulator can compute —
//! the resilient distributed dataset, the session and context that hand one out, and the
//! Hadoop filesystem a Spark program reads and writes through. A program that reaches for
//! anything else (the SQL planner, a streaming source, the machine-learning library) raises
//! the way a missing attribute raises, so the work it was submitted by FAILS carrying that
//! error rather than reporting a success nothing computed.
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

mod context;
mod dataset;
mod filesystem;
mod sql;

/// The `pyspark` package this emulator puts on a Spark program's import path, as
/// `(module path, source)` pairs — the form every embedded-interpreter caller hands its
/// modules over in.
///
/// `spark_version` is the version of Spark the environment the program runs in ships, which
/// is what the program is told it is running on ([`release_source`]) — `None` for an
/// environment that bundles no Spark, where nothing is known to tell it.
#[must_use]
pub fn pyspark_modules(spark_version: Option<&str>) -> Vec<(String, String)> {
    vec![
        ("pyspark/__init__.py".to_string(), PYSPARK_SOURCE.to_string()),
        ("pyspark/release.py".to_string(), release_source(spark_version)),
        ("pyspark/conf.py".to_string(), CONF_SOURCE.to_string()),
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
        ("pyspark/sql/__init__.py".to_string(), sql::SQL_SOURCE.to_string()),
    ]
}

/// `pyspark.release`: the ONE value of this runtime that is not the same for every run — the
/// version of Spark the environment ships, which a program reads as `spark.version`.
///
/// It is the version that environment's own catalog publishes for Spark, so a program on an
/// older release is told that release's version rather than one spelled here. An environment
/// that bundles no Spark tells the program nothing about a version it has none of.
fn release_source(spark_version: Option<&str>) -> String {
    let version = match spark_version {
        // Rendered through the JSON encoder, so the catalog's value reaches the module as a
        // string literal whatever it holds.
        Some(version) => ::serde_json::Value::String(version.to_string()).to_string(),
        None => "None".to_string(),
    };
    format!(
        "{RELEASE_SOURCE}\n# The version of Spark this environment's own release ships.\nVERSION \
         = {version}\n"
    )
}

/// The doc comment [`release_source`] builds `pyspark/release.py` from.
const RELEASE_SOURCE: &str = r#""""The Spark this environment's release ships, as its program is told it."""
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


class SparkConf(object):
    """The key/value configuration `SparkConf` holds, as set and read back."""

    def __init__(self, loadDefaults=True, _jvm=None, _jconf=None):
        self._settings = {}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every module the runtime supplies is named once and carries source, and the ONE
    /// per-run value — the Spark version the environment ships — is the one the caller
    /// named.
    #[test]
    fn the_runtime_names_each_module_once_and_carries_the_environments_version() {
        // (case, the version the environment ships, what the module must state)
        let cases: [(&str, Option<&str>, &str); 3] = [
            ("an environment shipping Spark 3.3", Some("3.3.0"), "VERSION = \"3.3.0\""),
            ("an environment shipping Spark 3.5", Some("3.5.4"), "VERSION = \"3.5.4\""),
            ("an environment bundling no Spark", None, "VERSION = None"),
        ];
        for (case, version, stated) in cases {
            let modules = pyspark_modules(version);
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
            assert!(
                release.contains(stated),
                "case {case}: expected the release module to state {stated:?}, got \
                 {release:?}"
            );
        }
    }
}
