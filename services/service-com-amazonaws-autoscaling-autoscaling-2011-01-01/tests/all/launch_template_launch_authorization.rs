// SPDX-License-Identifier: CC0-1.0

//! WHO may point an Auto Scaling group at a launch template — the decision taken before the
//! group's launch source is stored.
//!
//! The rule under test: a group configured to launch from an Amazon EC2 launch template
//! launches its instances from that template, so a request that points a group at one asks
//! for a launch from it beside the Auto Scaling action it names, and IAM judges that launch
//! as `ec2:RunInstances` against the launch template's own ARN. A caller whose identity-based
//! policies hold no `Allow` of it — or whose policies `Deny` it — is refused HTTP 403
//! `AccessDenied` with the sentence naming that launch template, and the group is left exactly
//! as it was: a refused create creates NOTHING and a refused update changes nothing. A caller
//! the policies in force permit is served and the group reports the template it was pointed at.
//!
//! The rows span what that rule generalises over: the documents in force (no Amazon EC2 grant
//! at all, an allow of every Amazon EC2 action, an allow of exactly the launch action, an allow
//! overridden by a deny of the launch, and an allow of Amazon EC2's reads alone — so the
//! decision is per ACTION and not per service, and the refusal is the same sentence whichever
//! of IAM's rules withheld the grant), and, for each, WHICH member of the request names the
//! template: the group's own `LaunchTemplate` by name and by id, the `MixedInstancesPolicy`'s
//! launch template, and the template one of that policy's overrides names. One further row
//! grants the launch on ONE template's ARN, so a request naming the other is refused on the
//! template it named: every template a request points the group at is judged, each on its own.
//! Every row reads the group back as the account's own credential, so no row can pass on a
//! refusal that still configured the group.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts_service;

use crate::support::{
    borrowed, create_launch_template, emulator_with_iam, form_encode, list_members, p, query,
    seed_subnets, tag_value, xml_unescape, SeededSubnet, REGION,
};

/// The Amazon EC2 action a launch from a launch template requires, and the code this service
/// refuses an unauthorized caller with — spelled out here rather than imported, so a wrong
/// constant in the implementation cannot make a row pass.
const RUN_INSTANCES_ACTION: &str = "ec2:RunInstances";
const ACCESS_DENIED: &str = "AccessDenied";

/// The access key id the account's own published credential presents: a key neither STS nor
/// IAM holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The API versions a client sends in each service's query body.
const AUTOSCALING_API_VERSION: &str = "2011-01-01";
const IAM_API_VERSION: &str = "2010-05-08";
const STS_API_VERSION: &str = "2011-06-15";

/// The emulator these rows drive: the shared four-service router — Auto Scaling beside Amazon
/// EC2, Elastic Load Balancing v2 and IAM on ONE engine — with STS mounted beside them, since
/// the caller a decision is taken for is the session STS minted the request's key for.
fn authorized_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_iam();
    router.register_service(sts_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One query-protocol call to `service`, presented as signed by the access key id `akid` — the
/// only place the credential appears on the wire, and so what the service reads the caller's
/// identity from.
async fn call_as(
    router: &MultiServiceRouter,
    service: &str,
    api_version: &str,
    akid: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    // IAM is a global service, so its endpoint carries no Region.
    let host = if service == "iam" {
        "iam.amazonaws.com".to_string()
    } else {
        format!("{service}.{REGION}.amazonaws.com")
    };
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/{service}/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Auto Scaling call signed by `akid`.
async fn autoscaling_as(
    router: &MultiServiceRouter,
    akid: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call_as(
        router,
        "autoscaling",
        AUTOSCALING_API_VERSION,
        akid,
        action,
        &borrowed(params),
    )
    .await
}

/// A one-statement identity-based document: `effect` on `action` over every resource.
fn statement(effect: &str, action: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"*"}}]}}"#
    )
}

/// A one-statement identity-based document allowing `action` on `resource` ALONE.
fn resource_statement(action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// The document every row carries beside its Amazon EC2 one: the Auto Scaling actions these
/// calls name, so what a row is refused for is the LAUNCH and never the operation itself.
fn autoscaling_allowed() -> (&'static str, String) {
    ("allow-autoscaling", statement("Allow", "autoscaling:*"))
}

/// Create the role `role_name` with `inline` policies on it, then assume it: answers the
/// session's access key id and the ARN `sts:GetCallerIdentity` reports for it, which a refusal
/// must name.
async fn session_of(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: &[(&str, String)],
) -> (String, String) {
    let account = engine::emulator_account_id();
    let trust = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{account}:root"}},"Action":"sts:AssumeRole"}}]}}"#
    );
    let (status, created) = call_as(
        router,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        "CreateRole",
        &[("RoleName", role_name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be created, got {status} {created}"
    );
    for (policy_name, document) in inline {
        let (status, put) = call_as(
            router,
            "iam",
            IAM_API_VERSION,
            ACCOUNT_KEY_ID,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the inline policy {policy_name:?} must be stored, got {status} {put}"
        );
    }
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    let session_name = format!("{role_name}-session");
    let (status, assumed) = call_as(
        router,
        "sts",
        STS_API_VERSION,
        ACCOUNT_KEY_ID,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", &session_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be assumable, got {status} {assumed}"
    );
    let akid = tag_value(&assumed, "AccessKeyId");
    let arn = tag_value(&assumed, "Arn");
    assert!(
        !akid.is_empty() && !arn.is_empty(),
        "case {case:?}: the session must report its key and its ARN, got {assumed}"
    );
    (akid, arn)
}

/// What a row expects of one call it drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force allow the launch.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request allows it.
    NoAllow,
}

/// The sentence this service refuses a caller that may not launch from the launch template `id`
/// with: fixed text naming the template, the only part taken from the request. Written out here
/// rather than imported, so a wrong constant in the implementation cannot make a row pass.
///
/// The refusal is about the TEMPLATE and not about the policy statement that withheld the
/// grant, so it is the same sentence whichever of IAM's rules refused the launch.
fn refusal_message(id: &str) -> String {
    format!("You are not authorized to use launch template: {id}")
}

/// The ARN IAM judges a launch from the launch template `id` against: Amazon EC2's own form for
/// a launch template, in the Region the request was signed for — what a policy scoping the
/// launch to particular templates names.
fn launch_template_arn(id: &str) -> String {
    engine::emulator_arn("ec2", REGION, &format!("launch-template/{id}"))
}

/// Which member of the request names the launch template the group launches from — every one
/// that can: the group's own `LaunchTemplate` (by either half of the template's identity), the
/// launch template of its `MixedInstancesPolicy`, and the template an override of that policy
/// names (`LaunchTemplateOverrides$LaunchTemplateSpecification`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    OwnByName,
    OwnById,
    PolicyByName,
    PolicyOverrideById,
}

/// Every member a row drives the rule through.
const NAMED: [Named; 4] = [
    Named::OwnByName,
    Named::OwnById,
    Named::PolicyByName,
    Named::PolicyOverrideById,
];

/// The parameters naming the launch template `(name, id)` through `named`.
fn launch_source_params(named: Named, name: &str, id: &str) -> Vec<(String, String)> {
    match named {
        Named::OwnByName => vec![p("LaunchTemplate.LaunchTemplateName", name)],
        Named::OwnById => vec![p("LaunchTemplate.LaunchTemplateId", id)],
        Named::PolicyByName => vec![p(
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
            name,
        )],
        // The policy names its own template and the override names one too: both are templates
        // the group's instances are launched from, so both are judged.
        Named::PolicyOverrideById => vec![
            p(
                "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
                name,
            ),
            p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.LaunchTemplateSpecification.LaunchTemplateId",
                id,
            ),
        ],
    }
}

/// The parameters of a group that holds no capacity, so nothing is launched and the row is
/// about the configuration alone.
fn group_params(name: &str, subnet: &SeededSubnet) -> Vec<(String, String)> {
    vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "0"),
        p("VPCZoneIdentifier", subnet.id.clone()),
    ]
}

/// Judge one call the session drove against what the row expects: a served call answers 200,
/// and a refused one answers 403 `AccessDenied` naming the launch template `refused_template`.
fn judge(
    case: &str,
    operation: &str,
    refused_template: &str,
    expected: Expect,
    answered: (u16, String),
) {
    let (status, body) = answered;
    if expected == Expect::Served {
        assert_eq!(
            status, 200,
            "case {case:?} / {operation}: the permitted call must be served, got {status} {body}"
        );
        return;
    }
    assert_eq!(
        status, 403,
        "case {case:?} / {operation}: the unauthorized call must be refused, got {status} {body}"
    );
    assert_eq!(
        tag_value(&body, "Code"),
        ACCESS_DENIED,
        "case {case:?} / {operation}: expected {ACCESS_DENIED}, got {body}"
    );
    let expected_message = refusal_message(refused_template);
    let reported = xml_unescape(&tag_value(&body, "Message"));
    assert_eq!(
        reported, expected_message,
        "case {case:?} / {operation}: expected {expected_message:?}, got {reported:?}"
    );
}

/// The launch template id the group `name` reports, read as the account's own credential, and
/// `None` when the account holds no group under that name at all.
async fn reported_template(router: &MultiServiceRouter, case: &str, name: &str) -> Option<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read-back of {name:?} expected 200, got {status} with {body:?}"
    );
    if list_members(&body, "AutoScalingGroups").is_empty() {
        return None;
    }
    let id = tag_value(&body, "LaunchTemplateId");
    assert!(
        !id.is_empty(),
        "case {case:?}: the group {name:?} must report the template it launches from, got {body:?}"
    );
    Some(id)
}

/// RULE: a create that points a group at a launch template is authorized to LAUNCH from that
/// template — `ec2:RunInstances` on the template's own ARN — whichever member names it, so a
/// caller the policies in force do not permit creates NOTHING and is refused 403 `AccessDenied`
/// naming that launch template, while a caller they permit is served and the group reports the
/// template.
#[tokio::test]
async fn a_group_create_is_authorized_to_launch_from_its_launch_template() {
    let (router, engine) = authorized_emulator();
    let subnets = seed_subnets(&engine, 1);
    let (template_id, _) = create_launch_template(&router, "lt-launch-authorization").await;

    /// One row: the case, the role's name, the documents in force on it, and what a call
    /// pointing a group at a launch template is answered with.
    type Row = (
        &'static str,
        &'static str,
        Vec<(&'static str, String)>,
        Expect,
    );
    let rows: Vec<Row> = vec![
        (
            "a role carrying no Amazon EC2 grant at all",
            "launch-bare-role",
            vec![autoscaling_allowed()],
            Expect::NoAllow,
        ),
        (
            "a role allowed every Amazon EC2 action",
            "launch-ec2-role",
            vec![autoscaling_allowed(), ("allow-ec2", statement("Allow", "ec2:*"))],
            Expect::Served,
        ),
        (
            "a role allowed exactly the launch action",
            "launch-action-role",
            vec![
                autoscaling_allowed(),
                ("allow-run", statement("Allow", RUN_INSTANCES_ACTION)),
            ],
            Expect::Served,
        ),
        (
            "a role whose allow is overridden by a deny of the launch",
            "launch-denied-role",
            vec![
                autoscaling_allowed(),
                ("allow-ec2", statement("Allow", "ec2:*")),
                ("deny-run", statement("Deny", RUN_INSTANCES_ACTION)),
            ],
            Expect::ExplicitDeny,
        ),
        (
            "a role allowed Amazon EC2's reads alone",
            "launch-describe-role",
            vec![
                autoscaling_allowed(),
                ("allow-describe", statement("Allow", "ec2:Describe*")),
            ],
            Expect::NoAllow,
        ),
    ];

    for (row, (case, role_name, inline, expected)) in rows.iter().enumerate() {
        let (akid, _) = session_of(&router, case, role_name, inline).await;
        for (index, named) in NAMED.iter().enumerate() {
            let case = format!("{case} naming the template through {named:?}");
            let name = format!("asg-launch-auth-{row}-{index}");
            let mut params = group_params(&name, &subnets[0]);
            params.extend(launch_source_params(
                *named,
                "lt-launch-authorization",
                &template_id,
            ));
            let answered = autoscaling_as(&router, &akid, "CreateAutoScalingGroup", &params).await;
            judge(
                &case,
                "CreateAutoScalingGroup",
                &template_id,
                *expected,
                answered,
            );
            let held = reported_template(&router, &case, &name).await;
            let wanted = if *expected == Expect::Served {
                Some(template_id.clone())
            } else {
                None
            };
            assert_eq!(
                held, wanted,
                "case {case:?}: after the create the account must hold {wanted:?} for \
                 {name:?}, got {held:?}"
            );
        }
    }
}

/// RULE: the same decision is taken by the update that REPLACES a group's launch source, so a
/// caller who may not launch from the template it names leaves the group on the template it
/// already had, and one who may is served and the group reports the new template.
#[tokio::test]
async fn an_update_is_authorized_to_launch_from_the_template_it_names() {
    let (router, engine) = authorized_emulator();
    let subnets = seed_subnets(&engine, 1);
    let (held_id, _) = create_launch_template(&router, "lt-update-held").await;
    let (next_id, _) = create_launch_template(&router, "lt-update-next").await;

    /// One row: the case, the role's name, the documents in force on it, and what the update
    /// is answered with.
    type Row = (
        &'static str,
        &'static str,
        Vec<(&'static str, String)>,
        Expect,
    );
    let rows: Vec<Row> = vec![
        (
            "a role carrying no Amazon EC2 grant at all",
            "update-bare-role",
            vec![autoscaling_allowed()],
            Expect::NoAllow,
        ),
        (
            "a role allowed exactly the launch action",
            "update-action-role",
            vec![
                autoscaling_allowed(),
                ("allow-run", statement("Allow", RUN_INSTANCES_ACTION)),
            ],
            Expect::Served,
        ),
        (
            "a role whose allow is overridden by a deny of the launch",
            "update-denied-role",
            vec![
                autoscaling_allowed(),
                ("allow-ec2", statement("Allow", "ec2:*")),
                ("deny-run", statement("Deny", RUN_INSTANCES_ACTION)),
            ],
            Expect::ExplicitDeny,
        ),
    ];

    for (row, (case, role_name, inline, expected)) in rows.iter().enumerate() {
        let (akid, _) = session_of(&router, case, role_name, inline).await;
        let name = format!("asg-update-auth-{row}");
        let mut created = group_params(&name, &subnets[0]);
        created.push(p("LaunchTemplate.LaunchTemplateName", "lt-update-held"));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&created)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own create must be served, got {status} {body}"
        );

        let update = vec![
            p("AutoScalingGroupName", &name),
            p("LaunchTemplate.LaunchTemplateId", &next_id),
        ];
        let answered = autoscaling_as(&router, &akid, "UpdateAutoScalingGroup", &update).await;
        judge(
            case,
            "UpdateAutoScalingGroup",
            &next_id,
            *expected,
            answered,
        );
        let held = reported_template(&router, case, &name).await;
        let wanted = if *expected == Expect::Served {
            Some(next_id.clone())
        } else {
            Some(held_id.clone())
        };
        assert_eq!(
            held, wanted,
            "case {case:?}: after the update the group {name:?} must launch from {wanted:?}, got \
             {held:?}"
        );
    }
}

/// RULE: the launch is judged per TEMPLATE, against each template's own ARN — a grant of the
/// launch on one template's ARN permits a group pointed at that template and refuses one
/// pointed at another, naming the template the request named.
#[tokio::test]
async fn the_launch_is_judged_against_each_named_templates_own_arn() {
    let (router, engine) = authorized_emulator();
    let subnets = seed_subnets(&engine, 1);
    let (granted_id, _) = create_launch_template(&router, "lt-granted").await;
    let (withheld_id, _) = create_launch_template(&router, "lt-withheld").await;
    let case = "a role allowed the launch on one template's ARN alone";
    let (akid, _) = session_of(
        &router,
        case,
        "launch-one-template-role",
        &[
            autoscaling_allowed(),
            (
                "allow-run-on-one",
                resource_statement(RUN_INSTANCES_ACTION, &launch_template_arn(&granted_id)),
            ),
        ],
    )
    .await;

    let granted_source = vec![p("LaunchTemplate.LaunchTemplateId", &granted_id)];
    let withheld_source = vec![p("LaunchTemplate.LaunchTemplateId", &withheld_id)];
    let policy_with_withheld_override = vec![
        p(
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateId",
            &granted_id,
        ),
        p(
            "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.LaunchTemplateSpecification.LaunchTemplateId",
            &withheld_id,
        ),
    ];
    /// One row: the case, the launch source's parameters, the group's name, what the create is
    /// answered with, and the template the group must launch from afterwards.
    type Row<'a> = (
        &'a str,
        &'a Vec<(String, String)>,
        &'a str,
        Expect,
        Option<String>,
    );
    let rows: Vec<Row<'_>> = vec![
        (
            "pointed at the granted template",
            &granted_source,
            "asg-per-template-granted",
            Expect::Served,
            Some(granted_id.clone()),
        ),
        (
            "pointed at the withheld template",
            &withheld_source,
            "asg-per-template-withheld",
            Expect::NoAllow,
            None,
        ),
        (
            "whose policy override names the withheld template",
            &policy_with_withheld_override,
            "asg-per-template-override",
            Expect::NoAllow,
            None,
        ),
    ];

    for (row, source, name, expected, wanted) in rows {
        let row_case = format!("{case}, {row}");
        let mut params = group_params(name, &subnets[0]);
        params.extend(source.iter().cloned());
        let answered = autoscaling_as(&router, &akid, "CreateAutoScalingGroup", &params).await;
        // Whichever template a refused row named, the refusal is about THAT one: the granted
        // template of the override row is permitted, so the row is refused on the withheld one.
        judge(
            &row_case,
            "CreateAutoScalingGroup",
            &withheld_id,
            expected,
            answered,
        );
        let held = reported_template(&router, &row_case, name).await;
        assert_eq!(
            held, wanted,
            "case {row_case:?}: after the create the account must hold {wanted:?} for {name:?}, \
             got {held:?}"
        );
    }
}
