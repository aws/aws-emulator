// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the domain **tag** surface — `AddTags`, `ListTags` and
//! `RemoveTags` — driven as real restJson1 requests through the router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded call:
//!
//! 1. A domain's tags are addressed by the ARN its create reported: `ListTags`
//!    answers the stored set (empty for a domain nothing has tagged), `AddTags`
//!    merges into it — a key already held takes the submitted value, every other
//!    key is untouched — and `RemoveTags` drops only the keys it names.
//! 2. The tag set is PERSISTED per domain: tagging one domain leaves another
//!    domain's tags alone, and a read never changes what the next read sees.
//! 3. A domain may carry at most the documented number of tags, on BOTH paths
//!    that store a tag set: the call that would take an existing domain's set
//!    past the allowance is refused and attaches nothing, and a create whose
//!    `TagList` is longer than the allowance stores no domain at all.
//! 4. An `ARN` that resolves to no domain of this account is refused as invalid
//!    input by all three operations, and attaches nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for; the ARNs the service mints carry it.
const REGION: &str = "us-west-2";

/// `AddTags`'s own documentation: "An Elasticsearch domain may have up to 10
/// tags."
const MAX_TAGS_PER_DOMAIN: usize = 10;

async fn create_domain(router: &MultiServiceRouter, body: &Value) -> Answer {
    call_in(router, REGION, "POST", "/2015-01-01/es/domain", Some(body)).await
}

/// The ARN a create reported for the domain it made.
async fn domain_arn(router: &MultiServiceRouter, name: &str, tags: &[(&str, &str)]) -> String {
    let body = json!({
        "DomainName": name,
        "ElasticsearchVersion": "7.10",
        "ElasticsearchClusterConfig": {"InstanceType": "t2.small.elasticsearch", "InstanceCount": 1},
        "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 10},
        "TagList": tag_list(tags)
    });
    let created = create_domain(router, &body).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    created.text(name, "/DomainStatus/ARN")
}

fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

async fn add_tags(router: &MultiServiceRouter, arn: &str, tags: &[(&str, &str)]) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/tags",
        Some(&json!({"ARN": arn, "TagList": tag_list(tags)})),
    )
    .await
}

async fn remove_tags(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/tags-removal",
        Some(&json!({"ARN": arn, "TagKeys": keys})),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/tags?arn={arn}"),
        None,
    )
    .await
}

/// The tags `ListTags` reported, as a key/value map: the model documents no
/// order for `TagList`, so a case asserts the SET.
async fn tags_of(router: &MultiServiceRouter, arn: &str, case: &str) -> BTreeMap<String, String> {
    let answer = list_tags(router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListTags must answer 200, got {} {}",
        answer.status, answer.body
    );
    let entries = answer
        .at(case, "/TagList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: TagList is not a list, got {body}",
                body = answer.body
            )
        })
        .clone();
    entries
        .iter()
        .map(|entry| {
            (
                entry
                    .pointer("/Key")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: a tag has no Key, got {entry}"))
                    .to_string(),
                entry
                    .pointer("/Value")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: a tag has no Value, got {entry}"))
                    .to_string(),
            )
        })
        .collect()
}

fn expected(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// One tag lifecycle: the tags the create gave the domain, the `AddTags` calls
/// made against it in order, the keys one `RemoveTags` then names, and the set
/// `ListTags` must report after each stage.
struct TagCase {
    label: &'static str,
    domain: &'static str,
    created_with: Vec<(&'static str, &'static str)>,
    adds: Vec<Vec<(&'static str, &'static str)>>,
    removed_keys: Vec<&'static str>,
    after_adds: Vec<(&'static str, &'static str)>,
    after_removal: Vec<(&'static str, &'static str)>,
}

/// Rows differ along what rule 1 generalises over: how many keys one call
/// carries, how many calls the set is built up over, whether a key is new or
/// already held, the sizes the `Tag` members admit, and whether a removed key
/// is held at all.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "a domain nothing has tagged",
            domain: "tags-untagged",
            created_with: vec![],
            adds: vec![],
            removed_keys: vec![],
            after_adds: vec![],
            after_removal: vec![],
        },
        TagCase {
            label: "two keys in one call, one of them removed",
            domain: "tags-pair",
            created_with: vec![],
            adds: vec![vec![("team", "search"), ("cost-center", "platform")]],
            removed_keys: vec!["cost-center"],
            after_adds: vec![("team", "search"), ("cost-center", "platform")],
            after_removal: vec![("team", "search")],
        },
        TagCase {
            label: "two calls are cumulative",
            domain: "tags-cumulative",
            created_with: vec![],
            adds: vec![vec![("alpha", "1")], vec![("beta", "2")]],
            removed_keys: vec![],
            after_adds: vec![("alpha", "1"), ("beta", "2")],
            after_removal: vec![("alpha", "1"), ("beta", "2")],
        },
        TagCase {
            label: "a key added again takes the new value",
            domain: "tags-revalued",
            created_with: vec![],
            adds: vec![vec![("env", "dev")], vec![("env", "prod"), ("owner", "a")]],
            removed_keys: vec!["owner"],
            after_adds: vec![("env", "prod"), ("owner", "a")],
            after_removal: vec![("env", "prod")],
        },
        TagCase {
            // `Tag$Key` is @length(1..128) and `Tag$Value` @length(0..256), so
            // the shortest key and an empty value are both inside the model.
            label: "the shortest key and an empty value",
            domain: "tags-edges",
            created_with: vec![],
            adds: vec![vec![("k", ""), ("key-with-a-longer-name", "a value with spaces")]],
            removed_keys: vec!["key-with-a-longer-name"],
            after_adds: vec![("k", ""), ("key-with-a-longer-name", "a value with spaces")],
            after_removal: vec![("k", "")],
        },
        TagCase {
            label: "the create's own tags are the starting set",
            domain: "tags-from-create",
            created_with: vec![("team", "search")],
            adds: vec![vec![("team", "relevance"), ("tier", "prod")]],
            removed_keys: vec!["tier"],
            after_adds: vec![("team", "relevance"), ("tier", "prod")],
            after_removal: vec![("team", "relevance")],
        },
        TagCase {
            label: "removing a key the domain never had changes nothing",
            domain: "tags-ghost-key",
            created_with: vec![],
            adds: vec![vec![("alpha", "1")]],
            removed_keys: vec!["never-attached"],
            after_adds: vec![("alpha", "1")],
            after_removal: vec![("alpha", "1")],
        },
        TagCase {
            label: "removing every key empties the set",
            domain: "tags-emptied",
            created_with: vec![],
            adds: vec![vec![("alpha", "1"), ("beta", "2")]],
            removed_keys: vec!["alpha", "beta"],
            after_adds: vec![("alpha", "1"), ("beta", "2")],
            after_removal: vec![],
        },
    ]
}

/// RULE 1: the tag lifecycle over the whole class of inputs.
#[tokio::test]
async fn the_tag_lifecycle_adds_lists_and_removes_by_key() {
    for case in tag_cases() {
        let router = router();
        let label = case.label;
        let arn = domain_arn(&router, case.domain, &case.created_with).await;

        assert_eq!(
            tags_of(&router, &arn, label).await,
            expected(&case.created_with),
            "case {label:?}: a fresh domain must report exactly the tags its create gave it"
        );

        for (index, add) in case.adds.iter().enumerate() {
            let added = add_tags(&router, &arn, add).await;
            assert_eq!(
                added.status, 200,
                "case {label:?}: AddTags call {index} must answer 200, got {} {}",
                added.status, added.body
            );
        }
        assert_eq!(
            tags_of(&router, &arn, label).await,
            expected(&case.after_adds),
            "case {label:?}: expected {:?} after the adds",
            expected(&case.after_adds)
        );

        let removed = remove_tags(&router, &arn, &case.removed_keys).await;
        assert_eq!(
            removed.status, 200,
            "case {label:?}: RemoveTags must answer 200, got {} {}",
            removed.status, removed.body
        );
        assert_eq!(
            tags_of(&router, &arn, label).await,
            expected(&case.after_removal),
            "case {label:?}: expected {:?} after removing {:?}",
            expected(&case.after_removal),
            case.removed_keys
        );
    }
}

/// RULE 2: each domain holds its own tags, and reading them changes nothing.
#[tokio::test]
async fn tags_are_per_domain_and_a_read_does_not_move_them() {
    let router = router();
    let one = domain_arn(&router, "tags-domain-one", &[]).await;
    let two = domain_arn(&router, "tags-domain-two", &[("kept", "yes")]).await;

    let added = add_tags(&router, &one, &[("only-on-one", "1")]).await;
    assert_eq!(
        added.status, 200,
        "AddTags must answer 200, got {} {}",
        added.status, added.body
    );

    for case in ["first read", "second read"] {
        assert_eq!(
            tags_of(&router, &one, case).await,
            expected(&[("only-on-one", "1")]),
            "case {case:?}: the tagged domain must report its own tag"
        );
        assert_eq!(
            tags_of(&router, &two, case).await,
            expected(&[("kept", "yes")]),
            "case {case:?}: the other domain's tags must be untouched"
        );
    }

    let removed = remove_tags(&router, &one, &["only-on-one"]).await;
    assert_eq!(
        removed.status, 200,
        "RemoveTags must answer 200, got {} {}",
        removed.status, removed.body
    );
    assert_eq!(
        tags_of(&router, &two, "after removing from the other domain").await,
        expected(&[("kept", "yes")]),
        "removing a key from one domain must not touch another domain's tags"
    );
}

/// RULE 3: the documented per-domain allowance bounds the set, and the refused
/// call attaches nothing.
#[tokio::test]
async fn the_documented_tag_allowance_bounds_one_domains_set() {
    let router = router();
    let arn = domain_arn(&router, "tags-at-the-limit", &[]).await;

    let keys: Vec<String> = (0..MAX_TAGS_PER_DOMAIN)
        .map(|index| format!("key-{index}"))
        .collect();
    let at_limit: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
    let filled = add_tags(&router, &arn, &at_limit).await;
    assert_eq!(
        filled.status, 200,
        "a set at the allowance must be accepted, got {} {}",
        filled.status, filled.body
    );
    assert_eq!(
        tags_of(&router, &arn, "at the allowance").await.len(),
        MAX_TAGS_PER_DOMAIN,
        "the domain must hold the whole set it was given"
    );

    // Re-valuing keys the domain already holds does not grow the set, so it is
    // not refused by the allowance.
    let revalued = add_tags(&router, &arn, &[(keys[0].as_str(), "w")]).await;
    assert_eq!(
        revalued.status, 200,
        "re-valuing a held key must be accepted at the allowance, got {} {}",
        revalued.status, revalued.body
    );

    let over = add_tags(&router, &arn, &[("one-too-many", "v")]).await;
    assert_eq!(
        over.status, 409,
        "a key past the allowance must be refused with 409, got {} {}",
        over.status, over.body
    );
    assert!(
        over.error_type.contains("LimitExceededException"),
        "the error must be LimitExceededException, got {:?} / {}",
        over.error_type,
        over.body
    );

    // A single call naming more tags than a domain may carry is refused on its
    // own list, before any of it is merged.
    let long: Vec<String> = (0..MAX_TAGS_PER_DOMAIN + 1)
        .map(|index| format!("bulk-{index}"))
        .collect();
    let long_list: Vec<(&str, &str)> = long.iter().map(|key| (key.as_str(), "v")).collect();
    let bulk = add_tags(&router, &arn, &long_list).await;
    assert_eq!(
        bulk.status, 409,
        "a list longer than the allowance must be refused with 409, got {} {}",
        bulk.status, bulk.body
    );
    let keys_to_remove: Vec<&str> = long.iter().map(String::as_str).collect();
    let bulk_removal = remove_tags(&router, &arn, &keys_to_remove).await;
    assert_eq!(
        bulk_removal.status, 200,
        "a removal may name any number of keys, got {} {}",
        bulk_removal.status, bulk_removal.body
    );
    let left = tags_of(&router, &arn, "after the bulk removal").await;
    assert_eq!(
        left.len(),
        MAX_TAGS_PER_DOMAIN,
        "a removal naming only keys the domain never had must drop nothing, got {left:?}"
    );
    let held = tags_of(&router, &arn, "after the refusal").await;
    assert_eq!(
        held.len(),
        MAX_TAGS_PER_DOMAIN,
        "the refused call must attach nothing, got {held:?}"
    );
    assert!(
        !held.contains_key("one-too-many"),
        "the refused key must not be held, got {held:?}"
    );
    assert_eq!(
        held.get(&keys[0]).map(String::as_str),
        Some("w"),
        "the accepted re-valuing must have landed, got {held:?}"
    );
}

/// RULE 3, on the other path that stores a tag set: a CREATE whose `TagList` is
/// longer than the allowance is refused on that list and stores no domain, so no
/// path can leave a domain carrying more tags than a domain may have.
#[tokio::test]
async fn the_allowance_bounds_the_tag_list_a_create_stores() {
    let router = router();

    // The rows cross the published bound: one below it, exactly at it, and one
    // past it.
    for (case, count, expected) in [
        ("one below the allowance", MAX_TAGS_PER_DOMAIN - 1, 200),
        ("exactly at the allowance", MAX_TAGS_PER_DOMAIN, 200),
        ("one past the allowance", MAX_TAGS_PER_DOMAIN + 1, 409),
    ] {
        let name = format!("created-with-{count}-tags");
        let keys: Vec<String> = (0..count).map(|index| format!("key-{index}")).collect();
        let tags: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
        let created = create_domain(
            &router,
            &json!({"DomainName": name, "TagList": tag_list(&tags)}),
        )
        .await;
        assert_eq!(
            created.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {}",
            created.status, created.body
        );
        if expected == 200 {
            let arn = created.text(case, "/DomainStatus/ARN");
            assert_eq!(
                tags_of(&router, &arn, case).await.len(),
                count,
                "case {case:?}: the domain must hold the whole set its create carried"
            );
            continue;
        }
        assert!(
            created.error_type.contains("LimitExceededException"),
            "case {case:?}: the error must be LimitExceededException, got {:?} / {}",
            created.error_type,
            created.body
        );
        let read = call_in(
            &router,
            REGION,
            "GET",
            &format!("/2015-01-01/es/domain/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store no domain, got {} {}",
            read.status, read.body
        );
    }
}

/// RULE 4: an ARN naming no domain of this account is refused by all three
/// operations.
#[tokio::test]
async fn an_arn_naming_no_domain_is_refused_as_invalid_input() {
    let router = router();
    // One real domain, so the refusals below are about the ARN rather than about
    // an empty store.
    let real = domain_arn(&router, "tags-real-domain", &[("kept", "yes")]).await;

    // Rows differ along HOW the ARN fails to resolve: not an ARN at all, an ARN
    // of another service, a domain ARN for a domain that was never created, and
    // the right domain in the wrong Region.
    for (case, arn) in [
        ("an empty ARN", String::new()),
        ("a string that is no ARN", "not-an-arn".to_string()),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
        (
            "a domain ARN for a domain that was never created",
            "arn:aws:es:us-west-2:000000000000:domain/tags-absent".to_string(),
        ),
        (
            "the domain's name in another Region",
            "arn:aws:es:eu-west-1:000000000000:domain/tags-real-domain".to_string(),
        ),
    ] {
        for (operation, answer) in [
            ("ListTags", list_tags(&router, &arn).await),
            ("AddTags", add_tags(&router, &arn, &[("a", "1")]).await),
            ("RemoveTags", remove_tags(&router, &arn, &["a"]).await),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: {operation} must refuse it with 400, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains("ValidationException"),
                "case {case:?}: {operation} must answer ValidationException, got {:?} / {}",
                answer.error_type,
                answer.body
            );
        }
    }

    assert_eq!(
        tags_of(&router, &real, "after every refusal").await,
        expected(&[("kept", "yes")]),
        "a refused tag call must leave the real domain's tags alone"
    );
}
