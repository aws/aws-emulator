// SPDX-License-Identifier: CC0-1.0

//! WHICH tag of one of IAM's principals a tag key names, end to end over the awsQuery wire.
//!
//! RULE under test: a tag key identifies a tag of an IAM principal — a user or a role —
//! WITHOUT case, and the spelling the tag was first stored under is preserved. Tag
//! "key–value pairs are not case sensitive, but case is preserved" and "you cannot have
//! separate" keys differing only in case (`AssumeRoleRequest$Tags`, the STS model, of the
//! tags a role carries). So, for a user and for a role alike:
//!
//! * `TagUser`/`TagRole` supplying a key the principal already holds in ANOTHER case
//!   overwrites that tag's value in place — "if a tag with the same key name already
//!   exists, then that tag is overwritten with the new value" (`TagUser`) — leaving ONE tag,
//!   whose `Key` is the spelling the principal already held;
//! * `UntagUser`/`UntagRole` naming a held key in ANY case removes that tag — "the tags with
//!   matching keys are removed" (`UntagUser$TagKeys`);
//! * the per-resource tag quota is therefore consumed once per key so read, so re-tagging a
//!   held key in another case is served by a principal sitting AT the quota.
//!
//! The rows differ along what the rule generalises over: which principal carries the tags,
//! the case the key was stored in and the case it is named again in, how many calls the
//! re-tagging takes, how many keys the list carries, the alphabet the key is spelled with
//! (ASCII letters, letters outside ASCII, and the spaces and punctuation `tagKeyType`'s
//! pattern admits), and the principal's position relative to the published tag quota. No one
//! pair of spellings is the rule.
//!
//! The boundary of the rule is pinned too: the tag surface IAM added for its other resources
//! reads a tag key by its TEXT, which is why its own repeated-key refusal says nothing about
//! case (`tests/all/duplicate_tag_key_wording_by_operation.rs`), so a policy holds two
//! spellings of one word as two tags.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the HTTP
//! behaviour of a principal that was PERSISTED. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, and the fixtures and tag builders are the ones
//! `tests/all/user_tags.rs`, `tests/all/role_tags.rs`, `tests/all/role_tag_removal.rs` and
//! `tests/all/policy_tags.rs` own, so a request is built and read exactly one way across
//! this crate.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_tags::{create_policy, stored_tags as policy_stored_tags, tag_policy};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{
    create_role, expected_map, stored_tags as role_stored_tags, tag_role, tags,
};
use crate::user_lifecycle::{call, elements, error_code, router};
use crate::user_tags::{create_user, stored_tags as user_stored_tags, tag_user};

/// WHICH of IAM's principals a row's tags are carried by. Both halves of the rule's subject,
/// so no row's principal is the rule.
#[derive(Debug, Clone, Copy)]
enum Principal {
    User,
    Role,
}

impl Principal {
    /// Both principals IAM tags, so every table spans them.
    const ALL: [Self; 2] = [Self::User, Self::Role];

    /// Create the principal `name` already carrying `initial`.
    async fn create(self, router: &MultiServiceRouter, name: &str, initial: &[(String, String)]) {
        match self {
            Self::User => create_user(router, name, "/", initial).await,
            Self::Role => create_role(router, name, initial).await,
        }
    }

    /// One tag call against it: the HTTP status and the XML body.
    async fn tag(
        self,
        router: &MultiServiceRouter,
        name: &str,
        list: &[(String, String)],
    ) -> (u16, String) {
        match self {
            Self::User => tag_user(router, name, list).await,
            Self::Role => tag_role(router, name, list).await,
        }
    }

    /// One untag call against it: the HTTP status and the XML body.
    async fn untag(
        self,
        router: &MultiServiceRouter,
        name: &str,
        named: &[String],
    ) -> (u16, String) {
        let (action, addressing) = match self {
            Self::User => ("UntagUser", "UserName"),
            Self::Role => ("UntagRole", "RoleName"),
        };
        let params = with_key_params(vec![param(addressing, name)], named);
        call(router, action, &borrowed(&params)).await
    }

    /// The tag set the principal holds, as the two readers of it report it: the entity read
    /// (`GetUser`/`GetRole`) and the tag listing (`ListUserTags`/`ListRoleTags`), each as a
    /// map plus how many `<Key>` elements it carried — so a second tag cannot hide inside a
    /// map, and neither reader is the only witness.
    async fn held(
        self,
        router: &MultiServiceRouter,
        name: &str,
    ) -> ((BTreeMap<String, String>, usize), (BTreeMap<String, String>, usize)) {
        let (read, (action, addressing)) = match self {
            Self::User => (
                user_stored_tags(router, name).await,
                ("ListUserTags", "UserName"),
            ),
            Self::Role => (
                role_stored_tags(router, name).await,
                ("ListRoleTags", "RoleName"),
            ),
        };
        let (status, body) = call(router, action, &[(addressing, name)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: {action} must report the principal's tags, got {status}: {body}"
        );
        (
            read,
            (reported_tags(&body), elements(&body, "Key").len()),
        )
    }

    /// The label a case reads by.
    fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Role => "role",
        }
    }
}

/// One row: a principal created carrying `initial`, then `calls` successive tag requests,
/// then the ONE tag set both readers must report.
struct OverwriteCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: a key supplied in another case names the tag the principal already holds, so the
/// value is replaced in place, the HELD spelling stays, and no second tag appears.
#[tokio::test]
async fn a_key_in_another_case_overwrites_the_held_tag_in_place() {
    let cases = || {
        vec![
            OverwriteCase {
                name: "a lower-case key replaces the value of an upper-case held key",
                initial: tags(&[("Env", "prod")]),
                calls: vec![tags(&[("env", "dev")])],
                expected: tags(&[("Env", "dev")]),
            },
            OverwriteCase {
                name: "an upper-case key replaces the value of a lower-case held key",
                initial: tags(&[("env", "prod")]),
                calls: vec![tags(&[("ENV", "dev")])],
                expected: tags(&[("env", "dev")]),
            },
            OverwriteCase {
                name: "three spellings over three calls leave one tag at the last value",
                initial: Vec::new(),
                calls: vec![
                    tags(&[("Project", "one")]),
                    tags(&[("project", "two")]),
                    tags(&[("PROJECT", "three")]),
                ],
                expected: tags(&[("Project", "three")]),
            },
            OverwriteCase {
                name: "one call overwrites a held key in another case and adds a new one",
                initial: tags(&[("Team", "storage"), ("env", "prod")]),
                calls: vec![tags(&[("team", "platform"), ("owner", "iam")])],
                expected: tags(&[("Team", "platform"), ("env", "prod"), ("owner", "iam")]),
            },
            OverwriteCase {
                // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so a key is
                // spelled with any letter and not only an ASCII one: the comparison folds
                // those letters too.
                name: "a key spelled with letters outside ASCII",
                initial: tags(&[("Ünité", "a")]),
                calls: vec![tags(&[("ünité", "b")])],
                expected: tags(&[("Ünité", "b")]),
            },
            OverwriteCase {
                name: "a key carrying the spaces and punctuation the pattern admits",
                initial: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "a")]),
                calls: vec![tags(&[("cost center_1.2:3/4=5+6-7@8", "b")])],
                expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "b")]),
            },
            OverwriteCase {
                // The control: the SAME spelling overwrites, as it always did, so what the
                // rows above answer is the comparison and not the second call.
                name: "the same spelling overwrites the held value",
                initial: tags(&[("env", "prod")]),
                calls: vec![tags(&[("env", "staging")])],
                expected: tags(&[("env", "staging")]),
            },
            OverwriteCase {
                name: "a new key spelled like none of the held ones is added",
                initial: tags(&[("Env", "prod")]),
                calls: vec![tags(&[("environment", "dev")])],
                expected: tags(&[("Env", "prod"), ("environment", "dev")]),
            },
        ]
    };

    let router = router();
    for principal in Principal::ALL {
        for (index, case) in cases().into_iter().enumerate() {
            let name = format!("{}-overwrite-{index}", principal.label());
            let case_label = format!("{} on a {}", case.name, principal.label());
            principal.create(&router, &name, &case.initial).await;

            for (call_index, list) in case.calls.iter().enumerate() {
                let (status, body) = principal.tag(&router, &name, list).await;
                assert_eq!(
                    status, 200,
                    "case {case_label:?}: tag call {call_index} must be served, got \
                     {status}: {body}"
                );
            }

            let expected = expected_map(&case.expected);
            let (read, listed) = principal.held(&router, &name).await;
            assert_eq!(
                read,
                (expected.clone(), case.expected.len()),
                "case {case_label:?}: the entity read must report {expected:?}, got {read:?}"
            );
            assert_eq!(
                listed,
                (expected.clone(), case.expected.len()),
                "case {case_label:?}: the tag listing must report {expected:?}, got {listed:?}"
            );
        }
    }
}

/// One row: a principal created carrying `initial`, then one untag naming `remove`, then the
/// tag set both readers must report.
struct RemoveCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: an untag names a held tag by its key in ANY case, so a differently-cased key
/// removes it — while a key that is not that one in any case removes nothing.
#[tokio::test]
async fn an_untag_names_a_held_key_in_any_case() {
    let cases = || {
        vec![
            RemoveCase {
                name: "a lower-case key removes an upper-case held tag",
                initial: tags(&[("Env", "prod"), ("team", "storage")]),
                remove: keys(&["env"]),
                expected: tags(&[("team", "storage")]),
            },
            RemoveCase {
                name: "an upper-case key removes a lower-case held tag",
                initial: tags(&[("env", "prod")]),
                remove: keys(&["ENV"]),
                expected: Vec::new(),
            },
            RemoveCase {
                name: "a mixed-case key removes the held tag",
                initial: tags(&[("Project", "one"), ("Owner", "iam")]),
                remove: keys(&["pRoJeCt"]),
                expected: tags(&[("Owner", "iam")]),
            },
            RemoveCase {
                name: "several spellings of one held key remove it once",
                initial: tags(&[("Env", "prod"), ("team", "storage")]),
                remove: keys(&["env", "ENV", "Env"]),
                expected: tags(&[("team", "storage")]),
            },
            RemoveCase {
                name: "a key spelled with letters outside ASCII",
                initial: tags(&[("Ünité", "a")]),
                remove: keys(&["ünité"]),
                expected: Vec::new(),
            },
            RemoveCase {
                // The control: a key that is no spelling of a held one removes nothing, so
                // what the rows above answer is the comparison and not the call.
                name: "a key the principal does not hold removes nothing",
                initial: tags(&[("Env", "prod")]),
                remove: keys(&["environment"]),
                expected: tags(&[("Env", "prod")]),
            },
        ]
    };

    let router = router();
    for principal in Principal::ALL {
        for (index, case) in cases().into_iter().enumerate() {
            let name = format!("{}-remove-{index}", principal.label());
            let case_label = format!("{} on a {}", case.name, principal.label());
            principal.create(&router, &name, &case.initial).await;

            let (status, body) = principal.untag(&router, &name, &case.remove).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: the untag must be served, got {status}: {body}"
            );

            let expected = expected_map(&case.expected);
            let (read, listed) = principal.held(&router, &name).await;
            assert_eq!(
                read,
                (expected.clone(), case.expected.len()),
                "case {case_label:?}: the entity read must report {expected:?}, got {read:?}"
            );
            assert_eq!(
                listed,
                (expected.clone(), case.expected.len()),
                "case {case_label:?}: the tag listing must report {expected:?}, got {listed:?}"
            );
        }
    }
}

/// RULE: one key read without case is ONE tag of the quota, so a principal holding the
/// published maximum takes a re-tag of a held key spelled in another case — and is still
/// refused a key it does not hold at all.
#[tokio::test]
async fn a_key_read_without_case_consumes_one_tag_of_the_quota() {
    let router = router();
    let quota = MAX_TAGS_PER_ENTITY;

    for principal in Principal::ALL {
        let name = format!("{}-quota", principal.label());
        let held = fixture(quota);
        principal.create(&router, &name, &held).await;

        // The held keys are `key-00`..., so this names one of them in another case.
        let (held_key, _) = held.first().expect("a held tag").clone();
        let shouted = held_key.to_uppercase();
        let (status, body) = principal
            .tag(&router, &name, &tags(&[(shouted.as_str(), "overwritten")]))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: re-tagging a held key in another case must be served at the quota, \
             got {status}: {body}",
            principal.label()
        );

        let (read, listed) = principal.held(&router, &name).await;
        assert_eq!(
            (read.1, listed.1),
            (quota, quota),
            "case {:?}: the principal must still hold exactly {quota} tags",
            principal.label()
        );
        assert_eq!(
            read.0.get(&held_key).map(String::as_str),
            Some("overwritten"),
            "case {:?}: the held spelling must keep the key and take the new value, got {:?}",
            principal.label(),
            read.0.get(&held_key)
        );
        assert_eq!(
            read.0.get(&shouted),
            None,
            "case {:?}: no second tag may appear under the supplied spelling, got {:?}",
            principal.label(),
            read.0.get(&shouted)
        );

        // And a key that is no spelling of a held one is still past the quota.
        let (status, refused) = principal
            .tag(&router, &name, &tags(&[("brand-new", "value")]))
            .await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 past the tag quota, got {status}: {refused}",
            principal.label()
        );
        assert_eq!(
            error_code(&refused),
            "LimitExceeded",
            "case {:?}: expected the modelled awsQuery code, got {refused}",
            principal.label()
        );
    }
}

/// The BOUNDARY of the rule: the tag surface IAM added for its other resources reads a tag
/// key by its text, so a policy holds two spellings of one word as two tags and each keeps
/// its own value.
#[tokio::test]
async fn another_resource_s_tag_keys_are_still_read_by_their_text() {
    let router = router();
    let arn = create_policy(&router, "case-distinct-policy", None, &tags(&[("Env", "prod")])).await;

    let (status, body) = tag_policy(&router, &arn, &tags(&[("env", "dev")])).await;
    assert_eq!(
        status, 200,
        "expected the second spelling to be taken, got {status}: {body}"
    );

    let expected = expected_map(&tags(&[("Env", "prod"), ("env", "dev")]));
    let read = policy_stored_tags(&router, &arn).await;
    assert_eq!(
        read,
        (expected.clone(), 2),
        "expected the policy to hold both spellings {expected:?}, got {read:?}"
    );
}
