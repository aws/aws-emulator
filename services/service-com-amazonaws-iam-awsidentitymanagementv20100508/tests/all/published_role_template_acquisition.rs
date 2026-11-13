// SPDX-License-Identifier: CC0-1.0

//! `AcquireRole` against the role templates a SERVICE publishes, end to end over the
//! awsQuery wire.
//!
//! The rules asserted here:
//!
//! 1. A role template a service publishes exists in EVERY account without anything having
//!    recorded it — "indicates that the role template is managed by an Amazon Web Services
//!    service" (`RoleTemplateVersion$ManagedByType`, whose `managedByTypeType` declares the
//!    single value `Service`) and no operation of this API creates one — so an acquisition
//!    naming one creates the role that template version describes in an account that has
//!    recorded nothing: its rendered name, the template's path, trust policy, description,
//!    maximum session duration and managed policies, and the `SourceRoleTemplate` that
//!    names the template it came from. That role is then the one the role reads report,
//!    the one a detach changes and the one a delete removes.
//! 2. An ARN in that namespace no service publishes a template under, and a minor version
//!    no published template carries, reference "a resource entity that does not exist", so
//!    the call is refused `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`)
//!    and nothing is created.
//! 3. A published template's parameter definitions are the ones the rendering is held to:
//!    a value supplied for a name it defines no parameter under, and a required parameter
//!    nothing supplies a value for, are both values the template cannot be rendered with.
//! 4. A template the ACCOUNT holds under the same ARN takes precedence over the published
//!    one, so a recorded template is not superseded by the catalog.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: which
//! published template is acquired (the AWS Backup default service role, filed under the
//! service-role path with two managed policies, and the IAM power-user template at the
//! account root with one, whose trust policy names a principal the request supplies), how
//! the addressed template or version fails to resolve, and which half of the parameter
//! contract the request breaks.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share; `acquire` (the `ReplacementValues` map encoding) and `seed` (a
//! template written straight into the account's own store) are the ones
//! `tests/all/role_template_acquisition.rs` owns.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::RoleTemplateVersion;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_template_acquisition::{acquire, seed};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router_with_engine};

/// The ARN the AWS Backup default service role template is published under, as
/// `PublishedRoleTemplate::arn` builds one:
/// `arn:aws:iam::aws:role-template/<managing service>/<name>:<major version>`.
const BACKUP_TEMPLATE_ARN: &str =
    "arn:aws:iam::aws:role-template/backup.amazonaws.com/AWSBackupDefaultServiceRoleTemplate:1";

/// The ARN the IAM power-user role template is published under.
const POWER_USER_TEMPLATE_ARN: &str =
    "arn:aws:iam::aws:role-template/iam.amazonaws.com/PowerUserRoleTemplate:1";

/// The account the AWS Backup template's trust policy is rendered for in the rows below —
/// a parameter value the REQUEST supplies, not this account's own id.
const SOURCE_ACCOUNT: &str = "111122223333";

/// One row of the acquisition table: which published template is acquired, with which
/// replacement values, and the role the template version describes.
struct Acquired {
    case: &'static str,
    template_arn: &'static str,
    /// The minor version the request names, or `None` to leave the resolution to the
    /// template's default minor version.
    minor_version: Option<i32>,
    values: &'static [(&'static str, &'static [&'static str])],
    role_name: &'static str,
    path: &'static str,
    description: &'static str,
    /// The trust policy the template renders with `values`.
    trust_policy: &'static str,
    /// "The maximum session duration (in seconds) for roles that are created from this
    /// template" (`RoleTemplateVersion$MaxSessionDuration`).
    max_session_duration: &'static str,
    /// The managed policies the template attaches, in the order this service lists a
    /// role's attachments in (by ARN).
    managed_policies: &'static [&'static str],
    /// The minor version the created role's `SourceRoleTemplate` must name.
    source_minor_version: &'static str,
}

/// The published templates this suite acquires, one row each.
///
/// The AWS Backup row is the service-role case: a template filed under the path AWS
/// reserves for service roles, whose trust policy names the publishing service as the
/// principal and is rendered with the account the service acts for, and which attaches TWO
/// managed policies. The power-user row is the other shape the catalog holds: a template at
/// the account root whose trust policy names a principal the REQUEST supplies, carrying one
/// managed policy and no description of the publishing service's own wording.
const ROWS: &[Acquired] = &[
    Acquired {
        case: "the AWS Backup default service role, at the template's default minor version",
        template_arn: BACKUP_TEMPLATE_ARN,
        minor_version: None,
        values: &[
            ("RoleName", &["backup-default-service-role"]),
            ("accountId", &[SOURCE_ACCOUNT]),
        ],
        role_name: "backup-default-service-role",
        path: "/service-role/",
        description: "Standard service role the provides AWS Backup service permission to create and restore backups on your behalf across AWS services",
        trust_policy: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"backup.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringEquals":{"aws:SourceAccount":"111122223333"}}}]}"#,
        max_session_duration: "3600",
        managed_policies: &[
            "arn:aws:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForBackup",
            "arn:aws:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForRestores",
        ],
        source_minor_version: "0",
    },
    Acquired {
        case: "the IAM power-user template, at the minor version the request names",
        template_arn: POWER_USER_TEMPLATE_ARN,
        minor_version: Some(0),
        values: &[
            ("AWSServiceName", &["lambda.amazonaws.com"]),
            ("RoleName", &["power_user.role-1"]),
        ],
        role_name: "power_user.role-1",
        path: "/",
        description: "Power user role for lambda.amazonaws.com",
        trust_policy: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
        max_session_duration: "3600",
        managed_policies: &["arn:aws:iam::aws:policy/PowerUserAccess"],
        source_minor_version: "0",
    },
];

/// One `GetRole` call.
async fn get_role(router: &MultiServiceRouter, role_name: &str) -> (u16, String) {
    call(router, "GetRole", &[("RoleName", role_name)]).await
}

/// The role names the account reports through `ListRoles`: what a refused call must leave
/// exactly as it found it, read over the wire rather than out of the store.
async fn listed_roles(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "ListRoles must succeed, got {status}: {body}");
    elements(&body, "RoleName")
}

/// RULE 1: a published role template needs nothing recorded in the account, so acquiring
/// from one creates the role that template version describes — and that role is what the
/// role reads report, what a detach changes and what a delete removes.
#[tokio::test]
async fn a_published_role_template_creates_the_role_it_describes() {
    for row in ROWS {
        let case = row.case;
        // Nothing is seeded: the account holds no template of its own at all.
        let (router, _engine) = router_with_engine();

        let (status, body) =
            acquire(&router, None, row.template_arn, row.minor_version, row.values).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the published template to create a role, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleName"),
            row.role_name,
            "case {case:?}: expected the rendered role name {:?}, got {:?}",
            row.role_name,
            element(&body, "RoleName")
        );

        // The round trip: the role the acquisition answered with is the one the read
        // reports, member for member.
        let (status, read) = get_role(&router, row.role_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the acquired role must be readable, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "Path"),
            row.path,
            "case {case:?}: expected the template's path {:?}, got {:?}",
            row.path,
            element(&read, "Path")
        );
        assert_eq!(
            element(&read, "Arn"),
            format!(
                "arn:aws:iam::{}:role{}{}",
                account_id(),
                row.path,
                row.role_name
            ),
            "case {case:?}: expected the role's ARN at the template's path, got {:?}",
            element(&read, "Arn")
        );
        assert_eq!(
            element(&read, "Description"),
            row.description,
            "case {case:?}: expected the rendered description {:?}, got {:?}",
            row.description,
            element(&read, "Description")
        );
        assert_eq!(
            element(&read, "MaxSessionDuration"),
            row.max_session_duration,
            "case {case:?}: expected the template's maximum session duration {:?}, got {:?}",
            row.max_session_duration,
            element(&read, "MaxSessionDuration")
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&read, "AssumeRolePolicyDocument"),
            row.trust_policy,
            "case {case:?}: expected the template's trust policy rendered with the supplied \
             values, got {:?}",
            crate::user_lifecycle::document_element(&read, "AssumeRolePolicyDocument")
        );
        // "Contains information about the role template that this role was created from.
        // This member is present only for roles created with AcquireRole"
        // (`Role$SourceRoleTemplate`).
        assert_eq!(
            element(&read, "TemplateArn"),
            row.template_arn,
            "case {case:?}: expected the role to name the template it came from, got {:?}",
            element(&read, "TemplateArn")
        );
        assert_eq!(
            element(&read, "TemplateMinorVersion"),
            row.source_minor_version,
            "case {case:?}: expected the resolved minor version {:?}, got {:?}",
            row.source_minor_version,
            element(&read, "TemplateMinorVersion")
        );

        // The managed policies the template attaches are the ones the role carries, and the
        // template embeds no inline policy, so the role holds none.
        let (status, attached) =
            call(&router, "ListAttachedRolePolicies", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the role's attachments to be listable, got {status}: \
             {attached}"
        );
        assert_eq!(
            elements(&attached, "PolicyArn"),
            row.managed_policies,
            "case {case:?}: expected the template's managed policies {:?}, got {:?}",
            row.managed_policies,
            elements(&attached, "PolicyArn")
        );
        let (status, inline) =
            call(&router, "ListRolePolicies", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the role's inline policies to be listable, got {status}: \
             {inline}"
        );
        assert!(
            elements(&inline, "member").is_empty(),
            "case {case:?}: expected no inline policy on a role the template embedded none in, \
             got {:?}",
            elements(&inline, "member")
        );

        // The teardown path: each attachment is detachable and the role is deletable once
        // nothing is attached to it.
        for policy_arn in row.managed_policies {
            let (status, detached) = call(
                &router,
                "DetachRolePolicy",
                &[("RoleName", row.role_name), ("PolicyArn", policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {policy_arn} to be detachable, got {status}: {detached}"
            );
        }
        let (status, attached) =
            call(&router, "ListAttachedRolePolicies", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the emptied attachment set to be listable, got {status}: \
             {attached}"
        );
        assert!(
            elements(&attached, "PolicyArn").is_empty(),
            "case {case:?}: expected no attachment left, got {:?}",
            elements(&attached, "PolicyArn")
        );

        let (status, deleted) =
            call(&router, "DeleteRole", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the acquired role to be deletable, got {status}: {deleted}"
        );
        let (status, gone) = get_role(&router, row.role_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted role to be gone, got {status}: {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity after the delete, got {:?}: {gone}",
            error_code(&gone)
        );
    }
}

/// RULE 2: an ARN no service publishes a template under, and a minor version no published
/// template carries, name no role template, so the call is refused as the missing entity
/// and creates nothing.
#[tokio::test]
async fn an_unpublished_template_or_minor_version_names_no_template() {
    // Rows: an unpublished template name under a publishing service, a published template
    // at another major version, a published template name under another service, and the
    // two kinds of minor version a published entry does not carry.
    let rows: Vec<(&str, String, Option<i32>)> = vec![
        (
            "a template name no service publishes",
            "arn:aws:iam::aws:role-template/backup.amazonaws.com/AWSBackupNeverPublishedTemplate:1"
                .to_string(),
            None,
        ),
        (
            "a published template at a major version that is not published",
            "arn:aws:iam::aws:role-template/backup.amazonaws.com/AWSBackupDefaultServiceRoleTemplate:2"
                .to_string(),
            None,
        ),
        (
            "a published template name under another service",
            "arn:aws:iam::aws:role-template/s3.amazonaws.com/AWSBackupDefaultServiceRoleTemplate:1"
                .to_string(),
            None,
        ),
        (
            "a minor version past the one the published template carries",
            BACKUP_TEMPLATE_ARN.to_string(),
            Some(1),
        ),
        (
            "a minor version far past it",
            POWER_USER_TEMPLATE_ARN.to_string(),
            Some(97),
        ),
    ];

    for (case, template_arn, minor_version) in rows {
        let (router, _engine) = router_with_engine();
        let before = listed_roles(&router).await;

        let (status, body) = acquire(
            &router,
            None,
            &template_arn,
            minor_version,
            &[
                ("RoleName", &["probe-role"]),
                ("accountId", &[SOURCE_ACCOUNT]),
            ],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Role template not found: {template_arn}"),
            "case {case:?}: the refusal must name the template the request addressed, got {:?}",
            element(&body, "Message")
        );
        assert_eq!(
            listed_roles(&router).await,
            before,
            "case {case:?}: a refused call creates no role, got {:?}",
            listed_roles(&router).await
        );
    }
}

/// RULE 3: a published template's parameter definitions are what the rendering is held to —
/// a value supplied for a name it defines no parameter under, and a required parameter
/// nothing supplies a value for, are values the template cannot be rendered with.
#[tokio::test]
async fn a_published_template_is_rendered_only_with_the_parameters_it_defines() {
    // Rows: an undefined parameter name on each published template, and each template's
    // required parameters left unsupplied one at a time.
    struct Row {
        case: &'static str,
        template_arn: &'static str,
        values: &'static [(&'static str, &'static [&'static str])],
        expected: &'static str,
    }

    let rows: &[Row] = &[
        Row {
            case: "a value for a parameter the AWS Backup template does not define",
            template_arn: BACKUP_TEMPLATE_ARN,
            values: &[
                ("RoleName", &["backup-default-service-role"]),
                ("accountId", &["111122223333"]),
                ("Region", &["us-east-1"]),
            ],
            expected: "The role template version defines no parameter named Region",
        },
        Row {
            case: "a value for a parameter the power-user template does not define",
            template_arn: POWER_USER_TEMPLATE_ARN,
            values: &[
                ("RoleName", &["power-user-role"]),
                ("accountId", &["111122223333"]),
            ],
            expected: "The role template version defines no parameter named accountId",
        },
        Row {
            case: "no value for the account the AWS Backup template's trust policy names",
            template_arn: BACKUP_TEMPLATE_ARN,
            values: &[("RoleName", &["backup-default-service-role"])],
            expected: "No replacement value was supplied for the required role template \
                       parameter accountId",
        },
        Row {
            case: "no value for the principal the power-user template's trust policy names",
            template_arn: POWER_USER_TEMPLATE_ARN,
            values: &[("RoleName", &["power-user-role"])],
            expected: "No replacement value was supplied for the required role template \
                       parameter AWSServiceName",
        },
        Row {
            case: "no name for the role the power-user template creates",
            template_arn: POWER_USER_TEMPLATE_ARN,
            values: &[("AWSServiceName", &["lambda.amazonaws.com"])],
            expected: "No replacement value was supplied for the required role template \
                       parameter RoleName",
        },
    ];

    for Row {
        case,
        template_arn,
        values,
        expected,
    } in rows
    {
        let (router, _engine) = router_with_engine();

        let (status, body) = acquire(&router, None, template_arn, None, values).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            *expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            element(&body, "Message")
        );
        assert!(
            listed_roles(&router).await.is_empty(),
            "case {case:?}: a refused call creates no role, got {:?}",
            listed_roles(&router).await
        );
    }
}

/// RULE 4: a template the ACCOUNT holds under an ARN is the one that resolves there, so a
/// recorded template is not superseded by the published catalog — and the published
/// templates under every other ARN still resolve.
#[tokio::test]
async fn a_recorded_template_takes_precedence_over_the_published_one() {
    let (router, engine) = router_with_engine();
    seed(&engine, BACKUP_TEMPLATE_ARN, &recorded_template());

    let (status, body) = acquire(
        &router,
        None,
        BACKUP_TEMPLATE_ARN,
        None,
        &[("RoleName", &["recorded"])],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the recorded template to create a role, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "RoleName"),
        "Recorded-recorded",
        "expected the RECORDED template's name pattern to render the role name, got {:?}",
        element(&body, "RoleName")
    );

    let (status, read) = get_role(&router, "Recorded-recorded").await;
    assert_eq!(
        status, 200,
        "the role the recorded template created must be readable, got {status}: {read}"
    );
    assert_eq!(
        element(&read, "Path"),
        "/recorded/",
        "expected the RECORDED template's path, got {:?}",
        element(&read, "Path")
    );
    let (status, attached) = call(
        &router,
        "ListAttachedRolePolicies",
        &[("RoleName", "Recorded-recorded")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the recorded role's attachments must be listable, got {status}: {attached}"
    );
    assert!(
        elements(&attached, "PolicyArn").is_empty(),
        "expected none of the published template's attachments on the recorded role, got {:?}",
        elements(&attached, "PolicyArn")
    );

    // The published template under another ARN is untouched by the record.
    let (status, body) = acquire(
        &router,
        None,
        POWER_USER_TEMPLATE_ARN,
        None,
        &[
            ("AWSServiceName", &["lambda.amazonaws.com"]),
            ("RoleName", &["power-user-role"]),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the published template to still resolve, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "RoleName"),
        "power-user-role",
        "expected the published template's name pattern to render the role name, got {:?}",
        element(&body, "RoleName")
    );
}

/// A template of the ACCOUNT's own, recorded under the ARN a service publishes one at: its
/// name pattern, path and trust policy are all the record's, so which template answered an
/// acquisition is observable in the role.
fn recorded_template() -> RoleTemplateVersion {
    RoleTemplateVersion {
        template_arn: Some(BACKUP_TEMPLATE_ARN.to_string()),
        template_name: Some("RecordedTemplate".to_string()),
        template_version_id: Some("roletemplateversionrecorded".to_string()),
        description: Some("a template the account recorded itself".to_string()),
        major_version: Some(1),
        default_minor_version: Some(0),
        managed_by_type: Some(state::ROLE_TEMPLATE_MANAGED_BY_SERVICE.to_string()),
        managed_by_value: Some("backup.amazonaws.com".to_string()),
        enabled: true,
        minor_version: Some(0),
        role_name_pattern: Some("Recorded-@{RoleName}".to_string()),
        role_path_pattern: Some("/recorded/".to_string()),
        role_description_pattern: Some("a role the account's own template created".to_string()),
        assume_role_policy_document_template: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"backup.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
                .to_string(),
        ),
        inline_policy_templates: None,
        managed_policy_arns: None,
        permission_boundary_arn: None,
        parameters_definition: Some(vec![
            service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::ParameterDefinition {
                name: "RoleName".to_string(),
                r#type: "String".to_string(),
                sub_type: None,
                description: Some("the name of the role to create".to_string()),
                is_required: true,
                default_value: None,
                immutable: true,
            },
        ]),
        role_tags_template: None,
        max_session_duration: Some(3600),
        version_enabled: true,
        create_timestamp: Some(state::now()),
        update_timestamp: Some(state::now()),
    }
}
