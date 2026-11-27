// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH WIRE a job run's own AWS SDK calls travel on.
//!
//! The rule under test: **a Glue runtime supplies the code it runs the AWS SDK release its
//! `GlueVersion` pins, and an SDK's generation decides which wire its client of a service
//! speaks — so a run on a version from before the AWS SDKs moved the queue service's API onto
//! its JSON protocol reaches that service on the form-encoded AWS Query surface it kept for
//! exactly those clients, and is answered THERE: served calls answered by that surface's XML
//! document, and a refusal worded as that surface words it. A run on a version from after the
//! move speaks the JSON protocol the service's model declares and is answered that surface's
//! own wording.** One API, two surfaces, and the runtime's own SDK decides which one a script's
//! call reaches.
//!
//! The rows span what the rule generalises over: the `GlueVersion` the run is on, across the
//! boundary — the earliest versions, the last one before the move, the first one after it, and
//! the version the service fills in for a job that named none — and each row asserts the WHOLE
//! of what its wire answers rather than the refusal alone:
//!
//! * a SERVED call travels it and is answered: the script's first message reaches the queue, so
//!   the form-encoded request really carried the input;
//! * that answer's own MEMBERS are read back under their own names whatever the wire called
//!   them: the script reports the `MessageId` its send answered, and the test holds it against
//!   the id the queue itself reports for that very message;
//! * the REFUSAL of a queue that is not there carries the one error code and the sentence of
//!   the surface the call arrived on — the two sentences this service exports
//!   ([`service_com_amazonaws_sqs_amazonsqs::state::QUEUE_DOES_NOT_EXIST_MESSAGE`] and
//!   [`service_com_amazonaws_sqs_amazonsqs::state::QUEUE_DOES_NOT_EXIST_IN_THIS_VERSION`]), read
//!   off that service rather than written out here.
//!
//! Amazon S3 and Amazon SQS are mounted BESIDE Glue on the SAME engine, so the script is read
//! from the object store a test staged it in and every call it makes travels the real
//! in-process callback to the real queue service. Every wait is a poll for the CONDITION it
//! waits on against a generous deadline ([`crate::job_runs::RUN_AT_REST_DEADLINE`]); nothing
//! here sleeps for a fixed span and nothing asserts on this host's clock.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::JOB_RUN_STATE_SUCCEEDED;
use service_com_amazonaws_sqs_amazonsqs::state::{
    QUEUE_DOES_NOT_EXIST_CODE, QUEUE_DOES_NOT_EXIST_IN_THIS_VERSION, QUEUE_DOES_NOT_EXIST_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_run_state_change_events::{given_queue, sqs};
use crate::job_runs::{run_at_rest, start_run, RUN_AT_REST_DEADLINE, RUN_POLL_INTERVAL};
use crate::job_script_execution::{create_bucket, stage_script, ROLE_ARN, SCRIPTS};
use crate::wire::{emulator_on, ok};

/// The body the script's first message carries: a call that was SERVED, so the wire it
/// travelled carried its input.
const CONTROL: &str = "control-ok";

/// The script every row runs: it sends one message to the queue its `--QUEUE_URL` names,
/// reports the `MessageId` that send answered, then sends one to the url its
/// `--ABSENT_QUEUE_URL` names and reports the refusal it is given.
///
/// Every one of those calls is made through ONE client, so what the row observes is what the
/// wire that client speaks answered: the served calls and the refused one alike.
const REPORTS_WHAT_THE_QUEUE_SERVICE_ANSWERED: &str = r#"
import sys

import boto3
from botocore.exceptions import ClientError

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
queues = boto3.client("sqs")
answered = queues.send_message(QueueUrl=named["--QUEUE_URL"], MessageBody="control-ok")
queues.send_message(
    QueueUrl=named["--QUEUE_URL"],
    MessageBody="sent:%s" % answered["MessageId"],
)
try:
    queues.send_message(
        QueueUrl=named["--ABSENT_QUEUE_URL"],
        MessageBody="should-not-arrive",
    )
except ClientError as error:
    reported = error.response["Error"]
    queues.send_message(
        QueueUrl=named["--QUEUE_URL"],
        MessageBody="error:%s|%s" % (reported["Code"], reported["Message"]),
    )
"#;

/// An emulator with Glue, Amazon S3 AND Amazon SQS on one shared engine, plus that engine.
///
/// Amazon S3 is mounted because a run READS its script from it; Amazon SQS because every call
/// the script makes is one this suite is about, answered by the real queue service over the
/// emulator's in-process callback.
fn emulator_with_a_queue() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(Arc::clone(&engine));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A url of this account and Region naming a queue that was never created, built by replacing
/// the name in the url of one that was — so the only thing wrong with it is the queue.
fn absent_queue_url(url: &str, name: &str) -> String {
    match url.rsplit_once('/') {
        Some((prefix, _)) => format!("{prefix}/{name}"),
        None => format!("{url}/{name}"),
    }
}

/// Every message the queue at `url` holds, as `(body, MessageId)`, received until the queue
/// has handed over at least `wanted` of them or the deadline passes.
///
/// The wait is a poll for the CONDITION — that the script's messages have arrived — against a
/// generous deadline, because what the row needs is the messages and never how fast the host
/// ran.
async fn received(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<(String, String)> {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    let mut held: Vec<(String, String)> = Vec::new();
    loop {
        let answer = sqs(
            router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        for message in answer["Messages"].as_array().cloned().unwrap_or_default() {
            if let (Some(body), Some(id)) = (
                message["Body"].as_str(),
                message["MessageId"].as_str(),
            ) {
                held.push((body.to_string(), id.to_string()));
            }
            if let Some(handle) = message["ReceiptHandle"].as_str() {
                sqs(
                    router,
                    "DeleteMessage",
                    json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                )
                .await;
            }
        }
        if held.len() >= wanted {
            return held;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case}: the run's script must leave {wanted} messages on {url}, and it has \
             left {held:?}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// Create the Python shell job of one row, on the `GlueVersion` the row is about — or on none
/// at all, which is the row asking for the version the service fills in.
async fn create_job(
    router: &MultiServiceRouter,
    name: &str,
    location: &str,
    glue_version: Option<&str>,
) {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": {
            "Name": "pythonshell",
            "PythonVersion": "3.9",
            "ScriptLocation": location,
        },
        "MaxCapacity": 0.0625,
    });
    if let Some(version) = glue_version {
        body["GlueVersion"] = json!(version);
    }
    ok(router, "CreateJob", &body).await;
}

#[tokio::test]
async fn the_sdk_a_runs_glue_version_pins_decides_the_queue_wire_it_is_answered_on() {
    // (case, the `GlueVersion` the run is on, the sentence that wire words the absence with)
    let rows: [(&str, Option<&str>, &str); 4] = [
        (
            "a version from before the queue API moved to JSON",
            Some("3.0"),
            QUEUE_DOES_NOT_EXIST_IN_THIS_VERSION,
        ),
        (
            "the last version before that move",
            Some("4.0"),
            QUEUE_DOES_NOT_EXIST_IN_THIS_VERSION,
        ),
        (
            "the first version from after it",
            Some("5.0"),
            QUEUE_DOES_NOT_EXIST_MESSAGE,
        ),
        (
            "the version the service fills a job in with",
            None,
            QUEUE_DOES_NOT_EXIST_MESSAGE,
        ),
    ];

    let (router, engine) = emulator_with_a_queue();
    create_bucket(&router).await;

    for (index, (case, glue_version, sentence)) in rows.into_iter().enumerate() {
        let name = format!("queue-wire-{index}");
        let (queue_url, _arn) = given_queue(&router, &format!("queue-wire-queue-{index}")).await;
        let absent = absent_queue_url(&queue_url, &format!("queue-wire-absent-{index}"));
        let location = stage_script(
            &router,
            &format!("{SCRIPTS}{name}.py"),
            REPORTS_WHAT_THE_QUEUE_SERVICE_ANSWERED,
        )
        .await;
        create_job(&router, &name, &location, glue_version).await;

        let run_id = start_run(
            &router,
            &json!({
                "JobName": name,
                "Arguments": {
                    "--QUEUE_URL": queue_url,
                    "--ABSENT_QUEUE_URL": absent,
                },
            }),
        )
        .await;
        let run = run_at_rest(&router, &engine, case, &name, &run_id).await;

        // The script ran to its end: every call it made was answered — the served ones and
        // the refused one — so nothing of the wire it spoke was a failure of the run itself.
        assert_eq!(
            (
                run.get("JobRunState").and_then(Value::as_str),
                run.get("ErrorMessage").and_then(Value::as_str),
            ),
            (Some(JOB_RUN_STATE_SUCCEEDED), None),
            "case {case}: a script whose every call was answered brings its run to rest in \
             {JOB_RUN_STATE_SUCCEEDED} with no ErrorMessage, got {run}"
        );

        let messages = received(&router, case, &queue_url, 3).await;
        let bodies: BTreeSet<String> = messages
            .iter()
            .map(|(body, _)| body.clone())
            .collect();

        // The SERVED call travelled that wire and arrived: the message the script sent first
        // is on the queue, which no amount of refusal wording could produce.
        assert!(
            bodies.contains(CONTROL),
            "case {case}: the script's served call must reach the queue, and it holds {bodies:?}"
        );

        // And that call's ANSWER was read back under the output member's own name: the id the
        // script reported is the id the queue itself reports for the very message it sent.
        let control_id = messages
            .iter()
            .find(|(body, _)| body == CONTROL)
            .map(|(_, id)| id.clone())
            .unwrap_or_default();
        assert!(
            bodies.contains(&format!("sent:{control_id}")),
            "case {case}: the script must report the MessageId its send answered \
             ({control_id:?}), and it reported {bodies:?}"
        );

        // The refusal of a queue that is not there carries the one error code, and the
        // sentence of the SURFACE this run's own SDK reached the service on.
        let expected = format!("error:{QUEUE_DOES_NOT_EXIST_CODE}|{sentence}");
        let reported = bodies
            .iter()
            .find(|body| body.starts_with("error:"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case}: expected the job to observe {expected:?}, got {reported:?} out of \
             {bodies:?}"
        );

        // Nothing was sent to the url that names no queue: the refusal is a refusal.
        assert!(
            !bodies.iter().any(|body| body == "should-not-arrive"),
            "case {case}: the message sent to the absent queue must reach nothing, and the \
             queue holds {bodies:?}"
        );
    }
}
