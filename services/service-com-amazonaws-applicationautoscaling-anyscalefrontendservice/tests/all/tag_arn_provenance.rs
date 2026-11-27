// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal the ARN-addressed tagging family answers a `ResourceARN` that names no
//! registered scalable target with, and what decides between the two.
//!
//! RULE under test: `TagResource`, `UntagResource` and `ListTagsForResource` address a scalable
//! target by the ARN this service minted for it, so what a `ResourceARN` naming no REGISTERED
//! target earns is decided by whether that ARN names a scalable-target IDENTITY of the endpoint
//! serving the request — its partition, account and Region, and a last segment of the form a
//! scalable target's identifier takes — not by the lookup miss both cases share:
//!
//! * an ARN that names such an identity names a scalable target of this endpoint, and the answer
//!   is that there is none — the resource is reported ABSENT, by
//!   the modelled `ResourceNotFoundException`
//!   (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//!   declares it on all three), the fixed sentence "Scalable Target not found" and the ARN
//!   asked about in the shape's own `ResourceName` member. Whether this service ever minted
//!   that ARN does not enter into it;
//! * an ARN that names no such identity — another partition, account or Region, or a last
//!   segment the identifier space does not contain — names no resource this endpoint could hold,
//!   so the tagging
//!   call is refused without the service speaking about the resource at all: the modelled
//!   `ValidationException`'s code at its 400, and the fixed sentence "Access Denied", which
//!   carries nothing of the ARN, the account or the Region asked about.
//!
//! The model lists both errors on the mutations but says which covers which condition for
//! neither.
//!
//! The rows span what the rule generalises over: the OPERATION (the read and both mutations,
//! each with payloads of its own) crossed with the identity the ARN names — several ways an ARN
//! can name no registered target here (including one of exactly the form this service mints, so
//! the rule is not keyed on an ARN's PROVENANCE), and several targets whose ARNs this Region
//! minted and has since retired. A registered target stands beside every row and round-trips a
//! tag through the same three operations, so no row passes because the family refuses
//! everything.

use serde_json::{json, Value};

use crate::emulator::{error_code, error_message, resource_id_for, Emulator, ACCOUNT, REGION};

/// The sentence an ARN of this endpoint that names no registered target earns. Written out
/// here rather than read off the code under test: the WORDING is part of the rule.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The code that answer carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole sentence an ARN of another endpoint earns — fixed text, interpolating nothing
/// from the request or from state.
const ACCESS_DENIED: &str = "Access Denied";

/// The code that answer carries.
const VALIDATION: &str = "ValidationException";

/// The status both are served on: the client-error status a request that will not be served
/// carries.
const CLIENT_ERROR: u16 = 400;

/// One request per operation of the family, all three addressed to `resource_arn` with a
/// payload of its own: the answer is about the resource the ARN names, so neither the tags
/// written nor the keys removed change it. Every key and value here is a legal one, so no row
/// is refused for its payload.
fn family(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": {"env": "sim2real"}}),
        ),
        (
            "TagResource",
            json!({
                "ResourceARN": resource_arn,
                "Tags": {"env": "sim2real", "owner": "orders-team", "cost-center": "1234"},
            }),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env"]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env", "owner"]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": []}),
        ),
        ("ListTagsForResource", json!({"ResourceARN": resource_arn})),
    ]
}

/// ARNs that satisfy the modelled `@pattern` on `ResourceARN`
/// (`^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$`), so each one
/// reaches the handler rather than the boundary validator, and that name no registered target.
///
/// `live` is the ARN of a target that IS registered here, which the last rows vary one field of.
/// The flag is whether the ARN names a scalable-target IDENTITY of this endpoint — its
/// partition, account and Region, and an identifier of a scalable target — which is what decides
/// which refusal it earns: an ARN of the very form this service mints, differing from a minted
/// one only in not having been minted, earns the ABSENT answer just as a retired one does, which
/// is what tells this rule apart from one keyed on an ARN's provenance.
fn unresolvable(live: &str) -> Vec<(String, String, bool)> {
    let twin = {
        let mut twin = live.to_string();
        twin.truncate(live.len() - 4);
        twin.push_str("abcd");
        twin
    };
    vec![
        (
            "an id this service never minted".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            true,
        ),
        (
            "an id in another form".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 00000000-0000-0000-0000-000000000000"
            ),
            false,
        ),
        (
            "the shortest id the pattern admits".to_string(),
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/a"),
            false,
        ),
        (
            "an ARN of another account".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
        (
            "an ARN of another Region".to_string(),
            format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
        (
            "an ARN of another partition".to_string(),
            format!(
                "arn:aws-us-gov:application-autoscaling:us-gov-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
        (
            "a registered target's ARN with another account in it".to_string(),
            live.replace(ACCOUNT, "111122223333"),
            false,
        ),
        (
            "a registered target's ARN with another Region in it".to_string(),
            live.replace(REGION, "eu-west-1"),
            false,
        ),
        (
            "an ARN of the form this service mints, that it did not mint".to_string(),
            twin,
            true,
        ),
    ]
}

/// Assert `body` is the refusal of an ARN of another endpoint: the access-denied sentence,
/// stating nothing about the resource asked about.
fn assert_access_denied(case: &str, status: u16, body: &Value, arn: &str) {
    assert_eq!(
        status, CLIENT_ERROR,
        "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, VALIDATION,
        "case {case:?}: expected {VALIDATION}, got {code:?} in {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED:?}, got {message:?}"
    );
    // The service discloses nothing about a resource the caller does not reach: neither the ARN
    // asked about nor any part of it is reported back, in the sentence or in a member of its
    // own.
    assert!(
        body.get("ResourceName").is_none(),
        "case {case:?}: expected no resource to be named in the refusal, got {body:?}"
    );
    let id = arn.rsplit('/').next().unwrap_or(arn);
    assert!(
        !message.contains(arn) && !message.contains(id) && !message.contains("scalable-target"),
        "case {case:?}: expected the sentence to carry nothing of the ARN, got {message:?}"
    );
}

/// Assert `body` reports the resource `arn` names as absent.
fn assert_target_not_found(case: &str, status: u16, body: &Value, arn: &str) {
    assert_eq!(
        status, CLIENT_ERROR,
        "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, RESOURCE_NOT_FOUND,
        "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {code:?} in {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, TARGET_NOT_FOUND,
        "case {case:?}: expected {TARGET_NOT_FOUND:?}, got {message:?}"
    );
    // The ARN asked about is reported in the modelled member rather than spliced into the
    // sentence, which states the condition and nothing else.
    assert_eq!(
        body["ResourceName"],
        Value::String(arn.to_string()),
        "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
    );
    assert!(
        !message.contains(arn),
        "case {case:?}: expected the sentence to echo nothing of the ARN, got {message:?}"
    );
}

/// Read the tag set of `arn` back, through the implemented read.
async fn tags_of(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} with {body:?}"
    );
    body["Tags"].clone()
}

/// RULE: an ARN that names no registered scalable target is answered by the IDENTITY it names:
/// one of this account and Region carrying a scalable target's identifier is the resource
/// reported absent, whether this service minted it or not, and one that names no such identity
/// is refused as a tagging call the
/// service will not serve — the access-denied sentence, disclosing nothing about whether such a
/// resource exists. Every operation of the family answers alike, and neither refusal writes
/// anything.
#[tokio::test]
async fn an_unresolvable_arn_is_answered_by_the_endpoint_it_names() {
    let emulator = Emulator::empty();

    // One target that stays registered, so each row below is about the ARN it names.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"tag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );

    for (reason, arn, served_here) in unresolvable(&resident_arn) {
        assert_ne!(
            arn, resident_arn,
            "case {reason:?}: expected a row that is not the registered target's own ARN"
        );
        for (operation, request) in family(&arn) {
            let case = format!("{reason} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            if served_here {
                assert_target_not_found(&case, status, &body, &arn);
            } else {
                assert_access_denied(&case, status, &body, &arn);
            }
        }
    }

    // None of those refusals touched the store: the target that IS registered still carries the
    // tag written before them, and still gives it up again — so no row passed because the
    // family refuses everything.
    let stored = tags_of(&emulator, &resident_arn, "the resident target's tags").await;
    assert_eq!(
        stored,
        json!({"env": "sim2real"}),
        "case \"the resident target's tags\": expected the refusals to change nothing, got \
         {stored:?}"
    );
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": resident_arn, "TagKeys": ["env"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"untag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );
    let stored = tags_of(&emulator, &resident_arn, "the emptied tag set").await;
    assert_eq!(
        stored,
        json!({}),
        "case \"the emptied tag set\": expected an empty tag set, got {stored:?}"
    );
}

/// RULE: an ARN this Region MINTED, for a target it no longer holds, is the resource reported
/// absent by every operation of the family — and the refusal does not revive the target.
#[tokio::test]
async fn an_arn_minted_for_a_target_that_is_gone_reports_the_absent_resource() {
    let emulator = Emulator::empty();

    // Several targets, each registered and then deregistered, so the rows differ in the target
    // whose ARN was retired rather than in one identity's spelling. One stays registered
    // beside them.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;

    let mut retired: Vec<(String, String)> = Vec::new();
    for dimension in [
        "dynamodb:table:WriteCapacityUnits",
        "ecs:service:DesiredCount",
        "dynamodb:index:ReadCapacityUnits",
    ] {
        let target = emulator.given_target(dimension, 1, 10).await;
        let arn = emulator.target_arn(&target).await;
        // The ARN answers while the target is registered: what changes below is the target's
        // existence, not the ARN's readability.
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected ListTagsForResource to answer 200 while the target is \
             registered, got {status} with {body:?}"
        );
        let (status, body) = emulator
            .call("DeregisterScalableTarget", target.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected DeregisterScalableTarget to answer 200, got {status} \
             with {body:?}"
        );
        retired.push((format!("the retired ARN of a {dimension} target"), arn));
    }

    for (reason, arn) in &retired {
        for (operation, request) in family(arn) {
            let case = format!("{reason} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_target_not_found(&case, status, &body, arn);
        }
    }

    // A refused tag call creates nothing: the deregistered identities are still unregistered,
    // and the resident target still answers.
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": "ecs"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistered namespace\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"],
        json!([]),
        "case \"the deregistered namespace\": expected the refusals to revive no target, got \
         {body:?}"
    );
    let stored = tags_of(&emulator, &resident_arn, "the resident target's tags").await;
    assert_eq!(
        stored,
        json!({}),
        "case \"the resident target's tags\": expected the untagged target to still answer, got \
         {stored:?}"
    );
}

/// RULE: an endpoint answers for the scalable targets of its own Region, so an ARN of another
/// Region is one it does not serve, before and after that
/// target is deregistered.
#[tokio::test]
async fn an_arn_is_served_only_by_the_region_it_names() {
    // Region names are opaque to the rule, so the rows differ in the shape of the pair: two
    // Regions of one geography, and a pair whose home is not the Region an unscoped request
    // falls back to.
    for (case, home, other) in [
        ("two Regions of one geography", "us-east-1", "us-west-2"),
        ("a home Region that is not the fallback one", "sa-east-1", "us-east-1"),
    ] {
        let emulator = Emulator::empty();
        let (status, body) = emulator
            .call_dynamodb_in(
                home,
                "CreateTable",
                json!({
                    "TableName": "catalog",
                    "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                    "BillingMode": "PROVISIONED",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTable in {home} to answer 200, got {status} with \
             {body:?}"
        );
        let target = json!({
            "ServiceNamespace": "dynamodb",
            "ResourceId": "table/catalog",
            "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
        });
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(1);
        registration["MaxCapacity"] = json!(10);
        let (status, body) = emulator
            .call_in(home, "RegisterScalableTarget", registration)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget in {home} to answer 200, got \
             {status} with {body:?}"
        );
        let arn = body["ScalableTargetARN"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The Region that minted it serves the tag write; the other Region does not serve it.
        let (status, body) = emulator
            .call_in(
                home,
                "TagResource",
                json!({"ResourceARN": arn, "Tags": {"env": "sim2real"}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource in {home} to answer 200, got {status} with \
             {body:?}"
        );
        for (operation, request) in family(&arn) {
            let (status, body) = emulator.call_in(other, operation, request).await;
            assert_access_denied(
                &format!("{case}: a {home} ARN asked at {other} on {operation}"),
                status,
                &body,
                &arn,
            );
        }

        // Deregistering it in its own Region stops it resolving THERE: that Region reports the
        // resource absent, and the other Region still does not serve it.
        let (status, body) = emulator
            .call_in(home, "DeregisterScalableTarget", target.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeregisterScalableTarget in {home} to answer 200, got \
             {status} with {body:?}"
        );
        for (operation, request) in family(&arn) {
            let (status, body) = emulator.call_in(home, operation, request.clone()).await;
            assert_target_not_found(
                &format!("{case}: the retired ARN asked at {home} on {operation}"),
                status,
                &body,
                &arn,
            );
            let (status, body) = emulator.call_in(other, operation, request).await;
            assert_access_denied(
                &format!("{case}: the retired ARN asked at {other} on {operation}"),
                status,
                &body,
                &arn,
            );
        }
    }
}

/// The identifier a scalable-target ARN carries, as the modelled `ResourceARN` publishes one:
/// `arn:aws:application-autoscaling:us-east-1:123456789012:scalable-target/1234abcd56ab78cd901ef1234567890ab123`
/// (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#TagResourceRequest",member="ResourceARN")`)
/// — 36 hexadecimal characters, which is also what this service's own mint hands back.
const TARGET_ID_CHARS: usize = 36;

/// RULE: a `ResourceARN` names the scalable target the call acts on by its identifier, so an id
/// of this account and Region that IS an identifier of a scalable target
/// ([`TARGET_ID_CHARS`] hexadecimal characters) and that no registered target answers to is the
/// resource reported absent — whoever wrote it — while an id the identifier space does not
/// contain names no scalable target at all, and the tagging call is refused without the service
/// reporting on a resource.
///
/// The rows differ along the id's FORM, which is what the rule turns on: identifiers of the
/// published length drawn from the published alphabet in several spellings, against ids outside
/// it — one character short of the length and one past it, a canonical UUID of exactly 36
/// characters whose separators the alphabet does not admit, the shortest id the modelled
/// `@pattern` admits, and a long hyphenated name.
#[tokio::test]
async fn which_refusal_an_unregistered_id_earns_follows_the_identifier_form() {
    let emulator = Emulator::empty();

    // A registered target stands beside the rows, so a refusal below is about the ARN it names
    // and not about the family refusing everything.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"tag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );

    let hex = "0123456789abcdef0123456789abcdef0123";
    let rows: Vec<(String, String, bool)> = vec![
        (
            "an id of the form this service mints".to_string(),
            hex.to_string(),
            true,
        ),
        (
            "an identifier spelled with capitals".to_string(),
            hex.to_uppercase(),
            true,
        ),
        (
            "an identifier of digits only".to_string(),
            "0".repeat(TARGET_ID_CHARS),
            true,
        ),
        (
            "an identifier of letters only".to_string(),
            "abcdef".repeat(TARGET_ID_CHARS / 6),
            true,
        ),
        (
            "one character short of an identifier".to_string(),
            hex[..TARGET_ID_CHARS - 1].to_string(),
            false,
        ),
        (
            "one character past an identifier".to_string(),
            format!("{hex}a"),
            false,
        ),
        (
            "a UUID of exactly as many characters".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
            false,
        ),
        (
            "the shortest id the pattern admits".to_string(),
            "a".to_string(),
            false,
        ),
        (
            "a long hyphenated name".to_string(),
            "probe-scalable-target-that-was-never-registered-0000".to_string(),
            false,
        ),
    ];

    for (case, id, is_identifier) in rows {
        let arn =
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/{id}");
        assert_ne!(
            arn, resident_arn,
            "case {case:?}: expected a row that is not the registered target's own ARN"
        );
        for (operation, request) in family(&arn) {
            let case = format!("{case} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            if is_identifier {
                assert_target_not_found(&case, status, &body, &arn);
            } else {
                assert_access_denied(&case, status, &body, &arn);
            }
        }
    }

    // None of those refusals touched the store: the resident target still carries the tag
    // written before them.
    let stored = tags_of(&emulator, &resident_arn, "the resident target's tags").await;
    assert_eq!(
        stored,
        json!({"env": "sim2real"}),
        "case \"the resident target's tags\": expected the refusals to change nothing, got \
         {stored:?}"
    );
}

/// One target's whole tagging life, as a client holding its `ScalableTargetARN` sees it.
///
/// The rows this is built from differ along what the rule generalises over: the namespace and
/// dimension the target was registered on, the tag set its REGISTRATION carried, and the batch
/// each refused write names once the target is gone.
struct Lifecycle {
    /// The dimension the target is registered on, which also names the resource it scales.
    dimension: &'static str,
    /// The `Tags` the registration carried, which `RegisterScalableTarget` takes.
    registered: Value,
    /// The `Tags` the write aimed at the retired ARN carries.
    written: Value,
    /// The `TagKeys` the removal aimed at the retired ARN names.
    removed: Value,
}

/// The key the positive control writes: one no row's registration carries, so the tag set read
/// back after the control is the registration's plus exactly this one.
const CONTROL_KEY: &str = "control";

/// Its value.
const CONTROL_VALUE: &str = "served";

/// The keys of a row's registration tag set, in the order the map reports them.
fn keys_of(tags: &Value) -> Vec<String> {
    tags.as_object()
        .map(|tags| tags.keys().cloned().collect())
        .unwrap_or_default()
}

/// RULE: a `ResourceARN` identifies the scalable target the call acts on, so once that target is
/// DEREGISTERED the ARN names a scalable target of this account and Region that is absent — and
/// both tag WRITES answer that exactly as the tag READ does, leaving the identity unregistered
/// and no tag of the refused batch anywhere in the store.
///
/// The whole life of one target per row: registered with a tag set, served a write, a removal
/// and a read while it exists, then deregistered, then asked the same three operations again,
/// then registered afresh — whose tag set is empty, because a refused write landed nothing.
#[tokio::test]
async fn the_tag_writes_report_a_deregistered_target_absent_and_land_nothing() {
    let rows = [
        Lifecycle {
            dimension: "dynamodb:table:ReadCapacityUnits",
            registered: json!({"team": "blue"}),
            written: json!({"env": "prod"}),
            removed: json!(["env"]),
        },
        Lifecycle {
            dimension: "dynamodb:index:WriteCapacityUnits",
            registered: json!({}),
            written: json!({"env": "prod", "owner": "orders-team", "cost-center": "1234"}),
            removed: json!(["team", "env", "owner"]),
        },
        Lifecycle {
            dimension: "ecs:service:DesiredCount",
            registered: json!({"team": "blue", "env": "test"}),
            written: json!({"team": "green"}),
            removed: json!([]),
        },
    ];

    for row in rows {
        let case = row.dimension;
        let emulator = Emulator::empty();
        let namespace = row.dimension.split(':').next().unwrap_or_default();
        let resource_id = resource_id_for(row.dimension);
        // A scalable target is a capacity dimension OF a resource, so the resource it names is
        // created in the service that owns it first.
        emulator.given_resource(namespace, &resource_id, 1).await;
        let identity = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": row.dimension,
        });
        let mut registration = identity.clone();
        registration["MinCapacity"] = json!(1);
        registration["MaxCapacity"] = json!(10);
        if !keys_of(&row.registered).is_empty() {
            registration["Tags"] = row.registered.clone();
        }
        let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} with \
             {body:?}"
        );
        // The ARN a client keeps is the one the registration handed back.
        let arn = body["ScalableTargetARN"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: expected a ScalableTargetARN from the registration, got {body:?}"
        );

        // While the target exists, all three operations are served: a tag is written, the
        // registration's own keys are removed, and the read reports what is left — so a refusal
        // below is about the target being gone and not about the family refusing everything.
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceARN": arn, "Tags": {CONTROL_KEY: CONTROL_VALUE}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource on the live ARN to answer 200, got {status} with \
             {body:?}"
        );
        let mut expected = row.registered.as_object().cloned().unwrap_or_default();
        expected.insert(CONTROL_KEY.to_string(), json!(CONTROL_VALUE));
        let stored = tags_of(&emulator, &arn, &format!("{case}: the live tag set")).await;
        assert_eq!(
            stored,
            Value::Object(expected),
            "case {case:?}: expected the registration's tags with the written one beside them, \
             got {stored:?}"
        );
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": keys_of(&row.registered)}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource on the live ARN to answer 200, got {status} \
             with {body:?}"
        );
        let stored = tags_of(&emulator, &arn, &format!("{case}: the emptied tag set")).await;
        assert_eq!(
            stored,
            json!({CONTROL_KEY: CONTROL_VALUE}),
            "case {case:?}: expected only the key the control wrote, got {stored:?}"
        );

        // Deregistering retires the resource that ARN named.
        let (status, body) = emulator
            .call("DeregisterScalableTarget", identity.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeregisterScalableTarget to answer 200, got {status} with \
             {body:?}"
        );

        // Each operation of the family, with this row's own payload, reports the resource the
        // ARN names as absent.
        for (operation, request) in [
            (
                "TagResource",
                json!({"ResourceARN": arn, "Tags": row.written}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": row.removed}),
            ),
            ("ListTagsForResource", json!({"ResourceARN": arn})),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_target_not_found(
                &format!("{case}: the retired ARN on {operation}"),
                status,
                &body,
                &arn,
            );
        }

        // The refusals registered nothing: the identity the retired ARN belonged to is still
        // absent from the read that lists it.
        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceIds": [resource_id],
                    "ScalableDimension": row.dimension,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body["ScalableTargets"],
            json!([]),
            "case {case:?}: expected the refused writes to revive no target, got {body:?}"
        );

        // Nor did they store a tag against the identity: registering it again, with no tags of
        // its own, mints a new resource whose tag set is empty — and the retired ARN keeps
        // reporting the resource it names as absent.
        let mut reregistration = identity.clone();
        reregistration["MinCapacity"] = json!(1);
        reregistration["MaxCapacity"] = json!(10);
        let (status, body) = emulator.call("RegisterScalableTarget", reregistration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-registration to answer 200, got {status} with {body:?}"
        );
        let reissued = body["ScalableTargetARN"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            reissued, arn,
            "case {case:?}: expected the re-registration to mint a new ARN"
        );
        let stored = tags_of(
            &emulator,
            &reissued,
            &format!("{case}: the re-registered target's tags"),
        )
        .await;
        assert_eq!(
            stored,
            json!({}),
            "case {case:?}: expected the refused writes to have landed nothing, got {stored:?}"
        );
        for (operation, request) in family(&arn) {
            let (status, body) = emulator.call(operation, request).await;
            assert_target_not_found(
                &format!("{case}: the superseded ARN on {operation}"),
                status,
                &body,
                &arn,
            );
        }
    }
}

/// RULE: an ARN identifies one TARGET, so retiring one capacity dimension of a resource reports
/// that target's ARN absent while every other dimension of the SAME resource keeps being
/// served — the resolution is per target, not per resource.
#[tokio::test]
async fn retiring_one_dimension_of_a_resource_leaves_the_others_serving() {
    let emulator = Emulator::empty();
    // Two dimensions of one DynamoDB table, so both targets name the same resource id.
    let retired_target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_target = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;
    assert_eq!(
        retired_target["ResourceId"], resident_target["ResourceId"],
        "case \"the pair\": expected both dimensions of one resource, got {retired_target:?} and \
         {resident_target:?}"
    );
    let retired_arn = emulator.target_arn(&retired_target).await;
    let resident_arn = emulator.target_arn(&resident_target).await;
    assert_ne!(
        retired_arn, resident_arn,
        "case \"the pair\": expected each dimension its own ARN"
    );

    let (status, body) = emulator
        .call("DeregisterScalableTarget", retired_target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected 200, got {status} with {body:?}"
    );

    for (operation, request) in family(&retired_arn) {
        let (status, body) = emulator.call(operation, request).await;
        assert_target_not_found(
            &format!("the retired dimension's ARN on {operation}"),
            status,
            &body,
            &retired_arn,
        );
    }

    // The sibling dimension of the same resource is untouched: it takes a tag, gives it up and
    // reads back as it was written.
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the sibling dimension\": expected TagResource to answer 200, got {status} with \
         {body:?}"
    );
    let stored = tags_of(&emulator, &resident_arn, "the sibling dimension's tags").await;
    assert_eq!(
        stored,
        json!({"env": "sim2real"}),
        "case \"the sibling dimension\": expected the tag just written, got {stored:?}"
    );
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": resident_arn, "TagKeys": ["env"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the sibling dimension\": expected UntagResource to answer 200, got {status} with \
         {body:?}"
    );
    let stored = tags_of(&emulator, &resident_arn, "the emptied sibling tag set").await;
    assert_eq!(
        stored,
        json!({}),
        "case \"the sibling dimension\": expected an empty tag set, got {stored:?}"
    );
}

/// RULE: registering a deregistered identity again is a NEW resource with a NEW ARN, and the
/// superseded ARN keeps reporting the resource it names as absent — the store, not the ARN's
/// history, is what decides.
#[tokio::test]
async fn a_superseded_arn_still_reports_the_target_absent() {
    let emulator = Emulator::empty();
    // Several identities, so the rule is not read off one dimension's spelling.
    for dimension in [
        "dynamodb:table:WriteCapacityUnits",
        "ecs:service:DesiredCount",
        "dynamodb:index:ReadCapacityUnits",
    ] {
        let target = emulator.given_target(dimension, 1, 10).await;
        let first = emulator.target_arn(&target).await;
        let (status, body) = emulator
            .call("DeregisterScalableTarget", target.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected DeregisterScalableTarget to answer 200, got {status} \
             with {body:?}"
        );

        let mut registration = target.clone();
        registration["MinCapacity"] = json!(1);
        registration["MaxCapacity"] = json!(10);
        let (status, body) = emulator
            .call("RegisterScalableTarget", registration)
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected the re-registration to answer 200, got {status} with \
             {body:?}"
        );
        let reissued = emulator.target_arn(&target).await;
        assert_ne!(
            reissued, first,
            "case {dimension:?}: expected the re-registration to mint a new ARN"
        );

        // The new ARN answers, and every operation of the family reports the superseded one's
        // resource as absent.
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": reissued}))
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected the re-registered target to answer 200, got {status} \
             with {body:?}"
        );
        for (operation, request) in family(&first) {
            let (status, body) = emulator.call(operation, request).await;
            assert_target_not_found(
                &format!("the superseded ARN of {dimension} on {operation}"),
                status,
                &body,
                &first,
            );
        }
    }
}
