// SPDX-License-Identifier: CC0-1.0

//! `UntagRole`, end to end over the awsQuery wire, read back through `GetRole`.
//!
//! RULE under test: an untag call drops from the resource's stored tag set exactly
//! the tags whose keys it names, comparing a key as the tagged resource's own
//! surface reads one — a role is one of IAM's principals, whose tag keys read
//! without case — and leaves every other
//! held tag with its value — so a named key the resource does not hold removes
//! nothing and the call still succeeds, which makes it idempotent, and the keys it
//! did remove are free to be tagged again. `UntagRole`'s own documentation:
//! "removes the specified tags from the role", with "the tags with matching keys
//! are removed from the specified role" (`UntagRole$TagKeys`), and the comparison
//! itself is `tests/all/principal_tag_key_case.rs`'s rule.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour of a role that was PERSISTED rather than a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and the role and tag fixtures are `tests/all/role_tags.rs`'s, so a request
//! is built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over:
//! the held tag count, how many keys one call names, whether a named key is held or
//! not, the case of a key, the case of the role name, the number of calls, the
//! alphabet and length bounds `tagKeyType` publishes (1..128), and the published
//! per-resource tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::role_tags::{create_role, expected_map, stored_tags, tag_role, tags};
use crate::user_lifecycle::{error_code, router};

/// The `TagKeys.member.N` parameters of an awsQuery tag-key list, after the
/// parameters `leading` carries.
pub(crate) fn with_key_params(
    leading: Vec<(String, String)>,
    keys: &[String],
) -> Vec<(String, String)> {
    let mut params = leading;
    for (index, key) in keys.iter().enumerate() {
        params.push(param(&format!("TagKeys.member.{}", index + 1), key));
    }
    params
}

/// One `UntagRole` call: the HTTP status and the XML body.
async fn untag_role(
    router: &MultiServiceRouter,
    role_name: &str,
    keys: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("RoleName", role_name)], keys);
    crate::user_lifecycle::call(router, "UntagRole", &borrowed(&params)).await
}

/// `keys` as the owned list a table row names.
pub(crate) fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|key| key.to_string()).collect()
}

/// `count` distinct tags, the fixture the quota rows are built from.
pub(crate) fn fixture(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// One row: a role created with `initial`, then one `UntagRole` naming `remove`,
/// then the tag set the role must hold.
struct RemoveCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagRole` drops exactly the held tags whose keys the request names and
/// leaves every other held tag untouched, so `GetRole` reports the remaining set.
/// One row per thing the rule generalises over: the held tag count, how many keys
/// one call names, held versus unheld keys, the case of a key, the empty tag value,
/// the published key-length bound and alphabet, and the per-resource tag quota.
#[tokio::test]
async fn untag_role_removes_exactly_the_named_keys() {
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
            initial: tags(&[("team", "storage")]),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            initial: tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage"), ("owner", "dlm")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the role does not hold removes nothing",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "storage"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            // Tag "key–value pairs are not case sensitive, but case is preserved"
            // (`AssumeRoleRequest$Tags`, the STS model, of a principal's tags), so a
            // key named in any case names the held tag of a role.
            name: "a key named in another case removes the held tag",
            initial: tags(&[("Env", "prod"), ("team", "storage")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "several spellings of one held key remove it once",
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "storage")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            initial: tags(&[("k", "short"), ("team", "storage")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a role at the tag quota",
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50, and the quota is 50 tags per
            // resource, so this names the most keys one call can carry.
            name: "every key of a role at the tag quota removed in one call",
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let role_name = format!("remove-{index}");
        create_role(&router, &role_name, &case.initial).await;

        let (status, body) = untag_role(&router, &role_name, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagRole must succeed, got {status}: {body}",
            case.name
        );

        let (reported, key_count) = stored_tags(&router, &role_name).await;
        assert_eq!(
            reported,
            expected_map(&case.expected),
            "case {:?}: expected the remaining tag set {:?}, got {reported:?}",
            case.name,
            expected_map(&case.expected)
        );
        assert_eq!(
            key_count,
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {key_count}",
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
        ("one held key", keys(&["env"]), tags(&[("team", "storage"), ("owner", "dlm")])),
        (
            "two held keys",
            keys(&["env", "owner"]),
            tags(&[("team", "storage")]),
        ),
        ("only unheld keys", keys(&["absent", "other"]), held.clone()),
        (
            "every held key",
            keys(&["team", "env", "owner"]),
            Vec::new(),
        ),
    ];

    let router = router();
    for (index, (case, removed, expected)) in repeated.iter().enumerate() {
        let role_name = format!("repeat-{index}");
        create_role(&router, &role_name, &held).await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_role(&router, &role_name, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagRole attempt {attempt} must succeed, got {status}: {body}"
            );
            let (reported, key_count) = stored_tags(&router, &role_name).await;
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

/// RULE: an untag call names its role by an IAM entity name, which is unique within
/// the account and "not distinguished by case" (`CreateRole$RoleName`), so any
/// casing of a held name untags the SAME role — and a name the account does not
/// hold is refused with the modelled `NoSuchEntityException`.
#[tokio::test]
async fn the_role_is_found_by_its_case_folded_name() {
    let router = router();
    let held: Vec<(String, String)> = (0..4)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    create_role(&router, "MyRole", &held).await;

    // Each spelling untags the one held role, so the set shrinks across them
    // instead of each spelling getting a role of its own.
    for (index, spelling) in ["MyRole", "myrole", "MYROLE", "mYrOlE"].iter().enumerate() {
        let (status, body) = untag_role(&router, spelling, &keys(&[&format!("key-{index}")])).await;
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
            "case {spelling:?}: the removed key must be gone from the held role, got {reported:?}"
        );
    }

    for absent in ["OtherRole", "myrole2", "my_role", "role"] {
        let (status, body) = untag_role(&router, absent, &keys(&["team"])).await;
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

/// One row: a role created with `initial`, then an `UntagRole` naming `removed`,
/// then a `TagRole` supplying `re_tagged`.
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

    // One row per starting point: a role holding one tag, one holding several, and
    // one at the published quota, where the re-tag only fits because a key was
    // removed.
    let rows = [
        ReTagCase {
            name: "a role holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a role holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a role at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let role_name = format!("retag-{index}");
        let name = case.name;
        create_role(&router, &role_name, &case.initial).await;

        let (status, body) = untag_role(&router, &role_name, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagRole must succeed, got {status}: {body}"
        );

        let (status, body) = tag_role(&router, &role_name, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the role holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());

        let (reported, key_count) = stored_tags(&router, &role_name).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the tag set {expected:?}, got {reported:?}"
        );
        assert!(
            key_count <= quota,
            "case {name:?}: a role must never hold more than {quota} tags, got {key_count}"
        );
    }
}
