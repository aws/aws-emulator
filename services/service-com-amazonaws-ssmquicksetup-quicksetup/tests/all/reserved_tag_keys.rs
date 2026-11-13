// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the reserved tag-key prefix.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a tag key under the Amazon Web Services-reserved prefix is not
//! the caller's to assign, edit or delete, so EVERY member that takes tag keys from the
//! caller — `CreateConfigurationManager$Tags`, `TagResource$Tags` and
//! `UntagResource$TagKeys` — refuses a request naming one with a 400
//! `ValidationException` and leaves the stored tags exactly as they were. The prefix is
//! reserved however its letters are cased, while a key that merely RESEMBLES it is an
//! ordinary key the service stores, reports and removes like any other. The refusal
//! names the member and the prefix; whether it also names the offending key follows the
//! member's sensitivity ([`KeyEcho`]), which `all/sensitive_refusal_messages.rs` is the
//! suite for.
//!
//! The tables span what the rule generalises over: the casing of the prefix, what
//! follows it (nothing, a plain tail, a punctuated tail, a multi-byte tail), where the
//! prefix sits in the key, how many keys one request names and — when it names several —
//! the order they travel in, and which operation carries them. Both readers a stored tag
//! becomes visible through — `GetConfigurationManager` and `ListTagsForResource` — are
//! checked, since a refused write must leave neither reporting a reserved key.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    QUICK_SETUP_TYPE_CATALOG, RESERVED_TAG_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create ever
/// minted, for the case that pits request validation against resolving the target.
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/\
     00000000-0000-4000-8000-000000000000";

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a refusal carries: the member this service's own modelled
/// `ValidationException` / `ResourceNotFoundException` state their sentence in. A refusal
/// of the layer in FRONT of the service names its member in lowercase, so a suite that
/// asserts one of those reads it at its own call site rather than through this helper.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, which is what an SDK writes for an `@httpQuery` member.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource`. `TagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated `tagKeys=` parameter.
pub(crate) async fn untag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[&str],
) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    let uri = format!("/tags/{}?{}", label(resource_arn), query.join("&"));
    call(router, "DELETE", &uri, None).await
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
pub(crate) async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let body = json!({ "Tags": tag_object(tags) });
    call(
        router,
        "PUT",
        &format!("/tags/{}", label(resource_arn)),
        Some(&body),
    )
    .await
}

/// A tag map as a restJson1 payload member.
pub(crate) fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// `POST /configurationManager` carrying `tags` — `CreateConfigurationManager`.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    // `Type` must name a configuration type this service publishes; the catalog's first
    // entry is the one every case here deploys, since no case is about the type. Its
    // parameters name somewhere to deploy to and the Region to deploy into, and provide
    // every parameter the table documented for that type marks `(Required)`, which every
    // accepted definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
        "Tags": tag_object(tags),
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// Create one configuration manager carrying `tags`, answering its ARN.
pub(crate) async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (status, answer) = create(router, "reserved-tag-case", tags).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The tags `GetConfigurationManager` reports for `manager_arn`.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let tags = answer
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {answer:?}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a map, got {tags:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} must be a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// The tags `ListTagsForResource` reports for `manager_arn` — the OTHER reader a stored
/// tag becomes visible through, so a refused write is shown to have left neither reader
/// reporting a reserved key. The entries are parsed by the one helper that owns that
/// reader, `all/list_tags_for_resource.rs`'s `listed`.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeMap<String, String> {
    super::list_tags_for_resource::listed(router, case, manager_arn).await
}

/// How many configuration managers the store holds.
async fn manager_count(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Tags as the tables below write them.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The ordinary tags every case seeds its resource with, so a refusal can be shown to
/// have left the stored set alone.
pub(crate) fn seeded() -> BTreeMap<String, String> {
    tags(&[("Env", "prod"), ("Owner", "ops")])
}

/// Keys the rule REFUSES, spanning the casing of the reserved prefix and what follows
/// it. Each is derived from [`RESERVED_TAG_PREFIX`] rather than written out, so the
/// table follows the handler-side constant.
///
/// The casings come from the AWS General Reference's tag naming limits, which reserve
/// the prefix in any combination of upper and lower case; the tails span nothing at all,
/// a plain word, one of the punctuation characters `TagsMap`'s key pattern admits
/// (`smithy_query(query="shape",shape="TagsMap")`) and a multi-byte tail, which only
/// `UntagResource$TagKeys` (a bare `list<String>`) can carry.
pub(crate) fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_PREFIX;
    let upper = prefix.to_ascii_uppercase();
    // The prefix with only its first letter upper-cased, derived from the prefix itself.
    let mut mixed = String::new();
    for (index, character) in prefix.chars().enumerate() {
        if index == 0 {
            mixed.extend(character.to_uppercase());
        } else {
            mixed.push(character);
        }
    }
    vec![
        ("the reserved prefix alone".to_string(), prefix.to_string()),
        (
            "the reserved prefix with a plain tail".to_string(),
            format!("{prefix}createdBy"),
        ),
        (
            "the reserved prefix with a punctuated tail".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "the reserved prefix with a multi-byte tail".to_string(),
            format!("{prefix}владелец"),
        ),
        (
            "the reserved prefix in upper case".to_string(),
            format!("{upper}createdBy"),
        ),
        (
            "the reserved prefix in mixed case".to_string(),
            format!("{mixed}CreatedBy"),
        ),
        (
            "the reserved prefix in upper case, alone".to_string(),
            upper.clone(),
        ),
    ]
}

/// Keys that only RESEMBLE the reserved prefix, so they are the caller's own: the
/// prefix's letters without its separator, the prefix one character short, the prefix
/// somewhere other than the key's start, and the prefix's letters followed by more
/// letters.
fn lookalike_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_PREFIX;
    let letters: String = prefix
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    vec![
        (
            "the prefix's letters without its separator".to_string(),
            letters.clone(),
        ),
        (
            "the prefix's letters followed by more letters".to_string(),
            format!("{letters}ome"),
        ),
        (
            "the prefix one character short of itself".to_string(),
            format!("{}.region", &prefix[..prefix.len() - 1]),
        ),
        (
            "the reserved prefix away from the key's start".to_string(),
            format!("team.{prefix}owner"),
        ),
        (
            "the prefix's letters joined to a tail by another separator".to_string(),
            format!("{letters}_createdBy"),
        ),
    ]
}

/// Whether a refusal about a member may render the offending tag key.
///
/// `TagsMap` — what both write paths' `Tags` member targets — is `@sensitive`, and the
/// trait applies to the aggregate, so neither a key nor a value out of that map may be
/// rendered on the wire; `UntagResource$TagKeys` is a bare `list<String>` with no
/// sensitive trait anywhere in its path, so its refusal names the key it refused.
/// Source: `smithy_query(query="shape", shape="TagsMap,TagKeys")`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyEcho {
    /// The member's data is sensitive: no key out of it appears in the message.
    Withheld,
    /// The member's data is not sensitive: the refused key is named.
    Named,
}

/// The part of a reserved `key` that is NOT the reserved prefix.
///
/// Every refusal states the prefix itself, so the tail is what tells a refusal that
/// rendered the caller's key from one that only stated the requirement.
pub(crate) fn key_tail(key: &str) -> &str {
    key.get(RESERVED_TAG_PREFIX.len()..).unwrap_or("")
}

/// Every refusal states the member and the reserved prefix, and names the offending key
/// only where the member's data is not sensitive.
pub(crate) fn assert_reserved_refusal(
    case: &str,
    member: &str,
    echo: KeyEcho,
    key: &str,
    status: u16,
    answer: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    let message = error_message(answer);
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name the member {member}, got {message:?}"
    );
    assert!(
        message.contains(RESERVED_TAG_PREFIX),
        "case {case:?}: the refusal must name the reserved prefix {RESERVED_TAG_PREFIX:?}, \
         got {message:?}"
    );
    match echo {
        KeyEcho::Named => assert!(
            message.contains(key),
            "case {case:?}: the refusal must name the key {key:?}, got {message:?}"
        ),
        KeyEcho::Withheld => {
            // Nothing out of the map: neither the key's tail nor the clause a refusal
            // about a non-sensitive member ends in.
            assert!(
                !message.contains(", got '"),
                "case {case:?}: a refusal about the sensitive {member} map must render no \
                 value at all, got {message:?}"
            );
            let tail = key_tail(key);
            assert!(
                tail.is_empty() || !message.contains(tail),
                "case {case:?}: the refusal must render no key out of the sensitive \
                 {member} map, but it carried {tail:?}: {message:?}"
            );
        }
    }
}

/// `UntagResource` refuses a reserved key and removes nothing — alone, beside a key the
/// resource carries, and beside a key it does not.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        let arn = create_ok(&router, &case, &stored).await;
        // The request shapes differ in how many keys they name and in what the other
        // keys are: the reserved key alone, with a stored key, and with an absent one
        // (whose removal would otherwise be a no-op success).
        for (shape, keys) in [
            ("alone", vec![key.as_str()]),
            ("beside a stored key", vec!["Env", key.as_str()]),
            ("beside an absent key", vec![key.as_str(), "Absent"]),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = untag(&router, &arn, &keys).await;
            assert_reserved_refusal(&case, "TagKeys", KeyEcho::Named, &key, status, &answer);

            let left = stored_tags(&router, &case, &arn).await;
            assert_eq!(
                left, stored,
                "case {case:?}: a refused removal must remove nothing, expected {stored:?}, \
                 got {left:?}"
            );
        }
    }
}

/// Where the reserved-key refusal sits in the order: the request's own input is judged
/// BEFORE the resource the ARN names is resolved, so a reserved key earns the modelled
/// 400 whether or not the ARN names a stored resource, while the same request with an
/// ordinary key on an ARN naming nothing earns the modelled 404.
#[tokio::test]
async fn the_reserved_key_refusal_comes_before_the_target_is_resolved() {
    let router = emulator();
    let stored = seeded();
    let served = create_ok(&router, "a served target", &stored).await;

    // The refusal the same ARN earns with an ordinary key: nothing about the request's
    // input is wrong, so what is left is that the ARN names no resource.
    let (status, answer) = untag(&router, UNKNOWN_MANAGER_ARN, &["Env"]).await;
    assert_eq!(
        status, 404,
        "case \"an ordinary key on an unknown ARN\": expected 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case \"an ordinary key on an unknown ARN\": expected ResourceNotFoundException, \
         got {answer:?}"
    );

    for (case, key) in reserved_keys() {
        let unserved_case = format!("{case} on an unknown ARN");
        let (status, answer) = untag(&router, UNKNOWN_MANAGER_ARN, &[key.as_str()]).await;
        assert_reserved_refusal(
            &unserved_case,
            "TagKeys",
            KeyEcho::Named,
            &key,
            status,
            &answer,
        );

        // The same request against a stored resource reaches the same rule, so the
        // refusal is about the key and not about the ARN.
        let served_case = format!("{case} on a served ARN");
        let (status, answer) = untag(&router, &served, &[key.as_str()]).await;
        assert_reserved_refusal(
            &served_case,
            "TagKeys",
            KeyEcho::Named,
            &key,
            status,
            &answer,
        );
    }

    let left = stored_tags(&router, "a served target", &served).await;
    assert_eq!(
        left, stored,
        "a refused removal must remove nothing, expected {stored:?}, got {left:?}"
    );
}

/// A key that only resembles the reserved prefix is the caller's own: it is stored,
/// reported and removed like any other key.
#[tokio::test]
async fn untag_resource_removes_a_key_that_only_resembles_the_reserved_prefix() {
    let router = emulator();

    for (case, key) in lookalike_keys() {
        let mut created = seeded();
        created.insert(key.clone(), "mine".to_string());
        let arn = create_ok(&router, &case, &created).await;

        let (status, answer) = untag(&router, &arn, &[key.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing an ordinary key must succeed, got {status} / {answer:?}"
        );
        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left,
            seeded(),
            "case {case:?}: exactly the named key must be gone, expected {:?}, got {left:?}",
            seeded()
        );
    }
}

/// `TagResource` refuses a reserved key and stores nothing from that request.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        // A map key travels in the restJson1 payload, so `TagsMap`'s key `@pattern`
        // applies: a multi-byte key is refused by the constraint walk before the rule
        // under test is reached, and belongs to the `UntagResource` table only.
        if !key.is_ascii() {
            continue;
        }
        let arn = create_ok(&router, &case, &stored).await;
        for (shape, assign) in [
            ("alone", tags(&[(key.as_str(), "mine")])),
            (
                "beside an ordinary pair",
                tags(&[(key.as_str(), "mine"), ("Tier", "gold")]),
            ),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = tag(&router, &arn, &assign).await;
            assert_reserved_refusal(&case, "Tags", KeyEcho::Withheld, &key, status, &answer);

            let left = stored_tags(&router, &case, &arn).await;
            assert_eq!(
                left, stored,
                "case {case:?}: a refused assignment must store nothing, expected {stored:?}, \
                 got {left:?}"
            );
        }
    }
}

/// The OTHER reader agrees: after a refused assignment `ListTagsForResource` reports the
/// stored set unchanged, so no reserved key becomes visible through the tagging reader
/// even though the write path is what would have stored it.
#[tokio::test]
async fn list_tags_for_resource_reports_no_reserved_key_after_a_refused_assignment() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        // As in `tag_resource_refuses_a_reserved_tag_key`: a body map key is bound by
        // `TagsMap`'s key `@pattern`, so a multi-byte key never reaches the rule.
        if !key.is_ascii() {
            continue;
        }
        let arn = create_ok(&router, &case, &stored).await;

        let (status, answer) = tag(&router, &arn, &tags(&[(key.as_str(), "mine")])).await;
        assert_reserved_refusal(&case, "Tags", KeyEcho::Withheld, &key, status, &answer);

        let listed = listed_tags(&router, &case, &arn).await;
        assert_eq!(
            listed, stored,
            "case {case:?}: ListTagsForResource must report the stored set, expected \
             {stored:?}, got {listed:?}"
        );
    }
}

/// A key that only resembles the reserved prefix is assigned and read back.
#[tokio::test]
async fn tag_resource_assigns_a_key_that_only_resembles_the_reserved_prefix() {
    let router = emulator();

    for (case, key) in lookalike_keys() {
        let arn = create_ok(&router, &case, &seeded()).await;
        let assign = tags(&[(key.as_str(), "mine")]);
        let (status, answer) = tag(&router, &arn, &assign).await;
        assert_eq!(
            status, 200,
            "case {case:?}: assigning an ordinary key must succeed, got {status} / {answer:?}"
        );

        let mut expected = seeded();
        expected.insert(key.clone(), "mine".to_string());
        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left, expected,
            "case {case:?}: the assigned pair must be stored, expected {expected:?}, \
             got {left:?}"
        );
    }
}

/// `CreateConfigurationManager` refuses a reserved key in `Tags` and creates nothing, so
/// a reserved tag can never enter the store in the first place.
#[tokio::test]
async fn create_configuration_manager_refuses_a_reserved_tag_key() {
    let router = emulator();

    for (case, key) in reserved_keys() {
        // As in `tag_resource_refuses_a_reserved_tag_key`: a body map key is bound by
        // `TagsMap`'s key `@pattern`, so a multi-byte key never reaches the rule.
        if !key.is_ascii() {
            continue;
        }
        let before = manager_count(&router, &case).await;
        for (shape, supplied) in [
            ("alone", tags(&[(key.as_str(), "mine")])),
            (
                "beside an ordinary pair",
                tags(&[(key.as_str(), "mine"), ("Env", "prod")]),
            ),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = create(&router, "reserved-tag-create", &supplied).await;
            assert_reserved_refusal(&case, "Tags", KeyEcho::Withheld, &key, status, &answer);

            let after = manager_count(&router, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store no manager, expected {before}, \
                 got {after}"
            );
        }
    }
}

/// Every reserved key that is MORE than the reserved prefix itself, so a refusal naming
/// one is distinguishable from the prefix every such message also states. Drawn from
/// [`reserved_keys`], so the rows stay the ones that table spans.
pub(crate) fn tailed_reserved_keys(ascii_only: bool) -> Vec<String> {
    reserved_keys()
        .into_iter()
        .map(|(_, key)| key)
        .filter(|key| !key.eq_ignore_ascii_case(RESERVED_TAG_PREFIX))
        .filter(|key| !ascii_only || key.is_ascii())
        .collect()
}

/// The one reserved key a refusal that MAY name a key must name when a request carries
/// several: the lexically first, so one request has ONE answer however the collection it
/// travelled in is ordered or iterated.
pub(crate) fn first_reserved_key(keys: &[String]) -> String {
    keys.iter()
        .min()
        .expect("the reserved-key table is not empty")
        .clone()
}

/// A refusal names exactly one of `candidates`, the expected one. The key is matched in
/// the quoted form the service's refusals echo a value in, so a key that merely starts
/// another key cannot pass for it.
fn assert_names_only(case: &str, message: &str, expected: &str, candidates: &[String]) {
    assert!(
        message.contains(&format!("'{expected}'")),
        "case {case:?}: the refusal must name the lexically first reserved key \
         {expected:?}, got {message:?}"
    );
    for other in candidates.iter().filter(|key| key.as_str() != expected) {
        assert!(
            !message.contains(&format!("'{other}'")),
            "case {case:?}: the refusal must name only {expected:?}, but it also named \
             {other:?}: {message:?}"
        );
    }
}

/// A refusal about a SENSITIVE member renders NONE of `candidates`: neither the quoted
/// form the service echoes a non-sensitive value in, nor the part of a key beyond the
/// reserved prefix every such message states.
pub(crate) fn assert_names_no_key(case: &str, message: &str, candidates: &[String]) {
    for key in candidates {
        assert!(
            !message.contains(&format!("'{key}'")),
            "case {case:?}: the refusal must render no key out of the sensitive map, but \
             it named {key:?}: {message:?}"
        );
        let tail = key_tail(key);
        assert!(
            tail.is_empty() || !message.contains(tail),
            "case {case:?}: the refusal must render no key out of the sensitive map, but \
             it carried {tail:?}: {message:?}"
        );
    }
}

/// How many times a request whose answer must not vary is repeated. The handlers read
/// `Tags` from a `HashMap`, whose iteration order differs from one map to the next, so
/// repeating the identical request is what varies the order the keys are seen in.
const REPEATS: usize = 8;

/// A create naming SEVERAL reserved keys answers the same way every time: the same 400,
/// rendering no key out of the sensitive map, and still creating nothing.
#[tokio::test]
async fn create_configuration_manager_answers_one_refusal_however_tags_are_iterated() {
    let router = emulator();
    let keys = tailed_reserved_keys(true);
    let expected = first_reserved_key(&keys);
    let supplied: BTreeMap<String, String> = keys
        .iter()
        .map(|key| (key.clone(), "mine".to_string()))
        .collect();
    let before = manager_count(&router, "several reserved keys in one create").await;
    let mut answered: Option<String> = None;

    for attempt in 0..REPEATS {
        let case = format!("several reserved keys in one create, attempt {attempt}");
        let (status, answer) = create(&router, "reserved-tag-order", &supplied).await;
        assert_reserved_refusal(&case, "Tags", KeyEcho::Withheld, &expected, status, &answer);
        let message = error_message(&answer);
        assert_names_no_key(&case, &message, &keys);
        // The message carries nothing out of the request, so it cannot vary with the
        // order the supplied map happens to be iterated in.
        match &answered {
            None => answered = Some(message),
            Some(first) => assert_eq!(
                &message, first,
                "case {case:?}: the refusal must not vary with the iteration order, \
                 expected {first:?}, got {message:?}"
            ),
        }

        let after = manager_count(&router, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must store no manager, expected {before}, \
             got {after}"
        );
    }
}

/// The same for the assignment path: one answer however the supplied map is iterated, and
/// the stored set untouched.
#[tokio::test]
async fn tag_resource_answers_one_refusal_however_tags_are_iterated() {
    let router = emulator();
    let stored = seeded();
    let keys = tailed_reserved_keys(true);
    let expected = first_reserved_key(&keys);
    let assign: BTreeMap<String, String> = keys
        .iter()
        .map(|key| (key.clone(), "mine".to_string()))
        .collect();
    let arn = create_ok(&router, "several reserved keys in one assignment", &stored).await;
    let mut answered: Option<String> = None;

    for attempt in 0..REPEATS {
        let case = format!("several reserved keys in one assignment, attempt {attempt}");
        let (status, answer) = tag(&router, &arn, &assign).await;
        assert_reserved_refusal(&case, "Tags", KeyEcho::Withheld, &expected, status, &answer);
        let message = error_message(&answer);
        assert_names_no_key(&case, &message, &keys);
        match &answered {
            None => answered = Some(message),
            Some(first) => assert_eq!(
                &message, first,
                "case {case:?}: the refusal must not vary with the iteration order, \
                 expected {first:?}, got {message:?}"
            ),
        }

        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left, stored,
            "case {case:?}: a refused assignment must store nothing, expected {stored:?}, \
             got {left:?}"
        );
    }
}

/// `UntagResource$TagKeys` is an ORDERED list, so the order the caller writes it in is a
/// real input variation: every rotation of one key list, and that list reversed, earns the
/// same refusal naming the same key, and removes nothing.
#[tokio::test]
async fn untag_resource_names_one_reserved_key_however_tag_keys_are_ordered() {
    let router = emulator();
    let stored = seeded();
    // A query-string list carries a multi-byte key too, which a body map's key `@pattern`
    // would refuse before the rule under test, so this table keeps that row.
    let keys = tailed_reserved_keys(false);
    let expected = first_reserved_key(&keys);
    let arn = create_ok(&router, "several reserved keys in one removal", &stored).await;

    let mut orderings: Vec<Vec<String>> = (0..keys.len())
        .map(|shift| {
            let mut rotated = keys.clone();
            rotated.rotate_left(shift);
            rotated
        })
        .collect();
    let mut reversed = keys.clone();
    reversed.reverse();
    orderings.push(reversed);

    for ordering in orderings {
        let case = format!("several reserved keys in one removal, ordered {ordering:?}");
        let named: Vec<&str> = ordering.iter().map(String::as_str).collect();
        let (status, answer) = untag(&router, &arn, &named).await;
        assert_reserved_refusal(&case, "TagKeys", KeyEcho::Named, &expected, status, &answer);
        assert_names_only(&case, &error_message(&answer), &expected, &keys);

        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left, stored,
            "case {case:?}: a refused removal must remove nothing, expected {stored:?}, \
             got {left:?}"
        );
    }
}

/// A create carrying only lookalike keys succeeds and reports them back.
#[tokio::test]
async fn create_configuration_manager_stores_keys_that_only_resemble_the_reserved_prefix() {
    let router = emulator();

    let supplied: BTreeMap<String, String> = lookalike_keys()
        .into_iter()
        .map(|(_, key)| (key, "mine".to_string()))
        .collect();
    let case = "every lookalike key in one create";
    let arn = create_ok(&router, case, &supplied).await;
    let stored = stored_tags(&router, case, &arn).await;
    assert_eq!(
        stored, supplied,
        "case {case:?}: every ordinary key must be stored, expected {supplied:?}, \
         got {stored:?}"
    );
}
