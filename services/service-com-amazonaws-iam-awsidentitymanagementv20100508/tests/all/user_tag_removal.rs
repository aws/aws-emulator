// SPDX-License-Identifier: CC0-1.0

//! `UntagUser`, end to end over the awsQuery wire, read back through `GetUser` and
//! `ListUserTags`.
//!
//! RULE under test: an untag call drops from the resource's stored tag set exactly
//! the tags whose keys it names, comparing a key as the tagged resource's own
//! surface reads one — a user is one of IAM's principals, whose tag keys read
//! without case — and leaves every other
//! held tag with its value — so a named key the resource does not hold removes
//! nothing and the call still succeeds, which makes it idempotent, and the keys it
//! did remove are free to be tagged again. `UntagUser`'s own documentation:
//! "removes the specified tags from the user", with "the tags with matching keys
//! are removed from the specified user" (`UntagUser$TagKeys`), and the comparison
//! itself is `tests/all/principal_tag_key_case.rs`'s rule.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour of a user that was PERSISTED rather than a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns, the entity-independent tag builders are `tests/all/role_tags.rs`'s, the
//! entity-independent tag-key-list builders and quota fixture are
//! `tests/all/role_tag_removal.rs`'s, and the user fixtures and the `GetUser` tag
//! reader are `tests/all/user_tags.rs`'s, so a request is built and read exactly one
//! way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over:
//! the held tag count, how many keys one call names, whether a named key is held or
//! not, the case of a key, the user's path, the case the user name is spelled in,
//! the number of calls, the alphabet and length bounds `tagKeyType` publishes
//! (1..128), and the published per-resource tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{expected_map, tags};
use crate::user_lifecycle::{call, elements, error_code, router};
use crate::user_tags::{create_user, stored_tags, tag_user};

/// One `UntagUser` call: the HTTP status and the XML body.
async fn untag_user(
    router: &MultiServiceRouter,
    user_name: &str,
    keys: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("UserName", user_name)], keys);
    call(router, "UntagUser", &borrowed(&params)).await
}

/// The tag set `ListUserTags` reports for `user_name`, as a map — the second reader
/// of the same persisted set, so neither reader is the only witness of a removal.
async fn listed_tags(
    router: &MultiServiceRouter,
    user_name: &str,
) -> (BTreeMap<String, String>, usize) {
    let (status, body) = call(router, "ListUserTags", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: ListUserTags must report the user's tags, got {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key").len())
}

/// The published maximum length of `tagKeyListType`, the list `UntagUser$TagKeys`
/// targets (`smithy_query(query="operation", operation="UntagUser")`: len 0..50).
/// Stated separately from `MAX_TAGS_PER_ENTITY`, the per-resource tag quota, so a
/// change to either one does not quietly stop this suite probing the other.
const MAX_TAG_KEYS_PER_CALL: usize = 50;

/// One row: a user created under `path` with `initial`, then one `UntagUser` naming
/// `remove`, then the tag set the user must hold.
struct RemoveCase {
    name: &'static str,
    path: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagUser` drops exactly the held tags whose keys the request names and
/// leaves every other held tag untouched, so `GetUser` and `ListUserTags` both
/// report the remaining set. One row per thing the rule generalises over: the held
/// tag count, how many keys one call names, held versus unheld keys, the case of a
/// key, the empty tag value, the user's path, the published key-length bound and
/// alphabet, and the per-resource tag quota.
#[tokio::test]
async fn untag_user_removes_exactly_the_named_keys() {
    // `tagKeyType` is length 1..128 (`smithy_query(query="shape", shape="Tag")`),
    // so this is the longest key a stored tag can be named by.
    let longest_key = "k".repeat(128);
    // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so spaces,
    // non-ASCII letters, digits and that class's punctuation all name tags.
    let alphabet_key = "Cost Center_1.2:3/4=5+6-7@8";
    let quota = MAX_TAGS_PER_ENTITY;

    let cases = vec![
        RemoveCase {
            name: "the only held tag is removed, leaving none",
            path: "/",
            initial: tags(&[("team", "storage")]),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            path: "/eng/",
            initial: tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage"), ("owner", "dlm")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            path: "/eng/tools/",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            path: "/",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the user does not hold removes nothing",
            path: "/sales/west/",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "storage"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            path: "/",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            // Tag "key–value pairs are not case sensitive, but case is preserved"
            // (`AssumeRoleRequest$Tags`, the STS model, of a principal's tags), so a
            // key named in any case names the held tag of a user.
            name: "a key named in another case removes the held tag",
            path: "/",
            initial: tags(&[("Env", "prod"), ("team", "storage")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "several spellings of one held key remove it once",
            path: "/eng/",
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            path: "/",
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            path: "/",
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "storage")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            path: "/eng/",
            initial: tags(&[("k", "short"), ("team", "storage")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            path: "/",
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a user at the tag quota",
            path: "/",
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50, and the quota is 50 tags per
            // resource, so this names the most keys one call can carry.
            name: "every key of a user at the tag quota removed in one call",
            path: "/eng/",
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let user_name = format!("remove-{index}");
        create_user(&router, &user_name, case.path, &case.initial).await;

        let (status, body) = untag_user(&router, &user_name, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagUser must succeed, got {status}: {body}",
            case.name
        );

        let expected = expected_map(&case.expected);
        let (reported, key_count) = stored_tags(&router, &user_name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the remaining tag set {expected:?}, got {reported:?}",
            case.name
        );
        assert_eq!(
            key_count,
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {key_count}",
            case.name,
            case.expected.len()
        );

        // The other reader of the same persisted set agrees, so the removal reached
        // the stored user rather than one report of it.
        let (listed, listed_count) = listed_tags(&router, &user_name).await;
        assert_eq!(
            listed, expected,
            "case {:?}: ListUserTags must report {expected:?}, got {listed:?}",
            case.name
        );
        assert_eq!(
            listed_count,
            case.expected.len(),
            "case {:?}: expected ListUserTags to report {} tags, got {listed_count}",
            case.name,
            case.expected.len()
        );
    }
}

/// RULE: removing a key the resource does not hold removes nothing and is not an
/// error, so repeating an untag call leaves the same tag set however many times it
/// is made.
#[tokio::test]
async fn repeating_an_untag_call_changes_nothing_further() {
    let held = tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]);
    let repeated = [
        (
            "one held key",
            keys(&["env"]),
            tags(&[("team", "storage"), ("owner", "dlm")]),
        ),
        (
            "two held keys",
            keys(&["env", "owner"]),
            tags(&[("team", "storage")]),
        ),
        ("only unheld keys", keys(&["absent", "other"]), held.clone()),
        ("every held key", keys(&["team", "env", "owner"]), Vec::new()),
    ];

    let router = router();
    for (index, (case, removed, expected)) in repeated.iter().enumerate() {
        let user_name = format!("repeat-{index}");
        create_user(&router, &user_name, "/", &held).await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_user(&router, &user_name, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagUser attempt {attempt} must succeed, got {status}: {body}"
            );
            let (reported, key_count) = stored_tags(&router, &user_name).await;
            assert_eq!(
                reported,
                expected_map(expected),
                "case {case:?}: after attempt {attempt} expected {:?}, got {reported:?}",
                expected_map(expected)
            );
            assert_eq!(
                key_count,
                expected.len(),
                "case {case:?}: after attempt {attempt} expected {} stored tags, got {key_count}",
                expected.len()
            );
        }
    }
}

/// RULE: an untag call names its user by an IAM entity name, which is unique within
/// the account and "not distinguished by case" (`CreateUser$UserName`), so any
/// casing of a held name untags the SAME user — and a name the account does not hold
/// is refused with the modelled `NoSuchEntityException`.
#[tokio::test]
async fn the_user_is_found_by_its_case_folded_name() {
    let router = router();
    let held: Vec<(String, String)> = (0..4)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    create_user(&router, "MyUser", "/eng/", &held).await;

    // Each spelling untags the one held user, so the set shrinks across them
    // instead of each spelling getting a user of its own.
    for (index, spelling) in ["MyUser", "myuser", "MYUSER", "mYuSeR"].iter().enumerate() {
        let (status, body) = untag_user(&router, spelling, &keys(&[&format!("key-{index}")])).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200 for a held name, got {status}: {body}"
        );
        let (reported, key_count) = stored_tags(&router, spelling).await;
        let remaining = held.len() - index - 1;
        assert_eq!(
            key_count, remaining,
            "case {spelling:?}: expected {remaining} stored tags, got {key_count}: {reported:?}"
        );
        assert!(
            !reported.contains_key(&format!("key-{index}")),
            "case {spelling:?}: the removed key must be gone from the held user, got {reported:?}"
        );
    }

    for absent in ["OtherUser", "myuser2", "my_user", "user"] {
        let (status, body) = untag_user(&router, absent, &keys(&["team"])).await;
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

/// One row: a user created with `initial`, then an `UntagUser` naming `removed`,
/// then a `TagUser` supplying `re_tagged`.
struct ReTagCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
    re_tagged: Vec<(String, String)>,
}

/// RULE: an untagged key is simply not held any more, so it can be tagged again —
/// and because the per-resource tag quota is taken against the set the resource
/// actually holds, removing a tag frees room inside it.
#[tokio::test]
async fn a_removed_key_can_be_tagged_again() {
    let quota = MAX_TAGS_PER_ENTITY;

    // One row per starting point: a user holding one tag, one holding several, and
    // one at the published quota, where the re-tag only fits because a key was
    // removed.
    let rows = [
        ReTagCase {
            name: "a user holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a user holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a user at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let user_name = format!("retag-{index}");
        let name = case.name;
        create_user(&router, &user_name, "/", &case.initial).await;

        let (status, body) = untag_user(&router, &user_name, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagUser must succeed, got {status}: {body}"
        );

        let (status, body) = tag_user(&router, &user_name, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the user holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());

        let (reported, key_count) = stored_tags(&router, &user_name).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the tag set {expected:?}, got {reported:?}"
        );
        assert!(
            key_count <= quota,
            "case {name:?}: a user must never hold more than {quota} tags, got {key_count}"
        );
    }
}

/// RULE: `TagKeys` is a required member whose `tagKeyListType` admits at most 50
/// keys (`smithy_query(query="operation", operation="UntagUser")`), so a request
/// that names no key list at all, or more keys than that bound admits, is refused
/// before anything is removed — while a call naming exactly the bound succeeds.
#[tokio::test]
async fn the_published_key_list_bound_is_enforced_before_anything_is_removed() {
    // The published maximum length of `tagKeyListType`, the bound one call's
    // `TagKeys` is taken against
    // (`smithy_query(query="operation", operation="UntagUser")`). Stated separately
    // from the per-resource tag quota, which this list happens to equal today.
    let bound = MAX_TAG_KEYS_PER_CALL;
    // Hold as many tags as the largest accepted list can name, so the accepted call
    // at the bound names only keys the user really holds.
    let held = fixture(bound.min(MAX_TAGS_PER_ENTITY));

    let router = router();

    // At the bound: accepted, and every named key is gone.
    create_user(&router, "at-bound", "/", &held).await;
    let at_bound: Vec<String> = held.iter().map(|(key, _)| key.clone()).collect();
    let (status, body) = untag_user(&router, "at-bound", &at_bound).await;
    assert_eq!(
        status, 200,
        "case \"at the published bound\": expected 200 for {bound} keys, got {status}: {body}"
    );
    let (reported, key_count) = stored_tags(&router, "at-bound").await;
    assert_eq!(
        key_count, 0,
        "case \"at the published bound\": expected no stored tags, got {key_count}: {reported:?}"
    );

    // One past the bound: refused, and nothing it named was removed.
    create_user(&router, "past-bound", "/", &held).await;
    let past_bound: Vec<String> = (0..=bound).map(|index| format!("key-{index:02}")).collect();
    let (status, body) = untag_user(&router, "past-bound", &past_bound).await;
    assert_eq!(
        status, 400,
        "case \"one past the published bound\": expected 400 for {} keys, got {status}: {body}",
        past_bound.len()
    );
    let (reported, key_count) = stored_tags(&router, "past-bound").await;
    assert_eq!(
        reported,
        expected_map(&held),
        "case \"one past the published bound\": a refused request must remove nothing, got {reported:?}"
    );
    assert_eq!(
        key_count,
        held.len(),
        "case \"one past the published bound\": expected {} stored tags, got {key_count}",
        held.len()
    );

    // No key list at all: the member is required, so the request never reaches the
    // handler and the held set is untouched.
    create_user(&router, "no-keys", "/", &held).await;
    let (status, body) = call(&router, "UntagUser", &[("UserName", "no-keys")]).await;
    assert_eq!(
        status, 400,
        "case \"no key list at all\": expected 400 for a missing required member, got {status}: {body}"
    );
    let (reported, key_count) = stored_tags(&router, "no-keys").await;
    assert_eq!(
        key_count,
        held.len(),
        "case \"no key list at all\": expected {} stored tags, got {key_count}: {reported:?}",
        held.len()
    );
}

/// RULE: untagging one resource touches no other, so a key removed from one user
/// stays on every other user that holds it.
#[tokio::test]
async fn untagging_one_user_leaves_every_other_user_alone() {
    let held = tags(&[("team", "storage"), ("env", "prod")]);
    let names = ["first", "second", "third"];

    let router = router();
    for name in names {
        create_user(&router, name, "/eng/", &held).await;
    }

    let (status, body) = untag_user(&router, "second", &keys(&["team"])).await;
    assert_eq!(
        status, 200,
        "case \"second\": UntagUser must succeed, got {status}: {body}"
    );

    for name in names {
        let expected = if name == "second" {
            expected_map(&tags(&[("env", "prod")]))
        } else {
            expected_map(&held)
        };
        let (reported, key_count) = stored_tags(&router, name).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            key_count,
            expected.len(),
            "case {name:?}: expected {} stored tags, got {key_count}",
            expected.len()
        );
    }
}
