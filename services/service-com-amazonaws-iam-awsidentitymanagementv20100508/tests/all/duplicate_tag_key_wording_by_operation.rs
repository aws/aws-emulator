// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence IAM refuses a repeated tag key with, operation by operation, end to end
//! over the awsQuery wire.
//!
//! RULE under test: a tag list naming one key more than once is refused with the modelled
//! `InvalidInputException` — HTTP 400, awsQuery code `InvalidInput` — and the sentence that
//! refusal carries belongs to the operation that took the list: a list supplied for a
//! PRINCIPAL's tags (a user's or a role's, whether by its `Tag*` call or by the `Create*`
//! call that mints it) is refused with the class sentence followed by IAM's note about how it
//! reads a tag key, while a list supplied for any OTHER IAM resource's tags (a policy, an
//! instance profile, a virtual MFA device, an identity provider) is refused with that class
//! sentence alone. Both sentences are fixed text: neither carries the repeated key, the
//! values, the count or the resource addressed. WHICH keys are one key follows the same
//! split — a principal reads a tag key without case, every other resource by its text — so
//! the note the principal sentence carries is the comparison its own half makes.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation takes the list and which kind of resource's tags it is for (both halves of the
//! rule, four operations of the principal half and eight of the resource half), whether the
//! call writes onto a held resource or mints one, where the repetition sits in the list, how
//! long the list is, and whether the two values of the repeated key differ. The two
//! operations an issue happened to report are two of these rows, not the specification.
//!
//! The expectations are the LITERAL sentences, written out here rather than read from the
//! service's own constants, which cannot tell a wrong sentence from a right one. The
//! complementary suites are `tests/all/supplied_tag_lists.rs` (WHICH lists are refused, and
//! that a refused list stores nothing, across the operations that take one) and
//! `tests/all/reserved_tag_key_wording.rs` (the reserved-prefix sentence, which is one
//! sentence for every operation).
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The sentence IAM answers a repeated tag key with when the list was supplied for one of
/// its PRINCIPALS' tags.
const PRINCIPAL_SENTENCE: &str =
    "Duplicate tag keys found. Please note that Tag keys are case insensitive.";

/// The sentence IAM answers the same list with when it was supplied for any other IAM
/// resource's tags.
const RESOURCE_SENTENCE: &str = "Duplicate tag keys found.";

/// A permission document a managed policy can carry. None of these assertions is about it.
const SOME_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;

/// A trust policy `CreateRole` accepts: the account's own principal, as the role suites of
/// this crate create a fixture role with.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        account_id()
    )
}

/// Which operation a row drives, and therefore whose tags the list it carries is for.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// `TagUser` onto a held user.
    TagUser,
    /// `TagRole` onto a held role.
    TagRole,
    /// `CreateUser` carrying the list with the create.
    CreateUser,
    /// `CreateRole` carrying the list with the create.
    CreateRole,
    /// `TagPolicy` onto a held customer managed policy.
    TagPolicy,
    /// `TagInstanceProfile` onto a held instance profile.
    TagInstanceProfile,
    /// `TagMFADevice` onto a held virtual MFA device.
    TagMfaDevice,
    /// `TagOpenIDConnectProvider` onto a held identity provider.
    TagOpenIdConnectProvider,
    /// `CreatePolicy` carrying the list with the create.
    CreatePolicy,
    /// `CreateInstanceProfile` carrying the list with the create.
    CreateInstanceProfile,
    /// `CreateVirtualMFADevice` carrying the list with the create.
    CreateVirtualMfaDevice,
    /// `CreateOpenIDConnectProvider` carrying the list with the create.
    CreateOpenIdConnectProvider,
}

impl Target {
    /// Every target, so one table spans both halves of the rule and, within each, a `Tag*`
    /// call writing onto a held resource beside a `Create*` call minting one.
    const ALL: &'static [Self] = &[
        Self::TagUser,
        Self::TagRole,
        Self::CreateUser,
        Self::CreateRole,
        Self::TagPolicy,
        Self::TagInstanceProfile,
        Self::TagMfaDevice,
        Self::TagOpenIdConnectProvider,
        Self::CreatePolicy,
        Self::CreateInstanceProfile,
        Self::CreateVirtualMfaDevice,
        Self::CreateOpenIdConnectProvider,
    ];

    /// The awsQuery `Action` the row's refusal comes from.
    fn action(self) -> &'static str {
        match self {
            Self::TagUser => "TagUser",
            Self::TagRole => "TagRole",
            Self::CreateUser => "CreateUser",
            Self::CreateRole => "CreateRole",
            Self::TagPolicy => "TagPolicy",
            Self::TagInstanceProfile => "TagInstanceProfile",
            Self::TagMfaDevice => "TagMFADevice",
            Self::TagOpenIdConnectProvider => "TagOpenIDConnectProvider",
            Self::CreatePolicy => "CreatePolicy",
            Self::CreateInstanceProfile => "CreateInstanceProfile",
            Self::CreateVirtualMfaDevice => "CreateVirtualMFADevice",
            Self::CreateOpenIdConnectProvider => "CreateOpenIDConnectProvider",
        }
    }

    /// The sentence this operation words the repeated-key refusal with: the principal form
    /// for a user's or a role's tags, the resource form for every other resource's.
    fn expected_sentence(self) -> &'static str {
        match self {
            Self::TagUser | Self::TagRole | Self::CreateUser | Self::CreateRole => {
                PRINCIPAL_SENTENCE
            }
            _ => RESOURCE_SENTENCE,
        }
    }

    /// Whether the list this operation takes is one of a PRINCIPAL's tag lists, which is
    /// what decides whether two spellings of one word are one key in it.
    fn is_principal(self) -> bool {
        self.expected_sentence() == PRINCIPAL_SENTENCE
    }
}

/// Create whatever the row's target needs to exist before the call, and answer the
/// parameters addressing it — the held resource for a `Tag*` row, or the names and documents
/// a `Create*` row carries.
async fn addressed(
    router: &MultiServiceRouter,
    target: Target,
    name: &str,
) -> Vec<(String, String)> {
    match target {
        Target::TagUser => {
            expect_ok(router, "CreateUser", &[("UserName", name)]).await;
            vec![("UserName".to_string(), name.to_string())]
        }
        Target::TagRole => {
            expect_ok(
                router,
                "CreateRole",
                &[
                    ("RoleName", name),
                    ("AssumeRolePolicyDocument", &trust_policy()),
                ],
            )
            .await;
            vec![("RoleName".to_string(), name.to_string())]
        }
        Target::CreateUser => vec![("UserName".to_string(), name.to_string())],
        Target::CreateRole => vec![
            ("RoleName".to_string(), name.to_string()),
            ("AssumeRolePolicyDocument".to_string(), trust_policy()),
        ],
        Target::TagPolicy => {
            let created = expect_ok(
                router,
                "CreatePolicy",
                &[("PolicyName", name), ("PolicyDocument", SOME_DOCUMENT)],
            )
            .await;
            vec![("PolicyArn".to_string(), element(&created, "Arn"))]
        }
        Target::TagInstanceProfile => {
            expect_ok(
                router,
                "CreateInstanceProfile",
                &[("InstanceProfileName", name)],
            )
            .await;
            vec![("InstanceProfileName".to_string(), name.to_string())]
        }
        Target::TagMfaDevice => {
            let created = expect_ok(
                router,
                "CreateVirtualMFADevice",
                &[("VirtualMFADeviceName", name)],
            )
            .await;
            vec![(
                "SerialNumber".to_string(),
                element(&created, "SerialNumber"),
            )]
        }
        Target::TagOpenIdConnectProvider => {
            let created = expect_ok(
                router,
                "CreateOpenIDConnectProvider",
                &[("Url", &provider_url(name))],
            )
            .await;
            vec![(
                "OpenIDConnectProviderArn".to_string(),
                element(&created, "OpenIDConnectProviderArn"),
            )]
        }
        Target::CreatePolicy => vec![
            ("PolicyName".to_string(), name.to_string()),
            ("PolicyDocument".to_string(), SOME_DOCUMENT.to_string()),
        ],
        Target::CreateInstanceProfile => {
            vec![("InstanceProfileName".to_string(), name.to_string())]
        }
        Target::CreateVirtualMfaDevice => {
            vec![("VirtualMFADeviceName".to_string(), name.to_string())]
        }
        Target::CreateOpenIdConnectProvider => {
            vec![("Url".to_string(), provider_url(name))]
        }
    }
}

/// The issuer URL a row's identity provider is created under — one per row, because a
/// provider is held under its URL.
fn provider_url(name: &str) -> String {
    format!("https://{name}.example.org")
}

/// One list shape a row supplies: the label it is named by in every assertion, the tags in
/// request order, and whether a PRINCIPAL reads the list as naming one key twice (every
/// surface refuses a key repeated in the same spelling; only a principal refuses two
/// spellings of one word, reading a key without case).
struct ListShape {
    case: &'static str,
    tags: &'static [(&'static str, &'static str)],
    repeated_for_a_principal: bool,
}

/// The list shapes every target is driven with: the repetition first, in the middle and
/// last, a list of two and a list of four, one key named three times, two keys each named
/// twice, and a repetition whose two values are EQUAL as well as ones whose values differ.
const REPEATED: &[ListShape] = &[
    ListShape {
        case: "the only key, named twice with different values",
        tags: &[("env", "prod"), ("env", "dev")],
        repeated_for_a_principal: true,
    },
    ListShape {
        case: "the only key, named twice with the same value",
        tags: &[("env", "prod"), ("env", "prod")],
        repeated_for_a_principal: true,
    },
    ListShape {
        case: "the repetition last in a list of four",
        tags: &[
            ("owner", "sre"),
            ("env", "prod"),
            ("team", "storage"),
            ("env", "dev"),
        ],
        repeated_for_a_principal: true,
    },
    ListShape {
        case: "the repetition first, and one key named three times",
        tags: &[("env", "a"), ("env", "b"), ("env", "c"), ("owner", "sre")],
        repeated_for_a_principal: true,
    },
    ListShape {
        case: "two keys each named twice",
        tags: &[("env", "a"), ("team", "x"), ("env", "b"), ("team", "y")],
        repeated_for_a_principal: true,
    },
];

/// The list shapes the control is driven with: distinct keys, and two spellings of one word,
/// which are two keys of any resource but ONE key of a user or a role — tag "key–value pairs
/// are not case sensitive, but case is preserved" (`AssumeRoleRequest$Tags`, the STS model,
/// of a principal's tags), which is what the principal sentence notes.
const DISTINCT: &[ListShape] = &[
    ListShape {
        case: "two distinct keys",
        tags: &[("env", "prod"), ("owner", "sre")],
        repeated_for_a_principal: false,
    },
    ListShape {
        case: "three spellings of one word",
        tags: &[("Env", "a"), ("env", "b"), ("ENV", "c")],
        repeated_for_a_principal: true,
    },
];

/// RULE over every (operation, repeated list) pair: HTTP 400 `InvalidInput` carrying the
/// sentence that operation words the class with, with nothing of the request in it.
#[tokio::test]
async fn the_repeated_key_refusal_is_worded_by_the_operation() {
    let router = router();

    for (row, target) in Target::ALL.iter().enumerate() {
        for (column, shape) in REPEATED.iter().enumerate() {
            let case = format!("{} refusing {}", target.action(), shape.case);
            let name = format!("dup-{row}-{column}");
            let mut params = addressed(&router, *target, &name).await;
            params.extend(tag_parameters(shape.tags));
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();

            let (status, body) = call(&router, target.action(), &borrowed).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a repeated tag key, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                target.expected_sentence(),
                "case {case:?}: expected {:?}, got {message:?}",
                target.expected_sentence()
            );
        }
    }
}

/// RULE's control: a list of keys that are DISTINCT on the surface it is supplied to is
/// served by the same operations, so what the refusals above answer is the repetition and
/// not the shape of the request — and the two sentences differ only in which operation took
/// the list, never in whether it was refused. A list of two spellings of one word is
/// distinct for every resource but a principal, which reads such a list as a repeated key
/// and refuses it with its own sentence.
#[tokio::test]
async fn a_list_of_distinct_keys_is_served_by_every_one_of_them() {
    let router = router();

    for (row, target) in Target::ALL.iter().enumerate() {
        for (column, shape) in DISTINCT.iter().enumerate() {
            let refused = target.is_principal() && shape.repeated_for_a_principal;
            let case = format!(
                "{} {} {}",
                target.action(),
                if refused { "refusing" } else { "serving" },
                shape.case
            );
            let name = format!("ok-{row}-{column}");
            let mut params = addressed(&router, *target, &name).await;
            params.extend(tag_parameters(shape.tags));
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();

            let (status, body) = call(&router, target.action(), &borrowed).await;
            if refused {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a principal to read these as one key, got \
                     {status}: {body}"
                );
                let message = element(&body, "Message");
                assert_eq!(
                    message, PRINCIPAL_SENTENCE,
                    "case {case:?}: expected {PRINCIPAL_SENTENCE:?}, got {message:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the distinct keys to be served, got {status}: \
                     {body}"
                );
            }
        }
    }
}

/// RULE: the refusal is decided before anything is written, so the resource the refused call
/// addressed holds exactly what it held — for a resource-worded operation as much as for a
/// principal-worded one.
#[tokio::test]
async fn a_refused_list_leaves_the_addressed_resource_as_it_was() {
    let router = router();

    // A policy and an instance profile, each already carrying one tag, plus a user for the
    // principal-worded half of the rule.
    let created = expect_ok(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "untouched-policy"),
            ("PolicyDocument", SOME_DOCUMENT),
            ("Tags.member.1.Key", "owner"),
            ("Tags.member.1.Value", "sre"),
        ],
    )
    .await;
    let policy_arn = element(&created, "Arn");
    expect_ok(
        &router,
        "CreateInstanceProfile",
        &[
            ("InstanceProfileName", "untouched-profile"),
            ("Tags.member.1.Key", "owner"),
            ("Tags.member.1.Value", "sre"),
        ],
    )
    .await;
    expect_ok(
        &router,
        "CreateUser",
        &[
            ("UserName", "untouched-user"),
            ("Tags.member.1.Key", "owner"),
            ("Tags.member.1.Value", "sre"),
        ],
    )
    .await;

    let rows = [
        Untouched {
            action: "TagPolicy",
            addressing: ("PolicyArn", policy_arn.as_str()),
            listing: "ListPolicyTags",
        },
        Untouched {
            action: "TagInstanceProfile",
            addressing: ("InstanceProfileName", "untouched-profile"),
            listing: "ListInstanceProfileTags",
        },
        Untouched {
            action: "TagUser",
            addressing: ("UserName", "untouched-user"),
            listing: "ListUserTags",
        },
    ];

    for Untouched {
        action,
        addressing,
        listing,
    } in rows
    {
        let mut params = vec![(addressing.0.to_string(), addressing.1.to_string())];
        params.extend(tag_parameters(&[("repeated", "a"), ("repeated", "b")]));
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let (status, body) = call(&router, action, &borrowed).await;
        assert_eq!(
            status, 400,
            "case {action:?}: expected the repeated key to be refused, got {status}: {body}"
        );

        let (status, listed) = call(&router, listing, &[addressing]).await;
        assert_eq!(
            status, 200,
            "case {action:?}: {listing} must be served, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "Key"),
            vec!["owner".to_string()],
            "case {action:?}: a refused tag call must leave the held set alone, got {listed}"
        );
    }
}

/// One row of the "nothing was written" table: the `Tag*` call the repeated key is supplied
/// to, the single parameter addressing its resource, and the listing a later caller reads
/// that resource's tags back through.
struct Untouched<'a> {
    action: &'a str,
    addressing: (&'a str, &'a str),
    listing: &'a str,
}

/// Make one call that must succeed for a row to be set up at all, answering its body.
async fn expect_ok(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "the fixture {action} must succeed, got {status}: {body}"
    );
    body
}

/// The `Tags.member.N.{Key,Value}` parameters for `tags`, as the query protocol flattens a
/// tag list.
fn tag_parameters(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(at, (key, value))| {
            [
                (format!("Tags.member.{}.Key", at + 1), (*key).to_string()),
                (format!("Tags.member.{}.Value", at + 1), (*value).to_string()),
            ]
        })
        .collect()
}
