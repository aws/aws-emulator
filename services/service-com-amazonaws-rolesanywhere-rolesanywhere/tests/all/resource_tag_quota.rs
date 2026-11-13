// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the per-resource tag quota, read back through
//! `ListTagsForResource`.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test: a resource's tag set is addressed by tag key and holds at most
//! `MAX_TAGS_PER_RESOURCE` tags, so a write whose resultant set would hold more distinct
//! keys than that is refused — a 400 `ValidationException` naming the quota — and stores
//! nothing, whichever door the tags come through (the resource's own create, or a later
//! `TagResource`). The tables below therefore span the doors (every taggable resource
//! type's create, plus `TagResource`), the number of keys already held and newly
//! submitted, the bound itself and counts past it, lists whose entries repeat a key,
//! how many of a write's keys the resource already carries (which consume no slot), and
//! how many slots an `UntagResource` freed before the write is retried.
//!
//! The harness below is local to this suite: each sibling suite's client is private to
//! its own module, and the targets are compiled into one test binary only as separate
//! modules.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rolesanywhere_rolesanywhere::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form; `ImportCrlRequest$crlData` is an opaque `Blob`.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the tests sign for.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published `TagList` `@length` maximum — the most entries ONE call's list may carry
/// (`smithy_query(query="shape", shape="TagList")`: `@length(0, 200)`). A longer list is a
/// boundary constraint failure rather than a tag-quota refusal.
const MAX_TAG_LIST_LENGTH: usize = 200;

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn tag_resource(&self, resource_arn: &str, tags: &[(String, String)]) -> Answer {
        self.send(
            "POST",
            "/TagResource",
            Some(json!({"resourceArn": resource_arn, "tags": tag_json(tags)})),
        )
        .await
    }

    async fn untag_resource(&self, resource_arn: &str, tag_keys: &[String]) -> Answer {
        self.send(
            "POST",
            "/UntagResource",
            Some(json!({"resourceArn": resource_arn, "tagKeys": tag_keys})),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.send(
            "GET",
            &format!("/ListTagsForResource?resourceArn={resource_arn}"),
            None,
        )
        .await
    }

    /// Create a resource of `kind` named `name` carrying `tags`, answering the create's
    /// own reply so a case can assert either a success or a refusal.
    async fn create(&self, kind: Kind, name: &str, tags: &[(String, String)]) -> Answer {
        let trust_anchor_arn = match kind {
            // A CRL is imported AGAINST a trust anchor, and `ImportCrl` resolves that
            // reference, so a CRL case mints the anchor it links to first.
            Kind::Crl => self.minted_trust_anchor_arn(&format!("{name}-anchor")).await,
            Kind::TrustAnchor | Kind::Profile => String::new(),
        };
        let (uri, mut body) = match kind {
            Kind::TrustAnchor => (
                "/trustanchors",
                json!({
                    "name": name,
                    "source": {
                        "sourceType": "CERTIFICATE_BUNDLE",
                        "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                    },
                }),
            ),
            Kind::Profile => (
                "/profiles",
                json!({
                    "name": name,
                    "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/TagQuotaRole")],
                }),
            ),
            Kind::Crl => (
                "/crls",
                json!({
                    "name": name,
                    "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
                    "trustAnchorArn": trust_anchor_arn,
                }),
            ),
        };
        body["tags"] = tag_json(tags);
        self.send("POST", uri, Some(body)).await
    }

    /// Create a resource that must succeed, and answer the ARN it was minted with.
    async fn created_arn(&self, kind: Kind, name: &str, tags: &[(String, String)]) -> String {
        let answer = self.create(kind, name, tags).await;
        assert_eq!(
            answer.status,
            201,
            "case {name:?}: expected the {} create to answer 201, got {answer:?}",
            kind.label()
        );
        answer
            .body
            .pointer(kind.arn_pointer())
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: no ARN at {} in {answer:?}",
                    kind.arn_pointer()
                )
            })
            .to_string()
    }

    /// Mint an untagged trust anchor and answer its ARN. Sent directly rather than
    /// through [`Self::create`], which is what a CRL case calls.
    async fn minted_trust_anchor_arn(&self, name: &str) -> String {
        let answer = self
            .send(
                "POST",
                "/trustanchors",
                Some(json!({
                    "name": name,
                    "source": {
                        "sourceType": "CERTIFICATE_BUNDLE",
                        "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                    },
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "trust anchor {name:?}: expected CreateTrustAnchor 201, got {answer:?}"
        );
        answer
            .body
            .pointer(Kind::TrustAnchor.arn_pointer())
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("trust anchor {name:?}: no ARN in {answer:?}"))
            .to_string()
    }

    /// How many resources of `kind` this emulator holds, so a refused create can be shown
    /// to have stored nothing.
    async fn stored_count(&self, kind: Kind) -> usize {
        let answer = self
            .send("GET", &format!("{}?pageSize=100", kind.list_uri()), None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "expected the {} list to answer 200, got {answer:?}",
            kind.label()
        );
        answer
            .body
            .get(kind.list_member())
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "expected a {} list in {answer:?}",
                    kind.list_member()
                )
            })
            .len()
    }
}

/// The wire form of a tag list: `TagList` of `Tag { key, value }`.
fn tag_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// `count` distinct tags whose keys are prefixed with `prefix`.
fn tags(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}-{index:03}"), format!("value-{index:03}")))
        .collect()
}

/// The tags a `ListTagsForResource` answer reports, keyed by tag key. A duplicated key
/// would collapse here, so the entry count is asserted against the list length too.
fn listed_tags(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let listed = answer
        .body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {answer:?}"));
    let keyed: BTreeMap<String, String> = listed
        .iter()
        .map(|tag| {
            let key = tag
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a key in {answer:?}"))
                .to_string();
            let value = tag
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a value in {answer:?}"))
                .to_string();
            (key, value)
        })
        .collect();
    assert_eq!(
        keyed.len(),
        listed.len(),
        "case {case:?}: every key must appear at most once, got {answer:?}"
    );
    keyed
}

/// Which taggable resource a case goes through. Each one has its own create, and the rule
/// has to hold at every one of them.
#[derive(Debug, Clone, Copy)]
enum Kind {
    TrustAnchor,
    Profile,
    Crl,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "trust-anchor",
            Kind::Profile => "profile",
            Kind::Crl => "crl",
        }
    }

    fn arn_pointer(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "/trustAnchor/trustAnchorArn",
            Kind::Profile => "/profile/profileArn",
            Kind::Crl => "/crl/crlArn",
        }
    }

    fn list_uri(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "/trustanchors",
            Kind::Profile => "/profiles",
            Kind::Crl => "/crls",
        }
    }

    fn list_member(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "trustAnchors",
            Kind::Profile => "profiles",
            Kind::Crl => "crls",
        }
    }
}

/// The message this service refuses a tag write past the per-resource quota with: the
/// fixed wording, naming the quota. No tag is echoed into it, since `TagKey` and
/// `TagValue` are both `@sensitive`.
fn past_cap_message() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// A create's tag set is bounded by the per-resource quota: at the bound it is stored
/// and reported in full, and one key past it is refused with the quota's validation
/// error, storing no resource at all.
#[tokio::test]
async fn a_create_stores_tags_up_to_the_quota_and_refuses_a_list_past_it() {
    // Row: how many distinct keys the create's list names. The bound and its neighbours,
    // plus a count far past it that is still within the published `TagList` constraint.
    let counts = [
        0,
        1,
        MAX_TAGS_PER_RESOURCE - 1,
        MAX_TAGS_PER_RESOURCE,
        MAX_TAGS_PER_RESOURCE + 1,
        MAX_TAG_LIST_LENGTH,
    ];

    for kind in [Kind::TrustAnchor, Kind::Profile, Kind::Crl] {
        let client = Client::new();
        for count in counts {
            let case = format!("{}/create-with-{count}-tags", kind.label());
            // `ResourceName` is `@pattern("^[ a-zA-Z0-9-_]*$")`, so the count names the
            // resource.
            let name = format!("quota-{}-{count}", kind.label());
            // A CRL case mints its own anchor, so the anchors held before this create is
            // sent is the baseline a refusal is checked against.
            let before = client.stored_count(kind).await;
            let submitted = tags("created", count);
            let answer = client.create(kind, &name, &submitted).await;

            if count <= MAX_TAGS_PER_RESOURCE {
                assert_eq!(
                    answer.status, 201,
                    "case {case:?}: {count} tags is within the quota of \
                     {MAX_TAGS_PER_RESOURCE}, got {answer:?}"
                );
                let arn = answer
                    .body
                    .pointer(kind.arn_pointer())
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no ARN in {answer:?}"))
                    .to_string();
                let listed = client.list_tags(&arn).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected ListTagsForResource 200, got {listed:?}"
                );
                assert_eq!(
                    listed_tags(&case, &listed),
                    submitted.iter().cloned().collect::<BTreeMap<_, _>>(),
                    "case {case:?}: the create's whole tag set must be what the read reports"
                );
                continue;
            }

            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: {count} tags is past the quota of {MAX_TAGS_PER_RESOURCE} \
                 and must be refused, got {answer:?}"
            );
            assert_eq!(
                answer.body.get("message").and_then(Value::as_str),
                Some(past_cap_message().as_str()),
                "case {case:?}: expected the constraint wording, got {answer:?}"
            );
            assert_eq!(
                client.stored_count(kind).await,
                before,
                "case {case:?}: a refused create must store no resource"
            );
        }
    }
}

/// One row of the key-addressing table: the list a create carries, and the set a later
/// read must report for it.
struct KeyedCase {
    name: &'static str,
    submitted: Vec<(String, String)>,
    expected: BTreeMap<String, String>,
}

/// A list whose entries repeat a key names ONE tag: the stored set is addressed by key,
/// so a repeated key is held once with the value that came last, and the quota counts
/// distinct keys rather than list entries.
#[tokio::test]
async fn a_create_holds_one_entry_per_key() {
    let client = Client::new();

    // A list at the published `TagList` maximum whose entries cycle through exactly
    // `MAX_TAGS_PER_RESOURCE` keys: far more entries than one resource may hold, naming
    // not one key too many.
    let keys = MAX_TAGS_PER_RESOURCE;
    let distinct = tags("cycled", keys);
    let repeated: Vec<(String, String)> = (0..MAX_TAG_LIST_LENGTH)
        .map(|index| {
            let (key, _) = &distinct[index % keys];
            (key.clone(), format!("round-{}", index / keys))
        })
        .collect();

    let rows = vec![
        KeyedCase {
            name: "a-key-repeated-twice-keeps-its-last-value",
            submitted: vec![
                ("dup".to_string(), "first".to_string()),
                ("dup".to_string(), "second".to_string()),
            ],
            expected: [("dup".to_string(), "second".to_string())]
                .into_iter()
                .collect(),
        },
        KeyedCase {
            name: "a-list-at-the-tag-list-maximum-naming-keys-within-the-quota",
            expected: repeated.iter().cloned().collect(),
            submitted: repeated,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let KeyedCase {
            name: case,
            submitted,
            expected,
        } = row;
        let arn = client
            .created_arn(Kind::TrustAnchor, &format!("keyed-{index}"), &submitted)
            .await;
        let listed = client.list_tags(&arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {listed:?}"
        );
        assert_eq!(
            listed_tags(case, &listed),
            expected,
            "case {case:?}: the stored set holds one entry per key"
        );
    }
}

/// The quota bounds what a resource ends up holding, so what a `TagResource` may add
/// depends on what the resource was created with: the merged set is accepted up to the
/// bound and refused past it with the quota's validation error, which stores nothing.
#[tokio::test]
async fn tag_resource_is_bounded_by_the_set_the_create_left() {
    let client = Client::new();
    // Row: distinct keys the create carried, then distinct keys a later `TagResource`
    // adds. Accepted exactly while their sum is within the quota.
    let rows = [
        (0, 1),
        (0, MAX_TAGS_PER_RESOURCE),
        (1, MAX_TAGS_PER_RESOURCE - 1),
        (MAX_TAGS_PER_RESOURCE, 0),
        (MAX_TAGS_PER_RESOURCE - 1, 1),
        (MAX_TAGS_PER_RESOURCE, 1),
        (1, MAX_TAGS_PER_RESOURCE),
        (MAX_TAGS_PER_RESOURCE / 2, MAX_TAGS_PER_RESOURCE),
        (0, MAX_TAGS_PER_RESOURCE + 1),
        (0, MAX_TAG_LIST_LENGTH),
    ];

    for (index, (held, added)) in rows.into_iter().enumerate() {
        let case = format!("held-{held}/added-{added}");
        let created = tags("held", held);
        let arn = client
            .created_arn(Kind::TrustAnchor, &format!("merge-{index}"), &created)
            .await;
        let submitted = tags("added", added);
        let answer = client.tag_resource(&arn, &submitted).await;

        let mut expected: BTreeMap<String, String> = created.iter().cloned().collect();
        if held + added <= MAX_TAGS_PER_RESOURCE {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: {} tags is within the quota of {MAX_TAGS_PER_RESOURCE}, \
                 got {answer:?}",
                held + added
            );
            expected.extend(submitted.iter().cloned());
        } else {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: {} tags is past the quota of {MAX_TAGS_PER_RESOURCE} and \
                 must be refused, got {answer:?}",
                held + added
            );
            assert_eq!(
                answer.body.get("message").and_then(Value::as_str),
                Some(past_cap_message().as_str()),
                "case {case:?}: expected the quota wording, got {answer:?}"
            );
        }

        let listed = client.list_tags(&arn).await;
        assert_eq!(
            listed_tags(&case, &listed),
            expected,
            "case {case:?}: a refused call stores nothing and an accepted one stores \
             exactly the merged set, got {listed:?}"
        );
    }
}

/// The quota bounds the RESULTANT set, so a resource sitting exactly at the quota still
/// accepts a write whose keys it already carries — re-tagging replaces values in place
/// and consumes no slot — while a write naming even one key it does not carry is
/// refused, however many of the submitted keys are re-tags.
#[tokio::test]
async fn a_resource_at_the_quota_retags_in_place_and_refuses_one_new_key() {
    // Row: how many of the submitted keys the resource already holds (re-tagged, so no
    // slot is consumed), and how many are keys it does not hold (one slot each).
    let rows = [
        (0, 0),
        (1, 0),
        (MAX_TAGS_PER_RESOURCE / 2, 0),
        (MAX_TAGS_PER_RESOURCE, 0),
        (0, 1),
        (1, 1),
        (MAX_TAGS_PER_RESOURCE - 1, 1),
        (MAX_TAGS_PER_RESOURCE, 1),
        (MAX_TAGS_PER_RESOURCE, MAX_TAGS_PER_RESOURCE),
    ];

    for kind in [Kind::TrustAnchor, Kind::Profile, Kind::Crl] {
        let client = Client::new();
        for (index, (retagged, fresh)) in rows.into_iter().enumerate() {
            let case = format!("{}/retag-{retagged}/new-{fresh}", kind.label());
            let held = tags("held", MAX_TAGS_PER_RESOURCE);
            let arn = client
                .created_arn(kind, &format!("atquota-{}-{index}", kind.label()), &held)
                .await;

            // The submitted list: `retagged` keys the resource already carries, each
            // with a value it does not hold yet, then `fresh` keys it has never seen.
            let mut submitted: Vec<(String, String)> = held
                .iter()
                .take(retagged)
                .map(|(key, value)| (key.clone(), format!("re-{value}")))
                .collect();
            submitted.extend(tags("fresh", fresh));

            let answer = client.tag_resource(&arn, &submitted).await;
            let mut expected: BTreeMap<String, String> = held.iter().cloned().collect();
            if fresh == 0 {
                assert_eq!(
                    answer.status, 201,
                    "case {case:?}: re-tagging {retagged} held keys leaves the merged set \
                     at the quota of {MAX_TAGS_PER_RESOURCE}, got {answer:?}"
                );
                expected.extend(submitted.iter().cloned());
            } else {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: {fresh} key(s) the resource does not hold carry it past \
                     the quota of {MAX_TAGS_PER_RESOURCE} and must be refused, got {answer:?}"
                );
                assert_eq!(
                    answer.body.get("message").and_then(Value::as_str),
                    Some(past_cap_message().as_str()),
                    "case {case:?}: expected the quota wording, got {answer:?}"
                );
            }

            assert_eq!(
                expected.len(),
                MAX_TAGS_PER_RESOURCE,
                "case {case:?}: the resource stays at the quota either way"
            );
            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case, &listed),
                expected,
                "case {case:?}: a refused call stores nothing and an accepted re-tag only \
                 replaces values, got {listed:?}"
            );
        }
    }
}

/// The quota is judged against the set the resource holds when the write arrives, so
/// detaching keys moves the bound: the identical `TagResource` that was refused at the
/// quota is accepted once it fits the freed slots, and one key more than was freed is
/// still refused.
#[tokio::test]
async fn freeing_slots_lets_the_identical_tag_write_through() {
    // Row: how many of the held keys are detached before the same write is retried.
    let freed_counts = [
        1,
        2,
        MAX_TAGS_PER_RESOURCE / 2,
        MAX_TAGS_PER_RESOURCE,
    ];

    for kind in [Kind::TrustAnchor, Kind::Profile, Kind::Crl] {
        let client = Client::new();
        for (index, freed) in freed_counts.into_iter().enumerate() {
            let case = format!("{}/freed-{freed}", kind.label());
            let held = tags("held", MAX_TAGS_PER_RESOURCE);
            let arn = client
                .created_arn(kind, &format!("freeing-{}-{index}", kind.label()), &held)
                .await;
            let incoming = tags("incoming", freed);
            let at_quota: BTreeMap<String, String> = held.iter().cloned().collect();

            // At the quota the write is refused and stores nothing.
            let refused = client.tag_resource(&arn, &incoming).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: {freed} new key(s) on a resource at the quota of \
                 {MAX_TAGS_PER_RESOURCE} must be refused, got {refused:?}"
            );
            assert_eq!(
                refused.body.get("message").and_then(Value::as_str),
                Some(past_cap_message().as_str()),
                "case {case:?}: expected the quota wording, got {refused:?}"
            );
            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case, &listed),
                at_quota,
                "case {case:?}: the refused write must leave the stored set alone, got \
                 {listed:?}"
            );

            // Detaching keys frees exactly that many slots.
            let detached: Vec<String> =
                held.iter().take(freed).map(|(key, _)| key.clone()).collect();
            let untagged = client.untag_resource(&arn, &detached).await;
            assert_eq!(
                untagged.status, 200,
                "case {case:?}: expected UntagResource 200, got {untagged:?}"
            );
            let remaining: BTreeMap<String, String> = held.iter().skip(freed).cloned().collect();
            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case, &listed),
                remaining,
                "case {case:?}: the read must report the set the removal left, got {listed:?}"
            );

            // One key more than was freed still does not fit.
            let mut one_too_many = incoming.clone();
            one_too_many.push(("one-too-many".to_string(), "value".to_string()));
            let still_refused = client.tag_resource(&arn, &one_too_many).await;
            assert_eq!(
                (still_refused.status, still_refused.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: {} new keys exceed the {freed} freed slot(s) and must be \
                 refused, got {still_refused:?}",
                one_too_many.len()
            );
            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case, &listed),
                remaining,
                "case {case:?}: the second refusal must store nothing either, got {listed:?}"
            );

            // The identical write now fits exactly, carrying the resource back to the
            // quota.
            let accepted = client.tag_resource(&arn, &incoming).await;
            assert_eq!(
                accepted.status, 201,
                "case {case:?}: the write refused at the quota must be accepted once the \
                 {freed} freed slot(s) hold it, got {accepted:?}"
            );
            let mut expected = remaining.clone();
            expected.extend(incoming.iter().cloned());
            assert_eq!(
                expected.len(),
                MAX_TAGS_PER_RESOURCE,
                "case {case:?}: the accepted write lands the resource back at the quota"
            );
            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case, &listed),
                expected,
                "case {case:?}: the read must report the merged set, got {listed:?}"
            );
        }
    }
}
