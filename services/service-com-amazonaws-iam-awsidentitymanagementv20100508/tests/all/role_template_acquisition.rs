// SPDX-License-Identifier: CC0-1.0

//! `AcquireRole`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket sees.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.
//!
//! The rules being asserted:
//!
//! * the role a call creates is the one the ADDRESSED role template version describes,
//!   with every templated member rendered from the replacement values the request supplied
//!   and the parameter definitions the template carries — which is what `GetRole`,
//!   `ListRolePolicies`, `GetRolePolicy` and `ListAttachedRolePolicies` report afterwards;
//! * an ARN the account holds no role template under, and a minor version it holds no such
//!   version of, reference a resource entity that does not exist, so the call is refused
//!   with the modelled `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) and
//!   nothing is created;
//! * every other refusal is the modelled one for what it refuses — a disabled template, a
//!   value the template cannot be rendered with, a rendered policy that is no policy
//!   document, a rendered name the account already holds;
//! * a caller whose identity-based policies do not allow the action is refused, and a
//!   refused call creates nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: the
//! ARN's shape and both published length bounds of `arnType`, which minor version the
//! request names, how many values a parameter carries and whether it carries any, which
//! templated member the rendering breaks, and what the caller is permitted.
//!
//! A role template is PUBLISHED by a service — `managedByTypeType` declares the single
//! value `Service` (`smithy_query(query="shape", shape="managedByTypeType")`) — and no
//! operation of this API creates one, so the suites seed the account's own template store
//! directly, as `tests/all/delegation_request_reads.rs` seeds a record whose writer is not
//! an API of this service. The ARN is the publisher's handle, held verbatim.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    InlinePolicy, ParameterDefinition, RoleTemplateVersion, TagTemplate,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The published bounds of `arnType`, the type `TemplateArn` is carried through: "len
/// 20..2048" (`smithy_query(query="operation", operation="AcquireRole")`).
const MIN_ARN_CHARACTERS: usize = 20;
const MAX_ARN_CHARACTERS: usize = 2048;

/// The ARN the fixture template is published under.
const TEMPLATE_ARN: &str = "arn:aws:iam::aws:role-template/PartnerDeployment";

/// The minor version the fixture template declares as its default.
const DEFAULT_MINOR_VERSION: i32 = 3;

/// The managed policy the fixture template attaches: an AWS-managed policy, which belongs
/// to AWS rather than to this account and so needs no fixture of its own.
const TEMPLATE_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// The permissions boundary the fixture template sets.
const TEMPLATE_BOUNDARY: &str = "arn:aws:iam::aws:policy/PowerUserAccess";

/// The maximum session duration the fixture template states, inside the published range of
/// `roleMaxSessionDurationType` (3600..43200).
const TEMPLATE_MAX_SESSION_DURATION: i32 = 7_200;

/// The trust policy template the fixture carries, with a placeholder in it so the rendering
/// is observable in the document the role ends up with.
fn trust_policy_template() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole","Condition":{{"StringEquals":{{"sts:ExternalId":"@{{Team}}"}}}}}}]}}"#,
        account_id()
    )
}

/// The inline policy template the fixture embeds, with placeholders in both the name and
/// the document.
fn inline_policy_template() -> InlinePolicy {
    InlinePolicy {
        policy_name: "@{Team}-drop".to_string(),
        policy_document:
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:PutObject","Resource":"arn:aws:s3:::drop-@{Team}/*"}]}"#
                .to_string(),
    }
}

/// The parameters the fixture template defines: one REQUIRED single-valued parameter and
/// one optional parameter with a default, which is what `ParameterDefinition$IsRequired`
/// ("whether you must supply a value for the parameter when you create a role from the
/// template") and `ParameterDefinition$DefaultValue` ("the value that the service uses for
/// the parameter when you do not supply one") distinguish. The types are values
/// `parameterTypeType` declares (`smithy_query(query="shape",
/// shape="parameterTypeType")`).
fn parameters() -> Vec<ParameterDefinition> {
    vec![
        ParameterDefinition {
            name: "Team".to_string(),
            r#type: "String".to_string(),
            sub_type: None,
            description: Some("the team the role is created for".to_string()),
            is_required: true,
            default_value: None,
            immutable: true,
        },
        ParameterDefinition {
            name: "Regions".to_string(),
            r#type: "StringList".to_string(),
            sub_type: None,
            description: Some("the Regions the role is deployed in".to_string()),
            is_required: false,
            default_value: Some("us-east-1".to_string()),
            immutable: false,
        },
    ]
}

/// The fixture role template version, at the minor version `minor_version`.
fn template(minor_version: i32) -> RoleTemplateVersion {
    RoleTemplateVersion {
        template_arn: Some(TEMPLATE_ARN.to_string()),
        template_name: Some("PartnerDeployment".to_string()),
        template_version_id: Some("roletemplateversion01".to_string()),
        description: Some("deployment roles for a partner integration".to_string()),
        major_version: Some(1),
        default_minor_version: Some(DEFAULT_MINOR_VERSION),
        managed_by_type: Some("Service".to_string()),
        managed_by_value: Some("deploy.amazonaws.com".to_string()),
        enabled: true,
        minor_version: Some(minor_version),
        role_name_pattern: Some("Partner-@{Team}-Deploy".to_string()),
        role_path_pattern: Some("/partners/@{Team}/".to_string()),
        role_description_pattern: Some("Deployment role for @{Team} in @{Regions}".to_string()),
        assume_role_policy_document_template: Some(trust_policy_template()),
        inline_policy_templates: Some(vec![inline_policy_template()]),
        managed_policy_arns: Some(vec![TEMPLATE_MANAGED_POLICY.to_string()]),
        permission_boundary_arn: Some(TEMPLATE_BOUNDARY.to_string()),
        parameters_definition: Some(parameters()),
        role_tags_template: Some(vec![TagTemplate {
            key: "Team".to_string(),
            value: "@{Team}".to_string(),
        }]),
        max_session_duration: Some(TEMPLATE_MAX_SESSION_DURATION),
        version_enabled: true,
        create_timestamp: Some(state::now()),
        update_timestamp: Some(state::now()),
    }
}

/// Seed one minor version of a role template into the service's OWN store, under the ARN
/// it is published at.
pub(crate) fn seed(
    engine: &engine::InMemoryEngine,
    template_arn: &str,
    version: &RoleTemplateVersion,
) {
    let store = state::service_state(engine).expect("the IAM store");
    let minor_version = version
        .minor_version
        .expect("a seeded role template version carries its own minor version");
    store
        .lock()
        .role_templates
        .entry(template_arn.to_string())
        .or_default()
        .insert(minor_version, version.clone());
}

/// The role names the account holds, read straight out of the store: what a refused call
/// must leave exactly as it found it.
fn held_roles(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock().roles.keys().cloned().collect();
    held
}

/// One `AcquireRole` call: the template ARN, the minor version when a row names one, and
/// the replacement values as the awsQuery map encoding carries them
/// (`ReplacementValues.entry.<n>.key` / `.value.Values.member.<m>`).
pub(crate) async fn acquire(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    template_arn: &str,
    minor_version: Option<i32>,
    replacement_values: &[(&str, &[&str])],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = vec![("TemplateArn".to_string(), template_arn.to_string())];
    if let Some(minor_version) = minor_version {
        owned.push((
            "TemplateMinorVersion".to_string(),
            minor_version.to_string(),
        ));
    }
    for (entry, (name, values)) in replacement_values.iter().enumerate() {
        let entry = entry + 1;
        owned.push((
            format!("ReplacementValues.entry.{entry}.key"),
            (*name).to_string(),
        ));
        for (index, value) in values.iter().enumerate() {
            owned.push((
                format!(
                    "ReplacementValues.entry.{entry}.value.Values.member.{}",
                    index + 1
                ),
                (*value).to_string(),
            ));
        }
    }
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_signed_by(router, akid, "AcquireRole", &params).await
}

/// The tag set a `GetRole` response reports, as a map.
fn reported_tags(body: &str) -> BTreeMap<String, String> {
    elements(body, "Key")
        .into_iter()
        .zip(elements(body, "Value"))
        .collect()
}

/// One row of the round-trip table: the replacement values the call carries and the role
/// the rendering must produce.
struct Rendered {
    case: &'static str,
    values: &'static [(&'static str, &'static [&'static str])],
    role_name: &'static str,
    path: &'static str,
    description: &'static str,
    external_id: &'static str,
}

/// RULE: the role a call creates is the one the addressed role template version describes,
/// with every templated member rendered from the replacement values supplied and the
/// parameter definitions the template carries — which is what the reads report afterwards.
///
/// Rows differ along what the rendering generalises over: a parameter carrying ONE value, a
/// parameter carrying SEVERAL, a parameter carrying none so its default is used, and a
/// value drawn from the whole alphabet a role name admits.
#[tokio::test]
async fn a_role_created_from_a_template_is_what_the_reads_report() {
    let rows = [
        Rendered {
            case: "one value for the required parameter, the optional one defaulted",
            values: &[("Team", &["Alpha"])],
            role_name: "Partner-Alpha-Deploy",
            path: "/partners/Alpha/",
            description: "Deployment role for Alpha in us-east-1",
            external_id: "Alpha",
        },
        Rendered {
            case: "several values for the list parameter",
            values: &[("Team", &["Beta"]), ("Regions", &["eu-west-1", "ap-south-1"])],
            role_name: "Partner-Beta-Deploy",
            path: "/partners/Beta/",
            description: "Deployment role for Beta in eu-west-1,ap-south-1",
            external_id: "Beta",
        },
        Rendered {
            case: "a value carrying the rest of the alphabet a role name admits",
            values: &[("Team", &["Gamma_7"]), ("Regions", &["us-west-2"])],
            role_name: "Partner-Gamma_7-Deploy",
            path: "/partners/Gamma_7/",
            description: "Deployment role for Gamma_7 in us-west-2",
            external_id: "Gamma_7",
        },
    ];

    for row in rows {
        let (router, engine) = router_with_engine();
        seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));

        let (status, body) = acquire(&router, None, TEMPLATE_ARN, None, row.values).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the role to be created, got {status}: {body}",
            row.case
        );
        assert_eq!(
            element(&body, "RoleName"),
            row.role_name,
            "case {:?}: expected the rendered role name {}, got {:?}",
            row.case,
            row.role_name,
            element(&body, "RoleName")
        );

        // The round trip: the role the create answered with is the one the read reports.
        let (status, read) = call(&router, "GetRole", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the created role must be readable, got {status}: {read}",
            row.case
        );
        assert_eq!(
            element(&read, "Path"),
            row.path,
            "case {:?}: expected the rendered path {}, got {:?}",
            row.case,
            row.path,
            element(&read, "Path")
        );
        assert_eq!(
            element(&read, "Arn"),
            format!("arn:aws:iam::{}:role{}{}", account_id(), row.path, row.role_name),
            "case {:?}: expected the role's ARN at the rendered path, got {:?}",
            row.case,
            element(&read, "Arn")
        );
        assert_eq!(
            element(&read, "Description"),
            row.description,
            "case {:?}: expected the rendered description {:?}, got {:?}",
            row.case,
            row.description,
            element(&read, "Description")
        );
        assert_eq!(
            element(&read, "MaxSessionDuration"),
            TEMPLATE_MAX_SESSION_DURATION.to_string(),
            "case {:?}: expected the template's maximum session duration, got {:?}",
            row.case,
            element(&read, "MaxSessionDuration")
        );
        assert_eq!(
            element(&read, "PermissionsBoundaryArn"),
            TEMPLATE_BOUNDARY,
            "case {:?}: expected the template's permissions boundary, got {:?}",
            row.case,
            element(&read, "PermissionsBoundaryArn")
        );
        assert_eq!(
            reported_tags(&read).get("Team").map(String::as_str),
            Some(row.external_id),
            "case {:?}: expected the rendered tag value, got {:?}",
            row.case,
            reported_tags(&read)
        );
        // "This member is present only for roles created with AcquireRole": the template
        // the role came from, and the minor version that was resolved.
        assert_eq!(
            element(&read, "TemplateArn"),
            TEMPLATE_ARN,
            "case {:?}: expected the source template's ARN, got {:?}",
            row.case,
            element(&read, "TemplateArn")
        );
        assert_eq!(
            element(&read, "TemplateMinorVersion"),
            DEFAULT_MINOR_VERSION.to_string(),
            "case {:?}: expected the resolved minor version, got {:?}",
            row.case,
            element(&read, "TemplateMinorVersion")
        );

        // The trust policy is the template's, rendered: it parses as the JSON policy it is
        // and carries the value the parameter was rendered with.
        let trust = crate::user_lifecycle::document_element(&read, "AssumeRolePolicyDocument");
        let parsed: serde_json::Value = serde_json::from_str(&trust).unwrap_or_else(|error| {
            panic!(
                "case {:?}: the rendered trust policy must be JSON ({error}): {trust:?}",
                row.case
            )
        });
        assert_eq!(
            parsed["Statement"][0]["Condition"]["StringEquals"]["sts:ExternalId"],
            serde_json::Value::String(row.external_id.to_string()),
            "case {:?}: expected the rendered trust policy to carry {}, got {trust}",
            row.case,
            row.external_id
        );
        assert!(
            !trust.contains("@{"),
            "case {:?}: expected every placeholder rendered, got {trust}",
            row.case
        );

        // The inline policy the template embeds, under its rendered name and with its
        // rendered document — read back through the operations that report a role's
        // embedded policies.
        let policy_name = format!("{}-drop", row.external_id);
        let (status, listed) = call(&router, "ListRolePolicies", &[("RoleName", row.role_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the role's inline policies must be listable, got {status}: {listed}",
            row.case
        );
        assert_eq!(
            elements(&listed, "member"),
            vec![policy_name.clone()],
            "case {:?}: expected the template's inline policy under its rendered name, got {:?}",
            row.case,
            elements(&listed, "member")
        );
        let (status, embedded) = call(
            &router,
            "GetRolePolicy",
            &[("RoleName", row.role_name), ("PolicyName", &policy_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the embedded policy must be readable, got {status}: {embedded}",
            row.case
        );
        let document = crate::user_lifecycle::document_element(&embedded, "PolicyDocument");
        assert!(
            document.contains(&format!("arn:aws:s3:::drop-{}/*", row.external_id))
                && !document.contains("@{"),
            "case {:?}: expected the rendered inline policy document, got {document}",
            row.case
        );

        // The managed policy the template attaches.
        let (status, attached) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", row.role_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the role's attached policies must be listable, got {status}: {attached}",
            row.case
        );
        assert_eq!(
            elements(&attached, "PolicyArn"),
            vec![TEMPLATE_MANAGED_POLICY.to_string()],
            "case {:?}: expected the template's managed policy attached, got {:?}",
            row.case,
            elements(&attached, "PolicyArn")
        );
    }
}

/// RULE: "if you do not specify a minor version, the service uses the template's default
/// minor version", and a request that names one addresses THAT version — so the rows are
/// the version the request names, and the version the resolution lands on is the one the
/// created role records as its source.
#[tokio::test]
async fn the_addressed_minor_version_is_the_one_the_request_names_or_the_default() {
    // Rows: the template is published at three minor versions, one of which it declares as
    // its default, so naming none is distinguishable from naming each.
    let published = [1, DEFAULT_MINOR_VERSION, 5];
    let rows: Vec<(&str, Option<i32>, i32)> = vec![
        ("no minor version named", None, DEFAULT_MINOR_VERSION),
        ("the lowest published minor version", Some(1), 1),
        ("the default minor version, named", Some(DEFAULT_MINOR_VERSION), DEFAULT_MINOR_VERSION),
        ("the highest published minor version", Some(5), 5),
    ];

    for (case, named, expected) in rows {
        let (router, engine) = router_with_engine();
        for minor_version in published {
            seed(&engine, TEMPLATE_ARN, &template(minor_version));
        }

        let (status, body) =
            acquire(&router, None, TEMPLATE_ARN, named, &[("Team", &["Alpha"])]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the role to be created, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "TemplateMinorVersion"),
            expected.to_string(),
            "case {case:?}: expected the role to record minor version {expected}, got {:?}",
            element(&body, "TemplateMinorVersion")
        );
    }

    // A template published at ONE minor version and declaring no default is addressed at
    // that version, because it is the only one there is.
    let (router, engine) = router_with_engine();
    let mut only = template(9);
    only.default_minor_version = None;
    seed(&engine, TEMPLATE_ARN, &only);
    let (status, body) = acquire(&router, None, TEMPLATE_ARN, None, &[("Team", &["Alpha"])]).await;
    assert_eq!(
        status, 200,
        "a template with one minor version and no default must be addressed at it, \
         got {status}: {body}"
    );
    assert_eq!(
        element(&body, "TemplateMinorVersion"),
        "9",
        "expected the only published minor version, got {:?}",
        element(&body, "TemplateMinorVersion")
    );
}

/// RULE: an ARN the account holds no role template under references "a resource entity
/// that does not exist", so the call is refused with the modelled `NoSuchEntityException`
/// (HTTP 404, awsQuery code `NoSuchEntity`) naming the template asked about, and nothing is
/// created.
///
/// Rows span the ARN's shape and BOTH published length bounds of `arnType` (20..2048).
#[tokio::test]
async fn an_arn_the_account_holds_no_template_under_is_refused_as_a_missing_entity() {
    let longest = format!(
        "arn:aws:iam::aws:role-template/{}",
        "L".repeat(MAX_ARN_CHARACTERS - "arn:aws:iam::aws:role-template/".len())
    );
    let shortest = "arn:aws:iam:::rt/aaa".to_string();
    assert_eq!(
        shortest.chars().count(),
        MIN_ARN_CHARACTERS,
        "the shortest row must sit on the published minimum, got {shortest:?}"
    );
    let rows = vec![
        ("the published minimum length", shortest),
        ("the published maximum length", longest),
        (
            "another template of the same publisher",
            "arn:aws:iam::aws:role-template/OtherDeployment".to_string(),
        ),
        (
            "a template of this account rather than of a service",
            format!("arn:aws:iam::{}:role-template/PartnerDeployment", account_id()),
        ),
        (
            "the held template's ARN in another partition",
            "arn:aws-us-gov:iam::aws:role-template/PartnerDeployment".to_string(),
        ),
        (
            "the held template's ARN in another case",
            "arn:aws:iam::aws:role-template/partnerdeployment".to_string(),
        ),
    ];

    for (case, template_arn) in rows {
        let (router, engine) = router_with_engine();
        // The account holds ANOTHER template, so what is asserted is the lookup of the ARN
        // the request named rather than whether the account holds any template at all.
        seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));
        let before = held_roles(&engine);

        let (status, body) =
            acquire(&router, None, &template_arn, None, &[("Team", &["Alpha"])]).await;
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
            "case {case:?}: the refusal must name the template asked about, got {:?}",
            element(&body, "Message")
        );
        assert_eq!(
            held_roles(&engine),
            before,
            "case {case:?}: a refused call creates no role, got {:?}",
            held_roles(&engine)
        );
    }
}

/// RULE: a minor version the account holds no such version of is the same missing entity —
/// the resource the call addresses is the role template, which its ARN identifies — so the
/// rows are versions on each side of the published one and the refusal names that template.
#[tokio::test]
async fn a_minor_version_the_template_does_not_hold_is_refused_as_a_missing_entity() {
    let (router, engine) = router_with_engine();
    seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));

    for minor_version in [DEFAULT_MINOR_VERSION - 1, DEFAULT_MINOR_VERSION + 1, 0, -1] {
        let case = format!("minor version {minor_version}");
        let (status, body) = acquire(
            &router,
            None,
            TEMPLATE_ARN,
            Some(minor_version),
            &[("Team", &["Alpha"])],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Role template not found: {TEMPLATE_ARN}"),
            "case {case:?}: the refusal must name the template asked about, got {:?}",
            element(&body, "Message")
        );
    }

    assert!(
        held_roles(&engine).is_empty(),
        "no refused call may create a role, got {:?}",
        held_roles(&engine)
    );
}

/// RULE: "when a template is disabled, you cannot create roles from it", and
/// `VersionEnabled` says the same of one minor version — so either flag down is the
/// modelled `RoleTemplateDisabledException` (HTTP 400, awsQuery code
/// `RoleTemplateDisabled`) and nothing is created.
#[tokio::test]
async fn a_disabled_template_or_minor_version_creates_no_role() {
    // Rows: the template-level flag down, the version-level flag down, and both.
    let rows: Vec<(&str, bool, bool)> = vec![
        ("the template is disabled", false, true),
        ("this minor version is disabled", true, false),
        ("both are disabled", false, false),
    ];

    for (case, enabled, version_enabled) in rows {
        let (router, engine) = router_with_engine();
        let mut disabled = template(DEFAULT_MINOR_VERSION);
        disabled.enabled = enabled;
        disabled.version_enabled = version_enabled;
        seed(&engine, TEMPLATE_ARN, &disabled);

        let (status, body) =
            acquire(&router, None, TEMPLATE_ARN, None, &[("Team", &["Alpha"])]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "RoleTemplateDisabled",
            "case {case:?}: expected RoleTemplateDisabled, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!(
                "Role template {TEMPLATE_ARN} is disabled and cannot be used to create new \
                 roles."
            ),
            "case {case:?}: the refusal must name the disabled template, got {:?}",
            element(&body, "Message")
        );
        assert!(
            held_roles(&engine).is_empty(),
            "case {case:?}: a refused call creates no role, got {:?}",
            held_roles(&engine)
        );
    }
}

/// RULE: a value the template cannot be rendered with is "an invalid or out-of-range value
/// … supplied for an input parameter" — the modelled `InvalidInputException` (HTTP 400,
/// awsQuery code `InvalidInput`) — and nothing is created.
///
/// Rows span every way a request can fail to fill the template: a `TemplateArn` that is no
/// ARN at all, one that is an ARN of another service, a required parameter nothing supplies
/// a value for, a value supplied for a parameter the template does not define, and a
/// rendering that produces a name or a path the role's own members do not admit.
#[tokio::test]
async fn a_value_the_template_cannot_be_rendered_with_is_refused_as_invalid_input() {
    // The rendered-name and rendered-path rows need their own template, so each row names
    // the template it seeds.
    let mut long_name = template(DEFAULT_MINOR_VERSION);
    long_name.role_name_pattern = Some(format!("{}-@{{Team}}", "N".repeat(64)));
    let mut bad_path = template(DEFAULT_MINOR_VERSION);
    bad_path.role_path_pattern = Some("partners/@{Team}".to_string());
    let mut spaced_name = template(DEFAULT_MINOR_VERSION);
    spaced_name.role_name_pattern = Some("Partner @{Team}".to_string());

    struct Row {
        case: &'static str,
        seeded: RoleTemplateVersion,
        template_arn: String,
        values: Vec<(&'static str, Vec<&'static str>)>,
        message: String,
    }

    let rows = vec![
        Row {
            case: "a template ARN that is no ARN at all",
            seeded: template(DEFAULT_MINOR_VERSION),
            template_arn: "not-an-arn-but-long-enough".to_string(),
            values: vec![("Team", vec!["Alpha"])],
            message: state::ARN_UNPARSEABLE.to_string(),
        },
        Row {
            case: "a well-formed ARN of another service",
            seeded: template(DEFAULT_MINOR_VERSION),
            template_arn: "arn:aws:s3:::role-template/PartnerDeployment".to_string(),
            values: vec![("Team", vec!["Alpha"])],
            message: state::ARN_WRONG_RESOURCE_TYPE.to_string(),
        },
        Row {
            case: "no value for the required parameter",
            seeded: template(DEFAULT_MINOR_VERSION),
            template_arn: TEMPLATE_ARN.to_string(),
            values: vec![("Regions", vec!["us-east-1"])],
            message: "No replacement value was supplied for the required role template \
                      parameter Team"
                .to_string(),
        },
        Row {
            case: "a value for a parameter the template does not define",
            seeded: template(DEFAULT_MINOR_VERSION),
            template_arn: TEMPLATE_ARN.to_string(),
            values: vec![("Team", vec!["Alpha"]), ("Squad", vec!["Red"])],
            message: "The role template version defines no parameter named Squad".to_string(),
        },
        Row {
            case: "a rendering one character past the role name's published maximum",
            seeded: long_name,
            template_arn: TEMPLATE_ARN.to_string(),
            values: vec![("Team", vec!["Alpha"])],
            message: format!(
                "The value rendered for RoleName is not valid: {}-Alpha",
                "N".repeat(64)
            ),
        },
        Row {
            case: "a rendering carrying a character the role name does not admit",
            seeded: spaced_name,
            template_arn: TEMPLATE_ARN.to_string(),
            values: vec![("Team", vec!["Alpha"])],
            message: "The value rendered for RoleName is not valid: Partner Alpha".to_string(),
        },
        Row {
            case: "a rendering that is no entity path",
            seeded: bad_path,
            template_arn: TEMPLATE_ARN.to_string(),
            values: vec![("Team", vec!["Alpha"])],
            message: "The value rendered for Path is not valid: partners/Alpha".to_string(),
        },
    ];

    for row in rows {
        let (router, engine) = router_with_engine();
        seed(&engine, TEMPLATE_ARN, &row.seeded);
        let values: Vec<(&str, &[&str])> = row
            .values
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();

        let (status, body) = acquire(&router, None, &row.template_arn, None, &values).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status}: {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {:?}: expected InvalidInput, got {:?}: {body}",
            row.case,
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            row.message,
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            row.message,
            element(&body, "Message")
        );
        assert!(
            held_roles(&engine).is_empty(),
            "case {:?}: a refused call creates no role, got {:?}",
            row.case,
            held_roles(&engine)
        );
    }
}

/// RULE: a rendered trust or inline policy that is no IAM policy document is the modelled
/// `MalformedPolicyDocumentException` (HTTP 400, awsQuery code `MalformedPolicyDocument`),
/// and nothing is created — the same decision every operation of this service takes a
/// policy document through.
#[tokio::test]
async fn a_rendered_policy_that_is_no_policy_document_is_refused() {
    let mut broken_trust = template(DEFAULT_MINOR_VERSION);
    broken_trust.assume_role_policy_document_template = Some("@{Team}".to_string());
    let mut trust_without_statements = template(DEFAULT_MINOR_VERSION);
    trust_without_statements.assume_role_policy_document_template =
        Some(r#"{"Version":"2012-10-17"}"#.to_string());
    let mut broken_inline = template(DEFAULT_MINOR_VERSION);
    broken_inline.inline_policy_templates = Some(vec![InlinePolicy {
        policy_name: "@{Team}-drop".to_string(),
        policy_document: "not a policy at all".to_string(),
    }]);

    // Rows: the trust policy rendering to something that is not JSON, to a document with no
    // statements, and an inline policy rendering to something that is not JSON.
    let rows = vec![
        ("a trust policy that is not JSON", broken_trust),
        ("a trust policy with no statements", trust_without_statements),
        ("an inline policy that is not JSON", broken_inline),
    ];

    for (case, seeded) in rows {
        let (router, engine) = router_with_engine();
        seed(&engine, TEMPLATE_ARN, &seeded);

        let (status, body) =
            acquire(&router, None, TEMPLATE_ARN, None, &[("Team", &["Alpha"])]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected MalformedPolicyDocument, got {:?}: {body}",
            error_code(&body)
        );
        assert!(
            !element(&body, "Message").is_empty(),
            "case {case:?}: the refusal must describe the specific error, got {body}"
        );
        assert!(
            held_roles(&engine).is_empty(),
            "case {case:?}: a refused call creates no role, got {:?}",
            held_roles(&engine)
        );
    }
}

/// RULE: "the resulting role name conflicts with an existing role in the account" is the
/// modelled `NameConflictException` (HTTP 409, awsQuery code `NameConflict`), and IAM entity
/// names "are not distinguished by case", so a rendering that lands on any casing of a held
/// name conflicts — whichever path created the role that holds it.
#[tokio::test]
async fn a_rendered_name_the_account_already_holds_is_a_name_conflict() {
    // Rows: the name held by a role created THROUGH this operation, and by one created with
    // `CreateRole` under a casing that differs.
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("a role this operation already created", None),
        ("a role created by hand under another casing", Some("partner-alpha-deploy")),
    ];

    for (case, held_by_hand) in rows {
        let (router, engine) = router_with_engine();
        seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));

        match held_by_hand {
            Some(role_name) => {
                let trust = format!(
                    r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
                    account_id()
                );
                let (status, body) = call(
                    &router,
                    "CreateRole",
                    &[
                        ("RoleName", role_name),
                        ("AssumeRolePolicyDocument", trust.as_str()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
                );
            }
            None => {
                let (status, body) =
                    acquire(&router, None, TEMPLATE_ARN, None, &[("Team", &["Alpha"])]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the first create must succeed, got {status}: {body}"
                );
            }
        }
        let before = held_roles(&engine);

        let (status, body) =
            acquire(&router, None, TEMPLATE_ARN, None, &[("Team", &["Alpha"])]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NameConflict",
            "case {case:?}: expected NameConflict, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            "Role with name Partner-Alpha-Deploy already exists in this account.",
            "case {case:?}: the refusal must name the rendered role, got {:?}",
            element(&body, "Message")
        );
        assert_eq!(
            held_roles(&engine),
            before,
            "case {case:?}: a refused call creates no second role, got {:?}",
            held_roles(&engine)
        );
    }
}

/// RULE: IAM decides the call against the caller's identity-based policies before it reads
/// the template, so a caller no policy allows the action for is refused with HTTP 403
/// `AccessDenied` naming the principal, the action and the TEMPLATE the request addressed —
/// whether or not the account holds that template — and the refused call creates nothing.
/// The same call under a statement that names its action is served.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied this
    // action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:GetRole", "*"),
            )],
            "because no identity-based policy allows the iam:AcquireRole action".to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                (
                    "allow".to_string(),
                    statement("Allow", "iam:AcquireRole", "*"),
                ),
                (
                    "deny".to_string(),
                    statement("Deny", "iam:AcquireRole", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));
        let user_name = format!("acquirer{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let before = held_roles(&engine);

        for template_arn in [
            TEMPLATE_ARN,
            "arn:aws:iam::aws:role-template/NeverPublished",
        ] {
            let case = format!("{case} acquiring from {template_arn}");
            let (status, body) = acquire(
                &router,
                Some(&access_key_id),
                template_arn,
                None,
                &[("Team", &["Alpha"])],
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                "iam:AcquireRole",
                &format!("role template {template_arn}"),
                &reason,
            );
        }

        assert_eq!(
            held_roles(&engine),
            before,
            "case {case:?}: a refused caller creates no role, got {:?}",
            held_roles(&engine)
        );
    }

    let (router, engine) = router_with_engine();
    seed(&engine, TEMPLATE_ARN, &template(DEFAULT_MINOR_VERSION));
    let access_key_id = principal(
        &router,
        "a granted caller",
        "acquirer-granted",
        &[(
            "acquire".to_string(),
            statement("Allow", "iam:AcquireRole", TEMPLATE_ARN),
        )],
    )
    .await;
    let (status, body) = acquire(
        &router,
        Some(&access_key_id),
        TEMPLATE_ARN,
        None,
        &[("Team", &["Alpha"])],
    )
    .await;
    assert_eq!(
        status, 200,
        "a caller granted the action on the template must be served, got {status}: {body}"
    );
    assert!(
        held_roles(&engine).contains(&"partner-alpha-deploy".to_string()),
        "the created role must be held, got {:?}",
        held_roles(&engine)
    );
}
