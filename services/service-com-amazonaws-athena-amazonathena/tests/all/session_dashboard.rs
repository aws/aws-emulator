// SPDX-License-Identifier: CC0-1.0

//! The DASHBOARD of one session, driven end to end over the same router the emulator boots
//! (awsJson1_1, `x-amz-target: AmazonAthena.GetResourceDashboard`): `StartSession` starts a
//! session in a Spark enabled workgroup and `GetResourceDashboard` is asked for that
//! session's user interface by the session's own ARN.
//!
//! THE RULE UNDER TEST: `GetResourceDashboard` resolves the session its `ResourceARN` names
//! — an Athena ARN of this account, in the Region the call was addressed to, naming a
//! `SessionId` this account holds a session under — and answers with the presigned address of
//! the interface THAT session serves, the live one while its resources are running and the
//! persistent one once they are gone; every ARN this account resolves no session from is
//! refused with HTTP 400 `InvalidRequestException` carrying one sentence for the whole class.
//!
//! The rows span what the rule generalises over: the ARN forms AWS writes a resource in (the
//! slash and the colon separator, the session named alone and scoped to its workgroup,
//! another partition), and every ARN that names a session this account resolves none of
//! (another vendor, another account, another Region, an id nothing was started under, a
//! session scoped to a workgroup it does not belong to, and a session whose workgroup has
//! been deleted). A value out of which no session reference can be read at all is refused for
//! its FORM instead, which `tests/all/refusal_condition_reported_first.rs` drives.
//!
//! Nothing here waits for a fixed span: the one test that needs a transition the clock owes
//! POLLS the service for the state it is waiting on, under a deadline far above the span that
//! transition is due after, so a loaded host cannot turn a slow transition into a failure.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, MAX_SIGNED_URL_EXPIRY, UNSERVED_URL_BASE};

use service_com_amazonaws_athena_amazonathena::state::{
    ERROR_CODE_INVALID_INPUT, LIVE_UI_PATH, PERSISTENCE_UI_PATH, PYSPARK_ENGINE_VERSION,
    RESOURCE_ARN_NOT_FOUND, SESSION_DASHBOARD_URL_VALIDITY, SESSION_STATE_TERMINATED,
};

use crate::harness::{assert_refusal, call, router};

/// The operation under test.
const OPERATION: &str = "GetResourceDashboard";

/// The Spark enabled workgroup the sessions below are started in.
const SPARK_WORK_GROUP: &str = "spark-dashboards";

/// A second Spark enabled workgroup, so a row can scope a session's ARN to a workgroup the
/// session does not belong to.
const OTHER_WORK_GROUP: &str = "spark-dashboards-other";

/// The execution role a Spark workgroup accesses user resources through, inside the modelled
/// `RoleArn` pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/AthenaSparkExecution";

/// Where the Spark workgroups below write their results.
const OUTPUT_LOCATION: &str = "s3://athena-spark-results/sessions/";

/// The Region every request of this suite is signed for, which is the Region the harness's
/// SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// How long a poll for a transition the clock owes keeps looking, and how long it waits
/// between looks. The deadline is far above the span a shutdown is due after, so the
/// CONDITION is what is waited on rather than a span of the host's own speed.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const POLL_LOOKS: u32 = 1_200;

/// Create a Spark enabled workgroup: a PySpark engine version, an execution role and an
/// output location, which is the workgroup Athena documents sessions run in.
async fn given_spark_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": name,
            "Configuration": {
                "EngineVersion": { "SelectedEngineVersion": PYSPARK_ENGINE_VERSION },
                "ExecutionRole": EXECUTION_ROLE,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Spark workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Start one session in `work_group` and hand back its `SessionId`.
async fn given_session(router: &MultiServiceRouter, work_group: &str) -> String {
    let (status, started) = call(
        router,
        "StartSession",
        &json!({
            "WorkGroup": work_group,
            "EngineConfiguration": {
                "CoordinatorDpuSize": 1,
                "MaxConcurrentDpus": 4,
                "DefaultExecutorDpuSize": 1
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSession in {work_group} must be accepted, got {status} with {started:?}"
    );
    started["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSession must answer with a SessionId, got {started:?}"))
        .to_string()
}

/// The account every ARN of this suite names: the one canonical account id this emulator
/// presents.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The dashboard answer for `resource_arn`.
async fn dashboard(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, OPERATION, &json!({ "ResourceARN": resource_arn })).await
}

/// The `Url` a successful dashboard read answered with.
async fn dashboard_url(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> String {
    let (status, answer) = dashboard(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {OPERATION}({resource_arn}) to answer 200, got {status} with \
         {answer:?}"
    );
    answer["Url"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Url in the answer, got {answer:?}")
        })
        .to_string()
}

/// Everything before the query a URL carries: the interface it opens.
fn opened_interface(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// The query fields a URL carries, `name` to `value`, each as the URL spells it.
fn query_fields(case: &str, url: &str) -> BTreeMap<String, String> {
    let query = url
        .split_once('?')
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a presigned URL carrying a query, got {url}")
        })
        .1;
    query
        .split('&')
        .map(|field| {
            let (name, value) = field.split_once('=').unwrap_or_else(|| {
                panic!("case {case:?}: expected name=value query fields, got {query:?} in {url}")
            });
            (name.to_string(), value.to_string())
        })
        .collect()
}

/// The fields a SigV4 query-signed URL carries: what makes it verifiable, and what bounds
/// how long it verifies for.
const SIGNATURE_FIELDS: [&str; 5] = [
    "X-Amz-Algorithm",
    "X-Amz-Credential",
    "X-Amz-Date",
    "X-Amz-SignedHeaders",
    "X-Amz-Signature",
];

/// THE RULE's presigning half, asserted on one URL: an address this emulator hands a caller
/// to open later leads BACK to this emulator rather than to the vendor's own domain, is
/// signed for this service, and carries the bounded validity this service gives a dashboard,
/// so it stops working instead of opening an interface for ever.
fn assert_presigned_against_this_emulator(case: &str, url: &str) {
    assert!(
        url.starts_with(UNSERVED_URL_BASE),
        "case {case:?}: a presigned URL leads back to this emulator ({UNSERVED_URL_BASE}), got \
         {url}"
    );
    let fields = query_fields(case, url);
    for field in SIGNATURE_FIELDS {
        assert!(
            fields.get(field).is_some_and(|value| !value.is_empty()),
            "case {case:?}: a presigned URL carries {field}, got {url}"
        );
    }
    assert!(
        fields
            .get("X-Amz-Credential")
            .is_some_and(|scope| scope.contains("athena")),
        "case {case:?}: the URL is signed for this service, got {url}"
    );
    let expires = fields
        .get("X-Amz-Expires")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a presigned URL carries X-Amz-Expires, got {url}")
        })
        .parse::<u64>()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: X-Amz-Expires counts seconds, got {error} in {url}")
        });
    assert_eq!(
        expires,
        SESSION_DASHBOARD_URL_VALIDITY.as_secs(),
        "case {case:?}: the URL declares the validity this service gives a session dashboard, \
         got {url}"
    );
    assert!(
        expires > 0 && Duration::from_secs(expires) <= MAX_SIGNED_URL_EXPIRY,
        "case {case:?}: the expiration is bounded (0 < {expires}s <= {}s), got {url}",
        MAX_SIGNED_URL_EXPIRY.as_secs()
    );
}

/// RULE: the dashboard is addressed by the session's own ARN, in every form AWS writes a
/// resource ARN in, and the address answered is that session's interface presigned against
/// this emulator.
#[tokio::test]
async fn a_sessions_arn_addresses_that_sessions_dashboard_in_every_arn_form() {
    let router = router();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    let session_id = given_session(&router, SPARK_WORK_GROUP).await;
    let account = account();

    // The forms differ along what the rule generalises over: which separator divides the
    // resource field (AWS writes a resource as `<type>/<id>` and as `<type>:<id>`), whether
    // the ARN scopes the session to the workgroup it belongs to, and the partition, which
    // this endpoint does not read. Every form spells out the Region and the account: a
    // session is a regional resource of one account, so an ARN leaving either field empty is
    // not a complete ARN of a session at all
    // (`tests/all/refusal_condition_reported_first.rs`).
    let forms = [
        (
            "the session named alone",
            format!("arn:aws:athena:{REGION}:{account}:session/{session_id}"),
        ),
        (
            "the colon form of that same resource",
            format!("arn:aws:athena:{REGION}:{account}:session:{session_id}"),
        ),
        (
            "scoped to the workgroup it belongs to",
            format!(
                "arn:aws:athena:{REGION}:{account}:workgroup/{SPARK_WORK_GROUP}/session/\
                 {session_id}"
            ),
        ),
        (
            "the colon form of the workgroup-scoped resource",
            format!(
                "arn:aws:athena:{REGION}:{account}:workgroup:{SPARK_WORK_GROUP}:session:\
                 {session_id}"
            ),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:athena:{REGION}:{account}:session/{session_id}"),
        ),
    ];

    for (case, resource_arn) in forms {
        let url = dashboard_url(&router, case, &resource_arn).await;
        assert_presigned_against_this_emulator(case, &url);
        let interface = opened_interface(&url);
        assert!(
            interface.contains(session_id.as_str()) && interface.ends_with(&format!("/{LIVE_UI_PATH}/")),
            "case {case:?}: expected the live interface of the session {session_id}, got {url}"
        );
    }
}

/// RULE: which of the two interfaces the address opens follows the SESSION's own state — the
/// live one while its resources are running, the persistent one once they are gone — and a
/// dashboard read changes nothing a later caller sees.
#[tokio::test]
async fn the_interface_follows_the_sessions_state_and_the_read_changes_nothing() {
    let router = router();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    let session_id = given_session(&router, SPARK_WORK_GROUP).await;
    let arn = format!(
        "arn:aws:athena:{REGION}:{}:session/{session_id}",
        account()
    );

    // While the session's resources are running, the live interface — and two reads address
    // the same one, so reporting it moved nothing on.
    for case in ["the first read of a running session", "and the next one"] {
        let url = dashboard_url(&router, case, &arn).await;
        assert!(
            opened_interface(&url).ends_with(&format!("/{LIVE_UI_PATH}/")),
            "case {case:?}: expected the live interface, got {url}"
        );
    }

    // The session is still read by `GetSession` after those reads, in the state it was in:
    // the dashboard is a projection of the record rather than a change to it.
    let (status, read) = call(&router, "GetSession", &json!({ "SessionId": session_id })).await;
    assert_eq!(
        (status, read["SessionId"].as_str()),
        (200, Some(session_id.as_str())),
        "case \"the session after two dashboard reads\": expected it to read back, got {status} \
         with {read:?}"
    );
    assert_ne!(
        read["Status"]["State"].as_str(),
        Some(SESSION_STATE_TERMINATED),
        "case \"the session after two dashboard reads\": a read must not shut it down, got \
         {read:?}"
    );

    // Once the shutdown the clock owes is done, the persistent interface is what remains.
    let (status, terminated) = call(
        &router,
        "TerminateSession",
        &json!({ "SessionId": session_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the terminate\": expected it to be accepted, got {status} with {terminated:?}"
    );
    settled_in(&router, &session_id, SESSION_STATE_TERMINATED).await;

    let case = "a session whose resources are gone";
    let url = dashboard_url(&router, case, &arn).await;
    assert_presigned_against_this_emulator(case, &url);
    assert!(
        opened_interface(&url).ends_with(&format!("/{PERSISTENCE_UI_PATH}/")),
        "case {case:?}: expected the persistent interface of a session that has been shut \
         down, got {url}"
    );
}

/// Poll until the session `session_id` reports `wanted`.
///
/// The CONDITION is what is waited on: every look asks the service what state the session is
/// in, and the deadline is far above the span the transition is due after.
async fn settled_in(router: &MultiServiceRouter, session_id: &str, wanted: &str) {
    for _ in 0..POLL_LOOKS {
        let (status, body) = call(
            router,
            "GetSessionStatus",
            &json!({ "SessionId": session_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSessionStatus({session_id}) must answer 200 while polling, got {status} with \
             {body:?}"
        );
        if body["Status"]["State"].as_str() == Some(wanted) {
            return;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("the session {session_id} never reached {wanted}");
}

/// RULE: an ARN this account resolves no session from is refused as a bad request, with one
/// sentence for the whole class — whatever made it unresolvable.
#[tokio::test]
async fn an_arn_no_session_resolves_from_is_refused_as_an_invalid_request() {
    let router = router();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    given_spark_work_group(&router, OTHER_WORK_GROUP).await;
    // A real session exists throughout, so each refusal below is about the ARN the request
    // carried rather than about an account holding no sessions at all.
    let session_id = given_session(&router, SPARK_WORK_GROUP).await;
    let account = account();
    let unknown = ::uuid::Uuid::new_v4().to_string();

    // Every row here SPELLS OUT the ARN of a session and still resolves none: a value out of
    // which no session reference can be read at all is a different condition, worded apart,
    // and is driven in `tests/all/refusal_condition_reported_first.rs`.
    let unresolvable = [
        (
            "another vendor's ARN",
            format!("arn:aws:glue:{REGION}:{account}:session/{session_id}"),
        ),
        (
            "another account's ARN",
            format!("arn:aws:athena:{REGION}:999999999999:session/{session_id}"),
        ),
        (
            "another Region's ARN",
            format!("arn:aws:athena:eu-west-1:{account}:session/{session_id}"),
        ),
        (
            "a session id nothing was started under",
            format!("arn:aws:athena:{REGION}:{account}:session/{unknown}"),
        ),
        (
            "a session scoped to a workgroup it does not belong to",
            format!(
                "arn:aws:athena:{REGION}:{account}:workgroup/{OTHER_WORK_GROUP}/session/\
                 {session_id}"
            ),
        ),
        (
            "a workgroup that does not exist at all",
            format!("arn:aws:athena:{REGION}:{account}:workgroup/absent/session/{session_id}"),
        ),
    ];

    for (case, resource_arn) in unresolvable {
        let (status, refused) = dashboard(&router, &resource_arn).await;
        assert_refusal(
            case,
            status,
            &refused,
            ERROR_CODE_INVALID_INPUT,
            RESOURCE_ARN_NOT_FOUND,
        );
    }

    // And the ARN that DOES resolve still answers, so the refusals above are about the ARN
    // and not about the operation.
    let url = dashboard_url(
        &router,
        "the session's own ARN",
        &format!("arn:aws:athena:{REGION}:{account}:session/{session_id}"),
    )
    .await;
    assert!(
        opened_interface(&url).contains(session_id.as_str()),
        "case \"the session's own ARN\": expected this session's interface, got {url}"
    );
}

/// RULE: the dashboard is backed by the session registry, so a session that is no longer held
/// — its workgroup having been deleted with its contents — resolves from its ARN no longer.
#[tokio::test]
async fn a_session_its_work_group_took_with_it_resolves_no_dashboard() {
    let router = router();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    let session_id = given_session(&router, SPARK_WORK_GROUP).await;
    let arn = format!(
        "arn:aws:athena:{REGION}:{}:session/{session_id}",
        account()
    );

    let url = dashboard_url(&router, "before the workgroup is deleted", &arn).await;
    assert!(
        opened_interface(&url).contains(session_id.as_str()),
        "case \"before the workgroup is deleted\": expected this session's interface, got {url}"
    );

    let (status, deleted) = call(
        &router,
        "DeleteWorkGroup",
        &json!({ "WorkGroup": SPARK_WORK_GROUP, "RecursiveDeleteOption": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the recursive delete\": expected it to be accepted, got {status} with \
         {deleted:?}"
    );

    let (status, refused) = dashboard(&router, &arn).await;
    assert_refusal(
        "after the workgroup is deleted",
        status,
        &refused,
        ERROR_CODE_INVALID_INPUT,
        RESOURCE_ARN_NOT_FOUND,
    );
}
