// SPDX-License-Identifier: CC0-1.0

//! The `AppInstance` / `AppInstanceUser` lifecycle, end to end over HTTP.
//!
//! Every case here drives the service the way the production router does — a buffered
//! `http::Request` in, a buffered `http::Response` out — so what it asserts is the wire
//! behaviour a client sees, not the shape of a Rust struct.
//!
//! Each test states the RULE it pins and then exercises a TABLE of inputs spanning what
//! that rule generalises over (value, size, form, count, ordering), so a hardcoded
//! answer cannot pass.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_chimesdkidentity_chimeidentityservice::state::{
    ComAmazonawsChimesdkidentityChimeidentityserviceState, MAX_APP_INSTANCES,
    MAX_APP_INSTANCE_USERS, MAX_IDEMPOTENCY_TOKENS,
};
use service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service it is for (the
/// 4th credential-scope field) and the region it addressed (the 3rd). Authentication is
/// switched off on the test router, so the signature itself is never checked.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/chime/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// Percent-encode a value for use as a single path segment or query value. An ARN
/// carries `:` and `/`, and a non-greedy `@httpLabel` is one segment, so the `/`
/// separators have to be escaped exactly as an AWS SDK escapes them. Every byte
/// outside the unreserved set is escaped, which is always valid in both positions.
/// `pub(crate)` so the suites beside this one in the same test binary share this one
/// encoder rather than each carrying a copy.
pub(crate) fn uri_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A one-service emulator over the shared engine, driven through the same
/// `MultiServiceRouter::dispatch` the production front door uses.
/// `pub(crate)` so the suites beside this one in the same test binary drive the service
/// through this one harness rather than each building another.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the service holds, so a test can read the store's shape
    /// directly to assert an invariant no response exposes (how many records it holds).
    engine: Arc<engine::InMemoryEngine>,
    region: String,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::in_region("us-west-2")
    }

    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            engine,
            region: region.to_string(),
        }
    }

    /// The SAME engine the service holds, for a suite that asserts an invariant of a store
    /// no response exposes — the family's shared directories among them, which this
    /// service's own API reports nothing about once the resource they were keyed on is
    /// gone. `pub(crate)` so those suites read the one engine this harness built.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// How many app instances, users and idempotency entries the store holds.
    fn store_sizes(&self) -> (usize, usize, usize) {
        let state = self
            .engine
            .state::<ComAmazonawsChimesdkidentityChimeidentityserviceState>();
        let state = state.lock();
        (
            state.app_instances.len(),
            state
                .app_instances
                .values()
                .map(|instance| instance.users.len())
                .sum(),
            state.idempotency.len(),
        )
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let (status, _headers, parsed) = self.call_with_headers(method, path_and_query, body).await;
        (status, parsed)
    }

    /// The same dispatch [`Emulator::call`] makes, with the response HEADERS kept.
    ///
    /// A REST+JSON client resolves the error SHAPE of a failure from `x-amzn-errortype`
    /// rather than from a body member, so the error class an SDK reports is only
    /// assertable with the headers in hand. `pub(crate)` so the suites beside this one
    /// read the wire through this one harness.
    pub(crate) async fn call_with_headers(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> (u16, http::HeaderMap, Value) {
        let payload = body
            .map(|b| Bytes::from(serde_json::to_vec(&b).expect("a serializable request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://identity-chime.{}.amazonaws.com{path_and_query}",
                self.region
            ))
            .header("host", format!("identity-chime.{}.amazonaws.com", self.region))
            .header("content-type", "application/json")
            .header("authorization", authorization(&self.region))
            .body(payload)
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let raw = response.into_body();
        if raw.is_empty() {
            return (status, headers, Value::Null);
        }
        let parsed = serde_json::from_slice(&raw).unwrap_or_else(|_| {
            panic!(
                "{method} {path_and_query} answered {status} with a non-JSON body: {:?}",
                String::from_utf8_lossy(&raw)
            )
        });
        (status, headers, parsed)
    }

    async fn create_app_instance(
        &self,
        name: &str,
        metadata: Option<&str>,
        token: &str,
    ) -> (u16, Value) {
        let mut body = json!({ "Name": name, "ClientRequestToken": token });
        if let Some(metadata) = metadata {
            body["Metadata"] = json!(metadata);
        }
        self.call("POST", "/app-instances", Some(body)).await
    }

    /// Create an app instance and return its ARN, failing the test if it was refused.
    pub(crate) async fn app_instance(&self, name: &str, token: &str) -> String {
        let (status, body) = self.create_app_instance(name, None, token).await;
        assert_eq!(status, 201, "CreateAppInstance({name}) answered: {body:?}");
        body["AppInstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateAppInstance({name}) returned no ARN: {body:?}"))
            .to_string()
    }

    async fn create_user(
        &self,
        app_instance_arn: &str,
        user_id: &str,
        name: &str,
        metadata: Option<&str>,
        expiration: Option<(i32, &str)>,
        token: &str,
    ) -> (u16, Value) {
        let mut body = json!({
            "AppInstanceArn": app_instance_arn,
            "AppInstanceUserId": user_id,
            "Name": name,
            "ClientRequestToken": token,
        });
        if let Some(metadata) = metadata {
            body["Metadata"] = json!(metadata);
        }
        if let Some((days, criterion)) = expiration {
            body["ExpirationSettings"] =
                json!({ "ExpirationDays": days, "ExpirationCriterion": criterion });
        }
        self.call("POST", "/app-instance-users", Some(body)).await
    }

    /// Create a user and return its ARN, failing the test if it was refused.
    pub(crate) async fn user(&self, app_instance_arn: &str, user_id: &str, token: &str) -> String {
        let (status, body) = self
            .create_user(app_instance_arn, user_id, user_id, None, None, token)
            .await;
        assert_eq!(
            status, 201,
            "CreateAppInstanceUser({user_id}) answered: {body:?}"
        );
        body["AppInstanceUserArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateAppInstanceUser({user_id}) returned no ARN: {body:?}"))
            .to_string()
    }

    async fn describe_app_instance(&self, arn: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/app-instances/{}", uri_encode(arn)),
            None,
        )
        .await
    }

    async fn describe_user(&self, arn: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(arn)),
            None,
        )
        .await
    }

    async fn delete_app_instance(&self, arn: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(arn)),
            None,
        )
        .await
    }

    async fn delete_user(&self, arn: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/app-instance-users/{}", uri_encode(arn)),
            None,
        )
        .await
    }

    async fn list_app_instances(&self) -> Vec<String> {
        let (status, body) = self.call("GET", "/app-instances", None).await;
        assert_eq!(status, 200, "ListAppInstances answered: {body:?}");
        arns_of(&body["AppInstances"], "AppInstanceArn")
    }

    async fn list_users(&self, app_instance_arn: &str) -> Vec<String> {
        let (status, body) = self
            .call(
                "GET",
                &format!(
                    "/app-instance-users?app-instance-arn={}",
                    uri_encode(app_instance_arn)
                ),
                None,
            )
            .await;
        assert_eq!(status, 200, "ListAppInstanceUsers answered: {body:?}");
        arns_of(&body["AppInstanceUsers"], "AppInstanceUserArn")
    }
}

/// The ARNs of a summary list, in the order the service returned them.
fn arns_of(list: &Value, member: &str) -> Vec<String> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item[member]
                        .as_str()
                        .unwrap_or_else(|| panic!("a summary with no {member}: {item:?}"))
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a created `AppInstance` is persisted, and `DescribeAppInstance` reports back
/// exactly the `Name` and `Metadata` that created it — for any value either may take.
#[tokio::test]
async fn describe_app_instance_reports_what_create_persisted() {
    // Rows span the model's own constraints: Name is `NonEmptyResourceName` (len
    // 1..256, printable + unicode) and Metadata is optional `Metadata` (len 0..1024).
    let cases: Vec<(&str, String, Option<String>)> = vec![
        ("name at the lower length bound", "a".to_string(), None),
        (
            "metadata at its lower length bound",
            "demo-app".to_string(),
            Some(String::new()),
        ),
        (
            "metadata carrying structured text",
            "demo-app".to_string(),
            Some("{\"env\":\"prod\",\"tier\":1}".to_string()),
        ),
        (
            "name with spaces and punctuation",
            "My App (prod) #1".to_string(),
            Some("env=prod;tier=1".to_string()),
        ),
        (
            "non-ascii name and metadata",
            "demo \u{2713} app".to_string(),
            Some("\u{30e1}\u{30bf}".to_string()),
        ),
        (
            "both members at their upper length bound",
            "n".repeat(256),
            Some("m".repeat(1024)),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, name, metadata)) in cases.iter().enumerate() {
        let token = format!("tok-describe-{index}");
        let (status, created) = emulator
            .create_app_instance(name, metadata.as_deref(), &token)
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateAppInstance to answer 201, got {status} {created:?}"
        );
        let arn = created["AppInstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no AppInstanceArn in {created:?}"));
        assert!(
            arn.starts_with("arn:aws:chime:us-west-2:") && arn.contains(":app-instance/"),
            "case {case:?}: expected an app-instance ARN in the signed region, got {arn:?}"
        );

        let (status, body) = emulator.describe_app_instance(arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAppInstance to answer 200, got {status} {body:?}"
        );
        let instance = &body["AppInstance"];
        assert_eq!(
            instance["AppInstanceArn"].as_str(),
            Some(arn),
            "case {case:?}: expected the described ARN to be {arn:?}, got {instance:?}"
        );
        assert_eq!(
            instance["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected Name {name:?}, got {:?}",
            instance["Name"]
        );
        assert_eq!(
            instance["Metadata"].as_str(),
            metadata.as_deref(),
            "case {case:?}: expected Metadata {metadata:?}, got {:?}",
            instance["Metadata"]
        );

        let created_at = instance["CreatedTimestamp"].as_f64();
        let updated_at = instance["LastUpdatedTimestamp"].as_f64();
        assert!(
            created_at.is_some() && updated_at.is_none(),
            "case {case:?}: expected a created instant and NO last-updated instant on a \
             never-updated instance, got created={created_at:?} updated={updated_at:?}"
        );

        // A read must not change what the next reader sees: the same ARN describes
        // identically a second time.
        let (status, again) = emulator.describe_app_instance(arn).await;
        assert_eq!(
            (status, &again), (200, &body),
            "case {case:?}: a second DescribeAppInstance differed from the first"
        );
    }

    // Every instance created above is still listed, once each.
    let listed = emulator.list_app_instances().await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "expected {} persisted instances, got {listed:?}",
        cases.len()
    );
}

/// RULE: `ClientRequestToken` is an idempotency token — repeating one token answers
/// with the ARN its first call minted and creates nothing new, while a token never
/// used before always mints a new resource.
#[tokio::test]
async fn repeating_a_client_request_token_mints_nothing_new() {
    // Rows differ in how many times one token is replayed.
    let repeats: &[(&str, usize)] = &[
        ("a single call", 1),
        ("one retry", 2),
        ("several retries", 4),
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for (case, repeats) in repeats {
        let token = format!("tok-idem-{repeats}");
        let mut arns = BTreeSet::new();
        for _ in 0..*repeats {
            // Every call sends the SAME request, which is what a retrying client does.
            let (status, body) = emulator
                .create_app_instance("idempotent-app", Some("m"), &token)
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 on every replay, got {status} {body:?}"
            );
            arns.insert(
                body["AppInstanceArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ARN in {body:?}"))
                    .to_string(),
            );
        }
        assert_eq!(
            arns.len(),
            1,
            "case {case:?}: expected one ARN across {repeats} replays of one token, got {arns:?}"
        );
        minted.extend(arns);
    }

    let listed = emulator.list_app_instances().await;
    assert_eq!(
        listed.len(),
        repeats.len(),
        "expected one instance per distinct token ({}), got {listed:?}",
        repeats.len()
    );
    for arn in &minted {
        assert!(
            listed.contains(arn),
            "expected {arn:?} among the listed instances {listed:?}"
        );
    }

    // The same token on a different operation is a different request, and a user's
    // token is tracked separately from an instance's.
    let instance = minted.first().expect("a minted instance");
    let user_one = emulator.user(instance, "user1", "tok-idem-1").await;
    let user_two = emulator.user(instance, "user1", "tok-idem-1").await;
    assert_eq!(
        user_one, user_two,
        "expected the replayed CreateAppInstanceUser to answer with the same user ARN"
    );
    assert_eq!(
        emulator.list_app_instances().await.len(),
        repeats.len(),
        "a CreateAppInstanceUser token must not disturb the instances"
    );

    // A user's ARN is derived from the request, so one token replayed for a DIFFERENT
    // user is a mismatched reuse, not a retry: it is refused and creates nothing.
    for (case, parent, user_id) in [
        ("a different user id", instance.clone(), "user2"),
        (
            "a different parent instance",
            minted.get(1).expect("a second minted instance").clone(),
            "user1",
        ),
    ] {
        let (status, body) = emulator
            .create_user(&parent, user_id, "Mismatched", None, None, "tok-idem-1")
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a mismatched token reuse to conflict, got {status} {body:?}"
        );
        let (status, described) = emulator
            .describe_user(&format!("{parent}/user/{user_id}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refused create must not have minted anything, got \
             {status} {described:?}"
        );
    }
}

/// RULE: an `AppInstanceUser` is stored under the parent `AppInstance` named at create
/// time, its ARN is that parent's ARN plus `/user/<AppInstanceUserId>`, and
/// `DescribeAppInstanceUser` reports back the `Name`, `Metadata` and
/// `ExpirationSettings` that created it.
#[tokio::test]
async fn describe_app_instance_user_reports_what_create_persisted() {
    // Rows span `UserId` (len 1..64, `^[A-Za-z0-9]([A-Za-z0-9:\-_.@]{0,62}[A-Za-z0-9])?$`),
    // `UserName` (len 1..100), optional `Metadata`, and `ExpirationSettings`
    // (`ExpirationDays` range 1..5475, `ExpirationCriterion` enum value
    // `CREATED_TIMESTAMP`) — all from smithy_query on this service's model.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, String, String, Option<String>, Option<(i32, &str)>)> = vec![
        (
            "plain id, no metadata, no expiry",
            "user1".to_string(),
            "User One".to_string(),
            None,
            None,
        ),
        (
            "id with the punctuation the pattern allows",
            "a.b-c_d@e:1".to_string(),
            "Punctuated".to_string(),
            Some(String::new()),
            None,
        ),
        (
            "expiry at the lower bound of ExpirationDays",
            "shortlived".to_string(),
            "Short Lived".to_string(),
            Some("m1".to_string()),
            Some((1, "CREATED_TIMESTAMP")),
        ),
        (
            "expiry at the upper bound of ExpirationDays",
            "longlived".to_string(),
            "Long Lived".to_string(),
            Some("m2".to_string()),
            Some((5475, "CREATED_TIMESTAMP")),
        ),
        (
            "id and name at their upper length bounds",
            format!("u{}", "0".repeat(63)),
            "n".repeat(100),
            Some("m".repeat(1024)),
            Some((30, "CREATED_TIMESTAMP")),
        ),
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("parent-app", "tok-user-parent").await;

    for (index, (case, user_id, name, metadata, expiration)) in cases.iter().enumerate() {
        let token = format!("tok-user-{index}");
        let (status, created) = emulator
            .create_user(
                &instance,
                user_id,
                name,
                metadata.as_deref(),
                *expiration,
                &token,
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateAppInstanceUser to answer 201, got {status} {created:?}"
        );
        let expected_arn = format!("{instance}/user/{user_id}");
        assert_eq!(
            created["AppInstanceUserArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the user ARN {expected_arn:?}, got {created:?}"
        );

        let (status, body) = emulator.describe_user(&expected_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAppInstanceUser to answer 200, got {status} {body:?}"
        );
        let user = &body["AppInstanceUser"];
        assert_eq!(
            user["AppInstanceUserArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the described ARN {expected_arn:?}, got {user:?}"
        );
        assert_eq!(
            user["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected Name {name:?}, got {:?}",
            user["Name"]
        );
        assert_eq!(
            user["Metadata"].as_str(),
            metadata.as_deref(),
            "case {case:?}: expected Metadata {metadata:?}, got {:?}",
            user["Metadata"]
        );
        match expiration {
            Some((days, criterion)) => {
                assert_eq!(
                    (
                        user["ExpirationSettings"]["ExpirationDays"].as_i64(),
                        user["ExpirationSettings"]["ExpirationCriterion"].as_str()
                    ),
                    (Some(i64::from(*days)), Some(*criterion)),
                    "case {case:?}: expected ExpirationSettings {expiration:?}, got {:?}",
                    user["ExpirationSettings"]
                );
            }
            None => assert!(
                user["ExpirationSettings"].is_null(),
                "case {case:?}: expected no ExpirationSettings, got {:?}",
                user["ExpirationSettings"]
            ),
        }
        assert!(
            user["CreatedTimestamp"].as_f64().is_some()
                && user["LastUpdatedTimestamp"].as_f64() == user["CreatedTimestamp"].as_f64(),
            "case {case:?}: expected both timestamps populated and equal on a fresh user, got {user:?}"
        );
    }

    let listed = emulator.list_users(&instance).await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "expected {} persisted users of {instance:?}, got {listed:?}",
        cases.len()
    );
}

/// Which list operation a pagination case drives.
#[derive(Debug, Clone, Copy)]
enum Listing {
    Instances,
    Users,
}

/// One page of a listing: the ARNs it carried and the token that resumes after it.
async fn page(
    emulator: &Emulator,
    listing: Listing,
    parent: &str,
    max_results: i32,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (path, member) = match listing {
        Listing::Instances => ("/app-instances".to_string(), "AppInstanceArn"),
        Listing::Users => (
            format!(
                "/app-instance-users?app-instance-arn={}",
                uri_encode(parent)
            ),
            "AppInstanceUserArn",
        ),
    };
    let separator = if path.contains('?') { '&' } else { '?' };
    let mut query = format!("{path}{separator}max-results={max_results}");
    if let Some(token) = next_token {
        query.push_str(&format!("&next-token={}", uri_encode(token)));
    }

    let (status, body) = emulator.call("GET", &query, None).await;
    assert_eq!(status, 200, "{listing:?}: {query} answered {status} {body:?}");
    if matches!(listing, Listing::Users) {
        assert_eq!(
            body["AppInstanceArn"].as_str(),
            Some(parent),
            "{listing:?}: expected the echoed AppInstanceArn {parent:?}, got {body:?}"
        );
    }
    let list = match listing {
        Listing::Instances => &body["AppInstances"],
        Listing::Users => &body["AppInstanceUsers"],
    };
    (
        arns_of(list, member),
        body["NextToken"].as_str().map(str::to_string),
    )
}

/// RULE: a paginated list returns at most `MaxResults` entries per page in a stable
/// key order, emits a `NextToken` only while entries remain, and resuming from that
/// token yields every remaining entry exactly once.
#[tokio::test]
async fn a_paginated_listing_partitions_the_whole_collection() {
    // Rows vary the collection size against the page size, including a page size
    // equal to the collection (so the first page is also the last) and both ends of
    // the model's `MaxResults` range (min 1, max 50).
    let cases: &[(&str, usize, i32)] = &[
        ("one entry, page size 1", 1, 1),
        ("two entries, page size 1", 2, 1),
        ("three entries, page size 2", 3, 2),
        ("five entries, page size exactly five", 5, 5),
        ("seven entries, page size 3", 7, 3),
        ("four entries, page size at the upper bound", 4, 50),
    ];

    for listing in [Listing::Instances, Listing::Users] {
        for (case, total, max_results) in cases {
            let emulator = Emulator::new();
            let parent = emulator.app_instance("paging-parent", "tok-paging").await;

            let mut expected: BTreeSet<String> = BTreeSet::new();
            for n in 0..*total {
                let arn = match listing {
                    Listing::Instances => {
                        emulator
                            .app_instance(&format!("app-{n}"), &format!("tok-page-{n}"))
                            .await
                    }
                    Listing::Users => {
                        emulator
                            .user(&parent, &format!("user{n:02}"), &format!("tok-page-{n}"))
                            .await
                    }
                };
                expected.insert(arn);
            }
            if matches!(listing, Listing::Instances) {
                // The parent created above is an instance too, so it is part of the
                // collection this listing pages over.
                expected.insert(parent.clone());
            }

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let (arns, next) = page(&emulator, listing, &parent, *max_results, token.as_deref()).await;
                pages += 1;
                assert!(
                    arns.len() <= *max_results as usize,
                    "case {case:?} ({listing:?}): page {pages} carried {} entries, more than \
                     MaxResults {max_results}",
                    arns.len()
                );
                assert!(
                    next.is_none() || arns.len() == *max_results as usize,
                    "case {case:?} ({listing:?}): page {pages} carried {} entries yet still \
                     emitted a NextToken",
                    arns.len()
                );
                seen.extend(arns);
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?} ({listing:?}): paging did not terminate after {pages} pages"
                );
            }

            assert!(
                seen.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?} ({listing:?}): expected a strictly increasing key order \
                 across pages, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {case:?} ({listing:?}): the pages did not cover the collection exactly once \
                 (saw {seen:?})"
            );
            assert_eq!(
                pages,
                expected.len().div_ceil(*max_results as usize),
                "case {case:?} ({listing:?}): expected {} page(s) for {} entries at \
                 MaxResults {max_results}, got {pages}",
                expected.len().div_ceil(*max_results as usize),
                expected.len()
            );
        }
    }
}

/// RULE: a `MaxResults` outside the modelled range, and a `NextToken` this service
/// never issued, are refused with a 4xx client error rather than served.
#[tokio::test]
async fn a_listing_refuses_a_page_request_it_cannot_honour() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("paging-bounds", "tok-bounds").await;
    let encoded = uri_encode(&instance);

    // `MaxResults` is `range(min: 1, max: 50)`, so one past each published bound is
    // refused and each bound itself is served.
    let cases: &[(&str, i32, bool)] = &[
        ("one below the lower bound", 0, false),
        ("the lower bound", 1, true),
        ("the upper bound", 50, true),
        ("one above the upper bound", 51, false),
    ];
    for (case, max_results, served) in cases {
        let (status, body) = emulator
            .call(
                "GET",
                &format!("/app-instance-users?app-instance-arn={encoded}&max-results={max_results}"),
                None,
            )
            .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected MaxResults {max_results} to be served, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected MaxResults {max_results} to be refused with 400, \
                 got {status} {body:?}"
            );
        }
    }

    for (case, token) in [
        ("not an encoded token at all", "not-a-token!!".to_string()),
        ("an encoding of something that is not a token", "68656c6c6f".to_string()),
    ] {
        let (status, body) = emulator
            .call(
                "GET",
                &format!("/app-instance-users?app-instance-arn={encoded}&next-token={token}"),
                None,
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected an unissued NextToken to be refused, got {status} {body:?}"
        );
    }

    // A token is scoped to the listing that issued it: one from ListAppInstances, or
    // from another instance's user listing, does not resume this one.
    let other = emulator.app_instance("paging-bounds-2", "tok-bounds2").await;
    emulator.user(&instance, "u0", "tok-bounds-u0").await;
    emulator.user(&instance, "u1", "tok-bounds-u1").await;
    emulator.user(&other, "u0", "tok-bounds2-u0").await;
    emulator.user(&other, "u1", "tok-bounds2-u1").await;

    let (_, instances_token) = page(&emulator, Listing::Instances, &instance, 1, None).await;
    let (_, other_users_token) = page(&emulator, Listing::Users, &other, 1, None).await;
    for (case, token) in [
        ("a ListAppInstances token", instances_token),
        ("another instance's user token", other_users_token),
    ] {
        let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a NextToken"));
        let (status, body) = emulator
            .call(
                "GET",
                &format!(
                    "/app-instance-users?app-instance-arn={encoded}&next-token={}",
                    uri_encode(&token)
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a token from another listing to be refused, \
             got {status} {body:?}"
        );
    }
}

/// RULE: an operation keyed by the ARN of a resource that does not exist is refused
/// with the modelled client error carrying the `NotFound` code — never a hollow success.
#[tokio::test]
async fn an_unknown_arn_is_refused_rather_than_answered() {
    let emulator = Emulator::new();
    let live = emulator.app_instance("live-app", "tok-unknown").await;
    let absent_instance = format!("{live}x");
    let absent_user = format!("{live}/user/nobody");

    let cases: Vec<(&str, &str, String)> = vec![
        ("describe a missing instance", "GET", format!("/app-instances/{}", uri_encode(&absent_instance))),
        ("delete a missing instance", "DELETE", format!("/app-instances/{}", uri_encode(&absent_instance))),
        ("describe a missing user", "GET", format!("/app-instance-users/{}", uri_encode(&absent_user))),
        ("delete a missing user", "DELETE", format!("/app-instance-users/{}", uri_encode(&absent_user))),
        (
            "list the users of a missing instance",
            "GET",
            format!("/app-instance-users?app-instance-arn={}", uri_encode(&absent_instance)),
        ),
    ];

    for (case, method, path) in cases {
        let (status, body) = emulator.call(method, &path, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an absent resource, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("NotFound"),
            "case {case:?}: expected the NotFound error code, got {body:?}"
        );
    }

    // Creating a user under a parent that does not exist is the same refusal, and
    // leaves nothing behind.
    let (status, body) = emulator
        .create_user(&absent_instance, "orphan", "Orphan", None, None, "tok-orphan")
        .await;
    assert_eq!(
        status, 400,
        "expected CreateAppInstanceUser under an absent parent to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("NotFound"),
        "expected the NotFound error code, got {body:?}"
    );
    let (status, body) = emulator
        .describe_user(&format!("{absent_instance}/user/orphan"))
        .await;
    assert_eq!(
        status, 400,
        "the refused user must not exist afterwards, got {status} {body:?}"
    );
}

/// RULE: an `AppInstanceUserId` is unique within its `AppInstance`, so creating it
/// again under a NEW `ClientRequestToken` is a conflict (409) — while the same id in a
/// DIFFERENT instance is a different resource and is created.
#[tokio::test]
async fn a_duplicate_user_id_conflicts_within_one_instance_only() {
    let emulator = Emulator::new();
    let first = emulator.app_instance("first-app", "tok-conflict-1").await;
    let second = emulator.app_instance("second-app", "tok-conflict-2").await;

    // Rows differ in the id's form, so the rule is not keyed to one shape of id.
    for (index, (case, user_id)) in [
        ("plain id", "dup"),
        ("id with punctuation", "a.b-c_d@e:1"),
        ("numeric id", "0123456789"),
    ]
    .into_iter()
    .enumerate()
    {
        // A `ClientRequestToken` is `^[-_a-zA-Z0-9]*$`, so it is built from the row
        // index rather than from the id under test.
        let original = emulator
            .user(&first, user_id, &format!("tok-dup-first-{index}"))
            .await;

        let (status, body) = emulator
            .create_user(
                &first,
                user_id,
                "Another Name",
                None,
                None,
                &format!("tok-dup-again-{index}"),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a 409 conflict for a repeated id under one instance, \
             got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("Conflict"),
            "case {case:?}: expected the Conflict error code, got {body:?}"
        );

        // The conflict changed nothing: the original user still reads back.
        let (status, described) = emulator.describe_user(&original).await;
        assert_eq!(
            (status, described["AppInstanceUser"]["Name"].as_str()),
            (200, Some(user_id)),
            "case {case:?}: the refused create must not have altered {original:?}"
        );

        // The same id under another instance is a different resource.
        let elsewhere = emulator
            .user(&second, user_id, &format!("tok-dup-second-{index}"))
            .await;
        assert_ne!(
            original, elsewhere,
            "case {case:?}: the same id in two instances must be two ARNs"
        );
    }
}

/// RULE: a delete removes state — the deleted resource stops being described and
/// stops being listed, deleting an `AppInstance` cascades to the users it owns, and
/// everything NOT deleted is untouched.
#[tokio::test]
async fn delete_removes_the_resource_and_cascades_to_its_users() {
    let emulator = Emulator::new();
    let kept_instance = emulator.app_instance("kept-app", "tok-del-kept").await;
    let kept_user = emulator.user(&kept_instance, "keeper", "tok-del-keeper").await;

    // Rows differ in how many users the deleted instance owns, so the cascade is not
    // tested only at one count.
    for (case, user_count) in [("no users", 0usize), ("one user", 1), ("several users", 4)] {
        let doomed = emulator
            .app_instance(&format!("doomed-{user_count}"), &format!("tok-del-{user_count}"))
            .await;
        let mut users = Vec::new();
        for n in 0..user_count {
            users.push(
                emulator
                    .user(&doomed, &format!("u{n}"), &format!("tok-del-{user_count}-{n}"))
                    .await,
            );
        }

        // Deleting one user removes just that one.
        if let Some(first) = users.first().cloned() {
            let (status, body) = emulator.delete_user(&first).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected DeleteAppInstanceUser to answer 204, got {status} {body:?}"
            );
            let (status, body) = emulator.describe_user(&first).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a deleted user must not describe, got {status} {body:?}"
            );
            let listed = emulator.list_users(&doomed).await;
            assert!(
                !listed.contains(&first),
                "case {case:?}: a deleted user must not be listed, got {listed:?}"
            );
            assert_eq!(
                listed.len(),
                user_count - 1,
                "case {case:?}: expected {} remaining users, got {listed:?}",
                user_count - 1
            );
            // Deleting it twice: the second call has nothing to delete.
            let (status, body) = emulator.delete_user(&first).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a second delete of one user must be refused, got {status} {body:?}"
            );
            users.remove(0);
        }

        let (status, body) = emulator.delete_app_instance(&doomed).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected DeleteAppInstance to answer 204, got {status} {body:?}"
        );
        let (status, body) = emulator.describe_app_instance(&doomed).await;
        assert_eq!(
            (status, body["Code"].as_str()),
            (403, Some("Forbidden")),
            "case {case:?}: a deleted instance must not describe, got {status} {body:?}"
        );
        let instances = emulator.list_app_instances().await;
        assert!(
            !instances.contains(&doomed),
            "case {case:?}: a deleted instance must not be listed, got {instances:?}"
        );
        for user in &users {
            let (status, body) = emulator.describe_user(user).await;
            assert_eq!(
                (status, body["Code"].as_str()),
                (403, Some("Forbidden")),
                "case {case:?}: deleting the instance must cascade to {user:?}, got {status} {body:?}"
            );
        }

        // The untouched instance and its user are unaffected by any of it.
        let (status, described) = emulator.describe_app_instance(&kept_instance).await;
        assert_eq!(
            (status, described["AppInstance"]["Name"].as_str()),
            (200, Some("kept-app")),
            "case {case:?}: the untouched instance must still describe"
        );
        assert_eq!(
            emulator.list_users(&kept_instance).await,
            vec![kept_user.clone()],
            "case {case:?}: the untouched instance must still own exactly its own user"
        );
    }
}

/// RULE: the store is bounded — a create past the emulator's capacity is refused with
/// the modelled `ResourceLimitExceededException`, and the capacity is released again
/// by a delete, so the store never grows past it.
#[tokio::test]
async fn a_create_past_capacity_is_refused_and_a_delete_releases_it() {
    // Both bounds are exercised: the per-instance user count and the per-account
    // instance count. Each row fills its collection to the bound, checks that one
    // more is refused, then frees one and checks that one more is accepted again.
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bounded-app", "tok-cap-parent").await;

    let mut last_user = String::new();
    for n in 0..MAX_APP_INSTANCE_USERS {
        last_user = emulator
            .user(&instance, &format!("u{n:06}"), &format!("tok-cap-u{n}"))
            .await;
    }
    let (status, body) = emulator
        .create_user(&instance, "overflow", "Overflow", None, None, "tok-cap-over")
        .await;
    assert_eq!(
        status, 400,
        "expected the {MAX_APP_INSTANCE_USERS}-user bound to refuse one more, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("ResourceLimitExceeded"),
        "expected the ResourceLimitExceeded error code, got {body:?}"
    );
    let (status, _) = emulator.delete_user(&last_user).await;
    assert_eq!(status, 204, "expected the freeing delete to answer 204");
    let (status, body) = emulator
        .create_user(&instance, "overflow", "Overflow", None, None, "tok-cap-over2")
        .await;
    assert_eq!(
        status, 201,
        "expected the freed slot to accept one more user, got {status} {body:?}"
    );

    // One instance already exists, so the bound is reached after the rest are made.
    let mut last_instance = instance.clone();
    for n in 1..MAX_APP_INSTANCES {
        last_instance = emulator
            .app_instance(&format!("app-{n}"), &format!("tok-cap-a{n}"))
            .await;
    }
    let (status, body) = emulator
        .create_app_instance("overflow", None, "tok-cap-aover")
        .await;
    assert_eq!(
        status, 400,
        "expected the {MAX_APP_INSTANCES}-instance bound to refuse one more, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("ResourceLimitExceeded"),
        "expected the ResourceLimitExceeded error code, got {body:?}"
    );
    let (status, _) = emulator.delete_app_instance(&last_instance).await;
    assert_eq!(status, 204, "expected the freeing delete to answer 204");
    let (status, body) = emulator
        .create_app_instance("overflow", None, "tok-cap-aover2")
        .await;
    assert_eq!(
        status, 201,
        "expected the freed slot to accept one more instance, got {status} {body:?}"
    );
}

/// RULE: an idempotency token lives exactly as long as the resource it minted — once
/// that resource is deleted the token no longer replays it, so a create/delete cycle
/// leaves nothing behind.
#[tokio::test]
async fn a_deleted_resource_releases_its_idempotency_token() {
    let emulator = Emulator::new();

    // Rows: the token of a deleted instance, and the token of a user deleted both
    // directly and by its parent's cascade.
    let first = emulator.app_instance("recreate-me", "tok-release").await;
    assert_eq!(emulator.delete_app_instance(&first).await.0, 204);
    let second = emulator.app_instance("recreate-me", "tok-release").await;
    assert_ne!(
        first, second,
        "expected the replayed token to mint a new instance once the first was deleted"
    );

    for (case, delete_the_parent) in [("the user itself", false), ("its parent", true)] {
        let user = emulator.user(&second, "released", "tok-release-user").await;
        if delete_the_parent {
            // This is the last row, so the parent is not needed again.
            assert_eq!(
                emulator.delete_app_instance(&second).await.0,
                204,
                "case {case:?}: expected the parent delete to answer 204"
            );
        } else {
            assert_eq!(
                emulator.delete_user(&user).await.0,
                204,
                "case {case:?}: expected the user delete to answer 204"
            );
            let recreated = emulator.user(&second, "released", "tok-release-user").await;
            assert_eq!(
                recreated, user,
                "case {case:?}: a re-created user of the same id has the same ARN"
            );
            let (status, described) = emulator.describe_user(&recreated).await;
            assert_eq!(
                (status, described["AppInstanceUser"]["Name"].as_str()),
                (200, Some("released")),
                "case {case:?}: the re-created user must describe"
            );
            assert_eq!(
                emulator.delete_user(&recreated).await.0,
                204,
                "case {case:?}: tidy up for the next row"
            );
        }
    }
}

/// RULE: the store holds one record, and one idempotency entry, per LIVE resource —
/// a refused create adds neither, so a create loop driven by request input cannot
/// grow the store past the resources that actually exist.
#[tokio::test]
async fn refused_creates_add_nothing_to_the_store() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("stable-app", "tok-grow-parent").await;
    let user = emulator.user(&instance, "stable", "tok-grow-user").await;
    let baseline = emulator.store_sizes();
    assert_eq!(
        baseline,
        (1, 1, 2),
        "expected one instance, one user and one token each, got {baseline:?}"
    );

    // Rows: the same create replayed with a FRESH token every time (a conflict), and
    // a create against a parent that does not exist (a refusal). Neither mints
    // anything, so neither may leave a record or a token behind.
    let absent_parent = format!("{instance}x");
    for n in 0..100 {
        let (status, body) = emulator
            .create_user(
                &instance,
                "stable",
                "Stable",
                None,
                None,
                &format!("tok-grow-dup-{n}"),
            )
            .await;
        assert_eq!(
            status, 409,
            "replay {n}: expected a conflict for an id that already exists, got {status} {body:?}"
        );
        let (status, body) = emulator
            .create_user(
                &absent_parent,
                "orphan",
                "Orphan",
                None,
                None,
                &format!("tok-grow-orphan-{n}"),
            )
            .await;
        assert_eq!(
            status, 400,
            "replay {n}: expected a refusal for an absent parent, got {status} {body:?}"
        );
    }

    let after = emulator.store_sizes();
    assert_eq!(
        after, baseline,
        "200 refused creates with distinct ClientRequestTokens must leave the store \
         exactly as it was (instances, users, idempotency entries)"
    );
    let (instances, users, tokens) = after;
    assert_eq!(
        tokens,
        instances + users,
        "expected one idempotency entry per live resource, got {tokens} for \
         {instances} instance(s) and {users} user(s)"
    );
    assert!(
        tokens <= MAX_IDEMPOTENCY_TOKENS,
        "expected at most {MAX_IDEMPOTENCY_TOKENS} idempotency entries, got {tokens}"
    );

    // And the resource the refused creates named is untouched.
    let (status, described) = emulator.describe_user(&user).await;
    assert_eq!(
        (status, described["AppInstanceUser"]["Name"].as_str()),
        (200, Some("stable")),
        "the conflicting creates must not have altered the stored user"
    );
}

/// RULE: the id in a minted `AppInstanceArn` is a version-4 UUID, and every create
/// mints a fresh one.
#[tokio::test]
async fn a_minted_app_instance_id_is_a_fresh_v4_uuid() {
    let emulator = Emulator::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for n in 0..20 {
        let arn = emulator
            .app_instance("uuid-app", &format!("tok-uuid-{n}"))
            .await;
        let id = arn
            .split('/')
            .next_back()
            .unwrap_or_else(|| panic!("create {n}: {arn:?} carries no resource id"))
            .to_string();

        // RFC 4122: 8-4-4-4-12 lowercase hex, the version nibble (first of group 3)
        // is `4`, and the variant nibble (first of group 4) is one of 8, 9, a, b.
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "create {n}: expected a 8-4-4-4-12 uuid, got {id:?}"
        );
        assert!(
            id.chars()
                .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase())),
            "create {n}: expected lowercase hex, got {id:?}"
        );
        assert_eq!(
            groups[2].as_bytes().first(),
            Some(&b'4'),
            "create {n}: expected the version-4 nibble, got {id:?}"
        );
        assert!(
            matches!(groups[3].as_bytes().first(), Some(b'8' | b'9' | b'a' | b'b')),
            "create {n}: expected the RFC 4122 variant nibble, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "create {n}: expected a fresh id per create, saw {id:?} twice"
        );
    }
}

/// The `Message` this service words a `NextToken` it did not issue with, on every one
/// of its listings.
const UNISSUED_TOKEN_MESSAGE: &str = "NextToken is not a token this listing issued";

/// The width of the authenticator a `NextToken` this service mints carries ahead of its
/// cursor, from `service_common::page_tokens::AUTHENTICATOR_BYTES`: a value with the
/// SHAPE of a minted token is that many bytes of authenticator followed by a cursor.
const AUTHENTICATOR_BYTES: usize = service_common::page_tokens::AUTHENTICATOR_BYTES;

/// One paginated listing of this service, as a caller reaches it.
struct PagedListing {
    /// The operation it serves, which is also the first field of its token's scope.
    operation: &'static str,
    /// Everything before the query string of the request that drives it.
    path: String,
    /// The query parameter the listing REQUIRES (the parent it pages under), already
    /// percent-encoded, or empty for a listing that pages the account's own collection.
    required_query: String,
    /// The scope a token of this listing carries, which a caller knows in full: the
    /// operation name, plus for a per-parent listing the parent ARN it just supplied.
    scope: String,
    /// The response member carrying the page.
    member: &'static str,
    /// Where a summary of this listing carries the key the collection is ordered by —
    /// the cursor a token of it resumes after.
    key_path: &'static [&'static str],
}

impl PagedListing {
    /// This listing with `parameters` appended to the query it requires.
    async fn call(&self, emulator: &Emulator, parameters: &str) -> (u16, Value) {
        let mut query = self.required_query.clone();
        if !parameters.is_empty() {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(parameters);
        }
        let path_and_query = if query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{query}", self.path)
        };
        emulator.call("GET", &path_and_query, None).await
    }

    /// One page of this listing at `max_results`, resuming after `next_token`.
    async fn page(
        &self,
        emulator: &Emulator,
        max_results: i32,
        next_token: Option<&str>,
    ) -> (u16, Value) {
        let mut parameters = format!("max-results={max_results}");
        if let Some(token) = next_token {
            parameters.push_str(&format!("&next-token={}", uri_encode(token)));
        }
        self.call(emulator, &parameters).await
    }

    /// The keys a page carried, in the order the listing reported them.
    fn keys(&self, body: &Value) -> Vec<String> {
        body[self.member]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let mut value = item;
                        for member in self.key_path {
                            value = &value[*member];
                        }
                        value
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!(
                                    "{}: a summary carrying no {:?}: {item:?}",
                                    self.operation, self.key_path
                                )
                            })
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Register one endpoint under a user, failing the test if it was refused. `ResourceArn`
/// is the Amazon Pinpoint application ARN
/// `smithy_query(query="operation",operation="RegisterAppInstanceUserEndpoint")`
/// documents, whose application id is 32 hexadecimal characters.
async fn register_endpoint(emulator: &Emulator, user_arn: &str, token: &str, seed: usize) {
    let (status, body) = emulator
        .call(
            "POST",
            &format!("/app-instance-users/{}/endpoints", uri_encode(user_arn)),
            Some(json!({
                "ClientRequestToken": token,
                "Type": "APNS",
                "ResourceArn": format!(
                    "arn:aws:mobiletargeting:us-east-1:123456789012:apps/{:032x}",
                    seed + 1
                ),
                "EndpointAttributes": { "DeviceToken": format!("device-token-{seed}") },
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "RegisterAppInstanceUserEndpoint({token}) answered {status} {body:?}"
    );
}

/// Promote one identity of an app instance to administrator, failing the test if it was
/// refused.
async fn promote_admin(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) {
    let (status, body) = emulator
        .call(
            "POST",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            Some(json!({ "AppInstanceAdminArn": admin_arn })),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateAppInstanceAdmin({admin_arn}) answered {status} {body:?}"
    );
}

/// Every paginated listing this service serves, over a store whose collections each hold
/// three entries: more than one page at `MaxResults` 1, so each listing both issues a
/// token and has a position after it that a forged token could name.
///
/// `suffix` distinguishes one store's resources from another's, so two stores built by
/// this helper never agree on an idempotency token.
async fn paginated_listings(emulator: &Emulator, suffix: &str) -> Vec<PagedListing> {
    let instance = emulator
        .app_instance(&format!("forged-{suffix}"), &format!("tok-forged-{suffix}"))
        .await;
    // Two more instances, so the account-wide listing pages as well.
    for index in 0..2 {
        emulator
            .app_instance(
                &format!("forged-{suffix}-{index}"),
                &format!("tok-forged-{suffix}-{index}"),
            )
            .await;
    }
    let mut users = Vec::new();
    for index in 0..3 {
        users.push(
            emulator
                .user(
                    &instance,
                    &format!("u{index}"),
                    &format!("tok-forged-{suffix}-u{index}"),
                )
                .await,
        );
    }
    for index in 0..3 {
        crate::app_instance_bots::bot(
            emulator,
            &instance,
            &format!("tok-forged-{suffix}-b{index}"),
            &format!("bot{index}"),
        )
        .await;
    }
    for index in 0..3 {
        register_endpoint(
            emulator,
            &users[0],
            &format!("tok-forged-{suffix}-e{index}"),
            index,
        )
        .await;
    }
    for user in &users {
        promote_admin(emulator, &instance, user).await;
    }

    let encoded_instance = uri_encode(&instance);
    vec![
        PagedListing {
            operation: "ListAppInstances",
            path: "/app-instances".to_string(),
            required_query: String::new(),
            scope: "ListAppInstances".to_string(),
            member: "AppInstances",
            key_path: &["AppInstanceArn"],
        },
        PagedListing {
            operation: "ListAppInstanceUsers",
            path: "/app-instance-users".to_string(),
            required_query: format!("app-instance-arn={encoded_instance}"),
            scope: format!("ListAppInstanceUsers:{instance}"),
            member: "AppInstanceUsers",
            key_path: &["AppInstanceUserArn"],
        },
        PagedListing {
            operation: "ListAppInstanceBots",
            path: "/app-instance-bots".to_string(),
            required_query: format!("app-instance-arn={encoded_instance}"),
            scope: format!("ListAppInstanceBots:{instance}"),
            member: "AppInstanceBots",
            key_path: &["AppInstanceBotArn"],
        },
        PagedListing {
            operation: "ListAppInstanceUserEndpoints",
            path: format!("/app-instance-users/{}/endpoints", uri_encode(&users[0])),
            required_query: String::new(),
            scope: format!("ListAppInstanceUserEndpoints:{}", users[0]),
            member: "AppInstanceUserEndpoints",
            key_path: &["EndpointId"],
        },
        PagedListing {
            operation: "ListAppInstanceAdmins",
            path: format!("/app-instances/{encoded_instance}/admins"),
            required_query: String::new(),
            scope: format!("ListAppInstanceAdmins:{instance}"),
            member: "AppInstanceAdmins",
            key_path: &["Admin", "Arn"],
        },
    ]
}

/// The token an UNauthenticated codec renders for a position: hex of the three fields a
/// caller knows — the version marker, the listing's scope and a cursor it reads off the
/// page it was handed.
fn plaintext_token(scope: &str, cursor: &str) -> String {
    hex::encode(format!("v1\n{scope}\n{cursor}"))
}

/// A value with the SHAPE of a minted token: an authenticator of `filler` bytes ahead of
/// the plaintext fields a caller knows.
fn shaped_like_minted(filler: u8, scope: &str, cursor: &str) -> String {
    let mut bytes = vec![filler; AUTHENTICATOR_BYTES];
    bytes.extend_from_slice(format!("v1\n{scope}\n{cursor}").as_bytes());
    hex::encode(bytes)
}

/// A minted token with one byte of it changed.
fn with_byte_changed(token: &str, index: usize) -> String {
    let mut bytes = hex::decode(token).expect("a token this service minted is hex");
    bytes[index] ^= 0x01;
    hex::encode(bytes)
}

/// RULE: a continuation token is one the serving listing ISSUED or it names no position
/// at all — a value a caller assembled from the fields it knows, one another store
/// minted, one another listing minted, or one changed on the way back is refused as bad
/// input rather than served as a page.
#[tokio::test]
async fn only_a_token_a_listing_itself_issued_resumes_it() {
    let emulator = Emulator::new();
    let listings = paginated_listings(&emulator, "a").await;
    // A SECOND store, whose listings mint tokens of exactly the same form, under a key
    // of its own.
    let other_store = Emulator::new();
    let other_listings = paginated_listings(&other_store, "b").await;

    // What each listing of this store actually issued: the token off its first page and
    // the cursor that token resumes after.
    let mut issued: Vec<(String, String)> = Vec::new();
    for listing in &listings {
        let operation = listing.operation;
        let (status, first) = listing.page(&emulator, 1, None).await;
        assert_eq!(
            status, 200,
            "{operation}: a first page of one entry answered {status} {first:?}"
        );
        let first_keys = listing.keys(&first);
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("{operation}: a first page of one entry issued no NextToken: {first:?}")
            })
            .to_string();

        // The token the listing issued resumes it, at the position after the page it came
        // with — the behaviour an authenticated token keeps.
        let (status, second) = listing.page(&emulator, 1, Some(&token)).await;
        assert_eq!(
            status, 200,
            "{operation}: the token this listing issued answered {status} {second:?}"
        );
        let second_keys = listing.keys(&second);
        assert!(
            first_keys.len() == 1
                && second_keys.len() == 1
                && first_keys[0] < second_keys[0],
            "{operation}: expected the issued token to resume after {first_keys:?} in key \
             order, got {second_keys:?}"
        );
        issued.push((token, first_keys[0].clone()));
    }

    for (index, listing) in listings.iter().enumerate() {
        let operation = listing.operation;
        let scope = listing.scope.as_str();
        let (token, cursor) = &issued[index];
        // The token of the NEXT listing of this same store, and of the SAME listing of
        // the other store: both minted, neither by this listing.
        let (another_listings_token, _) = &issued[(index + 1) % issued.len()];
        let (other_store_token, _) = {
            let other = &other_listings[index];
            assert_eq!(
                other.operation, operation,
                "the two stores' listings must line up"
            );
            let (status, page) = other.page(&other_store, 1, None).await;
            assert_eq!(
                status, 200,
                "{operation}: the other store's first page answered {status} {page:?}"
            );
            let minted = page["NextToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("{operation}: the other store issued no NextToken: {page:?}")
                })
                .to_string();
            (minted, ())
        };

        // Rows span the ways a value that is not a token this listing issued can reach
        // it: the plaintext rendering of the fields a caller knows (the version marker,
        // the scope it just addressed and a cursor off its own page), that rendering at
        // positions nobody was offered, a value with a minted token's shape and a
        // made-up authenticator, a minted token with a byte changed, truncated or
        // extended, a token minted for another listing or by another store, and values
        // that are no token at all.
        let forged: Vec<(String, String)> = vec![
            (
                "the plaintext fields with no authenticator".to_string(),
                plaintext_token(scope, cursor),
            ),
            (
                "plaintext naming a cursor past every key".to_string(),
                plaintext_token(scope, "~"),
            ),
            (
                "plaintext naming an empty cursor".to_string(),
                plaintext_token(scope, ""),
            ),
            (
                "an all-zero authenticator ahead of the cursor".to_string(),
                shaped_like_minted(0x00, scope, cursor),
            ),
            (
                "an all-ones authenticator ahead of the cursor".to_string(),
                shaped_like_minted(0xff, scope, cursor),
            ),
            (
                "the issued token with a byte of its authenticator changed".to_string(),
                with_byte_changed(token, 0),
            ),
            (
                "the issued token with a byte of its cursor changed".to_string(),
                with_byte_changed(token, hex::decode(token).expect("hex").len() - 1),
            ),
            ("the issued token truncated by a byte".to_string(), {
                let mut bytes = hex::decode(token).expect("hex");
                bytes.pop();
                hex::encode(bytes)
            }),
            ("the issued token with a byte appended".to_string(), {
                let mut bytes = hex::decode(token).expect("hex");
                bytes.push(b'x');
                hex::encode(bytes)
            }),
            (
                "a token another listing of this store minted".to_string(),
                another_listings_token.clone(),
            ),
            (
                "a token another store minted for this same listing".to_string(),
                other_store_token.clone(),
            ),
            ("not an encoded token at all".to_string(), "not-a-token!!".to_string()),
            (
                "hex of bytes that are no token".to_string(),
                hex::encode("hello"),
            ),
        ];

        for (case, value) in forged {
            let (status, body) = listing.page(&emulator, 1, Some(&value)).await;
            assert_eq!(
                (status, body["Code"].as_str(), body["Message"].as_str()),
                (400, Some("BadRequest"), Some(UNISSUED_TOKEN_MESSAGE)),
                "{operation}: case {case:?}: expected a NextToken this listing never \
                 issued to be refused, got {status} {body:?}"
            );
        }

        // The refusals changed nothing: the token this listing really issued still
        // resumes it.
        let (status, resumed) = listing.page(&emulator, 1, Some(token)).await;
        assert_eq!(
            status, 200,
            "{operation}: after the refusals the issued token answered {status} {resumed:?}"
        );
        assert_eq!(
            listing.keys(&resumed).len(),
            1,
            "{operation}: expected the issued token to still resume one entry, got {resumed:?}"
        );
    }
}

/// RULE: a minted ARN carries the region the caller addressed, read from the request's
/// SigV4 credential scope.
#[tokio::test]
async fn a_minted_arn_carries_the_signed_region() {
    // Rows span regions in different partitions' naming forms; the rule is "whatever
    // region the request was signed for", not one specific region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let arn = emulator.app_instance("regional-app", "tok-region").await;
        let expected_prefix = format!("arn:aws:chime:{region}:");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {region:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );

        // And the user ARN hangs off that same instance ARN.
        let user = emulator.user(&arn, "regional", "tok-region-user").await;
        assert_eq!(
            user,
            format!("{arn}/user/regional"),
            "case {region:?}: expected the user ARN to extend its instance ARN"
        );
    }
}
