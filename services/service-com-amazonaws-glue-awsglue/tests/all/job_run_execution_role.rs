// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADMISSION of a job run against the job's execution role.
//!
//! The rule under test: a job run is performed BY Glue acting as the job's `Role` ("the name
//! or Amazon Resource Name (ARN) of the IAM role associated with this job", `Job$Role`), so
//! a run is dispatched only while this account's own IAM shows that Glue may both ASSUME that
//! role and READ, under it, the object the job's `JobCommand$ScriptLocation` names. A run
//! whose role Glue cannot assume comes to rest in the modelled failure state carrying the
//! assume-role requirement for that role; a run whose role may not read the script comes to
//! rest carrying Glue's script-access failure naming that script; and neither run executes
//! anything, so the object its script would have written is not there. The role is resolved
//! BEFORE the script, because a script is fetched as the role.
//!
//! A role nothing can be SHOWN about is not refused — an emulator with no IAM to ask refuses
//! nothing, which is why the other job-run suites in this crate, which mount Glue without
//! IAM, still run scripts under roles no IAM record backs.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: what the
//! role's trust document says (which principal — a service, the account root —, which action,
//! whether the account holds the role at all) and what the policies in force on it grant (the
//! script read, the output write, both, neither, on this bucket or another, named exactly or
//! through a wildcard, through an inline policy or an attached AWS-managed one, with an
//! explicit deny beside an allow).
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness; the scripts are
//! staged through Amazon S3's own `PutObject` and the IAM records are seeded into the SAME
//! shared engine the service reads them from — the store `CreateRole`, `PutRolePolicy` and
//! `AttachRolePolicy` write. The due instant a run's terminal state falls at is moved THROUGH
//! THE STORE rather than slept for, and each run is read back until it HAS come to rest
//! ([`crate::job_runs::run_at_rest`]), so every transition here is deterministic.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    GLUE_SERVICE_PRINCIPAL, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_SUCCEEDED,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    aws_managed_policy_named, name_key, role_arn,
    ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, InlinePolicy, DEFAULT_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::crawler_role_trust::{emulator_with_iam, glue_trust, seed_role, trust};
use crate::job_script_execution::{create_bucket, object_body, stage_script, BUCKET, SCRIPTS};
use crate::job_runs::run_at_rest;
use crate::wire::{ok, ACCOUNT_ID};

/// The fixed text Glue's script-access failure states, written out here rather than read from
/// the crate's own constant so this suite pins the wording.
const SCRIPT_FILE_DOESNT_EXIST: &str = "Script file doesn't exist";

/// The AWS-managed policy a Glue service role is routinely given, which grants
/// `s3:GetObject` only on the buckets AWS Glue itself owns. Named by its friendly name and
/// resolved to the ARN IAM publishes it under, so the attachment is the one
/// `AttachRolePolicy` would store.
const GLUE_SERVICE_ROLE_POLICY: &str = "AWSGlueServiceRole";

/// The script every row stages: it writes a token object at the key its arguments name, so a
/// row can tell a run that EXECUTED from one that never reached the interpreter.
const WRITES_A_TOKEN: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
boto3.client("s3").put_object(
    Bucket=named["--bucket"],
    Key=named["--key"],
    Body=b"ran",
)
"#;

/// An emulator with Glue, IAM AND Amazon S3 on one shared engine, plus that engine.
///
/// IAM is mounted because the role resolution asks it over the emulator's own in-process
/// callback; Amazon S3 because a run READS its script from it and a script WRITES through it.
fn emulator_with_iam_and_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_iam();
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Give the role `role_name` the inline policies `documents` and the attached managed
/// policies `attachments` — the records `PutRolePolicy` and `AttachRolePolicy` write.
fn grant_role(
    engine: &Arc<engine::InMemoryEngine>,
    role_name: &str,
    documents: &[Value],
    attachments: &[String],
) {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let mut policies: BTreeMap<String, InlinePolicy> = BTreeMap::new();
    for (index, document) in documents.iter().enumerate() {
        let policy_name = format!("{role_name}-policy-{index}");
        policies.insert(
            name_key(&policy_name),
            InlinePolicy {
                policy_name,
                policy_document: document.to_string(),
            },
        );
    }
    store
        .role_inline_policies
        .insert(name_key(role_name), policies);
    store.role_attached_policies.insert(
        name_key(role_name),
        attachments.iter().cloned().collect(),
    );
}

/// An identity policy document with one statement: `effect` for `actions` on `resource`.
fn statement(effect: &str, actions: &[&str], resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": actions, "Resource": resource }],
    })
}

/// Everything under this suite's bucket, as a policy names it.
fn every_object() -> String {
    format!("arn:aws:s3:::{BUCKET}/*")
}

/// A trust document admitting the ACCOUNT ROOT rather than a service: the cross-account form
/// a client reaches for, which does not let Glue assume the role.
fn account_root_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root") },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// What a row expects of the run that was admitted, or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The script ran: the run succeeded and the token object is there.
    Ran,
    /// Glue could not assume the role: the run failed carrying the assume-role requirement,
    /// and nothing ran.
    RoleNotAssumable,
    /// Glue could not read the script under the role: the run failed carrying Glue's
    /// script-access failure, and nothing ran.
    ScriptNotAccessible,
}

/// One row: what the account holds for the row's role, and what the run of a job carrying it
/// must come to.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The trust document the account holds for the role — `None` for a row whose role the
    /// account does not hold at all.
    held: Option<Option<String>>,
    /// The inline policy documents in force on the role.
    documents: Vec<Value>,
    /// The AWS-managed policies attached to it, by friendly name.
    attachments: Vec<&'static str>,
    /// What the run must come to.
    expected: Expected,
}

impl Row {
    /// A row whose role the account holds carrying `trust`.
    fn new(case: &'static str, trust_document: String, expected: Expected) -> Self {
        Self {
            case,
            held: Some(Some(trust_document)),
            documents: Vec::new(),
            attachments: Vec::new(),
            expected,
        }
    }

    /// The same row, with `documents` in force on its role.
    fn granting(mut self, documents: Vec<Value>) -> Self {
        self.documents = documents;
        self
    }

    /// The same row, with the AWS-managed policies `names` attached to its role.
    fn attaching(mut self, names: &[&'static str]) -> Self {
        self.attachments = names.to_vec();
        self
    }
}

/// The rows: every form of trust document and every shape of grant, against what the run of a
/// job carrying that role comes to.
fn rows() -> Vec<Row> {
    let read_and_write = statement(
        "Allow",
        &["s3:GetObject", "s3:PutObject"],
        &every_object(),
    );
    vec![
        Row::new(
            "a role Glue can assume, allowed to read the script and write the output",
            glue_trust(),
            Expected::Ran,
        )
        .granting(vec![read_and_write.clone()]),
        Row::new(
            "the same grants through wildcards",
            glue_trust(),
            Expected::Ran,
        )
        .granting(vec![statement("Allow", &["s3:*"], "*")]),
        Row::new(
            "the grants spread over two documents",
            glue_trust(),
            Expected::Ran,
        )
        .granting(vec![
            statement("Allow", &["s3:GetObject"], &every_object()),
            statement("Allow", &["s3:PutObject"], &every_object()),
        ]),
        Row::new(
            "a role that may write the output but not read the script",
            glue_trust(),
            Expected::ScriptNotAccessible,
        )
        .granting(vec![statement("Allow", &["s3:PutObject"], &every_object())]),
        Row::new(
            "a role carrying no policy at all",
            glue_trust(),
            Expected::ScriptNotAccessible,
        ),
        Row::new(
            "a role whose only policy is the AWS-managed Glue service role policy",
            glue_trust(),
            Expected::ScriptNotAccessible,
        )
        .attaching(&[GLUE_SERVICE_ROLE_POLICY]),
        Row::new(
            "a role allowed to read another bucket's objects",
            glue_trust(),
            Expected::ScriptNotAccessible,
        )
        .granting(vec![statement(
            "Allow",
            &["s3:GetObject", "s3:PutObject"],
            "arn:aws:s3:::some-other-bucket/*",
        )]),
        Row::new(
            "a role whose policies deny the script read beside an allow of everything",
            glue_trust(),
            Expected::ScriptNotAccessible,
        )
        .granting(vec![
            statement("Allow", &["s3:*"], "*"),
            statement("Deny", &["s3:GetObject"], &every_object()),
        ]),
        Row::new(
            "a role trusting another service",
            trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole"),
            Expected::RoleNotAssumable,
        )
        .granting(vec![read_and_write.clone()]),
        Row::new(
            "a role trusting the account root rather than Glue",
            account_root_trust(),
            Expected::RoleNotAssumable,
        )
        .granting(vec![read_and_write.clone()]),
        Row::new(
            "a role whose trust document names another action for Glue",
            trust("Allow", GLUE_SERVICE_PRINCIPAL, "s3:GetObject"),
            Expected::RoleNotAssumable,
        )
        .granting(vec![read_and_write.clone()]),
        Row::new(
            "a role that trusts Glue explicitly denying it",
            trust("Deny", GLUE_SERVICE_PRINCIPAL, "sts:AssumeRole"),
            Expected::RoleNotAssumable,
        )
        .granting(vec![read_and_write]),
        // The role is resolved BEFORE the script, so a run that is wrong in both ways
        // reports the role.
        Row {
            case: "a role Glue cannot assume that could not read the script either",
            held: Some(Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole"))),
            documents: Vec::new(),
            attachments: Vec::new(),
            expected: Expected::RoleNotAssumable,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            documents: Vec::new(),
            attachments: Vec::new(),
            expected: Expected::RoleNotAssumable,
        },
    ]
}

/// A run is dispatched only while IAM shows Glue may assume the job's role AND read the job's
/// script under it; otherwise the run comes to rest carrying what it was refused, and nothing
/// of the script runs.
#[tokio::test]
async fn a_job_run_is_dispatched_only_for_a_role_that_can_assume_and_read_the_script() {
    let (router, engine) = emulator_with_iam_and_s3();
    create_bucket(&router).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let role_name = format!("job-run-role-{index}");
        let role = match &row.held {
            Some(trust_document) => seed_role(&engine, &role_name, trust_document.clone()),
            // Not seeded, so IAM answers that this account holds no such role. The ARN is
            // the one IAM would hold it under.
            None => role_arn(DEFAULT_PATH, &role_name),
        };
        let attachments: Vec<String> = row
            .attachments
            .iter()
            .map(|name| {
                aws_managed_policy_named(name)
                    .unwrap_or_else(|| panic!("case {case:?}: IAM must publish {name}"))
                    .arn()
            })
            .collect();
        if row.held.is_some() {
            grant_role(&engine, &role_name, &row.documents, &attachments);
        }

        let job_name = format!("job_run_admission_{index}");
        let script_key = format!("{SCRIPTS}{job_name}.py");
        let token_key = format!("tokens/{job_name}.txt");
        let location = stage_script(&router, &script_key, WRITES_A_TOKEN).await;
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": job_name,
                "Role": role,
                "Command": { "Name": "pythonshell", "ScriptLocation": location },
                "DefaultArguments": { "--bucket": BUCKET, "--key": token_key },
            }),
        )
        .await;

        let started = ok(&router, "StartJobRun", &json!({ "JobName": job_name })).await;
        let run_id = started["JobRunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: StartJobRun must answer a JobRunId"))
            .to_string();
        // The run is read back until it HAS come to rest: whichever call finds a run still
        // owed its script's ONE execution performs it — the start, or a later read when no
        // interpreter was free for the start — and that execution records an owed span of its
        // own, so a single read made after the start's span is forced due can still report a
        // run in flight.
        let run = run_at_rest(&router, &engine, case, &job_name, &run_id).await;
        let state = run["JobRunState"].as_str().unwrap_or_default();
        let message = run["ErrorMessage"].as_str().unwrap_or_default().to_string();

        match row.expected {
            Expected::Ran => {
                assert_eq!(
                    state, JOB_RUN_STATE_SUCCEEDED,
                    "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
                );
                assert!(
                    run.get("ErrorMessage").is_none(),
                    "case {case:?}: a run that succeeded carries no ErrorMessage, got {run}"
                );
                let token = object_body(&router, &token_key).await;
                assert_eq!(
                    token.as_deref(),
                    Some("ran"),
                    "case {case:?}: expected the script's token object, got {token:?}"
                );
            }
            Expected::RoleNotAssumable => {
                assert_eq!(
                    state, JOB_RUN_STATE_FAILED,
                    "case {case:?}: expected {JOB_RUN_STATE_FAILED}, got {run}"
                );
                let expected = format!(
                    "Role {role} should be given assume role permissions for Glue Service."
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected ErrorMessage {expected:?}, got {message:?}"
                );
                let token = object_body(&router, &token_key).await;
                assert!(
                    token.is_none(),
                    "case {case:?}: nothing must have run, but the token object is {token:?}"
                );
            }
            Expected::ScriptNotAccessible => {
                assert_eq!(
                    state, JOB_RUN_STATE_FAILED,
                    "case {case:?}: expected {JOB_RUN_STATE_FAILED}, got {run}"
                );
                let expected = format!("{SCRIPT_FILE_DOESNT_EXIST}: {location}");
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected ErrorMessage {expected:?}, got {message:?}"
                );
                let token = object_body(&router, &token_key).await;
                assert!(
                    token.is_none(),
                    "case {case:?}: nothing must have run, but the token object is {token:?}"
                );
            }
        }
    }
}
