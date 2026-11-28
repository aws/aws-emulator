// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request that supplied a TAG LIST, and what the refusals of
//! the tagging path say, end to end over the awsQuery wire.
//!
//! The rules under test:
//!
//! 1. a tag list naming one key MORE THAN ONCE is ambiguous — a tag is a key and ONE value
//!    ("a tag consists of a key name and an associated value", `TagRole`) — so the whole
//!    request is refused HTTP 400 `InvalidInput` with the fixed sentence the operation words
//!    that class with (`tests/all/duplicate_tag_key_wording_by_operation.rs` pins which
//!    sentence that is) before anything is read or written, and nothing of it is stored.
//!    WHEN two of the keys are one key is the surface's own comparison: for any resource
//!    other than a user or a role the keys compare by their text, so two spellings of one
//!    word are two keys and such a list is taken, while a principal reads a key without
//!    case (`tests/all/principal_tag_key_case.rs`);
//! 2. a tag call that would carry a resource past the published per-resource tag quota is
//!    refused HTTP 409 `LimitExceeded` with the fixed sentence `The number of tags has
//!    reached the maximum limit.`, which names neither the resource nor the figure — and the
//!    quota is taken against the MERGED set, so re-tagging a key the resource already holds
//!    is served at the quota;
//! 3. a tag call addressed to a policy outside the account's own policy domain — an ARN in
//!    AWS's managed-policy namespace — is refused HTTP 400 `InvalidInput` with the fixed
//!    sentence `Tags not supported for policies in this domain`, whether the call was adding
//!    tags or removing them.
//!
//! Rule 1 is a property of the LIST, not of one operation, so its table spans the operations
//! that take one: the `Tag*` calls of four resource types (a user, a role, a policy, an
//! instance profile) and the `Create*` calls that attach tags as they mint a resource. The
//! rows differ along what the rule generalises over: which operation takes the list, where
//! the repetition sits in it, how long the list is, whether the two values differ, and
//! whether the resource already holds tags. Rule 2's rows differ along the resource type and
//! how many new keys the over-quota call names. No single request is the specification.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of state that was PERSISTED. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the tag-list encoding and the per-resource fixtures
//! are the ones `tests/all/role_tags.rs`, `tests/all/user_tags.rs`,
//! `tests/all/policy_tags.rs` and `tests/all/instance_profile_tags.rs` own, and the quota
//! fixture is `tests/all/role_tag_removal.rs`'s, so a request is built and read exactly one
//! way across this crate.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    DUPLICATE_TAG_KEY_MESSAGE, MAX_TAGS_PER_ENTITY, POLICY_TAGS_UNSUPPORTED_DOMAIN,
    PRINCIPAL_DUPLICATE_TAG_KEY_MESSAGE, TAG_LIMIT_EXCEEDED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_profile_tags::{create_instance_profile, tag_instance_profile};
use crate::role_lifecycle::{borrowed, param};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{create_role, expected_map, tag_role, tags, with_tag_params, TRUST_POLICY};
use crate::user_lifecycle::{call, element, elements, error_code, router};
use crate::user_tags::{create_user, tag_user};

/// A managed policy document with a single statement, the shape `CreatePolicy` documents.
/// None of these assertions is about the document.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]}"#;

/// An ARN in AWS's OWN managed-policy namespace (account segment `aws`), which is the domain
/// rule 3 is about. A second one is named below, so no single entry of the catalog is the
/// specification.
const AWS_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// Another AWS-managed policy ARN, filed under a PATH, and one the catalog of this emulator
/// does not carry at all: the refusal is about the domain the ARN names, not about whether
/// that policy is published.
const AWS_MANAGED_POLICY_UNDER_A_PATH: &str =
    "arn:aws:iam::aws:policy/service-role/AmazonEC2RoleforSSM";
const AWS_MANAGED_POLICY_NOT_PUBLISHED: &str = "arn:aws:iam::aws:policy/NoSuchAwsPolicy";

/// WHICH operation a rule-1 row supplies its tag list to. Each variant is an operation this
/// service takes a `Tags` list on: the `Tag*` call of a resource type, or the `Create*` call
/// that attaches tags as it mints one.
#[derive(Debug, Clone, Copy)]
enum SuppliedTo {
    TagUser,
    TagRole,
    TagPolicy,
    TagInstanceProfile,
    CreateUser,
    CreateRole,
    CreatePolicy,
    CreateInstanceProfile,
}

impl SuppliedTo {
    /// The operation's wire name, which a case label reads by.
    fn action(self) -> &'static str {
        match self {
            Self::TagUser => "TagUser",
            Self::TagRole => "TagRole",
            Self::TagPolicy => "TagPolicy",
            Self::TagInstanceProfile => "TagInstanceProfile",
            Self::CreateUser => "CreateUser",
            Self::CreateRole => "CreateRole",
            Self::CreatePolicy => "CreatePolicy",
            Self::CreateInstanceProfile => "CreateInstanceProfile",
        }
    }

    /// The sentence this operation words the repeated-key refusal with: IAM words that class
    /// per operation, with the case-insensitivity note for a principal's tags only.
    fn duplicate_message(self) -> &'static str {
        match self {
            Self::TagUser | Self::TagRole | Self::CreateUser | Self::CreateRole => {
                PRINCIPAL_DUPLICATE_TAG_KEY_MESSAGE
            }
            Self::TagPolicy
            | Self::TagInstanceProfile
            | Self::CreatePolicy
            | Self::CreateInstanceProfile => DUPLICATE_TAG_KEY_MESSAGE,
        }
    }

    /// Supply `list` to this operation, against a resource named for `label`, and answer the
    /// status and body. A `Tag*` row addresses a resource the account already HOLDS, carrying
    /// `held`; a `Create*` row mints one, so `held` is what the create would have attached
    /// beside the list.
    async fn supply(
        self,
        router: &MultiServiceRouter,
        label: &str,
        held: &[(String, String)],
        list: &[(String, String)],
    ) -> (u16, String) {
        match self {
            Self::TagUser => {
                create_user(router, label, "/", held).await;
                tag_user(router, label, list).await
            }
            Self::TagRole => {
                create_role(router, label, held).await;
                tag_role(router, label, list).await
            }
            Self::TagPolicy => {
                let arn = create_policy(router, label, held).await;
                tag_policy(router, &arn, list).await
            }
            Self::TagInstanceProfile => {
                create_instance_profile(router, label, held).await;
                tag_instance_profile(router, label, list).await
            }
            Self::CreateUser => {
                let params = with_tag_params(vec![param("UserName", label)], list);
                call(router, "CreateUser", &borrowed(&params)).await
            }
            Self::CreateRole => {
                let params = with_tag_params(
                    vec![
                        param("RoleName", label),
                        param("AssumeRolePolicyDocument", TRUST_POLICY),
                    ],
                    list,
                );
                call(router, "CreateRole", &borrowed(&params)).await
            }
            Self::CreatePolicy => {
                let params = with_tag_params(
                    vec![
                        param("PolicyName", label),
                        param("PolicyDocument", POLICY_DOCUMENT),
                    ],
                    list,
                );
                call(router, "CreatePolicy", &borrowed(&params)).await
            }
            Self::CreateInstanceProfile => {
                let params = with_tag_params(vec![param("InstanceProfileName", label)], list);
                call(router, "CreateInstanceProfile", &borrowed(&params)).await
            }
        }
    }

    /// The tag set a later caller sees for the resource `label` names, as a map, plus how
    /// many `<Key>` elements the read carried — so a duplicated key cannot hide inside a
    /// map. `None` for a row whose `Create*` was refused, where the account holds no such
    /// resource at all (which the read is asserted to report).
    async fn read_back(
        self,
        router: &MultiServiceRouter,
        label: &str,
    ) -> Option<(BTreeMap<String, String>, usize)> {
        match self {
            Self::TagUser | Self::CreateUser => {
                read(router, "GetUser", &[("UserName", label)]).await
            }
            Self::TagRole | Self::CreateRole => {
                read(router, "GetRole", &[("RoleName", label)]).await
            }
            Self::TagPolicy | Self::CreatePolicy => {
                read(router, "GetPolicy", &[("PolicyArn", &policy_arn(label))]).await
            }
            Self::TagInstanceProfile | Self::CreateInstanceProfile => {
                read(
                    router,
                    "GetInstanceProfile",
                    &[("InstanceProfileName", label)],
                )
                .await
            }
        }
    }
}

/// The tags a read reports, or `None` when the account holds no such resource (HTTP 404).
async fn read(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> Option<(BTreeMap<String, String>, usize)> {
    let (status, body) = call(router, action, params).await;
    if status == 404 {
        return None;
    }
    assert_eq!(
        status, 200,
        "the read {action} must be served or report no such entity, got {status}: {body}"
    );
    Some((
        crate::role_lifecycle::reported_tags(&body),
        elements(&body, "Key").len(),
    ))
}

/// The ARN a policy of this account named `policy_name` carries at the default path.
fn policy_arn(policy_name: &str) -> String {
    format!(
        "arn:aws:iam::{}:policy/{policy_name}",
        crate::user_lifecycle::account_id()
    )
}

/// Create the policy `policy_name` carrying `initial`, answering its ARN.
async fn create_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    initial: &[(String, String)],
) -> String {
    let params = with_tag_params(
        vec![
            param("PolicyName", policy_name),
            param("PolicyDocument", POLICY_DOCUMENT),
        ],
        initial,
    );
    let (status, body) = call(router, "CreatePolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One `TagPolicy` call.
async fn tag_policy(
    router: &MultiServiceRouter,
    policy_arn: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("PolicyArn", policy_arn)], list);
    call(router, "TagPolicy", &borrowed(&params)).await
}

/// One `UntagPolicy` call.
async fn untag_policy(
    router: &MultiServiceRouter,
    policy_arn: &str,
    named: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("PolicyArn", policy_arn)], named);
    call(router, "UntagPolicy", &borrowed(&params)).await
}

/// One row of the duplicate-key table: the operation the list is supplied to, what the
/// resource already holds, and the list itself.
struct DuplicateCase {
    case: &'static str,
    supplied_to: SuppliedTo,
    held: Vec<(String, String)>,
    list: Vec<(String, String)>,
}

/// The rows: every operation that takes a tag list, with the repetition in each position a
/// list can carry it, lists of several lengths, two values that differ and two that do not,
/// and resources that hold tags already as well as resources that hold none.
fn duplicate_cases() -> Vec<DuplicateCase> {
    vec![
        DuplicateCase {
            case: "the only key, named twice with different values",
            supplied_to: SuppliedTo::TagPolicy,
            held: Vec::new(),
            list: tags(&[("k", "a"), ("k", "b")]),
        },
        DuplicateCase {
            case: "the only key, named twice with the SAME value",
            supplied_to: SuppliedTo::TagInstanceProfile,
            held: Vec::new(),
            list: tags(&[("k", "a"), ("k", "a")]),
        },
        DuplicateCase {
            case: "the repetition last in a longer list",
            supplied_to: SuppliedTo::TagUser,
            held: tags(&[("team", "storage")]),
            list: tags(&[("env", "prod"), ("owner", "dlm"), ("env", "staging")]),
        },
        DuplicateCase {
            case: "the repetition first in a longer list",
            supplied_to: SuppliedTo::TagRole,
            held: tags(&[("team", "storage"), ("env", "prod")]),
            list: tags(&[("env", "dev"), ("env", "test"), ("owner", "dlm")]),
        },
        DuplicateCase {
            case: "one key named three times",
            supplied_to: SuppliedTo::TagUser,
            held: Vec::new(),
            list: tags(&[("env", "a"), ("env", "b"), ("env", "c")]),
        },
        DuplicateCase {
            case: "two keys each named twice",
            supplied_to: SuppliedTo::TagRole,
            held: Vec::new(),
            list: tags(&[("env", "a"), ("team", "x"), ("env", "b"), ("team", "y")]),
        },
        // The `Create*` calls take the same list, so the same rule decides them: a refused
        // create leaves the account holding NO such resource.
        DuplicateCase {
            case: "a create carrying the repetition",
            supplied_to: SuppliedTo::CreateUser,
            held: Vec::new(),
            list: tags(&[("env", "prod"), ("env", "dev")]),
        },
        DuplicateCase {
            case: "a role create carrying the repetition",
            supplied_to: SuppliedTo::CreateRole,
            held: Vec::new(),
            list: tags(&[("team", "x"), ("team", "y"), ("env", "prod")]),
        },
        DuplicateCase {
            case: "a policy create carrying the repetition",
            supplied_to: SuppliedTo::CreatePolicy,
            held: Vec::new(),
            list: tags(&[("env", "prod"), ("env", "prod")]),
        },
        DuplicateCase {
            case: "a profile create carrying the repetition",
            supplied_to: SuppliedTo::CreateInstanceProfile,
            held: Vec::new(),
            list: tags(&[("a", "1"), ("b", "2"), ("a", "3")]),
        },
    ]
}

/// RULE 1: a tag list naming one key more than once is refused 400 `InvalidInput` with AWS's
/// fixed sentence, and nothing of the request is stored — whichever operation took the list.
#[tokio::test]
async fn a_tag_list_naming_one_key_twice_is_refused_and_stores_nothing() {
    let router = router();

    for (index, DuplicateCase {
        case,
        supplied_to,
        held,
        list,
    }) in duplicate_cases().into_iter().enumerate()
    {
        let label = format!("dup-{index}");
        let case = format!("{case} on {}", supplied_to.action());
        let (status, refused) = supplied_to.supply(&router, &label, &held, &list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {refused}"
        );
        let expected_message = supplied_to.duplicate_message();
        assert_eq!(
            element(&refused, "Message"),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            element(&refused, "Message")
        );

        // Nothing of the refused request is in the store: a `Tag*` row's resource carries
        // exactly what it held, and a refused `Create*` minted no resource at all.
        match supplied_to.read_back(&router, &label).await {
            Some((reported, key_count)) => {
                assert_eq!(
                    (reported, key_count),
                    (expected_map(&held), held.len()),
                    "case {case:?}: a refused tag call must leave the held set alone"
                );
            }
            None => assert!(
                matches!(
                    supplied_to,
                    SuppliedTo::CreateUser
                        | SuppliedTo::CreateRole
                        | SuppliedTo::CreatePolicy
                        | SuppliedTo::CreateInstanceProfile
                ),
                "case {case:?}: the addressed resource must still be held"
            ),
        }
    }
}

/// RULE 1, the other side of the comparison: for a resource that is not one of IAM's
/// principals, keys differing only in CASE are different keys, so a list naming several
/// spellings of one word is taken and every one of them is stored. (A user's and a role's
/// tag keys read without case, which is `tests/all/principal_tag_key_case.rs`'s rule.)
#[tokio::test]
async fn keys_differing_only_in_case_are_not_a_repeated_key() {
    let router = router();
    let label = "case-distinct";
    let list = tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]);
    let (status, served) = SuppliedTo::TagPolicy
        .supply(&router, label, &[], &list)
        .await;
    assert_eq!(
        status, 200,
        "expected three spellings of one word to be taken, got {status}: {served}"
    );
    let read = SuppliedTo::TagPolicy
        .read_back(&router, label)
        .await
        .expect("the tagged policy");
    assert_eq!(
        read,
        (expected_map(&list), list.len()),
        "expected each spelling to be stored as its own tag"
    );
}

/// RULE 2: a tag call that would carry a resource past the per-resource quota is refused 409
/// `LimitExceeded` with AWS's fixed sentence, nothing is stored, and the quota is taken
/// against the MERGED set, so re-tagging a held key is served at the quota.
///
/// The rows differ along the resource type the quota is taken against and how many new keys
/// the over-quota call names.
#[tokio::test]
async fn the_quota_refusal_states_the_published_sentence_and_stores_nothing() {
    let router = router();
    let quota = MAX_TAGS_PER_ENTITY;

    for (index, (case, supplied_to, new_keys)) in [
        ("a user at the quota, one new key", SuppliedTo::TagUser, 1),
        ("a role at the quota, two new keys", SuppliedTo::TagRole, 2),
        (
            "a policy one below the quota, two new keys",
            SuppliedTo::TagPolicy,
            2,
        ),
        (
            "a profile two below the quota, three new keys",
            SuppliedTo::TagInstanceProfile,
            3,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        // The held set is as many tags as leaves the call one key past the quota.
        let held = fixture(quota + 1 - new_keys);
        let label = format!("quota-{index}");
        let list: Vec<(String, String)> = (0..new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, refused) = supplied_to.supply(&router, &label, &held, &list).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 past the quota, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {refused}"
        );
        assert_eq!(
            element(&refused, "Message"),
            TAG_LIMIT_EXCEEDED_MESSAGE,
            "case {case:?}: expected {TAG_LIMIT_EXCEEDED_MESSAGE:?}, got {:?}",
            element(&refused, "Message")
        );
        let read = supplied_to
            .read_back(&router, &label)
            .await
            .expect("the addressed resource");
        assert_eq!(
            read,
            (expected_map(&held), held.len()),
            "case {case:?}: a refused tag call must leave the held set alone"
        );

        // And re-tagging a key the resource already holds consumes no quota, so it is served
        // even when the resource sits AT the quota.
        if read.1 == quota {
            let (held_key, _) = held.last().expect("a held tag").clone();
            let overwrite = tags(&[(held_key.as_str(), "overwritten")]);
            let (status, served) = match supplied_to {
                SuppliedTo::TagUser => tag_user(&router, &label, &overwrite).await,
                SuppliedTo::TagRole => tag_role(&router, &label, &overwrite).await,
                SuppliedTo::TagPolicy => {
                    tag_policy(&router, &policy_arn(&label), &overwrite).await
                }
                _ => tag_instance_profile(&router, &label, &overwrite).await,
            };
            assert_eq!(
                status, 200,
                "case {case:?}: re-tagging a held key must be served at the quota, got \
                 {status}: {served}"
            );
            let (reported, key_count) = supplied_to
                .read_back(&router, &label)
                .await
                .expect("the addressed resource");
            assert_eq!(
                (reported.get(&held_key).cloned(), key_count),
                (Some("overwritten".to_string()), quota),
                "case {case:?}: the overwrite must replace the held value in place"
            );
        }
    }
}

/// RULE 3: a tag call addressed to a policy in AWS's own managed-policy domain is refused
/// 400 `InvalidInput` with AWS's fixed sentence for the class, whether the call adds tags or
/// removes them, and whichever ARN of that domain it names.
#[tokio::test]
async fn tagging_a_policy_outside_the_account_s_domain_states_the_published_sentence() {
    let router = router();

    for arn in [
        AWS_MANAGED_POLICY,
        AWS_MANAGED_POLICY_UNDER_A_PATH,
        AWS_MANAGED_POLICY_NOT_PUBLISHED,
    ] {
        for (call_name, (status, refused)) in [
            (
                "TagPolicy",
                tag_policy(&router, arn, &tags(&[("env", "prod")])).await,
            ),
            (
                "UntagPolicy",
                untag_policy(&router, arn, &keys(&["env"])).await,
            ),
        ] {
            let case = format!("{call_name} on {arn}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidInput",
                "case {case:?}: expected the modelled awsQuery code, got {refused}"
            );
            assert_eq!(
                element(&refused, "Message"),
                POLICY_TAGS_UNSUPPORTED_DOMAIN,
                "case {case:?}: expected {POLICY_TAGS_UNSUPPORTED_DOMAIN:?}, got {:?}",
                element(&refused, "Message")
            );
        }
    }

    // The refusal is about the DOMAIN, not about tagging: a policy of this account takes the
    // same list.
    let arn = create_policy(&router, "own-domain-policy", &[]).await;
    let (status, served) = tag_policy(&router, &arn, &tags(&[("env", "prod")])).await;
    assert_eq!(
        status, 200,
        "a policy of this account must be tagged, got {status}: {served}"
    );
}
