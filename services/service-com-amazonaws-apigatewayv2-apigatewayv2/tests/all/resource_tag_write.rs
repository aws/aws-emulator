// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `TagResource`, read back through `GetTags` and
//! through each resource's own reader.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `TagResource` (`POST /v2/tags/{ResourceArn}`) is add-or-update on the tag collection
//! > the resource its ARN names ALREADY carries: every entry the request names is written —
//! > ADDED when the resource carries no such key, and REPLACING the value when it does —
//! > while every key the request does not name keeps the value it had, so the collection the
//! > following `GetTags`, addressed by the same ARN, and the resource's own reader report is
//! > the stored collection with the written entries merged over it, never a collection that
//! > replaced it. The answer carries no content and states the length of it
//! > (`content-length: 0`), as every empty answer of this service does. A well-formed ARN of
//! > a taggable kind the account holds no record for is 404 `NotFoundException` with the one
//! > sentence a tag operation answers an unresolved resource ARN with; a value that is no ARN
//! > of a taggable resource of this service is 400 `BadRequestException`; and neither refusal
//! > writes a tag.
//!
//! The rows vary what the rule generalises over: WHICH kind of resource the ARN names (all
//! six whose modelled shape carries a `Tags` member, which is also both ARN forms this
//! service mints — the account-less management form and the account-qualified one), how many
//! entries one write carries (none, one, several), whether a written key is one the resource
//! already carries, which collection the resource started from (no tag, one, several), and
//! the form of a key and of a value (ASCII, the separators a tag key may contain, a
//! non-ASCII key and value, a key at the published 128-character maximum, a value at the
//! modelled upper bound). The recorded units that filed the issue — a single-entry write and
//! a two-entry write onto an api that already carried tags, read back afterwards — are two
//! rows of the first table.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, call_reading_headers, emulator, emulator_with_engine, error_code, REGION,
};
use crate::empty_answer_framing::{content_length, NO_CONTENT_LENGTH};
use crate::path_label::addressed;
use crate::resource_tag_removal::{taggable_resources, Target, TAGS};

/// The emulator's canonical account id, the account segment of the ARNs this service mints
/// for its portal-plane resources.
const ACCOUNT_ID: &str = "000000000000";

/// The sentence this API answers a caller-supplied ARN it cannot read with. Spelled out
/// here rather than taken from the crate's constant: a test that asserts a handler's own
/// constant back at it cannot catch the wording drifting away from the service's.
const INVALID_ARN: &str = "Invalid ARN specified in the request";

/// The sentence this API answers a tag operation whose `ResourceArn` resolves to no
/// resource the account holds with. Spelled out for the same reason [`INVALID_ARN`] is.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The URI one tag operation of `arn` is addressed at: the ARN is the operation's one path
/// label, percent-encoded the way an SDK client sends it.
fn tags_uri(arn: &str) -> String {
    format!("{TAGS}/{}", addressed(arn))
}

/// Write `tags` onto `arn` and assert the answer is the empty, framed one a tag write
/// carries: the status API Gateway answers it with, no body at all, and the length of that
/// nothing stated.
async fn write_tags(emulator: &MultiServiceRouter, case: &str, arn: &str, tags: &Value) {
    let uri = tags_uri(arn);
    let (status, headers, body) =
        call_reading_headers(emulator, "POST", &uri, Some(json!({ "tags": tags }))).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from TagResource at {uri}, got {status} {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case {case:?}: expected TagResource to answer no body at all, got {body}"
    );
    assert_eq!(
        content_length(&headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: expected the tag write to state content-length {NO_CONTENT_LENGTH}, \
         got {:?}",
        content_length(&headers)
    );
}

/// The tag collection `target` reports, read BOTH ways and asserted to agree: through
/// `GetTags`, addressed by the very ARN the write was, and through the resource's own
/// reader — one collection, so a write cannot land in one reader and not the other. A
/// reader that reports no `tags` member at all reports no tags.
async fn reported_tags(emulator: &MultiServiceRouter, case: &str, target: &Target) -> Value {
    let uri = tags_uri(&target.arn);
    let (status, body) = call(emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetTags at {uri}, got {status} {body}"
    );
    let Some(through_get_tags) = body.get("tags").cloned() else {
        panic!("case {case:?}: expected GetTags to report a tags member, got {body}");
    };

    let (status, body) = call(emulator, "GET", &target.read_uri, None).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected 200 reading {} back, got {status} {body}",
        target.read_uri
    );
    let through_own_reader = body.get("tags").cloned().unwrap_or_else(|| json!({}));
    assert_eq!(
        through_get_tags, through_own_reader,
        "case {case:?}: expected GetTags and the resource's own reader to report one collection"
    );
    through_get_tags
}

/// A tag collection of one pair, for a row whose key or value is built rather than written
/// out — `json!` takes a literal key.
fn one_tag(key: &str, value: &str) -> Value {
    let mut tags = serde_json::Map::new();
    tags.insert(key.to_string(), Value::String(value.to_string()));
    Value::Object(tags)
}

/// One write row: the collection the row's resources are created with, the entries one
/// `TagResource` writes onto them, and the collection they carry afterwards.
struct Row {
    case: &'static str,
    /// The label the row's resources are named from — `label` of [`taggable_resources`].
    label: &'static str,
    created: Value,
    written: Value,
    expected: Value,
}

/// The rows of the add-or-update table: a key the resource does not carry, a key it does,
/// several at once, and both halves in one write.
fn add_or_update_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one key the resource does not carry, added beside the two it has",
            label: "added-key",
            created: json!({ "team": "payments", "tier": "gold" }),
            written: json!({ "env": "staging" }),
            expected: json!({ "team": "payments", "tier": "gold", "env": "staging" }),
        },
        Row {
            case: "two keys in one write, neither of them carried before",
            label: "two-keys",
            created: json!({ "team": "payments" }),
            written: json!({ "env": "staging", "owner": "platform" }),
            expected: json!({ "team": "payments", "env": "staging", "owner": "platform" }),
        },
        Row {
            case: "a key the resource already carries, re-set to another value",
            label: "reset-key",
            created: json!({ "team": "payments", "tier": "gold" }),
            written: json!({ "tier": "silver" }),
            expected: json!({ "team": "payments", "tier": "silver" }),
        },
        Row {
            case: "an added key and a re-set key in one write",
            label: "added-and-reset",
            created: json!({ "team": "payments", "tier": "gold" }),
            written: json!({ "tier": "silver", "env": "staging" }),
            expected: json!({ "team": "payments", "tier": "silver", "env": "staging" }),
        },
    ]
}

/// The rows that vary the collection written onto and the FORM of a key and a value,
/// including the published and modelled bounds — which cannot be `const` data.
fn form_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a write onto a resource created carrying no tag at all",
            label: "from-empty",
            created: json!({}),
            written: json!({ "team": "payments" }),
            expected: json!({ "team": "payments" }),
        },
        Row {
            case: "a write naming an empty collection, which changes nothing",
            label: "empty-write",
            created: json!({ "team": "payments" }),
            written: json!({}),
            expected: json!({ "team": "payments" }),
        },
        Row {
            // "A tag key can have a maximum of 128 characters, and can contain only
            // Unicode letters, digits, white space, or these symbols: _ . : / = + - @"
            // — so the separators and any Unicode letter are part of a key, and a key or a
            // value carrying them is stored and reported back exactly as it was written.
            case: "a key carrying the separators a tag key may contain, and a non-ASCII pair",
            label: "key-forms",
            created: json!({ "team": "payments" }),
            written: json!({ "cost:center/eu": "42", "équipe": "paiements" }),
            expected: json!({
                "team": "payments",
                "cost:center/eu": "42",
                "équipe": "paiements"
            }),
        },
        Row {
            // The 128-character maximum AWS publishes for a tag key.
            case: "a key at the 128-character maximum a tag key may carry",
            label: "key-at-bound",
            created: json!({ "team": "payments" }),
            written: one_tag(&"k".repeat(128), "at-the-key-bound"),
            expected: merged(
                &json!({ "team": "payments" }),
                &one_tag(&"k".repeat(128), "at-the-key-bound"),
            ),
        },
        Row {
            // `Tags` is a map of `__string` to `StringWithLengthBetween1And1600`
            // (`smithy_query(query="shape", shape="Tags")`), so 1600 characters is the
            // modelled upper bound of a tag VALUE and 1 character its lower one.
            case: "a value at the modelled 1600-character upper bound",
            label: "value-at-bound",
            created: json!({ "team": "payments" }),
            written: one_tag("notes", &"v".repeat(1600)),
            expected: merged(
                &json!({ "team": "payments" }),
                &one_tag("notes", &"v".repeat(1600)),
            ),
        },
        Row {
            case: "a value at the modelled 1-character lower bound",
            label: "value-at-lower-bound",
            created: json!({ "team": "payments" }),
            written: json!({ "tier": "g" }),
            expected: json!({ "team": "payments", "tier": "g" }),
        },
    ]
}

/// `base` with `written` merged over it, for a row whose expected collection is built from a
/// key or a value at a bound rather than written out.
fn merged(base: &Value, written: &Value) -> Value {
    let mut out = base.as_object().expect("a row's tags are a map").clone();
    for (key, value) in written.as_object().expect("a row's tags are a map") {
        out.insert(key.clone(), value.clone());
    }
    Value::Object(out)
}

/// Drive one row over every taggable kind: the created collection is reported first, the
/// row's entries are written, and the merged collection is what both readers then report —
/// twice, so a second identical write neither changes the collection nor the answer.
async fn assert_row(emulator: &MultiServiceRouter, row: &Row) {
    for target in taggable_resources(emulator, row.label, &row.created).await {
        let case = format!("{} on {}", row.case, target.kind);

        assert_eq!(
            reported_tags(emulator, &case, &target).await,
            row.created,
            "case {case:?}: expected the collection the resource was created with before the \
             write"
        );

        for round in 0..2 {
            write_tags(emulator, &case, &target.arn, &row.written).await;
            assert_eq!(
                reported_tags(emulator, &case, &target).await,
                row.expected,
                "case {case:?} (round {round}): expected the written entries merged over the \
                 collection the resource carried"
            );
        }
    }
}

#[tokio::test]
async fn tag_resource_adds_or_updates_the_keys_it_names_and_leaves_the_others() {
    let emulator = emulator();
    for row in add_or_update_rows() {
        assert_row(&emulator, &row).await;
    }
}

#[tokio::test]
async fn a_tag_write_keeps_the_form_of_every_key_and_value_it_carries() {
    let emulator = emulator();
    for row in form_rows() {
        assert_row(&emulator, &row).await;
    }
}

#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resource_as_it_was() {
    let emulator = emulator();
    let created = json!({ "team": "payments" });

    // Two resources of every class, created from the same collection.
    let written_to = taggable_resources(&emulator, "written-first", &created).await;
    let siblings = taggable_resources(&emulator, "written-second", &created).await;

    for target in &written_to {
        let case = format!("tagging {}", target.kind);
        write_tags(&emulator, &case, &target.arn, &json!({ "env": "staging" })).await;
    }

    for target in &written_to {
        let case = format!("the tagged {}", target.kind);
        assert_eq!(
            reported_tags(&emulator, &case, target).await,
            json!({ "team": "payments", "env": "staging" }),
            "case {case:?}: expected the written key beside the created one"
        );
    }

    // Every sibling is as it was created: one write reached one record only.
    for target in &siblings {
        let case = format!("the sibling {} after the writes", target.kind);
        assert_eq!(
            reported_tags(&emulator, &case, target).await,
            created,
            "case {case:?}: expected the sibling's collection to be untouched"
        );
    }
}

#[tokio::test]
async fn an_arn_no_tag_write_can_resolve_is_refused_and_writes_nothing() {
    let emulator = emulator();
    let created = json!({ "team": "payments" });
    let held = taggable_resources(&emulator, "refused-write", &created).await;
    let api_id = api_id_of(&held);

    // A well-formed ARN of a taggable kind the account holds no record for — including the
    // ARN of a resource a delete has taken away — names a resource that is not there.
    let (status, deleted_api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "deleted-before-write", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the api that is then deleted\": expected 201 from CreateApi, got {status} \
         {deleted_api}"
    );
    let deleted_api_id = as_str(&deleted_api, "apiId");
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{deleted_api_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case \"the api that is then deleted\": expected 204 from DeleteApi, got {status} {body}"
    );

    for (case, arn) in [
        (
            "an api id nothing minted",
            format!("arn:aws:apigateway:{REGION}::/apis/nosuchapi00"),
        ),
        (
            "the ARN of an api that was deleted",
            format!("arn:aws:apigateway:{REGION}::/apis/{deleted_api_id}"),
        ),
        (
            "a stage name the api holds no stage under",
            format!("arn:aws:apigateway:{REGION}::/apis/{api_id}/stages/nosuchstage"),
        ),
        (
            "a VPC link id nothing minted",
            format!("arn:aws:apigateway:{REGION}::/vpclinks/nosuchlink"),
        ),
        (
            "a domain name the account does not hold",
            format!("arn:aws:apigateway:{REGION}::/domainnames/absent.example.com"),
        ),
        (
            "a portal id nothing minted",
            format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portals/nosuchportal"),
        ),
        (
            "a portal product id nothing minted",
            format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portalproducts/nosuchprod0"),
        ),
    ] {
        let uri = tags_uri(&arn);
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({ "tags": { "env": "staging" } })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from TagResource at {uri}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from TagResource, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            RESOURCE_NOT_FOUND,
            "case {case:?}: expected TagResource to answer the sentence this API answers a tag \
             operation's unresolved resource ARN with, got {body}"
        );
    }

    // A value that is no ARN of a taggable resource of this service is a bad REQUEST, not a
    // missing resource — the same reading the tag reader and the tag removal take of it.
    for (case, value) in [
        ("a value that is no ARN at all", "not-an-arn".to_string()),
        (
            "an ARN whose resource field is empty",
            format!("arn:aws:apigateway:{REGION}::"),
        ),
        (
            "an ARN of another vendor's resource",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
        (
            "an ARN of the other API Gateway control plane's REST api",
            format!("arn:aws:apigateway:{REGION}::/restapis/abcdef1234"),
        ),
        (
            "an api child this service does not tag",
            format!("arn:aws:apigateway:{REGION}::/apis/{api_id}/routes/abcdefg"),
        ),
        (
            "the apis collection rather than one api",
            format!("arn:aws:apigateway:{REGION}::/apis"),
        ),
    ] {
        let uri = tags_uri(&value);
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({ "tags": { "env": "staging" } })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from TagResource for {value:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException from TagResource, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_ARN,
            "case {case:?}: expected TagResource to answer the sentence this API answers an \
             unreadable ARN with, got {body}"
        );
    }

    // Not one refused write reached a record: every held resource carries what it was
    // created with.
    for target in &held {
        let case = format!("the held {} after the refusals", target.kind);
        assert_eq!(
            reported_tags(&emulator, &case, target).await,
            created,
            "case {case:?}: expected a refused TagResource to write nothing"
        );
    }
}

#[tokio::test]
async fn both_writes_of_the_tag_plane_state_the_length_of_the_nothing_they_carry() {
    // Neither write of the tag plane reports anything about the resource, so both answers
    // carry no content — and an answer that carries no content states the length it has,
    // whatever its status, rather than leaving the framing header off.
    let emulator = emulator();
    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "framed-tag-plane", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the api the writes address\": expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let arn = format!("arn:aws:apigateway:{REGION}::/apis/{api_id}");

    // The tag write itself: the status API Gateway answers it with, no body, and the length
    // of that nothing stated.
    write_tags(
        &emulator,
        "the tag write",
        &arn,
        &json!({ "team": "payments", "tier": "gold" }),
    )
    .await;

    // And the removal, whose modelled 204 is framed the same way.
    let uri = format!("{}?tagKeys={}", tags_uri(&arn), addressed("tier"));
    let (status, headers, body) = call_reading_headers(&emulator, "DELETE", &uri, None).await;
    assert_eq!(
        status, 204,
        "case \"the tag removal\": expected 204 from UntagResource at {uri}, got {status} {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case \"the tag removal\": expected UntagResource to answer no body at all, got {body}"
    );
    assert_eq!(
        content_length(&headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case \"the tag removal\": expected the answer to state content-length \
         {NO_CONTENT_LENGTH}, got {:?}",
        content_length(&headers)
    );

    // Both writes landed in the one collection the readers project.
    let (status, body) = call(&emulator, "GET", &tags_uri(&arn), None).await;
    assert_eq!(
        status, 200,
        "case \"the tag plane after both writes\": expected 200 from GetTags, got {status} {body}"
    );
    assert_eq!(
        body.get("tags"),
        Some(&json!({ "team": "payments" })),
        "case \"the tag plane after both writes\": expected the written key the removal did not \
         name, got {body}"
    );
}

/// The `ApiId` of the api among `targets` — the row's api is the first one.
fn api_id_of(targets: &[Target]) -> String {
    targets
        .first()
        .expect("the api is the first taggable resource of a row")
        .read_uri
        .rsplit('/')
        .next()
        .expect("the api's read URI ends in its id")
        .to_string()
}

#[tokio::test]
async fn the_collection_a_tag_write_leaves_is_the_one_the_store_holds() {
    // The projection every reader of one resource serves comes from the stored record, so
    // what a write left is a fact about the store and not about the answer: the store is
    // read directly here, beside the readers' own report.
    let (emulator, engine) = emulator_with_engine();

    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "stored-write",
            "protocolType": "HTTP",
            "tags": { "team": "payments", "tier": "gold" }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the stored api\": expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let arn = format!("arn:aws:apigateway:{REGION}::/apis/{api_id}");

    write_tags(
        &emulator,
        "the stored api",
        &arn,
        &json!({ "tier": "silver", "env": "staging" }),
    )
    .await;

    let stored: BTreeMap<String, String> = engine
        .state::<service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .apis
        .get(&api_id)
        .expect("the api the create persisted")
        .tags
        .clone();
    assert_eq!(
        stored,
        BTreeMap::from([
            ("team".to_string(), "payments".to_string()),
            ("tier".to_string(), "silver".to_string()),
            ("env".to_string(), "staging".to_string()),
        ]),
        "case \"the stored api\": expected the store itself to hold the created keys with the \
         written entries merged over them, got {stored:?}"
    );
}
