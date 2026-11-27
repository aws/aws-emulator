// SPDX-License-Identifier: CC0-1.0

//! WHETHER a client can walk a whole chain of this service's job lifecycles: the window the
//! clock-owed schedule of a job's work fits inside, and the train → model → transform flow a
//! batch-inference client drives through it.
//!
//! RULE — a status transition this emulator owes the CLOCK is due inside the compressed band
//! this project gives such a transition (seconds, not the minutes the real service takes), and
//! that holds for the WHOLE walk of a job's work, stages and all: a client does not
//! wait out one lifecycle in isolation, it chains them — trains a model, hands the artifact to
//! `CreateModel`, transforms data with the model and reads the output — so a schedule laid out
//! on the service's own wall-clock scale leaves such a client with no terminal status to observe
//! inside its own budget at all. The band is a ceiling and not a target: each stage of the work
//! is a status the API reports to a client that POLLS, so it is in effect for a whole span of
//! its own — a share of the walk's, never none of one
//! (`crate::training_job_phase_window` holds that floor) — and a caller that reads straight
//! after its create still finds the job non-terminal.
//!
//! The rows of [`a_job_lays_its_whole_clock_owed_walk_out_inside_a_client_window`] span what the
//! rule generalises over: how many channels the job declares (and so how many stages its work
//! has), and how much work its container STATES it did — none at all, a few seconds, and far
//! more than any client would wait for — which is what the stage that runs the container is in
//! effect for.
//!
//! [`the_whole_train_model_transform_chain_settles_for_a_polling_client`] then drives the chain
//! itself, with the delivery boundary a batch-inference client depends on: the fully permitted
//! role leaves the scored object under the output prefix, and a role that may read the model and
//! the input but may not write the output leaves the job in the modelled `Failed` status with
//! the upload failure named and nothing written.
//!
//! Every claim about a span here is weighed against the instants the JOB itself reports — its
//! own stored schedule, which no host's speed changes — and every wait is a poll for the
//! CONDITION under a deadline far longer than any span this state gives a job's work
//! ([`SETTLE_BUDGET`]), so nothing here is decided by how fast this host ran.
//!
//! Driven over the emulator's own Amazon S3 and IAM mounted beside SageMaker on ONE shared
//! engine, through the real wire, on the harnesses of
//! [`crate::sagemaker_job_execution_over_s3`] and [`crate::model_artifact_admission`] rather
//! than a second copy of them.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::model_artifact_admission::{str_field, trust_for, Emulator};
use super::sagemaker_job_execution_over_s3::{
    allow, channel, every_s3_action_on, given_job_role, keys_until, listed_keys, look_again,
    object_bytes, own_image, SETTLE_BUDGET,
};

/// The bucket every object of this suite lives in.
const BUCKET: &str = "job-lifecycle-window";

/// The service principal a SageMaker execution role's trust document admits.
const SERVICE_PRINCIPAL: &str = service_com_amazonaws_sagemaker_sagemaker::state::SERVICE_PRINCIPAL;

/// The longest ONE clock-owed step of a job's work may be due after the step before it.
///
/// The band this project gives a transition the clock owes, stated here as the ceiling the rule
/// above fixes rather than read from the constant under test — a span read from that constant
/// would hold whatever the constant said. It covers a step wide enough for a polling client to
/// OBSERVE the stage before it over, and, for the step that leaves the stage running the
/// container, the work that container stated besides.
const STEP_CEILING: Duration = Duration::from_secs(15);

/// The longest the WHOLE clock-owed walk of a job's work may take, from the first stage of that
/// work to the terminal status it ends in.
///
/// A client chains these lifecycles, so one job's walk has to leave room for the next: a budget
/// of about a minute — what a client gives such a chain — has to hold a training job, a model and
/// a transform job one after another, each with the client's own poll interval on top, which is
/// what fixes this well inside half a minute.
const WALK_CEILING: Duration = Duration::from_secs(30);

/// The `SecondaryStatus` members a job's work passes through, in the order
/// `DescribeTrainingJob$SecondaryStatus`'s own doc lists them
/// (smithy_query query=doc shape=DescribeTrainingJobResponse member=SecondaryStatus), with the
/// terminal member a job whose work succeeded ends in.
const CANONICAL_ORDER: [&str; 5] = [
    "Starting",
    "Downloading",
    "Training",
    "Uploading",
    "Completed",
];

/// The `TrainingJobStatus` members a job's work has ENDED in
/// (smithy_query query=shape shape=TrainingJobStatus).
const TERMINAL_STATUSES: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// One row of the schedule table.
struct Row {
    case: &'static str,
    /// The name the row's job is created under.
    name: &'static str,
    /// The channels the job declares, as prefixes under [`BUCKET`].
    channels: &'static [&'static str],
    /// How many seconds of work the job's container program states it did, or `None` for a row
    /// that declares no program at all.
    works_for: Option<u64>,
}

/// The table of the RULE: what the walk is laid out over has to stay inside the window whatever
/// the job's work is.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one channel of data and no container program",
            name: "windowonechannel",
            channels: &["one"],
            works_for: None,
        },
        Row {
            case: "two channels of data",
            name: "windowtwochannels",
            channels: &["train", "validation"],
            works_for: None,
        },
        Row {
            case: "no channel at all, so no stage that downloads",
            name: "windownochannel",
            channels: &[],
            works_for: None,
        },
        Row {
            case: "a container that states a few seconds of work",
            name: "windowshortwork",
            channels: &["train"],
            works_for: Some(5),
        },
        Row {
            case: "a container that states far more work than a client would wait for",
            name: "windowlongwork",
            channels: &["train"],
            works_for: Some(1_800),
        },
    ]
}

/// A `CreateTrainingJob` body for `row`: its channels, its output location and the program its
/// container runs when the row states one.
fn training_body(row: &Row, role: &str) -> Value {
    let channels: Vec<Value> = row
        .channels
        .iter()
        .map(|name| channel(name, BUCKET, &format!("{}/{name}/", row.name)))
        .collect();
    let mut body = json!({
        "TrainingJobName": row.name,
        "AlgorithmSpecification": { "TrainingImage": own_image(), "TrainingInputMode": "File" },
        "RoleArn": role,
        "OutputDataConfig": { "S3OutputPath": format!("s3://{BUCKET}/{}/out/", row.name) },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        // Far past the work any row states, so no row is ended by the cap instead of finishing.
        "StoppingCondition": { "MaxRuntimeInSeconds": 86_400 },
    });
    // `CreateTrainingJobRequest$InputDataConfig` is bounded at one channel or more
    // (smithy_query query=shape shape=InputDataConfig), so a job with no channel declares the
    // member not at all.
    if !channels.is_empty() {
        body["InputDataConfig"] = Value::Array(channels);
    }
    if let Some(seconds) = row.works_for {
        body["AlgorithmSpecification"]["ContainerEntrypoint"] = json!(["/bin/sh", "-c"]);
        body["AlgorithmSpecification"]["ContainerArguments"] = json!([format!("sleep {seconds}")]);
    }
    body
}

/// Comma-separated training records: `rows` records whose first column is the target and whose
/// second is the one feature it is a linear function of.
fn training_records(rows: usize) -> String {
    (0..rows)
        .map(|row| format!("{},{row}\n", 3 + row))
        .collect()
}

/// Stage the objects every channel of `row` reads, so the job's run has data to bring down.
async fn given_channel_objects(emulator: &Emulator, row: &Row) {
    for name in row.channels {
        emulator
            .given_object(
                BUCKET,
                &format!("{}/{name}/data.csv", row.name),
                &training_records(16),
            )
            .await;
    }
}

/// One `SecondaryStatusTransitions` entry: the status and the instant the job entered it.
type Stage = (String, f64);

/// The stages a read reports, in the order it reports them.
fn stages(read: &Value, case: &str) -> Vec<Stage> {
    read.get("SecondaryStatusTransitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected SecondaryStatusTransitions in {read}"))
        .iter()
        .map(|entry| {
            let start = entry
                .get("StartTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime in {entry}"));
            (str_field(entry, "Status"), start)
        })
        .collect()
}

/// Poll `DescribeTrainingJob` until the job leaves the non-terminal statuses, answering that
/// read.
async fn ended(emulator: &Emulator, job: &str, case: &str) -> Value {
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        let read = emulator
            .sagemaker_ok(case, "DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await;
        if TERMINAL_STATUSES.contains(&str_field(&read, "TrainingJobStatus").as_str()) {
            return read;
        }
        look_again(
            deadline,
            case,
            &format!("the training job {job} reached a terminal status"),
            &read.to_string(),
        )
        .await;
    }
}

/// Poll `DescribeTransformJob` until the job leaves `InProgress`, answering that read.
///
/// `pub(crate)`: every suite of this crate that waits for a transform job's terminal status
/// waits for it HERE, so the tree holds one reading of that wait.
pub(crate) async fn transform_ended(emulator: &Emulator, job: &str, case: &str) -> Value {
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        let read = emulator
            .sagemaker_ok(
                case,
                "DescribeTransformJob",
                &json!({ "TransformJobName": job }),
            )
            .await;
        if str_field(&read, "TransformJobStatus") != "InProgress" {
            return read;
        }
        look_again(
            deadline,
            case,
            &format!("the transform job {job} reached a terminal status"),
            &read.to_string(),
        )
        .await;
    }
}

/// RULE: the stages of a job's work are laid out inside the window a client can walk — every
/// step inside [`STEP_CEILING`] and the whole walk inside [`WALK_CEILING`] — and none of them is
/// instant, so the job is non-terminal for a caller that reads as soon as its create answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_lays_its_whole_clock_owed_walk_out_inside_a_client_window() {
    let emulator = Emulator::new();
    let role = given_job_role(&emulator, &every_s3_action_on(BUCKET)).await;
    emulator.given_bucket(BUCKET).await;

    let rows = rows();
    for row in &rows {
        let case = row.case;
        given_channel_objects(&emulator, row).await;
        emulator
            .sagemaker_ok(case, "CreateTrainingJob", &training_body(row, &role))
            .await;
        // The span is never zero: the first read a caller can make still finds the job running.
        let first = emulator
            .sagemaker_ok(
                case,
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": row.name }),
            )
            .await;
        assert_eq!(
            str_field(&first, "TrainingJobStatus"),
            "InProgress",
            "case {case:?}: the read a caller makes as its create answers must still find the \
             job running, got {first}"
        );
    }

    for row in &rows {
        let case = row.case;
        let read = ended(&emulator, row.name, case).await;
        assert_eq!(
            str_field(&read, "TrainingJobStatus"),
            "Completed",
            "case {case:?}: a job whose work succeeded completes, got {read}"
        );

        // The walk the job reports: the stage it was accepted in, the stages of its work in the
        // canonical order, and the terminal status last.
        let walked = stages(&read, case);
        let expected: Vec<String> = CANONICAL_ORDER
            .iter()
            .filter(|status| **status != "Downloading" || !row.channels.is_empty())
            .map(|status| (*status).to_string())
            .collect();
        let named: Vec<String> = walked.iter().map(|(status, _)| status.clone()).collect();
        assert_eq!(
            named, expected,
            "case {case:?}: expected the stages {expected:?}, got {named:?} in {read}"
        );

        // Every step of the walk is a step the clock owed, so each of them is due inside the
        // band such a transition is given — and none of them is instant.
        for pair in walked.windows(2) {
            let ((before, entered), (after, next)) = (&pair[0], &pair[1]);
            assert!(
                next > entered,
                "case {case:?}: the {after} stage must come after the {before} stage it \
                 follows, got {next} against {entered} in {read}"
            );
            // The stage the job is ACCEPTED in covers the run this create performed, which is
            // the one step of the walk this host's own speed enters into; every step the CLOCK
            // owes is held to the ceiling.
            if before != "Starting" {
                assert!(
                    next - entered <= STEP_CEILING.as_secs_f64(),
                    "case {case:?}: the {after} stage is owed at most {STEP_CEILING:?} after \
                     the {before} stage, got {} seconds in {read}",
                    next - entered
                );
            }
        }
        let (_, first_of_the_work) = walked
            .get(1)
            .unwrap_or_else(|| panic!("case {case:?}: expected a stage of the work in {read}"));
        let (_, settled_at) = walked
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected a terminal stage in {read}"));
        assert!(
            settled_at - first_of_the_work <= WALK_CEILING.as_secs_f64(),
            "case {case:?}: the whole walk of the job's work must be due inside \
             {WALK_CEILING:?}, got {} seconds in {read}",
            settled_at - first_of_the_work
        );

        // The walk ends in an artifact a client can read, so what settled inside the window is a
        // whole job and not a status alone.
        let artifact = str_field(
            read.get("ModelArtifacts").unwrap_or(&Value::Null),
            "S3ModelArtifacts",
        );
        let key = artifact
            .strip_prefix(&format!("s3://{BUCKET}/"))
            .unwrap_or_else(|| panic!("case {case:?}: the artifact URI names {BUCKET}: {artifact}"))
            .to_string();
        let held = object_bytes(&emulator, case, BUCKET, &key).await;
        assert!(
            !held.is_empty(),
            "case {case:?}: the artifact {key} the job reports must hold bytes"
        );
    }
}

/// RULE: the whole chain a batch-inference client drives — train, hand the artifact to a model,
/// transform data with it, read the output — settles for a client that polls each step, and the
/// delivery boundary holds: the permitted role leaves the scored object, the role that may not
/// write the output leaves none and says why.
#[tokio::test(flavor = "multi_thread")]
async fn the_whole_train_model_transform_chain_settles_for_a_polling_client() {
    let case = "the train, model and transform chain";
    let emulator = Emulator::new();
    let permitted = given_job_role(&emulator, &every_s3_action_on(BUCKET)).await;
    // The restricted role may read the model artifact and the transform input and list the
    // bucket, and may NOT write the output: the boundary the negative control exists for.
    let restricted_name = "transform-reader-role";
    let restricted = emulator
        .given_role(restricted_name, &trust_for(SERVICE_PRINCIPAL))
        .await;
    emulator
        .given_role_policy(
            restricted_name,
            &allow(
                json!(["s3:GetObject", "s3:ListBucket"]),
                json!([
                    format!("arn:aws:s3:::{BUCKET}"),
                    format!("arn:aws:s3:::{BUCKET}/*")
                ]),
            ),
        )
        .await;
    emulator.given_bucket(BUCKET).await;

    // Train the model the transforms apply, so what they score with is parameters a run of this
    // emulator really measured.
    let row = Row {
        case,
        name: "chaintraining",
        channels: &["train"],
        works_for: None,
    };
    given_channel_objects(&emulator, &row).await;
    emulator
        .sagemaker_ok(case, "CreateTrainingJob", &training_body(&row, &permitted))
        .await;
    let trained = ended(&emulator, row.name, case).await;
    assert_eq!(
        str_field(&trained, "TrainingJobStatus"),
        "Completed",
        "case {case:?}: the model the transforms apply must be trained, got {trained}"
    );
    let artifact = str_field(
        trained.get("ModelArtifacts").unwrap_or(&Value::Null),
        "S3ModelArtifacts",
    );
    let artifact_key = artifact
        .strip_prefix(&format!("s3://{BUCKET}/"))
        .unwrap_or_else(|| panic!("case {case:?}: the artifact URI names {BUCKET}: {artifact}"))
        .to_string();
    // What the client hands to `CreateModel` is the OBJECT, so it is read back first.
    object_bytes(&emulator, case, BUCKET, &artifact_key).await;

    // One model per role, both over the artifact the training job wrote.
    for (model, role) in [("chainokmodel", &permitted), ("chaindeniedmodel", &restricted)] {
        emulator
            .sagemaker_ok(
                case,
                "CreateModel",
                &json!({
                    "ModelName": model,
                    "ExecutionRoleArn": role,
                    "PrimaryContainer": { "Image": own_image(), "ModelDataUrl": artifact },
                }),
            )
            .await;
    }

    // The records the transforms score: one feature each, the shape the trained model takes.
    let records: String = (0..3).map(|record| format!("{record}\n")).collect();
    emulator
        .given_object(BUCKET, "chain/in/records.csv", &records)
        .await;
    for (job, model, prefix) in [
        ("chainokjob", "chainokmodel", "transform-out-ok/"),
        (
            "chaindeniedjob",
            "chaindeniedmodel",
            "transform-out-denied/",
        ),
    ] {
        emulator
            .sagemaker_ok(
                case,
                "CreateTransformJob",
                &json!({
                    "TransformJobName": job,
                    "ModelName": model,
                    "TransformInput": {
                        "DataSource": { "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": format!("s3://{BUCKET}/chain/in/"),
                        } },
                        "ContentType": "text/csv",
                        "SplitType": "Line",
                    },
                    "TransformOutput": { "S3OutputPath": format!("s3://{BUCKET}/{prefix}") },
                    "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
                }),
            )
            .await;
    }

    // The positive control: the job completes and the scored object is under its output prefix.
    let permitted_case = format!("{case}: the permitted role");
    let served = transform_ended(&emulator, "chainokjob", &permitted_case).await;
    assert_eq!(
        str_field(&served, "TransformJobStatus"),
        "Completed",
        "case {permitted_case:?}: a job whose role may write its output completes, got {served}"
    );
    let expected = vec!["transform-out-ok/records.csv.out".to_string()];
    let written = keys_until(
        &emulator,
        &permitted_case,
        BUCKET,
        "transform-out-ok/",
        &expected,
    )
    .await;
    assert_eq!(
        written, expected,
        "case {permitted_case:?}: expected one .out per input object, got {written:?}"
    );
    let scored = String::from_utf8(
        object_bytes(
            &emulator,
            &permitted_case,
            BUCKET,
            "transform-out-ok/records.csv.out",
        )
        .await,
    )
    .expect("the scored object is text");
    let predictions: Vec<&str> = scored.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        predictions.len(),
        3,
        "case {permitted_case:?}: the input object holds three records, so the output holds \
         that many predictions, got {predictions:?}"
    );

    // The negative control: the same flow, with the write withheld, reaches the modelled failure
    // and leaves nothing behind.
    let denied_case = format!("{case}: the role that may not write the output");
    let refused = transform_ended(&emulator, "chaindeniedjob", &denied_case).await;
    assert_eq!(
        str_field(&refused, "TransformJobStatus"),
        "Failed",
        "case {denied_case:?}: a job whose role may not write its output fails, got {refused}"
    );
    assert_eq!(
        str_field(&refused, "FailureReason"),
        service_com_amazonaws_sagemaker_sagemaker::state::transform_upload_failed(
            &format!("s3://{BUCKET}/transform-out-denied/records.csv.out"),
            &service_com_amazonaws_sagemaker_sagemaker::state::access_denied_detail(),
        ),
        "case {denied_case:?}: the reason must name the upload that was refused, got {refused}"
    );
    let left = listed_keys(&emulator, BUCKET, "transform-out-denied/").await;
    assert!(
        left.is_empty(),
        "case {denied_case:?}: a job that could not write its output leaves nothing, got {left:?}"
    );
}
