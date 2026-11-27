// SPDX-License-Identifier: CC0-1.0

//! `DisassociateEnvironmentOperationsRole`, driven end to end over the service's
//! awsQuery HTTP surface.
//!
//! The rule under test: disassociating removes the operations role STORED on the
//! environment the request names, leaving every other environment's role alone,
//! so every later read of that environment reports no operations role; a name
//! that resolves to no live environment is refused with `InvalidParameterValue`.
//!
//! Nothing here reaches into the store: a role is put there by `CreateEnvironment`
//! and read back through `DescribeEnvironments`, so a disassociate that persists
//! nothing fails these tests.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` already
//! owns — the router this service is mounted in, spoken to with a
//! form-urlencoded `Action=…` body.

use crate::lifecycle::{wait_for_status, Emulator, SOLUTION_STACK};

/// One environment the table below creates: the name it is created under, and
/// the operations role it is created with (`None` — created without one, so
/// there is nothing for the disassociate to remove).
struct EnvironmentCase {
    name: String,
    role: Option<String>,
}

/// The rows, differing along what the rule generalises over: the form and the
/// length of the stored role (including both modelled bounds), the length of the
/// environment name (including both modelled bounds), and whether a role was
/// stored at all.
///
/// The bounds come from the model:
/// `smithy_query(query="shape",shape="EnvironmentName,OperationsRole")` reports
/// `EnvironmentName` length 4..40 and `OperationsRole` length 1..256.
fn environment_cases() -> Vec<EnvironmentCase> {
    let account = engine::emulator_account_id();
    let role_prefix = format!("arn:aws:iam::{account}:role/");
    // Exactly the modelled maximum of 256 characters, padded in the role name.
    let longest_role = format!(
        "{role_prefix}{}",
        "r".repeat(256 - role_prefix.len())
    );
    vec![
        EnvironmentCase {
            // The modelled minimum name length of 4.
            name: "env1".to_string(),
            role: Some(format!("{role_prefix}aws-elasticbeanstalk-service-role")),
        },
        EnvironmentCase {
            name: "ops-role-env-with-a-path".to_string(),
            // An IAM role ARN may carry a path.
            role: Some(format!("{role_prefix}team/ops/eb-operations-role")),
        },
        EnvironmentCase {
            // The modelled maximum name length of 40.
            name: "e".repeat(40),
            role: Some(longest_role),
        },
        EnvironmentCase {
            name: "min-role-env".to_string(),
            // The modelled minimum role length of 1.
            role: Some("r".to_string()),
        },
        EnvironmentCase {
            name: "no-role-env".to_string(),
            role: None,
        },
    ]
}

/// The application every environment in this suite belongs to.
const APPLICATION: &str = "ops-role-app";

/// The `OperationsRole` `DescribeEnvironments` reports for `name`, read back
/// through the wire.
async fn stored_role(emulator: &Emulator, name: &str, case: &str) -> Option<String> {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", name)],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeEnvironments, got {} and body {}",
        described.status, described.body
    );
    assert_eq!(
        described.values_at("Environments/member/EnvironmentName"),
        vec![name],
        "case {case:?}: DescribeEnvironments must return exactly that environment, got {}",
        described.body
    );
    described
        .value("Environments/member/OperationsRole")
        .map(str::to_string)
}

#[tokio::test]
async fn disassociating_clears_only_the_named_environments_operations_role() {
    let emulator = Emulator::new();
    let cases = environment_cases();

    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", case.name.as_str()),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(role) = case.role.as_deref() {
            params.push(("OperationsRole", role));
        }
        let environment = emulator.call("CreateEnvironment", &params).await;
        assert_eq!(
            environment.status, 200,
            "case {:?}: expected 200 from CreateEnvironment, got {} and body {}",
            case.name, environment.status, environment.body
        );
        assert_eq!(
            environment.value("OperationsRole"),
            case.role.as_deref(),
            "case {:?}: CreateEnvironment must answer with the role it stored, got {}",
            case.name,
            environment.body
        );
        assert_eq!(
            stored_role(&emulator, &case.name, &case.name).await,
            case.role,
            "case {:?}: the role must survive the round trip through the store",
            case.name
        );
    }

    for (index, case) in cases.iter().enumerate() {
        let disassociated = emulator
            .call(
                "DisassociateEnvironmentOperationsRole",
                &[("EnvironmentName", case.name.as_str())],
            )
            .await;
        assert_eq!(
            disassociated.status, 200,
            "case {:?}: expected 200 from DisassociateEnvironmentOperationsRole, got {} and body {}",
            case.name, disassociated.status, disassociated.body
        );
        assert_eq!(
            disassociated.error_code(),
            "",
            "case {:?}: expected no error envelope, got body {}",
            case.name,
            disassociated.body
        );
        assert_eq!(
            stored_role(&emulator, &case.name, &case.name).await,
            None,
            "case {:?}: the environment must report no operations role after the disassociate",
            case.name
        );

        // Removing a role that is no longer there is the same removal: the
        // repeat call succeeds and the environment still reports none.
        let again = emulator
            .call(
                "DisassociateEnvironmentOperationsRole",
                &[("EnvironmentName", case.name.as_str())],
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {:?}: a repeated disassociate must still succeed, got {} and body {}",
            case.name, again.status, again.body
        );
        assert_eq!(
            stored_role(&emulator, &case.name, &case.name).await,
            None,
            "case {:?}: a repeated disassociate must leave the environment with no role",
            case.name
        );

        // Only the named environment changed: every environment this loop has
        // not reached yet still holds the role it was created with.
        for later in &cases[index + 1..] {
            assert_eq!(
                stored_role(&emulator, &later.name, &later.name).await,
                later.role,
                "case {:?}: disassociating {:?} must not touch this environment's role",
                later.name,
                case.name
            );
        }
    }
}

/// One request `DisassociateEnvironmentOperationsRole` must refuse, and the
/// error code it must be refused with.
struct RefusalCase {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    code: &'static str,
}

#[tokio::test]
async fn disassociate_refuses_a_name_that_names_no_live_environment() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", APPLICATION)])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    assert_eq!(
        emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", "dead-env"),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                    (
                        "OperationsRole",
                        &format!(
                            "arn:aws:iam::{}:role/eb-operations-role",
                            engine::emulator_account_id()
                        ),
                    ),
                ],
            )
            .await
            .status,
        200,
        "expected 200 from CreateEnvironment"
    );
    // A termination can only take down an environment whose launch has got past
    // its initial phase, which is what the clock owes it: waited for as a
    // CONDITION, never as a fixed time.
    wait_for_status(&emulator, "dead-env", "dead-env", "Ready").await;
    assert_eq!(
        emulator
            .call("TerminateEnvironment", &[("EnvironmentName", "dead-env")])
            .await
            .status,
        200,
        "expected 200 from TerminateEnvironment"
    );
    // The termination the clock owes, after which the name holds nothing: waited
    // for as the CONDITION the environment reaches, never as a fixed time.
    wait_for_status(&emulator, "dead-env", "dead-env", "Terminated").await;

    let cases = vec![
        RefusalCase {
            case: "a name no environment was ever created under",
            params: vec![("EnvironmentName", "ghost-env".to_string())],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "the name of an environment the clock has terminated",
            params: vec![("EnvironmentName", "dead-env".to_string())],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            // A value the MODEL refuses is a request-shape failure, so it carries
            // the request-validation code rather than `InvalidParameterValue`.
            case: "EnvironmentName under the modelled minimum of 4",
            params: vec![("EnvironmentName", "abc".to_string())],
            code: "ValidationError",
        },
        RefusalCase {
            case: "EnvironmentName over the modelled maximum of 40",
            params: vec![("EnvironmentName", "e".repeat(41))],
            code: "ValidationError",
        },
        RefusalCase {
            case: "the required EnvironmentName absent",
            params: vec![],
            code: "MissingRequiredParameter",
        },
    ];

    for RefusalCase { case, params, code } in &cases {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = emulator
            .call("DisassociateEnvironmentOperationsRole", &params)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected error code {code}, got body {}",
            refused.body
        );
        assert!(
            refused.value("Error/Message").is_some(),
            "case {case:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }
}
