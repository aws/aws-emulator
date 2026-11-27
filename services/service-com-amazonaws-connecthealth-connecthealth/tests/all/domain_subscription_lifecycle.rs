// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Domain -> Subscription lifecycle.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router's own dispatch — and asserts the RULE the operations follow
//! rather than one recorded response: a response member is whatever the store holds,
//! so a value written by one request is what a later request reads back.
//!
//! The id / ARN patterns and the `maxResults` range asserted here are the model's own
//! (`smithy_query(query="shape", shape="DomainId,SubscriptionId")`,
//! `smithy_query(query="operation", operation="ListDomains")`), quoted beside each
//! constant.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connecthealth_connecthealth::EmulatedService;

/// `DomainId`: length 20..25, pattern `^(hai-|dom-)[a-z0-9]+$`.
const DOMAIN_ID_PATTERN: &str = r"^(hai-|dom-)[a-z0-9]+$";
/// `SubscriptionId`: length 25..25, pattern `^sub-[a-zA-Z0-9]{21}$`.
const SUBSCRIPTION_ID_PATTERN: &str = r"^sub-[a-zA-Z0-9]{21}$";
/// `DomainArn`'s pattern.
const DOMAIN_ARN_PATTERN: &str =
    r"^arn:aws:health-agent:[a-z0-9-]+:[0-9]{12}:domain/(hai-|dom-)[a-z0-9]+$";
/// `SubscriptionArn`'s pattern.
const SUBSCRIPTION_ARN_PATTERN: &str = r"^arn:aws:health-agent:[a-z0-9-]+:[0-9]{12}:domain/(hai-|dom-)[a-z0-9]+/subscription/sub-[a-zA-Z0-9]{21}$";
/// The page-size bound both list operations enforce themselves — THIS EMULATOR's cap
/// on one page (`smithy_query` reports a `range {min: 1, max: 100}` for `maxResults`,
/// which the generated contract header does not restate). Asserted below at each end
/// and one past each.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);
/// How long a poll for the `DELETING` -> purged transition `state::DOMAIN_DELETION_SPAN`
/// owes keeps looking: many times over that span, so a loaded host has room while a fast
/// one never waits at all.
const PURGE_DEADLINE: Duration = Duration::from_secs(30);

/// One emulator: the shared engine, this service mounted on it, and the region every
/// request is signed for.
///
/// `pub(crate)` so the sibling suites of this test target drive the service exactly the
/// way this one does, rather than each growing its own copy of the harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
    /// The ONE shared engine this service was mounted on, kept so a suite can reach the
    /// service's own store directly — the way to seed a record no operation this
    /// emulator serves writes yet.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            region: region.to_string(),
            engine,
        }
    }

    /// The shared engine, for a suite that has to write a record directly into this
    /// service's store.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The region every [`Emulator::send`] of this emulator is signed for.
    pub(crate) fn region(&self) -> &str {
        &self.region
    }

    /// Dispatch one request and return its status and parsed JSON body.
    pub(crate) async fn send(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        self.send_in(&self.region, method, path, body).await
    }

    /// Dispatch one request SIGNED FOR `region`, against this same emulator and its one
    /// shared store — the way to address a second Region's endpoint without standing up a
    /// second emulator (which would have a store of its own and so could prove nothing
    /// about Region scoping).
    pub(crate) async fn send_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        dispatch_signed(&self.router, region, method, path, body).await
    }

    /// Dispatch one request carrying RAW BYTES and whatever headers the suite chose, and
    /// hand back the whole response — what an operation whose request body is not JSON,
    /// and whose answer is read off the response headers, has to be driven through.
    pub(crate) async fn send_bytes(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        self.send_bytes_in(&self.region, method, path, headers, body)
            .await
    }

    /// [`Emulator::send_bytes`], SIGNED FOR `region`.
    pub(crate) async fn send_bytes_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        dispatch_signed_bytes(&self.router, region, method, path, headers, body).await
    }

    /// `CreateDomain`, asserting it was accepted, and the created domain's id.
    pub(crate) async fn create_domain(&self, body: &Value) -> Value {
        self.create_domain_in(&self.region, body).await
    }

    /// `CreateDomain` SIGNED FOR `region`, asserting it was accepted.
    pub(crate) async fn create_domain_in(&self, region: &str, body: &Value) -> Value {
        let (status, created) = self.send_in(region, "POST", "/domain", Some(body)).await;
        assert_eq!(
            status, 201,
            "CreateDomain in {region} {body}: got {created:?}"
        );
        created
    }

    /// `CreateSubscription` under `domain_id`, asserting it was accepted, and the created
    /// subscription's id — the companion of [`Emulator::create_domain`], so a suite that
    /// needs a collection of subscriptions does not grow a copy of this walk.
    pub(crate) async fn create_subscription(&self, domain_id: &str) -> String {
        let (status, created) = self
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateSubscription under {domain_id} got {created:?}"
        );
        string_at(&created, "subscriptionId")
    }
}

/// Dispatch ONE signed restJson1 request against `router` and return its status and
/// parsed JSON body.
///
/// The one way this test target drives this service over the wire, free of any
/// particular emulator: [`Emulator::send_in`] is this function applied to the shared
/// in-memory store, and a suite that has to mount the service on a DIFFERENT engine
/// builds its own router and calls this directly rather than copying the request walk.
pub(crate) async fn dispatch_signed(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body
        .map(|body| serde_json::to_vec(body).expect("a serializable body"))
        .unwrap_or_default();
    let response = dispatch_signed_bytes(
        router,
        region,
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    )
    .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Dispatch ONE signed request with `headers` and `body` exactly as given, and hand back
/// the whole response.
///
/// The byte-level half of [`dispatch_signed`], which is this function with a JSON body
/// and a JSON content type: an operation that binds its members to request HEADERS and
/// answers in response headers cannot be driven through the JSON-shaped helper.
pub(crate) async fn dispatch_signed_bytes(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> http::Response<Bytes> {
    let credential = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/health-agent/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", credential);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("a well-formed request");
    router.dispatch(request).await
}

/// The wire error code (`__type`), without any namespace prefix.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// A required string member of a response.
pub(crate) fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {member} must be a string in {body:?}"))
        .to_string()
}

/// The two keys an error body of this service can carry its detail text under: `Message`,
/// which the subscription plane keys it by, and `message`, the member name the model
/// spells, which every other plane of this service and the framework's own refusal
/// envelope key it by (`src/ops/types/exception_wire.rs`).
pub(crate) const DETAIL_MEMBERS: [&str; 2] = ["Message", "message"];

/// The detail text of an error body, from whichever of [`DETAIL_MEMBERS`] the plane that
/// answered wrote it under — asserting the body states it under exactly ONE of them, so a
/// client never has to read two keys for one answer.
///
/// WHICH key each plane carries is the rule, and it is pinned per operation in
/// `tests/all/error_detail_member.rs`; a sibling suite about the TEXT a refusal states
/// reads it through here rather than growing a copy of the reader, because its rows
/// routinely span planes.
pub(crate) fn detail_text(case: &str, body: &Value) -> String {
    let stated: Vec<&str> = DETAIL_MEMBERS
        .into_iter()
        .filter(|member| body.get(*member).is_some())
        .collect();
    assert_eq!(
        stated.len(),
        1,
        "case {case:?}: an error body states its detail text under exactly one of \
         {DETAIL_MEMBERS:?}, got {stated:?} in {body}"
    );
    string_at(body, stated[0])
}

/// The member a reported subscription carries the instant of its last UPDATE in.
pub(crate) const UPDATE_INSTANT_MEMBER: &str = "lastUpdatedAt";

/// A reported subscription WITHOUT that member — what a create's 201 and a read of the
/// record it created have in common.
///
/// The create reports the subscription as creation established it, so it carries no
/// update instant for a record no call has updated, while a reader of the record reports
/// the instant it was last written. Comparing the two on everything else is what pins
/// that this is the ONE member they differ on.
pub(crate) fn without_update_instant(reported: &Value) -> Value {
    let mut stripped = reported.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove(UPDATE_INSTANT_MEMBER);
    }
    stripped
}

fn matches(pattern: &str, value: &str) -> bool {
    // The four patterns this suite checks, matched by hand rather than with a regex
    // engine (the sibling suites in this workspace do the same, and it keeps the
    // build's dependency set unchanged). Each arm follows its pattern constant above
    // literally, anchored at both ends.
    match pattern {
        DOMAIN_ID_PATTERN => {
            let body = value
                .strip_prefix("dom-")
                .or_else(|| value.strip_prefix("hai-"));
            body.is_some_and(|body| {
                !body.is_empty()
                    && body
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
        }
        SUBSCRIPTION_ID_PATTERN => value.strip_prefix("sub-").is_some_and(|body| {
            body.len() == 21 && body.bytes().all(|b| b.is_ascii_alphanumeric())
        }),
        DOMAIN_ARN_PATTERN => arn_resource(value).is_some_and(|resource| {
            resource
                .strip_prefix("domain/")
                .is_some_and(|id| matches(DOMAIN_ID_PATTERN, id))
        }),
        SUBSCRIPTION_ARN_PATTERN => arn_resource(value).is_some_and(|resource| {
            let Some(rest) = resource.strip_prefix("domain/") else {
                return false;
            };
            let Some((domain_id, subscription_id)) = rest.split_once("/subscription/") else {
                return false;
            };
            matches(DOMAIN_ID_PATTERN, domain_id)
                && matches(SUBSCRIPTION_ID_PATTERN, subscription_id)
        }),
        other => panic!("no hand-written matcher for pattern {other:?}"),
    }
}

/// The resource portion of `arn:aws:health-agent:<region>:<account>:<resource>`, when
/// the five leading fields are the ones every ARN pattern in this model declares:
/// literal `arn:aws:health-agent`, a `[a-z0-9-]+` region and a 12-digit account.
fn arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some("aws"), Some("health-agent"), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let region_ok = !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    (region_ok && account_ok).then_some(resource)
}

/// A `tags` map of `count` distinct entries, each key `key_len` and each value
/// `value_len` characters long.
///
/// `pub(crate)` so the sibling suites generate tag maps the same way rather than each
/// growing its own copy.
pub(crate) fn tag_map(count: usize, key_len: usize, value_len: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        let stem = format!("k{index}-");
        let padding = "a".repeat(key_len.saturating_sub(stem.len()));
        tags.insert(
            format!("{stem}{padding}"),
            Value::String("v".repeat(value_len)),
        );
    }
    Value::Object(tags)
}

/// Every read path's view of ONE subscription, labelled by the operation that produced
/// it: `GetSubscription`'s `subscription` member and the element `ListSubscriptions`
/// carries for the same id. Both project the same stored record through their own
/// serializer, so both must report the same members.
async fn read_paths(
    emulator: &Emulator,
    domain_id: &str,
    subscription_id: &str,
) -> Vec<(&'static str, Value)> {
    let (status, fetched) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "GetSubscription {subscription_id} got {fetched:?}"
    );
    let described = fetched
        .get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("GetSubscription must carry a subscription, got {fetched:?}"));

    let (status, page) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions?maxResults=100"),
            None,
        )
        .await;
    assert_eq!(status, 200, "ListSubscriptions got {page:?}");
    let element = page
        .get("subscriptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListSubscriptions must carry the required list, got {page:?}"))
        .iter()
        .find(|entry| string_at(entry, "subscriptionId") == subscription_id)
        .cloned()
        .unwrap_or_else(|| panic!("ListSubscriptions must list {subscription_id}, got {page:?}"));

    vec![("GetSubscription", described), ("ListSubscriptions", element)]
}

#[tokio::test]
async fn create_domain_persists_every_member_and_get_domain_reads_it_back() {
    // The rows differ along what CreateDomain generalises over: which optional members
    // are present, the size of `name`, and how many tags are supplied. The issue's own
    // case ({"name": "clinic-a"}) is just the first row.
    let kms_key = "arn:aws:kms:us-west-2:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    let web_app = json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    });
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({ "name": "clinic-a" })),
        ("name at the modelled minimum (1)", json!({ "name": "c" })),
        ("name at the modelled maximum (64)", json!({ "name": "c".repeat(64) })),
        (
            "customer-managed key",
            json!({ "name": "clinic-kms", "kmsKeyArn": kms_key }),
        ),
        (
            "one tag",
            json!({ "name": "clinic-tag", "tags": { "env": "test" } }),
        ),
        (
            "three tags",
            json!({ "name": "clinic-tags", "tags": { "env": "test", "team": "cardio", "cost": "42" } }),
        ),
        (
            "web app set up",
            json!({ "name": "clinic-web", "webAppSetupConfiguration": web_app }),
        ),
        (
            "every optional member at once",
            json!({
                "name": "clinic-everything",
                "kmsKeyArn": kms_key,
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": web_app,
            }),
        ),
    ];

    let emulator = Emulator::new("us-west-2");
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, request) in &cases {
        let created = emulator.create_domain(request).await;

        let domain_id = string_at(&created, "domainId");
        assert!(
            matches(DOMAIN_ID_PATTERN, &domain_id) && (20..=25).contains(&domain_id.len()),
            "case {case:?}: domainId must match {DOMAIN_ID_PATTERN} with length 20..25, got {domain_id:?}"
        );
        assert!(
            minted.insert(domain_id.clone()),
            "case {case:?}: domainId {domain_id} was minted twice"
        );
        let arn = string_at(&created, "arn");
        assert_eq!(
            arn,
            format!("arn:aws:health-agent:us-west-2:000000000000:domain/{domain_id}"),
            "case {case:?}: the ARN must name the minted domain"
        );
        assert_eq!(
            created.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: a new domain is ACTIVE, got {created:?}"
        );
        assert!(
            created.get("createdAt").and_then(Value::as_f64).unwrap_or(0.0) > 0.0,
            "case {case:?}: createdAt must be set, got {created:?}"
        );

        // The derived members: encryption follows the key the caller named, and a web
        // app exists only where one was asked for.
        let wanted_kms = request.get("kmsKeyArn").and_then(Value::as_str);
        let wanted_encryption = if wanted_kms.is_some() {
            "CUSTOMER_MANAGED_KEY"
        } else {
            "AWS_OWNED_KEY"
        };
        assert_eq!(
            created
                .get("encryptionContext")
                .and_then(|context| context.get("encryptionType"))
                .and_then(Value::as_str),
            Some(wanted_encryption),
            "case {case:?}: encryptionType must follow kmsKeyArn, got {created:?}"
        );
        assert_eq!(
            created.get("kmsKeyArn").and_then(Value::as_str),
            wanted_kms,
            "case {case:?}: kmsKeyArn must be the key the caller named, got {created:?}"
        );
        match request.get("webAppSetupConfiguration") {
            None => {
                assert!(
                    created.get("webAppConfiguration").is_none()
                        && created.get("webAppUrl").is_none(),
                    "case {case:?}: no web app was asked for, got {created:?}"
                );
            }
            Some(setup) => {
                let configured = created
                    .get("webAppConfiguration")
                    .unwrap_or_else(|| panic!("case {case:?}: webAppConfiguration in {created:?}"));
                assert_eq!(
                    configured.get("ehrRole"),
                    setup.get("ehrRole"),
                    "case {case:?}: the stored ehrRole is the one supplied"
                );
                assert_eq!(
                    configured.get("idcRegion"),
                    setup.get("idcRegion"),
                    "case {case:?}: the stored idcRegion is the one supplied"
                );
                let application_id = string_at(configured, "idcApplicationId");
                assert!(
                    !application_id.is_empty()
                        && Some(application_id.as_str())
                            != setup.get("idcInstanceId").and_then(Value::as_str),
                    "case {case:?}: the Identity Center APPLICATION id is created by the \
                     service, not the instance id it was given, got {application_id:?}"
                );
                let url = string_at(&created, "webAppUrl");
                assert!(
                    url.starts_with("https://") && url.contains(&domain_id),
                    "case {case:?}: webAppUrl must name the domain, got {url:?}"
                );
            }
        }

        // The round trip: GetDomain reports the PERSISTED record, member for member.
        let (status, fetched) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
        for member in [
            "domainId",
            "arn",
            "name",
            "kmsKeyArn",
            "status",
            "createdAt",
            "encryptionContext",
            "webAppUrl",
            "webAppConfiguration",
        ] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {case:?}: GetDomain must report the stored {member}"
            );
        }
        assert_eq!(
            fetched.get("name").and_then(Value::as_str),
            request.get("name").and_then(Value::as_str),
            "case {case:?}: the stored name is the one supplied"
        );
        // The tag map is ALWAYS reported — empty for a domain created without tags,
        // never absent.
        let stored_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched.get("tags"),
            Some(&stored_tags),
            "case {case:?}: GetDomain must report the tags CreateDomain stored, got {fetched:?}"
        );
    }

    // Every domain created above is listed, exactly once.
    let (status, listed) = emulator.send("GET", "/domain", None).await;
    assert_eq!(status, 200, "ListDomains got {listed:?}");
    let listed_ids: BTreeSet<String> = listed
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("domains must be a list in {listed:?}"))
        .iter()
        .map(|summary| string_at(summary, "domainId"))
        .collect();
    assert_eq!(
        listed_ids, minted,
        "ListDomains must report one summary per persisted domain"
    );
}

#[tokio::test]
async fn minted_ids_and_arns_match_the_model_patterns_in_every_signed_region() {
    // The rule generalises over the region the request is signed for: the ARN carries
    // that region, and whatever the request says, the minted ARN still satisfies the
    // model's own pattern (a region outside the pattern's `[a-z0-9-]+` falls back).
    let cases: [(&str, Option<&str>); 5] = [
        ("us-east-1", Some("us-east-1")),
        ("us-west-2", Some("us-west-2")),
        ("eu-central-1", Some("eu-central-1")),
        ("ap-southeast-4", Some("ap-southeast-4")),
        ("US_WEST_2", None),
    ];

    for (signed_region, expected_region) in cases {
        let emulator = Emulator::new(signed_region);
        let created = emulator.create_domain(&json!({ "name": "clinic" })).await;
        let domain_id = string_at(&created, "domainId");
        let domain_arn = string_at(&created, "arn");
        assert!(
            matches(DOMAIN_ARN_PATTERN, &domain_arn),
            "case {signed_region:?}: {domain_arn:?} must match {DOMAIN_ARN_PATTERN}"
        );
        if let Some(region) = expected_region {
            assert_eq!(
                domain_arn,
                format!("arn:aws:health-agent:{region}:000000000000:domain/{domain_id}"),
                "case {signed_region:?}: the ARN carries the region the request was signed for"
            );
        }

        let (status, subscription) = emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {signed_region:?}: CreateSubscription got {subscription:?}"
        );
        let subscription_id = string_at(&subscription, "subscriptionId");
        assert!(
            matches(SUBSCRIPTION_ID_PATTERN, &subscription_id) && subscription_id.len() == 25,
            "case {signed_region:?}: subscriptionId must match {SUBSCRIPTION_ID_PATTERN} with \
             length 25, got {subscription_id:?}"
        );
        let subscription_arn = string_at(&subscription, "arn");
        assert!(
            matches(SUBSCRIPTION_ARN_PATTERN, &subscription_arn),
            "case {signed_region:?}: {subscription_arn:?} must match {SUBSCRIPTION_ARN_PATTERN}"
        );
        assert_eq!(
            subscription_arn,
            format!("{domain_arn}/subscription/{subscription_id}"),
            "case {signed_region:?}: a subscription ARN extends its domain's"
        );
        assert_eq!(
            subscription.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {signed_region:?}: a new subscription is ACTIVE, got {subscription:?}"
        );
        assert!(
            subscription.get("lastUpdatedAt").is_none()
                && subscription.get("activatedAt").is_none()
                && subscription.get("deactivatedAt").is_none(),
            "case {signed_region:?}: a subscription that has taken no transition carries \
             none of the instants one records, got {subscription:?}"
        );
    }
}

#[tokio::test]
async fn subscription_status_transitions_are_persisted_and_idempotent() {
    // The rows differ along the sequence of transitions applied — including repeats.
    // `DeactivateSubscription` is the one of the two the model declares `@idempotent`,
    // so a repeat of it must neither error nor change the stored record;
    // `ActivateSubscription` carries no such trait, so every call of it runs the
    // transition and stamps `activatedAt`, including on a new subscription (which is
    // created ACTIVE without ever having been activated).
    let cases: [(&str, &[&str], &str, bool, bool); 7] = [
        ("no transition", &[], "ACTIVE", false, false),
        ("activate a new subscription", &["activate"], "ACTIVE", true, false),
        ("activate twice", &["activate", "activate"], "ACTIVE", true, false),
        ("deactivate first", &["deactivate"], "INACTIVE", false, true),
        (
            "deactivate twice",
            &["deactivate", "deactivate"],
            "INACTIVE",
            false,
            true,
        ),
        (
            "deactivate then activate",
            &["deactivate", "activate"],
            "ACTIVE",
            true,
            true,
        ),
        (
            "deactivate, activate, deactivate again",
            &["deactivate", "activate", "deactivate"],
            "INACTIVE",
            true,
            true,
        ),
    ];

    let emulator = Emulator::new("us-west-2");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic-subs" })).await,
        "domainId",
    );

    for (case, transitions, expected_status, activated, deactivated) in cases {
        let created = emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await
            .1;
        let subscription_id = string_at(&created, "subscriptionId");
        let path = format!("/domains/{domain_id}/subscriptions/{subscription_id}");

        let mut last = created.clone();
        for transition in transitions {
            let before = last.clone();
            let (status, answered) = emulator
                .send("POST", &format!("{path}/{transition}"), Some(&json!({})))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {transition} got {answered:?}"
            );
            last = answered
                .get("subscription")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: subscription in {answered:?}"));

            // The stamp this transition owns, and the one the OPPOSITE transition owns.
            let (stamped, other) = match *transition {
                "activate" => ("activatedAt", "deactivatedAt"),
                _ => ("deactivatedAt", "activatedAt"),
            };
            if *transition == "deactivate" && before.get("status") == last.get("status") {
                // `@idempotent`, which the model declares for `DeactivateSubscription`:
                // a call that does not move the status changes NOTHING, so nothing is
                // restamped and the whole record comes back identical.
                assert_eq!(
                    last, before,
                    "case {case:?}: a {transition} that does not move the status must \
                     change nothing, got {last:?}"
                );
            } else {
                // A real transition stamps its own boundary...
                assert!(
                    last.get(stamped).is_some(),
                    "case {case:?}: {transition} must stamp {stamped}, got {last:?}"
                );
                // ...and RETAINS the opposite one, so the interval it just ended stays
                // reconstructable.
                assert!(
                    before
                        .get(other)
                        .is_none_or(|kept| last.get(other) == Some(kept)),
                    "case {case:?}: {transition} must retain {other}, got {last:?} \
                     after {before:?}"
                );
            }
        }

        assert_eq!(
            last.get("status").and_then(Value::as_str),
            Some(expected_status),
            "case {case:?}: the stored status after {transitions:?} is wrong, got {last:?}"
        );
        assert_eq!(
            last.get("activatedAt").is_some(),
            activated,
            "case {case:?}: activatedAt presence after {transitions:?} is wrong, got {last:?}"
        );
        assert_eq!(
            last.get("deactivatedAt").is_some(),
            deactivated,
            "case {case:?}: deactivatedAt presence after {transitions:?} is wrong, got {last:?}"
        );

        // A read observes exactly what the transitions wrote.
        let (status, fetched) = emulator.send("GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: GetSubscription got {fetched:?}");
        let reported = fetched
            .get("subscription")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: subscription in {fetched:?}"));
        assert_eq!(
            without_update_instant(&reported),
            without_update_instant(&last),
            "case {case:?}: GetSubscription must report the stored subscription"
        );
        // A READ of the record reports the instant it was last written, whether or not a
        // transition has stamped one; the create is the one response that leaves the
        // member out until an update has happened.
        assert!(
            reported.get(UPDATE_INSTANT_MEMBER).is_some(),
            "case {case:?}: a read must report {UPDATE_INSTANT_MEMBER}, got {reported:?}"
        );

        // And so does the list of the domain's subscriptions.
        let (status, listed) = emulator
            .send(
                "GET",
                &format!("/domains/{domain_id}/subscriptions?maxResults=100"),
                None,
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: ListSubscriptions got {listed:?}");
        let found = listed
            .get("subscriptions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: subscriptions in {listed:?}"))
            .iter()
            .find(|entry| string_at(entry, "subscriptionId") == subscription_id)
            .cloned();
        assert_eq!(
            found.as_ref().map(without_update_instant),
            Some(without_update_instant(&last)),
            "case {case:?}: ListSubscriptions must report the stored subscription"
        );
        assert_eq!(
            found.as_ref().and_then(|entry| entry.get(UPDATE_INSTANT_MEMBER)),
            reported.get(UPDATE_INSTANT_MEMBER),
            "case {case:?}: both readers of the record must report one \
             {UPDATE_INSTANT_MEMBER}, got {found:?}"
        );
    }
}

#[tokio::test]
async fn list_domains_pages_deterministically_and_filters_by_status() {
    let emulator = Emulator::new("eu-west-1");
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..5 {
        let domain = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        created.insert(string_at(&domain, "domainId"));
    }

    // Every page size from 1 to one past the number of domains walks the whole set
    // exactly once, in one deterministic order.
    for page_size in 1..=6 {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &token {
                None => format!("/domain?maxResults={page_size}"),
                Some(token) => format!("/domain?maxResults={page_size}&nextToken={token}"),
            };
            let (status, page) = emulator.send("GET", &path, None).await;
            assert_eq!(status, 200, "page size {page_size}: got {page:?}");
            let entries = page
                .get("domains")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("page size {page_size}: domains in {page:?}"));
            assert!(
                entries.len() <= page_size,
                "page size {page_size}: a page returned {} entries",
                entries.len()
            );
            seen.extend(entries.iter().map(|entry| string_at(entry, "domainId")));
            pages += 1;
            assert!(pages <= 10, "page size {page_size}: pagination did not finish");
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "page size {page_size}: a domain was returned twice across pages ({seen:?})"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "page size {page_size}: paging must cover every persisted domain"
        );
        assert_eq!(
            seen, sorted,
            "page size {page_size}: pages must follow one deterministic order"
        );
    }

    // Filtering keys on the stored status, and the required list is present even when
    // nothing matches.
    for (case, status_filter, expected) in [
        ("ACTIVE matches every live domain", "ACTIVE", 5),
        ("DELETING matches none yet", "DELETING", 0),
        ("DELETED matches none yet", "DELETED", 0),
    ] {
        let (status, page) = emulator
            .send("GET", &format!("/domain?status={status_filter}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: got {page:?}");
        assert_eq!(
            page.get("domains").and_then(Value::as_array).map(Vec::len),
            Some(expected),
            "case {case:?}: got {page:?}"
        );
    }
}

#[tokio::test]
async fn list_domains_refuses_a_max_results_outside_the_modelled_range() {
    let emulator = Emulator::new("us-east-1");
    emulator.create_domain(&json!({ "name": "clinic" })).await;

    let (min, max) = MAX_RESULTS_RANGE;
    // Each published bound, and one past each.
    let cases: [(i64, u16); 4] = [(min - 1, 400), (min, 200), (max, 200), (max + 1, 400)];
    for (max_results, expected) in cases {
        let (status, body) = emulator
            .send("GET", &format!("/domain?maxResults={max_results}"), None)
            .await;
        assert_eq!(
            status, expected,
            "case maxResults={max_results}: got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case maxResults={max_results}: got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_subscriptions_pages_through_every_persisted_subscription() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let mut created: BTreeSet<String> = BTreeSet::new();
    for _ in 0..4 {
        let (status, subscription) = emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(status, 201, "CreateSubscription got {subscription:?}");
        created.insert(string_at(&subscription, "subscriptionId"));
    }
    assert_eq!(created.len(), 4, "every subscription id must be distinct");

    for page_size in [1, 2, 3, 4, 5] {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let base = format!("/domains/{domain_id}/subscriptions?maxResults={page_size}");
            let path = match &token {
                None => base,
                Some(token) => format!("{base}&nextToken={token}"),
            };
            let (status, page) = emulator.send("GET", &path, None).await;
            assert_eq!(status, 200, "page size {page_size}: got {page:?}");
            for entry in page
                .get("subscriptions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("page size {page_size}: subscriptions in {page:?}"))
            {
                assert!(
                    seen.insert(string_at(entry, "subscriptionId")),
                    "page size {page_size}: a subscription was returned twice"
                );
            }
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "page size {page_size}: paging must cover every persisted subscription"
        );
    }
}

#[tokio::test]
async fn unknown_resources_are_404_and_ids_outside_the_model_are_400() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let subscription_id = string_at(
        &emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await
            .1,
        "subscriptionId",
    );
    // A well-formed id (len 21, `^dom-[a-z0-9]+$`) that names nothing.
    let absent_domain = "dom-doesnotexist00000";
    let absent_subscription = "sub-000000000000000000000";
    // A domain this service DID allocate and has since deleted: its id names a domain
    // that is gone, which is the modelled not-found — the same answer an id this service
    // never allocated earns, since what the Region HOLDS is what decides it.
    let deleted_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-deleted" }))
            .await,
        "domainId",
    );
    let (status, body) = emulator
        .send("DELETE", &format!("/domain/{deleted_domain}"), None)
        .await;
    assert_eq!(status, 200, "the first DeleteDomain got {status} {body:?}");

    let not_found: [(&str, &str, &str); 7] = [
        ("GetDomain", "GET", &format!("/domain/{absent_domain}")),
        (
            "DeleteDomain of a domain that is gone",
            "DELETE",
            &format!("/domain/{deleted_domain}"),
        ),
        (
            "CreateSubscription under an absent domain",
            "POST",
            &format!("/domains/{absent_domain}/subscriptions"),
        ),
        (
            "GetSubscription of an absent subscription",
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{absent_subscription}"),
        ),
        (
            "ActivateSubscription of an absent subscription",
            "POST",
            &format!("/domains/{domain_id}/subscriptions/{absent_subscription}/activate"),
        ),
        (
            "DeactivateSubscription under an absent domain",
            "POST",
            &format!("/domains/{absent_domain}/subscriptions/{subscription_id}/deactivate"),
        ),
        (
            "ListSubscriptions of an absent domain",
            "GET",
            &format!("/domains/{absent_domain}/subscriptions"),
        ),
    ];
    for (case, method, path) in not_found {
        let (status, body) = emulator.send(method, path, Some(&json!({}))).await;
        assert_eq!(status, 404, "case {case:?}: got {status} {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body:?}"
        );
    }

    // A `domainId` outside the model's length/pattern is a refusal, never a 404 and
    // never a 200: each row breaks one rule the model states.
    let malformed: [(&str, &str); 5] = [
        ("too short (19)", "dom-aaaaaaaaaaaaaaa"),
        ("too long (26)", "dom-aaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase, outside the character class", "dom-AAAAAAAAAAAAAAAA"),
        ("underscore, outside the character class", "dom-aaaaaaaaaaaa_aaa"),
        ("an unmodelled prefix", "zzz-aaaaaaaaaaaaaaaa"),
    ];
    for (case, bad_id) in malformed {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{bad_id}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({bad_id}): got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({bad_id}): got {body:?}"
        );
    }
}

#[tokio::test]
async fn store_growth_is_bounded_and_refused_with_the_modelled_errors() {
    use service_com_amazonaws_connecthealth_connecthealth::state::{
        MAX_SUBSCRIPTIONS_PER_DOMAIN, MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
    };

    // A domain whose subscriptions are at the per-domain cap refuses the next one with
    // the operation's own quota error. (The account-wide Domain cap is its own rule,
    // asserted in `domain_account_cap_and_not_found_semantics`.)
    let emulator = Emulator::new("us-east-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let path = format!("/domains/{domain_id}/subscriptions");
    for index in 0..MAX_SUBSCRIPTIONS_PER_DOMAIN {
        let (status, body) = emulator.send("POST", &path, Some(&json!({}))).await;
        assert_eq!(
            status, 201,
            "subscription {index} of the cap: got {status} {body:?}"
        );
    }
    let (status, body) = emulator.send("POST", &path, Some(&json!({}))).await;
    assert_eq!(
        status, 402,
        "the subscription past the cap of {MAX_SUBSCRIPTIONS_PER_DOMAIN}: got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "the subscription past the cap: got {body:?}"
    );

    // The tag store is bounded the same way: each published tagging limit is accepted
    // and one past it is refused.
    let cases: [(&str, usize, usize, usize, u16); 6] = [
        ("tags at the maximum count", MAX_TAGS_PER_RESOURCE, 8, 8, 201),
        (
            "one tag past the maximum count",
            MAX_TAGS_PER_RESOURCE + 1,
            8,
            8,
            402,
        ),
        ("a key at the maximum length", 1, MAX_TAG_KEY_LEN, 8, 201),
        (
            "a key one past the maximum length",
            1,
            MAX_TAG_KEY_LEN + 1,
            8,
            400,
        ),
        ("a value at the maximum length", 1, 8, MAX_TAG_VALUE_LEN, 201),
        (
            "a value one past the maximum length",
            1,
            8,
            MAX_TAG_VALUE_LEN + 1,
            400,
        ),
    ];
    let emulator = Emulator::new("us-east-1");
    for (case, count, key_len, value_len, expected) in cases {
        let request = json!({
            "name": "clinic-tagged",
            "tags": tag_map(count, key_len, value_len),
        });
        let (status, body) = emulator.send("POST", "/domain", Some(&request)).await;
        assert_eq!(status, expected, "case {case:?}: got {status} {body:?}");
        match expected {
            201 => {
                // Accepted at the bound, and every stored tag is readable back.
                let domain_id = string_at(&body, "domainId");
                let (status, fetched) = emulator
                    .send("GET", &format!("/domain/{domain_id}"), None)
                    .await;
                assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
                assert_eq!(
                    fetched
                        .get("tags")
                        .and_then(Value::as_object)
                        .map(serde_json::Map::len),
                    Some(count),
                    "case {case:?}: every stored tag must be reported, got {fetched:?}"
                );
            }
            402 => assert_eq!(
                error_code(&body),
                "ServiceQuotaExceededException",
                "case {case:?}: got {body:?}"
            ),
            _ => assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body:?}"
            ),
        }
    }
}

#[tokio::test]
async fn delete_domain_moves_the_domain_to_deleting_then_purges_it() {
    let emulator = Emulator::new("us-west-2");
    // A domain with a web application is the case whose deletion cannot finish inside
    // the call (the Identity Center application it owns lives in another service), so
    // it is the one that passes through the non-terminal `DELETING`.
    let created = emulator
        .create_domain(&json!({
            "name": "clinic-doomed",
            "tags": { "env": "test" },
            "webAppSetupConfiguration": {
                "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
                "idcInstanceId": "ssoins-72230ed02aa4b4ea",
                "idcRegion": "us-east-1",
            },
        }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription:?}");
    let subscription_id = string_at(&subscription, "subscriptionId");

    // The delete starts the transition; the response reports the stored record.
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {deleted:?}");
    assert_eq!(
        (
            deleted.get("domainId").and_then(Value::as_str),
            deleted.get("arn").and_then(Value::as_str),
            deleted.get("status").and_then(Value::as_str),
        ),
        (Some(domain_id.as_str()), Some(arn.as_str()), Some("DELETING")),
        "DeleteDomain must report the domain it moved out of ACTIVE, got {deleted:?}"
    );

    // Straight after the call the transient state is observable, and `@idempotent`
    // means repeating the delete neither errors nor changes it.
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain got {fetched:?}");
    assert_eq!(
        fetched.get("status").and_then(Value::as_str),
        Some("DELETING"),
        "GetDomain must report the stored deletion, got {fetched:?}"
    );
    let (status, repeated) = emulator
        .send("DELETE", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "a repeated DeleteDomain got {repeated:?}");
    assert_eq!(
        repeated, deleted,
        "a repeated DeleteDomain must answer exactly as the first did"
    );
    let (status, listed) = emulator
        .send("GET", "/domain?status=DELETING", None)
        .await;
    assert_eq!(status, 200, "ListDomains got {listed:?}");
    assert_eq!(
        listed
            .get("domains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "a domain being deleted is listed under DELETING, got {listed:?}"
    );

    // ONE liveness rule across every subscription operation: a domain that is being
    // deleted no longer serves its subscriptions, so nothing can be enumerated that
    // cannot then be read.
    let subscriptions_path = format!("/domains/{domain_id}/subscriptions");
    let subscription_path = format!("{subscriptions_path}/{subscription_id}");
    let gone: [(&str, &str, &str); 5] = [
        ("ListSubscriptions", "GET", &subscriptions_path),
        ("CreateSubscription", "POST", &subscriptions_path),
        ("GetSubscription", "GET", &subscription_path),
        (
            "ActivateSubscription",
            "POST",
            &format!("{subscription_path}/activate"),
        ),
        (
            "DeactivateSubscription",
            "POST",
            &format!("{subscription_path}/deactivate"),
        ),
    ];
    for (case, method, path) in gone {
        let (status, body) = emulator.send(method, path, Some(&json!({}))).await;
        assert_eq!(
            status, 404,
            "case {case:?} under a domain being deleted: got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} under a domain being deleted: got {body:?}"
        );
    }

    // Once the span the clock owes has fallen due, the domain is purged: it is gone from
    // the list and every read of it is a 404. The purge is observed by LOOKING again, not
    // by waiting a fixed interval: it falls due at an instant, and a fixed wait would make
    // the outcome depend on how fast the host runs.
    let deadline = std::time::Instant::now() + PURGE_DEADLINE;
    loop {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        if status == 404 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the record under {domain_id} was still held after {PURGE_DEADLINE:?}, last \
             read {status} with {body:?}"
        );
        ::tokio::task::yield_now().await;
    }
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 404, "GetDomain after the purge got {fetched:?}");
    assert_eq!(
        error_code(&fetched),
        "ResourceNotFoundException",
        "GetDomain after the purge got {fetched:?}"
    );
    let (status, listed) = emulator.send("GET", "/domain", None).await;
    assert_eq!(status, 200, "ListDomains got {listed:?}");
    assert_eq!(
        listed
            .get("domains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a purged domain is listed nowhere, got {listed:?}"
    );
    let (status, body) = emulator
        .send("DELETE", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 404, "DeleteDomain after the purge got {body:?}");
}

#[tokio::test]
async fn a_new_subscription_is_active_everywhere_and_its_deactivation_stamp_is_retained() {
    // The two rules this pins:
    //
    //   * a subscription is created in the enabled status `CreateSubscription` puts it
    //     in ("Creates a new Subscription within a Domain for billing and user
    //     management"), and EVERY projection of the stored record — the create response,
    //     `GetSubscription`, `ListSubscriptions`, and the transition responses — reports
    //     that one persisted status;
    //   * `activatedAt` / `deactivatedAt` record that their transition HAPPENED: each is
    //     stamped when the transition runs and is retained afterwards, so the opposite
    //     transition does not clear it.
    //
    // The rows differ along how many subscriptions the domain holds, which of them the
    // transitions are applied to, and the region the requests are signed for: the rules
    // are about one resource's lifecycle, so its neighbours must be untouched however
    // many there are and wherever the list puts them.
    let cases: [(&str, usize, usize); 4] = [
        ("one subscription", 1, 0),
        ("three, first targeted", 3, 0),
        ("three, last targeted", 3, 2),
        ("five, middle targeted", 5, 2),
    ];
    // The signed regions are rotated across the rows so the rules are not checked in
    // one region only; each is a value the `SubscriptionArn` pattern's `[a-z0-9-]+`
    // region segment admits.
    let regions = ["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-4"];

    for ((case, count, target_index), region) in cases.into_iter().zip(regions) {
        let emulator = Emulator::new(region);
        let domain_id = string_at(
            &emulator
                .create_domain(&json!({ "name": "clinic-billing" }))
                .await,
            "domainId",
        );

        let mut ids: Vec<String> = Vec::new();
        for index in 0..count {
            let (status, created) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?} #{index}: CreateSubscription got {created:?}"
            );
            assert_eq!(
                created.get("status").and_then(Value::as_str),
                Some("ACTIVE"),
                "case {case:?} #{index}: a created subscription is ACTIVE, got {created:?}"
            );
            assert!(
                created.get("activatedAt").is_none() && created.get("deactivatedAt").is_none(),
                "case {case:?} #{index}: a created subscription has run neither transition, \
                 so it carries neither stamp, got {created:?}"
            );
            ids.push(string_at(&created, "subscriptionId"));
        }

        // Every read path agrees with the create response, element by element.
        for id in &ids {
            for (op, view) in read_paths(&emulator, &domain_id, id).await {
                assert_eq!(
                    (
                        view.get("status").and_then(Value::as_str),
                        view.get("deactivatedAt")
                    ),
                    (Some("ACTIVE"), None),
                    "case {case:?}: {op} must read the created subscription back as ACTIVE \
                     with no deactivation stamp, got {view:?}"
                );
            }
        }

        // Deactivating stamps the billing-stop boundary, and the response carries it.
        let target = ids[target_index].clone();
        let path = format!("/domains/{domain_id}/subscriptions/{target}");
        let (status, answered) = emulator
            .send("POST", &format!("{path}/deactivate"), Some(&json!({})))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeactivateSubscription got {answered:?}"
        );
        let deactivated = answered
            .get("subscription")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: subscription in {answered:?}"));
        assert_eq!(
            deactivated.get("status").and_then(Value::as_str),
            Some("INACTIVE"),
            "case {case:?}: a deactivated subscription is INACTIVE, got {deactivated:?}"
        );
        let stamp = deactivated.get("deactivatedAt").cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: DeactivateSubscription must stamp deactivatedAt, \
                 got {deactivated:?}"
            )
        });
        assert!(
            stamp.as_f64().is_some_and(|epoch| epoch > 0.0),
            "case {case:?}: deactivatedAt must be an epoch-seconds timestamp, got {stamp:?}"
        );

        // Both read paths report the stored stamp...
        for (op, view) in read_paths(&emulator, &domain_id, &target).await {
            assert_eq!(
                (
                    view.get("status").and_then(Value::as_str),
                    view.get("deactivatedAt")
                ),
                (Some("INACTIVE"), Some(&stamp)),
                "case {case:?}: {op} must report the stored deactivation, got {view:?}"
            );
        }
        // ...and the neighbours are untouched by it.
        for id in ids.iter().filter(|id| **id != target) {
            for (op, view) in read_paths(&emulator, &domain_id, id).await {
                assert_eq!(
                    (
                        view.get("status").and_then(Value::as_str),
                        view.get("deactivatedAt")
                    ),
                    (Some("ACTIVE"), None),
                    "case {case:?}: {op} must leave the neighbours of the deactivated \
                     subscription alone, got {view:?}"
                );
            }
        }

        // Re-activating RETAINS the deactivation stamp, so the inactive interval the
        // subscription just left stays reconstructable.
        let (status, answered) = emulator
            .send("POST", &format!("{path}/activate"), Some(&json!({})))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ActivateSubscription got {answered:?}"
        );
        let activated = answered
            .get("subscription")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: subscription in {answered:?}"));
        assert_eq!(
            (
                activated.get("status").and_then(Value::as_str),
                activated.get("deactivatedAt")
            ),
            (Some("ACTIVE"), Some(&stamp)),
            "case {case:?}: re-activation must keep the prior deactivatedAt, got {activated:?}"
        );
        assert!(
            activated.get("activatedAt").is_some(),
            "case {case:?}: an explicit activation stamps activatedAt, got {activated:?}"
        );
        for (op, view) in read_paths(&emulator, &domain_id, &target).await {
            assert_eq!(
                (
                    view.get("status").and_then(Value::as_str),
                    view.get("deactivatedAt")
                ),
                (Some("ACTIVE"), Some(&stamp)),
                "case {case:?}: {op} must still report the retained deactivatedAt, got {view:?}"
            );
        }
    }
}
