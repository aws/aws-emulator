// SPDX-License-Identifier: CC0-1.0

//! WHICH values name an environment's OPERATIONS ROLE, and what a role Elastic
//! Beanstalk cannot act under costs the calls that follow — driven end to end
//! over this service's awsQuery surface with the IAM service mounted beside it on
//! one shared engine, so the roles under test are the ones IAM holds.
//!
//! RULE 1 (an existing role): `OperationsRole` is "the ARN of an EXISTING IAM
//! role to be used as the environment's operations role", so a value IAM holds no
//! role under names nothing that can be associated: the request is refused
//! `InvalidParameterValue` with the sentence naming the ARN it rejected, and the
//! environment keeps whatever role it carried. A value IAM DOES hold a role under
//! is accepted, whatever that role's trust policy says.
//!
//! RULE 2 (the caller hands the role over): handing a role to a service is the
//! CALLER's act, so a caller IAM does not allow `iam:PassRole` on that role is
//! refused the operation's modelled privileges error with the service's own fixed
//! sentence for that grant, and nothing is associated.
//!
//! RULE 3 (the role is what later calls run under): "Elastic Beanstalk uses the
//! associated operations role for permissions to downstream services during
//! subsequent calls acting on this environment", so a call acting on an
//! environment whose operations role this service cannot assume — one whose trust
//! policy does not admit it — cannot be carried out, and is refused the
//! privileges error naming that role. Disassociating it is the way back: once the
//! role is off the environment, the same call is served.
//!
//! The rows span what the rules generalise over: WHICH operation carries the
//! member (`AssociateEnvironmentOperationsRole` and the launch that takes one from
//! the start), whether IAM holds the role, what its trust policy admits (this
//! service, another service, nothing at all), and whether the caller may pass it.

use std::collections::BTreeMap;

use crate::harness::{wait_for_status, Answer, Emulator, SOLUTION_STACK};
use crate::lifecycle::{account, ACCOUNT_KEY_ID, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

/// The application and the environment the rows act on.
const APPLICATION: &str = "ops-role-app";
const ENVIRONMENT: &str = "ops-role-env";

/// The access key id of the assumed-role session the pass-role rows sign with,
/// the role it speaks for and its session name.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEOPSROLE01";
const CALLER_ROLE: &str = "eb-operator";
const SESSION: &str = "ops-session";

/// The service principal an operations role has to admit, as a trust policy
/// spells this service — written out here rather than read off the crate, so the
/// suite holds the service to the published principal.
pub(crate) const ELASTIC_BEANSTALK_PRINCIPAL: &str = "elasticbeanstalk.amazonaws.com";

/// A trust policy admitting `principal` to assume the role, in the form
/// `iam:CreateRole` takes one.
pub(crate) fn trust(principal: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"Service":"{principal}"}},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// Record the IAM role `name` with the trust policy `trust_document`, as
/// `iam:CreateRole` records one, and answer its ARN.
pub(crate) fn given_role(
    emulator: &Emulator,
    name: &str,
    trust_document: Option<String>,
) -> String {
    let arn = format!("arn:aws:iam::{}:role/{name}", account());
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.roles.insert(
        iam::name_key(name),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: name.to_string(),
            role_id: format!("AROAEXAMPLE{:0>9}", name.len()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: trust_document,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// Record the assumed-role session [`SESSION_KEY_ID`] speaks for, carrying
/// `document` as the inline policy of the role behind it.
fn given_caller(emulator: &Emulator, document: &str) {
    let role_arn = given_role(emulator, CALLER_ROLE, Some(trust("ec2.amazonaws.com")));
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.insert(
        iam::name_key(CALLER_ROLE),
        BTreeMap::from([(
            iam::name_key("inline"),
            iam::InlinePolicy {
                policy_name: "inline".to_string(),
                policy_document: document.to_string(),
            },
        )]),
    );
    let now = ::aws_smithy_types::DateTime::from(std::time::SystemTime::now());
    let expires = ::aws_smithy_types::DateTime::from_secs(now.secs() + 3_600);
    store.record_role_session(
        SESSION_KEY_ID,
        iam::RoleSessionRecord {
            session_arn: format!(
                "arn:aws:sts::{}:assumed-role/{CALLER_ROLE}/{SESSION}",
                account()
            ),
            role_arn,
            expires,
            scope: iam::SessionPolicies::default(),
            tags: Vec::new(),
        },
        now,
    );
}

/// Create the application and the environment the rows act on, as the account,
/// and wait out the launch the clock owes.
async fn given_environment(emulator: &Emulator, case: &str) {
    for (action, params) in [
        ("CreateApplication", vec![("ApplicationName", APPLICATION)]),
        (
            "CreateEnvironment",
            vec![
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", ENVIRONMENT),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        ),
    ] {
        let answer = emulator.call(action, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from {action}, got {} and body {}",
            answer.status, answer.body
        );
    }
    wait_for_status(emulator, case, ENVIRONMENT, eb::state::STATUS_READY).await;
}

/// Associate `operations_role` with the environment, as the account.
async fn associate(emulator: &Emulator, operations_role: &str) -> Answer {
    emulator
        .call(
            "AssociateEnvironmentOperationsRole",
            &[
                ("EnvironmentName", ENVIRONMENT),
                ("OperationsRole", operations_role),
            ],
        )
        .await
}

/// The `OperationsRole` the environment reports, read back through the wire.
async fn stored_role(emulator: &Emulator, case: &str) -> Option<String> {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", ENVIRONMENT)],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeEnvironments, got {} and body {}",
        described.status, described.body
    );
    described
        .value("Environments/member/OperationsRole")
        .map(str::to_string)
}

/// RULE 1, over the values a request may name: whether IAM holds a role under the
/// name the ARN carries decides the association, and what that role trusts does
/// not.
#[tokio::test]
async fn an_operations_role_has_to_name_a_role_iam_holds() {
    let emulator = Emulator::with_identity_services();
    given_environment(&emulator, "setup").await;

    // The roles IAM holds for this account: one that admits this service, one
    // that admits another, and one that admits nothing at all.
    let trusted = given_role(
        &emulator,
        "eb-ops-trusted",
        Some(trust(ELASTIC_BEANSTALK_PRINCIPAL)),
    );
    let other_service = given_role(&emulator, "eb-ops-other", Some(trust("ec2.amazonaws.com")));
    let no_trust = given_role(&emulator, "eb-ops-untrusting", None);
    // And values IAM holds no role for: a never-created name, and a
    // never-created name under a path.
    let never_created = format!("arn:aws:iam::{}:role/eb-ops-never-created", account());
    let under_a_path = format!("arn:aws:iam::{}:role/team/eb-ops-absent", account());

    // (case, the value the request names, whether it is associated)
    let rows: [(&str, &str, bool); 5] = [
        ("a role admitting this service", trusted.as_str(), true),
        (
            "a role admitting another service",
            other_service.as_str(),
            true,
        ),
        ("a role admitting nothing", no_trust.as_str(), true),
        ("a name IAM holds no role under", never_created.as_str(), false),
        (
            "a name under a path IAM holds no role under",
            under_a_path.as_str(),
            false,
        ),
    ];

    for (case, operations_role, associated) in rows {
        let before = stored_role(&emulator, case).await;
        let answer = associate(&emulator, operations_role).await;
        if associated {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the association to be served, got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored_role(&emulator, case).await.as_deref(),
                Some(operations_role),
                "case {case:?}: the environment must report the role it was given"
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
        let expected =
            format!("Operations role ARN '{operations_role}' isn't valid. Specify an existing IAM role.");
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            stored_role(&emulator, case).await,
            before,
            "case {case:?}: a refused association must leave the environment's role alone"
        );
    }
}

/// RULE 1 through the OTHER operation that takes the member: a launch that
/// carries an operations role from the start is refused for a value naming no
/// existing role, and launches no environment for it.
#[tokio::test]
async fn a_launch_carrying_an_operations_role_resolves_it_too() {
    let case = "a launch naming a role IAM holds no role under";
    let emulator = Emulator::with_identity_services();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let never_created = format!("arn:aws:iam::{}:role/eb-ops-never-created", account());

    let refused = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", "launch-with-role"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("OperationsRole", never_created.as_str()),
            ],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: unexpected error code in {}",
        refused.body
    );
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", "launch-with-role"),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
    assert!(
        described
            .values_at("Environments/member/EnvironmentName")
            .is_empty(),
        "case {case:?}: a refused launch must record no environment, got {}",
        described.body
    );

    // The same launch naming a role IAM DOES hold is served, and the environment
    // reports it.
    let held = given_role(
        &emulator,
        "eb-ops-trusted",
        Some(trust(ELASTIC_BEANSTALK_PRINCIPAL)),
    );
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", "launch-with-role"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("OperationsRole", held.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case \"a launch naming a role IAM holds\": expected 200, got {} and body {}",
        launched.status, launched.body
    );
    assert_eq!(
        launched.value("OperationsRole"),
        Some(held.as_str()),
        "case \"a launch naming a role IAM holds\": the launch must report the role it stored, \
         got {}",
        launched.body
    );
}

/// RULE 2: the caller has to be allowed to hand the role over.
#[tokio::test]
async fn associating_a_role_needs_the_callers_permission_to_pass_it() {
    // (case, the document in force on the caller, whether the association is
    // served)
    let rows: [(&str, &str, bool); 3] = [
        (
            "a caller allowed this service's actions alone",
            r#"{"Statement":[{"Effect":"Allow","Action":"elasticbeanstalk:*","Resource":"*"}]}"#,
            false,
        ),
        (
            "a caller denied the pass beside an allow of everything",
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"},{"Effect":"Deny","Action":"iam:PassRole","Resource":"*"}]}"#,
            false,
        ),
        (
            "a caller allowed the pass",
            r#"{"Statement":[{"Effect":"Allow","Action":["elasticbeanstalk:*","iam:PassRole"],"Resource":"*"}]}"#,
            true,
        ),
    ];

    for (case, document, served) in rows {
        let emulator = Emulator::with_identity_services();
        given_environment(&emulator, case).await;
        let operations_role = given_role(
            &emulator,
            "eb-ops-trusted",
            Some(trust(ELASTIC_BEANSTALK_PRINCIPAL)),
        );
        given_caller(&emulator, document);

        let answer = emulator
            .call_as(
                SESSION_KEY_ID,
                TEST_REGION,
                "AssociateEnvironmentOperationsRole",
                &[
                    ("EnvironmentName", ENVIRONMENT),
                    ("OperationsRole", operations_role.as_str()),
                ],
            )
            .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the association to be served, got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored_role(&emulator, case).await.as_deref(),
                Some(operations_role.as_str()),
                "case {case:?}: the environment must report the role it was given"
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
            "InsufficientPrivilegesException",
            "case {case:?}: unexpected error code in {}",
            answer.body
        );
        // The service's own fixed sentence for that grant: it names the IAM
        // action and nothing of the request.
        assert_eq!(
            answer.error_message(),
            "Insufficient privileges for IAM PassRole Action.",
            "case {case:?}: unexpected refusal message in {}",
            answer.body
        );
        assert_eq!(
            stored_role(&emulator, case).await,
            None,
            "case {case:?}: a refused association must store no role"
        );
    }
}

/// RULE 3: what an operations role this service cannot assume costs the calls
/// that act on the environment afterwards.
#[tokio::test]
async fn a_call_acting_on_the_environment_needs_a_role_this_service_can_assume() {
    // (case, the trust policy of the associated role, whether later calls are
    // served)
    let rows: [(&str, Option<&str>, bool); 3] = [
        (
            "a role admitting this service",
            Some(ELASTIC_BEANSTALK_PRINCIPAL),
            true,
        ),
        ("a role admitting another service", Some("ec2.amazonaws.com"), false),
        ("a role admitting nothing", None, false),
    ];

    for (case, principal, served) in rows {
        let emulator = Emulator::with_identity_services();
        given_environment(&emulator, case).await;
        let operations_role = given_role(&emulator, "eb-ops-role", principal.map(trust));
        let associated = associate(&emulator, &operations_role).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: the association itself must be served, got {} and body {}",
            associated.status, associated.body
        );

        // A call acting on the environment: the rebuild, which Elastic Beanstalk
        // would make under that role.
        let answer = emulator
            .call_as(
                ACCOUNT_KEY_ID,
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
            "InsufficientPrivilegesException",
            "case {case:?}: unexpected error code in {}",
            answer.body
        );
        let message = answer.error_message();
        assert!(
            message.contains(&operations_role) && message.contains(ELASTIC_BEANSTALK_PRINCIPAL),
            "case {case:?}: the refusal must name the role and the principal its trust policy \
             must admit, got {message:?}"
        );

        // Disassociating the role is the way back: the same call is then served.
        let disassociated = emulator
            .call(
                "DisassociateEnvironmentOperationsRole",
                &[("EnvironmentName", ENVIRONMENT)],
            )
            .await;
        assert_eq!(
            disassociated.status, 200,
            "case {case:?}: expected 200 from DisassociateEnvironmentOperationsRole, got {} and \
             body {}",
            disassociated.status, disassociated.body
        );
        let after = emulator
            .call("RebuildEnvironment", &[("EnvironmentName", ENVIRONMENT)])
            .await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: once the role is off the environment the rebuild must be served, got \
             {} and body {}",
            after.status, after.body
        );
    }
}
