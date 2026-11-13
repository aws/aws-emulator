// SPDX-License-Identifier: CC0-1.0

//! The writes this service had no handler for until now, end to end over the awsQuery
//! wire: clearing a permissions boundary, registering a client ID on an OIDC provider,
//! replacing that provider's thumbprint list, moving a user, and taking back an attachment
//! or a group membership.
//!
//! The rule under test: **each of these operations answers from the account's own stored
//! records — a write a later read reports, and the modelled not-found, invalid-input or
//! conflict error where the request names something the account does not hold** — never a
//! 501, which says only that nobody has written the operation yet and leaves every
//! negative path of the family it belongs to unobservable.
//!
//! The rows differ along what each rule generalises over: the entity kind a boundary is
//! cleared on, whether the entity exists and whether it carries a boundary at all, a value
//! that is an ARN and one that is not, a registration that is new and one the provider
//! already holds, a thumbprint list that replaces a longer one and one that replaces a
//! shorter, and a move that changes a user's name, its path or both. The concurrency rows
//! drive their racers CONCURRENTLY (`tokio::join!`), which is what two SDK clients writing
//! at once really is.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does; the awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::organizations_access_report_generation::record_organization;
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// A managed policy document with a single statement.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which a fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// A certificate thumbprint: 40 hexadecimal characters, the form
/// `CreateOpenIDConnectProvider$ThumbprintList` documents ("the thumbprint is always a
/// 40-character string").
fn thumbprint(seed: u8) -> String {
    format!("{:040x}", u128::from(seed) + 1)
}

/// The `jobStatusType` values the model declares for a report job, written out here
/// rather than taken from the implementation: what the suite pins is the value AWS
/// answers (`smithy_query(query="shape", shape="jobStatusType")`: `IN_PROGRESS`,
/// `COMPLETED`, `FAILED`).
const JOB_IN_PROGRESS: &str = "IN_PROGRESS";
const JOB_COMPLETED: &str = "COMPLETED";

/// The `PermissionsBoundaryType` AWS puts on the wire for a boundary it reports, likewise
/// written out: `Policy`, not the name the model gives the single
/// `PermissionsBoundaryAttachmentType` entry.
const BOUNDARY_ATTACHMENT_TYPE: &str = "Policy";

/// Drive one call a fixture needs, asserting it succeeded, and answer the body.
async fn expect_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status}: {body}"
    );
    body
}

/// One permissions-boundary row: the entity kind, the calls that set, read, clear and
/// create it.
struct BoundaryKind {
    case: &'static str,
    entity: &'static str,
    name_member: &'static str,
    put_action: &'static str,
    delete_action: &'static str,
    read_action: &'static str,
}

/// RULE: a permissions boundary is a record of the entity it is set on, so the delete
/// clears what the put wrote and a later read reports none; a delete that names an entity
/// the account does not hold, or an entity carrying no boundary, is `NoSuchEntity` 404 and
/// writes nothing.
#[tokio::test]
async fn clearing_a_permissions_boundary_undoes_the_put_and_404s_otherwise() {
    let kinds = &[
        BoundaryKind {
            case: "a role's boundary",
            entity: "bounded-role",
            name_member: "RoleName",
            put_action: "PutRolePermissionsBoundary",
            delete_action: "DeleteRolePermissionsBoundary",
            read_action: "GetRole",
        },
        BoundaryKind {
            case: "a user's boundary",
            entity: "bounded-user",
            name_member: "UserName",
            put_action: "PutUserPermissionsBoundary",
            delete_action: "DeleteUserPermissionsBoundary",
            read_action: "GetUser",
        },
    ];

    for kind in kinds {
        let case = kind.case;
        let router = router();
        let body = expect_ok(
            &router,
            case,
            "CreatePolicy",
            &[
                ("PolicyName", "boundary-policy"),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        let policy_arn = element(&body, "Arn");
        if kind.name_member == "RoleName" {
            expect_ok(
                &router,
                case,
                "CreateRole",
                &[
                    ("RoleName", kind.entity),
                    ("AssumeRolePolicyDocument", TRUST_POLICY),
                ],
            )
            .await;
        } else {
            expect_ok(&router, case, "CreateUser", &[("UserName", kind.entity)]).await;
        }

        // Before any put there is no boundary to delete: the resource the delete names
        // does not exist.
        let (status, body) = call(
            &router,
            kind.delete_action,
            &[(kind.name_member, kind.entity)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 with no boundary set, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );

        // The put writes it, and the entity's own read reports it.
        expect_ok(
            &router,
            case,
            kind.put_action,
            &[
                (kind.name_member, kind.entity),
                ("PermissionsBoundary", &policy_arn),
            ],
        )
        .await;
        let body = expect_ok(
            &router,
            case,
            kind.read_action,
            &[(kind.name_member, kind.entity)],
        )
        .await;
        assert_eq!(
            element(&body, "PermissionsBoundaryArn"),
            policy_arn,
            "case {case:?}: expected the put boundary on the entity"
        );
        assert_eq!(
            element(&body, "PermissionsBoundaryType"),
            BOUNDARY_ATTACHMENT_TYPE,
            "case {case:?}: expected the modelled attachment type"
        );

        // The delete clears it: the read reports no boundary at all, and the entity is
        // otherwise untouched.
        expect_ok(
            &router,
            case,
            kind.delete_action,
            &[(kind.name_member, kind.entity)],
        )
        .await;
        let body = expect_ok(
            &router,
            case,
            kind.read_action,
            &[(kind.name_member, kind.entity)],
        )
        .await;
        assert!(
            elements(&body, "PermissionsBoundaryArn").is_empty(),
            "case {case:?}: expected no boundary after the delete, got {body}"
        );
        assert!(
            !elements(&body, "Arn").is_empty(),
            "case {case:?}: the entity itself must survive the delete, got {body}"
        );

        // A second delete has nothing to clear, and an entity the account does not hold
        // has nothing either.
        for (what, entity) in [
            ("the boundary already cleared", kind.entity),
            ("an entity nothing created", "never-created"),
        ] {
            let (status, body) =
                call(&router, kind.delete_action, &[(kind.name_member, entity)]).await;
            assert_eq!(
                status, 404,
                "case {case:?} / {what:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?} / {what:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
        }
    }
}

/// A service-linked role is a protected resource: only the service it is linked to may
/// change it, so clearing its boundary is refused as unmodifiable rather than cleared —
/// the same protection `PutRolePermissionsBoundary` answers for the same role.
#[tokio::test]
async fn a_service_linked_roles_boundary_cannot_be_cleared() {
    let router = router();
    let role = "AWSServiceRoleForAutoScaling";
    let (status, body) = call(
        &router,
        "CreateServiceLinkedRole",
        &[("AWSServiceName", "autoscaling.amazonaws.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture service-linked role must be created, got {status}: {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteRolePermissionsBoundary",
        &[("RoleName", role)],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected the protected-role refusal's HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "UnmodifiableEntity",
        "expected UnmodifiableEntity, got {:?}: {body}",
        error_code(&body)
    );
    assert_eq!(
        element(&body, "Message"),
        state::protected_role_message(role),
        "expected the one sentence this service words that protection with"
    );
}

/// Create an OIDC provider with the client IDs and thumbprints given, answering its ARN.
async fn create_provider(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    client_ids: &[&str],
    thumbprints: &[String],
) -> String {
    let mut params = vec![param("Url", url)];
    for (index, client_id) in client_ids.iter().enumerate() {
        params.push(param(
            &format!("ClientIDList.member.{}", index + 1),
            client_id,
        ));
    }
    for (index, value) in thumbprints.iter().enumerate() {
        params.push(param(
            &format!("ThumbprintList.member.{}", index + 1),
            value,
        ));
    }
    let body = expect_ok(
        router,
        case,
        "CreateOpenIDConnectProvider",
        &borrowed(&params),
    )
    .await;
    element(&body, "OpenIDConnectProviderArn")
}

/// RULE: registering a client ID on an OIDC provider is a write the provider's own read
/// reports, "idempotent; it does not fail or return an error if you add an existing client
/// ID", and refused with the modelled error where the request names no provider or no ARN.
#[tokio::test]
async fn a_client_id_registration_is_reported_by_the_provider_and_is_idempotent() {
    let router = router();
    let case = "the OIDC provider fixtures";
    let arn = create_provider(
        &router,
        case,
        "https://oidc.example.com/first",
        &["sts.amazonaws.com"],
        &[thumbprint(1)],
    )
    .await;

    // The rows differ along the audience's form, which `clientIDType` bounds only by
    // length: a service principal, a URL, a UUID-shaped id and a single character.
    let registered = [
        "another.amazonaws.com",
        "https://relying.party.example/app",
        "6b3a4d6e-1f2c-4f1b-9a52-0c2f7d9e1a33",
        "x",
    ];
    for client_id in registered {
        expect_ok(
            &router,
            case,
            "AddClientIDToOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", &arn), ("ClientID", client_id)],
        )
        .await;
        // The repeat registers nothing a second time and still succeeds.
        expect_ok(
            &router,
            case,
            "AddClientIDToOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", &arn), ("ClientID", client_id)],
        )
        .await;
    }

    let body = expect_ok(
        &router,
        case,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &arn)],
    )
    .await;
    let mut expected = vec!["sts.amazonaws.com".to_string()];
    expected.extend(registered.iter().map(ToString::to_string));
    assert_eq!(
        elements(&body, "member")
            .into_iter()
            .filter(|value| expected.contains(value))
            .collect::<Vec<String>>(),
        expected,
        "expected every registration once, in the order they were made, got {body}"
    );

    // The negative paths the family models: a value that is not an ARN, and a well-formed
    // ARN the account holds no provider under.
    let unheld = format!("arn:aws:iam::{}:oidc-provider/nothing.example", account_id());
    let refusals: &[(&str, &str, u16, &str)] = &[
        (
            "a value that is not an ARN",
            "oidc.example.com/first",
            400,
            "InvalidInput",
        ),
        (
            "an ARN the account holds no provider under",
            &unheld,
            404,
            "NoSuchEntity",
        ),
    ];
    for (what, value, expected_status, expected_code) in refusals {
        let (status, body) = call(
            &router,
            "AddClientIDToOpenIDConnectProvider",
            &[
                ("OpenIDConnectProviderArn", value),
                ("ClientID", "sts.amazonaws.com"),
            ],
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {what:?}: expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {what:?}: expected {expected_code}, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: "the list that you pass with this operation completely replaces the existing list
/// of thumbprints. (The lists are not merged.)" — so the provider reports exactly what the
/// request carried, whether that is more thumbprints than it held or fewer.
#[tokio::test]
async fn a_thumbprint_update_replaces_the_list_rather_than_merging_it() {
    let router = router();
    let case = "the thumbprint fixtures";
    let arn = create_provider(
        &router,
        case,
        "https://oidc.example.com/second",
        &["sts.amazonaws.com"],
        &[thumbprint(1), thumbprint(2)],
    )
    .await;

    // Rows spanning the counts a replacement can carry against the two the provider was
    // created with: one, three, and the five the provider's quota admits.
    for count in [1_usize, 3, state::MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER] {
        let replacement: Vec<String> = (0..count).map(|seed| thumbprint(seed as u8 + 10)).collect();
        let mut params = vec![param("OpenIDConnectProviderArn", &arn)];
        for (index, value) in replacement.iter().enumerate() {
            params.push(param(
                &format!("ThumbprintList.member.{}", index + 1),
                value,
            ));
        }
        expect_ok(
            &router,
            case,
            "UpdateOpenIDConnectProviderThumbprint",
            &borrowed(&params),
        )
        .await;
        let body = expect_ok(
            &router,
            case,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", &arn)],
        )
        .await;
        let reported: Vec<String> = elements(&body, "member")
            .into_iter()
            .filter(|value| value.len() == 40)
            .collect();
        assert_eq!(
            reported, replacement,
            "case {count} thumbprints: expected the list to be replaced, got {body}"
        );
    }

    // A list past the provider's quota is refused, and the provider keeps what it held.
    let held = state::MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER;
    let too_many: Vec<String> = (0..held + 1).map(|seed| thumbprint(seed as u8 + 40)).collect();
    let mut params = vec![param("OpenIDConnectProviderArn", &arn)];
    for (index, value) in too_many.iter().enumerate() {
        params.push(param(
            &format!("ThumbprintList.member.{}", index + 1),
            value,
        ));
    }
    let (status, body) = call(
        &router,
        "UpdateOpenIDConnectProviderThumbprint",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected the quota refusal's HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("Cannot exceed quota for ThumbprintsPerOpenIdConnectProvider: {held}"),
        "expected the quota's own wording"
    );

    // An ARN the account holds no provider under is the modelled not-found.
    let unheld = format!("arn:aws:iam::{}:oidc-provider/nothing.example", account_id());
    let (status, body) = call(
        &router,
        "UpdateOpenIDConnectProviderThumbprint",
        &[
            ("OpenIDConnectProviderArn", &unheld),
            ("ThumbprintList.member.1", &thumbprint(7)),
        ],
    )
    .await;
    assert_eq!(status, 404, "expected HTTP 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}: {body}",
        error_code(&body)
    );
}

/// RULE (contention): concurrent writers of one OIDC provider each leave their write in
/// place — the registrations are applied one after another rather than one overwriting the
/// other — and no racer is refused for anything but contention.
#[tokio::test]
async fn concurrent_client_id_registrations_all_land() {
    let router = router();
    let case = "the racing fixtures";
    let arn = create_provider(
        &router,
        case,
        "https://oidc.example.com/third",
        &[],
        &[thumbprint(1)],
    )
    .await;

    let register = |client_id: &'static str| {
        let arn = arn.clone();
        let router = &router;
        async move {
            call(
                router,
                "AddClientIDToOpenIDConnectProvider",
                &[("OpenIDConnectProviderArn", &arn), ("ClientID", client_id)],
            )
            .await
        }
    };
    let answers = tokio::join!(
        register("racer-one"),
        register("racer-two"),
        register("racer-three"),
        register("racer-four"),
        register("racer-five"),
    );
    let answers = [
        answers.0, answers.1, answers.2, answers.3, answers.4,
    ];
    let mut winners = 0;
    for (status, body) in &answers {
        if *status == 200 {
            winners += 1;
            continue;
        }
        // A loser may only be told it lost to contention — never that the provider is
        // missing, and never that the operation does not exist.
        assert_eq!(
            error_code(body),
            "ConcurrentModification",
            "a losing racer may only be refused ConcurrentModification, got {:?}: {body}",
            error_code(body)
        );
    }
    assert!(
        winners >= 1,
        "expected at least one successful registration, got {answers:?}"
    );

    let body = expect_ok(
        &router,
        case,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", &arn)],
    )
    .await;
    let registered: Vec<String> = elements(&body, "member")
        .into_iter()
        .filter(|value| value.starts_with("racer-"))
        .collect();
    assert_eq!(
        registered.len(),
        winners,
        "expected the provider to hold exactly the registrations that succeeded, got {body}"
    );
}

/// One call that must be refused with the modelled not-found: the row's name, the
/// operation, and the request it carries.
struct Refused {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
}

/// One `UpdateUser` row: what the move changes, and where the user must end up.
struct Move {
    case: &'static str,
    /// The `NewUserName` the request carries, or `None` for a move that leaves the name.
    new_name: Option<&'static str>,
    /// The `NewPath` the request carries, or `None` for a move that leaves the path.
    new_path: Option<&'static str>,
    /// The name and path the user must be found under afterwards.
    expected_name: &'static str,
    expected_path: &'static str,
}

/// RULE: `UpdateUser` moves the user's name, its path or both, and "the user's unique
/// identifier remains the same" — so everything the account holds ABOUT the user travels
/// with it, while a new name the account already holds is refused and nothing moves.
#[tokio::test]
async fn moving_a_user_keeps_its_identifier_and_everything_it_holds() {
    // The rows span what the move can change: the path alone, the name alone, both at
    // once, and a request that spells the name the user already has.
    let rows = &[
        Move {
            case: "the path alone",
            new_name: None,
            new_path: Some("/moved/"),
            expected_name: "mover-a",
            expected_path: "/moved/",
        },
        Move {
            case: "the name alone",
            new_name: Some("mover-b-renamed"),
            new_path: None,
            expected_name: "mover-b-renamed",
            expected_path: "/before/",
        },
        Move {
            case: "both at once",
            new_name: Some("mover-c-renamed"),
            new_path: Some("/after/"),
            expected_name: "mover-c-renamed",
            expected_path: "/after/",
        },
        Move {
            case: "the name it already has",
            new_name: Some("mover-d"),
            new_path: None,
            expected_name: "mover-d",
            expected_path: "/before/",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (what, new_name, new_path, expected_name, expected_path) = (
            row.case,
            row.new_name,
            row.new_path,
            row.expected_name,
            row.expected_path,
        );
        let router = router();
        let original = format!("mover-{}", ['a', 'b', 'c', 'd'][index]);
        expect_ok(
            &router,
            what,
            "CreateUser",
            &[("UserName", &original), ("Path", "/before/")],
        )
        .await;
        // The records that must travel with the user: an inline policy, an attached
        // managed policy, a console password and a group membership.
        let body = expect_ok(
            &router,
            what,
            "CreatePolicy",
            &[
                ("PolicyName", &format!("mover-policy-{index}")),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        let policy_arn = element(&body, "Arn");
        expect_ok(
            &router,
            what,
            "PutUserPolicy",
            &[
                ("UserName", &original),
                ("PolicyName", "embedded"),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        expect_ok(
            &router,
            what,
            "AttachUserPolicy",
            &[("UserName", &original), ("PolicyArn", &policy_arn)],
        )
        .await;
        expect_ok(
            &router,
            what,
            "CreateLoginProfile",
            &[("UserName", &original), ("Password", "a-password-1!")],
        )
        .await;
        expect_ok(&router, what, "CreateGroup", &[("GroupName", "movers")]).await;
        expect_ok(
            &router,
            what,
            "AddUserToGroup",
            &[("GroupName", "movers"), ("UserName", &original)],
        )
        .await;
        let body = expect_ok(&router, what, "GetUser", &[("UserName", &original)]).await;
        let created_id = element(&body, "UserId");

        let mut params = vec![param("UserName", &original)];
        if let Some(name) = new_name {
            params.push(param("NewUserName", name));
        }
        if let Some(path) = new_path {
            params.push(param("NewPath", path));
        }
        expect_ok(&router, what, "UpdateUser", &borrowed(&params)).await;

        // The user is the same user, under the name and path the move asked for.
        let body = expect_ok(&router, what, "GetUser", &[("UserName", expected_name)]).await;
        assert_eq!(
            element(&body, "UserId"),
            created_id,
            "case {what:?}: expected the user to keep its unique identifier"
        );
        assert_eq!(
            element(&body, "Path"),
            *expected_path,
            "case {what:?}: expected the path the move asked for"
        );
        assert_eq!(
            element(&body, "Arn"),
            format!(
                "arn:aws:iam::{}:user{expected_path}{expected_name}",
                account_id()
            ),
            "case {what:?}: expected the ARN built from the new path and name"
        );
        // Everything it held travelled with it.
        let body = expect_ok(
            &router,
            what,
            "GetUserPolicy",
            &[("UserName", expected_name), ("PolicyName", "embedded")],
        )
        .await;
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            POLICY_DOCUMENT,
            "case {what:?}: expected the inline policy to travel with the user"
        );
        let body = expect_ok(
            &router,
            what,
            "ListAttachedUserPolicies",
            &[("UserName", expected_name)],
        )
        .await;
        assert!(
            elements(&body, "PolicyArn").contains(&policy_arn),
            "case {what:?}: expected the attachment to travel with the user, got {body}"
        );
        expect_ok(
            &router,
            what,
            "GetLoginProfile",
            &[("UserName", expected_name)],
        )
        .await;
        let body = expect_ok(&router, what, "GetGroup", &[("GroupName", "movers")]).await;
        assert!(
            elements(&body, "UserName").contains(&expected_name.to_string()),
            "case {what:?}: expected the membership to travel with the user, got {body}"
        );
        // And the old name names nothing, where the move really changed it.
        if expected_name != original.as_str() {
            let (status, body) = call(&router, "GetUser", &[("UserName", &original)]).await;
            assert_eq!(
                status, 404,
                "case {what:?}: expected the old name to name nothing, got {status}: {body}"
            );
        }
    }
}

/// A move onto a name the account already holds is refused, and nothing moves.
#[tokio::test]
async fn moving_a_user_onto_a_held_name_is_refused() {
    let router = router();
    let case = "a name another user holds";
    expect_ok(&router, case, "CreateUser", &[("UserName", "first-user")]).await;
    expect_ok(&router, case, "CreateUser", &[("UserName", "second-user")]).await;

    // The rows span the casings IAM treats as one name: "names are not distinguished by
    // case", so either spelling of the held name is the same conflict.
    for held in ["second-user", "Second-User"] {
        let (status, body) = call(
            &router,
            "UpdateUser",
            &[("UserName", "first-user"), ("NewUserName", held)],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {held:?}: expected HTTP 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {held:?}: expected EntityAlreadyExists, got {:?}: {body}",
            error_code(&body)
        );
        let (status, _) = call(&router, "GetUser", &[("UserName", "first-user")]).await;
        assert_eq!(
            status, 200,
            "case {held:?}: a refused move must leave the user where it was"
        );
    }

    // A user the account does not hold cannot be moved either.
    let (status, body) = call(
        &router,
        "UpdateUser",
        &[("UserName", "never-created"), ("NewPath", "/elsewhere/")],
    )
    .await;
    assert_eq!(status, 404, "expected HTTP 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}: {body}",
        error_code(&body)
    );
}

/// RULE: an Organizations access report is a JOB the account records — the generate mints
/// its id and the read reports THAT job, moving from `IN_PROGRESS` to `COMPLETED` when the
/// generation the clock owes becomes due — and a job id the account holds nothing under is
/// the modelled not-found.
#[tokio::test]
async fn an_access_report_job_is_recorded_and_reported_until_the_clock_completes_it() {
    // The generate is served only to the management account of the organization the entity
    // path names ("to call this operation, you must be signed in using your Organizations
    // management account credentials"), so the fixture records this account as the
    // management account of the organization every row below asks about;
    // `tests/all/organizations_access_report_generation.rs` owns the refusal every other
    // caller gets.
    let (router, engine) = router_with_engine();
    record_organization(&engine, "o-abcdefghij", account_id());
    let case = "the report fixtures";

    // Rows spanning the Organizations entities a report can be asked for, as
    // `organizationsEntityPathType`'s own pattern admits them (`o-…/r-…[0-9a-z-/]*`): the
    // root, an organizational unit under it, an account under that, and a root with the
    // SCP the request narrows the report to.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("the organization root", "o-abcdefghij/r-root", None),
        (
            "an organizational unit",
            "o-abcdefghij/r-root/ou-root-engineering",
            None,
        ),
        (
            "an account under an unit",
            "o-abcdefghij/r-root/ou-root-engineering/111122223333",
            None,
        ),
        (
            "the root, narrowed to one service control policy",
            "o-abcdefghij/r-root",
            Some("p-FullAWSAccess"),
        ),
    ];

    let mut jobs = Vec::new();
    for (what, entity_path, policy_id) in rows {
        let mut params = vec![param("EntityPath", entity_path)];
        if let Some(policy_id) = policy_id {
            params.push(param("OrganizationsPolicyId", policy_id));
        }
        let body = expect_ok(
            &router,
            what,
            "GenerateOrganizationsAccessReport",
            &borrowed(&params),
        )
        .await;
        let job_id = element(&body, "JobId");
        assert_eq!(
            job_id.len(),
            36,
            "case {what:?}: expected the 36-character job id `jobIDType` bounds, got \
             {job_id:?}"
        );
        assert!(
            !jobs.contains(&job_id),
            "case {what:?}: every generate must mint its own job id, got {job_id:?} twice"
        );

        // The job is in flight the instant it is asked for: the generation is not due yet,
        // so the read reports it IN_PROGRESS and no completion instant.
        let body = expect_ok(
            &router,
            what,
            "GetOrganizationsAccessReport",
            &[("JobId", &job_id)],
        )
        .await;
        assert_eq!(
            element(&body, "JobStatus"),
            JOB_IN_PROGRESS,
            "case {what:?}: expected the job to be in flight, got {body}"
        );
        assert!(
            !elements(&body, "JobCreationDate").is_empty(),
            "case {what:?}: expected the creation instant, got {body}"
        );
        assert!(
            elements(&body, "JobCompletionDate").is_empty(),
            "case {what:?}: a job in progress has no completion instant, got {body}"
        );
        jobs.push(job_id);
    }

    // Past the instant the generation is due, every job the account holds reports as
    // completed, with that instant as its completion date.
    tokio::time::sleep(::std::time::Duration::from_secs(
        state::ACCESS_REPORT_GENERATION_SECONDS as u64 + 1,
    ))
    .await;
    for job_id in &jobs {
        let body = expect_ok(
            &router,
            case,
            "GetOrganizationsAccessReport",
            &[("JobId", job_id)],
        )
        .await;
        assert_eq!(
            element(&body, "JobStatus"),
            JOB_COMPLETED,
            "expected the generation the clock owed to be done, got {body}"
        );
        assert!(
            !elements(&body, "JobCompletionDate").is_empty(),
            "expected the completion instant on a completed job, got {body}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "expected the whole report in one page, got {body}"
        );
    }

    // A job id the account holds no report under is the modelled not-found — the negative
    // path a client polls for a report it never asked for.
    for (what, job_id) in [
        ("a job id nothing minted", "00000000-0000-0000-0000-000000000000"),
        ("the shape of a job id with another value", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    ] {
        let (status, body) = call(
            &router,
            "GetOrganizationsAccessReport",
            &[("JobId", job_id)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE (the quota): "the maximum number of concurrent requests for this account are
/// already running" is the modelled refusal of one generate too many, so a caller cannot
/// start report generations without limit — and once the jobs in flight have completed,
/// the next generate is served again.
#[tokio::test]
async fn one_access_report_generation_too_many_is_refused_while_the_others_run() {
    // The management account of the organization the rows ask about, for the reason
    // `an_access_report_job_is_recorded_and_reported_until_the_clock_completes_it` states.
    let (router, engine) = router_with_engine();
    record_organization(&engine, "o-abcdefghij", account_id());
    let limit = state::MAX_CONCURRENT_ACCESS_REPORT_JOBS;
    for index in 0..limit {
        let body = expect_ok(
            &router,
            "filling the concurrent quota",
            "GenerateOrganizationsAccessReport",
            &[("EntityPath", "o-abcdefghij/r-root")],
        )
        .await;
        assert!(
            !element(&body, "JobId").is_empty(),
            "generation {index} must answer a job id"
        );
    }
    let (status, body) = call(
        &router,
        "GenerateOrganizationsAccessReport",
        &[("EntityPath", "o-abcdefghij/r-root")],
    )
    .await;
    assert_eq!(
        status, 409,
        "expected the concurrency quota's HTTP 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ReportGenerationLimitExceeded",
        "expected ReportGenerationLimitExceeded, got {:?}: {body}",
        error_code(&body)
    );
    assert_eq!(
        element(&body, "Message"),
        format!("Cannot exceed quota for ConcurrentReportGenerations: {limit}"),
        "expected the quota's own wording"
    );

    // The quota is on what is RUNNING: once the generations the clock owed are done, the
    // next request is served.
    tokio::time::sleep(::std::time::Duration::from_secs(
        state::ACCESS_REPORT_GENERATION_SECONDS as u64 + 1,
    ))
    .await;
    expect_ok(
        &router,
        "after the running generations completed",
        "GenerateOrganizationsAccessReport",
        &[("EntityPath", "o-abcdefghij/r-root")],
    )
    .await;
}

/// RULE: taking back a grant — detaching a managed policy from a user or a group, removing
/// a user from a group — is a write the matching read reports, and the delete it was
/// blocking then succeeds; a grant the entity does not hold is the modelled not-found.
#[tokio::test]
async fn taking_back_an_attachment_or_a_membership_unblocks_the_delete() {
    let router = router();
    let case = "the detach fixtures";
    let body = expect_ok(
        &router,
        case,
        "CreatePolicy",
        &[
            ("PolicyName", "detachable"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    let policy_arn = element(&body, "Arn");
    expect_ok(&router, case, "CreateUser", &[("UserName", "detach-user")]).await;
    expect_ok(&router, case, "CreateGroup", &[("GroupName", "detach-group")]).await;
    expect_ok(
        &router,
        case,
        "AttachUserPolicy",
        &[("UserName", "detach-user"), ("PolicyArn", &policy_arn)],
    )
    .await;
    expect_ok(
        &router,
        case,
        "AttachGroupPolicy",
        &[("GroupName", "detach-group"), ("PolicyArn", &policy_arn)],
    )
    .await;
    expect_ok(
        &router,
        case,
        "AddUserToGroup",
        &[("GroupName", "detach-group"), ("UserName", "detach-user")],
    )
    .await;

    // While the grants stand, the policy cannot be deleted: it is attached to entities.
    let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &policy_arn)]).await;
    assert_eq!(
        status, 409,
        "expected the attached policy's DeleteConflict, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DeleteConflict",
        "expected DeleteConflict, got {:?}: {body}",
        error_code(&body)
    );

    // Each take-back is reported by the read that reported the grant.
    expect_ok(
        &router,
        case,
        "DetachUserPolicy",
        &[("UserName", "detach-user"), ("PolicyArn", &policy_arn)],
    )
    .await;
    let body = expect_ok(
        &router,
        case,
        "ListAttachedUserPolicies",
        &[("UserName", "detach-user")],
    )
    .await;
    assert!(
        !elements(&body, "PolicyArn").contains(&policy_arn),
        "expected the user's attachment to be gone, got {body}"
    );
    expect_ok(
        &router,
        case,
        "DetachGroupPolicy",
        &[("GroupName", "detach-group"), ("PolicyArn", &policy_arn)],
    )
    .await;
    let body = expect_ok(
        &router,
        case,
        "ListAttachedGroupPolicies",
        &[("GroupName", "detach-group")],
    )
    .await;
    assert!(
        !elements(&body, "PolicyArn").contains(&policy_arn),
        "expected the group's attachment to be gone, got {body}"
    );
    expect_ok(
        &router,
        case,
        "RemoveUserFromGroup",
        &[("GroupName", "detach-group"), ("UserName", "detach-user")],
    )
    .await;
    let body = expect_ok(&router, case, "GetGroup", &[("GroupName", "detach-group")]).await;
    assert!(
        !elements(&body, "UserName").contains(&"detach-user".to_string()),
        "expected the membership to be gone, got {body}"
    );

    // With every grant taken back, the deletes they blocked succeed.
    expect_ok(&router, case, "DeletePolicy", &[("PolicyArn", &policy_arn)]).await;
    expect_ok(
        &router,
        case,
        "DeleteGroup",
        &[("GroupName", "detach-group")],
    )
    .await;
    expect_ok(&router, case, "DeleteUser", &[("UserName", "detach-user")]).await;

    // A grant the entity does not hold is the modelled not-found, on each of the three.
    let unheld = format!("arn:aws:iam::{}:policy/NeverAttached", account_id());
    expect_ok(&router, case, "CreateUser", &[("UserName", "unattached")]).await;
    expect_ok(&router, case, "CreateGroup", &[("GroupName", "ungranted")]).await;
    let refusals = &[
        Refused {
            case: "a policy the user does not carry",
            action: "DetachUserPolicy",
            params: vec![param("UserName", "unattached"), param("PolicyArn", &unheld)],
        },
        Refused {
            case: "a policy the group does not carry",
            action: "DetachGroupPolicy",
            params: vec![param("GroupName", "ungranted"), param("PolicyArn", &unheld)],
        },
        Refused {
            case: "a user the account does not hold",
            action: "RemoveUserFromGroup",
            params: vec![
                param("GroupName", "ungranted"),
                param("UserName", "never-created"),
            ],
        },
    ];
    for row in refusals {
        let (what, action) = (row.case, row.action);
        let (status, body) = call(&router, action, &borrowed(&row.params)).await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
    }

    // Removing a membership the group does not hold is no error: the membership the
    // request asks to be gone is already gone.
    expect_ok(
        &router,
        case,
        "RemoveUserFromGroup",
        &[("GroupName", "ungranted"), ("UserName", "unattached")],
    )
    .await;
}
