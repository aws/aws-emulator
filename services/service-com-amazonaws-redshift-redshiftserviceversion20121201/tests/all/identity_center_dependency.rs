// SPDX-License-Identifier: CC0-1.0

//! The IAM Identity Center dependency an application registration has.
//!
//! RULE under test: the managed application a Redshift (or Query Editor) IAM Identity
//! Center application is backed by is a resource OF the Identity Center instance the
//! create names, so the create performs `sso:CreateApplication` in that instance on the
//! caller's behalf — an instance the account's Identity Center does not hold has no
//! application namespace to create one in, the dependent call is denied, and the create
//! is refused with the modelled `DependentServiceAccessDeniedFault` (HTTP 403) carrying
//! the standard IAM authorization-failure sentence: the CALLING PRINCIPAL, the action,
//! the Identity Center application resource of the named instance, and that no
//! resource-based policy allows that action. Nothing is registered, and a create naming
//! an instance the account DOES hold is served.
//!
//! The rows differ along what the rule generalises over: which operation registers the
//! application (both creates that take an `IdcInstanceArn`), the form of the instance id
//! and the partition the instance ARN carries, whether the instance is one the account
//! holds, and what else the create asks for (optional members, tags, token issuers) —
//! which the dependency cannot depend on.
//!
//! Every request goes over the real awsQuery wire through the shared harness
//! ([`crate::cluster_lifecycle`]), so what is asserted is what an SDK sees.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, IDENTITY_CENTER_INSTANCES, REGION};

/// The IAM Identity Center action a registration performs on the caller's behalf, as IAM
/// names it.
const ACTION: &str = "sso:CreateApplication";

/// The error code and HTTP status the dependent denial is relayed on
/// (`smithy_query(query="operation", operation="CreateRedshiftIdcApplication,CreateQev2IdcApplication")`
/// — `DependentServiceAccessDeniedFault`, httpError 403, awsQueryError code
/// `DependentServiceAccessDenied`).
const DENIED_CODE: &str = "DependentServiceAccessDenied";
const DENIED_STATUS: u16 = 403;

/// The IAM role an application assumes to invoke the IAM Identity Center API.
const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/RedshiftIdcRole";

/// Instance ARNs that name no instance this account holds, in the forms the owning
/// service's `InstanceArn` pattern admits (`^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`):
/// both spellings of the instance id, one carrying the punctuation the pattern allows,
/// and one in another partition — whose id is held in `aws`, so the partition is part of
/// what identifies the instance.
const NOT_HELD: [&str; 4] = [
    "arn:aws:sso:::instance/ssoins-0000000000000000",
    "arn:aws:sso:::instance/ins-ffffffffffffffff",
    "arn:aws:sso:::instance/ssoins-z.y-x.w-v.u-t1s2",
    "arn:aws-us-gov:sso:::instance/ssoins-1111111111111111",
];

/// The instance id inside an instance ARN.
fn instance_id(instance_arn: &str) -> &str {
    instance_arn
        .rsplit('/')
        .next()
        .expect("an instance ARN names an instance")
}

/// The partition an ARN carries.
fn partition(instance_arn: &str) -> &str {
    instance_arn
        .split(':')
        .nth(1)
        .expect("an ARN carries a partition")
}

/// The sentence IAM states for a denied cross-service action, written here from the rule
/// rather than from the implementation: the calling principal, the action, the Identity
/// Center APPLICATION resource of the named instance (the account and the application id
/// are the wildcards the decision is evaluated over, since the application has no id
/// until the call that would create it succeeds), and that no resource-based policy
/// allows it.
fn expected_message(principal: &str, instance_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ACTION} on resource: \
         arn:{}:sso::*:application/{}/* because no resource-based policy allows the \
         {ACTION} action",
        partition(instance_arn),
        instance_id(instance_arn)
    )
}

/// The principal every request of this suite is refused as: the harness signs with the
/// emulator's own long-term credential, and the account contract states which principal
/// that credential speaks for — the same one `sts:GetCallerIdentity` reports.
fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// `CreateRedshiftIdcApplication` naming `instance_arn`, with the extra parameters a row
/// carries.
async fn create_redshift_application(
    emulator: &Emulator,
    name: &str,
    instance_arn: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("IdcInstanceArn", instance_arn),
        ("RedshiftIdcApplicationName", name),
        ("IdcDisplayName", "RedshiftIdc"),
        ("IamRoleArn", IAM_ROLE_ARN),
    ];
    params.extend_from_slice(extra);
    emulator.call("CreateRedshiftIdcApplication", &params).await
}

/// `CreateQev2IdcApplication` naming `instance_arn`, with the extra parameters a row
/// carries.
async fn create_qev2_application(
    emulator: &Emulator,
    name: &str,
    instance_arn: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("IdcInstanceArn", instance_arn),
        ("Qev2IdcApplicationName", name),
        ("IdcDisplayName", "Qev2Idc"),
    ];
    params.extend_from_slice(extra);
    emulator.call("CreateQev2IdcApplication", &params).await
}

/// Which registration a row drives, and the read that reports what it registered.
#[derive(Debug, Clone, Copy)]
enum Registration {
    Redshift,
    Qev2,
}

impl Registration {
    /// The `Describe*` action that lists what this registration stores.
    fn describe(self) -> &'static str {
        match self {
            Self::Redshift => "DescribeRedshiftIdcApplications",
            Self::Qev2 => "DescribeQev2IdcApplications",
        }
    }

    /// The element each listed application carries its NAME in.
    fn name_member(self) -> &'static str {
        match self {
            Self::Redshift => "RedshiftIdcApplicationName",
            Self::Qev2 => "Qev2IdcApplicationName",
        }
    }

    /// The `IdcOnboardStatus` a registration of this kind is reported with.
    fn onboard_status(self) -> &'static str {
        match self {
            Self::Redshift => state::REDSHIFT_IDC_ONBOARD_STATUS,
            Self::Qev2 => state::QEV2_IDC_ONBOARD_STATUS,
        }
    }

    /// Drive this registration.
    async fn create(
        self,
        emulator: &Emulator,
        name: &str,
        instance_arn: &str,
        extra: &[(&str, &str)],
    ) -> Answer {
        match self {
            Self::Redshift => {
                create_redshift_application(emulator, name, instance_arn, extra).await
            }
            Self::Qev2 => create_qev2_application(emulator, name, instance_arn, extra).await,
        }
    }
}

/// The names of the applications this registration's own read lists.
async fn registered_names(
    emulator: &Emulator,
    registration: Registration,
    case: &str,
) -> Vec<String> {
    let listed = emulator.call(registration.describe(), &[]).await;
    assert_eq!(
        listed.status,
        200,
        "case {case:?}: expected {} served, got {} with body {}",
        registration.describe(),
        listed.status,
        listed.body
    );
    crate::cluster_lifecycle::all_text(&listed.body, registration.name_member())
}

/// Put one more instance into the account's IAM Identity Center store, where that
/// service's own `CreateInstance` writes it.
fn hold_instance(emulator: &Emulator, instance_arn: &str) {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state as identity_center;

    let state = emulator
        .engine
        .state::<identity_center::ComAmazonawsSsoadminSwbexternalserviceState>();
    let instance = identity_center::Instance::active(
        instance_id(instance_arn),
        identity_center::SEED_IDENTITY_STORE_ID,
        identity_center::SEED_INSTANCE_TYPE,
        Some(identity_center::SEED_INSTANCE_NAME.to_string()),
        ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
        REGION,
    );
    state
        .lock()
        .instances
        .insert(instance.instance_arn.clone(), instance);
}

/// One row of the request-shape table: which registration drives it and what else the
/// create asks for beside the members every row carries.
struct Shape {
    registration: Registration,
    case: &'static str,
    extra: Vec<(&'static str, &'static str)>,
}

/// RULE: a registration naming an Identity Center instance the account does not hold is
/// refused with the relayed `sso:CreateApplication` denial and registers nothing.
#[tokio::test]
async fn a_registration_naming_an_instance_the_account_does_not_hold_is_denied() {
    let emulator = Emulator::new();
    let principal = caller_principal();

    // The other members a row carries, so the dependency can be seen not to depend on
    // them: nothing more, every optional member of the Redshift create, and a tagged
    // create.
    let shapes: Vec<Shape> = vec![
        Shape {
            registration: Registration::Redshift,
            case: "the required members alone",
            extra: vec![],
        },
        Shape {
            registration: Registration::Redshift,
            case: "a namespaced Lakehouse create with a token issuer",
            extra: vec![
                ("IdentityNamespace", "corp.directory_1"),
                ("ApplicationType", "Lakehouse"),
                (
                    "AuthorizedTokenIssuerList.member.1.TrustedTokenIssuerArn",
                    "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-1",
                ),
                ("SsoTagKeys.TagKey.1", "owner"),
            ],
        },
        Shape {
            registration: Registration::Redshift,
            case: "a tagged create",
            extra: vec![("Tags.Tag.1.Key", "owner"), ("Tags.Tag.1.Value", "alice")],
        },
        Shape {
            registration: Registration::Qev2,
            case: "the required members alone",
            extra: vec![],
        },
        Shape {
            registration: Registration::Qev2,
            case: "a tagged create",
            extra: vec![("Tags.Tag.1.Key", "env"), ("Tags.Tag.1.Value", "prod")],
        },
    ];

    for (index, instance_arn) in NOT_HELD.into_iter().enumerate() {
        for (position, shape) in shapes.iter().enumerate() {
            let registration = shape.registration;
            let case = format!("{registration:?} {} naming {instance_arn}", shape.case);
            let name = format!("denied-app-{index}-{position}");
            let before = registered_names(&emulator, registration, &case).await;

            let answer = registration
                .create(&emulator, &name, instance_arn, &shape.extra)
                .await;

            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (DENIED_STATUS, Some(DENIED_CODE)),
                "case {case:?}: expected {DENIED_STATUS} {DENIED_CODE}, got {} with body {}",
                answer.status,
                answer.body
            );
            let expected = expected_message(&principal, instance_arn);
            assert_eq!(
                answer.text("Message"),
                Some(expected.clone()),
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.text("Message")
            );
            // The refusal registered nothing: no application was minted, so this
            // registration's own read lists exactly what it listed before.
            let after = registered_names(&emulator, registration, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refusal to register nothing, so {before:?}, got \
                 {after:?}"
            );
            assert!(
                !after.contains(&name),
                "case {case:?}: expected {name:?} to be registered by nothing, got {after:?}"
            );
        }
    }
}

/// RULE (the other half): a registration naming an instance the account DOES hold is
/// served, and the managed application it reports is named inside that instance — read
/// back through the operation's own `Describe`.
#[tokio::test]
async fn a_registration_naming_an_instance_the_account_holds_is_served() {
    let emulator = Emulator::new();

    for (index, instance_arn) in IDENTITY_CENTER_INSTANCES.into_iter().enumerate() {
        for registration in [Registration::Redshift, Registration::Qev2] {
            let case = format!("{registration:?} naming the held instance {instance_arn}");
            let name = format!("held-app-{index}");
            let created = registration
                .create(&emulator, &name, instance_arn, &[])
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected 200, got {} with body {}",
                created.status, created.body
            );

            // The managed application is a resource of the instance the request named,
            // so the instance's own id is part of its ARN.
            let managed = created
                .text("IdcManagedApplicationArn")
                .unwrap_or_default();
            let within = format!("application/{}/", instance_id(instance_arn));
            assert!(
                managed.contains(&within),
                "case {case:?}: expected the managed application to be named inside the \
                 instance ({within:?}), got {managed:?}"
            );
            assert_eq!(
                created.text("IdcOnboardStatus").as_deref(),
                Some(registration.onboard_status()),
                "case {case:?}: expected the registered onboard status, got {:?}",
                created.text("IdcOnboardStatus")
            );

            // Round trip: the application the create registered is what a later read
            // reports.
            let listed = registered_names(&emulator, registration, &case).await;
            assert!(
                listed.contains(&name),
                "case {case:?}: expected the read to list {name:?}, got {listed:?}"
            );
        }
    }
}

/// RULE (what the decision is grounded in): the held set is the OWNING service's state,
/// so the very ARN a registration was refused for is served once the account's Identity
/// Center holds that instance — nothing about the request changed.
#[tokio::test]
async fn an_instance_the_account_comes_to_hold_is_served_afterwards() {
    let emulator = Emulator::new();
    let instance_arn = NOT_HELD[0];

    for registration in [Registration::Redshift, Registration::Qev2] {
        let case = format!("{registration:?} before the account holds {instance_arn}");
        let name = "late-instance-app";
        let refused = registration.create(&emulator, name, instance_arn, &[]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (DENIED_STATUS, Some(DENIED_CODE)),
            "case {case:?}: expected {DENIED_STATUS} {DENIED_CODE}, got {} with body {}",
            refused.status,
            refused.body
        );
    }

    hold_instance(&emulator, instance_arn);

    for registration in [Registration::Redshift, Registration::Qev2] {
        let case = format!("{registration:?} after the account holds {instance_arn}");
        let name = "late-instance-app";
        let served = registration.create(&emulator, name, instance_arn, &[]).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            served.status, served.body
        );
        let listed = registered_names(&emulator, registration, &case).await;
        assert!(
            listed.contains(&name.to_string()),
            "case {case:?}: expected the read to list {name:?}, got {listed:?}"
        );
    }
}
