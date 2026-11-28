// SPDX-License-Identifier: CC0-1.0

//! Resource tagging (`TagResource` / `ListTagsForResource`), end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper. Note the tagging shapes' member names are lowercase
//! on the wire (`resourceArn`, `tags`), unlike the rest of this service.
//!
//! The rules under test:
//!
//! * `TagResource` merges the request's tags onto the tag set stored on the resource its
//!   `resourceArn` names — a key the request carries takes its value, every other key
//!   keeps its own — and `ListTagsForResource` reports exactly that merged set.
//! * `UntagResource`'s side of the same state — the removal — is the sibling suite
//!   `tests/all/resource_tags.rs`, which reuses the helpers below.
//! * a `resourceArn` naming no resource the service holds is refused
//!   `ResourceNotFoundException` (404) with the service's catalog-lookup message for the
//!   `Catalog` the request named, which the tagging operations never name.
//! * a merge that would take one resource past the modelled tag bound
//!   (`CreateBenefitApplicationInput$Tags` `length {max: 200}`, re-exported as
//!   `MAX_TAGS_PER_RESOURCE`) is refused `ServiceQuotaExceededException` (402).
//!
//! The tests are tables: the rows differ along the tag count, the tag values (new key,
//! overwritten key, empty value, Unicode key, each published key/value length bound), the
//! catalog the resource lives in, the resource type an ARN names, and the form of the ARN
//! itself. The recorded call the issue named — a well-formed tagging call on a benefit
//! that does not exist, from an account with no active partner benefit — is one row among
//! them.

use std::collections::BTreeMap;

// That a failed resource lookup is ONE construction, answered identically by all three
// tagging operations, is its own suite in the submodule below. `tests/all/resource_tagging.rs`
// is itself loaded by `#[path]` from `tests/all.rs`, so a submodule of it is looked up
// beside it rather than in a directory named after it: the `#[path]` here is what points at
// `tests/all/resource_tagging/`.
#[path = "resource_tagging/tagging_refusal_one_construction.rs"]
mod tagging_refusal_one_construction;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    catalog_not_found_message, APPLICATION_RESOURCE, BENEFIT_RESOURCE, MAX_TAGS_PER_RESOURCE,
    SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, Harness};

/// The region the harness signs for; the ARNs the service returns carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The status this service serves its `AccessDeniedException` with (recorded; the model
/// declares the shape `@httpError(403)`) — the rule's own table is
/// `tests/all/entitlement_denial.rs`.
const DENIED_STATUS: u16 = 400;

/// Call one operation that must answer 200, and return its body.
pub(crate) async fn ok(harness: &Harness, case: &str, operation: &str, body: &Value) -> Value {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {out:?}"
    );
    out
}

/// The id of the first benefit of `catalog`, which every create below applies for.
pub(crate) async fn benefit_id(harness: &Harness, case: &str, catalog: &str) -> String {
    let out = ok(harness, case, "ListBenefits", &json!({ "Catalog": catalog })).await;
    out["BenefitSummaries"]
        .as_array()
        .and_then(|summaries| summaries.first())
        .and_then(|summary| summary["Id"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a seeded benefit, got {out:?}"))
        .to_string()
}

/// Create one benefit application in `catalog`, carrying `tags`, and return its ARN.
pub(crate) async fn application_arn(
    harness: &Harness,
    case: &str,
    catalog: &str,
    token: &str,
    tags: &[(&str, &str)],
) -> String {
    let benefit = benefit_id(harness, case, catalog).await;
    let mut body = json!({
        "Catalog": catalog,
        "ClientToken": token,
        "BenefitIdentifier": benefit,
        "Name": token,
    });
    if !tags.is_empty() {
        body["Tags"] = tag_payload(tags);
    }
    let created = ok(harness, case, "CreateBenefitApplication", &body).await;
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no Arn: {created:?}"))
        .to_string()
}

/// The tag set `ListTagsForResource` reports for `resource_arn`.
pub(crate) async fn tags_of(
    harness: &Harness,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let out = ok(
        harness,
        case,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    let listed = out["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {out:?}"))
        .clone();
    let mut tags = BTreeMap::new();
    for tag in &listed {
        let key = tag["Key"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a tag carries no Key: {tag:?}"))
            .to_string();
        let value = tag["Value"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a tag carries no Value: {tag:?}"))
            .to_string();
        assert!(
            tags.insert(key.clone(), value).is_none(),
            "case {case:?}: the key {key:?} was reported twice in {listed:?}"
        );
    }
    tags
}

/// The modelled `Tags` list for `tags`, as the wire names its members.
pub(crate) fn tag_payload(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The error message of a failure body, under either casing the protocols use: the
/// framework's own refusals spell the member `message`, this service's modelled errors
/// `Message`.
pub(crate) fn error_message(body: &Value) -> String {
    ["message", "Message"]
        .into_iter()
        .find_map(|member| body.get(member).and_then(Value::as_str))
        .unwrap_or("<no message>")
        .to_string()
}

/// The ARN this service gives the `resource` named `id` in `catalog`.
///
/// The account field is the one the resource type itself declares: a benefit's ARN carries
/// an EMPTY account field (its modelled `@arn` trait declares `noAccount: true`), while a
/// benefit application's and a benefit allocation's carry the emulator account id. The
/// modelled `Arn` pattern admits both, its account field being optional
/// (`^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$`). ONE
/// construction of the rule for the whole test target, so no suite spells a form the
/// service does not mint.
pub(crate) fn expected_arn(catalog: &str, resource: &str, id: &str) -> String {
    let account = if resource == BENEFIT_RESOURCE {
        ""
    } else {
        ACCOUNT
    };
    format!("arn:aws:partnercentral:{REGION}:{account}:catalog/{catalog}/{resource}/{id}")
}

/// An ARN of the form the modelled `TaggableResourceArn` pattern requires: its account
/// field is a twelve-digit account id
/// (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/...$`),
/// whatever ARN form the resource type it names declares.
///
/// A tagging row naming a resource type whose own ARN carries no account field (a benefit)
/// spells it with this, so the call is answered by the resource LOOKUP — the rule those
/// rows are about — instead of being refused for the member's pattern.
pub(crate) fn taggable_arn(catalog: &str, resource: &str, id: &str) -> String {
    format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{catalog}/{resource}/{id}")
}

/// A key or value of exactly `len` characters, inside the modelled `TagKey` / `TagValue`
/// pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`.
pub(crate) fn filler(len: usize) -> String {
    "k".repeat(len)
}

/// The two tagging operations implemented here, with a body naming `resource_arn`.
fn tagging_calls(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "resourceArn": resource_arn, "tags": tag_payload(&[("env", "test")]) }),
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": resource_arn }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// TagResource + ListTagsForResource — the merge rule
// ---------------------------------------------------------------------------

/// A tagging call merges its tags onto the resource's stored set and nothing else: the
/// keys it names take its values, the keys it does not name survive, and the next
/// `ListTagsForResource` reports exactly that merge.
///
/// The expectation is computed by replaying the same merge over a `BTreeMap`, so what is
/// pinned is the rule rather than one recorded answer. The rows differ along the tag count
/// (0, 1, several), whether a key is new or overwritten, the value (ordinary, empty,
/// Unicode, at the published length bound), and the catalog the resource lives in — the
/// row set is `SEEDED_CATALOGS`, so it cannot drift from the store.
#[tokio::test]
async fn tagging_merges_onto_the_stored_tag_set_of_the_named_resource() {
    // Each row is one `TagResource` call on the same resource, applied in order.
    // `TagKey` is `length 1..128`, `TagValue` `length 0..256`, and both carry the pattern
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` (smithy_query shape=Tag).
    let long_key = filler(128);
    let long_value = filler(256);
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one new key", vec![("env", "test")]),
        (
            "several new keys at once",
            vec![("team", "partner-ops"), ("cost.center", "cc:1234")],
        ),
        ("the same call again", vec![("env", "test")]),
        ("an existing key, new value", vec![("env", "prod")]),
        ("an empty value", vec![("placeholder", "")]),
        (
            "a Unicode key and value",
            vec![("équipe partenaire", "日本 région")],
        ),
        (
            "each published length bound",
            vec![(long_key.as_str(), long_value.as_str())],
        ),
        (
            "a key the create supplied",
            vec![("seeded", "overwritten-by-tag")],
        ),
    ];

    for catalog in SEEDED_CATALOGS {
        let harness = Harness::new();
        // The create's own `Tags` are the resource's starting tag set, so the merge is
        // checked against state a different operation wrote.
        let seeded = [("seeded", "from-create"), ("kept", "untouched")];
        let arn = application_arn(
            &harness,
            catalog,
            catalog,
            &format!("tok-tag-{catalog}"),
            &seeded,
        )
        .await;
        let mut expected: BTreeMap<String, String> = seeded
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags_of(&harness, catalog, &arn).await,
            expected,
            "case {catalog:?}: ListTagsForResource must report the tags the create stored"
        );

        for (name, tags) in &rows {
            let case = format!("{catalog}: {name}");
            ok(
                &harness,
                &case,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": tag_payload(tags) }),
            )
            .await;
            for (key, value) in tags {
                expected.insert((*key).to_string(), (*value).to_string());
            }
            let got = tags_of(&harness, &case, &arn).await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected the merged tag set {expected:?}, got {got:?}"
            );
        }
    }
}

/// A resource carrying no tags reports an empty list rather than a missing member or an
/// invented tag.
#[tokio::test]
async fn a_resource_with_no_tags_reports_an_empty_list() {
    let harness = Harness::new();
    let arn = application_arn(&harness, "untagged", SEEDED_CATALOGS[0], "tok-empty", &[]).await;
    let got = tags_of(&harness, "untagged", &arn).await;
    assert!(
        got.is_empty(),
        "case \"untagged\": expected no tags, got {got:?}"
    );
}

/// Tagging one resource leaves every other resource's tag set alone, in the same catalog
/// and across catalogs.
#[tokio::test]
async fn tagging_one_resource_does_not_touch_another() {
    let harness = Harness::new();
    let mut arns: Vec<(String, String)> = Vec::new();
    for catalog in SEEDED_CATALOGS {
        for index in 0..2 {
            let case = format!("{catalog}-{index}");
            let arn = application_arn(
                &harness,
                &case,
                catalog,
                &format!("tok-scope-{catalog}-{index}"),
                &[("owner", &case)],
            )
            .await;
            arns.push((case, arn));
        }
    }

    for (case, arn) in &arns {
        ok(
            &harness,
            case,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": tag_payload(&[("tagged", case.as_str())]),
            }),
        )
        .await;
    }

    for (case, arn) in &arns {
        let got = tags_of(&harness, case, arn).await;
        let expected: BTreeMap<String, String> = [
            ("owner".to_string(), case.clone()),
            ("tagged".to_string(), case.clone()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: a resource must carry only its own tags, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The resource lookup
// ---------------------------------------------------------------------------

/// A well-formed `resourceArn` that names no resource this service holds is a 404
/// `ResourceNotFoundException`, on the write and on the read alike, and nothing is
/// persisted for it.
///
/// The rows span every way an ARN of the modelled form can fail to resolve: an id nothing
/// holds, a resource type that holds no tag set (a benefit — the model binds `Tags` on no
/// benefit shape, and spelled with the account field the member's pattern requires, see
/// [`taggable_arn`]), a catalog the service does not serve, and an existing resource's ARN
/// in another region, account or partition.
#[tokio::test]
async fn tagging_an_arn_that_names_no_resource_is_404() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let existing =
        application_arn(&harness, "seed", catalog, "tok-lookup", &[("env", "test")]).await;
    let benefit = benefit_id(&harness, "seed", catalog).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an application id nothing holds",
            expected_arn(catalog, APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            // The recorded instance of the rule.
            "a benefit that does not exist",
            taggable_arn(catalog, BENEFIT_RESOURCE, "ben-00000000000000"),
        ),
        (
            "a benefit that does exist, which holds no tag set",
            taggable_arn(catalog, BENEFIT_RESOURCE, &benefit),
        ),
        (
            "a catalog the service does not serve",
            expected_arn("NotACatalog", APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            "the application's ARN in another region",
            existing.replace(REGION, "eu-west-1"),
        ),
        (
            "the application's ARN under another account",
            existing.replace(ACCOUNT, "210987654321"),
        ),
        (
            "the application's ARN in another partition",
            existing.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "a resource type this service does not serve",
            expected_arn(catalog, "opportunity", "O-1"),
        ),
    ];

    for (name, resource_arn) in &rows {
        for (operation, body) in tagging_calls(resource_arn) {
            let case = format!("{operation}: {name}");
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for {resource_arn:?}, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
            assert_eq!(
                error_message(&out),
                catalog_not_found_message(None),
                "case {case:?}: unexpected not-found message in {out:?}"
            );
        }
    }

    // None of those refusals wrote anything: the one real resource still carries exactly
    // the tags its create supplied.
    let got = tags_of(&harness, "after the refusals", &existing).await;
    assert_eq!(
        got,
        [("env".to_string(), "test".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"after the refusals\": a refused tagging call must persist nothing, got {got:?}"
    );
}

/// The recorded shape of the issue's own call: the tag-MUTATING path is NOT behind the
/// partner-entitlement gate, so an account with no active partner benefit — every caller
/// from the wire — is answered by the resource lookup. The tag-READING operation IS
/// behind it (corpus `simparity-wire-aws-v1`, fixture
/// `partnercentral-benefits-list-and-tag-validation-error-paths` step `task#4`), so the
/// same ARN is refused as denied there and never says whether the resource exists; the
/// denial's own table is `tests/all/entitlement_denial.rs`.
#[tokio::test]
async fn an_unentitled_caller_is_answered_by_the_resource_lookup() {
    let harness = Harness::unentitled();
    let resource_arn = taggable_arn(
        SEEDED_CATALOGS[0],
        BENEFIT_RESOURCE,
        "ben-00000000000000",
    );

    let (status, out) = harness
        .call(
            "TagResource",
            &json!({
                "resourceArn": resource_arn,
                "tags": tag_payload(&[("env", "test")]),
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"TagResource, unentitled\": expected the lookup's 404, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ResourceNotFoundException",
        "case \"TagResource, unentitled\": expected ResourceNotFoundException, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        catalog_not_found_message(None),
        "case \"TagResource, unentitled\": unexpected not-found message in {out:?}"
    );

    let (status, out) = harness
        .call("ListTagsForResource", &json!({ "resourceArn": resource_arn }))
        .await;
    assert_eq!(
        status, DENIED_STATUS,
        "case \"ListTagsForResource, unentitled\": expected the gate's refusal, \
         got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "AccessDeniedException",
        "case \"ListTagsForResource, unentitled\": expected AccessDeniedException, got {out:?}"
    );
}

/// A `resourceArn` outside the modelled `TaggableResourceArn` pattern or length is a 400
/// `ValidationException`, not a 404 and not a success.
#[tokio::test]
async fn tagging_refuses_a_malformed_resource_arn() {
    let harness = Harness::new();

    // Each row breaks one part of the modelled pattern
    // `^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$`
    // (smithy_query shape=TaggableResourceArn).
    let rows: Vec<(&str, String)> = vec![
        // The recorded instance of the rule.
        ("not an ARN at all", "not-an-arn".to_string()),
        ("an empty ARN, under the 1..1000 length", String::new()),
        (
            "another service's namespace",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:catalog/AWS/{APPLICATION_RESOURCE}/benappl-00000000000000"),
        ),
        (
            "an account that is not 12 digits",
            format!("arn:aws:partnercentral:{REGION}:12345:catalog/AWS/{APPLICATION_RESOURCE}/benappl-00000000000000"),
        ),
        (
            "no catalog segment",
            format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:{APPLICATION_RESOURCE}/benappl-00000000000000"),
        ),
        (
            "a catalog name outside [a-zA-Z]+",
            format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/AWS-1/{APPLICATION_RESOURCE}/benappl-00000000000000"),
        ),
        (
            "a catalog with no resource under it",
            format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/AWS"),
        ),
        (
            "past the 1..1000 length",
            format!(
                "arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/AWS/{APPLICATION_RESOURCE}/{}",
                filler(1000)
            ),
        ),
    ];

    for (name, resource_arn) in &rows {
        for (operation, body) in tagging_calls(resource_arn) {
            let case = format!("{operation}: {name}");
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for {resource_arn:?}, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The tag bound
// ---------------------------------------------------------------------------

/// A merge is accepted up to the modelled tag bound and refused one tag past it, and the
/// refusal leaves the resource's tag set exactly as it was.
///
/// The bound is `MAX_TAGS_PER_RESOURCE`, the model's own
/// `CreateBenefitApplicationInput$Tags` `length {max: 200}`.
#[tokio::test]
async fn tagging_refuses_a_merge_past_the_tag_bound() {
    let harness = Harness::new();
    let arn = application_arn(&harness, "seed", SEEDED_CATALOGS[0], "tok-quota", &[]).await;

    let keys: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key-{index}"))
        .collect();
    let full: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();

    // At the bound: accepted.
    ok(
        &harness,
        "at the bound",
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tag_payload(&full) }),
    )
    .await;
    let at_bound = tags_of(&harness, "at the bound", &arn).await;
    assert_eq!(
        at_bound.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"at the bound\": expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        at_bound.len()
    );

    // Re-tagging keys the resource already carries adds nothing to the count, so it stays
    // accepted even though the set is full.
    let updates: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "w")).collect();
    ok(
        &harness,
        "overwriting a full set",
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tag_payload(&updates) }),
    )
    .await;
    let overwritten = tags_of(&harness, "overwriting a full set", &arn).await;
    assert!(
        overwritten.values().all(|value| value == "w"),
        "case \"overwriting a full set\": every value must be the new one, got {overwritten:?}"
    );

    // One key past the bound: refused, and nothing changes.
    let (status, out) = harness
        .call(
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": tag_payload(&[("one-too-many", "v")]),
            }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case \"one past the bound\": expected 402, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ServiceQuotaExceededException",
        "case \"one past the bound\": expected ServiceQuotaExceededException, got {out:?}"
    );
    for member in ["ResourceId", "ResourceType", "QuotaCode"] {
        assert!(
            out.get(member)
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty()),
            "case \"one past the bound\": the refusal must report {member}, got {out:?}"
        );
    }
    let after = tags_of(&harness, "one past the bound", &arn).await;
    assert_eq!(
        after, overwritten,
        "case \"one past the bound\": a refused merge must change nothing, got {after:?}"
    );
}
