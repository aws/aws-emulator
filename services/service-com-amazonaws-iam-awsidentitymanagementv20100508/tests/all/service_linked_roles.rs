// SPDX-License-Identifier: CC0-1.0

//! Service-linked roles, end to end over the awsQuery wire.
//!
//! `CreateServiceLinkedRole` mints the role a service links to itself, and
//! `GetRole` reads it back. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and
//! reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! whether AWS publishes a role name for the service principal, the principal's
//! label count and case, whether a `CustomSuffix` is supplied, whether a
//! `Description` is, and the length of the name the request derives.

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// One create: the service principal, the optional `CustomSuffix` and
/// `Description`, and the role name the request therefore names.
///
/// The expected name is written out per row rather than recomputed, so the table
/// pins the naming instead of restating it: the name AWS publishes for that service
/// where it publishes one, the service-provided prefix `AWSServiceRoleFor` plus the
/// principal's first label capitalised where it does not, and `_` plus the custom
/// suffix when one is supplied.
struct Case {
    service_principal: &'static str,
    custom_suffix: Option<&'static str>,
    description: Option<&'static str>,
    expected_role_name: &'static str,
}

/// Rows spanning principals whose role name AWS publishes (including names nothing
/// like the principal's first label, and one carrying a `_` of its own) and
/// principals it does not, two- and three-label principals, an already-capitalised
/// label, a label carrying `_`, suffixes of different shapes, and both presence and
/// absence of `Description`.
const CASES: &[Case] = &[
    // Published: AWS SSM for SAP's role, the one a client confirms with `GetRole`
    // before `RegisterApplication`.
    Case {
        service_principal: "ssm-sap.amazonaws.com",
        custom_suffix: None,
        description: None,
        expected_role_name: "AWSServiceRoleForAWSSSMForSAP",
    },
    // Published, and nothing like the principal's first label.
    Case {
        service_principal: "application-signals.amazonaws.com",
        custom_suffix: None,
        description: Some("Allows CloudWatch Application Signals to discover"),
        expected_role_name: "AWSServiceRoleForCloudWatchApplicationSignals",
    },
    // Published, with a `CustomSuffix` joined to it.
    Case {
        service_principal: "autoscaling.amazonaws.com",
        custom_suffix: Some("debug"),
        description: None,
        expected_role_name: "AWSServiceRoleForAutoScaling_debug",
    },
    // Published, three-label principal whose role name names the service rather
    // than the principal's first label (`ops`).
    Case {
        service_principal: "ops.apigateway.amazonaws.com",
        custom_suffix: Some("1"),
        description: Some("A three-label principal"),
        expected_role_name: "AWSServiceRoleForAPIGateway_1",
    },
    // Published, and already carrying a `_` of its own.
    Case {
        service_principal: "rds.application-autoscaling.amazonaws.com",
        custom_suffix: None,
        description: None,
        expected_role_name: "AWSServiceRoleForApplicationAutoScaling_RDSCluster",
    },
    Case {
        service_principal: "elasticbeanstalk.amazonaws.com",
        custom_suffix: None,
        description: Some("Allows Elastic Beanstalk to manage resources"),
        expected_role_name: "AWSServiceRoleForElasticBeanstalk",
    },
    // Not published: the derived fallback, all this emulator can know for a
    // principal no published name covers.
    Case {
        service_principal: "networkmonitor.amazonaws.com",
        custom_suffix: None,
        description: None,
        expected_role_name: "AWSServiceRoleForNetworkmonitor",
    },
    Case {
        service_principal: "Mediaconvert.amazonaws.com",
        custom_suffix: Some("Fleet_7"),
        description: None,
        expected_role_name: "AWSServiceRoleForMediaconvert_Fleet_7",
    },
    Case {
        service_principal: "sms_voice.amazonaws.com",
        custom_suffix: None,
        description: None,
        expected_role_name: "AWSServiceRoleForSms_voice",
    },
];

/// Principals whose service-linked role name AWS PUBLISHES, with that name.
///
/// A published name belongs to the SERVICE rather than to the spelling of its
/// principal, so every ASCII-case spelling of the principal names the same role.
/// Source: the same per-service "Using service-linked roles" documentation
/// `src/ops/create_service_linked_role.rs` cites for its own table; these four rows
/// are not the ones [`CASES`] reads, so the two tables do not pin one entry twice.
const PUBLISHED: &[(&str, &str)] = &[
    ("ecs.amazonaws.com", "AWSServiceRoleForECS"),
    ("lakeformation.amazonaws.com", "AWSServiceRoleForLakeFormationDataAccess"),
    (
        "replication.cassandra.amazonaws.com",
        "AWSServiceRoleForKeyspacesReplication",
    ),
    (
        "dynamodb.application-autoscaling.amazonaws.com",
        "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable",
    ),
];

/// The default maximum session duration a role created without one carries, as
/// the service holds it (`CreateRole$MaxSessionDuration`'s documented default of
/// one hour).
const DEFAULT_MAX_SESSION_DURATION: &str = "3600";

/// The form parameters one row's create is driven with.
fn create_params(case: &Case) -> Vec<(&'static str, &'static str)> {
    let mut params = vec![("AWSServiceName", case.service_principal)];
    if let Some(suffix) = case.custom_suffix {
        params.push(("CustomSuffix", suffix));
    }
    if let Some(description) = case.description {
        params.push(("Description", description));
    }
    params
}

/// RULE: `CreateServiceLinkedRole` mints and persists the role the request names —
/// the name AWS publishes for that service principal, or `AWSServiceRoleFor<Label>`
/// derived from the principal when AWS publishes none, with `_<CustomSuffix>` when
/// one is supplied,
/// filed under `/aws-service-role/<service principal>/` with the ARN built from
/// that path and name, carrying a trust policy that lets the named service
/// principal assume it — and a later `GetRole` returns that STORED record rather
/// than a value recomputed from the reading request.
#[tokio::test]
async fn create_service_linked_role_persists_the_role_and_get_role_returns_it() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for case in CASES {
        let label = case.service_principal;
        let (status, created) = call(&router, "CreateServiceLinkedRole", &create_params(case)).await;
        assert_eq!(status, 200, "case {label:?}: expected 200, got {status}: {created}");

        assert_eq!(
            element(&created, "RoleName"),
            case.expected_role_name,
            "case {label:?}: expected the role name {:?}, got {:?}",
            case.expected_role_name,
            element(&created, "RoleName")
        );
        let expected_path = format!("/aws-service-role/{}/", case.service_principal);
        assert_eq!(
            element(&created, "Path"),
            expected_path,
            "case {label:?}: expected the service-linked role path {expected_path:?}, got {:?}",
            element(&created, "Path")
        );
        let expected_arn = format!(
            "arn:aws:iam::{}:role{expected_path}{}",
            account_id(),
            case.expected_role_name
        );
        assert_eq!(
            element(&created, "Arn"),
            expected_arn,
            "case {label:?}: expected the ARN {expected_arn:?}, got {:?}",
            element(&created, "Arn")
        );

        // `Role$RoleId` is `idType`: len 16..128, pattern ^[\w]+$
        // (`smithy_query(query="shape", shape="Role")`).
        let role_id = element(&created, "RoleId");
        assert!(
            (16..=128).contains(&role_id.chars().count()),
            "case {label:?}: expected a RoleId of 16..128 characters, got {role_id:?}"
        );
        assert!(
            role_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "case {label:?}: expected a RoleId matching ^[\\w]+$, got {role_id:?}"
        );
        assert!(
            !minted_ids.contains(&role_id),
            "case {label:?}: expected a RoleId unique across roles, got the repeat {role_id:?}"
        );

        // `Role$CreateDate` is a timestamp, rendered by awsQuery in date-time form.
        let create_date = element(&created, "CreateDate");
        assert!(
            aws_smithy_types::DateTime::from_str(
                &create_date,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {label:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
        );

        assert_eq!(
            element(&created, "MaxSessionDuration"),
            DEFAULT_MAX_SESSION_DURATION,
            "case {label:?}: expected the default maximum session duration, got {:?}",
            element(&created, "MaxSessionDuration")
        );

        // The trust policy is the role's link to its service: the one statement
        // that lets THAT principal assume it.
        let policy = crate::user_lifecycle::document_element(&created, "AssumeRolePolicyDocument");
        let document: serde_json::Value = serde_json::from_str(&policy)
            .unwrap_or_else(|error| panic!("case {label:?}: {error} in {policy:?}"));
        let statement = &document["Statement"][0];
        assert_eq!(
            statement["Effect"],
            serde_json::json!("Allow"),
            "case {label:?}: expected an Allow statement, got {:?}",
            statement["Effect"]
        );
        assert_eq!(
            statement["Principal"]["Service"],
            serde_json::json!(case.service_principal),
            "case {label:?}: expected the trust policy to name the service principal, got {:?}",
            statement["Principal"]["Service"]
        );
        assert_eq!(
            statement["Action"],
            serde_json::json!(["sts:AssumeRole"]),
            "case {label:?}: expected sts:AssumeRole to be granted, got {:?}",
            statement["Action"]
        );

        // A `Description` is reported when it was supplied and by neither call
        // when it was not.
        match case.description {
            Some(description) => assert_eq!(
                element(&created, "Description"),
                description,
                "case {label:?}: expected the supplied Description, got {:?}",
                element(&created, "Description")
            ),
            None => assert!(
                elements(&created, "Description").is_empty(),
                "case {label:?}: expected no Description, got {:?}",
                elements(&created, "Description")
            ),
        }

        // The read returns the stored record, and finds it however the request
        // spells the name: IAM entity names are not distinguished by case.
        for spelling in [
            case.expected_role_name.to_string(),
            case.expected_role_name.to_ascii_lowercase(),
            case.expected_role_name.to_ascii_uppercase(),
        ] {
            let (status, fetched) = call(&router, "GetRole", &[("RoleName", spelling.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {label:?}/{spelling:?}: expected GetRole 200, got {status}: {fetched}"
            );
            for member in [
                "RoleName",
                "Path",
                "RoleId",
                "Arn",
                "CreateDate",
                "MaxSessionDuration",
                "AssumeRolePolicyDocument",
            ] {
                assert_eq!(
                    element(&fetched, member),
                    element(&created, member),
                    "case {label:?}/{spelling:?}: expected GetRole to return the stored {member}, \
                     got {:?}",
                    element(&fetched, member)
                );
            }
            assert_eq!(
                elements(&fetched, "Description"),
                elements(&created, "Description"),
                "case {label:?}/{spelling:?}: expected GetRole to return the stored Description, \
                 got {:?}",
                elements(&fetched, "Description")
            );
        }

        minted_ids.push(role_id);
    }
}

/// RULE: a role name AWS publishes for a service is a property of THAT SERVICE, so
/// every create addressing it names the same role whatever ASCII case the principal
/// is spelled in — and the role is retrievable by that published name, which is the
/// name a client that follows a service's documented bootstrap reads back.
#[tokio::test]
async fn the_published_role_name_is_the_one_aws_publishes_whatever_case_the_principal_is_spelled_in()
{
    for (principal, published) in PUBLISHED {
        // Rows: the principal as published, upper-cased, and with its first
        // character upper-cased — three spellings addressing one service. Each gets
        // its own account, because a second create of one role name is a duplicate.
        for spelling in [
            (*principal).to_string(),
            principal.to_ascii_uppercase(),
            {
                let mut chars = principal.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            },
        ] {
            let router = router();
            let (status, created) = call(
                &router,
                "CreateServiceLinkedRole",
                &[("AWSServiceName", spelling.as_str())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {spelling:?}: expected 200, got {status}: {created}"
            );
            assert_eq!(
                element(&created, "RoleName"),
                *published,
                "case {spelling:?}: expected the published role name {published:?}, got {:?}",
                element(&created, "RoleName")
            );

            let (status, fetched) = call(&router, "GetRole", &[("RoleName", *published)]).await;
            assert_eq!(
                status, 200,
                "case {spelling:?}: expected GetRole on {published:?} to return 200, \
                 got {status}: {fetched}"
            );
            assert_eq!(
                element(&fetched, "RoleId"),
                element(&created, "RoleId"),
                "case {spelling:?}: expected GetRole to return the created role, got {fetched}"
            );
        }
    }
}

/// RULE: a role name is taken only once in an account, so a second create for the
/// same service and suffix is refused with `InvalidInput` (HTTP 400) and leaves
/// the stored role untouched, while a create carrying a DIFFERENT suffix mints a
/// second role for the same service.
#[tokio::test]
async fn a_second_create_for_the_same_service_needs_a_different_suffix() {
    let router = router();

    for case in CASES {
        let label = case.service_principal;
        let (status, created) = call(&router, "CreateServiceLinkedRole", &create_params(case)).await;
        assert_eq!(status, 200, "case {label:?}: expected 200, got {status}: {created}");
        let role_id = element(&created, "RoleId");

        // Rows: the identical request, and one differing only in the CASE of the
        // service label, which derives the same role name — IAM entity names are
        // not distinguished by case.
        for repeat in [
            case.service_principal.to_string(),
            case.service_principal.to_ascii_uppercase(),
        ] {
            let mut params: Vec<(&str, &str)> = create_params(case);
            params[0] = ("AWSServiceName", repeat.as_str());
            let (status, body) = call(&router, "CreateServiceLinkedRole", &params).await;
            assert_eq!(
                status, 400,
                "case {label:?}/{repeat:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {label:?}/{repeat:?}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        }

        let (status, fetched) = call(
            &router,
            "GetRole",
            &[("RoleName", case.expected_role_name)],
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: expected GetRole 200, got {status}: {fetched}");
        assert_eq!(
            element(&fetched, "RoleId"),
            role_id,
            "case {label:?}: a refused duplicate replaced the stored role, got {:?}",
            element(&fetched, "RoleId")
        );

        // A different suffix is a different role name, so it is accepted.
        let mut params = create_params(case);
        params.retain(|(name, _)| *name != "CustomSuffix");
        params.push(("CustomSuffix", "second"));
        let (status, body) = call(&router, "CreateServiceLinkedRole", &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a different suffix to be accepted, got {status}: {body}"
        );
        assert_ne!(
            element(&body, "RoleName"),
            case.expected_role_name,
            "case {label:?}: expected the suffixed role to carry its own name"
        );
    }
}

/// RULE: `AWSServiceName` names a SERVICE PRINCIPAL — a DNS-style name of at least
/// two non-empty dot-separated labels — so a value that names no service is
/// refused with `InvalidInput` (HTTP 400) and no role is created.
#[tokio::test]
async fn a_name_that_is_not_a_service_principal_is_refused() {
    let router = router();

    // Rows: a bare label, a trailing empty label, a leading empty label, an inner
    // empty label, and nothing but separators. Each is inside the modelled
    // `groupNameType` pattern ^[\w+=,.@-]+$, so each reaches the handler.
    let refused = [
        "networkmonitor",
        "networkmonitor.",
        ".amazonaws.com",
        "networkmonitor..com",
        "..",
    ];
    for name in refused {
        let (status, body) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", name)],
        )
        .await;
        assert_eq!(status, 400, "case {name:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {name:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }

    // Nothing was stored for any of them: the name the refused requests would
    // have derived is still absent.
    for absent in [
        "AWSServiceRoleForNetworkmonitor",
        "AWSServiceRoleForAmazonaws",
        "AWSServiceRoleFor",
    ] {
        let (status, body) = call(&router, "GetRole", &[("RoleName", absent)]).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: a refused create stored the role anyway, got {status}: {body}"
        );
    }

    // And a principal that IS DNS-style is accepted, including one with many
    // labels.
    for accepted in ["networkmonitor.amazonaws.com", "a.b.c.d.amazonaws.com"] {
        let (status, body) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", accepted)],
        )
        .await;
        assert_eq!(status, 200, "case {accepted:?}: expected 200, got {status}: {body}");
    }
}

/// RULE: the name a create derives has to be a role name, so one past
/// `roleNameType`'s 64-character bound is refused with `InvalidInput` (HTTP 400),
/// while a name AT the bound is created.
#[tokio::test]
async fn a_derived_name_past_the_published_bound_is_refused() {
    const MAX_ROLE_NAME: usize =
        service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_ROLE_NAME_CHARACTERS;
    /// `AWSServiceRoleFor`, the service-provided prefix every derived name starts
    /// with.
    const PREFIX: usize = 17;

    let router = router();

    // The label length that lands the derived name exactly ON the bound, and the
    // one that lands it one past.
    let at_bound = "a".repeat(MAX_ROLE_NAME - PREFIX);
    let past_bound = "a".repeat(MAX_ROLE_NAME - PREFIX + 1);
    // The same bound reached through a `CustomSuffix` instead: the separator plus
    // the suffix counts toward the name.
    let suffix_at_bound = "s".repeat(MAX_ROLE_NAME - PREFIX - "svc".len() - 1);
    let suffix_past_bound = "s".repeat(MAX_ROLE_NAME - PREFIX - "svc".len());
    let at_bound_principal = format!("{at_bound}.amazonaws.com");
    let past_bound_principal = format!("{past_bound}.amazonaws.com");

    let cases: &[(&str, Option<&str>, u16)] = &[
        (at_bound_principal.as_str(), None, 200),
        (past_bound_principal.as_str(), None, 400),
        ("svc.amazonaws.com", Some(suffix_at_bound.as_str()), 200),
        ("svc.amazonaws.com", Some(suffix_past_bound.as_str()), 400),
    ];

    for (principal, suffix, expected) in cases {
        let mut params = vec![("AWSServiceName", *principal)];
        if let Some(suffix) = suffix {
            params.push(("CustomSuffix", suffix));
        }
        let (status, body) = call(&router, "CreateServiceLinkedRole", &params).await;
        let case = format!("{principal} + {suffix:?}");
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        } else {
            assert_eq!(
                element(&body, "RoleName").chars().count(),
                MAX_ROLE_NAME,
                "case {case:?}: expected a name at the bound, got {:?}",
                element(&body, "RoleName")
            );
        }
    }
}

/// RULE: `GetRole` reports a role the account holds; a name that neither the account
/// holds nor any service publishes for its own service-linked role is a
/// `NoSuchEntity` (HTTP 404) rather than an invented role. (The roles a service keeps
/// provisioned under the names AWS publishes are the subject of
/// `tests/all/service_provisioned_roles.rs`; every name below is outside that
/// catalog.)
#[tokio::test]
async fn reading_a_role_the_account_does_not_hold_is_not_found() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateServiceLinkedRole",
        &[("AWSServiceName", "networkmonitor.amazonaws.com")],
    )
    .await;
    assert_eq!(status, 200, "expected the fixture create to succeed, got {status}: {body}");

    // Rows: a name never created, two near-misses of the one that was, and a name
    // at the longest length `roleNameType` admits (64).
    let longest = "r".repeat(64);
    for absent in [
        "nobody",
        "AWSServiceRoleForNetworkmonito",
        "AWSServiceRoleForNetworkmonitors",
        longest.as_str(),
    ] {
        let (status, body) = call(&router, "GetRole", &[("RoleName", absent)]).await;
        assert_eq!(status, 404, "case {absent:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}
