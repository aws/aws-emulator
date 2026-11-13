// SPDX-License-Identifier: CC0-1.0

//! The WORK one canary run performs, and the verdict that work produces.
//!
//! A canary run's result is not a value this service chooses: it is the outcome of the work
//! the run did. The part of that work this emulator performs is the part that belongs to
//! other services of the account, and both halves of it are real:
//!
//! * the run's ARTIFACTS are written to the canary's `ArtifactS3Location`
//!   ([`perform`]), through this emulator's own Amazon S3, so a run reported as passed is one
//!   whose artifact a client can read back out of the bucket;
//! * whether the run MAY write them is IAM's answer about the canary's execution role
//!   (`service_common::iam_role::role_withholds_action`, this tree's one reading of what a
//!   role's policies permit), so a role that withholds the write is a run that fails.
//!
//! A run whose artifacts could not be stored carries the state reason code the model
//! documents for exactly that failure: "If this value is `EXECUTION_FAILURE`, a non-critical
//! failure occurred such as failing to save generated debug artifacts (for example,
//! screenshots or har files)"
//! (`smithy_query(query="doc",shape="CanaryRunStatus",member="StateReasonCode")`).
//!
//! Both hops are made the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]) — the AWS SDK over the
//! emulator's in-process transport, which opens no socket, resolves no name and reaches
//! nothing outside this process — through the tree's one construction of each client
//! (`service_common::s3_client`, `service_common::iam_client`). Nothing here executes the
//! canary's script: no browser verdict is reported, and `CanaryRunStatus$TestResult` (the
//! status "of canary script for this run") is left unset for that reason.

use super::{RunOutcome, S3_LOCATION_SCHEME};

/// The media type of the run report this emulator stores for a run.
const REPORT_CONTENT_TYPE: &str = "application/json";

/// The object a run's report is stored as, under the run's own folder.
///
/// CloudWatch Synthetics lays a run's artifacts out under a timestamped folder and names the
/// report after the verdict it carries (`SyntheticsReport-PASSED.json`). This emulator's
/// layout is CHOSEN and differs on both counts, deliberately: the folder is the run's own id,
/// which is the identity `GetCanaryRuns` addresses a run by (a timestamped folder would be an
/// invented second identity for it), and the name carries no verdict, because a run's verdict
/// can still change after its artifacts are stored — `StopCanary` cuts an open run short.
const REPORT_OBJECT: &str = "SyntheticsReport.json";

/// The action a run needs on the objects it writes: the artifacts of a run are PUT at the
/// canary's artifact location, so this is the one permission its execution role has to carry
/// for the run to produce anything.
const ARTIFACT_WRITE_ACTION: &str = "s3:PutObject";

/// Everything one run's work needs, copied out of the stored record so the store's lock is
/// not held while the work is done.
#[derive(Debug, Clone)]
pub struct RunWork {
    /// The canary the run belongs to.
    pub canary_name: String,
    /// The run's own id, which is the folder its artifacts are stored under.
    pub run_id: String,
    /// `Canary$ExecutionRoleArn` — "The ARN of the IAM role used to run the canary".
    pub execution_role_arn: String,
    /// The `ArtifactS3Location` the run's artifacts go to, in its `s3://` form.
    pub artifact_s3_location: String,
    /// The Region the canary lives in, which is the Region its artifact bucket and its role
    /// are resolved in.
    pub region: String,
}

/// Do the run's work, and report the verdict it produced.
///
/// In order: the artifact location is read as a bucket and a key prefix; IAM is asked whether
/// the execution role withholds the write of the run's artifact; and the artifact is written.
/// The run PASSES only on the last of those succeeding — the object exists in the account's
/// object store — and otherwise fails with the state reason code the model documents for
/// artifacts that could not be saved.
///
/// Call it with NO guard of this service's store held: it awaits.
pub async fn perform(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    work: &RunWork,
) -> RunOutcome {
    let Some((bucket, key)) = artifact_object(&work.artifact_s3_location, work) else {
        return RunOutcome::execution_failure(format!(
            "The canary's artifact location {} names no Amazon S3 bucket, so this run's \
             artifacts could not be saved.",
            work.artifact_s3_location
        ));
    };
    let object_arn = format!("arn:aws:s3:::{bucket}/{key}");
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        &work.region,
        &work.execution_role_arn,
        ARTIFACT_WRITE_ACTION,
        &object_arn,
    )
    .await
    {
        return RunOutcome::execution_failure(format!(
            "The canary's execution role {} does not permit {ARTIFACT_WRITE_ACTION} on \
             {object_arn}, so this run's artifacts could not be saved.",
            work.execution_role_arn
        ));
    }
    match write_report(ctx, work, &bucket, &key).await {
        Ok(()) => RunOutcome::passed(),
        Err(detail) => RunOutcome::execution_failure(format!(
            "This run's artifacts could not be saved to \
             {S3_LOCATION_SCHEME}{bucket}/{key}: {detail}"
        )),
    }
}

/// The bucket and the object key this run's report is stored at.
///
/// The location is `s3://<bucket>` or `s3://<bucket>/<key prefix>` (the form
/// [`super::is_s3_artifact_location`] admits and `CreateCanary` refuses anything else); the
/// report of one run goes under that prefix, in a folder of the canary's name and the run's
/// own id, so two runs of one canary cannot overwrite each other's artifacts.
fn artifact_object(location: &str, work: &RunWork) -> Option<(String, String)> {
    let bucket_and_prefix = location.strip_prefix(S3_LOCATION_SCHEME)?;
    let (bucket, prefix) = match bucket_and_prefix.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix.trim_end_matches('/')),
        None => (bucket_and_prefix, ""),
    };
    if bucket.is_empty() {
        return None;
    }
    let mut key = String::new();
    if !prefix.is_empty() {
        key.push_str(prefix);
        key.push('/');
    }
    key.push_str(&work.canary_name);
    key.push('/');
    key.push_str(&work.run_id);
    key.push('/');
    key.push_str(REPORT_OBJECT);
    Some((bucket.to_string(), key))
}

/// Write the run's report object, and report what the object store answered when it could not
/// be written.
///
/// The document reports the run it belongs to, read from the run's own record: the canary,
/// the run's id and the location its artifacts were stored under. Nothing about a browser
/// session is in it — no script was executed, so there is no step, screenshot or HAR file to
/// report.
async fn write_report(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    work: &RunWork,
    bucket: &str,
    key: &str,
) -> Result<(), String> {
    let document = ::serde_json::json!({
        "canaryName": work.canary_name,
        "runId": work.run_id,
        "artifactS3Location": work.artifact_s3_location,
    })
    .to_string();
    ::service_common::s3_client::s3_client(ctx.callback(), &work.region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(REPORT_CONTENT_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            document.into_bytes(),
        ))
        .send()
        .await
        .map(|_written| ())
        .map_err(|error| {
            // What the store ANSWERED, when it answered at all: a failure carrying no error
            // code is no object store having answered, which is still a run with no artifact
            // stored — so the reason says which of the two it was.
            use ::aws_smithy_types::error::metadata::ProvideErrorMetadata as _;
            match error.code() {
                Some(code) => format!("the object store answered {code}"),
                None => "no object store answered the write".to_string(),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The work a row below describes, with the bucket-and-prefix part under test.
    fn work(location: &str) -> RunWork {
        RunWork {
            canary_name: "probe".to_string(),
            run_id: "11111111-2222-3333-4444-555555555555".to_string(),
            execution_role_arn: "arn:aws:iam::123456789012:role/CanaryRole".to_string(),
            artifact_s3_location: location.to_string(),
            region: "us-east-1".to_string(),
        }
    }

    /// One row of the table below: what it varies, the artifact location it varies it with,
    /// and the bucket and key the report of a run is then addressed by.
    struct Row {
        case: &'static str,
        location: &'static str,
        object: Option<(&'static str, &'static str)>,
    }

    /// RULE: a run's report is stored in the bucket the artifact location names, under that
    /// location's key prefix, in a folder of the canary's name and the run's own id.
    #[test]
    fn the_report_object_is_addressed_by_the_artifact_location() {
        let rows = [
            Row {
                case: "bucket only",
                location: "s3://artifacts",
                object: Some((
                    "artifacts",
                    "probe/11111111-2222-3333-4444-555555555555/SyntheticsReport.json",
                )),
            },
            Row {
                case: "bucket and prefix",
                location: "s3://artifacts/canary",
                object: Some((
                    "artifacts",
                    "canary/probe/11111111-2222-3333-4444-555555555555/SyntheticsReport.json",
                )),
            },
            Row {
                case: "nested prefix with a trailing slash",
                location: "s3://artifacts/team/canaries/",
                object: Some((
                    "artifacts",
                    "team/canaries/probe/11111111-2222-3333-4444-555555555555/SyntheticsReport.json",
                )),
            },
            Row {
                case: "not an s3 location",
                location: "artifacts/canary",
                object: None,
            },
            Row {
                case: "no bucket at all",
                location: "s3:///canary",
                object: None,
            },
        ];
        for row in rows {
            let actual = artifact_object(row.location, &work(row.location));
            let expected = row
                .object
                .map(|(bucket, key)| (bucket.to_string(), key.to_string()));
            assert_eq!(
                actual, expected,
                "case {:?}: expected {expected:?}, got {actual:?}",
                row.case
            );
        }
    }
}
