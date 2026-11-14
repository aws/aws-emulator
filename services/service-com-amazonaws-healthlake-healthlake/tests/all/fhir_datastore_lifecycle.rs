// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the FHIR data store slice, over the real awsJson1_0
//! wire: `CreateFHIRDatastore`, `DescribeFHIRDatastore`, `ListFHIRDatastores`,
//! `TagResource`, `ListTagsForResource` and `DeleteFHIRDatastore`.
//!
//! Every call below goes through [`MultiServiceRouter::dispatch`] — the same
//! entry point `core/router` serves — so routing, deserialization, the modelled
//! constraints and the error envelope are all exercised, not bypassed.
//!
//! Every modelled CLIENT error below is asserted at HTTP 400, whatever exact code its
//! shape's `@httpError` trait names: `@httpError` is an HTTP-binding trait and
//! awsJson1_0 binds nothing to the transport, so this service reports an error's FAULT.
//! The rule itself, over every status the model annotates, is pinned in
//! `tests/all/error_envelopes.rs`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Long enough that every lifecycle transition `crate::state::LIFECYCLE_TRANSITION`
/// owes (500 ms) has fallen due by the time the next call runs.
///
/// `pub(crate)` so the sibling suites in `tests/all/` drive the lifecycle through
/// the same harness rather than each writing its own.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How long [`HealthLake::poll_until_status`] keeps looking for a status the CLOCK owes:
/// many times over `crate::state::LIFECYCLE_TRANSITION`, so a loaded host has room while
/// a fast one never waits for it.
pub(crate) const SETTLING_DEADLINE: Duration = Duration::from_secs(30);

/// The base URL every address this service hands a caller is rooted at: THIS emulator's
/// own, which for a router that is not served on a port is the framework's
/// [`smithy_server_runtime::UNSERVED_URL_BASE`]. A published address names the emulator,
/// never the real service's host, because a caller SENDS its next request to it.
pub(crate) const EMULATOR_BASE: &str = smithy_server_runtime::UNSERVED_URL_BASE;

/// The emulated HealthLake service behind the production router.
pub(crate) struct HealthLake {
    router: MultiServiceRouter,
    /// The SINGLE shared engine the service was built from — the same store
    /// every handler reads and writes.
    ///
    /// `pub(crate)` so a suite can seed, or read back, a record the operation
    /// that MINTS it has not been written yet (`tests/all/fhir_export_job_listing.rs`
    /// seeds export jobs this way), without each suite building its own router.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl HealthLake {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_healthlake_healthlake::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// Dispatch one awsJson1_0 call, returning the HTTP status and the parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("HealthLake.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// Dispatch a call that must succeed, returning its body.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status}: {payload}"
        );
        payload
    }

    /// Create a data store and return `(DatastoreId, DatastoreArn)`.
    pub(crate) async fn create(&self, request: &Value) -> (String, String) {
        let created = self.ok("CreateFHIRDatastore", request).await;
        (
            string_at(&created, "DatastoreId"),
            string_at(&created, "DatastoreArn"),
        )
    }

    /// The `DatastoreProperties` `DescribeFHIRDatastore` reports for `id`.
    ///
    /// `pub(crate)` so every suite in `tests/all/` reads a data store back
    /// through the same helper rather than each growing its own copy.
    pub(crate) async fn describe(&self, id: &str) -> Value {
        self.ok("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
            .await["DatastoreProperties"]
            .clone()
    }

    /// Look at `DescribeFHIRDatastore` until the data store `id` reports
    /// `DatastoreStatus` `status`, and answer that whole response; fail once
    /// [`SETTLING_DEADLINE`] has passed without it.
    ///
    /// A lifecycle transition this service owes is materialised by whichever call reads
    /// the record next, so waiting for one means LOOKING again until the condition holds —
    /// never sleeping for the span, which would make the outcome depend on how fast the
    /// host runs. `pub(crate)` so every suite waits for a status the one way.
    pub(crate) async fn poll_until_status(&self, id: &str, status: &str) -> Value {
        let deadline = std::time::Instant::now() + SETTLING_DEADLINE;
        loop {
            let described = self
                .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
                .await;
            if described["DatastoreProperties"]["DatastoreStatus"] == status {
                return described;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the data store {id} did not reach {status} within {SETTLING_DEADLINE:?}, \
                 last read {described}"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Every data store the service reports, in `ListFHIRDatastores` order.
    pub(crate) async fn list_all(&self, filter: &Value) -> Vec<Value> {
        let mut request = json!({});
        if !filter.is_null() {
            request["Filter"] = filter.clone();
        }
        let listed = self.ok("ListFHIRDatastores", &request).await;
        listed["DatastorePropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// A required string member of a response body.
pub(crate) fn string_at(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("response must carry a string {member}, got {body}"))
        .to_string()
}

/// The `message` of an error envelope, under either casing a protocol uses for
/// it (`Message` on the modelled shapes, `message` on a framework refusal).
///
/// `pub(crate)` beside [`error_code`], so every suite in `tests/all/` reads a
/// refusal's text through the same helper.
pub(crate) fn error_message(payload: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = payload.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("an error body must carry a message, got {payload}")
}

/// The modelled shape name in an awsJson error envelope's `__type`.
pub(crate) fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Seconds since the epoch, for building `CreatedBefore` / `CreatedAfter`.
fn epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// `DatastoreId` values are 32 lowercase hex characters, which is what the
/// modelled data store ARN (`datastore/fhir/.{32}`) and `DatastoreId`'s own
/// 1..32 length bound together require.
fn assert_well_formed_id(case: &str, id: &str) {
    assert_eq!(
        id.len(),
        32,
        "case {case:?}: DatastoreId must be 32 characters, got {id:?}"
    );
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "case {case:?}: DatastoreId must be lowercase hex, got {id:?}"
    );
}

/// The create inputs the round-trip table spans: the rule is that every member
/// `CreateFHIRDatastore` accepts is persisted and reported back by
/// `DescribeFHIRDatastore`, so the rows differ along which optional members are
/// present and along the form of the values (ASCII, unicode, punctuation, a
/// long name, zero/one/many tags).
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal: only the required DatastoreTypeVersion",
            json!({ "DatastoreTypeVersion": "R4" }),
        ),
        (
            "named, one tag",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "demo-store",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "unicode name with spaces, three tags",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "клиника записи 42",
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "owner", "Value": "alice" },
                    { "Key": "cost.center", "Value": "" },
                ],
            }),
        ),
        (
            "name at the modelled 256-character maximum, no tags",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "n".repeat(256),
                "Tags": [],
            }),
        ),
        (
            "customer managed key, Synthea preload, SMART on FHIR identity provider",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "fully.configured:store",
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc",
                    },
                },
                "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                    "Metadata": "{\"issuer\":\"https://example.test\"}",
                },
            }),
        ),
        (
            "AWS owned key, analytics and NLP configuration",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "analytics store",
                "SseConfiguration": {
                    "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" },
                },
                "AnalyticsConfiguration": { "Status": "ENABLED" },
                "NlpConfiguration": { "Status": "DISABLED" },
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "AWS_AUTH",
                    "FineGrainedAuthorizationEnabled": false,
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reports_it_back() {
    let service = HealthLake::new();

    for (case, request) in round_trip_cases() {
        let created = service.ok("CreateFHIRDatastore", &request).await;
        let id = string_at(&created, "DatastoreId");
        assert_well_formed_id(case, &id);

        let arn = string_at(&created, "DatastoreArn");
        assert_eq!(
            arn,
            format!("arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/{id}"),
            "case {case:?}: DatastoreArn must name the minted data store, got {arn:?}"
        );
        let endpoint = string_at(&created, "DatastoreEndpoint");
        assert_eq!(
            endpoint,
            format!("{EMULATOR_BASE}/datastore/{id}/r4/"),
            "case {case:?}: DatastoreEndpoint must name the minted data store on the \
             emulator's own base URL, got {endpoint:?}"
        );
        assert_eq!(
            created["DatastoreStatus"], "CREATING",
            "case {case:?}: a freshly created data store is CREATING, got {created}"
        );

        let described = service
            .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
            .await;
        let properties = &described["DatastoreProperties"];

        for member in ["DatastoreId", "DatastoreArn", "DatastoreEndpoint"] {
            assert_eq!(
                properties[member], created[member],
                "case {case:?}: Describe must report the created {member}, got {properties}"
            );
        }
        assert_eq!(
            properties["DatastoreTypeVersion"], request["DatastoreTypeVersion"],
            "case {case:?}: Describe must report the stored DatastoreTypeVersion, got {properties}"
        );
        assert!(
            properties["CreatedAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: Describe must report a CreatedAt, got {properties}"
        );

        // Every optional member supplied at create is reported verbatim; one
        // that was not supplied is absent rather than invented.
        for member in [
            "DatastoreName",
            "PreloadDataConfig",
            "IdentityProviderConfiguration",
            "AnalyticsConfiguration",
            "NlpConfiguration",
        ] {
            match request.get(member) {
                Some(supplied) => assert_eq!(
                    &properties[member], supplied,
                    "case {case:?}: Describe must report the stored {member}, got {properties}"
                ),
                None => assert!(
                    properties.get(member).is_none(),
                    "case {case:?}: {member} was never supplied, so Describe must omit it, got {properties}"
                ),
            }
        }

        // `SseConfiguration` is the one optional member with a DEFAULT: the
        // modelled `KmsEncryptionConfig` is documented "If a customer-owned key is
        // not specified, an Amazon Web Services-owned key is used for encryption",
        // so a create that named no key still reports the key that encrypts the
        // data store. The rule over a set of inputs is pinned in
        // `tests/all/datastore_encryption_and_preload.rs`.
        let expected_sse = request.get("SseConfiguration").cloned().unwrap_or_else(
            || json!({ "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" } }),
        );
        assert_eq!(
            properties["SseConfiguration"], expected_sse,
            "case {case:?}: Describe must report the key that encrypts the data store, got {properties}"
        );

        // The tags supplied at create are the tags the resource carries.
        let expected_tags = request["Tags"].as_array().cloned().unwrap_or_default();
        let listed = service
            .ok("ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        let mut actual_tags = listed["Tags"].as_array().cloned().unwrap_or_default();
        actual_tags.sort_by_key(|tag| string_at(tag, "Key"));
        let mut expected_sorted = expected_tags.clone();
        expected_sorted.sort_by_key(|tag| string_at(tag, "Key"));
        assert_eq!(
            actual_tags, expected_sorted,
            "case {case:?}: ListTagsForResource must report the tags given at create"
        );
    }
}

#[tokio::test]
async fn client_token_makes_create_idempotent() {
    let service = HealthLake::new();

    // The rule generalises over the token value, so the rows differ in form and
    // length within the modelled `ClientTokenString` pattern `^[a-zA-Z0-9-]+$`.
    for case in ["tok-1", "A", "9f2-B7-cafe", &"z".repeat(64)] {
        let request = json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": "idempotent store",
            "ClientToken": case,
        });
        let before = service.list_all(&Value::Null).await.len();
        let (first, _) = service.create(&request).await;
        let (second, _) = service.create(&request).await;
        let (third, _) = service.create(&request).await;
        let after = service.list_all(&Value::Null).await.len();

        assert_eq!(
            first, second,
            "case {case:?}: a repeated ClientToken must resolve to the same DatastoreId"
        );
        assert_eq!(
            first, third,
            "case {case:?}: a repeated ClientToken must resolve to the same DatastoreId"
        );
        assert_eq!(
            after,
            before + 1,
            "case {case:?}: three calls with one ClientToken must mint one data store, got {} new",
            after - before
        );
    }

    // Distinct tokens, and an absent token, each mint their own data store.
    let (a, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "ClientToken": "distinct-a" }))
        .await;
    let (b, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "ClientToken": "distinct-b" }))
        .await;
    let (c, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;
    let (d, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;
    for (case, left, right) in [
        ("distinct tokens", &a, &b),
        ("no token at all", &c, &d),
        ("token vs none", &a, &c),
    ] {
        assert_ne!(
            left, right,
            "case {case:?}: these creates must mint different data stores"
        );
    }

    // A token is idempotent for the SAME request only. Rows span the members a
    // second request can differ in; each must be refused rather than answered
    // with the data store the token already minted.
    let original = json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": "original",
        "ClientToken": "reused-token",
        "Tags": [{ "Key": "env", "Value": "test" }],
    });
    let (minted, _) = service.create(&original).await;
    let divergent: Vec<(&str, Value)> = vec![
        (
            "a different name",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "renamed",
                "ClientToken": "reused-token",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "a different tag value",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "original",
                "ClientToken": "reused-token",
                "Tags": [{ "Key": "env", "Value": "prod" }],
            }),
        ),
        (
            "no tags at all",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "original",
                "ClientToken": "reused-token",
            }),
        ),
        (
            "an added configuration",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "original",
                "ClientToken": "reused-token",
                "Tags": [{ "Key": "env", "Value": "test" }],
                "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
            }),
        ),
    ];
    for (case, request) in divergent {
        let (status, payload) = service.call("CreateFHIRDatastore", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reused ClientToken with a different request must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: a reused ClientToken with a different request must answer ValidationException, got {payload}"
        );
    }
    // The original request is still idempotent, and the tag order it arrives in
    // is not part of the request's identity.
    let (again, _) = service.create(&original).await;
    assert_eq!(
        again, minted,
        "the original request must still resolve to the data store its token minted"
    );
}

#[tokio::test]
async fn an_id_no_data_store_carries_is_not_found() {
    let service = HealthLake::new();
    // One real data store exists, so the refusal is about the id and not about an
    // empty service.
    let (real, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;

    // Rows span the forms an unknown `DatastoreId` can take within the member's
    // modelled 1..32 length bound. The last two are derived from the real id —
    // one character short, and the real id with its last character provably
    // changed — so neither can coincide with the minted one.
    let one_off = {
        let mut id = real.clone();
        let flipped = if id.ends_with('0') { '1' } else { '0' };
        id.pop();
        id.push(flipped);
        id
    };
    assert_ne!(
        one_off, real,
        "the one-character-off row must differ from the minted id"
    );
    for case in [
        "a",
        "00000000000000000000000000000000",
        "deadbeefdeadbeefdeadbeefdeadbeef",
        &real[..31],
        one_off.as_str(),
    ] {
        for operation in ["DescribeFHIRDatastore", "DeleteFHIRDatastore"] {
            let (status, payload) = service
                .call(operation, &json!({ "DatastoreId": case }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} on an unknown id must answer 400, got {status}: {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: {operation} on an unknown id must answer ResourceNotFoundException, got {payload}"
            );
        }
    }

    // And the real id still resolves, so the lookup is by identity.
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &real }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreId"], real,
        "the existing data store must still describe, got {described}"
    );
}

#[tokio::test]
async fn list_applies_every_filter_member() {
    let service = HealthLake::new();
    let (alpha_one, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "alpha" }))
        .await;
    let (beta, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "beta" }))
        .await;
    let (alpha_two, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "alpha" }))
        .await;
    let (unnamed, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let past = epoch_secs() - 3600.0;
    let future = epoch_secs() + 3600.0;
    let all = {
        let mut ids = vec![
            alpha_one.clone(),
            beta.clone(),
            alpha_two.clone(),
            unnamed.clone(),
        ];
        ids.sort();
        ids
    };

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter", Value::Null, all.clone()),
        (
            "name matches two stores",
            json!({ "DatastoreName": "alpha" }),
            vec![alpha_one.clone(), alpha_two.clone()],
        ),
        (
            "name matches one store",
            json!({ "DatastoreName": "beta" }),
            vec![beta.clone()],
        ),
        (
            "name matches nothing",
            json!({ "DatastoreName": "nope" }),
            vec![],
        ),
        (
            "status matches every settled store",
            json!({ "DatastoreStatus": "ACTIVE" }),
            all.clone(),
        ),
        (
            "status nothing is in",
            json!({ "DatastoreStatus": "CREATE_FAILED" }),
            vec![],
        ),
        (
            "created before a future cutoff",
            json!({ "CreatedBefore": future }),
            all.clone(),
        ),
        (
            "created before a past cutoff",
            json!({ "CreatedBefore": past }),
            vec![],
        ),
        (
            "created after a past cutoff",
            json!({ "CreatedAfter": past }),
            all.clone(),
        ),
        (
            "created after a future cutoff",
            json!({ "CreatedAfter": future }),
            vec![],
        ),
        (
            "name and window together",
            json!({ "DatastoreName": "alpha", "CreatedAfter": past, "CreatedBefore": future }),
            vec![alpha_one.clone(), alpha_two.clone()],
        ),
        (
            "name matches but the window excludes it",
            json!({ "DatastoreName": "alpha", "CreatedBefore": past }),
            vec![],
        ),
    ];

    for (case, filter, expected) in cases {
        let mut actual: Vec<String> = service
            .list_all(&filter)
            .await
            .iter()
            .map(|properties| string_at(properties, "DatastoreId"))
            .collect();
        actual.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: ListFHIRDatastores must report exactly the matching data stores"
        );
    }
}

#[tokio::test]
async fn status_settles_from_creating_to_active_after_the_due_span() {
    let service = HealthLake::new();
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "settling" }))
        .await;

    // Before the transition falls due the caller sees the transient status,
    // through Describe and through the status filter alike.
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"], "CREATING",
        "a data store describes as CREATING until its transition falls due, got {described}"
    );
    let creating: Vec<String> = service
        .list_all(&json!({ "DatastoreStatus": "CREATING" }))
        .await
        .iter()
        .map(|properties| string_at(properties, "DatastoreId"))
        .collect();
    assert!(
        creating.contains(&id),
        "a CREATING data store must be listed under CREATING, got {creating:?}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;

    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"], "ACTIVE",
        "a data store describes as ACTIVE once its transition falls due, got {described}"
    );
    for (case, filter, expected) in [
        ("CREATING", json!({ "DatastoreStatus": "CREATING" }), false),
        ("ACTIVE", json!({ "DatastoreStatus": "ACTIVE" }), true),
    ] {
        let ids: Vec<String> = service
            .list_all(&filter)
            .await
            .iter()
            .map(|properties| string_at(properties, "DatastoreId"))
            .collect();
        assert_eq!(
            ids.contains(&id),
            expected,
            "case {case:?}: a settled data store is listed under ACTIVE only, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn list_pages_with_max_results_and_resumes_from_next_token() {
    let service = HealthLake::new();
    let mut created = Vec::new();
    for index in 0..5 {
        let (id, _) = service
            .create(&json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": format!("paged-{index}"),
            }))
            .await;
        created.push(id);
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    let full: Vec<String> = service
        .list_all(&Value::Null)
        .await
        .iter()
        .map(|properties| string_at(properties, "DatastoreId"))
        .collect();
    assert_eq!(
        full.len(),
        created.len(),
        "an unpaged list must report every data store, got {full:?}"
    );

    // The rule generalises over the page size, including both published bounds
    // of the modelled `MaxResultsInteger` range (1..500) and sizes between.
    for page_size in [1_u32, 2, 3, 5, 6, 500] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(ref next) = token {
                request["NextToken"] = json!(next);
            }
            let listed = service.ok("ListFHIRDatastores", &request).await;
            let page = listed["DatastorePropertiesList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size as usize,
                "case {page_size}: a page must hold at most MaxResults entries, got {}",
                page.len()
            );
            walked.extend(page.iter().map(|properties| string_at(properties, "DatastoreId")));
            pages += 1;
            assert!(
                pages <= full.len() + 1,
                "case {page_size}: paging did not terminate after {pages} pages"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        walked.len() < full.len(),
                        "case {page_size}: a NextToken was emitted with nothing left to return"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, full,
            "case {page_size}: paging must walk every data store exactly once, in list order"
        );
    }

    // A token that resumes nothing is a client error, not an empty page.
    let (status, payload) = service
        .call(
            "ListFHIRDatastores",
            &json!({ "NextToken": "not-a-page-boundary" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an unresumable NextToken must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "an unresumable NextToken must answer ValidationException, got {payload}"
    );
}

#[tokio::test]
async fn tag_resource_merges_by_key_onto_the_stored_data_store() {
    let service = HealthLake::new();

    // Rows span the merge cases: adding to nothing, adding a new key,
    // overwriting an existing key, and doing both in one call.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "add to an untagged store",
            json!([]),
            json!([{ "Key": "env", "Value": "test" }]),
            json!([{ "Key": "env", "Value": "test" }]),
        ),
        (
            "add a new key",
            json!([{ "Key": "env", "Value": "test" }]),
            json!([{ "Key": "owner", "Value": "alice" }]),
            json!([
                { "Key": "env", "Value": "test" },
                { "Key": "owner", "Value": "alice" },
            ]),
        ),
        (
            "overwrite an existing key",
            json!([{ "Key": "env", "Value": "test" }]),
            json!([{ "Key": "env", "Value": "prod" }]),
            json!([{ "Key": "env", "Value": "prod" }]),
        ),
        (
            "overwrite one key and add two",
            json!([
                { "Key": "env", "Value": "test" },
                { "Key": "team", "Value": "clinical" },
            ]),
            json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "owner", "Value": "alice" },
                { "Key": "region.hint", "Value": "eu" },
            ]),
            json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "owner", "Value": "alice" },
                { "Key": "region.hint", "Value": "eu" },
                { "Key": "team", "Value": "clinical" },
            ]),
        ),
        (
            "tagging with no tags at all changes nothing",
            json!([{ "Key": "env", "Value": "test" }]),
            json!([]),
            json!([{ "Key": "env", "Value": "test" }]),
        ),
    ];

    for (case, initial, added, expected) in cases {
        let (_, arn) = service
            .create(&json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "tagged store",
                "Tags": initial,
            }))
            .await;
        service
            .ok(
                "TagResource",
                &json!({ "ResourceARN": &arn, "Tags": added }),
            )
            .await;

        let listed = service
            .ok("ListTagsForResource", &json!({ "ResourceARN": &arn }))
            .await;
        let mut actual = listed["Tags"].as_array().cloned().unwrap_or_default();
        actual.sort_by_key(|tag| string_at(tag, "Key"));
        assert_eq!(
            Value::Array(actual),
            expected,
            "case {case:?}: ListTagsForResource must report the merged tags"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_data_store_is_not_found() {
    let service = HealthLake::new();
    let (id, real) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;

    // Every row satisfies the modelled `AmazonResourceName` pattern, so the
    // boundary validator passes it and the handler decides. The rows span the
    // ways an ARN can fail to name THIS data store: an id nothing was minted
    // under, a `dataTransformationProfile` ARN (a different resource this slice
    // does not hold), and — carrying the real minted id — another region,
    // another account and another partition.
    let cases = vec![
        "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/00000000000000000000000000000000".to_string(),
        "arn:aws:healthlake:eu-west-1:000000000000:datastore/fhir/ffffffffffffffffffffffffffffffff".to_string(),
        "arn:aws:healthlake:us-east-1:000000000000:dataTransformationProfile/0123456789abcdef0123456789abcdef".to_string(),
        format!("arn:aws:healthlake:eu-west-1:000000000000:datastore/fhir/{id}"),
        format!("arn:aws:healthlake:us-east-1:210987654321:datastore/fhir/{id}"),
        format!("arn:aws-cn:healthlake:us-east-1:000000000000:datastore/fhir/{id}"),
    ];
    for case in &cases {
        for (operation, request) in [
            ("ListTagsForResource", json!({ "ResourceARN": case })),
            (
                "TagResource",
                json!({ "ResourceARN": case, "Tags": [{ "Key": "env", "Value": "test" }] }),
            ),
        ] {
            let (status, payload) = service.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} on an unknown ARN must answer 400, got {status}: {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: {operation} on an unknown ARN must answer ResourceNotFoundException, got {payload}"
            );
        }
    }

    // The real ARN still resolves, so the refusal is about the resource.
    let listed = service
        .ok("ListTagsForResource", &json!({ "ResourceARN": &real }))
        .await;
    assert!(
        listed["Tags"].is_array(),
        "the existing data store must still report its tags, got {listed}"
    );
}

#[tokio::test]
async fn delete_moves_the_stored_record_through_deleting_to_deleted() {
    let service = HealthLake::new();
    let (id, arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "doomed" }))
        .await;

    // While the create transition is still pending the data store is in a
    // transition state, which is exactly what ConflictException models.
    let (status, payload) = service
        .call("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        status, 400,
        "deleting a CREATING data store must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "deleting a CREATING data store must answer ConflictException, got {payload}"
    );

    // The transition the clock owes the create is waited for by LOOKING for it: a
    // lifecycle transition is materialised by whichever call reads the record next, so
    // the condition is what is waited on. Never a fixed interval — a wall-clock wait
    // passes on one machine and fails on a loaded or a faster one.
    service.poll_until_status(&id, "ACTIVE").await;

    let deleted = service
        .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        deleted["DatastoreStatus"], "DELETING",
        "DeleteFHIRDatastore must report the stored record as DELETING, got {deleted}"
    );
    assert_eq!(
        deleted["DatastoreArn"], arn,
        "DeleteFHIRDatastore must report the stored ARN, got {deleted}"
    );
    assert_eq!(
        deleted["DatastoreEndpoint"].as_str().unwrap_or_default(),
        format!("{EMULATOR_BASE}/datastore/{id}/r4/"),
        "DeleteFHIRDatastore must report the stored endpoint, got {deleted}"
    );

    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"], "DELETING",
        "Describe must show the deletion in progress, got {described}"
    );
    let active: Vec<String> = service
        .list_all(&json!({ "DatastoreStatus": "ACTIVE" }))
        .await
        .iter()
        .map(|properties| string_at(properties, "DatastoreId"))
        .collect();
    assert!(
        !active.contains(&id),
        "a deleting data store must no longer be listed as ACTIVE, got {active:?}"
    );

    // Deleting again while the deletion is in flight is the same conflict.
    let (status, payload) = service
        .call("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        status, 400,
        "deleting a DELETING data store must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "deleting a DELETING data store must answer ConflictException, got {payload}"
    );

    // As above: the deletion's own transition is waited for by looking for the status it
    // owes, not by sleeping for its span.
    let described = service.poll_until_status(&id, "DELETED").await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreStatus"], "DELETED",
        "Describe must show the completed deletion, got {described}"
    );
    // Once deleted the data store is gone, not in transition.
    let (status, payload) = service
        .call("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
        .await;
    assert_eq!(
        status, 400,
        "deleting a DELETED data store must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "deleting a DELETED data store must answer ResourceNotFoundException, got {payload}"
    );
}

#[tokio::test]
async fn the_store_bounds_how_many_data_stores_it_holds() {
    use service_com_amazonaws_healthlake_healthlake::state::MAX_DATASTORES;

    let service = HealthLake::new();
    let mut ids = Vec::new();
    for index in 0..MAX_DATASTORES {
        let (id, _) = service
            .create(&json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": format!("bounded-{index}"),
            }))
            .await;
        ids.push(id);
    }

    // A full store refuses the create instead of growing without limit.
    let (status, payload) = service
        .call(
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "one too many" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a create past the ceiling must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ThrottlingException",
        "a create past the ceiling must answer ThrottlingException, got {payload}"
    );

    // Deleting one frees its slot once the record reaches its terminal status,
    // so a caller that cleans up after itself can create again.
    tokio::time::sleep(PAST_TRANSITION).await;
    service
        .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &ids[0] }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let (reclaimed, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "reclaimed" }))
        .await;
    assert_well_formed_id("after reclaiming a terminal record", &reclaimed);
    assert_eq!(
        service.list_all(&Value::Null).await.len(),
        MAX_DATASTORES,
        "the store must still hold at most MAX_DATASTORES records"
    );
}

#[tokio::test]
async fn the_store_bounds_how_many_tags_one_data_store_carries() {
    use service_com_amazonaws_healthlake_healthlake::state::MAX_TAGS_PER_DATASTORE;

    let service = HealthLake::new();
    let (_, arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;

    let tags = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Key": format!("key{index}"), "Value": "v" }))
                .collect(),
        )
    };

    // Up to the ceiling the merge is stored; one key past it is refused whole,
    // so the resource's tag set is bounded however many calls are made.
    service
        .ok(
            "TagResource",
            &json!({ "ResourceARN": &arn, "Tags": tags(MAX_TAGS_PER_DATASTORE) }),
        )
        .await;
    let (status, payload) = service
        .call(
            "TagResource",
            &json!({ "ResourceARN": &arn, "Tags": [{ "Key": "one.too.many", "Value": "v" }] }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a merge past the tag ceiling must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "a merge past the tag ceiling must answer ValidationException, got {payload}"
    );

    let listed = service
        .ok("ListTagsForResource", &json!({ "ResourceARN": &arn }))
        .await;
    assert_eq!(
        listed["Tags"].as_array().map(Vec::len),
        Some(MAX_TAGS_PER_DATASTORE),
        "the refused merge must leave the stored tags unchanged, got {listed}"
    );

    // A create asking for more tags than the resource carries is refused too.
    let (status, payload) = service
        .call(
            "CreateFHIRDatastore",
            &json!({
                "DatastoreTypeVersion": "R4",
                "Tags": tags(MAX_TAGS_PER_DATASTORE + 1),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a create past the tag ceiling must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "a create past the tag ceiling must answer ValidationException, got {payload}"
    );
}

#[tokio::test]
async fn every_modelled_operation_of_this_service_is_implemented() {
    let service = HealthLake::new();
    let (id, _arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4" }))
        .await;

    // This suite used to pin the 501 a scaffolded operation answers, over the
    // operations of this service that were still scaffolds. There are none left:
    // `StartFHIRImportJob` was the last of them (see
    // `tests/all/fhir_import_job_start.rs`), so what is asserted here now is the
    // other half of that rule — a request satisfying every modelled constraint
    // reaches a handler that reads and writes this service's state, and NO
    // operation answers "nobody has written it yet" any more.
    let (status, payload) = service
        .call(
            "StartFHIRImportJob",
            &json!({
                "DatastoreId": &id,
                "InputDataConfig": { "S3Uri": "s3://healthlake-import-bucket/input" },
                "JobOutputDataConfig": {
                    "S3Configuration": {
                        "S3Uri": "s3://healthlake-import-bucket/output",
                        "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                    }
                },
                "DataAccessRoleArn": "arn:aws:iam::111122223333:role/HealthLakeImport",
            }),
        )
        .await;
    assert_ne!(
        status, 501,
        "no operation of this service is a scaffold any more, got {status}: {payload}"
    );
    // The data store is still `CREATING` here, which the implemented handler
    // refuses with the modelled access-denied answer — a refusal that comes from
    // the data store's own state rather than from the operation being unwritten.
    assert_eq!(
        error_code(&payload),
        "AccessDeniedException",
        "a start against a data store that is not ACTIVE must answer \
         AccessDeniedException, got {payload}"
    );
}
