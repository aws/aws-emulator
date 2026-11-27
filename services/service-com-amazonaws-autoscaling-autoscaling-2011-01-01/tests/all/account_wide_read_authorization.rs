// SPDX-License-Identifier: CC0-1.0

//! WHO may drive Amazon EC2 Auto Scaling's account-wide reads — the `Describe` family —
//! driven end to end over the wire this service speaks (`awsQuery`), with the role, its
//! policies and the session held by the IAM and STS services mounted on the SAME engine, so
//! the documents under test are the ones `iam:PutRolePolicy` stores and the principal a
//! refusal blames is the one `sts:AssumeRole` minted.
//!
//! THE RULE UNDER TEST: an Auto Scaling `Describe` action supports no resource-level
//! permissions (the IAM Service Authorization Reference lists every
//! `autoscaling:Describe*` action under "actions with no resource types"), so IAM judges it
//! against EVERY resource: the service serves such a request only when the identity-based
//! policies in force on the principal its credentials name allow
//! `autoscaling:<OperationName>` over every resource, and otherwise refuses it with HTTP
//! 403 `AccessDenied` naming the principal, the action and which of IAM's rules withheld
//! the grant — without reading or changing anything. An action IAM judges against a
//! resource's own ARN is decided on the readings that ARN cannot change: a caller whose
//! documents hold no `Allow` of the action at all is refused, and a grant scoped to resource
//! ARNs reaches the operation's own answer.
//!
//! The rows span what the rule generalises over: the operation (three of the `Describe`
//! family, plus one operation outside it), the documents in force (none at all, an inline
//! allow of one action, an allow of the whole service, an allow in another case, a grant
//! scoped to one group's ARN, an explicit deny of one action and a wildcard deny over the
//! family), and the kind of credential (an assumed-role session, an IAM user's long-term
//! access key, the account's own published credential).

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use crate::support::{form_encode, tag_value, xml_unescape, REGION};

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The API versions a client sends in each service's query body.
const AUTOSCALING_API_VERSION: &str = "2011-01-01";
const IAM_API_VERSION: &str = "2010-05-08";
const STS_API_VERSION: &str = "2011-06-15";

/// This service's IAM action prefix and the code it refuses an unauthorized caller with,
/// spelled out here rather than imported, so a wrong constant in the implementation cannot
/// make a test pass.
const ACTION_PREFIX: &str = "autoscaling";
const ACCESS_DENIED: &str = "AccessDenied";

/// The three `Describe` operations the rows drive: each is its own `autoscaling:` action, so
/// a grant or a deny of one says nothing about the others.
const READS: [&str; 3] = [
    "DescribeTags",
    "DescribeAutoScalingGroups",
    "DescribeAccountLimits",
];

/// What a row expects a read to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force allow the action.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request allows it.
    NoAllow,
}

/// The launch template the group this suite creates launches from, put in the account's own
/// Amazon EC2 store by [`emulator`]: a group launches from a template the account holds.
const LAUNCH_TEMPLATE: &str = "lt-authorization";

/// The emulator the rows drive: Auto Scaling beside the IAM and STS services on ONE shared
/// engine, so the policies a row stores and the session it mints are the ones the gate
/// reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_launch_templates(&engine, &[LAUNCH_TEMPLATE]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One query-protocol call to `service`, presented as signed by the access key id `akid` —
/// the only place the credential appears on the wire, and so what the service reads the
/// caller's identity from.
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
    params: &[(&str, &str)],
) -> (u16, String) {
    call_as(
        router,
        "autoscaling",
        AUTOSCALING_API_VERSION,
        akid,
        action,
        params,
    )
    .await
}

/// One IAM call, made by the account's own published credential — the identity that
/// provisions the roles and users the rows are about.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_as(
        router,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        action,
        params,
    )
    .await
}

/// This emulator's account id, which every ARN below carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A trust policy admitting the account's own principals to assume the role, which is what
/// lets the published credential mint the session a row signs with.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        account()
    )
}

/// A one-statement identity-based document: `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create the role `role_name` with `inline` policies put on it, then assume it: answers
/// the session's access key id and the ARN `sts:GetCallerIdentity` reports for it, which a
/// refusal must name.
async fn session_of(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: &[(&str, String)],
) -> (String, String) {
    let trust = trust_policy();
    let (status, created) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be created, got {status} {created}"
    );
    for (policy_name, document) in inline {
        let (status, put) = iam_call(
            router,
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
    let role_arn = format!("arn:aws:iam::{}:role/{role_name}", account());
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

/// Create the IAM user `user_name` and mint its long-term access key: answers that key's id
/// and the user's ARN.
async fn user_key_of(router: &MultiServiceRouter, case: &str, user_name: &str) -> (String, String) {
    let (status, created) = iam_call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the user must be created, got {status} {created}"
    );
    let user_arn = tag_value(&created, "Arn");
    let (status, minted) = iam_call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the access key must be minted, got {status} {minted}"
    );
    (tag_value(&minted, "AccessKeyId"), user_arn)
}

/// The refusal this service words for `principal` asking for `action`, with the clause the
/// rule the decision was taken under states.
fn refusal_message(principal: &str, action: &str, expected: Expect) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} {}",
        refusal_reason(action, expected)
    )
}

/// The same, for an action judged against the ARN of the resource the request addresses:
/// the sentence names that resource between the action and the clause.
fn resource_refusal_message(
    principal: &str,
    action: &str,
    resource: &str,
    expected: Expect,
) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {}",
        refusal_reason(action, expected)
    )
}

/// The clause a refusal closes with, naming which of IAM's rules withheld the grant.
fn refusal_reason(action: &str, expected: Expect) -> String {
    match expected {
        Expect::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        Expect::NoAllow => format!("because no identity-based policy allows the {action} action"),
        Expect::Served => unreachable!("a served call carries no refusal"),
    }
}

/// Assert that the call `read` signed by `akid` answered what `expected` requires of it,
/// and — when it was refused — that the refusal carries this service's code and sentence.
async fn assert_read(
    router: &MultiServiceRouter,
    case: &str,
    akid: &str,
    principal: &str,
    read: &str,
    expected: Expect,
) {
    let (status, body) = autoscaling_as(router, akid, read, &[]).await;
    match expected {
        Expect::Served => assert_eq!(
            status, 200,
            "case {case:?} / {read}: expected the read to be served, got {status} {body}"
        ),
        refused => {
            assert_eq!(
                status, 403,
                "case {case:?} / {read}: expected HTTP 403, got {status} {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                ACCESS_DENIED,
                "case {case:?} / {read}: expected {ACCESS_DENIED}, got {body}"
            );
            let action = format!("{ACTION_PREFIX}:{read}");
            assert_eq!(
                xml_unescape(&tag_value(&body, "Message")),
                refusal_message(principal, &action, refused),
                "case {case:?} / {read}: expected IAM's refusal wording, got {body}"
            );
            assert!(
                !body.contains("on resource"),
                "case {case:?} / {read}: an action judged over every resource names none, \
                 got {body}"
            );
        }
    }
}

/// One row: the documents in force on the role the caller is a session of, and what each of
/// the three reads must answer.
struct Row {
    case: &'static str,
    role: &'static str,
    inline: Vec<(&'static str, String)>,
    /// What the three [`READS`] answer, in that order.
    expect: [Expect; 3],
}

/// RULE: an assumed-role session's Auto Scaling `Describe` is decided by the identity-based
/// documents in force on its ROLE, judged over every resource — an explicit `Deny` refuses
/// whatever any `Allow` says, an `Allow` covering the action over every resource serves it,
/// and with neither the request falls to IAM's default deny.
#[tokio::test]
async fn the_documents_in_force_decide_an_account_wide_read() {
    let group_arn = format!(
        "arn:aws:autoscaling:{REGION}:{}:autoScalingGroup:11111111-0000-4000-8000-000000000001:\
         autoScalingGroupName/web",
        account()
    );
    let rows = vec![
        Row {
            case: "a role carrying no policy at all",
            role: "bare-role",
            inline: Vec::new(),
            expect: [Expect::NoAllow; 3],
        },
        Row {
            case: "a role allowed every action of this service",
            role: "autoscaling-full-role",
            inline: vec![(
                "allow-autoscaling",
                statement("Allow", "autoscaling:*", "*"),
            )],
            expect: [Expect::Served; 3],
        },
        Row {
            case: "a role allowed exactly one of the reads",
            role: "tags-only-role",
            inline: vec![(
                "allow-describe-tags",
                statement("Allow", "autoscaling:DescribeTags", "*"),
            )],
            expect: [Expect::Served, Expect::NoAllow, Expect::NoAllow],
        },
        Row {
            case: "a role allowed the family with one read explicitly denied",
            role: "denied-tags-role",
            inline: vec![
                (
                    "allow-describes",
                    statement("Allow", "autoscaling:Describe*", "*"),
                ),
                (
                    "deny-describe-tags",
                    statement("Deny", "autoscaling:DescribeTags", "*"),
                ),
            ],
            expect: [Expect::ExplicitDeny, Expect::Served, Expect::Served],
        },
        Row {
            case: "a role allowed everything under a wildcard deny over the family",
            role: "wildcard-denied-role",
            inline: vec![
                (
                    "allow-autoscaling",
                    statement("Allow", "autoscaling:*", "*"),
                ),
                (
                    "deny-describes",
                    statement("Deny", "autoscaling:Describe*", "*"),
                ),
            ],
            expect: [Expect::ExplicitDeny; 3],
        },
        Row {
            case: "a role allowed the read in another case",
            // "IAM compares action names case-insensitively", so the spelling of the
            // statement's action is not what decides the call.
            role: "mixed-case-role",
            inline: vec![(
                "allow-mixed-case",
                statement("Allow", "AUTOSCALING:describetags", "*"),
            )],
            expect: [Expect::Served, Expect::NoAllow, Expect::NoAllow],
        },
        Row {
            case: "a role whose grant is scoped to one group's ARN",
            // A `Describe` action supports no resource-level permissions, so a statement
            // naming a particular ARN is not about a request IAM judges over every
            // resource — it grants nothing here, exactly as it grants nothing on AWS.
            role: "resource-scoped-role",
            inline: vec![(
                "allow-one-group",
                statement("Allow", "autoscaling:DescribeTags", &group_arn),
            )],
            expect: [Expect::NoAllow; 3],
        },
    ];

    let router = emulator();
    for row in &rows {
        let (akid, principal) = session_of(&router, row.case, row.role, &row.inline).await;
        for (read, expected) in READS.iter().zip(row.expect.iter()) {
            assert_read(&router, row.case, &akid, &principal, read, *expected).await;
        }
    }
}

/// RULE: the principal a request is judged on is the one its access key id speaks for — the
/// session STS minted the key for, the IAM user that holds a long-term key, or the account's
/// own published credential, which holds no IAM policies and is therefore not bounded by
/// them.
#[tokio::test]
async fn the_credential_decides_which_principal_the_read_is_judged_on() {
    let router = emulator();

    // The account's own published credential: a key neither store holds a record for.
    for read in READS {
        let (status, body) = autoscaling_as(&router, ACCOUNT_KEY_ID, read, &[]).await;
        assert_eq!(
            status, 200,
            "case \"the account's own credential\" / {read}: expected the read to be served, \
             got {status} {body}"
        );
    }

    // An IAM user holding no policy at all is refused; the same user, once a policy
    // allowing the family is put on it, is served.
    let case = "an IAM user with no policy";
    let (user_akid, user_arn) = user_key_of(&router, case, "bare-user").await;
    for read in READS {
        assert_read(&router, case, &user_akid, &user_arn, read, Expect::NoAllow).await;
    }
    let (status, put) = iam_call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bare-user"),
            ("PolicyName", "allow-describes"),
            (
                "PolicyDocument",
                &statement("Allow", "autoscaling:Describe*", "*"),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the user's policy must be stored, got {status} {put}"
    );
    for read in READS {
        assert_read(
            &router,
            "an IAM user allowed the family",
            &user_akid,
            &user_arn,
            read,
            Expect::Served,
        )
        .await;
    }
}

/// RULE: an action IAM judges against the ARN of the resource the request addresses is
/// decided on the readings that ARN cannot change — a caller whose documents hold no `Allow`
/// of the action is refused 403, and a grant scoped to resource ARNs reaches the operation's
/// own answer, because the resources it covers are not ones this gate read.
#[tokio::test]
async fn an_action_judged_against_a_resource_is_decided_on_the_action_alone() {
    let router = emulator();
    // (the action, the parameters a client sends with it)
    let rows: &[(&str, &[(&str, &str)])] = &[
        (
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", "no-such-group"),
                ("DesiredCapacity", "1"),
            ],
        ),
        (
            "DeleteAutoScalingGroup",
            &[("AutoScalingGroupName", "no-such-group")],
        ),
    ];

    let case = "a role carrying no policy at all";
    let (akid, _) = session_of(&router, case, "resource-action-role", &[]).await;
    for (action, params) in rows {
        let (status, body) = autoscaling_as(&router, &akid, action, params).await;
        assert_eq!(
            status, 403,
            "case {case:?} / {action}: no readable document allows the action at all, got \
             {status} {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            ACCESS_DENIED,
            "case {case:?} / {action}: expected {ACCESS_DENIED}, got {status} {body}"
        );
    }

    let case = "a role whose grant of the action is scoped to one group's ARN";
    let scoped = format!(
        "arn:aws:autoscaling:{REGION}:{}:autoScalingGroup:*:autoScalingGroupName/no-such-group",
        account()
    );
    let (akid, _) = session_of(
        &router,
        case,
        "resource-scoped-role",
        &[
            (
                "allow-on-one-group",
                statement("Allow", "autoscaling:*", &scoped),
            ),
        ],
    )
    .await;
    for (action, params) in rows {
        let (status, body) = autoscaling_as(&router, &akid, action, params).await;
        assert_ne!(
            status, 403,
            "case {case:?} / {action}: a resource-scoped grant is left to the operation, got \
             {status} {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?} / {action}: expected the operation's own refusal, got {status} \
             {body}"
        );
    }
}

/// RULE: the decision is taken before the operation reads anything, so a refused read
/// reports nothing of the account's estate and leaves it exactly as it was — the same tag is
/// still reported to a caller the policies allow.
#[tokio::test]
async fn a_refused_read_reports_nothing_and_changes_nothing() {
    let router = emulator();
    let case = "a group the account holds";
    let (status, created) = autoscaling_as(
        &router,
        ACCOUNT_KEY_ID,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "authorized-reads"),
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ("Tags.member.1.Key", "env"),
            ("Tags.member.1.Value", "prod"),
            ("Tags.member.1.PropagateAtLaunch", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the group must be created, got {status} {created}"
    );

    let (akid, principal) = session_of(
        &router,
        case,
        "estate-denied-role",
        &[
            (
                "allow-describes",
                statement("Allow", "autoscaling:Describe*", "*"),
            ),
            (
                "deny-describe-tags",
                statement("Deny", "autoscaling:DescribeTags", "*"),
            ),
        ],
    )
    .await;
    let (status, refused) = autoscaling_as(&router, &akid, "DescribeTags", &[]).await;
    assert_eq!(
        status, 403,
        "case {case:?}: the denied read must be refused, got {status} {refused}"
    );
    assert!(
        !refused.contains("env") && !refused.contains("prod"),
        "case {case:?}: a refused read discloses no tag of the account, got {refused}"
    );
    assert_eq!(
        xml_unescape(&tag_value(&refused, "Message")),
        refusal_message(
            &principal,
            &format!("{ACTION_PREFIX}:DescribeTags"),
            Expect::ExplicitDeny
        ),
        "case {case:?}: expected IAM's refusal wording, got {refused}"
    );

    // Read back through the same operation as a caller the policies allow: the estate the
    // refusal did not disclose is still exactly what the account holds.
    let (status, reported) = autoscaling_as(&router, ACCOUNT_KEY_ID, "DescribeTags", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the permitted read must be served, got {status} {reported}"
    );
    assert!(
        reported.contains("env") && reported.contains("prod"),
        "case {case:?}: expected the group's tag to still be reported, got {reported}"
    );
}

/// RULE: a MUTATION is authorized before it touches the store, so a caller whose documents
/// hold no `Allow` of the action is refused 403 `AccessDenied` and nothing is created; a
/// caller the documents allow is served.
///
/// The rows span the documents in force on the principal about THIS service: none at all, an
/// allow of the whole service, an allow of this one action, and an explicit deny beside that
/// allow.
#[tokio::test]
async fn a_mutation_is_authorized_before_it_reaches_the_store() {
    let router = emulator();
    let action = format!("{ACTION_PREFIX}:CreateAutoScalingGroup");
    /// One row: the case, the role's name, its inline policies, and what the create answers.
    type MutationRow = (&'static str, &'static str, Vec<(&'static str, String)>, Expect);
    // A group created from a launch template launches its instances from that template, so
    // every row carries the Amazon EC2 grant that launch needs as well: what a row is judged
    // on here is the AUTO SCALING action its documents state, which is this suite's subject.
    let launch_allowed = || ("allow-launch", statement("Allow", "ec2:RunInstances", "*"));
    // (case, the role's inline policies, what the create must answer)
    let rows: Vec<MutationRow> = vec![
        (
            "a role carrying no policy of this service at all",
            "create-bare-role",
            vec![launch_allowed()],
            Expect::NoAllow,
        ),
        (
            "a role allowed the whole service",
            "create-service-role",
            vec![
                ("allow-all", statement("Allow", "autoscaling:*", "*")),
                launch_allowed(),
            ],
            Expect::Served,
        ),
        (
            "a role allowed exactly this action",
            "create-action-role",
            vec![
                (
                    "allow-create",
                    statement("Allow", "autoscaling:CreateAutoScalingGroup", "*"),
                ),
                launch_allowed(),
            ],
            Expect::Served,
        ),
        (
            "a role whose allow is overridden by a deny",
            "create-denied-role",
            vec![
                ("allow-all", statement("Allow", "autoscaling:*", "*")),
                (
                    "deny-create",
                    statement("Deny", "autoscaling:CreateAutoScalingGroup", "*"),
                ),
                launch_allowed(),
            ],
            Expect::ExplicitDeny,
        ),
    ];

    for (index, (case, role_name, inline, expected)) in rows.iter().enumerate() {
        let (akid, principal) = session_of(&router, case, role_name, inline).await;
        let name = format!("asg-authorized-{index}");
        let (status, body) = autoscaling_as(
            &router,
            &akid,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ],
        )
        .await;
        if *expected == Expect::Served {
            assert_eq!(
                status, 200,
                "case {case:?}: the permitted create must be served, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: the unauthorized create must be refused, got {status} {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {body}"
            );
            // The action is judged against the ARN of the group the request addresses, and a
            // request names the group by NAME: the resource id segment is the wildcard, the
            // group having none of its own until it is created.
            let resource = format!(
                "arn:aws:autoscaling:{REGION}:{}:autoScalingGroup:*:autoScalingGroupName/{name}",
                account()
            );
            assert_eq!(
                xml_unescape(&tag_value(&body, "Message")),
                resource_refusal_message(&principal, &action, &resource, *expected),
                "case {case:?}: expected IAM's refusal wording, got {body}"
            );
        }
        // What the account holds is read back as a caller the policies allow: a refused
        // create left no group behind.
        let (status, described) = autoscaling_as(
            &router,
            ACCOUNT_KEY_ID,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read back must be served, got {status} {described}"
        );
        assert_eq!(
            described.contains(name.as_str()),
            *expected == Expect::Served,
            "case {case:?}: expected the group to exist only when the create was served, got \
             {described}"
        );
    }
}
