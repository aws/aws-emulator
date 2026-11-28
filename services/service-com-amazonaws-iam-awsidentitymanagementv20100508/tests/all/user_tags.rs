// SPDX-License-Identifier: CC0-1.0

//! `TagUser`, end to end over the awsQuery wire, read back through `GetUser`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the named resource's
//! stored tag set — a supplied key the resource already holds has its value
//! overwritten and a key it does not hold is added — the merged set is bounded by
//! the per-resource tag quota, and a request carrying any invalid tag stores
//! nothing. `TagUser`'s own documentation: "if a tag with the same key name already
//! exists, then that tag is overwritten with the new value", and "if any one of the
//! tags is invalid or if you exceed the allowed maximum number of tags, then the
//! entire request fails".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour of a user that was PERSISTED rather than a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns, the parameter and tag readers are `tests/all/role_lifecycle.rs`'s, and the
//! entity-independent tag-list builders are `tests/all/role_tags.rs`'s, so a
//! request is built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over:
//! the held tag count, the number of calls, whether a key is new or held, the case
//! of a key, the case and path of the user, and the published length bounds of
//! `tagKeyType` (1..128) and `tagValueType` (0..256). A list naming one key TWICE is
//! refused rather than merged, which `tests/all/supplied_tag_lists.rs` states for
//! every operation taking a tag list, and WHICH keys are one key on a principal is
//! `tests/all/principal_tag_key_case.rs`'s rule.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::user_lifecycle::{call, elements, error_code, router};

/// Create the user `user_name` under `path` carrying `initial`, asserting the
/// create succeeded.
pub(crate) async fn create_user(
    router: &MultiServiceRouter,
    user_name: &str,
    path: &str,
    initial: &[(String, String)],
) {
    let params = with_tag_params(
        vec![param("UserName", user_name), param("Path", path)],
        initial,
    );
    let (status, body) = call(router, "CreateUser", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
}

/// One `TagUser` call: the HTTP status and the XML body.
pub(crate) async fn tag_user(
    router: &MultiServiceRouter,
    user_name: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("UserName", user_name)], list);
    call(router, "TagUser", &borrowed(&params)).await
}

/// The tag set `GetUser` reports for `user_name`, as a map, plus how many `<Key>`
/// elements the response carried — so a duplicated key cannot hide inside a map.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    user_name: &str,
) -> (BTreeMap<String, String>, usize) {
    let (status, body) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: GetUser must report the tagged user, got {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key").len())
}

/// One row: a user created with `initial`, then `calls` successive `TagUser`
/// requests, then the tag set the user must hold.
struct MergeCase {
    name: &'static str,
    path: &'static str,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagUser` merges its tags into the user's stored set — a held key is
/// overwritten, a new key is added — and `GetUser` reports exactly that merged set.
/// One row per thing the rule generalises over: the held tag count, the number of
/// calls, new versus held keys, the case of a key, a key named again by a later
/// call, the user's path, and the published length bounds of `tagKeyType` and
/// `tagValueType`.
#[tokio::test]
async fn tag_user_merges_its_tags_into_the_stored_set() {
    // `tagValueType` is length 0..256 and `tagKeyType` length 1..128
    // (`smithy_query(query="shape", shape="Tag")`), so these are the published
    // bounds a stored tag must survive.
    let longest_value = "v".repeat(256);
    let longest_key = "k".repeat(128);

    let cases = vec![
        MergeCase {
            name: "a user holding no tags gains one",
            path: "/",
            initial: Vec::new(),
            calls: vec![tags(&[("team", "storage")])],
            expected: tags(&[("team", "storage")]),
        },
        MergeCase {
            name: "a new key is added beside the held ones",
            path: "/eng/",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("owner", "iam")])],
            expected: tags(&[("team", "storage"), ("env", "prod"), ("owner", "iam")]),
        },
        MergeCase {
            name: "a held key is overwritten rather than duplicated",
            path: "/eng/tools/",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("env", "staging")])],
            expected: tags(&[("team", "storage"), ("env", "staging")]),
        },
        MergeCase {
            name: "one call both overwrites and adds",
            path: "/",
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![tags(&[("b", "two"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "two"), ("c", "3")]),
        },
        MergeCase {
            name: "successive calls accumulate, the last value winning",
            path: "/sales/west/",
            initial: Vec::new(),
            calls: vec![
                tags(&[("k1", "v1")]),
                tags(&[("k2", "v2")]),
                tags(&[("k1", "v1-again")]),
            ],
            expected: tags(&[("k1", "v1-again"), ("k2", "v2")]),
        },
        MergeCase {
            // Tag "key–value pairs are not case sensitive, but case is preserved"
            // (`AssumeRoleRequest$Tags`, the STS model, of a principal's tags), so two
            // spellings of one word are ONE tag of a user: the held spelling stays and
            // the later value replaces the held one.
            name: "a key supplied in another case overwrites the held tag",
            path: "/",
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev")]), tags(&[("ENV", "test")])],
            expected: tags(&[("Env", "test")]),
        },
        MergeCase {
            name: "a value at the published minimum length",
            path: "/eng/",
            initial: tags(&[("team", "storage")]),
            calls: vec![tags(&[("empty", "")])],
            expected: tags(&[("team", "storage"), ("empty", "")]),
        },
        MergeCase {
            name: "a held value overwritten with the empty value",
            path: "/",
            initial: tags(&[("env", "prod")]),
            calls: vec![tags(&[("env", "")])],
            expected: tags(&[("env", "")]),
        },
        MergeCase {
            name: "a value at the published maximum length",
            path: "/",
            initial: Vec::new(),
            calls: vec![tags(&[("long", longest_value.as_str())])],
            expected: tags(&[("long", longest_value.as_str())]),
        },
        MergeCase {
            name: "a key at the published maximum length",
            path: "/",
            initial: Vec::new(),
            calls: vec![tags(&[(longest_key.as_str(), "fits")])],
            expected: tags(&[(longest_key.as_str(), "fits")]),
        },
        MergeCase {
            // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` and
            // `tagValueType`'s the same with `*`, so spaces, non-ASCII letters,
            // digits and every punctuation character of that class belong to the
            // class the rule covers.
            name: "a key and a value spanning the modelled alphabet",
            path: "/eng/",
            initial: Vec::new(),
            calls: vec![tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")])],
            expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")]),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let user_name = format!("merge-{index}");
        create_user(&router, &user_name, case.path, &case.initial).await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_user(&router, &user_name, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagUser call {call_index} must succeed, got {status}: {body}",
                case.name
            );
        }

        let (reported, keys) = stored_tags(&router, &user_name).await;
        assert_eq!(
            reported,
            expected_map(&case.expected),
            "case {:?}: expected the merged tag set {:?}, got {reported:?}",
            case.name,
            expected_map(&case.expected)
        );
        assert_eq!(
            keys,
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {keys}",
            case.name,
            case.expected.len()
        );
    }
}

/// RULE: a tag call names its resource by an IAM entity name, which is unique
/// within the account and "not distinguished by case" (`CreateUser$UserName`), so
/// any casing of a held name tags the SAME user — and a name the account does not
/// hold is refused with the modelled `NoSuchEntityException`, which `GetUser`
/// reports the same way.
#[tokio::test]
async fn the_user_is_found_by_its_case_folded_name() {
    let router = router();
    create_user(&router, "MyUser", "/eng/", &tags(&[("team", "storage")])).await;

    // Each spelling tags the one held user, so the set accumulates across them
    // instead of each spelling getting a user of its own.
    for (index, spelling) in ["MyUser", "myuser", "MYUSER", "mYuSeR"].iter().enumerate() {
        let added = tags(&[(&format!("key-{index}"), &format!("value-{index}"))]);
        let (status, body) = tag_user(&router, spelling, &added).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200 for a held name, got {status}: {body}"
        );
        let (reported, keys) = stored_tags(&router, spelling).await;
        assert_eq!(
            keys,
            index + 2,
            "case {spelling:?}: expected {} stored tags, got {keys}: {reported:?}",
            index + 2
        );
        assert_eq!(
            reported.get(&format!("key-{index}")).map(String::as_str),
            Some(format!("value-{index}").as_str()),
            "case {spelling:?}: the tag must be stored on the held user, got {reported:?}"
        );
    }

    for absent in ["OtherUser", "myuser2", "my_user", "user"] {
        let (status, body) = tag_user(&router, absent, &tags(&[("team", "storage")])).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected 404 for a name the account does not hold, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// RULE: the `aws:` tag-key prefix is reserved ("You cannot create tag keys that
/// begin with `aws:`", "Tagging IAM resources"), so a request naming one is refused
/// with the modelled `InvalidInputException` and the ENTIRE request is dropped —
/// no tag of it reaches the user.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let held = tags(&[("team", "storage")]);
    let refused = [
        ("the prefix alone", tags(&[("aws:", "x")])),
        (
            "a reserved key",
            tags(&[("aws:cloudformation:stack-name", "stack")]),
        ),
        // The prefix is reserved however it is capitalised.
        ("an upper-case prefix", tags(&[("AWS:Owner", "aws")])),
        ("a mixed-case prefix", tags(&[("Aws:Owner", "aws")])),
        (
            "a valid tag beside a reserved one",
            tags(&[("owner", "iam"), ("aws:service", "iam")]),
        ),
        (
            "a reserved key before a valid one",
            tags(&[("aws:service", "iam"), ("owner", "iam")]),
        ),
    ];

    let router = router();
    for (index, (case, list)) in refused.iter().enumerate() {
        let user_name = format!("reserved-{index}");
        create_user(&router, &user_name, "/", &held).await;

        let (status, body) = tag_user(&router, &user_name, list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a reserved tag key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The whole request failed, so the user still holds exactly what it was
        // created with: neither the reserved key nor the valid one beside it.
        let (reported, keys) = stored_tags(&router, &user_name).await;
        assert_eq!(
            reported,
            expected_map(&held),
            "case {case:?}: a refused request must store nothing, got {reported:?}"
        );
        assert_eq!(
            keys,
            held.len(),
            "case {case:?}: expected {} stored tags, got {keys}",
            held.len()
        );
    }
}

/// RULE: a resource carries at most [`MAX_TAGS_PER_ENTITY`] tags (AWS's "IAM and
/// STS quotas": "Number of tags per IAM resource: 50"), and the bound is taken
/// against the MERGED set — so overwriting a held key is never refused, while a
/// call that would carry the resource past the bound is refused with the modelled
/// `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_tag_bound_is_taken_against_the_merged_set() {
    let bound = MAX_TAGS_PER_ENTITY;
    let fixture = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect()
    };

    // One row per position relative to the published bound: just inside it, at it,
    // and the first call that would pass it.
    let accepted = [
        ("one below the bound, one new key", bound - 1, fixture(1)),
        (
            "two below the bound, two new keys",
            bound - 2,
            vec![
                ("new-a".to_string(), "a".to_string()),
                ("new-b".to_string(), "b".to_string()),
            ],
        ),
        (
            "at the bound, overwriting a held key",
            bound,
            vec![("key-00".to_string(), "overwritten".to_string())],
        ),
        (
            "at the bound, overwriting several held keys",
            bound,
            vec![
                ("key-01".to_string(), "one".to_string()),
                ("key-02".to_string(), "two".to_string()),
            ],
        ),
    ];

    let router = router();
    for (index, (case, held_count, list)) in accepted.iter().enumerate() {
        let user_name = format!("bound-ok-{index}");
        let held = fixture(*held_count);
        create_user(&router, &user_name, "/", &held).await;

        let (status, body) = tag_user(&router, &user_name, list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 inside the tag bound, got {status}: {body}"
        );

        let (reported, keys) = stored_tags(&router, &user_name).await;
        let mut expected: BTreeMap<String, String> = expected_map(&held);
        expected.extend(list.iter().cloned());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the merged tag set {expected:?}, got {reported:?}"
        );
        assert!(
            keys <= bound,
            "case {case:?}: a user must never hold more than {bound} tags, got {keys}"
        );
    }

    let refused = [
        ("at the bound, one new key", bound, 1usize),
        ("one below the bound, two new keys", bound - 1, 2),
        ("two below the bound, three new keys", bound - 2, 3),
    ];

    for (index, (case, held_count, new_keys)) in refused.iter().enumerate() {
        let user_name = format!("bound-refused-{index}");
        let held = fixture(*held_count);
        create_user(&router, &user_name, "/", &held).await;

        let list: Vec<(String, String)> = (0..*new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, body) = tag_user(&router, &user_name, &list).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 past the tag bound, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // Nothing of the refused request was stored.
        let (reported, keys) = stored_tags(&router, &user_name).await;
        assert_eq!(
            reported,
            expected_map(&held),
            "case {case:?}: a refused request must store nothing, got {reported:?}"
        );
        assert_eq!(
            keys, *held_count,
            "case {case:?}: expected {held_count} stored tags, got {keys}"
        );
    }
}
