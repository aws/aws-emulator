// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of IoT POLICY VERSIONS: `CreatePolicyVersion`, the
//! `ListPolicyVersions` and `GetPolicyVersion` readers that prove what it persisted,
//! and the cap a policy's versions are held under.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a policy holds at most [`MAX_POLICY_VERSIONS`] versions: each create adds one
//!   under a fresh id, and a create past the cap is refused
//!   `VersionsLimitExceededException` (409) with nothing added — so however many
//!   callers add versions at once, a policy never holds more than the cap;
//! * a version is read back with the document it was created with, and which version
//!   is in EFFECT is the policy's own pointer, moved only by a create that asked for
//!   it;
//! * a document that is not an IoT policy document is `MalformedPolicyException`
//!   (400), and a policy the account does not hold is `ResourceNotFoundException`
//!   (404).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::MAX_POLICY_VERSIONS;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for.
const REGION: &str = "ap-northeast-1";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the policy request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_policy(&self, policy_name: &str, document: &str) -> Reply {
        self.dispatch(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": document })),
        )
        .await
    }

    async fn create_version(
        &self,
        policy_name: &str,
        document: &str,
        set_as_default: bool,
    ) -> Reply {
        self.dispatch(
            "POST",
            &format!("/policies/{policy_name}/version?setAsDefault={set_as_default}"),
            Some(&json!({ "policyDocument": document })),
        )
        .await
    }

    async fn list_versions(&self, policy_name: &str) -> Reply {
        self.dispatch("GET", &format!("/policies/{policy_name}/version"), None)
            .await
    }

    async fn get_version(&self, policy_name: &str, version_id: &str) -> Reply {
        self.dispatch(
            "GET",
            &format!("/policies/{policy_name}/version/{version_id}"),
            None,
        )
        .await
    }

    async fn get_policy(&self, policy_name: &str) -> Reply {
        self.dispatch("GET", &format!("/policies/{policy_name}"), None)
            .await
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// An IoT policy document allowing one action, numbered so each version differs.
fn document(index: usize) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["iot:Publish"],
            "Resource": [format!("arn:aws:iot:{REGION}:000000000000:topic/version-{index}")],
        }],
    })
    .to_string()
}

/// RULE: versions accumulate up to the cap, each under a fresh id, and the create past
/// the cap adds nothing.
#[tokio::test]
async fn versions_accumulate_up_to_the_cap_and_no_further() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_policy("fleet-policy", &document(1)).await.status,
        200,
        "the policy a case adds versions to must be creatable"
    );

    // The create of the policy is its first version, so the cap leaves one fewer to
    // add. Each row asks for a different `setAsDefault`, so the pointer's rule is
    // exercised in both directions.
    for index in 2..=MAX_POLICY_VERSIONS {
        let set_as_default = index % 2 == 0;
        let created = emulator
            .create_version("fleet-policy", &document(index), set_as_default)
            .await;
        assert_eq!(
            created.status,
            200,
            "case version {index}: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        let body = created.json();
        assert_eq!(
            body["policyVersionId"],
            json!(index.to_string()),
            "case version {index}: expected a fresh id, got {body}"
        );
        assert_eq!(
            body["isDefaultVersion"],
            json!(set_as_default),
            "case version {index}: expected isDefaultVersion {set_as_default}, got {body}"
        );
        assert_eq!(
            body["policyDocument"],
            json!(document(index)),
            "case version {index}: expected the stored document, got {body}"
        );
    }

    // One past the cap: refused, and the policy still holds exactly the cap.
    let over = emulator
        .create_version("fleet-policy", &document(99), false)
        .await;
    assert_eq!(
        over.status,
        409,
        "case one past the cap: expected 409, got {} body {}",
        over.status,
        over.text()
    );
    assert_eq!(
        over.error_code(),
        "VersionsLimitExceededException",
        "case one past the cap: got {}",
        over.text()
    );

    let listed = emulator.list_versions("fleet-policy").await.json();
    let versions = listed["policyVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("no policyVersions in {listed}"));
    assert_eq!(
        versions.len(),
        MAX_POLICY_VERSIONS,
        "case the listing: expected the cap and no more, got {listed}"
    );
    let ids: Vec<String> = versions
        .iter()
        .map(|version| version["versionId"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected_ids: Vec<String> = (1..=MAX_POLICY_VERSIONS)
        .map(|index| index.to_string())
        .collect();
    assert_eq!(
        ids, expected_ids,
        "case the listing: expected the versions oldest-first, got {ids:?}"
    );
    let defaults: Vec<&Value> = versions
        .iter()
        .filter(|version| version["isDefaultVersion"] == json!(true))
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "case the listing: exactly one version is in effect, got {listed}"
    );

    // Each version reads back the document it was created with, and the one in effect
    // is the one the policy's own reader reports.
    for index in 1..=MAX_POLICY_VERSIONS {
        let read = emulator
            .get_version("fleet-policy", &index.to_string())
            .await;
        assert_eq!(
            read.status,
            200,
            "case reading version {index}: expected 200, got {} body {}",
            read.status,
            read.text()
        );
        let body = read.json();
        assert_eq!(
            body["policyDocument"],
            json!(document(index)),
            "case reading version {index}: expected its own document, got {body}"
        );
        assert_eq!(
            body["policyName"],
            json!("fleet-policy"),
            "case reading version {index}: expected the policy named, got {body}"
        );
    }
    let policy = emulator.get_policy("fleet-policy").await.json();
    let default_version_id = policy["defaultVersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no defaultVersionId in {policy}"));
    let in_effect = emulator
        .get_version("fleet-policy", default_version_id)
        .await
        .json();
    assert_eq!(
        in_effect["isDefaultVersion"],
        json!(true),
        "case the version in effect: the two readers must agree, got {in_effect}"
    );
    assert_eq!(
        policy["policyDocument"], in_effect["policyDocument"],
        "case the version in effect: the policy reports that version's document, got {policy}"
    );
}

/// RULE: a version is only created for a policy that exists, out of a document that is
/// an IoT policy document.
#[tokio::test]
async fn a_version_of_no_policy_or_of_no_policy_document_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_policy("fleet-policy", &document(1)).await.status,
        200
    );

    let missing = emulator
        .create_version("no-such-policy", &document(2), false)
        .await;
    assert_eq!(
        missing.status,
        404,
        "case a policy the account does not hold: expected 404, got {} body {}",
        missing.status,
        missing.text()
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case a policy the account does not hold: got {}",
        missing.text()
    );

    // The rows break the document rule in a different way each: not JSON at all, JSON
    // that is not an object, an object with no statement, a statement with no effect,
    // and a statement naming no action.
    let malformed = [
        "not json at all",
        "[]",
        "{}",
        r#"{"Statement":[{"Action":["iot:Publish"]}]}"#,
        r#"{"Statement":[{"Effect":"Allow"}]}"#,
        r#"{"Statement":[{"Effect":"Maybe","Action":["iot:Publish"]}]}"#,
    ];
    for document in malformed {
        let refused = emulator
            .create_version("fleet-policy", document, false)
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {document:?}: expected 400, got {} body {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "MalformedPolicyException",
            "case {document:?}: got {}",
            refused.text()
        );
    }

    // Nothing above was written: the policy still holds only the version its create
    // wrote.
    let listed = emulator.list_versions("fleet-policy").await.json();
    assert_eq!(
        listed["policyVersions"].as_array().map(Vec::len),
        Some(1),
        "case after the refusals: expected the one version the create wrote, got {listed}"
    );

    // A version id the policy does not hold, and one that is no version id at all.
    for version_id in ["7", "0"] {
        let read = emulator.get_version("fleet-policy", version_id).await;
        assert_eq!(
            read.status,
            404,
            "case version {version_id}: expected 404, got {} body {}",
            read.status,
            read.text()
        );
    }
    let listed_missing = emulator.list_versions("no-such-policy").await;
    assert_eq!(
        listed_missing.status,
        404,
        "case listing a policy the account does not hold: expected 404, got {} body {}",
        listed_missing.status,
        listed_missing.text()
    );
}
