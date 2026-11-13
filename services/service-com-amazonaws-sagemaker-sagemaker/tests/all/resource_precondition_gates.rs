// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the PRECONDITION a SageMaker handler consults before it acts
//! on the resource a request names, over the two families whose gate is a refusal rather
//! than a modelled not-found: `DeleteFeatureGroup`'s `FeatureGroupStatus` gate and
//! `CreatePresignedNotebookInstanceUrl`'s existence check.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! The suites beside this one already pin the gates for one shape of subject each —
//! `service_resolved_members` varies which stores the feature group keeps and
//! `presigned_notebook_instance_url` checks that the refusal mentions the instance ARN.
//! What these tests assert is the VARIABLE PARTS of the two refusals, over a table of
//! identifiers spanning each member's modelled constraint: the sentence names the
//! identifier the REQUEST carried and the state the EMULATOR holds, and the refused call
//! leaves the resource — or the absence of one — exactly as it was.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::notebook_instance_lifecycle::{create_body as notebook_body, PAST_IN_SERVICE_DUE};

/// Longer than `state::FEATURE_GROUP_CREATED_AFTER` (300 ms), so the clock owes a created
/// group its `Created` status by the time the next call runs.
const PAST_CREATED_DUE: std::time::Duration = std::time::Duration::from_millis(600);

/// A valid `CreateFeatureGroup` body for `name`: the four members the operation marks
/// required plus an OnlineStore, built here rather than imported so this suite's rows are
/// independent of the ones `feature_group_deletion` builds.
fn feature_group_body(name: &str) -> Value {
    json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": "customer-id",
        "EventTimeFeatureName": "event-time",
        "FeatureDefinitions": [
            { "FeatureName": "customer-id", "FeatureType": "String" },
            { "FeatureName": "event-time", "FeatureType": "String" },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    })
}

/// How SageMaker words a refusal to delete a feature group that is not provisioned yet.
///
/// Spelled out here rather than imported from the crate, so the wording itself is under
/// test: the fixed text, the `FeatureGroupName` the request carried, and the
/// `FeatureGroupStatus` the group is in — which this message spells in upper case.
fn not_deletable_sentence(name: &str, status: &str) -> String {
    format!(
        "Validation Error: FeatureGroup with name {name} cannot be deleted, because \
         FeatureGroupStatus is '{status}'."
    )
}

/// How SageMaker's presigned-URL call words a notebook instance this account does not
/// hold. Spelled out here for the same reason: the fixed text plus the ARN the request's
/// `NotebookInstanceName` resolves to, and NO closing period.
fn instance_absent_sentence(arn: &str) -> String {
    format!("Notebook Instance {arn} does not exist")
}

/// The ARN a `NotebookInstanceName` resolves to against the region the request signed for.
fn notebook_instance_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance/{name}")
}

/// `DescribeFeatureGroup` for `name`, requiring a 200.
async fn describe_feature_group(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("DescribeFeatureGroup", &json!({ "FeatureGroupName": name }))
        .await
}

/// RULE: a feature group is deletable only once it EXISTS — while the clock still owes its
/// provisioning, `DeleteFeatureGroup` refuses with HTTP 400 `ValidationException` whose
/// sentence names the `FeatureGroupName` the REQUEST carried and the `FeatureGroupStatus`
/// the group is in, writes nothing, and accepts the same call once the group has settled.
#[tokio::test]
async fn a_delete_refused_by_the_status_gate_names_the_request_and_the_stored_status() {
    // Rows differ along the name — the part of the sentence the request supplies — across
    // what `FeatureGroupName` admits: `len 1..64`, pattern
    // `^[a-zA-Z0-9]([_-]*[a-zA-Z0-9]){0,63}$` (smithy_query operation=DeleteFeatureGroup),
    // so each published bound of the length and each separator the pattern allows is a
    // row. Every row satisfies the constraint, so the refusal comes from the handler
    // rather than from boundary validation.
    let longest = format!("z{}", "y".repeat(63));
    let cases: Vec<(&str, String)> = vec![
        ("the shortest name the pattern admits", "f".to_string()),
        ("digits only", "20261031".to_string()),
        ("underscores between alphanumerics", "customer_features_v2".to_string()),
        ("hyphens between alphanumerics", "customer-features-v2".to_string()),
        ("mixed case", "CustomerFeatures".to_string()),
        ("alternating separators", "a_b-c_d".to_string()),
        ("the longest name the pattern admits", longest),
    ];

    // Each row gets its own store, and its delete is the very next call after its create:
    // the `Creating` window is `state::FEATURE_GROUP_CREATED_AFTER` and nothing may settle
    // it before the call under test, so the status in the sentence is the only one a
    // freshly created group can be in.
    for (case, name) in &cases {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call("CreateFeatureGroup", &feature_group_body(name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateFeatureGroup, got {status} with \
             {created}"
        );
        let arn = str_field(&created, "FeatureGroupArn");

        let (status, refused) = emulator
            .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": name }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException from the status gate, got \
             {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            not_deletable_sentence(name, "CREATING"),
            "case {case:?}: the refusal must name the requested name and the stored \
             status, got {refused}"
        );

        // The refused delete wrote nothing: the group is still served under the same name
        // and ARN, still replays what the create stored, and no deletion was started for
        // it — whether or not the clock has settled its provisioning by now.
        let after = describe_feature_group(&emulator, name).await;
        assert_eq!(
            (
                str_field(&after, "FeatureGroupName"),
                str_field(&after, "FeatureGroupArn"),
                str_field(&after, "RecordIdentifierFeatureName"),
            ),
            (name.clone(), arn, "customer-id".to_string()),
            "case {case:?}: the group must survive the refusal unchanged, got {after}"
        );
        assert_ne!(
            str_field(&after, "FeatureGroupStatus"),
            "Deleting",
            "case {case:?}: a refused delete must start no deletion, got {after}"
        );
    }

    // The gate is keyed on the STATUS, not on the name: once the clock has settled the
    // provisioning, the very same requests are accepted — proved on one store holding
    // every row at once, so each delete has to pick out the group its own request names.
    let emulator = Emulator::new();
    for (case, name) in &cases {
        let (status, created) = emulator
            .call("CreateFeatureGroup", &feature_group_body(name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateFeatureGroup, got {status} with \
             {created}"
        );
    }
    tokio::time::sleep(PAST_CREATED_DUE).await;
    for (case, name) in &cases {
        assert_eq!(
            str_field(&describe_feature_group(&emulator, name).await, "FeatureGroupStatus"),
            "Created",
            "case {case:?}: the group must settle to Created once the clock owes it"
        );
        let (status, deleted) = emulator
            .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a settled group is deletable, got {status} with {deleted}"
        );
        assert_eq!(
            str_field(&describe_feature_group(&emulator, name).await, "FeatureGroupStatus"),
            "Deleting",
            "case {case:?}: the accepted delete must move the group to Deleting"
        );
    }
}

/// RULE: `CreatePresignedNotebookInstanceUrl` mints nothing for a notebook instance this
/// account does not hold — it refuses with HTTP 400 `ValidationException` whose sentence
/// names the ARN the request's `NotebookInstanceName` resolves to against the signed
/// region — and serves the identical request once the instance exists and is serving
/// Jupyter.
#[tokio::test]
async fn a_presigned_url_for_an_instance_that_does_not_exist_is_refused_by_its_arn() {
    // Rows differ along the two parts of the ARN the request supplies — the name's shape
    // across `NotebookInstanceName`'s modelled `len 0..63` and pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, and the signed region — plus whether the request
    // names a `SessionExpirationDurationInSeconds` at all, which the gate precedes
    // (smithy_query operation=CreatePresignedNotebookInstanceUrl).
    let longest = format!("z{}", "y".repeat(62));
    let cases: Vec<(&str, &str, String, Value)> = vec![
        (
            "the shortest name the pattern admits",
            "us-east-1",
            "n".to_string(),
            json!({}),
        ),
        (
            "hyphens between alphanumerics",
            "eu-west-1",
            "team-notebook-2".to_string(),
            json!({}),
        ),
        (
            "digits only, with a session duration",
            "ap-southeast-2",
            "20261031".to_string(),
            json!({ "SessionExpirationDurationInSeconds": 1800 }),
        ),
        (
            "mixed case, with a session duration",
            "us-west-2",
            "TeamNotebook".to_string(),
            json!({ "SessionExpirationDurationInSeconds": 43200 }),
        ),
        (
            "the longest name the pattern admits",
            "eu-central-1",
            longest,
            json!({}),
        ),
    ];

    for (case, region, name, extra) in &cases {
        let emulator = Emulator::new();
        let mut request = json!({ "NotebookInstanceName": name });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            request[key.as_str()] = value.clone();
        }

        let (status, refused) = emulator
            .call_in(region, "CreatePresignedNotebookInstanceUrl", &request)
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for an instance this account \
             holds none of, got {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            instance_absent_sentence(&notebook_instance_arn(region, name)),
            "case {case:?}: the refusal must name the ARN the request resolves to, got \
             {refused}"
        );
        assert!(
            refused.get("AuthorizedUrl").is_none(),
            "case {case:?}: a refused call mints no URL, got {refused}"
        );

        // The gate is keyed on the instance's EXISTENCE, not on the name or the region:
        // the identical request is served once the instance is there and serving Jupyter.
        let (status, created) = emulator
            .call_in(region, "CreateNotebookInstance", &notebook_body(name, &json!({})))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateNotebookInstance, got {status} \
             with {created}"
        );
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

        let (status, minted) = emulator
            .call_in(region, "CreatePresignedNotebookInstanceUrl", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 once the instance exists, got {status} with \
             {minted}"
        );
        let url = str_field(&minted, "AuthorizedUrl");
        // The URL leads back to THIS emulator (the session address, whose own suite is
        // `tests/all/presigned_session.rs`), naming the instance it was minted for.
        assert!(
            url.starts_with(&format!(
                "{}/_aws/sagemaker/notebook-instance/{name}/auth?authToken=",
                smithy_server_runtime::UNSERVED_URL_BASE
            )),
            "case {case:?}: the URL must address this emulator's session path for the \
             instance, got {url}"
        );
    }
}
