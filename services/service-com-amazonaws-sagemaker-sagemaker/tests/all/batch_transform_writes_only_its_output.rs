// SPDX-License-Identifier: CC0-1.0

//! WHERE a batch transform job's run touches Amazon S3 — driven end to end over the wire with
//! SageMaker, Amazon S3 and IAM on ONE shared engine, the way the production router mounts every
//! service, so what is asserted is what a client sees.
//!
//! RULE: an accepted batch transform job settles to a terminal `TransformJobStatus` and the only
//! objects its run adds are one `<object>.out` per input object UNDER
//! `TransformOutput.S3OutputPath` — "For every S3 object used as input for the transform job,
//! batch transform stores the transformed data with an .out suffix in a corresponding subfolder
//! in the location in the output prefix" (`TransformOutput$S3OutputPath`). So the objects the
//! input location holds are exactly the ones the caller staged, unchanged, no other location of
//! the bucket gains anything, and a run that could not write its output adds NOTHING at all and
//! settles `Failed` with the reason naming the upload that was refused.
//!
//! This is the invariant a client that keeps its dataset and its results in one bucket depends
//! on: it stages input, submits a transform and then reads the input prefix again, so an `.out`
//! written beside the dataset — or a prefix neither the input nor the output named — corrupts
//! the input of the next run. The completed side of the rule is pinned per output object
//! elsewhere ([`crate::sagemaker_job_execution_over_s3`],
//! [`crate::batch_transform_output_publication`]); what no suite pinned is what the rest of the
//! bucket holds afterwards, which is what this one asserts, over the WHOLE bucket rather than
//! under the output prefix alone.
//!
//! The rows differ along what the rule generalises over: how many input objects the job is given,
//! how deep they sit under the input location, whether the input location names a prefix or one
//! object, whether the output path carries the key delimiter, and — for the failing side — why
//! the output write is withheld (the grant on the input's own bucket, or a bucket the role
//! carries no grant on at all).
//!
//! Every wait here is a poll for the CONDITION — the terminal status is waited for through this
//! crate's one reading of that wait
//! ([`crate::job_lifecycle_settles_for_a_polling_client::transform_ended`]), under the one
//! deadline its job suites bound a wait with, far longer than the span this state gives a
//! transform job's work — so nothing rests on how fast the host runs.
//! The model every job applies is read out of an artifact carrying its own parameters, built
//! with the crate's own archive writer, so no row needs a training run of its own.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    access_denied_detail, model_archive, transform_upload_failed,
};

use super::job_lifecycle_settles_for_a_polling_client::transform_ended;
use super::model_artifact_admission::{str_field, Emulator};
use super::sagemaker_job_execution_over_s3::{
    allow, every_s3_action_on, given_job_role, keys_until, listed_keys, own_image, ROLE,
};

/// The key the model's artifact is staged at in every row's bucket.
const ARTIFACT_KEY: &str = "model/model.tar.gz";

/// The model every job of this suite applies.
const MODEL: &str = "scopemodel";

/// The artifact the model is served out of: a model's own PARAMETERS, read the one way this
/// service reads an artifact that carries them (`state::artifact_from_bytes`), so every row
/// scores its records without a training run of its own.
fn parameter_artifact() -> Vec<u8> {
    let fitted = json!({
        "model": { "type": "linear", "intercept": 1.0, "weights": [2.0], "epochs": 1 },
    })
    .to_string()
    .into_bytes();
    model_archive(&[("model.json".to_string(), fitted)]).expect("the archive is built")
}

/// Hold the role, the bucket, the artifact and the model of one row, and answer the role's ARN.
///
/// `grant` is the Amazon S3 grant the row's execution role carries, so a row narrows what the
/// job may do by handing a narrower document here.
async fn given_model(emulator: &Emulator, case: &str, bucket: &str, grant: &Value) -> String {
    let role = given_job_role(emulator, grant).await;
    emulator.given_bucket(bucket).await;
    let staged = parameter_artifact();
    let (status, answered) = emulator
        .s3("PUT", &format!("/{bucket}/{ARTIFACT_KEY}"), &staged)
        .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the artifact must be stored, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
    emulator
        .sagemaker_ok(
            case,
            "CreateModel",
            &json!({
                "ModelName": MODEL,
                "ExecutionRoleArn": role,
                "PrimaryContainer": { "Image": own_image(), "ModelDataUrl": format!("s3://{bucket}/{ARTIFACT_KEY}") },
            }),
        )
        .await;
    role
}

/// One `CreateTransformJob` request: the job `job` over the input location `input_uri`, writing
/// under `output_path`.
fn transform_request(job: &str, input_uri: &str, output_path: &str) -> Value {
    json!({
        "TransformJobName": job,
        "ModelName": MODEL,
        "TransformInput": {
            "DataSource": { "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": input_uri } },
            "ContentType": "text/csv",
            "SplitType": "Line",
        },
        "TransformOutput": { "S3OutputPath": output_path },
        "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
    })
}

/// One row of the completed table: the input objects the caller stages, the location the job
/// reads, the path it writes under, and the objects its run has to add.
struct Row {
    case: &'static str,
    /// The objects staged in the bucket before the job is submitted, as (key, records).
    staged: &'static [(&'static str, &'static str)],
    /// The key the input location names, under the row's own bucket.
    input_key: &'static str,
    /// The key the output path names, under the row's own bucket.
    output_key: &'static str,
    /// The objects the run adds, as (key, how many records its input held).
    written: &'static [(&'static str, usize)],
}

/// RULE: the only objects an accepted batch transform job's run adds are one `<object>.out` per
/// input object under its output path — the input location keeps exactly what the caller staged,
/// and no other location of the bucket gains anything.
///
/// The rows differ along how many input objects the job is given, how deep they sit under the
/// input location, whether that location names a prefix or one object, and whether the output
/// path carries the key delimiter.
#[tokio::test]
async fn a_completed_transform_job_adds_only_the_out_objects_under_its_output_path() {
    let rows = [
        Row {
            case: "one object under the input prefix",
            staged: &[("input/one.csv", "1\n")],
            input_key: "input/",
            output_key: "scored/",
            written: &[("scored/one.csv.out", 1)],
        },
        Row {
            case: "several objects under the input prefix",
            staged: &[("input/one.csv", "1\n2\n3\n"), ("input/two.csv", "4\n5\n")],
            input_key: "input/",
            output_key: "scored/",
            written: &[("scored/one.csv.out", 3), ("scored/two.csv.out", 2)],
        },
        Row {
            case: "objects in subfolders under the input prefix",
            staged: &[("input/a/part-0", "6\n"), ("input/b/part-0", "7\n8\n")],
            input_key: "input/",
            output_key: "scored/",
            written: &[("scored/a/part-0.out", 1), ("scored/b/part-0.out", 2)],
        },
        Row {
            case: "an input location naming one object beside another",
            staged: &[("input/one.csv", "9\n"), ("input/other.csv", "10\n")],
            input_key: "input/one.csv",
            output_key: "scored/",
            written: &[("scored/one.csv.out", 1)],
        },
        Row {
            case: "an output path carrying no key delimiter",
            staged: &[("input/one.csv", "11\n12\n")],
            input_key: "input/",
            output_key: "scored",
            written: &[("scored/one.csv.out", 2)],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let bucket = format!("transform-scope-{index}");
        let emulator = Emulator::new();
        given_model(&emulator, case, &bucket, &every_s3_action_on(&bucket)).await;
        for (key, records) in row.staged {
            emulator.given_object(&bucket, key, records).await;
        }

        let job = format!("scopejob{index}");
        emulator
            .sagemaker_ok(
                case,
                "CreateTransformJob",
                &transform_request(
                    &job,
                    &format!("s3://{bucket}/{}", row.input_key),
                    &format!("s3://{bucket}/{}", row.output_key),
                ),
            )
            .await;

        let read = transform_ended(&emulator, &job, case).await;
        let status = str_field(&read, "TransformJobStatus");
        assert_eq!(
            status, "Completed",
            "case {case:?}: a job whose run scored its input must complete, got {status} with \
             {read}"
        );
        assert_eq!(
            read.get("FailureReason"),
            None,
            "case {case:?}: a completed job reports no failure reason, got {read}"
        );
        assert!(
            read.get("TransformEndTime").is_some(),
            "case {case:?}: a job that reached a terminal status reports when its work ended, \
             got {read}"
        );

        // Everything the bucket is to hold once the run is done: what the caller staged, and one
        // `.out` per input object the job read. The listing spans the WHOLE bucket, so an object
        // under the input prefix, under the artifact's prefix or under a location neither the
        // input nor the output named fails the case.
        let mut expected: Vec<String> = row
            .staged
            .iter()
            .map(|(key, _)| (*key).to_string())
            .chain(row.written.iter().map(|(key, _)| (*key).to_string()))
            .chain([ARTIFACT_KEY.to_string()])
            .collect();
        expected.sort();
        let held = keys_until(&emulator, case, &bucket, "", &expected).await;
        assert_eq!(
            held, expected,
            "case {case:?}: a completed job's bucket holds what was staged plus one .out per \
             input object and nothing else, got {held:?}"
        );

        // The input objects are the caller's, so the run leaves their bytes alone.
        for (key, records) in row.staged {
            let (read_status, body) = emulator.s3("GET", &format!("/{bucket}/{key}"), &[]).await;
            assert_eq!(
                read_status,
                200,
                "case {case:?}: the staged object {key} must still be served, got {read_status} \
                 {}",
                String::from_utf8_lossy(&body)
            );
            let served = String::from_utf8(body).expect("the staged object is text");
            assert_eq!(
                &served, records,
                "case {case:?}: the run leaves the input object {key} as the caller staged it, \
                 expected {records:?}, got {served:?}"
            );
        }

        // And each object it added carries one prediction per record of its own input.
        for (key, records) in row.written {
            let (read_status, body) = emulator.s3("GET", &format!("/{bucket}/{key}"), &[]).await;
            assert_eq!(
                read_status,
                200,
                "case {case:?}: the object {key} the job reported writing must be served, got \
                 {read_status} {}",
                String::from_utf8_lossy(&body)
            );
            let served = String::from_utf8(body).expect("the scored object is text");
            let predictions: Vec<&str> = served.lines().filter(|line| !line.is_empty()).collect();
            assert_eq!(
                predictions.len(),
                *records,
                "case {case:?}: {key} holds one prediction per input record, expected {records}, \
                 got {predictions:?}"
            );
            for prediction in &predictions {
                assert!(
                    prediction.parse::<f64>().is_ok(),
                    "case {case:?}: every line of {key} is a prediction, got {prediction:?}"
                );
            }
        }
    }
}

/// One row of the withheld-write table: why the output write is refused, and where the output
/// would have gone.
struct DeniedRow {
    case: &'static str,
    /// Whether the output path names a bucket of its own rather than the input's.
    separate_output_bucket: bool,
    /// The key the output path names, under whichever bucket the row writes to.
    output_key: &'static str,
}

/// The one input object every row of the withheld-write table is given, so which upload is
/// refused first is the same whatever the grant.
const DENIED_INPUT: (&str, &str) = ("input/one.csv", "13\n14\n");

/// RULE: a batch transform job whose run may not write its output adds NOTHING — the bucket
/// holds exactly what the caller staged — and settles `Failed` with the reason naming the upload
/// that was refused.
///
/// The rows differ along why the write is withheld: the grant in force on the input's own
/// bucket, and an output bucket the execution role carries no grant on at all.
#[tokio::test]
async fn a_transform_job_whose_output_write_is_withheld_adds_nothing() {
    let rows = [
        DeniedRow {
            case: "the output prefix in the input's own bucket, with the write withheld",
            separate_output_bucket: false,
            output_key: "scored/",
        },
        DeniedRow {
            case: "an output bucket the role carries no grant on",
            separate_output_bucket: true,
            output_key: "scored/",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let bucket = format!("transform-denied-{index}");
        let output_bucket = if row.separate_output_bucket {
            format!("transform-denied-out-{index}")
        } else {
            bucket.clone()
        };
        let emulator = Emulator::new();
        // The input's own bucket is granted every Amazon S3 action a job needs, so the model's
        // artifact and the dataset are readable in both rows; what a row withholds is the WRITE,
        // either by narrowing that grant or by sending the output to a bucket no statement names.
        given_model(&emulator, case, &bucket, &every_s3_action_on(&bucket)).await;
        if row.separate_output_bucket {
            emulator.given_bucket(&output_bucket).await;
        } else {
            emulator
                .given_role_policy(
                    ROLE,
                    &allow(
                        json!(["s3:GetObject", "s3:ListBucket"]),
                        json!([
                            format!("arn:aws:s3:::{bucket}"),
                            format!("arn:aws:s3:::{bucket}/*")
                        ]),
                    ),
                )
                .await;
        }
        let (input_key, records) = DENIED_INPUT;
        emulator.given_object(&bucket, input_key, records).await;

        let job = format!("deniedjob{index}");
        emulator
            .sagemaker_ok(
                case,
                "CreateTransformJob",
                &transform_request(
                    &job,
                    &format!("s3://{bucket}/input/"),
                    &format!("s3://{output_bucket}/{}", row.output_key),
                ),
            )
            .await;

        let read = transform_ended(&emulator, &job, case).await;
        let status = str_field(&read, "TransformJobStatus");
        assert_eq!(
            status, "Failed",
            "case {case:?}: a job that may not write its output must fail, got {status} with \
             {read}"
        );
        let name = input_key.rsplit('/').next().unwrap_or(input_key);
        let expected = transform_upload_failed(
            &format!("s3://{output_bucket}/{}{name}.out", row.output_key),
            &access_denied_detail(),
        );
        let reason = str_field(&read, "FailureReason");
        assert_eq!(
            reason, expected,
            "case {case:?}: the reason must name the upload that was refused, expected \
             {expected:?}, got {reason:?}"
        );

        // Nothing was added anywhere: the run is over — the status says so — so the bucket is
        // listed ONCE, and holds exactly what the caller staged.
        let mut staged = vec![ARTIFACT_KEY.to_string(), input_key.to_string()];
        staged.sort();
        let mut held = listed_keys(&emulator, &bucket, "").await;
        held.sort();
        assert_eq!(
            held, staged,
            "case {case:?}: a job that could not write its output adds nothing, got {held:?}"
        );
        if row.separate_output_bucket {
            let left = listed_keys(&emulator, &output_bucket, "").await;
            assert!(
                left.is_empty(),
                "case {case:?}: the output bucket of a refused upload stays empty, got {left:?}"
            );
        }
    }
}
