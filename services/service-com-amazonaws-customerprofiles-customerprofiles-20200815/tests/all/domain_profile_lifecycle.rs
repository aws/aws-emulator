// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Customer Profiles domain -> profile lifecycle.
//!
//! Every test drives the service through the real HTTP surface — the same
//! `MultiServiceRouter` the emulator binary mounts — so what is asserted is the wire
//! behaviour a client sees, not a handler's return value.
//!
//! The rule under test: every one of these operations computes its answer from the
//! service's stored state. A create persists the resource and answers from the record it
//! stored; a read answers from that record and changes nothing; a delete removes the
//! resource and everything owned by it; a reference to a resource that is not stored is
//! `ResourceNotFoundException` (404) and an input outside a modelled constraint is
//! `BadRequestException` (400). Each test asserts that over a SET of inputs rather than
//! for one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::ops::types::Profile;
use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::{
    service_state, Domain, StoredProfile, MAX_DOMAINS, MAX_PROFILES_PER_DOMAIN,
};
use service_com_amazonaws_customerprofiles_customerprofiles_20200815::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`profile`), which is how the router picks the service that claims a request.
/// The signature itself is never checked: these tests run with request authentication
/// off, exactly as the emulator does by default.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/profile/aws4_request, \
     SignedHeaders=host, Signature=0";

/// One emulator: a single shared engine behind the router, so every request in a test
/// observes the same state.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the router handed the service, so a test can seed the store
    /// directly when driving a store bound through thousands of HTTP calls would only
    /// make the test slow, or when what has to be set is a retention measured in days.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let (status, _, value) = self.send_parts(method, path, body).await;
        (status, value)
    }

    /// As [`Self::send`], and with the `x-amzn-errortype` header the response carried —
    /// the header a restJson1 client resolves an error SHAPE from, so a suite that asserts
    /// WHICH shape a refusal names reads it here rather than from the body alone.
    pub(crate) async fn send_parts(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String, Value) {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://profile.us-east-1.amazonaws.com{path}"))
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.body();
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, error_type, value)
    }

    pub(crate) async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.send("POST", path, Some(body)).await
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.send("GET", path, None).await
    }

    async fn delete(&self, path: &str) -> (u16, Value) {
        self.send("DELETE", path, None).await
    }

    /// `CreateDomain` with the given expiration, asserting it succeeded.
    pub(crate) async fn create_domain(&self, domain: &str, expiration_days: u32) -> Value {
        let (status, body) = self
            .post(
                &format!("/domains/{domain}"),
                json!({ "DefaultExpirationDays": expiration_days }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected CreateDomain 200, got {status} {body:?}"
        );
        body
    }

    /// `CreateProfile`, asserting it succeeded, and returning the minted `ProfileId`.
    pub(crate) async fn create_profile(&self, domain: &str, profile: Value) -> String {
        let (status, body) = self
            .post(&format!("/domains/{domain}/profiles"), profile.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {profile:?}: expected CreateProfile 200, got {status} {body:?}"
        );
        body.get("ProfileId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {profile:?}: no ProfileId in {body:?}"))
            .to_string()
    }

    /// `CreateUploadJob`: `POST /domains/{DomainName}/upload-jobs`. Shared by every
    /// upload-job suite, so the route is spelled for a call exactly once.
    pub(crate) async fn create_upload_job(&self, domain: &str, body: Value) -> (u16, Value) {
        self.send("POST", &format!("/domains/{domain}/upload-jobs"), Some(body))
            .await
    }

    /// `GetUploadJob`: `GET /domains/{DomainName}/upload-jobs/{JobId}`.
    pub(crate) async fn get_upload_job(&self, domain: &str, job_id: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!("/domains/{domain}/upload-jobs/{job_id}"),
            None,
        )
        .await
    }

    /// `SearchProfiles` for one key, with optional query parameters.
    pub(crate) async fn search(&self, domain: &str, body: Value, query: &str) -> (u16, Value) {
        self.post(&format!("/domains/{domain}/profiles/search{query}"), body)
            .await
    }

    /// `DeleteProfile`: `POST /domains/{DomainName}/profiles/delete`. The one place the
    /// route is spelled for a call; the refusal tables below drive it through
    /// [`Self::send`] because what they vary IS the path.
    pub(crate) async fn delete_profile(&self, domain: &str, profile_id: &str) -> (u16, Value) {
        self.post(
            &format!("/domains/{domain}/profiles/delete"),
            json!({ "ProfileId": profile_id }),
        )
        .await
    }
}

/// The wire error code restJson1 carries in the body's `__type` discriminator.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The `Message` member of a modelled error body.
pub(crate) fn error_message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// A `FieldMap` carrying one column: `ObjectTypeField`'s three members, with a
/// `ContentType` out of the modelled `FieldContentType` enum (STRING | NUMBER |
/// PHONE_NUMBER | EMAIL_ADDRESS | NAME).
pub(crate) fn upload_job_fields(column: &str, content_type: &str, target: &str) -> Value {
    json!({
        column: {
            "ContentType": content_type,
            "Source": format!("_source.{column}"),
            "Target": target,
        },
    })
}

/// The `ProfileId`s of a `SearchProfiles` response's `Items`.
pub(crate) fn item_ids(body: &Value) -> Vec<String> {
    body.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("ProfileId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The lifecycle the whole slice exists for: a domain and a profile created, read back,
/// then deleted, with every read answering from what the writes stored.
#[tokio::test]
async fn the_domain_profile_lifecycle_round_trips_through_stored_state() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .post(
            "/domains/testdom",
            json!({ "DefaultExpirationDays": 90, "Tags": { "env": "dev" } }),
        )
        .await;
    assert_eq!(status, 200, "CreateDomain: expected 200, got {status} {created:?}");
    assert_eq!(
        created.get("DomainName").and_then(Value::as_str),
        Some("testdom"),
        "CreateDomain: expected DomainName testdom, got {:?}",
        created.get("DomainName")
    );
    assert_eq!(
        created.get("DefaultExpirationDays").and_then(Value::as_i64),
        Some(90),
        "CreateDomain: expected DefaultExpirationDays 90, got {:?}",
        created.get("DefaultExpirationDays")
    );
    assert_eq!(
        created.get("Tags"),
        Some(&json!({ "env": "dev" })),
        "CreateDomain: expected the Tags it was given, got {:?}",
        created.get("Tags")
    );
    assert!(
        created.get("CreatedAt").is_some() && created.get("LastUpdatedAt").is_some(),
        "CreateDomain: both timestamps are required, got {created:?}"
    );

    let (status, fetched) = emulator.get("/domains/testdom").await;
    assert_eq!(status, 200, "GetDomain: expected 200, got {status} {fetched:?}");
    for member in ["DomainName", "DefaultExpirationDays", "Tags", "CreatedAt", "LastUpdatedAt"] {
        assert_eq!(
            fetched.get(member),
            created.get(member),
            "GetDomain: expected {member} = {:?} (as CreateDomain stored it), got {:?}",
            created.get(member),
            fetched.get(member)
        );
    }

    let (status, missing) = emulator.get("/domains/nosuchdom").await;
    assert_eq!(
        (status, error_code(&missing)),
        (404, "ResourceNotFoundException"),
        "GetDomain on an absent domain: expected 404 ResourceNotFoundException, got {status} {missing:?}"
    );

    let profile_id = emulator
        .create_profile(
            "testdom",
            json!({
                "FirstName": "Ada",
                "LastName": "Lovelace",
                "EmailAddress": "ada@example.com",
                "Attributes": { "tier": "gold" },
            }),
        )
        .await;
    assert!(
        profile_id.len() == 32 && profile_id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "CreateProfile: expected a ProfileId matching ^[a-f0-9]{{32}}$, got {profile_id:?}"
    );

    let (status, found) = emulator
        .search(
            "testdom",
            json!({ "KeyName": "_profileId", "Values": [profile_id.clone()] }),
            "",
        )
        .await;
    assert_eq!(status, 200, "SearchProfiles: expected 200, got {status} {found:?}");
    let items = found.get("Items").and_then(Value::as_array).cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "SearchProfiles by _profileId: expected exactly the created profile, got {found:?}"
    );
    let item = &items[0];
    for (member, expected) in [
        ("ProfileId", json!(profile_id.clone())),
        ("FirstName", json!("Ada")),
        ("LastName", json!("Lovelace")),
        ("EmailAddress", json!("ada@example.com")),
        ("Attributes", json!({ "tier": "gold" })),
    ] {
        assert_eq!(
            item.get(member),
            Some(&expected),
            "SearchProfiles: expected {member} = {expected:?}, got {:?}",
            item.get(member)
        );
    }

    let (status, empty) = emulator
        .search(
            "testdom",
            json!({ "KeyName": "_profileId", "Values": ["0".repeat(32)] }),
            "",
        )
        .await;
    assert_eq!(
        (status, item_ids(&empty).len()),
        (200, 0),
        "SearchProfiles for an unknown id: expected 200 with empty Items, got {status} {empty:?}"
    );

    let (status, deleted) = emulator.delete_profile("testdom", &profile_id).await;
    assert_eq!(status, 200, "DeleteProfile: expected 200, got {status} {deleted:?}");
    assert!(
        deleted.get("Message").and_then(Value::as_str).is_some(),
        "DeleteProfile: expected a Message, got {deleted:?}"
    );

    let (status, gone) = emulator
        .search(
            "testdom",
            json!({ "KeyName": "_profileId", "Values": [profile_id.clone()] }),
            "",
        )
        .await;
    assert_eq!(
        (status, item_ids(&gone).len()),
        (200, 0),
        "SearchProfiles after DeleteProfile: expected 200 with empty Items, got {status} {gone:?}"
    );

    let (status, listed) = emulator.get("/domains").await;
    assert_eq!(status, 200, "ListDomains: expected 200, got {status} {listed:?}");
    let names: Vec<&str> = listed
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("DomainName").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&"testdom"),
        "ListDomains: expected testdom to be listed, got {names:?}"
    );

    let (status, dropped) = emulator.delete("/domains/testdom").await;
    assert_eq!(status, 200, "DeleteDomain: expected 200, got {status} {dropped:?}");
    assert!(
        dropped.get("Message").and_then(Value::as_str).is_some(),
        "DeleteDomain: expected a Message, got {dropped:?}"
    );

    let (status, after) = emulator.get("/domains/testdom").await;
    assert_eq!(
        (status, error_code(&after)),
        (404, "ResourceNotFoundException"),
        "GetDomain after DeleteDomain: expected 404 ResourceNotFoundException, got {status} {after:?}"
    );

    let (status, listed) = emulator.get("/domains").await;
    assert_eq!(
        (status, listed.get("Items")),
        (200, Some(&json!([]))),
        "ListDomains after DeleteDomain: expected an empty list, got {status} {listed:?}"
    );
}

/// `GetDomain` reports exactly what `CreateDomain` persisted, whichever optional members
/// the create carried. The rows differ along WHICH members are present and their shape
/// (scalar, map, nested structure), which is what the rule generalises over.
#[tokio::test]
async fn get_domain_reports_what_create_domain_persisted() {
    let emulator = Emulator::new();

    // The published bounds of `DefaultExpirationDays` (range 1..1098) are rows here, so
    // the round trip is checked at both ends of the range as well as inside it.
    let cases: Vec<(&str, Value)> = vec![
        ("minimum-expiration", json!({ "DefaultExpirationDays": 1 })),
        ("maximum-expiration", json!({ "DefaultExpirationDays": 1098 })),
        (
            "tags",
            json!({ "DefaultExpirationDays": 90, "Tags": { "env": "dev", "team": "crm" } }),
        ),
        (
            "encryption-key",
            json!({
                "DefaultExpirationDays": 30,
                "DefaultEncryptionKey": "arn:aws:kms:us-east-1:123456789012:key/1234abcd",
            }),
        ),
        (
            "dead-letter-queue",
            json!({
                "DefaultExpirationDays": 30,
                "DeadLetterQueueUrl": "https://sqs.us-east-1.amazonaws.com/123456789012/dlq",
            }),
        ),
        (
            "matching",
            json!({
                "DefaultExpirationDays": 30,
                "Matching": {
                    "Enabled": true,
                    "JobSchedule": { "DayOfTheWeek": "SATURDAY", "Time": "12:00" },
                },
            }),
        ),
        (
            "matching-disabled",
            json!({ "DefaultExpirationDays": 30, "Matching": { "Enabled": false } }),
        ),
        (
            "everything",
            json!({
                "DefaultExpirationDays": 1097,
                "DefaultEncryptionKey": "arn:aws:kms:us-east-1:123456789012:key/beef",
                "DeadLetterQueueUrl": "https://sqs.us-east-1.amazonaws.com/123456789012/other",
                "Matching": { "Enabled": true },
                "Tags": { "owner": "crm" },
            }),
        ),
    ];

    for (case, request) in cases {
        let domain = format!("dom_{}", case.replace('-', "_"));
        let (status, created) = emulator.post(&format!("/domains/{domain}"), request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDomain 200, got {status} {created:?}"
        );

        // Every member the request carried is stored and reported back. `Matching` is a
        // request/response shape pair, so it is compared through the read below.
        for (member, expected) in request.as_object().expect("an object request") {
            if member == "Matching" {
                continue;
            }
            assert_eq!(
                created.get(member),
                Some(expected),
                "case {case:?}: expected CreateDomain {member} = {expected:?}, got {:?}",
                created.get(member)
            );
        }
        assert_eq!(
            created.get("CreatedAt"),
            created.get("LastUpdatedAt"),
            "case {case:?}: a freshly created domain has one generation time, got CreatedAt {:?} and LastUpdatedAt {:?}",
            created.get("CreatedAt"),
            created.get("LastUpdatedAt")
        );

        let (status, fetched) = emulator.get(&format!("/domains/{domain}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDomain 200, got {status} {fetched:?}"
        );
        for member in [
            "DomainName",
            "DefaultExpirationDays",
            "DefaultEncryptionKey",
            "DeadLetterQueueUrl",
            "Matching",
            "Tags",
            "CreatedAt",
            "LastUpdatedAt",
        ] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {case:?}: expected GetDomain {member} = {:?} (as CreateDomain stored it), got {:?}",
                created.get(member),
                fetched.get(member)
            );
        }
        assert_eq!(
            fetched.pointer("/Stats/ProfileCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a new domain holds no profiles, got {:?}",
            fetched.get("Stats")
        );

        // A read never changes what the next read sees.
        let (_, again) = emulator.get(&format!("/domains/{domain}")).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two reads of an unchanged domain must be identical, got {fetched:?} then {again:?}"
        );
    }
}

/// `SearchProfiles` finds a profile under every predefined key the profile's own
/// attributes populate, and returns an empty page for a key or value nothing is indexed
/// under. The key list is the one `SearchProfiles$KeyName` documents; the rows cover the
/// five a profile's attributes can populate, a predefined key only an ingested profile
/// OBJECT can populate (`_orderId`), and a key the service does not define at all.
#[tokio::test]
async fn search_profiles_resolves_every_standard_key_from_the_index() {
    let emulator = Emulator::new();
    emulator.create_domain("keys", 90).await;

    let profile_id = emulator
        .create_profile(
            "keys",
            json!({
                "AccountNumber": "ACCT-1",
                "FirstName": "Ada",
                "MiddleName": "Byron",
                "LastName": "Lovelace",
                "EmailAddress": "ada@example.com",
                "PersonalEmailAddress": "ada.home@example.com",
                "BusinessEmailAddress": "ada.work@example.com",
                "PhoneNumber": "+15550000001",
                "MobilePhoneNumber": "+15550000002",
                "HomePhoneNumber": "+15550000003",
                "BusinessPhoneNumber": "+15550000004",
            }),
        )
        .await;
    // A second profile sharing none of the first one's values, so a hit is a real index
    // lookup rather than "the only profile in the domain".
    let other_id = emulator
        .create_profile(
            "keys",
            json!({ "AccountNumber": "ACCT-2", "EmailAddress": "grace@example.com" }),
        )
        .await;

    let cases: Vec<(&str, String, bool)> = vec![
        ("_profileId", profile_id.clone(), true),
        ("_account", "ACCT-1".to_string(), true),
        ("_email", "ada@example.com".to_string(), true),
        ("_email", "ada.home@example.com".to_string(), true),
        ("_email", "ada.work@example.com".to_string(), true),
        ("_phone", "+15550000001".to_string(), true),
        ("_phone", "+15550000002".to_string(), true),
        ("_phone", "+15550000003".to_string(), true),
        ("_phone", "+15550000004".to_string(), true),
        ("_fullName", "Ada Byron Lovelace".to_string(), true),
        ("_profileId", "0".repeat(32), false),
        ("_account", "ACCT-404".to_string(), false),
        ("_email", "nobody@example.com".to_string(), false),
        ("_phone", "+15559999999".to_string(), false),
        ("_fullName", "Grace Hopper".to_string(), false),
        ("_orderId", "order-1".to_string(), false),
        ("_notAServiceKey", "anything".to_string(), false),
    ];

    for (key_name, value, expect_match) in cases {
        let case = format!("{key_name}={value}");
        let (status, body) = emulator
            .search(
                "keys",
                json!({ "KeyName": key_name, "Values": [value.clone()] }),
                "",
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a search is answered, found or not — expected 200, got {status} {body:?}"
        );
        let ids = item_ids(&body);
        let expected: Vec<String> = if expect_match {
            vec![profile_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {case:?}: expected Items {expected:?}, got {ids:?}"
        );
        if expect_match {
            let found_by = body
                .pointer("/Items/0/FoundByItems/0/KeyName")
                .and_then(Value::as_str);
            assert_eq!(
                found_by,
                Some(key_name),
                "case {case:?}: expected FoundByItems to name the key that matched, got {:?}",
                body.pointer("/Items/0/FoundByItems")
            );
        }
    }

    // The other profile is still reachable by its own key — the index is per value, not
    // per domain.
    let (_, body) = emulator
        .search(
            "keys",
            json!({ "KeyName": "_account", "Values": ["ACCT-2"] }),
            "",
        )
        .await;
    assert_eq!(
        item_ids(&body),
        vec![other_id],
        "case \"_account=ACCT-2\": expected the second profile, got {body:?}"
    );
}

/// `LogicalOperator` combines the search keys: `AND` keeps the profiles every key
/// matches, `OR` (the default when the member is absent) those at least one matches.
#[tokio::test]
async fn search_profiles_combines_keys_with_the_logical_operator() {
    let emulator = Emulator::new();
    emulator.create_domain("logic", 90).await;

    let both = emulator
        .create_profile(
            "logic",
            json!({ "EmailAddress": "a@example.com", "PhoneNumber": "+15551110000" }),
        )
        .await;
    let phone_only = emulator
        .create_profile(
            "logic",
            json!({ "EmailAddress": "b@example.com", "PhoneNumber": "+15551110000" }),
        )
        .await;
    let email_only = emulator
        .create_profile(
            "logic",
            json!({ "EmailAddress": "a@example.com", "PhoneNumber": "+15552220000" }),
        )
        .await;

    let all_three: BTreeSet<String> =
        [both.clone(), phone_only.clone(), email_only.clone()].into_iter().collect();
    let only_both: BTreeSet<String> = [both.clone()].into_iter().collect();

    let cases: Vec<(&str, Option<&str>, BTreeSet<String>)> = vec![
        ("absent-operator-defaults-to-or", None, all_three.clone()),
        ("explicit-or", Some("OR"), all_three),
        ("and", Some("AND"), only_both),
    ];

    for (case, operator, expected) in cases {
        let mut request = json!({
            "KeyName": "_email",
            "Values": ["a@example.com"],
            "AdditionalSearchKeys": [{ "KeyName": "_phone", "Values": ["+15551110000"] }],
        });
        if let Some(operator) = operator {
            request["LogicalOperator"] = json!(operator);
        }
        let (status, body) = emulator.search("logic", request, "?max-results=100").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let ids: BTreeSet<String> = item_ids(&body).into_iter().collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected Items {expected:?}, got {ids:?}"
        );
    }
}

/// Paging a search returns every match exactly once, whatever the page size: the
/// invariant is swept over EVERY page size from 1 to one past the match count, plus the
/// absent-`MaxResults` case, whose documented default is 20.
#[tokio::test]
async fn search_profiles_pages_through_every_match_exactly_once() {
    let emulator = Emulator::new();
    emulator.create_domain("paging", 90).await;

    const MATCHES: usize = 25;
    let mut created = BTreeSet::new();
    for index in 0..MATCHES {
        created.insert(
            emulator
                .create_profile(
                    "paging",
                    json!({
                        "EmailAddress": "shared@example.com",
                        "AccountNumber": format!("ACCT-{index}"),
                    }),
                )
                .await,
        );
    }

    let page_sizes: Vec<Option<usize>> = std::iter::once(None)
        .chain((1..=MATCHES + 1).map(Some))
        .collect();

    for page_size in page_sizes {
        let case = format!("max-results={page_size:?}");
        let limit = page_size.unwrap_or(20);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let mut query = match page_size {
                Some(size) => format!("?max-results={size}"),
                None => String::new(),
            };
            if let Some(token) = &token {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("next-token={token}"));
            }
            let (status, body) = emulator
                .search(
                    "paging",
                    json!({ "KeyName": "_email", "Values": ["shared@example.com"] }),
                    &query,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            let ids = item_ids(&body);
            assert!(
                ids.len() <= limit,
                "case {case:?}: a page may not exceed MaxResults {limit}, got {} items",
                ids.len()
            );
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_some() {
                assert_eq!(
                    ids.len(),
                    limit,
                    "case {case:?}: a page that hands back a NextToken is full — expected {limit} items, got {}",
                    ids.len()
                );
            }
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= MATCHES + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "case {case:?}: a profile was returned twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case:?}: expected every created profile exactly once, got {unique:?}"
        );
    }
}

/// An operation naming a resource the store does not hold answers
/// `ResourceNotFoundException` (404) — for the domain on every domain-scoped operation,
/// and for the profile on `DeleteProfile`.
#[tokio::test]
async fn an_absent_resource_answers_resource_not_found() {
    let emulator = Emulator::new();
    emulator.create_domain("present", 90).await;
    let profile_id = emulator
        .create_profile("present", json!({ "FirstName": "Ada" }))
        .await;

    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("get-domain", "GET", "/domains/absent", None),
        ("delete-domain", "DELETE", "/domains/absent", None),
        (
            "create-profile",
            "POST",
            "/domains/absent/profiles",
            Some(json!({ "FirstName": "Ada" })),
        ),
        (
            "search-profiles",
            "POST",
            "/domains/absent/profiles/search",
            Some(json!({ "KeyName": "_profileId", "Values": ["0".repeat(32)] })),
        ),
        (
            "delete-profile-absent-domain",
            "POST",
            "/domains/absent/profiles/delete",
            Some(json!({ "ProfileId": profile_id.clone() })),
        ),
        (
            "delete-profile-absent-profile",
            "POST",
            "/domains/present/profiles/delete",
            Some(json!({ "ProfileId": "0".repeat(32) })),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = emulator.send(method, path, body).await;
        assert_eq!(
            (status, error_code(&response)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {response:?}"
        );
    }

    // And the profile that IS there is untouched by the failed deletes.
    let (_, body) = emulator
        .search(
            "present",
            json!({ "KeyName": "_profileId", "Values": [profile_id.clone()] }),
            "",
        )
        .await;
    assert_eq!(
        item_ids(&body),
        vec![profile_id],
        "a refused delete must not remove anything, got {body:?}"
    );
}

/// A value outside a modelled constraint is refused as `BadRequestException` (400) — the
/// only client-input error this service's model declares. Each range is exercised on
/// both published bounds and one step past each of them.
#[tokio::test]
async fn input_outside_a_modelled_constraint_is_refused_as_bad_request() {
    let emulator = Emulator::new();
    emulator.create_domain("bounds", 90).await;

    // (case, method, path, body, expected status)
    let cases: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "expiration-below-range",
            "POST",
            "/domains/low".to_string(),
            Some(json!({ "DefaultExpirationDays": 0 })),
            400,
        ),
        (
            "expiration-at-minimum",
            "POST",
            "/domains/at_min".to_string(),
            Some(json!({ "DefaultExpirationDays": 1 })),
            200,
        ),
        (
            "expiration-at-maximum",
            "POST",
            "/domains/at_max".to_string(),
            Some(json!({ "DefaultExpirationDays": 1098 })),
            200,
        ),
        (
            "expiration-above-range",
            "POST",
            "/domains/high".to_string(),
            Some(json!({ "DefaultExpirationDays": 1099 })),
            400,
        ),
        (
            "expiration-absent",
            "POST",
            "/domains/absent_days".to_string(),
            Some(json!({})),
            400,
        ),
        (
            "domain-name-outside-the-pattern",
            "POST",
            "/domains/not.a.name".to_string(),
            Some(json!({ "DefaultExpirationDays": 90 })),
            400,
        ),
        (
            "domain-name-at-maximum-length",
            "POST",
            format!("/domains/{}", "d".repeat(64)),
            Some(json!({ "DefaultExpirationDays": 90 })),
            200,
        ),
        (
            "domain-name-above-maximum-length",
            "POST",
            format!("/domains/{}", "d".repeat(65)),
            Some(json!({ "DefaultExpirationDays": 90 })),
            400,
        ),
        (
            "max-results-below-range",
            "POST",
            "/domains/bounds/profiles/search?max-results=0".to_string(),
            Some(json!({ "KeyName": "_email", "Values": ["a@example.com"] })),
            400,
        ),
        (
            "max-results-at-minimum",
            "POST",
            "/domains/bounds/profiles/search?max-results=1".to_string(),
            Some(json!({ "KeyName": "_email", "Values": ["a@example.com"] })),
            200,
        ),
        (
            "max-results-at-maximum",
            "POST",
            "/domains/bounds/profiles/search?max-results=100".to_string(),
            Some(json!({ "KeyName": "_email", "Values": ["a@example.com"] })),
            200,
        ),
        (
            "max-results-above-range",
            "POST",
            "/domains/bounds/profiles/search?max-results=101".to_string(),
            Some(json!({ "KeyName": "_email", "Values": ["a@example.com"] })),
            400,
        ),
        (
            "search-key-absent",
            "POST",
            "/domains/bounds/profiles/search".to_string(),
            Some(json!({ "Values": ["a@example.com"] })),
            400,
        ),
        (
            "profile-id-outside-the-pattern",
            "POST",
            "/domains/bounds/profiles/delete".to_string(),
            Some(json!({ "ProfileId": "not-a-uuid" })),
            400,
        ),
        // `Values` carries no modelled length, and the service takes exactly ONE value per
        // key name: one value is served and any other number is refused, for the required
        // pair and for an `AdditionalSearchKeys` entry alike.
        (
            "search-values-one-per-key",
            "POST",
            "/domains/bounds/profiles/search".to_string(),
            Some(json!({
                "KeyName": "_email",
                "Values": ["a@example.com"],
                "AdditionalSearchKeys": [{ "KeyName": "_phone", "Values": ["15550100"] }],
            })),
            200,
        ),
        (
            "search-values-two-for-one-key",
            "POST",
            "/domains/bounds/profiles/search".to_string(),
            Some(json!({
                "KeyName": "_email",
                "Values": ["a@example.com", "b@example.com"],
            })),
            400,
        ),
        (
            "search-values-two-for-an-additional-key",
            "POST",
            "/domains/bounds/profiles/search".to_string(),
            Some(json!({
                "KeyName": "_email",
                "Values": ["a@example.com"],
                "AdditionalSearchKeys": [{
                    "KeyName": "_phone",
                    "Values": ["15550100", "15550101"],
                }],
            })),
            400,
        ),
    ];

    for (case, method, path, body, expected_status) in cases {
        let (status, response) = emulator.send(method, &path, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case:?}: expected the error code BadRequestException, got {response:?}"
            );
        }
    }

    // A second create of a name already taken is not a create either.
    let (status, response) = emulator
        .post("/domains/bounds", json!({ "DefaultExpirationDays": 90 }))
        .await;
    assert_eq!(
        (status, error_code(&response)),
        (400, "BadRequestException"),
        "case \"duplicate-domain\": expected 400 BadRequestException, got {status} {response:?}"
    );
}

/// A new domain configured for rule-based matching reports the status of the rule the
/// create started: `IN_PROGRESS` while the rule is being built — "the system is creating
/// the rule-based matching rule" — and `ACTIVE` once it is. `PENDING` is skipped because
/// the status documentation says a NEW domain skips it. A domain that did not ask for
/// rule-based matching reports none.
#[tokio::test]
async fn rule_based_matching_reaches_active_after_the_create_call() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .post(
            "/domains/rbm",
            json!({
                "DefaultExpirationDays": 90,
                "RuleBasedMatching": {
                    "Enabled": true,
                    "MatchingRules": [{ "Rule": ["EmailAddress"] }],
                    "MaxAllowedRuleLevelForMerging": 2,
                },
                "DataStore": { "Enabled": true },
            }),
        )
        .await;
    assert_eq!(status, 200, "CreateDomain: expected 200, got {status} {created:?}");
    assert_eq!(
        created.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "case \"at-create\": expected the rule to be building, got {:?}",
        created.get("RuleBasedMatching")
    );
    assert_eq!(
        created.pointer("/RuleBasedMatching/MatchingRules"),
        Some(&json!([{ "Rule": ["EmailAddress"] }])),
        "case \"at-create\": expected the stored matching rules, got {:?}",
        created.get("RuleBasedMatching")
    );

    let (_, immediately) = emulator.get("/domains/rbm").await;
    assert_eq!(
        immediately.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "case \"straight-after-create\": the transient status must still be observable, got {:?}",
        immediately.get("RuleBasedMatching")
    );
    assert!(
        immediately
            .pointer("/DataStore/Readiness/ProgressPercentage")
            .and_then(Value::as_i64)
            .is_some_and(|percentage| (0..100).contains(&percentage)),
        "case \"straight-after-create\": expected data-store readiness below 100%, got {:?}",
        immediately.get("DataStore")
    );

    // The transition is owed by the clock, so waiting past it is all it takes.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    let (_, settled) = emulator.get("/domains/rbm").await;
    assert_eq!(
        settled.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "case \"after-the-span\": expected the rule to be ready, got {:?}",
        settled.get("RuleBasedMatching")
    );
    assert_eq!(
        settled
            .pointer("/DataStore/Readiness/ProgressPercentage")
            .and_then(Value::as_i64),
        Some(100),
        "case \"after-the-span\": expected the data store to be ready, got {:?}",
        settled.get("DataStore")
    );
    assert_eq!(
        settled.get("CreatedAt"),
        created.get("CreatedAt"),
        "case \"after-the-span\": a server-generated timestamp is stable across reads — expected {:?}, got {:?}",
        created.get("CreatedAt"),
        settled.get("CreatedAt")
    );

    let plain = emulator.create_domain("plain", 90).await;
    assert_eq!(
        plain.get("RuleBasedMatching"),
        None,
        "case \"not-configured\": a domain that asked for no rule-based matching reports none, got {plain:?}"
    );
    assert_eq!(
        plain.get("DataStore"),
        None,
        "case \"not-configured\": a domain that asked for no data store reports none, got {plain:?}"
    );
}

/// The store cannot be grown without limit: a create is refused once its collection is
/// at its cap, and the refusal is the modelled `BadRequestException` (400) naming the
/// limit. The store is seeded directly — through the SAME shared engine the router
/// handed the service — because driving the cap over HTTP would only make the test slow.
#[tokio::test]
async fn a_create_is_refused_once_its_collection_is_at_its_cap() {
    let emulator = Emulator::new();
    let now = SystemTime::now();

    let state = service_state(&*emulator.engine).expect("the service's store");

    // One below the domain cap: the next create still succeeds.
    for index in 0..MAX_DOMAINS - 1 {
        let name = format!("seeded_{index}");
        state
            .lock()
            .domains
            .insert(name.clone(), Domain::new(name, 90, now));
    }
    let (status, body) = emulator
        .post("/domains/at_the_cap", json!({ "DefaultExpirationDays": 90 }))
        .await;
    assert_eq!(
        status, 200,
        "case \"one-below-the-domain-cap\": expected 200, got {status} {body:?}"
    );

    // Now at the cap: the next create is refused.
    let (status, body) = emulator
        .post("/domains/past_the_cap", json!({ "DefaultExpirationDays": 90 }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case \"at-the-domain-cap\": expected 400 BadRequestException, got {status} {body:?}"
    );
    assert!(
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains(&MAX_DOMAINS.to_string())),
        "case \"at-the-domain-cap\": the refusal must name the limit, got {body:?}"
    );
    assert_eq!(
        state.lock().domains.len(),
        MAX_DOMAINS,
        "case \"at-the-domain-cap\": a refused create must not store anything"
    );

    // Same for the per-domain profile cap, seeded one short of it.
    {
        let mut store = state.lock();
        let domain = store
            .domains
            .get_mut("at_the_cap")
            .expect("the domain created above");
        for index in 0..MAX_PROFILES_PER_DOMAIN - 1 {
            domain.put_profile(StoredProfile {
                profile: Profile {
                    profile_id: Some(format!("{index:032x}")),
                    ..Profile::default()
                },
                created_at: now,
                last_updated_at: now,
            });
        }
    }
    let (status, body) = emulator
        .post("/domains/at_the_cap/profiles", json!({ "FirstName": "Ada" }))
        .await;
    assert_eq!(
        status, 200,
        "case \"one-below-the-profile-cap\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .post("/domains/at_the_cap/profiles", json!({ "FirstName": "Grace" }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case \"at-the-profile-cap\": expected 400 BadRequestException, got {status} {body:?}"
    );
    assert_eq!(
        state
            .lock()
            .domains
            .get("at_the_cap")
            .map(|domain| domain.profiles.len()),
        Some(MAX_PROFILES_PER_DOMAIN),
        "case \"at-the-profile-cap\": a refused create must not store anything"
    );
}
