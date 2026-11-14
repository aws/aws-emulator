// SPDX-License-Identifier: CC0-1.0

//! The certificate-authority enrollment policy of a directory —
//! `EnableCAEnrollmentPolicy`, `DescribeCAEnrollmentPolicy` and
//! `DisableCAEnrollmentPolicy` — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing a RESPONSE claims is read from the
//! state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! Enrollment is performed by a PCA connector, which belongs to the AWS Private CA
//! Connector for Active Directory service, so that service is mounted beside this one on
//! the same engine and every connector a case enrolls through is one IT minted and reports
//! from its own read — a connector this suite invented would prove nothing about a policy
//! that has to be backed by one.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a directory's CA enrollment policy is reported from the store: `Disabled` with no
//!   connector and no modification instant until an enable records one, the transient
//!   `InProgress` while the transition that enable owes is outstanding, and `Success`
//!   once the clock has settled it — then `Disabling` and `Disabled` the same way for the
//!   withdrawal — over a table of directories differing in type, with an untouched
//!   directory in the same store as the control;
//! * an enable needs the connector it names to be one the service that OWNS connectors
//!   holds, created for THIS directory and in an active state: a connector nothing minted
//!   is refused with `EntityDoesNotExistException`, and one of another directory or one
//!   still being created with `InvalidParameterException`, over a table of connector forms;
//! * each toggle is refused on the state the directory's own policy is in: a second
//!   enable while the activation is outstanding with `EnableAlreadyInProgressException`
//!   and once it is in force with `EntityAlreadyExistsException`; a disable with no
//!   policy to withdraw with `EntityDoesNotExistException` and a second one while the
//!   withdrawal is outstanding with `DisableAlreadyInProgressException`;
//! * both writes need a directory that is in service, so one still being built by the
//!   transition its create owes is refused with `DirectoryUnavailableException` — before
//!   the connector is looked at at all — while the read answers throughout;
//! * all three operations resolve the `DirectoryId` in the store and refuse an id no
//!   directory is stored under with `DirectoryDoesNotExistException` — over a table of
//!   well-formed ids differing in digits and form — while an id the MODEL refuses is
//!   refused before that resolution happens at all, and none of the three ever answers
//!   the scaffold's `NotImplemented`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::ops::types::CA_ENROLLMENT_POLICY_STATUS_VALUES;
use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use service_com_amazonaws_pcaconnectorad_pcaconnectorad as pcad;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::minimal_create;
use crate::harness::{call, ok, refused, string};
use crate::microsoft_ad::minimal_microsoft_ad;

/// The status a directory no enable has ever named carries, and the one a withdrawal
/// settles on: the `Disabled` value of the modelled `CaEnrollmentPolicyStatus` enum, whose
/// whole catalog the crate generates from the model as
/// [`CA_ENROLLMENT_POLICY_STATUS_VALUES`].
const DISABLED: &str = "Disabled";

/// The status a policy carries between the enable and the transition it owes.
const IN_PROGRESS: &str = "InProgress";

/// The status a policy carries once that transition has fallen due.
const SUCCESS: &str = "Success";

/// The status a policy carries between the disable and the transition it owes.
const DISABLING: &str = "Disabling";

/// How long a polled status may take to advance before the case fails: far longer than any
/// span either service publishes, and well inside what a client's waiter allows. Nothing is
/// asserted about HOW LONG a transition took — only that it converges — so a loaded or a
/// fast machine reaches the same verdict.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How often a poll asks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The Region the Directory Service harness signs for, which the connector calls below sign
/// for too, so a minted connector ARN carries the Region the directory's calls name.
const REGION: &str = "us-east-1";

/// A private CA the connectors below join their directory to: `CertificateAuthorityArn` is
/// `arn:aws:acm-pca:…:certificate-authority/<uuid>`
/// (`smithy_query(query="operation",operation="CreateConnector",model_path="services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json")`).
const CERTIFICATE_AUTHORITY: &str =
    "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// Well-formed `PcaConnectorArn`s no connector is ever minted under, as `(arn, class)`.
///
/// The form comes from the model: `PcaConnectorArn` is
/// `^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$`
/// (`smithy_query(query="operation", operation="EnableCAEnrollmentPolicy")`), so the rows
/// are that shape differing along every segment it leaves open — partition, Region, account
/// id and connector id.
const ABSENT_CONNECTORS: [(&str, &str); 4] = [
    (
        "arn:aws:pca-connector-ad:us-east-1:000000000000:connector/01234567-89ab-cdef-0123-456789abcdef",
        "the commercial partition",
    ),
    (
        "arn:aws-us-gov:pca-connector-ad:us-gov-west-1:123456789012:connector/abcdef01-2345-6789-abcd-ef0123456789",
        "another partition and Region",
    ),
    (
        "arn:aws-cn:pca-connector-ad:cn-north-1:999999999999:connector/ffffffff-ffff-ffff-ffff-ffffffffffff",
        "the high end of the connector-id alphabet",
    ),
    (
        "arn:aws:pca-connector-ad:eu-west-3:1:connector/00000000-0000-0000-0000-000000000000",
        "the low end of every open segment",
    ),
];

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// `DirectoryId` is `^d-[0-9a-f]{10}$` (the same `smithy_query` call), so every row is a
/// value the boundary validator admits and the handler must resolve in the store.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-9f8e7d6c5b", "digits and letters interleaved"),
];

/// A router with the Directory Service control plane and the service that owns PCA
/// connectors mounted on ONE engine, so a connector a case enrolls through is one the
/// owning service minted and reports.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The cases exercise the services, not SigV4: requests carry a credential scope so the
    // router can name the target service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(pcad::EmulatedService::new(engine));
    router
}

/// One REST-JSON call to the service that owns connectors, returning the HTTP status and
/// the parsed body.
async fn connector_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://pca-connector-ad.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("pca-connector-ad.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDSTEST/20260101/{REGION}/pca-connector-ad/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode an ARN the way an AWS SDK encodes a non-greedy `@httpLabel`: encoding
/// every non-alphanumeric byte is a superset of what a label needs and covers the `:` and
/// `/` of an ARN, so the whole ARN travels inside ONE path segment. The encoding itself is
/// the `percent-encoding` crate's.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Mint a connector FOR `directory_id` through the service that owns connectors, and answer
/// its ARN: the directory is registered with that service first, which is the prerequisite
/// its `CreateConnector` resolves.
async fn create_connector(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> String {
    let (status, parsed) = connector_call(
        router,
        "POST",
        "/directoryRegistrations",
        Some(json!({
            "DirectoryId": directory_id,
            "ClientToken": format!("register-{directory_id}"),
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateDirectoryRegistration must answer 202, got {status} with \
         {parsed}"
    );

    let (status, parsed) = connector_call(
        router,
        "POST",
        "/connectors",
        Some(json!({
            "DirectoryId": directory_id,
            "CertificateAuthorityArn": CERTIFICATE_AUTHORITY,
            "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateConnector must answer 202, got {status} with {parsed}"
    );
    parsed["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no ConnectorArn: {parsed}"))
        .to_string()
}

/// Poll the owning service's own read until the connector is in service, so a case about
/// the enable is not racing the activation that service owes.
async fn await_connector_active(router: &MultiServiceRouter, connector_arn: &str, case: &str) {
    let started = Instant::now();
    loop {
        let (status, parsed) = connector_call(
            router,
            "GET",
            &format!("/connectors/{}", encode_label(connector_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConnector must answer 200, got {status} with {parsed}"
        );
        let connector_status = parsed["Connector"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if connector_status == "ACTIVE" {
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected {connector_arn} to be in service within \
             {POLL_DEADLINE:?}, last observed {connector_status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A connector of `directory_id` that is ready to enroll: minted by the owning service and
/// polled there until it is in service.
async fn active_connector(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> String {
    let arn = create_connector(router, directory_id, case).await;
    await_connector_active(router, &arn, case).await;
    arn
}

/// One row of the directory table: how the directory is created, and whether this case
/// enrolls it at all.
struct PolicyCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    enrolled: bool,
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &PolicyCase) -> String {
    let answer = ok(router, row.operation, row.body.clone(), row.case).await;
    string(&answer, "DirectoryId", row.case)
}

/// The whole `DescribeCAEnrollmentPolicy` answer for `directory_id`, with the members every
/// answer must carry checked: the directory it is about, and a status the model declares.
async fn describe_policy(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Value {
    let answer = ok(
        router,
        "DescribeCAEnrollmentPolicy",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    let reported = string(&answer, "DirectoryId", case);
    assert_eq!(
        reported, directory_id,
        "case {case:?}: expected the answer to name the directory it is about, got \
         {reported:?}"
    );
    let status = string(&answer, "CaEnrollmentPolicyStatus", case);
    assert!(
        CA_ENROLLMENT_POLICY_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: expected a modelled CaEnrollmentPolicyStatus one of \
         {CA_ENROLLMENT_POLICY_STATUS_VALUES:?}, got {status:?}"
    );
    answer
}

/// The `CaEnrollmentPolicyStatus` reported for `directory_id`.
async fn policy_status(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
    let answer = describe_policy(router, directory_id, case).await;
    string(&answer, "CaEnrollmentPolicyStatus", case)
}

/// Poll the read until the status reported for `directory_id` is `expected`, and answer the
/// body that reported it; fail naming the case and the last status observed if a generous
/// deadline passes first.
///
/// A wait for the CONDITION rather than for a duration: the clock settles the transition a
/// toggle owed on its own, and this looks again until it has.
async fn await_policy_status(
    router: &MultiServiceRouter,
    directory_id: &str,
    expected: &str,
    case: &str,
) -> Value {
    let started = Instant::now();
    loop {
        let answer = describe_policy(router, directory_id, case).await;
        let status = string(&answer, "CaEnrollmentPolicyStatus", case);
        if status == expected {
            return answer;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected the policy of {directory_id} to reach {expected} \
             within {POLL_DEADLINE:?}, last observed {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `DescribeDirectories` until `directory_id` is in service, so a case about a write
/// that needs a directory in service is not racing the build chain its create owes.
async fn await_directory_in_service(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let started = Instant::now();
    loop {
        let answer = ok(
            router,
            "DescribeDirectories",
            json!({ "DirectoryIds": [directory_id] }),
            case,
        )
        .await;
        let stage = answer["DirectoryDescriptions"][0]["Stage"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if stage == "Active" {
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected {directory_id} to be in service within \
             {POLL_DEADLINE:?}, last observed stage {stage:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `LastUpdatedDateTime` of an answer, as the epoch seconds the `awsJson1_1` protocol
/// carries a timestamp as.
fn last_updated(answer: &Value, case: &str) -> f64 {
    answer["LastUpdatedDateTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LastUpdatedDateTime, got {answer}"))
}

/// The directory table the status rules are asserted over: both creates, and one directory
/// nothing enrolls.
fn policy_rows() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "a Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("simple.example.com"),
            enrolled: true,
        },
        PolicyCase {
            case: "a Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("corp.example.com"),
            enrolled: true,
        },
        PolicyCase {
            case: "a second Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("other.example.com"),
            enrolled: true,
        },
        PolicyCase {
            case: "the control directory, never enrolled",
            operation: "CreateDirectory",
            body: minimal_create("control.example.com"),
            enrolled: false,
        },
    ]
}

#[tokio::test]
async fn the_policy_reported_is_the_one_the_toggles_recorded_for_that_directory() {
    let router = emulator();
    let rows = policy_rows();

    let mut ids: Vec<(&PolicyCase, String)> = Vec::new();
    for row in &rows {
        ids.push((row, create(&router, row).await));
    }

    // Every directory is readable while it is still being built, and a directory nothing
    // has enrolled reports the inactive status with no connector and no modification
    // instant: there is nothing configured to report.
    for (row, id) in &ids {
        let answer = describe_policy(&router, id, row.case).await;
        let status = string(&answer, "CaEnrollmentPolicyStatus", row.case);
        assert_eq!(
            status, DISABLED,
            "case {:?}: a directory nothing has enrolled reports {DISABLED}, got {status:?}",
            row.case
        );
        assert!(
            answer["PcaConnectorArn"].is_null(),
            "case {:?}: expected no PcaConnectorArn before an enable, got {answer}",
            row.case
        );
        assert!(
            answer["LastUpdatedDateTime"].is_null(),
            "case {:?}: expected no LastUpdatedDateTime before an enable, got {answer}",
            row.case
        );
    }

    // The writes need a directory in service, so the build chain each create owes is
    // waited out by polling the directory itself; each enrolled row then gets a connector
    // of its own from the service that owns connectors.
    let mut connectors: Vec<(&PolicyCase, String, Option<String>)> = Vec::new();
    for (row, id) in &ids {
        await_directory_in_service(&router, id, row.case).await;
        let connector = if row.enrolled {
            Some(active_connector(&router, id, row.case).await)
        } else {
            None
        };
        connectors.push((row, id.clone(), connector));
    }

    for (row, id, connector) in &connectors {
        let Some(connector) = connector else {
            continue;
        };
        let answer = ok(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": id, "PcaConnectorArn": connector }),
            row.case,
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {:?}: EnableCAEnrollmentPolicy has no modelled output members, got \
             {answer}",
            row.case
        );
    }

    // The instant after the enable the transition it owes is still outstanding, so the
    // transient status is observable — and the control directory is untouched, so the
    // status is per-directory rather than per-store.
    for (row, id, connector) in &connectors {
        let answer = describe_policy(&router, id, row.case).await;
        let status = string(&answer, "CaEnrollmentPolicyStatus", row.case);
        let expected = if connector.is_some() {
            IN_PROGRESS
        } else {
            DISABLED
        };
        assert_eq!(
            status, expected,
            "case {:?}: straight after the enable the directory reports {expected}, got \
             {status:?}",
            row.case
        );
        // The connector the enable named is the one reported back, from the moment the
        // activation starts.
        match connector {
            Some(connector) => {
                let reported = string(&answer, "PcaConnectorArn", row.case);
                assert_eq!(
                    &reported, connector,
                    "case {:?}: expected the connector the enable named, got {reported:?}",
                    row.case
                );
            }
            None => assert!(
                answer["PcaConnectorArn"].is_null(),
                "case {:?}: the control directory names no connector, got {answer}",
                row.case
            ),
        }
    }

    // A second enable while that transition is outstanding, and one once the policy is in
    // force, are told apart by the modelled error each reports.
    for (row, id, connector) in &connectors {
        let Some(connector) = connector else {
            continue;
        };
        refused(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": id, "PcaConnectorArn": connector }),
            "EnableAlreadyInProgressException",
            row.case,
        )
        .await;
    }

    for (row, id, connector) in &connectors {
        let Some(connector) = connector else {
            // Nothing is owed for a directory nothing enrolled: it stays inactive.
            let status = policy_status(&router, id, row.case).await;
            assert_eq!(
                status, DISABLED,
                "case {:?}: the control directory stays {DISABLED}, got {status:?}",
                row.case
            );
            continue;
        };

        // The clock settles the transition the enable owed; nothing else did.
        let settled = await_policy_status(&router, id, SUCCESS, row.case).await;
        let reported = string(&settled, "PcaConnectorArn", row.case);
        assert_eq!(
            &reported, connector,
            "case {:?}: expected the settled policy to keep its connector, got {reported:?}",
            row.case
        );
        let stamped = last_updated(&settled, row.case);

        // Repeating the read does not move the status on, and does not restamp the
        // policy: a report is not a mutation.
        let again = describe_policy(&router, id, row.case).await;
        let status = string(&again, "CaEnrollmentPolicyStatus", row.case);
        assert_eq!(
            status, SUCCESS,
            "case {:?}: a repeated read reports {SUCCESS}, got {status:?}",
            row.case
        );
        let restamped = last_updated(&again, row.case);
        assert_eq!(
            restamped, stamped,
            "case {:?}: expected the modification instant to be unchanged by a read, got \
             {restamped} after {stamped}",
            row.case
        );

        // An enable on a directory whose policy is in force meets an entity that is
        // already there.
        refused(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": id, "PcaConnectorArn": connector }),
            "EntityAlreadyExistsException",
            row.case,
        )
        .await;

        // The withdrawal answers in its own transient status and settles the same way.
        ok(
            &router,
            "DisableCAEnrollmentPolicy",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        let status = policy_status(&router, id, row.case).await;
        assert_eq!(
            status, DISABLING,
            "case {:?}: straight after the disable the directory reports {DISABLING}, got \
             {status:?}",
            row.case
        );
        refused(
            &router,
            "DisableCAEnrollmentPolicy",
            json!({ "DirectoryId": id }),
            "DisableAlreadyInProgressException",
            row.case,
        )
        .await;
        let withdrawn = await_policy_status(&router, id, DISABLED, row.case).await;
        let settled_at = last_updated(&withdrawn, row.case);
        assert!(
            settled_at >= stamped,
            "case {:?}: expected the withdrawal to be the policy's latest modification, \
             got {settled_at} after {stamped}",
            row.case
        );

        // Nothing is left to withdraw, and an enable is admitted again — through ANOTHER
        // connector of the same directory, which is then the one reported.
        refused(
            &router,
            "DisableCAEnrollmentPolicy",
            json!({ "DirectoryId": id }),
            "EntityDoesNotExistException",
            row.case,
        )
        .await;
        let replacement = active_connector(&router, id, row.case).await;
        assert_ne!(
            &replacement, connector,
            "case {:?}: expected the owning service to mint a second connector, got the \
             first one again",
            row.case
        );
        ok(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": id, "PcaConnectorArn": &replacement }),
            row.case,
        )
        .await;
        let reenabled = await_policy_status(&router, id, SUCCESS, row.case).await;
        let reported = string(&reenabled, "PcaConnectorArn", row.case);
        assert_eq!(
            reported, replacement,
            "case {:?}: expected the connector the second enable named, got {reported:?}",
            row.case
        );
    }

    // The control directory never held a policy, so there is nothing to withdraw from it
    // — while every other directory in the same store has been through the whole toggle.
    for (row, id, connector) in &connectors {
        if connector.is_some() {
            continue;
        }
        refused(
            &router,
            "DisableCAEnrollmentPolicy",
            json!({ "DirectoryId": id }),
            "EntityDoesNotExistException",
            row.case,
        )
        .await;
    }
}

#[tokio::test]
async fn an_enable_needs_a_connector_the_owning_service_holds_for_this_directory() {
    let router = emulator();

    let subject = create(
        &router,
        &PolicyCase {
            case: "the directory being enrolled",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("enrolled.example.com"),
            enrolled: true,
        },
    )
    .await;
    let other = create(
        &router,
        &PolicyCase {
            case: "another directory with a connector of its own",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("neighbour.example.com"),
            enrolled: true,
        },
    )
    .await;
    for id in [&subject, &other] {
        await_directory_in_service(&router, id, "the enrolled directories").await;
    }

    // A connector nothing ever minted: every well-formed form of the ARN meets the same
    // absent entity, and the diagnostic is built from the request — it names the connector
    // the call sent, so a caller can tell which value to fix.
    for (absent, class) in ABSENT_CONNECTORS {
        let (status, parsed) = call(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": &subject, "PcaConnectorArn": absent }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {class:?}: expected the enable to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {class:?}: expected EntityDoesNotExistException, got {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(absent),
            "case {class:?}: expected the refusal to name the connector the request sent, \
             got {message:?}"
        );
        let status = policy_status(&router, &subject, class).await;
        assert_eq!(
            status, DISABLED,
            "case {class:?}: a refused enable records nothing, got {status:?}"
        );
    }

    // A connector the owning service holds for ANOTHER directory enrolls for that one, so
    // it is not one this directory can enroll through.
    let neighbours = active_connector(&router, &other, "another directory's connector").await;
    refused(
        &router,
        "EnableCAEnrollmentPolicy",
        json!({ "DirectoryId": &subject, "PcaConnectorArn": &neighbours }),
        "InvalidParameterException",
        "another directory's connector",
    )
    .await;

    // A connector of this directory that the owning service has not brought into service
    // yet cannot enroll either: the enable needs it active.
    let creating = create_connector(&router, &subject, "a connector still being created").await;
    refused(
        &router,
        "EnableCAEnrollmentPolicy",
        json!({ "DirectoryId": &subject, "PcaConnectorArn": &creating }),
        "InvalidParameterException",
        "a connector still being created",
    )
    .await;
    let status = policy_status(&router, &subject, "a connector still being created").await;
    assert_eq!(
        status, DISABLED,
        "case {:?}: a refused enable records nothing, got {status:?}",
        "a connector still being created"
    );

    // The very same connector is admitted once the owning service reports it in service,
    // so the refusal was about its state and nothing else.
    await_connector_active(&router, &creating, "a connector still being created").await;
    ok(
        &router,
        "EnableCAEnrollmentPolicy",
        json!({ "DirectoryId": &subject, "PcaConnectorArn": &creating }),
        "a connector brought into service",
    )
    .await;
    let settled = await_policy_status(
        &router,
        &subject,
        SUCCESS,
        "a connector brought into service",
    )
    .await;
    let reported = string(&settled, "PcaConnectorArn", "a connector brought into service");
    assert_eq!(
        reported, creating,
        "case {:?}: expected the connector the enable named, got {reported:?}",
        "a connector brought into service"
    );

    // And the neighbour, whose own connector this directory could not use, is untouched.
    let status = policy_status(&router, &other, "another directory's connector").await;
    assert_eq!(
        status, DISABLED,
        "case {:?}: the neighbour directory holds no policy, got {status:?}",
        "another directory's connector"
    );
}

#[tokio::test]
async fn a_disable_needs_a_policy_to_withdraw() {
    let router = emulator();

    // Both creates, so the refusal is about the directory's policy rather than about one
    // directory type.
    let rows = vec![
        PolicyCase {
            case: "a Simple AD directory with no policy",
            operation: "CreateDirectory",
            body: minimal_create("nothing-enrolled.example.com"),
            enrolled: false,
        },
        PolicyCase {
            case: "a Managed Microsoft AD directory with no policy",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("nothing-corp.example.com"),
            enrolled: false,
        },
    ];

    for row in &rows {
        let id = create(&router, row).await;
        await_directory_in_service(&router, &id, row.case).await;
        refused(
            &router,
            "DisableCAEnrollmentPolicy",
            json!({ "DirectoryId": &id }),
            "EntityDoesNotExistException",
            row.case,
        )
        .await;
        // And the refusal recorded nothing: the read still reports the inactive status.
        let status = policy_status(&router, &id, row.case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: a refused disable records nothing, got {status:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn both_writes_need_a_directory_that_is_in_service() {
    let router = emulator();

    let rows = vec![
        PolicyCase {
            case: "a Simple AD directory still being built",
            operation: "CreateDirectory",
            body: minimal_create("pending-simple.example.com"),
            enrolled: true,
        },
        PolicyCase {
            case: "a Managed Microsoft AD directory still being built",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("pending-corp.example.com"),
            enrolled: true,
        },
    ];

    let mut ids: Vec<(&PolicyCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        // The read needs no particular stage: it reports what the store holds.
        let status = policy_status(&router, &id, row.case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: a directory still being built reports {DISABLED}, got {status:?}",
            row.case
        );
        // The stage is resolved BEFORE the connector is looked at, so a directory that is
        // not in service is reported as such even for a connector nothing minted.
        for (operation, body) in [
            (
                "EnableCAEnrollmentPolicy",
                json!({
                    "DirectoryId": &id,
                    "PcaConnectorArn": ABSENT_CONNECTORS[0].0,
                }),
            ),
            ("DisableCAEnrollmentPolicy", json!({ "DirectoryId": &id })),
        ] {
            refused(
                &router,
                operation,
                body,
                "DirectoryUnavailableException",
                row.case,
            )
            .await;
        }
        ids.push((row, id));
    }

    // The refusal was about the stage, so the very same enable is admitted once the
    // directory is in service and the connector it names is one the owning service holds.
    for (row, id) in &ids {
        await_directory_in_service(&router, id, row.case).await;
        let connector = active_connector(&router, id, row.case).await;
        ok(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": id, "PcaConnectorArn": &connector }),
            row.case,
        )
        .await;
        await_policy_status(&router, id, SUCCESS, row.case).await;
    }
}

#[tokio::test]
async fn every_operation_resolves_the_directory_in_the_store() {
    let router = emulator();

    // One stored directory, so the store is not empty and a refusal cannot come from
    // there being nothing at all.
    let stored = create(
        &router,
        &PolicyCase {
            case: "the stored directory",
            operation: "CreateDirectory",
            body: minimal_create("resolved.example.com"),
            enrolled: true,
        },
    )
    .await;
    await_directory_in_service(&router, &stored, "the stored directory").await;
    let connector = active_connector(&router, &stored, "the stored directory").await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        for (operation, body) in [
            (
                "EnableCAEnrollmentPolicy",
                json!({ "DirectoryId": absent, "PcaConnectorArn": &connector }),
            ),
            ("DisableCAEnrollmentPolicy", json!({ "DirectoryId": absent })),
            ("DescribeCAEnrollmentPolicy", json!({ "DirectoryId": absent })),
        ] {
            refused(
                &router,
                operation,
                body,
                "DirectoryDoesNotExistException",
                class,
            )
            .await;
        }
    }

    // An id the MODEL refuses never reaches that resolution: the boundary validator
    // refuses it first, so the answer names neither the directory nor the scaffold.
    let malformed = [
        ("d-ABCDEF0123", "upper case, outside the pattern's alphabet"),
        ("d-0a1b2c3d", "too few digits"),
        ("d-0a1b2c3d4e5", "too many digits"),
        ("", "empty"),
    ];
    for (id, class) in malformed {
        for operation in [
            "EnableCAEnrollmentPolicy",
            "DisableCAEnrollmentPolicy",
            "DescribeCAEnrollmentPolicy",
        ] {
            let mut body = json!({ "DirectoryId": id });
            if operation == "EnableCAEnrollmentPolicy" {
                body["PcaConnectorArn"] = json!(&connector);
            }
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {class:?}: the model refuses {id:?} for {operation}, got {status} \
                 with {parsed}"
            );
            let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                !wire_type.contains("DirectoryDoesNotExistException"),
                "case {class:?}: {operation} refuses the input before the directory is \
                 resolved, got {parsed}"
            );
            assert!(
                !wire_type.contains("NotImplemented"),
                "case {class:?}: {operation} must not answer the scaffold signal, got \
                 {parsed}"
            );
        }
    }

    // A connector the MODEL refuses is refused the same way, before anything is recorded.
    let malformed_connectors = [
        (
            "arn:aws:pca-connector-ad:us-east-1:000000000000:connector/not-a-uuid",
            "a connector id outside the pattern",
        ),
        (
            "arn:aws:acm-pca:us-east-1:000000000000:connector/01234567-89ab-cdef-0123-456789abcdef",
            "another service's namespace",
        ),
        ("", "empty"),
    ];
    for (malformed, class) in malformed_connectors {
        let (status, parsed) = call(
            &router,
            "EnableCAEnrollmentPolicy",
            json!({ "DirectoryId": &stored, "PcaConnectorArn": malformed }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {class:?}: the model refuses {malformed:?}, got {status} with {parsed}"
        );
        let status = policy_status(&router, &stored, class).await;
        assert_eq!(
            status, DISABLED,
            "case {class:?}: a refused enable records nothing, got {status:?}"
        );
    }

    // And the stored directory still round-trips through the enable, so none of the
    // refusals above touched it.
    ok(
        &router,
        "EnableCAEnrollmentPolicy",
        json!({ "DirectoryId": &stored, "PcaConnectorArn": &connector }),
        "the stored directory",
    )
    .await;
    let settled = await_policy_status(&router, &stored, SUCCESS, "the stored directory").await;
    let reported = string(&settled, "PcaConnectorArn", "the stored directory");
    assert_eq!(
        reported, connector,
        "case {:?}: the stored directory enrolls and reports back, got {reported:?}",
        "the stored directory"
    );
}
