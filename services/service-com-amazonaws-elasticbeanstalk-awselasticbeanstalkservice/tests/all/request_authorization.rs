// SPDX-License-Identifier: CC0-1.0

//! WHO may create, change or tear down an environment, driven end to end over
//! this service's awsQuery surface with the IAM service mounted beside it on one
//! shared engine.
//!
//! RULE: a call that creates, changes or tears down an environment — or composes
//! the group of environments an application runs — is decided against the
//! identity-based IAM policies in force on the principal the request's access key
//! id speaks for, as the action `elasticbeanstalk:<OperationName>` on the
//! resource the request names, BEFORE anything is read or written. A caller no
//! readable statement allows, one an explicit `Deny` covers, and one the session
//! policies its own credential was minted with do not allow are each refused
//! `AccessDenied` with HTTP 403 and the environment is left exactly as it was,
//! while a caller a matching `Allow` covers — and the account's own published
//! credential, which holds no policies — is served.
//!
//! The refusal names the principal, the action and the resource the request
//! addressed, and nothing else.
//!
//! `create_platform_version_authorization.rs` holds the same rule for the one
//! operation that creates a platform version; this suite holds it over the
//! environment plane, and spans what the rule generalises over: the OPERATION
//! (eight of them, naming the environment by name and by id), the document in
//! force (none, an allow of another action of this service, an explicit deny
//! beside an allow of everything, an allow of exactly the action, an allow of the
//! whole service), WHERE the cap comes from (an identity policy and a session
//! policy), and the principal (an assumed-role session and the account's own
//! credential).
//!
//! The role, its policies and the session live in the IAM store of the SAME
//! shared engine — the records `iam:CreateRole`, `iam:PutRolePolicy` and
//! `sts:AssumeRole` write — seeded the way
//! `create_platform_version_authorization.rs` seeds them. This suite holds its
//! OWN principal, so what decides a row here is the document this suite put on
//! its own role and nothing else.

use std::collections::BTreeMap;

use crate::harness::{wait_for_status, Answer, Emulator, SOLUTION_STACK};
use crate::lifecycle::{account, ACCOUNT_KEY_ID, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

/// The access key id the assumed-role session every row below signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEGUARD0001";

/// The role that session is assumed from, its session name, and the name of the
/// inline policy it carries.
const ROLE: &str = "eb-environment-operator";
const SESSION: &str = "guard-session";
const POLICY_NAME: &str = "inline";

/// The application and the environment every row acts on.
const APPLICATION: &str = "guarded-app";
const ENVIRONMENT: &str = "guarded-env";

/// The ARN the account holds the role under.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", account())
}

/// The principal ARN the seeded session speaks for, as `sts:AssumeRole` reports
/// it — what a refusal names.
fn session_arn() -> String {
    format!("arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}", account())
}

/// The ARN of the environment the rows act on, as this service builds it.
fn environment_arn() -> String {
    format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:environment/{APPLICATION}/{ENVIRONMENT}",
        account()
    )
}

/// The ARN of the application the composition acts on, as this service builds it.
fn application_arn() -> String {
    format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/{APPLICATION}",
        account()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for,
/// with `scope` as the session policies its mint passed.
fn given_session(emulator: &Emulator, scope: iam::SessionPolicies) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEEBGUARD01".to_string(),
            arn: role_arn(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let now = ::aws_smithy_types::DateTime::from(std::time::SystemTime::now());
    let expires = ::aws_smithy_types::DateTime::from_secs(now.secs() + 3_600);
    store.record_role_session(
        SESSION_KEY_ID,
        iam::RoleSessionRecord {
            session_arn: session_arn(),
            role_arn: role_arn(),
            expires,
            scope,
            tags: Vec::new(),
        },
        now,
    );
}

/// Put `document` on the role as its inline policy, as `iam:PutRolePolicy` does;
/// `None` leaves it carrying none.
fn given_role_policy(emulator: &Emulator, document: Option<&str>) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    let Some(document) = document else {
        store.role_inline_policies.remove(&iam::name_key(ROLE));
        return;
    };
    store.role_inline_policies.insert(
        iam::name_key(ROLE),
        BTreeMap::from([(
            iam::name_key(POLICY_NAME),
            iam::InlinePolicy {
                policy_name: POLICY_NAME.to_string(),
                policy_document: document.to_string(),
            },
        )]),
    );
}

/// A policy document allowing `actions` on every resource.
fn allow(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":{actions},"Resource":"*"}}]}}"#
    )
}

/// A policy document allowing every action of every service and denying
/// `actions`.
fn allow_everything_but(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},{{"Effect":"Deny","Action":{actions},"Resource":"*"}}]}}"#
    )
}

/// One operation under test: how the request names it, the parameters that
/// address the environment (or the application), and the resource ARN the
/// decision is taken on.
struct Operation {
    action: &'static str,
    params: Vec<(&'static str, String)>,
    resource: String,
}

/// Every operation the rule covers, naming the environment the way that
/// operation's own input does — by name, by id, and through the application for
/// the two that act on more than one environment.
fn operations(environment_id: &str) -> Vec<Operation> {
    let role = format!("arn:aws:iam::{}:role/eb-operations", account());
    vec![
        Operation {
            action: "AssociateEnvironmentOperationsRole",
            params: vec![
                ("EnvironmentName", ENVIRONMENT.to_string()),
                ("OperationsRole", role.clone()),
            ],
            resource: environment_arn(),
        },
        Operation {
            action: "DisassociateEnvironmentOperationsRole",
            params: vec![("EnvironmentName", ENVIRONMENT.to_string())],
            resource: environment_arn(),
        },
        Operation {
            action: "RebuildEnvironment",
            params: vec![("EnvironmentName", ENVIRONMENT.to_string())],
            resource: environment_arn(),
        },
        // The same environment named by its id instead: the resource the
        // decision is taken on is the environment, however the request addresses
        // it.
        Operation {
            action: "RestartAppServer",
            params: vec![("EnvironmentId", environment_id.to_string())],
            resource: environment_arn(),
        },
        Operation {
            action: "AbortEnvironmentUpdate",
            params: vec![("EnvironmentId", environment_id.to_string())],
            resource: environment_arn(),
        },
        Operation {
            action: "UpdateEnvironment",
            params: vec![
                ("EnvironmentName", ENVIRONMENT.to_string()),
                ("Description", "guarded".to_string()),
            ],
            resource: environment_arn(),
        },
        Operation {
            action: "TerminateEnvironment",
            params: vec![("EnvironmentName", ENVIRONMENT.to_string())],
            resource: environment_arn(),
        },
        Operation {
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("EnvironmentName", "second-env".to_string()),
                ("SolutionStackName", SOLUTION_STACK.as_str().to_string()),
            ],
            resource: format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:environment/{APPLICATION}/second-env",
                account()
            ),
        },
        Operation {
            action: "ComposeEnvironments",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("VersionLabels.member.1", "v1".to_string()),
            ],
            resource: application_arn(),
        },
    ]
}

/// `RestartAppServer` models no privileges error of its own, so a refusal of it
/// is the service-level one; the operations that CHANGE an environment are the
/// ones this rule covers, and a restart of its application server is one of them.
///
/// Create the application and the environment every row acts on, as the account,
/// and wait for the launch the clock owes so no row is refused for the launch
/// phase instead of for the decision under test.
async fn given_environment(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", ENVIRONMENT),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    wait_for_status(emulator, case, ENVIRONMENT, eb::state::STATUS_READY).await;
    launched.expect("EnvironmentId", case).to_string()
}

/// Call `operation` as `access_key_id`.
async fn call_as(emulator: &Emulator, access_key_id: &str, operation: &Operation) -> Answer {
    let params: Vec<(&str, &str)> = operation
        .params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    emulator
        .call_as(access_key_id, TEST_REGION, operation.action, &params)
        .await
}

/// What `DescribeEnvironments` reports for the environment the rows act on: its
/// status and its operations role, which is what says a refused call changed
/// nothing.
async fn reported(emulator: &Emulator, case: &str) -> (String, Option<String>) {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", ENVIRONMENT),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeEnvironments, got {} and body {}",
        described.status, described.body
    );
    (
        described
            .value("Environments/member/Status")
            .unwrap_or_default()
            .to_string(),
        described
            .value("Environments/member/OperationsRole")
            .map(str::to_string),
    )
}

/// RULE, over every operation it covers and every document a caller may carry.
#[tokio::test]
async fn an_environment_mutation_is_authorized_against_the_callers_policies() {
    // (case, the document in force, whether the call is served)
    let documents: [(&str, Option<String>, bool); 5] = [
        ("no policy at all", None, false),
        (
            "an allow of another action of this service",
            Some(allow("\"elasticbeanstalk:DescribeEnvironments\"")),
            false,
        ),
        (
            "an explicit deny beside an allow of everything",
            Some(allow_everything_but("\"elasticbeanstalk:*\"")),
            false,
        ),
        (
            "an allow of the whole service",
            Some(allow("\"elasticbeanstalk:*\"")),
            true,
        ),
        (
            "an allow of every action of every service",
            Some(allow("\"*\"")),
            true,
        ),
    ];

    for (document_case, document, served) in documents {
        // One emulator per row, so what a served mutation leaves behind cannot
        // decide the next row.
        let emulator = Emulator::with_identity_services();
        let environment_id = given_environment(&emulator, document_case).await;
        given_session(&emulator, iam::SessionPolicies::default());
        given_role_policy(&emulator, document.as_deref());

        for operation in operations(&environment_id) {
            let case = format!("{document_case}: {}", operation.action);
            let before = reported(&emulator, &case).await;
            let answer = call_as(&emulator, SESSION_KEY_ID, &operation).await;
            if served {
                // The rows above are about the ONE decision this suite holds the
                // service to: a served call may still be refused for something
                // else it needs (a role it may not pass, a resource another
                // service owns), but never for this gate.
                assert_ne!(
                    answer.error_code(),
                    "AccessDenied",
                    "case {case:?}: a caller the policies allow must not be refused by this gate, \
                     got body {}",
                    answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected HTTP 403, got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "AccessDenied",
                "case {case:?}: unexpected error code in {}",
                answer.body
            );
            // The refusal names the principal, the action and the resource the
            // request addressed — and nothing else.
            let expected = format!(
                "User: {} is not authorized to perform: elasticbeanstalk:{} on resource: {}",
                session_arn(),
                operation.action,
                operation.resource
            );
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
            // And the refused call performed nothing.
            assert_eq!(
                reported(&emulator, &case).await,
                before,
                "case {case:?}: a refused call must leave the environment as it was"
            );
        }
    }
}

/// RULE for the cap a SESSION POLICY sets: the permissions of a credential are
/// the intersection of the role's own policies and the session policies its mint
/// passed, so a session policy that does not allow the action refuses the call
/// even where the role's policy allows everything.
#[tokio::test]
async fn a_session_policy_caps_what_the_roles_policies_allow() {
    // (case, the session policies the mint passed, whether the call is served)
    let rows: [(&str, iam::SessionPolicies, bool); 3] = [
        (
            "no session policy at all",
            iam::SessionPolicies::default(),
            true,
        ),
        (
            "a session policy allowing another action",
            iam::SessionPolicies {
                inline: Some(allow("\"elasticbeanstalk:DescribeEnvironments\"")),
                ..iam::SessionPolicies::default()
            },
            false,
        ),
        (
            "a session policy allowing this action",
            iam::SessionPolicies {
                // The rebuild recreates the environment's instances through
                // Amazon EC2, which the scope has to allow as well — the point
                // of this row is the Elastic Beanstalk action, so that grant is
                // in the document too.
                inline: Some(allow(
                    "[\"elasticbeanstalk:RebuildEnvironment\",\"ec2:RunInstances\"]",
                )),
                ..iam::SessionPolicies::default()
            },
            true,
        ),
    ];

    for (case, scope, served) in rows {
        let emulator = Emulator::with_identity_services();
        given_environment(&emulator, case).await;
        given_session(&emulator, scope);
        // The ROLE may do everything, so what decides the row is the scope its
        // credential was minted with.
        given_role_policy(&emulator, Some(&allow("\"*\"")));

        let answer = emulator
            .call_as(
                SESSION_KEY_ID,
                TEST_REGION,
                "RebuildEnvironment",
                &[("EnvironmentName", ENVIRONMENT)],
            )
            .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the rebuild to be served, got {} and body {}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: unexpected error code in {}",
            answer.body
        );
    }
}

/// RULE for the work a call makes in ANOTHER service for the caller: a rebuild
/// "deletes and recreates all of the Amazon Web Services resources … for a
/// specified environment", and nothing it recreates exists without launching the
/// environment's Amazon EC2 instances again — so a caller EC2 would refuse that
/// launch to cannot have the environment rebuilt, and what the call reports is
/// the refusal EC2 answered: its own sentence, IAM's statement of what was
/// refused, and the envelope naming the called service, the status and that
/// call's id.
#[tokio::test]
async fn a_rebuild_reports_the_refusal_the_owning_service_answered() {
    // (case, the document in force on the caller, whether the rebuild is served)
    let rows: [(&str, String, bool); 3] = [
        (
            "a caller allowed this service's actions alone",
            allow("\"elasticbeanstalk:*\""),
            false,
        ),
        (
            "a caller denied the instance launch beside an allow of everything",
            allow_everything_but("\"ec2:RunInstances\""),
            false,
        ),
        (
            "a caller allowed the instance launch",
            allow("[\"elasticbeanstalk:*\",\"ec2:RunInstances\"]"),
            true,
        ),
    ];

    for (case, document, served) in rows {
        let emulator = Emulator::with_identity_services();
        given_environment(&emulator, case).await;
        given_session(&emulator, iam::SessionPolicies::default());
        given_role_policy(&emulator, Some(&document));

        let answer = emulator
            .call_as(
                SESSION_KEY_ID,
                TEST_REGION,
                "RebuildEnvironment",
                &[("EnvironmentName", ENVIRONMENT)],
            )
            .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the rebuild to be served, got {} and body {}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected error code in {}",
            answer.body
        );
        // The refusal the owning service answered, in the envelope a relayed
        // answer carries: its fixed sentence, IAM's own statement naming the
        // principal, the action and the resource, then the called service, the
        // status, the code and that call's own id.
        let message = answer.error_message();
        let opening = format!(
            "You are not authorized to perform this operation. User: {} is not authorized to \
             perform: ec2:RunInstances on resource: arn:aws:ec2:{TEST_REGION}:{}:instance/* ",
            session_arn(),
            account()
        );
        assert!(
            message.starts_with(&opening),
            "case {case:?}: expected the refusal to open {opening:?}, got {message:?}"
        );
        assert!(
            message.contains("(Service: AmazonEC2; Status Code: 403; Error Code: \
                              UnauthorizedOperation; Request ID: ")
                && message.ends_with("; Proxy: null)"),
            "case {case:?}: expected the relayed envelope naming the called service, the status, \
             the code and the call's id, got {message:?}"
        );
    }
}

/// RULE for the caller that holds no IAM policies: the account's own published
/// credential, which every other suite signs with, is served — so the refusals
/// above are about the principal the credential speaks for and not about the
/// gate being on.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let case = "the account's own published credential";
    let emulator = Emulator::with_identity_services();
    given_environment(&emulator, case).await;
    given_session(&emulator, iam::SessionPolicies::default());
    // A policy denying everything is in force on the ROLE, which this caller is
    // not.
    given_role_policy(&emulator, Some(&allow_everything_but("\"*\"")));

    let answer = emulator
        .call_as(
            ACCOUNT_KEY_ID,
            TEST_REGION,
            "RebuildEnvironment",
            &[("EnvironmentName", ENVIRONMENT)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the rebuild to be served, got {} and body {}",
        answer.status, answer.body
    );
}
