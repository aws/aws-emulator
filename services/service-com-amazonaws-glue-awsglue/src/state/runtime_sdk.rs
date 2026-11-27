// SPDX-License-Identifier: CC0-1.0

//! WHICH AWS SDK a run's runtime supplies the code it runs.
//!
//! A Glue runtime ships the AWS SDK for Python on the import path of everything it runs — a
//! job script, a session's statements — pinned to the release that runtime was built with, so
//! `boto3` is there without the script installing anything. Which RELEASE that is follows the
//! `GlueVersion` the run is on: "Glue version determines the versions of Apache Spark and
//! Python that Glue supports" (`CreateJob$GlueVersion`), and AWS's table of those versions
//! pairs each one with the Python and the AWS SDK its runtime ships.
//!
//! That pinning is observable, because an SDK's own generation decides WHICH WIRE its client
//! of a service speaks. The services that had published an AWS Query API — the queue service
//! among them — publish ONE API over TWO wires: the JSON protocol their models declare and the
//! form-encoded AWS Query surface they kept for the clients released before that move, which is
//! what the query-compatible modifier on their models records. Each of those surfaces words its
//! own refusals. So a run on a Glue version whose runtime pins an SDK from before that move
//! reaches such a service on the Query surface and is answered there, exactly as it is on AWS.
//!
//! RECORDED: a `boto3` call inside a run of a Python shell job on Glue 3.0 observes the Query
//! surface's own wording of the refusal for a queue that is not there, rather than the JSON
//! surface's.

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::BundledSdk;

/// The first `GlueVersion` whose runtime ships an AWS SDK release published AFTER the AWS SDKs
/// moved the services that had published an AWS Query API — the queue service among them — onto
/// an AWS JSON protocol.
///
/// SOURCE: the Glue versions AWS publishes for `Job$GlueVersion`, whose table pairs each
/// version with the Python and the AWS SDK its runtime ships — the releases pinned into every
/// version before this one predate that move, and the ones from this version on follow it. A
/// `GlueVersion` is "major.minor" ("for example: 1.0, 2.0, 3.0"), so it is the major number
/// that says which runtime a run gets.
const FIRST_GLUE_VERSION_WITH_THE_JSON_QUERY_APIS: u32 = 5;

/// The AWS SDK generation the runtime of `glue_version` supplies the code it runs.
///
/// The version a run is ON is this crate's one reading of that member
/// ([`super::effective_glue_version`]), so a job that left the choice to the service is given
/// the runtime of the version the service runs it on. A value that is no such version says
/// nothing about a runtime, and a run of one is given the current generation rather than a
/// pinning this emulator invented for it — which is also what a run on a version at or past the
/// move gets.
#[must_use]
pub fn bundled_sdk_of(glue_version: &str) -> BundledSdk {
    let pins_the_older_sdk = super::effective_glue_version(glue_version)
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major < FIRST_GLUE_VERSION_WITH_THE_JSON_QUERY_APIS);
    if pins_the_older_sdk {
        BundledSdk::BeforeTheQueryApisMovedToJson
    } else {
        BundledSdk::Current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glue_version_decides_which_sdk_generation_a_run_is_given() {
        // (case, the `GlueVersion` the run is on, the generation its runtime supplies)
        let rows: [(&str, &str, BundledSdk); 9] = [
            ("the earliest published version", "0.9", BundledSdk::BeforeTheQueryApisMovedToJson),
            ("the first whole version", "1.0", BundledSdk::BeforeTheQueryApisMovedToJson),
            ("the version before last", "2.0", BundledSdk::BeforeTheQueryApisMovedToJson),
            (
                "the version this issue was recorded on",
                "3.0",
                BundledSdk::BeforeTheQueryApisMovedToJson,
            ),
            (
                "the last version before the move",
                "4.0",
                BundledSdk::BeforeTheQueryApisMovedToJson,
            ),
            ("the first version from after it", "5.0", BundledSdk::Current),
            ("a minor release of that version", "5.1", BundledSdk::Current),
            (
                "the version a create with none is given",
                super::super::AUTO_GLUE_VERSION,
                BundledSdk::Current,
            ),
            ("a value that names no version at all", "not-a-version", BundledSdk::Current),
        ];

        for (case, glue_version, expected) in rows {
            let given = bundled_sdk_of(glue_version);
            assert_eq!(
                given, expected,
                "case {case} ({glue_version:?}): expected {expected:?}, got {given:?}"
            );
        }
    }
}
