// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a name another resource already holds.
//!
//! The rule these suites assert: a create whose requested name is already taken is
//! refused 409 `ConflictException` whose `message` is the FIXED sentence of the
//! colliding resource's class — the whole message being fixed text, with no part taken
//! from the request and no part taken from the resource already holding the name — while
//! the ARN of that resource is reported in `resourceId` and its class in `resourceType`.
//!
//! The rule generalises over the operation that mints the resource, over the resource
//! class and over the NAME asked for, so the tables below span every name-minting
//! operation of the implemented lifecycle and, for each, names differing in length (both
//! published bounds of the modelled member), in case, in character set and in which
//! request member carries them: one sentence per class must serve all of them, which is
//! exactly what a name-interpolated message cannot do.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The sentence this service answers when a CLUSTER already holds the requested name.
///
/// Recorded AWS behavior for a duplicate `clusterName`: the entire message, with no
/// variable part.
const CLUSTER_SENTENCE: &str = "A cluster already exists with given name";

/// The same sentence for the other resource class this service owns, the cluster
/// snapshot: one fixed sentence per class, naming the class and nothing else.
///
/// Recorded AWS behavior for a duplicate snapshot name, on the operations that mint a
/// snapshot (`CreateClusterSnapshot`, `CopyClusterSnapshot`).
const SNAPSHOT_SENTENCE: &str = "A snapshot already exists with given name";

/// The AWS resource type names the error bodies report for the two classes.
///
/// Source: `smithy_query(query="shape", shape="ConflictException")` — `resourceType` is
/// "The type of the resource where there was an access conflict", spelled as this
/// service's API names the class.
const CLUSTER_TYPE: &str = "Cluster";
const SNAPSHOT_TYPE: &str = "Snapshot";

/// The inclusive length bounds of the snapshot-name members.
///
/// Source: `smithy_query(query="operation", operation="CreateClusterSnapshot,CopyClusterSnapshot")`
/// — `snapshotName` and `targetSnapshotName` both carry `length(min: 1, max: 63)`.
const SNAPSHOT_NAME_LEN: (usize, usize) = (1, 63);

/// The inclusive length bounds of a `clusterName`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterInput", member="clusterName")`
/// — "Must contain from 1 to 63 letters, numbers, or hyphens. The first character must be
/// a letter."
const CLUSTER_NAME_LEN: (usize, usize) = (1, 63);

/// A SigV4 `Authorization` header for this service in [`REGION`].
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when the
/// body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// Create a cluster that must succeed, returning its `Cluster`.
async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, response) = call(router, "POST", "/cluster", Some(minimal_create(name))).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture cluster must be created, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Snapshot `cluster_arn` under `name`, which must succeed, returning the snapshot.
async fn create_snapshot(router: &MultiServiceRouter, cluster_arn: &str, name: &str) -> Value {
    let (status, response) = call(
        router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture snapshot must be created, got {status} with {response}"
    );
    response["snapshot"].clone()
}

/// Check one refusal against the rule, returning the `message` it carried.
fn assert_fixed_conflict(
    case: &str,
    status: u16,
    body: &Value,
    sentence: &str,
    resource_type: &str,
    existing_arn: &str,
    requested_name: &str,
) -> String {
    assert_eq!(
        status, 409,
        "case {case:?}: a taken name is HTTP 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: expected a ConflictException, got {body}"
    );
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry `message`, got {body}"))
        .to_string();
    assert_eq!(
        message, sentence,
        "case {case:?}: the refusal must be the fixed sentence {sentence:?}, got {message:?}"
    );
    // Nothing the request asked for, and nothing identifying the resource that already
    // holds the name, may appear in the message. Short names are substrings of ordinary
    // English, so the name is only checked for once it cannot match by accident.
    if requested_name.chars().count() >= 4 {
        assert!(
            !message.contains(requested_name),
            "case {case:?}: the message must carry no part of the request, found \
             {requested_name:?} in {message:?}"
        );
    }
    assert!(
        !message.contains(existing_arn),
        "case {case:?}: the message must not state the colliding resource's ARN, got \
         {message:?}"
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(existing_arn),
        "case {case:?}: resourceId must name the resource already holding the name, got {body}"
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: resourceType must be {resource_type:?}, got {body}"
    );
    message
}

/// Cluster names differing in length (both published bounds), case and character set.
/// Each row is `(case, the name created, the name the colliding request asks for)`.
fn cluster_name_rows() -> Vec<(&'static str, String, String)> {
    let (min, max) = CLUSTER_NAME_LEN;
    let shortest = "a".repeat(min);
    // The first character must be a letter, so the longest well-formed name is a letter
    // followed by `max - 1` more.
    let longest = format!("z{}", "y".repeat(max - 1));
    vec![
        (
            "the shortest allowed name",
            shortest.clone(),
            shortest.clone(),
        ),
        ("the longest allowed name", longest.clone(), longest.clone()),
        (
            "a name of letters, digits and hyphens",
            "fleet-07-shard-3".to_string(),
            "fleet-07-shard-3".to_string(),
        ),
        (
            // Cluster names are stored lowercased, so a differently cased request asks
            // for the same name and must be refused with the same sentence.
            "the same name in another case",
            "MixedCaseFleet".to_string(),
            "mIXEDcASEfLEET".to_string(),
        ),
    ]
}

/// Snapshot names differing in length (both published bounds), case and character set.
fn snapshot_name_rows() -> Vec<(&'static str, String)> {
    let (min, max) = SNAPSHOT_NAME_LEN;
    vec![
        ("the shortest allowed name", "s".repeat(min)),
        ("the longest allowed name", "q".repeat(max)),
        (
            "a name of letters, digits and hyphens",
            "nightly-2026-10-31".to_string(),
        ),
        ("a name carrying spaces and case", "Nightly Backup".to_string()),
        (
            "a name outside ASCII",
            "sauvegarde-de-nuit-é".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_duplicate_cluster_name_is_refused_with_the_fixed_cluster_sentence() {
    let router = emulator();
    for (case, created, requested) in cluster_name_rows() {
        let cluster = create_cluster(&router, &created).await;
        let cluster_arn = cluster["clusterArn"]
            .as_str()
            .expect("the created cluster carries its ARN")
            .to_string();

        let (status, response) =
            call(&router, "POST", "/cluster", Some(minimal_create(&requested))).await;
        assert_fixed_conflict(
            case,
            status,
            &response,
            CLUSTER_SENTENCE,
            CLUSTER_TYPE,
            &cluster_arn,
            &requested,
        );

        // The refusal is the whole effect: the cluster that held the name is still the
        // only one under it.
        let (status, listing) = call(&router, "GET", "/clusters", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListClusters must still answer, got {status} with {listing}"
        );
        let under_the_name = listing["clusters"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListClusters must carry `clusters`"))
            .iter()
            .filter(|c| c["clusterName"].as_str() == Some(created.to_ascii_lowercase().as_str()))
            .count();
        assert_eq!(
            under_the_name, 1,
            "case {case:?}: the refusal must have created nothing, got {under_the_name} \
             clusters under the name"
        );
    }
}

#[tokio::test]
async fn a_duplicate_snapshot_name_is_refused_with_the_fixed_snapshot_sentence() {
    let router = emulator();
    let cluster = create_cluster(&router, "snapshot-conflicts").await;
    let cluster_arn = cluster["clusterArn"]
        .as_str()
        .expect("clusterArn")
        .to_string();
    // A second cluster, so a row can ask for a taken snapshot name while snapshotting
    // another cluster: the name is the snapshot's, whichever cluster is being snapshotted.
    let other = create_cluster(&router, "snapshot-conflicts-other").await;
    let other_arn = other["clusterArn"].as_str().expect("clusterArn").to_string();

    for (case, name) in snapshot_name_rows() {
        let snapshot = create_snapshot(&router, &cluster_arn, &name).await;
        let snapshot_arn = snapshot["snapshotArn"]
            .as_str()
            .expect("the created snapshot carries its ARN")
            .to_string();
        let copy_path = format!("/cluster-snapshot/{}/copy", label(&snapshot_arn));

        // Every operation that mints a snapshot refuses the taken name the same way:
        // the same cluster, another cluster, and a copy of the snapshot itself.
        let attempts: Vec<(String, &str, String, Value)> = vec![
            (
                format!("CreateClusterSnapshot on the same cluster / {case}"),
                "POST",
                "/cluster-snapshot".to_string(),
                json!({"clusterArn": cluster_arn, "snapshotName": name}),
            ),
            (
                format!("CreateClusterSnapshot on another cluster / {case}"),
                "POST",
                "/cluster-snapshot".to_string(),
                json!({"clusterArn": other_arn, "snapshotName": name}),
            ),
            (
                format!("CopyClusterSnapshot / {case}"),
                "POST",
                copy_path.clone(),
                json!({"targetSnapshotName": name}),
            ),
        ];

        for (attempt, method, path, body) in attempts {
            let (status, response) = call(&router, method, &path, Some(body)).await;
            assert_fixed_conflict(
                &attempt,
                status,
                &response,
                SNAPSHOT_SENTENCE,
                SNAPSHOT_TYPE,
                &snapshot_arn,
                &name,
            );
        }

        // Nothing was minted by the refusals: the name is still held by one snapshot.
        let (status, listing) = call(&router, "GET", "/cluster-snapshots", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListClusterSnapshots must still answer, got {status} with {listing}"
        );
        let under_the_name = listing["snapshots"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListClusterSnapshots must carry `snapshots`"))
            .iter()
            .filter(|s| s["snapshotName"].as_str() == Some(name.as_str()))
            .count();
        assert_eq!(
            under_the_name, 1,
            "case {case:?}: the refusal must have created nothing, got {under_the_name} \
             snapshots under the name"
        );
    }
}

#[tokio::test]
async fn one_sentence_serves_every_taken_name_of_a_class() {
    // The defect this pins: a message built from the requested name varies per request,
    // where AWS's is constant. So the sentence is collected over every name of a class
    // and must come back as ONE distinct value per class, and the two classes must not
    // share it.
    let router = emulator();
    let mut cluster_messages = BTreeSet::new();
    let mut snapshot_messages = BTreeSet::new();

    for (case, created, requested) in cluster_name_rows() {
        create_cluster(&router, &created).await;
        let (status, response) =
            call(&router, "POST", "/cluster", Some(minimal_create(&requested))).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a taken cluster name is HTTP 409, got {status} with {response}"
        );
        cluster_messages.insert(response["message"].as_str().unwrap_or_default().to_string());
    }

    let cluster = create_cluster(&router, "sentence-probe").await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
    for (case, name) in snapshot_name_rows() {
        create_snapshot(&router, &cluster_arn, &name).await;
        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a taken snapshot name is HTTP 409, got {status} with {response}"
        );
        snapshot_messages.insert(response["message"].as_str().unwrap_or_default().to_string());
    }

    assert_eq!(
        cluster_messages,
        [CLUSTER_SENTENCE.to_string()].into_iter().collect(),
        "case cluster: every taken name must share one fixed sentence, got {cluster_messages:?}"
    );
    assert_eq!(
        snapshot_messages,
        [SNAPSHOT_SENTENCE.to_string()].into_iter().collect(),
        "case snapshot: every taken name must share one fixed sentence, got {snapshot_messages:?}"
    );
    assert_ne!(
        cluster_messages, snapshot_messages,
        "case both classes: each resource class states its own sentence, got {cluster_messages:?}"
    );

    // Positive control: a free name is still minted, so the refusal is about the name
    // being taken and not about the request's shape.
    let free = create_cluster(&router, "nobody-holds-this").await;
    assert_eq!(
        free["clusterName"].as_str(),
        Some("nobody-holds-this"),
        "case free name: a name nobody holds must still be created, got {free}"
    );
}
