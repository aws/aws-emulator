// SPDX-License-Identifier: CC0-1.0

//! Default Host Management Configuration (DHMC) end to end:
//! `RegisterDefaultHostManagementConfiguration` enables it for the account and Region the
//! request is signed for and `DeregisterDefaultHostManagementConfiguration` removes it.
//!
//! The rules under test:
//!
//! 1. A register RECORDS the role it registered in the one setting DHMC lives in —
//!    `/ssm/managed-instance/default-ec2-instance-management-role`, whose
//!    `ssm:GetServiceSetting`/`ssm:UpdateServiceSetting` permissions the operation documents
//!    — so `GetServiceSetting` after the call reports that role's NAME at the `Customized`
//!    status, and the call answers the role's ARN: the one the caller provided for a
//!    `SELF_MANAGED` registration, and the role Systems Manager keeps in the IAM reserved
//!    path for a `SERVICE_MANAGED` one.
//! 2. `Overwrite` is the guardrail: a register made while a role is already registered is
//!    refused unless it asked to replace it, and writes nothing; with `Overwrite` set it
//!    replaces the registration.
//! 3. A request whose `RoleArn` disagrees with its `PermissionModel` — absent for
//!    `SELF_MANAGED`, present for `SERVICE_MANAGED` — is refused and writes nothing.
//! 4. A deregister RESTORES that setting to the value its service team provisioned, so a
//!    later `GetServiceSetting` reports no registered role; it is idempotent, so a
//!    deregister of an account that registered nothing records nothing.
//! 5. The registration belongs to the Region its endpoint names: a register through one
//!    Region's endpoint leaves another Region's setting alone.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`call`], [`call_in_region`], [`router`]) dispatches `POST /`
//! with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router — and every
//! assertion about what was recorded is a READ through `GetServiceSetting`, so a response
//! that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, ACCOUNT, REGION};

/// The `SettingId` a DHMC registration lives under, as `GetServiceSetting` names it.
const SETTING_ID: &str = "/ssm/managed-instance/default-ec2-instance-management-role";

/// The value that setting reports while the account has registered no role: the sentinel
/// Systems Manager reports for an unconfigured name.
const UNSET_VALUE: &str = "$None";

/// `ServiceSetting$Status` for a setting at the value its service team provisioned.
const STATUS_DEFAULT: &str = "Default";

/// `ServiceSetting$Status` for a setting the account has written.
const STATUS_CUSTOMIZED: &str = "Customized";

/// The role Systems Manager keeps in the IAM reserved path for its own service principal,
/// which is what a `SERVICE_MANAGED` registration registers: "SERVICE_MANAGED The ARN of the
/// role that Amazon Web Services Systems Manager created in the IAM reserved path"
/// (`RegisterDefaultHostManagementConfigurationResult$RoleArn`). The reserved path is
/// `/aws-service-role/<service principal>/` and the name is the one IAM publishes for
/// `ssm.amazonaws.com`.
fn service_managed_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/aws-service-role/ssm.amazonaws.com/AWSServiceRoleForAmazonSSM")
}

/// The name that role is held under, which is what the setting records.
const SERVICE_MANAGED_ROLE_NAME: &str = "AWSServiceRoleForAmazonSSM";

/// A self-managed role at the account root, in the form `DHMCRoleArn` models
/// (`^arn:aws[a-z0-9-]*:iam::\d{12}:role/[\w+=,.@/-]+$`).
fn role_arn(path_and_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{path_and_name}")
}

/// The sentence a `SELF_MANAGED` request that names no role is refused with.
const ROLE_ARN_REQUIRED: &str =
    "RoleArn is required when PermissionModel is SELF_MANAGED: it names the self-managed IAM \
     role to register for Default Host Management Configuration.";

/// The sentence a `SERVICE_MANAGED` request that names one is refused with.
const ROLE_ARN_NOT_ALLOWED: &str =
    "RoleArn must not be set when PermissionModel is SERVICE_MANAGED: Amazon Web Services \
     Systems Manager provides the role for Default Host Management Configuration.";

/// The sentence a register that would replace a registration without asking to is refused
/// with.
const OVERWRITE_REQUIRED: &str =
    "A role is already registered for Default Host Management Configuration. Set Overwrite to \
     true to replace the currently registered role.";

/// `RegisterDefaultHostManagementConfiguration` with `body`, at `region`'s endpoint.
async fn register_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "RegisterDefaultHostManagementConfiguration",
        body,
    )
    .await
}

/// The same at [`REGION`]'s endpoint.
async fn register(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    register_in_region(router, REGION, body).await
}

/// `DeregisterDefaultHostManagementConfiguration` at `region`'s endpoint.
async fn deregister_in_region(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DeregisterDefaultHostManagementConfiguration",
        &json!({}),
    )
    .await
}

/// The `(SettingValue, Status)` the DHMC setting reports at `region`'s endpoint, read
/// through `GetServiceSetting` — the reader whose permissions both operations document.
async fn registered(router: &MultiServiceRouter, case: &str, region: &str) -> (String, String) {
    let (status, body) = call_in_region(
        router,
        region,
        "GetServiceSetting",
        &json!({ "SettingId": SETTING_ID }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetServiceSetting must succeed, got {status} {body}"
    );
    let setting = &body["ServiceSetting"];
    assert_eq!(
        setting["SettingId"].as_str(),
        Some(SETTING_ID),
        "case {case:?}: the read must report the setting asked for, got {body}"
    );
    (
        setting["SettingValue"].as_str().unwrap_or_default().to_string(),
        setting["Status"].as_str().unwrap_or_default().to_string(),
    )
}

/// RULE 1 and RULE 4 over the permission models and the roles each admits: a register
/// records the role it registered and answers its ARN, and a deregister takes it away again.
#[tokio::test]
async fn a_registration_is_recorded_in_the_setting_dhmc_lives_in() {
    /// One row: the request a register sends, the ARN it must answer, and the value the
    /// setting must then hold.
    struct Row {
        case: &'static str,
        request: Value,
        expected_arn: String,
        expected_value: String,
    }

    let rows = vec![
        Row {
            case: "a self-managed role at the account root",
            request: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("MyDefaultHostManagementRole"),
            }),
            expected_arn: role_arn("MyDefaultHostManagementRole"),
            expected_value: "MyDefaultHostManagementRole".to_string(),
        },
        Row {
            case: "a self-managed role under a path",
            request: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("service-role/AWSSystemsManagerDefaultEC2InstanceManagementRole"),
            }),
            expected_arn: role_arn("service-role/AWSSystemsManagerDefaultEC2InstanceManagementRole"),
            expected_value: "AWSSystemsManagerDefaultEC2InstanceManagementRole".to_string(),
        },
        Row {
            case: "a self-managed role whose name carries the characters the pattern admits",
            request: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("dhmc.role_1+2=3,x@example-role"),
            }),
            expected_arn: role_arn("dhmc.role_1+2=3,x@example-role"),
            expected_value: "dhmc.role_1+2=3,x@example-role".to_string(),
        },
        Row {
            case: "a service-managed registration, which names no role",
            request: json!({ "PermissionModel": "SERVICE_MANAGED" }),
            expected_arn: service_managed_role_arn(),
            expected_value: SERVICE_MANAGED_ROLE_NAME.to_string(),
        },
        Row {
            case: "a service-managed registration that states the Overwrite default",
            request: json!({ "PermissionModel": "SERVICE_MANAGED", "Overwrite": false }),
            expected_arn: service_managed_role_arn(),
            expected_value: SERVICE_MANAGED_ROLE_NAME.to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        // A router per row: the guardrail is about an account that already holds a
        // registration, which rule 2 owns.
        let router = router();

        // Nothing is registered before the call, whatever the row asks for.
        assert_eq!(
            registered(&router, case, REGION).await,
            (UNSET_VALUE.to_string(), STATUS_DEFAULT.to_string()),
            "case {case:?}: no role is registered before the register"
        );

        let (status, body) = register(&router, &row.request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the register must succeed, got {status} {body}"
        );
        assert_eq!(
            body["RoleArn"].as_str(),
            Some(row.expected_arn.as_str()),
            "case {case:?}: expected the register to answer {:?}, got {body}",
            row.expected_arn
        );

        // RULE 1: what the register recorded is what the reader reports.
        assert_eq!(
            registered(&router, case, REGION).await,
            (row.expected_value.clone(), STATUS_CUSTOMIZED.to_string()),
            "case {case:?}: expected the setting to hold {:?} at the Customized status",
            row.expected_value
        );
        // A read changes nothing: the second read reports what the first did.
        assert_eq!(
            registered(&router, case, REGION).await,
            (row.expected_value.clone(), STATUS_CUSTOMIZED.to_string()),
            "case {case:?}: a second read reports the same registration"
        );

        // RULE 4: the deregister restores the provisioned value, and is idempotent.
        for round in ["the first deregister", "the second deregister"] {
            let (status, body) = deregister_in_region(&router, REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?} {round}: must succeed, got {status} {body}"
            );
            assert_eq!(
                registered(&router, case, REGION).await,
                (UNSET_VALUE.to_string(), STATUS_DEFAULT.to_string()),
                "case {case:?} {round}: expected no registered role afterwards"
            );
        }
    }
}

/// RULE 2: `Overwrite` guards a register that would replace a registration — refused, and
/// nothing written — and admits it when the request asks for the replacement.
#[tokio::test]
async fn overwrite_guards_a_replacement_of_the_registered_role() {
    /// One row: the registration in force, and the register that follows it.
    struct Row {
        case: &'static str,
        first: Value,
        /// The value the setting holds after the first register.
        first_value: String,
        /// The register that follows, with no `Overwrite`.
        second: Value,
        /// The value the setting holds once the same request asks to overwrite.
        replaced_value: String,
    }

    let rows = vec![
        Row {
            case: "another self-managed role",
            first: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("FirstRole"),
            }),
            first_value: "FirstRole".to_string(),
            second: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("SecondRole"),
            }),
            replaced_value: "SecondRole".to_string(),
        },
        Row {
            case: "the same self-managed role again",
            first: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("FirstRole"),
            }),
            first_value: "FirstRole".to_string(),
            second: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("FirstRole"),
            }),
            replaced_value: "FirstRole".to_string(),
        },
        Row {
            case: "a service-managed registration over a self-managed one",
            first: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("FirstRole"),
            }),
            first_value: "FirstRole".to_string(),
            second: json!({ "PermissionModel": "SERVICE_MANAGED" }),
            replaced_value: SERVICE_MANAGED_ROLE_NAME.to_string(),
        },
        Row {
            case: "a self-managed registration over a service-managed one",
            first: json!({ "PermissionModel": "SERVICE_MANAGED" }),
            first_value: SERVICE_MANAGED_ROLE_NAME.to_string(),
            second: json!({
                "PermissionModel": "SELF_MANAGED",
                "RoleArn": role_arn("TakingOver"),
            }),
            replaced_value: "TakingOver".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = router();
        let (status, body) = register(&router, &row.first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first register must succeed, got {status} {body}"
        );

        // The guardrail is up: the second register is refused and changes nothing.
        let (status, body) = register(&router, &row.second).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                400,
                "ValidationException".to_string(),
                OVERWRITE_REQUIRED.to_string()
            ),
            "case {case:?}: a register over a registration must be refused, got {status} {body}"
        );
        assert_eq!(
            registered(&router, case, REGION).await,
            (row.first_value.clone(), STATUS_CUSTOMIZED.to_string()),
            "case {case:?}: the refused register must leave the registration in force"
        );

        // The same request asking to overwrite replaces it.
        let mut asking = row.second.clone();
        asking["Overwrite"] = json!(true);
        let (status, body) = register(&router, &asking).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a register asking to overwrite must succeed, got {status} {body}"
        );
        assert_eq!(
            registered(&router, case, REGION).await,
            (row.replaced_value.clone(), STATUS_CUSTOMIZED.to_string()),
            "case {case:?}: expected the registration to become {:?}",
            row.replaced_value
        );
    }
}

/// RULE 3: a `RoleArn` that disagrees with the `PermissionModel` is refused and writes
/// nothing, and so is a permission model outside the two the model declares.
#[tokio::test]
async fn a_role_that_disagrees_with_the_permission_model_is_refused() {
    // (case, the request, the sentence it is refused with — `None` where the boundary's own
    // clause words the refusal of a value outside the modelled enum)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a self-managed registration naming no role",
            json!({ "PermissionModel": "SELF_MANAGED" }),
            Some(ROLE_ARN_REQUIRED),
        ),
        (
            "a self-managed registration naming no role, asking to overwrite",
            json!({ "PermissionModel": "SELF_MANAGED", "Overwrite": true }),
            Some(ROLE_ARN_REQUIRED),
        ),
        (
            "a service-managed registration naming one",
            json!({
                "PermissionModel": "SERVICE_MANAGED",
                "RoleArn": role_arn("NotMineToChoose"),
            }),
            Some(ROLE_ARN_NOT_ALLOWED),
        ),
        (
            "a permission model outside the modelled enum",
            json!({ "PermissionModel": "SERVICE-MANAGED" }),
            None,
        ),
        (
            "no permission model at all, which the model requires",
            json!({}),
            None,
        ),
    ];

    for (case, request, expected) in &rows {
        let router = router();
        let (status, body) = register(&router, request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected a 400 ValidationException, got {status} {body}"
        );
        if let Some(expected) = expected {
            assert_eq!(
                error_message(&body),
                *expected,
                "case {case:?}: expected the refusal to state {expected:?}, got {body}"
            );
        }
        // Nothing was written: the account still has no registration.
        assert_eq!(
            registered(&router, case, REGION).await,
            (UNSET_VALUE.to_string(), STATUS_DEFAULT.to_string()),
            "case {case:?}: a refused register must write nothing"
        );
    }
}

/// RULE 5: the registration belongs to the Region its endpoint names, so a register through
/// one Region's endpoint leaves every other Region's setting alone — and a deregister there
/// takes away that Region's registration only.
#[tokio::test]
async fn a_registration_belongs_to_the_region_it_was_made_through() {
    // The Regions a row registers through, and the one it leaves alone.
    let elsewhere = "eu-central-1";
    let router = router();

    let (status, body) = register_in_region(
        &router,
        REGION,
        &json!({
            "PermissionModel": "SELF_MANAGED",
            "RoleArn": role_arn("RegionalRole"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first Region\": the register must succeed, got {status} {body}"
    );
    assert_eq!(
        registered(&router, "the first Region", REGION).await,
        ("RegionalRole".to_string(), STATUS_CUSTOMIZED.to_string()),
        "case \"the first Region\": expected its own registration"
    );
    assert_eq!(
        registered(&router, "another Region", elsewhere).await,
        (UNSET_VALUE.to_string(), STATUS_DEFAULT.to_string()),
        "case \"another Region\": a register elsewhere leaves this Region unregistered"
    );

    // That Region registers a role of its own, and the first one is untouched.
    let (status, body) = register_in_region(
        &router,
        elsewhere,
        &json!({ "PermissionModel": "SERVICE_MANAGED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another Region\": its own register must succeed, got {status} {body}"
    );
    assert_eq!(
        registered(&router, "another Region", elsewhere).await,
        (
            SERVICE_MANAGED_ROLE_NAME.to_string(),
            STATUS_CUSTOMIZED.to_string()
        ),
        "case \"another Region\": expected the role it registered"
    );
    assert_eq!(
        registered(&router, "the first Region", REGION).await,
        ("RegionalRole".to_string(), STATUS_CUSTOMIZED.to_string()),
        "case \"the first Region\": another Region's register leaves it alone"
    );

    // A deregister is regional too.
    let (status, body) = deregister_in_region(&router, elsewhere).await;
    assert_eq!(
        status, 200,
        "case \"another Region\": the deregister must succeed, got {status} {body}"
    );
    assert_eq!(
        registered(&router, "another Region", elsewhere).await,
        (UNSET_VALUE.to_string(), STATUS_DEFAULT.to_string()),
        "case \"another Region\": its registration is gone"
    );
    assert_eq!(
        registered(&router, "the first Region", REGION).await,
        ("RegionalRole".to_string(), STATUS_CUSTOMIZED.to_string()),
        "case \"the first Region\": a deregister elsewhere leaves it registered"
    );
}
