// SPDX-License-Identifier: CC0-1.0

//! The service-linked roles the OWNING SERVICE keeps provisioned, read end to end
//! over the awsQuery wire.
//!
//! A service-linked role is "predefined by the service" and controlled by it rather
//! than by the account ("only the service that depends on the service-linked role
//! can modify or delete the role on your behalf",
//! `UnmodifiableEntityException`), so for every service this emulator serves that
//! role is already provisioned and `GetRole` resolves it under the name AWS
//! publishes for the service — which is what lets a client that waits for IAM to
//! report one (the documented bootstrap before using a dependent resource) converge
//! instead of spinning on `NoSuchEntity`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! service principal and its label count, the shape of the published name (named for
//! the service, for a feature of it, or carrying a resource-type suffix), the case
//! the request spells the name in, and whether any service publishes the name at
//! all. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads
//! a request the same way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The default maximum session duration a role carries when nothing set one
/// (`CreateRole$MaxSessionDuration`'s documented default of one hour), as the
/// service reports it.
const DEFAULT_MAX_SESSION_DURATION: &str = "3600";

/// Catalog rows written out by hand — the service principal and the role name AWS
/// publishes for it — so this suite pins the catalog's CONTENT independently of the
/// table the service reads.
///
/// Source: the "Using service-linked roles for <service>" page of each service's own
/// user guide, the same documentation
/// `src/state.rs`'s `PUBLISHED_SERVICE_LINKED_ROLE_NAMES` cites. Rows span a name
/// that is a feature of a service rather than the service itself (AWS Backup Audit
/// Manager's report runner, under the three-label principal
/// `reports.backup.amazonaws.com`), the plain service's own role, a name nothing like
/// the principal's first label, and one carrying the resource type it scales.
const PUBLISHED_ROWS: &[(&str, &str)] = &[
    ("reports.backup.amazonaws.com", "AWSServiceRoleForBackupReports"),
    ("backup.amazonaws.com", "AWSServiceRoleForBackup"),
    ("ops.apigateway.amazonaws.com", "AWSServiceRoleForAPIGateway"),
    (
        "ecs.application-autoscaling.amazonaws.com",
        "AWSServiceRoleForApplicationAutoScaling_ECSService",
    ),
    // A name whose capitalisation the principal does not carry: the principal is
    // `codestar-notifications`, the published role is AWSServiceRoleForCodeStarNotifications
    // ("Using service-linked roles for AWS CodeStar Notifications").
    (
        "codestar-notifications.amazonaws.com",
        "AWSServiceRoleForCodeStarNotifications",
    ),
];

/// The `Role` members a read reports, pulled off one response body.
#[derive(Debug, PartialEq, Eq)]
struct ReadRole {
    role_name: String,
    path: String,
    role_id: String,
    arn: String,
    create_date: String,
    max_session_duration: String,
    trust_policy: String,
}

/// Read the role one response body carries.
fn read_role(body: &str) -> ReadRole {
    ReadRole {
        role_name: element(body, "RoleName"),
        path: element(body, "Path"),
        role_id: element(body, "RoleId"),
        arn: element(body, "Arn"),
        create_date: element(body, "CreateDate"),
        max_session_duration: element(body, "MaxSessionDuration"),
        trust_policy: crate::user_lifecycle::document_element(body, "AssumeRolePolicyDocument"),
    }
}

/// RULE: a role name AWS PUBLISHES for a service names the role that service keeps
/// provisioned, so `GetRole` on it answers 200 with a record derived from the service
/// principal the role is linked to — filed under `/aws-service-role/<principal>/`,
/// with the ARN of that path and name, the service's own `MaxSessionDuration`
/// default, and a trust policy whose one statement lets THAT principal assume it.
#[tokio::test]
async fn get_role_resolves_the_role_every_service_keeps_provisioned() {
    let router = router();
    let mut read_ids: Vec<String> = Vec::new();

    // Rows: every catalog entry, so the rule is asserted over the whole class of
    // published names rather than over the one a fixture happens to read.
    for (service_principal, published_name) in PUBLISHED_SERVICE_LINKED_ROLE_NAMES {
        let case = *published_name;
        let (status, body) = call(&router, "GetRole", &[("RoleName", published_name)]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        let role = read_role(&body);

        assert_eq!(
            role.role_name, *published_name,
            "case {case:?}: expected the published role name, got {:?}",
            role.role_name
        );
        let expected_path = format!("/aws-service-role/{service_principal}/");
        assert_eq!(
            role.path, expected_path,
            "case {case:?}: expected the service-linked role path {expected_path:?}, got {:?}",
            role.path
        );
        let expected_arn = format!(
            "arn:aws:iam::{}:role{expected_path}{published_name}",
            account_id()
        );
        assert_eq!(
            role.arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {:?}",
            role.arn
        );
        assert_eq!(
            role.max_session_duration, DEFAULT_MAX_SESSION_DURATION,
            "case {case:?}: expected the default maximum session duration, got {:?}",
            role.max_session_duration
        );

        // `Role$RoleId` is `idType`: len 16..128, pattern ^[\w]+$
        // (`smithy_query(query="shape", shape="Role")`), and it identifies the role,
        // so no two roles share one.
        assert!(
            (16..=128).contains(&role.role_id.chars().count()),
            "case {case:?}: expected a RoleId of 16..128 characters, got {:?}",
            role.role_id
        );
        assert!(
            role.role_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "case {case:?}: expected a RoleId matching ^[\\w]+$, got {:?}",
            role.role_id
        );
        assert!(
            !read_ids.contains(&role.role_id),
            "case {case:?}: expected a RoleId unique across roles, got the repeat {:?}",
            role.role_id
        );

        // `Role$CreateDate` is a timestamp, rendered by awsQuery in date-time form,
        // and for a role nothing in this account created it is the FIXED instant
        // `src/state.rs` documents (`SERVICE_PROVISIONED_ROLE_CREATE_MILLIS`, the
        // epoch) rather than a reading of the clock, so every read answers the same
        // value.
        let create_date = aws_smithy_types::DateTime::from_str(
            &role.create_date,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected an ISO-8601 CreateDate, got {:?}: {error}",
                role.create_date
            )
        });
        assert_eq!(
            (create_date.secs(), create_date.subsec_nanos()),
            (0, 0),
            "case {case:?}: expected the fixed epoch CreateDate a derived record carries, \
             got {:?}",
            role.create_date
        );

        // The trust policy is the role's link to its service: the one statement that
        // lets THAT principal assume it.
        let document: serde_json::Value = serde_json::from_str(&role.trust_policy)
            .unwrap_or_else(|error| panic!("case {case:?}: {error} in {:?}", role.trust_policy));
        let statement = &document["Statement"][0];
        assert_eq!(
            statement["Effect"],
            serde_json::json!("Allow"),
            "case {case:?}: expected an Allow statement, got {:?}",
            statement["Effect"]
        );
        assert_eq!(
            statement["Principal"]["Service"],
            serde_json::json!(*service_principal),
            "case {case:?}: expected the trust policy to name the service principal, got {:?}",
            statement["Principal"]["Service"]
        );
        assert_eq!(
            statement["Action"],
            serde_json::json!(["sts:AssumeRole"]),
            "case {case:?}: expected sts:AssumeRole to be granted, got {:?}",
            statement["Action"]
        );

        read_ids.push(role.role_id);
    }

    assert_eq!(
        read_ids.len(),
        PUBLISHED_SERVICE_LINKED_ROLE_NAMES.len(),
        "expected every catalog row to be read, got {}",
        read_ids.len()
    );
}

/// RULE: the published role name belongs to the SERVICE, not to the spelling of a
/// request, so every ASCII-case spelling of it resolves the same record and a second
/// read answers exactly what the first did — the stability a client polling for the
/// role depends on.
#[tokio::test]
async fn the_provisioned_role_is_the_same_record_however_it_is_read() {
    let router = router();

    for (service_principal, published_name) in PUBLISHED_ROWS {
        // Rows: the published spelling, lower case, upper case — IAM entity names
        // "are not distinguished by case" (`CreateRole$RoleName`) — and the
        // published spelling again, read after the others.
        let spellings = [
            (*published_name).to_string(),
            published_name.to_ascii_lowercase(),
            published_name.to_ascii_uppercase(),
            (*published_name).to_string(),
        ];
        let mut first: Option<ReadRole> = None;
        for spelling in &spellings {
            let case = format!("{published_name} as {spelling}");
            let (status, body) = call(&router, "GetRole", &[("RoleName", spelling.as_str())]).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
            let role = read_role(&body);
            assert_eq!(
                role.role_name, *published_name,
                "case {case:?}: expected the role reported under its published name, got {:?}",
                role.role_name
            );
            assert_eq!(
                role.path,
                format!("/aws-service-role/{service_principal}/"),
                "case {case:?}: expected the principal's service-linked path, got {:?}",
                role.path
            );
            match &first {
                None => first = Some(role),
                Some(expected) => assert_eq!(
                    &role, expected,
                    "case {case:?}: expected the same record every read, got {role:?}"
                ),
            }
        }
    }
}

/// RULE: a read reports; it stores nothing — so reading the roles the services keep
/// provisioned leaves the account's own collection of roles exactly as it was, and
/// `ListRoles` still enumerates only what the ACCOUNT holds.
#[tokio::test]
async fn reading_a_provisioned_role_leaves_the_accounts_own_roles_untouched() {
    let router = router();

    let (status, body) = call(&router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "expected ListRoles 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "RoleName"),
        Vec::<String>::new(),
        "expected an account holding no roles of its own to list none, got {:?}",
        elements(&body, "RoleName")
    );

    // Rows: every catalog row is read, twice, so any write a read performed would
    // show up in the listing below.
    for (_, published_name) in PUBLISHED_SERVICE_LINKED_ROLE_NAMES {
        for _ in 0..2 {
            let (status, body) = call(&router, "GetRole", &[("RoleName", published_name)]).await;
            assert_eq!(
                status, 200,
                "case {published_name:?}: expected 200, got {status}: {body}"
            );
        }
    }

    let (status, body) = call(&router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "expected ListRoles 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "RoleName"),
        Vec::<String>::new(),
        "a read stored a role: expected the account's roles to be untouched, got {:?}",
        elements(&body, "RoleName")
    );

    // And the account's OWN record for such a name takes precedence once it creates
    // one: the create is what puts a role in the account's collection, and every
    // later read reports that stored record.
    for (service_principal, published_name) in PUBLISHED_ROWS {
        let router = crate::user_lifecycle::router();
        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", service_principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {published_name:?}: expected the create to succeed, got {status}: {created}"
        );
        let created_id = element(&created, "RoleId");
        let (status, fetched) = call(&router, "GetRole", &[("RoleName", published_name)]).await;
        assert_eq!(
            status, 200,
            "case {published_name:?}: expected GetRole 200, got {status}: {fetched}"
        );
        assert_eq!(
            element(&fetched, "RoleId"),
            created_id,
            "case {published_name:?}: expected the account's own record to be reported, got {:?}",
            element(&fetched, "RoleId")
        );
        let (status, listed) = call(&router, "ListRoles", &[]).await;
        assert_eq!(
            status, 200,
            "case {published_name:?}: expected ListRoles 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "RoleName"),
            vec![(*published_name).to_string()],
            "case {published_name:?}: expected the created role to be the account's only one, \
             got {:?}",
            elements(&listed, "RoleName")
        );
    }
}

/// RULE: `NoSuchEntity` (HTTP 404) is reserved for a name NOTHING holds a role
/// under — neither the account nor any service — so a name no service publishes is
/// still not found however much it looks like one that is.
#[tokio::test]
async fn a_name_no_service_publishes_is_still_not_found() {
    let router = router();

    // Rows: near-misses of a published name (one character short, one character
    // long, the feature name split off), the name `CreateServiceLinkedRole` DERIVES
    // for a principal AWS publishes no name for, the bare prefix, a name of no
    // particular shape, and one at the longest length `roleNameType` admits (64).
    let longest = "r".repeat(64);
    let absent = [
        "AWSServiceRoleForBackupReport",
        "AWSServiceRoleForBackupReportss",
        "AWSServiceRoleForReports",
        "AWSServiceRoleForNetworkmonitor",
        "AWSServiceRoleFor",
        "nobody",
        longest.as_str(),
    ];
    for name in absent {
        let (status, body) = call(&router, "GetRole", &[("RoleName", name)]).await;
        assert_eq!(status, 404, "case {name:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: the catalog of published names is AWS's, not this emulator's — each row
/// names the role that service's own documentation names — so a read of a row
/// written out here resolves to exactly that role.
#[tokio::test]
async fn the_published_rows_name_the_roles_aws_documents() {
    let router = router();

    for (service_principal, published_name) in PUBLISHED_ROWS {
        assert!(
            PUBLISHED_SERVICE_LINKED_ROLE_NAMES
                .iter()
                .any(|(principal, name)| principal == service_principal && name == published_name),
            "case {published_name:?}: expected the catalog to carry the documented row for \
             {service_principal:?}"
        );
        let (status, body) = call(&router, "GetRole", &[("RoleName", published_name)]).await;
        assert_eq!(
            status, 200,
            "case {published_name:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Path"),
            format!("/aws-service-role/{service_principal}/"),
            "case {published_name:?}: expected the role to be linked to {service_principal:?}, \
             got {:?}",
            element(&body, "Path")
        );
    }
}
