// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which `AssociateEnvironmentOperationsRole` resolves what the
//! request names, driven end to end over this service's awsQuery surface with the
//! IAM service mounted beside it on one shared engine — so the roles under test
//! are the ones IAM really holds, and a role nobody holds is really absent.
//!
//! The rule under test: an operation that acts on an existing environment
//! resolves the environment its request names against this service's state
//! FIRST, and a name no running environment of the account answers to is refused
//! as not found before any other value of the request is checked against another
//! service's state. So a request that is wrong in both respects — a name nothing
//! carries and a role IAM holds nothing under — is refused for the environment,
//! and the refusal names the environment alone; a caller keyed on the message
//! then repairs the target it really lost instead of a role that was never
//! reached.
//!
//! The rows span what the rule generalises over: the FORM of the environment name
//! (a name never created, the shortest and the longest the model allows for
//! `EnvironmentName` — `len 4..40` — and a name whose environment existed and
//! whose termination has completed, which is absent to a call that acts on a
//! running one) crossed with the operations-role value (a name IAM holds no role
//! under, such a name under a path, a value that is no ARN at all, and a role IAM
//! DOES hold — which must not change the answer either).
//!
//! The other half of the rule is pinned below: for an environment that IS there,
//! the role is still resolved and a value naming none is still refused in the
//! role's own wording, so the precedence is an ORDER and not a dropped check.
//!
//! The templates are written out HERE rather than imported from the service, so
//! a change to the service's builders that changes the wire wording fails this
//! suite instead of moving with it.

use crate::harness::{wait_for_status, Answer, Emulator, SOLUTION_STACK};
use crate::lifecycle::account;
use crate::operations_role_validation::{given_role, trust, ELASTIC_BEANSTALK_PRINCIPAL};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the fixture environments are launched under.
const APPLICATION: &str = "ops-role-order-app";

/// The environment that is launched, settled and then terminated: it EXISTED,
/// and a call that acts on a running environment finds it absent.
const TERMINATED: &str = "ops-role-order-gone";

/// The environment that stays running, for the rows that show the role is still
/// resolved once the target is there.
const RUNNING: &str = "ops-role-order-live";

/// The modelled minimum length of `EnvironmentName` (`len 4..40`), from the
/// member's own constraint in this service's Smithy model.
const ENVIRONMENT_NAME_MIN: usize = 4;
/// The modelled maximum length of `EnvironmentName` (`len 4..40`).
const ENVIRONMENT_NAME_MAX: usize = 40;

/// The template this service words a missing environment with for this
/// operation: the member the caller used and the value it carried.
fn no_environment_found(environment_name: &str) -> String {
    format!("No Environment found for EnvironmentName = '{environment_name}'.")
}

/// The template this service refuses an operations role naming no existing IAM
/// role with: the ARN the request supplied, and the service's fixed sentence.
fn invalid_operations_role(operations_role: &str) -> String {
    format!("Operations role ARN '{operations_role}' isn't valid. Specify an existing IAM role.")
}

/// Create the application the environments are launched under.
async fn given_application(emulator: &Emulator, case: &str) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// Launch `name` and wait out the launch the clock owes.
async fn given_environment(emulator: &Emulator, case: &str, name: &str) {
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", name),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment for {name:?}, got {} and body {}",
        launched.status, launched.body
    );
    wait_for_status(emulator, case, name, eb::state::STATUS_READY).await;
}

/// Associate `operations_role` with the environment `environment_name`.
async fn associate(emulator: &Emulator, environment_name: &str, operations_role: &str) -> Answer {
    emulator
        .call(
            "AssociateEnvironmentOperationsRole",
            &[
                ("EnvironmentName", environment_name),
                ("OperationsRole", operations_role),
            ],
        )
        .await
}

/// The `OperationsRole` the environment `environment_name` reports, read back
/// through the wire.
async fn stored_role(emulator: &Emulator, case: &str, environment_name: &str) -> Option<String> {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment_name)],
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

/// A request naming no running environment is refused for the ENVIRONMENT,
/// whatever the operations role it carries names.
#[tokio::test]
async fn an_absent_environment_is_reported_before_the_operations_role() {
    let case = "the fixture";
    let emulator = Emulator::with_identity_services();
    given_application(&emulator, case).await;
    given_environment(&emulator, case, TERMINATED).await;
    let terminated = emulator
        .call("TerminateEnvironment", &[("EnvironmentName", TERMINATED)])
        .await;
    assert_eq!(
        terminated.status, 200,
        "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    wait_for_status(&emulator, case, TERMINATED, eb::state::STATUS_TERMINATED).await;

    // The role values a row may name: one IAM holds, and three it holds nothing
    // under.
    let held = given_role(
        &emulator,
        "eb-ops-order-held",
        Some(trust(ELASTIC_BEANSTALK_PRINCIPAL)),
    );
    let absent = format!("arn:aws:iam::{}:role/eb-ops-order-absent", account());
    let under_a_path = format!("arn:aws:iam::{}:role/team/eb-ops-order-absent", account());
    let not_an_arn = "eb-ops-order-not-an-arn".to_string();

    // The environment names a row may carry: never created, at each modelled
    // length bound, and one whose termination has completed.
    let never_created = "ops-role-order-ghost".to_string();
    let shortest = "g".repeat(ENVIRONMENT_NAME_MIN);
    let longest = "g".repeat(ENVIRONMENT_NAME_MAX);

    // (case, the EnvironmentName the request carries, the OperationsRole it carries)
    let rows: [(&str, &str, &str); 8] = [
        (
            "a name never created, a role IAM holds nothing under",
            never_created.as_str(),
            absent.as_str(),
        ),
        (
            "a name never created, a name under a path IAM holds nothing under",
            never_created.as_str(),
            under_a_path.as_str(),
        ),
        (
            "a name never created, a value that is no ARN",
            never_created.as_str(),
            not_an_arn.as_str(),
        ),
        (
            "a name never created, a role IAM DOES hold",
            never_created.as_str(),
            held.as_str(),
        ),
        (
            "the shortest name the model allows, a role IAM holds nothing under",
            shortest.as_str(),
            absent.as_str(),
        ),
        (
            "the longest name the model allows, a role IAM holds nothing under",
            longest.as_str(),
            absent.as_str(),
        ),
        (
            "an environment whose termination completed, a role IAM holds nothing under",
            TERMINATED,
            absent.as_str(),
        ),
        (
            "an environment whose termination completed, a role IAM DOES hold",
            TERMINATED,
            held.as_str(),
        ),
    ];

    for (case, environment_name, operations_role) in rows {
        let refused = associate(&emulator, environment_name, operations_role).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = no_environment_found(environment_name);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        // The refusal is about the environment alone: it neither echoes the role
        // the request carried nor advertises repairing one.
        let message = refused.error_message();
        assert!(
            !message.contains(operations_role) && !message.contains("Operations role"),
            "case {case:?}: the refusal must name the environment alone, got {message:?}"
        );
    }

    // The terminated environment carries no role either: a refused association
    // writes nothing.
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", TERMINATED),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
    assert!(
        described
            .values_at("Environments/member/OperationsRole")
            .is_empty(),
        "case {case:?}: a refused association must store no role, got body {}",
        described.body
    );
}

/// The other half of the order: once the environment IS there, the operations
/// role is resolved and a value naming no existing role is still refused in the
/// role's own wording — and a value naming one is associated and read back.
#[tokio::test]
async fn an_existing_environment_still_resolves_the_operations_role() {
    let case = "an environment that is there";
    let emulator = Emulator::with_identity_services();
    given_application(&emulator, case).await;
    given_environment(&emulator, case, RUNNING).await;

    let absent = format!("arn:aws:iam::{}:role/eb-ops-order-absent", account());
    let refused = associate(&emulator, RUNNING, &absent).await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {}",
        refused.body
    );
    let expected = invalid_operations_role(&absent);
    assert_eq!(
        refused.error_message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        refused.error_message()
    );
    assert_eq!(
        stored_role(&emulator, case, RUNNING).await,
        None,
        "case {case:?}: a refused association must store no role"
    );

    // A role IAM holds is associated, and every later read of the environment
    // reports it.
    let held = given_role(
        &emulator,
        "eb-ops-order-held",
        Some(trust(ELASTIC_BEANSTALK_PRINCIPAL)),
    );
    let associated = associate(&emulator, RUNNING, &held).await;
    assert_eq!(
        associated.status, 200,
        "case {case:?}: expected the association to be served, got {} and body {}",
        associated.status, associated.body
    );
    assert_eq!(
        stored_role(&emulator, case, RUNNING).await.as_deref(),
        Some(held.as_str()),
        "case {case:?}: the environment must report the role it was given"
    );
}
