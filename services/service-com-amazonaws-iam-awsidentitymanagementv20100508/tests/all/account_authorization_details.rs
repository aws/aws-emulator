// SPDX-License-Identifier: CC0-1.0

//! `GetAccountAuthorizationDetails`, end to end over the awsQuery wire, cross-read
//! through the entities' own readers.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour (status, awsQuery error code, XML members) of entities that were
//! PERSISTED by `CreateUser`, `CreateGroup`, `CreateRole`, `CreatePolicy` and the
//! operations that attach policies to them — never a handler called directly. The
//! awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.
//!
//! The rule being asserted: this operation reports the account's whole authorization
//! configuration from persisted state — each user, group, role and managed policy with
//! the policies attached to it — selects which entity TYPES to include by `Filter`,
//! and pages over one enumeration of all four with `MaxItems` and a resumable
//! `Marker`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! entity type, the filter (absent, one type, several), the number of inline and
//! attached policies an entity carries (none, one, two), the path depth, the name's
//! case and alphabet, the policy's owner (the account or AWS) and the page size. No
//! single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// A managed-policy and inline-policy document with a single statement — the form
/// `CreatePolicy`'s own documentation describes. None of these assertions is about the
/// document itself, only about which document is reported where.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A second document, so a row carrying two inline policies reports two DIFFERENT
/// documents rather than one text twice.
const OTHER_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sqs:SendMessage","Resource":"*"}]}"#;

/// A trust policy with a single statement — every role needs one to exist at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture users: a name spanning the `^[\w+=,.@-]+$` alphabet and a nested path,
/// beside a plain name at the account root that carries NOTHING — so the rule is
/// exercised on an entity with policies and on one without.
const USERS: &[(&str, &str)] = &[("alice", "/eng/"), ("Bob", "/")];

/// Every user the account holds while the fixture is in place: the two the fixture
/// created, and the one the account holds BEFORE any request — the IAM user this
/// emulator's own long-term credential belongs to
/// (`service_common::caller_principal`), at the account root. A report of the
/// account's configuration carries it like any other user of the account.
const ALL_USERS: &[(&str, &str)] = &[
    ("alice", "/eng/"),
    ("Bob", "/"),
    (
        ::service_common::caller_principal::AMBIENT_USER_NAME,
        ::service_common::caller_principal::AMBIENT_USER_PATH,
    ),
];

/// The fixture groups, likewise: one carrying an inline policy, one carrying none.
const GROUPS: &[(&str, &str)] = &[("Admins", "/"), ("auditors_1", "/audit/")];

/// The fixture roles: one with an inline policy, an attachment, a tag and an instance
/// profile, one with nothing but its trust policy.
const ROLES: &[(&str, &str)] = &[("RoleOne", "/"), ("role.two", "/svc/")];

/// The fixture customer managed policies, at two path depths and in both cases.
const POLICIES: &[(&str, &str)] = &[("readers", "/"), ("Writers+1", "/data/")];

/// The user that carries policies, and the inline policies it carries, in the
/// policy-name order the account holds them in (ascending by the case-folded name).
const ALICE_INLINE: &[(&str, &str)] = &[("alice-read", DOCUMENT), ("AliceWrite", OTHER_DOCUMENT)];

/// The instance profile the fixture binds to the role that has one.
const PROFILE: &str = "profile-one";

/// The ARN a fixture policy is created under: `arn:aws:iam::<account>:policy<path><name>`
/// — IAM is global, so the region segment is EMPTY.
fn local_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// The ARNs of the account's own fixture policies, ascending — the order this
/// operation enumerates managed policies in.
fn local_arns() -> Vec<String> {
    let mut arns: Vec<String> = POLICIES
        .iter()
        .map(|(name, path)| local_arn(path, name))
        .collect();
    arns.sort();
    arns
}

/// The AWS-managed policies at the account root, in catalog order.
///
/// SOURCE: `state::AWS_MANAGED_POLICIES`, the catalog the service itself serves an
/// `arn:aws:iam::aws:policy/` read from, so the rows name catalog entries rather than
/// policy names written out here. The root path is what keeps a fixture attachment
/// clear of the path AWS reserves for service-linked roles.
fn aws_catalog_at_root() -> Vec<&'static state::AwsManagedPolicy> {
    state::AWS_MANAGED_POLICIES
        .iter()
        .filter(|entry| entry.path == state::DEFAULT_PATH)
        .collect()
}

/// The AWS-managed policy the fixture ATTACHES to a role.
fn attached_aws_arn() -> String {
    aws_catalog_at_root()
        .first()
        .expect("a catalog entry at the account root")
        .arn()
}

/// The AWS-managed policy the fixture uses to set a role's permissions BOUNDARY — the
/// other relation that puts an AWS-owned policy to use in the account.
fn boundary_aws_arn() -> String {
    aws_catalog_at_root()
        .get(1)
        .expect("a second catalog entry at the account root")
        .arn()
}

/// The AWS-managed policies the fixture account USES, ascending by ARN: the one
/// attached to a role and the one setting a role's permissions boundary.
fn aws_arns() -> Vec<String> {
    let mut arns = vec![attached_aws_arn(), boundary_aws_arn()];
    arns.sort();
    arns
}

/// Every AWS-managed policy the emulator carries that the fixture account does NOT
/// use, ascending by ARN — the catalog entries a report of this account's
/// configuration must leave out.
fn unused_aws_arns() -> Vec<String> {
    let used = aws_arns();
    let mut arns: Vec<String> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| entry.arn())
        .filter(|arn| !used.contains(arn))
        .collect();
    arns.sort();
    arns
}

/// The fixture entity names of one kind in the order the service enumerates them:
/// ascending by the case-folded name, which is the key its store is ordered on.
fn in_store_order(entities: &[(&'static str, &'static str)]) -> Vec<String> {
    let mut names: Vec<String> = entities.iter().map(|(name, _)| name.to_string()).collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// Drive one fixture call, asserting it succeeded.
async fn seed(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) {
    let (status, body) = call(router, action, &borrowed(params)).await;
    assert_eq!(
        status, 200,
        "the fixture {action} must succeed, got {status}: {body}"
    );
}

/// The account whose configuration every suite below reads: two users, two groups, two
/// roles and two customer managed policies, where the FIRST of each kind carries
/// policies (inline, attached, a boundary, a tag, an instance profile) and the second
/// carries none, plus the two AWS-owned policies the account puts to use.
async fn fixture() -> MultiServiceRouter {
    let router = router();

    for (name, path) in POLICIES {
        seed(
            &router,
            "CreatePolicy",
            &[
                param("PolicyName", name),
                param("Path", path),
                param("PolicyDocument", DOCUMENT),
                param("Description", "a fixture policy"),
            ],
        )
        .await;
    }

    for (name, path) in USERS {
        seed(
            &router,
            "CreateUser",
            &[param("UserName", name), param("Path", path)],
        )
        .await;
    }
    for (policy_name, document) in ALICE_INLINE {
        seed(
            &router,
            "PutUserPolicy",
            &[
                param("UserName", USERS[0].0),
                param("PolicyName", policy_name),
                param("PolicyDocument", document),
            ],
        )
        .await;
    }
    seed(
        &router,
        "AttachUserPolicy",
        &[
            param("UserName", USERS[0].0),
            param("PolicyArn", &local_arn(POLICIES[0].1, POLICIES[0].0)),
        ],
    )
    .await;
    seed(
        &router,
        "PutUserPermissionsBoundary",
        &[
            param("UserName", USERS[0].0),
            param(
                "PermissionsBoundary",
                &local_arn(POLICIES[1].1, POLICIES[1].0),
            ),
        ],
    )
    .await;
    seed(
        &router,
        "TagUser",
        &[
            param("UserName", USERS[0].0),
            param("Tags.member.1.Key", "team"),
            param("Tags.member.1.Value", "alpha"),
        ],
    )
    .await;

    for (name, path) in GROUPS {
        seed(
            &router,
            "CreateGroup",
            &[param("GroupName", name), param("Path", path)],
        )
        .await;
    }
    seed(
        &router,
        "PutGroupPolicy",
        &[
            param("GroupName", GROUPS[0].0),
            param("PolicyName", "admins-all"),
            param("PolicyDocument", DOCUMENT),
        ],
    )
    .await;

    for (name, path) in ROLES {
        seed(
            &router,
            "CreateRole",
            &[
                param("RoleName", name),
                param("Path", path),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
    }
    seed(
        &router,
        "PutRolePolicy",
        &[
            param("RoleName", ROLES[0].0),
            param("PolicyName", "role-one-inline"),
            param("PolicyDocument", OTHER_DOCUMENT),
        ],
    )
    .await;
    seed(
        &router,
        "AttachRolePolicy",
        &[
            param("RoleName", ROLES[0].0),
            param("PolicyArn", &local_arn(POLICIES[0].1, POLICIES[0].0)),
        ],
    )
    .await;
    // The two relations that put an AWS-OWNED policy to use in this account: one
    // attached to a role, one setting a role's permissions boundary.
    seed(
        &router,
        "AttachRolePolicy",
        &[
            param("RoleName", ROLES[0].0),
            param("PolicyArn", &attached_aws_arn()),
        ],
    )
    .await;
    seed(
        &router,
        "PutRolePermissionsBoundary",
        &[
            param("RoleName", ROLES[1].0),
            param("PermissionsBoundary", &boundary_aws_arn()),
        ],
    )
    .await;
    seed(
        &router,
        "TagRole",
        &[
            param("RoleName", ROLES[0].0),
            param("Tags.member.1.Key", "team"),
            param("Tags.member.1.Value", "storage"),
        ],
    )
    .await;
    seed(
        &router,
        "CreateInstanceProfile",
        &[param("InstanceProfileName", PROFILE)],
    )
    .await;
    seed(
        &router,
        "AddRoleToInstanceProfile",
        &[
            param("InstanceProfileName", PROFILE),
            param("RoleName", ROLES[0].0),
        ],
    )
    .await;

    router
}

/// The four list members `GetAccountAuthorizationDetailsResponse` declares
/// (`smithy_query(query="operation", operation="GetAccountAuthorizationDetails")`).
const LIST_TAGS: &[&str] = &[
    "UserDetailList",
    "GroupDetailList",
    "RoleDetailList",
    "Policies",
];

/// Where the next `<tag>` open tag at or after `from` starts, and where its content
/// starts — the second differs from the first by the tag's own length plus any
/// attributes it carries, which the serializer puts on the first element of a nested
/// document (`xmlns`).
fn next_open(haystack: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let needle = format!("<{tag}");
    let mut cursor = from;
    while let Some(offset) = haystack[cursor..].find(&needle) {
        let start = cursor + offset;
        let after = &haystack[start + needle.len()..];
        match after.chars().next() {
            Some('>') => return Some((start, start + needle.len() + 1)),
            // An open tag carrying attributes: its content starts after the `>` that
            // ends the tag.
            Some(whitespace) if whitespace.is_whitespace() => {
                let end = after
                    .find('>')
                    .unwrap_or_else(|| panic!("<{tag} is never closed in {haystack}"));
                return Some((start, start + needle.len() + end + 1));
            }
            // A longer tag name that merely starts with this one.
            _ => cursor = start + needle.len(),
        }
    }
    None
}

/// The inner XML of each `<member>` of the `list_tag` element, in document order, or
/// no blocks at all when the response omits that list.
///
/// A scan rather than a document model, as the sibling awsQuery suites do: nesting is
/// tracked only for `<member>` itself, which is what a detail carrying its own lists
/// (a role's instance profiles, a policy's versions) requires.
fn members(body: &str, list_tag: &str) -> Vec<String> {
    let close = format!("</{list_tag}>");
    let Some((_, inner_start)) = next_open(body, list_tag, 0) else {
        return Vec::new();
    };
    let rest = &body[inner_start..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("<{list_tag}> is never closed in {body}"));
    let inner = &rest[..end];
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some((_, content)) = next_open(inner, "member", cursor) {
        let mut depth = 1usize;
        let mut scan = content;
        while depth > 0 {
            let next_close = inner[scan..]
                .find("</member>")
                .map(|at| scan + at)
                .unwrap_or_else(|| panic!("a <member> is never closed in {body}"));
            match next_open(inner, "member", scan) {
                Some((nested, nested_content)) if nested < next_close => {
                    depth += 1;
                    scan = nested_content;
                }
                _ => {
                    depth -= 1;
                    if depth == 0 {
                        blocks.push(inner[content..next_close].to_string());
                    }
                    scan = next_close + "</member>".len();
                }
            }
        }
        cursor = scan;
    }
    blocks
}

/// The text of the FIRST `<tag>` element of `body`, or `None` when it carries none.
///
/// A detail's own members precede the lists nested inside it (the serializer emits
/// them in the shape's member order), so the first `<RoleName>` of a role detail is
/// the role's own and not that of a role inside one of its instance profiles.
fn first(body: &str, tag: &str) -> Option<String> {
    elements(body, tag).into_iter().next()
}

/// [`first`] for a member that carries a POLICY DOCUMENT, percent-decoded back to plain JSON
/// text: "policies returned by this operation are URL-encoded compliant with RFC 3986"
/// (`GetAccountAuthorizationDetails`), so a caller reads one through a URL decode.
fn first_document(body: &str, tag: &str) -> Option<String> {
    first(body, tag).map(|encoded| ::smithy_server_runtime::percent_decode(&encoded))
}

/// The `name_tag` of each detail in the `list_tag` of `body`, in document order.
fn detail_names(body: &str, list_tag: &str, name_tag: &str) -> Vec<String> {
    members(body, list_tag)
        .iter()
        .map(|block| {
            first(block, name_tag).unwrap_or_else(|| {
                panic!("a {list_tag} detail carries no <{name_tag}>: {block}")
            })
        })
        .collect()
}

/// The one detail of `list_tag` whose `name_tag` is `name`.
fn detail_block(body: &str, list_tag: &str, name_tag: &str, name: &str) -> String {
    let found: Vec<String> = members(body, list_tag)
        .into_iter()
        .filter(|block| first(block, name_tag).as_deref() == Some(name))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {name:?}: expected exactly one {list_tag} detail named {name:?}, got {}: {body}",
        found.len()
    );
    found.into_iter().next().expect("one detail")
}

/// One whole, untruncated report of the configuration, filtered by `filter`.
///
/// `MaxItems` is the published maximum (`maxItemsType` is range 1..1000), which is
/// above the fixture's entity count, so the assertions about WHAT is reported are
/// separate from the assertions about paging.
async fn report(router: &MultiServiceRouter, case: &str, filter: &[&str]) -> String {
    let mut params = vec![param("MaxItems", "1000")];
    for (position, entity_type) in filter.iter().enumerate() {
        params.push(param(
            &format!("Filter.member.{}", position + 1),
            entity_type,
        ));
    }
    let (status, body) = call(router, "GetAccountAuthorizationDetails", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "IsTruncated"),
        "false",
        "case {case:?}: the whole fixture fits in one page of 1000, so it must not be truncated: {body}"
    );
    body
}

/// The response body without its per-request envelope.
///
/// A `RequestId` is minted per call, so it is not part of what a repeated read of an
/// unchanged account must answer identically; everything the operation reports from
/// state is.
fn without_request_metadata(body: &str) -> String {
    let open = "<ResponseMetadata>";
    let close = "</ResponseMetadata>";
    match body.find(open) {
        Some(start) => {
            let end = body[start..]
                .find(close)
                .map(|at| start + at + close.len())
                .unwrap_or_else(|| panic!("{open} is never closed in {body}"));
            format!("{}{}", &body[..start], &body[end..])
        }
        None => body.to_string(),
    }
}

/// One row of the entity-type table: the `EntityType` the request filters by, the
/// response list the matching entities are reported in, the member they are named by,
/// and the names the report must carry in the order it must carry them.
struct TypeCase {
    case: &'static str,
    entity_type: &'static str,
    list_tag: &'static str,
    name_tag: &'static str,
    expected: Vec<String>,
}

/// Every entity type the `EntityType` enum declares
/// (`smithy_query(query="shape", shape="EntityType")`), with the fixture entities each
/// one selects.
fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            case: "users",
            entity_type: state::ENTITY_FILTER_USER,
            list_tag: "UserDetailList",
            name_tag: "UserName",
            expected: in_store_order(ALL_USERS),
        },
        TypeCase {
            case: "groups",
            entity_type: state::ENTITY_FILTER_GROUP,
            list_tag: "GroupDetailList",
            name_tag: "GroupName",
            expected: in_store_order(GROUPS),
        },
        TypeCase {
            case: "roles",
            entity_type: state::ENTITY_FILTER_ROLE,
            list_tag: "RoleDetailList",
            name_tag: "RoleName",
            expected: in_store_order(ROLES),
        },
        TypeCase {
            case: "the account's own managed policies",
            entity_type: state::ENTITY_FILTER_LOCAL_MANAGED_POLICY,
            list_tag: "Policies",
            name_tag: "Arn",
            expected: local_arns(),
        },
        TypeCase {
            case: "the AWS-managed policies the account uses",
            entity_type: state::ENTITY_FILTER_AWS_MANAGED_POLICY,
            list_tag: "Policies",
            name_tag: "Arn",
            expected: aws_arns(),
        },
    ]
}

/// RULE: a `Filter` naming one entity type reports exactly the persisted entities of
/// that type — "only the entities that match the types you specify are included in the
/// output" — and nothing of any other type.
#[tokio::test]
async fn a_filter_naming_one_entity_type_reports_exactly_that_type() {
    let router = fixture().await;

    for row in type_cases() {
        let case = row.case;
        let body = report(&router, case, &[row.entity_type]).await;
        let reported = detail_names(&body, row.list_tag, row.name_tag);
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected {:?} in <{}>, got {reported:?}",
            row.expected, row.list_tag
        );
        for other in LIST_TAGS.iter().filter(|tag| **tag != row.list_tag) {
            let stray = members(&body, other);
            assert!(
                stray.is_empty(),
                "case {case:?}: <{other}> must be empty when the filter names only {:?}, got {stray:?}",
                row.entity_type
            );
        }
    }
}

/// RULE: an absent `Filter` leaves no entity type out, so one report carries the
/// account's users, groups, roles and both kinds of managed policy at once.
#[tokio::test]
async fn an_absent_filter_reports_every_entity_type() {
    let router = fixture().await;
    let body = report(&router, "no filter", &[]).await;

    for row in type_cases() {
        let case = row.case;
        let reported = detail_names(&body, row.list_tag, row.name_tag);
        for name in &row.expected {
            assert!(
                reported.contains(name),
                "case {case:?}: an unfiltered report must carry {name:?} in <{}>, got {reported:?}",
                row.list_tag
            );
        }
    }

    // The two policy rows share one list, which must therefore carry both kinds and
    // nothing else: the account's own policies and the AWS-owned policies it uses.
    let mut expected_policies = local_arns();
    expected_policies.extend(aws_arns());
    let reported = detail_names(&body, "Policies", "Arn");
    let mut sorted = reported.clone();
    sorted.sort();
    expected_policies.sort();
    assert_eq!(
        sorted, expected_policies,
        "case \"no filter\": expected every managed policy the account holds or uses, got {reported:?}"
    );
}

/// RULE: an AWS-owned managed policy is published to every account, so it is part of
/// THIS account's configuration only while the account uses it — attached to an
/// identity, or setting one's permissions boundary — unlike a policy the account owns,
/// which it holds whether or not anything uses it.
#[tokio::test]
async fn an_aws_owned_policy_is_reported_only_while_the_account_uses_it() {
    let router = fixture().await;

    // The catalog entries the fixture leaves unused are reported by nothing, however
    // many of them the emulator carries.
    let body = report(
        &router,
        "the AWS-managed policies the account uses",
        &[state::ENTITY_FILTER_AWS_MANAGED_POLICY],
    )
    .await;
    let reported = detail_names(&body, "Policies", "Arn");
    for arn in unused_aws_arns() {
        assert!(
            !reported.contains(&arn),
            "case {arn:?}: a published policy the account does not use is no part of its configuration, got {reported:?}"
        );
    }
    for arn in aws_arns() {
        assert!(
            reported.contains(&arn),
            "case {arn:?}: a published policy the account uses must be reported, got {reported:?}"
        );
    }

    // And the report follows the relation: the attachment that put the policy to use is
    // what keeps it in the report, so detaching it takes it out again — while the
    // account's own policy, attached to nothing, stays.
    let detached = attached_aws_arn();
    seed(
        &router,
        "DetachRolePolicy",
        &[param("RoleName", ROLES[0].0), param("PolicyArn", &detached)],
    )
    .await;
    let body = report(
        &router,
        "after detaching the attached AWS-managed policy",
        &[
            state::ENTITY_FILTER_AWS_MANAGED_POLICY,
            state::ENTITY_FILTER_LOCAL_MANAGED_POLICY,
        ],
    )
    .await;
    let reported = detail_names(&body, "Policies", "Arn");
    assert!(
        !reported.contains(&detached),
        "case {detached:?}: a published policy nothing uses any more must leave the report, got {reported:?}"
    );
    assert!(
        reported.contains(&boundary_aws_arn()),
        "case {:?}: the policy still setting a role's boundary must stay, got {reported:?}",
        boundary_aws_arn()
    );
    for arn in local_arns() {
        assert!(
            reported.contains(&arn),
            "case {arn:?}: the account's own policy is reported whether or not anything uses it, got {reported:?}"
        );
    }
}

/// RULE: a `Filter` naming several entity types reports the union of those types and
/// nothing else.
#[tokio::test]
async fn a_filter_naming_several_entity_types_reports_their_union() {
    let router = fixture().await;

    // Rows span the arity (two types, three, every one the enum declares) and mix the
    // entity and the policy kinds, which are reported in different lists.
    let rows: &[(&str, &[&str])] = &[
        ("users and roles", &["User", "Role"]),
        (
            "groups and the account's policies",
            &["Group", "LocalManagedPolicy"],
        ),
        (
            "AWS's policies, users and groups",
            &["AWSManagedPolicy", "User", "Group"],
        ),
        (
            "every declared type",
            &[
                "User",
                "Group",
                "Role",
                "LocalManagedPolicy",
                "AWSManagedPolicy",
            ],
        ),
    ];

    for (case, filter) in rows {
        let body = report(&router, case, filter).await;
        for row in type_cases() {
            let wanted = filter.contains(&row.entity_type);
            let reported = detail_names(&body, row.list_tag, row.name_tag);
            for name in &row.expected {
                assert_eq!(
                    reported.contains(name),
                    wanted,
                    "case {case:?}: {name:?} must be {} in <{}>, got {reported:?}",
                    if wanted { "reported" } else { "left out" },
                    row.list_tag
                );
            }
        }
    }
}

/// RULE: a reported user carries the account's record of it together with the policies
/// attached to it — its inline policies with their documents, its managed-policy
/// attachments, its permissions boundary and its tags — and a user carrying none of
/// those reports none.
#[tokio::test]
async fn a_user_is_reported_with_the_policies_attached_to_it() {
    let router = fixture().await;
    let body = report(&router, "users", &[state::ENTITY_FILTER_USER]).await;

    // The user that carries policies.
    let block = detail_block(&body, "UserDetailList", "UserName", USERS[0].0);
    assert_eq!(
        first(&block, "Path").as_deref(),
        Some(USERS[0].1),
        "case {:?}: expected the stored path, got {block}",
        USERS[0].0
    );
    assert_eq!(
        first(&block, "Arn"),
        Some(format!(
            "arn:aws:iam::{}:user{}{}",
            account_id(),
            USERS[0].1,
            USERS[0].0
        )),
        "case {:?}: expected the stored ARN, got {block}",
        USERS[0].0
    );

    // The inline policies, each with the document that was put: `UserPolicyList` is
    // the only list of a user detail that carries documents.
    let mut expected_inline: Vec<(String, String)> = ALICE_INLINE
        .iter()
        .map(|(name, document)| (name.to_string(), document.to_string()))
        .collect();
    expected_inline.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    let reported_inline: Vec<(String, String)> = members(&block, "UserPolicyList")
        .iter()
        .map(|policy| {
            (
                first(policy, "PolicyName").expect("an inline policy name"),
                first_document(policy, "PolicyDocument").expect("an inline policy document"),
            )
        })
        .collect();
    assert_eq!(
        reported_inline, expected_inline,
        "case {:?}: expected the stored inline policies, got {reported_inline:?}",
        USERS[0].0
    );

    // The managed-policy attachments, named by the ARN they were made with.
    let attached: Vec<String> = members(&block, "AttachedManagedPolicies")
        .iter()
        .map(|policy| first(policy, "PolicyArn").expect("an attachment ARN"))
        .collect();
    assert_eq!(
        attached,
        vec![local_arn(POLICIES[0].1, POLICIES[0].0)],
        "case {:?}: expected the stored attachment, got {attached:?}",
        USERS[0].0
    );

    assert_eq!(
        first(&block, "PermissionsBoundaryArn"),
        Some(local_arn(POLICIES[1].1, POLICIES[1].0)),
        "case {:?}: expected the stored permissions boundary, got {block}",
        USERS[0].0
    );
    assert_eq!(
        reported_tags(&block).get("team").map(String::as_str),
        Some("alpha"),
        "case {:?}: expected the stored tag, got {block}",
        USERS[0].0
    );

    // The user that carries nothing: every one of those lists is empty, and the
    // optional members are absent rather than invented.
    let bare = detail_block(&body, "UserDetailList", "UserName", USERS[1].0);
    for list_tag in ["UserPolicyList", "AttachedManagedPolicies", "GroupList"] {
        let reported = members(&bare, list_tag);
        assert!(
            reported.is_empty(),
            "case {:?}: <{list_tag}> must be empty for a user carrying no policy, got {reported:?}",
            USERS[1].0
        );
    }
    assert_eq!(
        first(&bare, "PermissionsBoundaryArn"),
        None,
        "case {:?}: a user with no boundary must report none, got {bare}",
        USERS[1].0
    );
}

/// RULE: a reported role carries the account's record of it together with its trust
/// policy, the instance profiles that contain it and the policies attached to it; a
/// role carrying nothing but a trust policy reports only that.
#[tokio::test]
async fn a_role_is_reported_with_its_trust_policy_and_instance_profiles() {
    let router = fixture().await;
    let body = report(&router, "roles", &[state::ENTITY_FILTER_ROLE]).await;

    let block = detail_block(&body, "RoleDetailList", "RoleName", ROLES[0].0);
    assert_eq!(
        first_document(&block, "AssumeRolePolicyDocument").as_deref(),
        Some(TRUST_POLICY),
        "case {:?}: expected the stored trust policy, got {block}",
        ROLES[0].0
    );
    let profiles: Vec<String> = members(&block, "InstanceProfileList")
        .iter()
        .map(|profile| first(profile, "InstanceProfileName").expect("a profile name"))
        .collect();
    assert_eq!(
        profiles,
        vec![PROFILE.to_string()],
        "case {:?}: expected the instance profile the role was added to, got {profiles:?}",
        ROLES[0].0
    );
    let inline: Vec<String> = members(&block, "RolePolicyList")
        .iter()
        .map(|policy| first_document(policy, "PolicyDocument").expect("an inline document"))
        .collect();
    assert_eq!(
        inline,
        vec![OTHER_DOCUMENT.to_string()],
        "case {:?}: expected the stored inline policy document, got {inline:?}",
        ROLES[0].0
    );
    let attached: Vec<String> = members(&block, "AttachedManagedPolicies")
        .iter()
        .map(|policy| first(policy, "PolicyArn").expect("an attachment ARN"))
        .collect();
    let mut expected_attached = vec![
        local_arn(POLICIES[0].1, POLICIES[0].0),
        attached_aws_arn(),
    ];
    expected_attached.sort();
    assert_eq!(
        attached, expected_attached,
        "case {:?}: expected the stored attachments, got {attached:?}",
        ROLES[0].0
    );
    assert_eq!(
        reported_tags(&block).get("team").map(String::as_str),
        Some("storage"),
        "case {:?}: expected the stored tag, got {block}",
        ROLES[0].0
    );

    let bare = detail_block(&body, "RoleDetailList", "RoleName", ROLES[1].0);
    for list_tag in [
        "InstanceProfileList",
        "RolePolicyList",
        "AttachedManagedPolicies",
    ] {
        let reported = members(&bare, list_tag);
        assert!(
            reported.is_empty(),
            "case {:?}: <{list_tag}> must be empty for a role carrying no policy, got {reported:?}",
            ROLES[1].0
        );
    }
    assert_eq!(
        first_document(&bare, "AssumeRolePolicyDocument").as_deref(),
        Some(TRUST_POLICY),
        "case {:?}: expected the stored trust policy, got {bare}",
        ROLES[1].0
    );
}

/// RULE: a reported user carries the groups the account records it as a member of,
/// each named as the account holds it, and a user the account records no membership
/// for carries none.
///
/// The memberships read back here are seeded into the service's OWN store
/// (`state::group_members`), the way `tests/all/group_membership.rs` seeds the
/// memberships `GetGroup` reports, so this case exercises the REPORT alone;
/// `tests/all/group_membership_writes.rs` drives `AddUserToGroup`, which writes one.
#[tokio::test]
async fn a_user_is_reported_with_the_groups_it_belongs_to() {
    let (router, engine) = router_with_engine();
    for (name, path) in USERS {
        seed(
            &router,
            "CreateUser",
            &[param("UserName", name), param("Path", path)],
        )
        .await;
    }
    for (name, path) in GROUPS {
        seed(
            &router,
            "CreateGroup",
            &[param("GroupName", name), param("Path", path)],
        )
        .await;
    }
    {
        let store = state::service_state(&*engine).expect("the IAM store");
        let mut store = store.lock();
        for (group_name, _) in GROUPS {
            store
                .group_members
                .entry(state::name_key(group_name))
                .or_default()
                .insert(state::name_key(USERS[0].0));
        }
    }

    let body = report(&router, "users", &[state::ENTITY_FILTER_USER]).await;
    // Rows span the membership count: a user the account records in every fixture
    // group, and one it records in none.
    let rows: &[(&str, Vec<String>)] = &[
        (USERS[0].0, in_store_order(GROUPS)),
        (USERS[1].0, Vec::new()),
    ];
    for (user_name, expected) in rows {
        let block = detail_block(&body, "UserDetailList", "UserName", user_name);
        let reported = members(&block, "GroupList");
        assert_eq!(
            &reported, expected,
            "case {user_name:?}: expected the groups the account records the user in, got {reported:?}"
        );
    }
}

/// RULE: a reported group carries the account's record of it together with its inline
/// policies; a group carrying none reports none.
#[tokio::test]
async fn a_group_is_reported_with_its_inline_policies() {
    let router = fixture().await;
    let body = report(&router, "groups", &[state::ENTITY_FILTER_GROUP]).await;

    let block = detail_block(&body, "GroupDetailList", "GroupName", GROUPS[0].0);
    assert_eq!(
        first(&block, "Arn"),
        Some(format!(
            "arn:aws:iam::{}:group{}{}",
            account_id(),
            GROUPS[0].1,
            GROUPS[0].0
        )),
        "case {:?}: expected the stored ARN, got {block}",
        GROUPS[0].0
    );
    let inline: Vec<(String, String)> = members(&block, "GroupPolicyList")
        .iter()
        .map(|policy| {
            (
                first(policy, "PolicyName").expect("an inline policy name"),
                first_document(policy, "PolicyDocument").expect("an inline policy document"),
            )
        })
        .collect();
    assert_eq!(
        inline,
        vec![("admins-all".to_string(), DOCUMENT.to_string())],
        "case {:?}: expected the stored inline policy, got {inline:?}",
        GROUPS[0].0
    );

    let bare = detail_block(&body, "GroupDetailList", "GroupName", GROUPS[1].0);
    let reported = members(&bare, "GroupPolicyList");
    assert!(
        reported.is_empty(),
        "case {:?}: a group carrying no inline policy must report none, got {reported:?}",
        GROUPS[1].0
    );
}

/// RULE: a reported managed policy carries the same record its own `GetPolicy` read
/// reports, plus every version it holds with the document `GetPolicyVersion` serves —
/// for a policy the account owns and for one AWS owns alike.
#[tokio::test]
async fn a_managed_policy_is_reported_as_its_own_reader_reports_it() {
    let router = fixture().await;

    // Rows span the policy's owner: the account's own policies at two path depths, and
    // AWS's catalog, whose entries the account did not create.
    let mut arns = local_arns();
    arns.extend(aws_arns());
    let body = report(
        &router,
        "every managed policy",
        &[
            state::ENTITY_FILTER_LOCAL_MANAGED_POLICY,
            state::ENTITY_FILTER_AWS_MANAGED_POLICY,
        ],
    )
    .await;

    for arn in arns {
        let block = detail_block(&body, "Policies", "Arn", &arn);
        let (status, metadata) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the cross-read GetPolicy must succeed, got {status}: {metadata}"
        );
        // Every member of the policy's record the detail shares with `GetPolicy`. The
        // FIRST occurrence is the policy's own: its versions follow it and carry a
        // `CreateDate` of their own.
        for tag in [
            "PolicyName",
            "PolicyId",
            "Arn",
            "Path",
            "DefaultVersionId",
            "AttachmentCount",
            "PermissionsBoundaryUsageCount",
            "IsAttachable",
            "Description",
            "CreateDate",
            "UpdateDate",
        ] {
            assert_eq!(
                first(&block, tag),
                first(&metadata, tag),
                "case {arn:?}: <{tag}> must be what GetPolicy reports: {block}"
            );
        }

        let versions: Vec<String> = members(&block, "PolicyVersionList")
            .iter()
            .map(|version| first(version, "VersionId").expect("a version id"))
            .collect();
        assert!(
            !versions.is_empty(),
            "case {arn:?}: a managed policy must report the versions it holds, got {block}"
        );
        for version_id in versions {
            let version = members(&block, "PolicyVersionList")
                .into_iter()
                .find(|version| first(version, "VersionId").as_deref() == Some(&version_id))
                .expect("the reported version");
            let (status, served) = call(
                &router,
                "GetPolicyVersion",
                &[("PolicyArn", arn.as_str()), ("VersionId", &version_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {arn:?}/{version_id:?}: the cross-read GetPolicyVersion must succeed, got {status}: {served}"
            );
            for tag in ["VersionId", "Document", "IsDefaultVersion", "CreateDate"] {
                assert_eq!(
                    first(&version, tag),
                    first(&served, tag),
                    "case {arn:?}/{version_id:?}: <{tag}> must be what GetPolicyVersion serves: {version}"
                );
            }
        }
    }
}

/// Every entity the fixture persists, in the four lists it is reported in: the pairs a
/// complete walk of the configuration has to see exactly once.
fn every_detail() -> Vec<(&'static str, String)> {
    let mut expected: Vec<(&'static str, String)> = Vec::new();
    for row in type_cases() {
        for name in row.expected {
            expected.push((row.list_tag, name));
        }
    }
    expected.sort();
    expected
}

/// RULE: the four lists are pages of ONE enumeration under a single `Marker`, so
/// following the marker to the end reports every entity exactly once, whatever page
/// size the request asks for, and only the last page omits the marker.
#[tokio::test]
async fn paging_the_report_walks_the_whole_configuration_exactly_once() {
    let router = fixture().await;
    let expected = every_detail();

    // Rows span the page size: the published minimum, sizes that do and do not divide
    // the set, a size equal to the set, the published maximum, and the absent-parameter
    // default. `maxItemsType` is range 1..1000
    // (`smithy_query(query="operation", operation="GetAccountAuthorizationDetails")`).
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(7),
        Some(expected.len() as i32),
        Some(1000),
        None,
    ];

    for page_size in page_sizes {
        let case = format!("MaxItems={page_size:?}");
        let mut seen: Vec<(&'static str, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut params: Vec<(String, String)> = Vec::new();
            if let Some(size) = page_size {
                params.push(param("MaxItems", &size.to_string()));
            }
            if let Some(token) = &marker {
                params.push(param("Marker", token));
            }
            let (status, body) = call(
                &router,
                "GetAccountAuthorizationDetails",
                &borrowed(&params),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );

            let mut on_this_page = 0;
            for row in type_cases() {
                // A list is read once per entity type, and the two policy rows share
                // one list, so count and collect it only on its first row.
                if row.entity_type == state::ENTITY_FILTER_AWS_MANAGED_POLICY {
                    continue;
                }
                for name in detail_names(&body, row.list_tag, row.name_tag) {
                    seen.push((row.list_tag, name));
                    on_this_page += 1;
                }
            }
            if let Some(size) = page_size {
                assert!(
                    on_this_page <= *size,
                    "case {case:?}: expected at most {size} details on a page, got {on_this_page}"
                );
            }

            let truncated = element(&body, "IsTruncated");
            let page_marker = elements(&body, "Marker");
            if truncated == "true" {
                assert_eq!(
                    page_marker.len(),
                    1,
                    "case {case:?}: a truncated page must carry a Marker, got {page_marker:?}"
                );
                marker = Some(page_marker.into_iter().next().expect("a marker"));
            } else {
                assert_eq!(
                    truncated, "false",
                    "case {case:?}: expected IsTruncated true or false, got {truncated:?}"
                );
                assert!(
                    page_marker.is_empty(),
                    "case {case:?}: a final page must carry no Marker, got {page_marker:?}"
                );
                break;
            }

            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }

        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every entity exactly once, got {seen:?}"
        );
    }
}

/// RULE: a `Marker` this service did not issue for this report names no position in
/// the enumeration, so the call is refused rather than silently answered from the
/// beginning.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_is_refused() {
    let router = fixture().await;

    // Rows span the ways a token can fail to be one of this report's: text that is not
    // base64 at all, base64 that decodes to something else, and a token this service
    // DID issue while paging another collection.
    let (status, body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "the fixture ListUsers must succeed: {body}");
    let other_collections_marker = element(&body, "Marker");

    for bogus in [
        "not-a-marker",
        "zz",
        "0g",
        other_collections_marker.as_str(),
    ] {
        let (status, body) = call(
            &router,
            "GetAccountAuthorizationDetails",
            &[("Marker", bogus)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected the marker to be refused as invalid, got {body}"
        );
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs,
/// so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = fixture().await;
    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = call(
            &router,
            "GetAccountAuthorizationDetails",
            &[("MaxItems", out_of_range)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(
            &router,
            "GetAccountAuthorizationDetails",
            &[("MaxItems", in_range)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: reporting the configuration is a READ, so it changes nothing a later caller
/// sees — two reports of an unchanged account are identical, and the entities' own
/// readers answer afterwards exactly as they did before.
#[tokio::test]
async fn reporting_the_configuration_changes_nothing_a_later_caller_sees() {
    let router = fixture().await;

    let before = report(&router, "the first report", &[]).await;
    let cross_reads: &[(&str, &str, &str)] = &[
        ("GetUser", "UserName", USERS[0].0),
        ("GetGroup", "GroupName", GROUPS[0].0),
        ("GetRole", "RoleName", ROLES[0].0),
    ];
    let mut readers_before = Vec::new();
    for (action, name_param, name) in cross_reads {
        let (status, body) = call(&router, action, &[(name_param, name)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the cross-read {action} must succeed, got {status}: {body}"
        );
        readers_before.push(body);
    }

    let after = report(&router, "the second report", &[]).await;
    assert_eq!(
        without_request_metadata(&after),
        without_request_metadata(&before),
        "case \"repeated report\": a read must answer identically on an unchanged account"
    );

    for ((action, name_param, name), was) in cross_reads.iter().zip(readers_before) {
        let (status, body) = call(&router, action, &[(name_param, name)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: {action} must still succeed, got {status}: {body}"
        );
        assert_eq!(
            without_request_metadata(&body),
            without_request_metadata(&was),
            "case {name:?}: {action} must report what it reported before the configuration was read"
        );
    }
}
