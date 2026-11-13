// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `CreatePartnerApp` and the reader that makes its effect
//! observable, `DescribePartnerApp`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! shared harness in `tests/all/inference_deployment_lifecycle.rs`, so what these tests
//! exercise is what a real SDK client gets.
//!
//! The RULES asserted here, each over a TABLE of inputs rather than one recorded call:
//!
//! 1. A partner AI app can only be provisioned for an app type the account holds a
//!    partner-application subscription for; a type it holds none for is refused 400
//!    `ValidationException` and nothing is persisted.
//! 2. For a subscribed type the create mints the app's identity — an
//!    `…:partner-app/app-[A-Z0-9]{12}` ARN in the Region the caller signed for — stores
//!    every member the request carried, and `DescribePartnerApp` reports it back.
//! 3. `Name` is the one identity the caller chooses, so a second create under a name the
//!    account already holds is the modelled `ConflictException`; a repeated `ClientToken`
//!    answers the app that token already minted.
//! 4. A created app is `Creating` until the instant the clock owes its move to
//!    `Available`, and only an `Available` app has a presigned URL.
//! 5. An ARN this account holds no app for is the modelled `ResourceNotFound`.
//!
//! The account's AWS Marketplace subscriptions are seeded into the ONE shared store the
//! handler reads (`engine.state::<…>()`): subscribing to a partner application is a
//! purchase in AWS Marketplace, so no operation this service models grants one, and a case
//! that needs one grants it in the store rather than through a second copy of the state.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state as sm;

use crate::inference_deployment_lifecycle::{
    epoch_seconds, error_code, f64_field, message, str_field, Emulator, ACCOUNT,
};

/// Every member of `PartnerAppType`, from the model
/// (`smithy_query(query="shape",shape="PartnerAppType")`).
pub(crate) const PARTNER_APP_TYPES: [&str; 4] = [
    "lakera-guard",
    "comet",
    "deepchecks-llm-evaluation",
    "fiddler",
];

/// The members `DescribePartnerApp` reports exactly as `CreatePartnerApp` was given them
/// (`smithy_query(query="operation",operation="CreatePartnerApp,DescribePartnerApp")`:
/// both shapes carry these under the same names and the same target shapes). `IdcConfig`
/// is left out because the output shape is `IdcConfigOutput` rather than the input's
/// `IdcConfigInput`, and is asserted on its own below.
const ECHOED_MEMBERS: [&str; 10] = [
    "Name",
    "Type",
    "ExecutionRoleArn",
    "Tier",
    "AuthType",
    "KmsKeyId",
    "MaintenanceConfig",
    "ApplicationConfig",
    "EnableIamSessionBasedIdentity",
    "EnableAutoMinorVersionUpgrade",
];

/// An `IDC` app's Identity Center instance: `InstanceArn` is modelled
/// `^arn:aws[a-z\-]*:sso:::instance/(sso)?ins-[a-zA-Z0-9.-]{16}$`
/// (`smithy_query(query="shape",shape="IdcConfigInput")`), so the id carries exactly 16
/// characters after `ins-`.
const IDC_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// Grant `types` to the account, as an AWS Marketplace subscription does. See this
/// module's header: no operation of this service grants one.
pub(crate) fn subscribe(emulator: &Emulator, types: &[&str]) {
    let state = sm::service_state(&*emulator.engine).expect("the store is on the shared engine");
    let mut store = state.lock();
    for app_type in types {
        store
            .partner_app_subscriptions
            .insert((*app_type).to_string());
    }
}

/// A `CreatePartnerApp` request body: the four required members, plus whatever `extras`
/// carries.
pub(crate) fn create_body(name: &str, app_type: &str, extras: &Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Type": app_type,
        // `RoleArn` is modelled `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/partner-app-execution"),
        // `Tier` is a free-form `NonEmptyString64`, not an enum, so a row varies the
        // string rather than picking from a catalog.
        "Tier": "small",
        "AuthType": "IAM",
    });
    for (member, value) in extras.as_object().expect("extras is an object") {
        body[member] = value.clone();
    }
    body
}

/// The `Summaries` a `ListPartnerApps` response carries (an absent member is an empty
/// page), as the ARNs they report.
async fn listed_arns(emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("ListPartnerApps", &json!({})).await;
    body.get("Summaries")
        .and_then(Value::as_array)
        .map(|page| {
            page.iter()
                .map(|summary| str_field(summary, "Arn"))
                .collect()
        })
        .unwrap_or_default()
}

/// The `app-...` id an app's ARN carries.
pub(crate) fn app_id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or_default()
}

/// Assert `arn` is the ARN SageMaker mints for a partner app in `region`: this account's,
/// in that Region, with an `app-` id of exactly twelve characters from `[A-Z0-9]` — the
/// modelled `PartnerAppArn` pattern
/// (`smithy_query(query="operation",operation="CreatePartnerApp")`).
fn assert_partner_app_arn(case: &str, arn: &str, region: &str) {
    let prefix = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:partner-app/app-");
    assert!(
        arn.starts_with(&prefix),
        "case {case}: the ARN must be {prefix}…, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    assert_eq!(
        id.len(),
        12,
        "case {case}: the app id must carry 12 characters after `app-`, got {arn:?}"
    );
    assert!(
        id.chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()),
        "case {case}: the app id must be upper-case alphanumeric, got {arn:?}"
    );
    assert!(
        arn.len() <= 128,
        "case {case}: `PartnerAppArn` is modelled len 1..128, got {} in {arn:?}",
        arn.len()
    );
}

/// RULE: for an app type the account holds a subscription for, the create mints the app's
/// identity in the Region the caller signed for, stores every member the request carried,
/// and the reader reports it back — whatever the type, the authorization type and the
/// optional members.
#[tokio::test]
async fn a_create_for_a_subscribed_type_round_trips_through_describe() {
    // Rows differ along the axes the rule generalises over: the app type, the Region the
    // caller signs for, the authorization type, and which optional members the request
    // carries (none, the encryption and maintenance ones, the Identity Center one, the
    // application configuration).
    let extras = [
        ("required members only", json!({})),
        (
            "key, window and upgrade flags",
            json!({
                // `KmsKeyId` is modelled `^[a-zA-Z0-9:/_-]*$`.
                "KmsKeyId": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/partner-app-key"),
                // `WeeklyScheduleTimeFormat` is
                // `^(Mon|Tue|Wed|Thu|Fri|Sat|Sun):([01]\d|2[0-3]):([0-5]\d)$`
                // (smithy_query shape=PartnerAppMaintenanceConfig).
                "MaintenanceConfig": {"MaintenanceWindowStart": "Tue:03:30"},
                "EnableAutoMinorVersionUpgrade": true,
                "EnableIamSessionBasedIdentity": true,
                "Tier": "medium",
            }),
        ),
        (
            "Identity Center authorization",
            json!({
                "AuthType": "IDC",
                "IdcConfig": {"InstanceArn": IDC_INSTANCE_ARN},
                "Tier": "large",
            }),
        ),
        (
            "application configuration and tags",
            json!({
                "ApplicationConfig": {
                    "AdminUsers": ["admin@example.com"],
                    "Arguments": {"ingest": "enabled"},
                    "AssignedGroupPatterns": ["ml-*"],
                },
                "Tags": [{"Key": "team", "Value": "ml"}],
            }),
        ),
    ];

    for (index, app_type) in PARTNER_APP_TYPES.iter().enumerate() {
        for region in ["us-east-1", "eu-west-1"] {
            // Every row sees a different optional-member set, so the four sets are each
            // exercised against more than one type and more than one Region.
            let (label, carried) = &extras[index % extras.len()];
            let case = format!("type={app_type} region={region} extras={label}");
            let emulator = Emulator::new();
            subscribe(&emulator, &[app_type]);

            let body = create_body("MyPartnerApp1", app_type, carried);
            let (status, created) = emulator.call_in(region, "CreatePartnerApp", &body).await;
            assert_eq!(
                status, 200,
                "case {case}: CreatePartnerApp must answer 200, got {status} with {created}"
            );
            let arn = str_field(&created, "Arn");
            assert_partner_app_arn(&case, &arn, region);

            let (status, described) = emulator
                .call_in(region, "DescribePartnerApp", &json!({"Arn": arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribePartnerApp must answer 200, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "Arn"),
                arn,
                "case {case}: the reader must report the created ARN, got {described}"
            );
            for member in ECHOED_MEMBERS {
                if let Some(sent) = body.get(member) {
                    assert_eq!(
                        described.get(member),
                        Some(sent),
                        "case {case}: {member} must read back as created, got {described}"
                    );
                }
            }
            if body.get("IdcConfig").is_some() {
                let reported = described
                    .get("IdcConfig")
                    .unwrap_or_else(|| panic!("case {case}: expected an IdcConfig in {described}"));
                assert_eq!(
                    str_field(reported, "InstanceArn"),
                    IDC_INSTANCE_ARN,
                    "case {case}: the Identity Center instance must be the one created \
                     with, got {described}"
                );
            } else {
                assert!(
                    described.get("IdcConfig").is_none(),
                    "case {case}: an app created with no IdcConfig has none, got {described}"
                );
            }
            // A freshly created app is being provisioned, and is reported at the instant
            // it was created.
            assert_eq!(
                str_field(&described, "Status"),
                sm::PARTNER_APP_STATUS_CREATING,
                "case {case}: a created app is {} first, got {described}",
                sm::PARTNER_APP_STATUS_CREATING
            );
            let created_at = f64_field(&described, "CreationTime");
            assert!(
                (created_at - f64_field(&described, "LastModifiedTime")).abs() < f64::EPSILON,
                "case {case}: nothing has modified the app since it was created, got \
                 {described}"
            );
            // Where the app is served, which the presigned URL is built on.
            assert_eq!(
                str_field(&described, "BaseUrl"),
                sm::partner_app_base_url(region, app_id_of(&arn)),
                "case {case}: BaseUrl must address the app itself, got {described}"
            );
            // And the one store the reader read is the one the listing pages.
            assert_eq!(
                listed_arns(&emulator).await,
                vec![arn.clone()],
                "case {case}: the created app must be the listing's one entry"
            );
        }
    }
}

/// RULE: an app type the account holds no partner-application subscription for cannot be
/// provisioned: the create is refused 400 `ValidationException` naming the missing AWS
/// Marketplace subscription, nothing is persisted, and the SAME request succeeds once the
/// subscription is held — so the subscription is what the refusal turns on.
#[tokio::test]
async fn an_unsubscribed_type_is_refused_and_persists_nothing() {
    for app_type in PARTNER_APP_TYPES {
        // Rows: an account subscribed to nothing, and one subscribed to every OTHER type —
        // so the gate is per type rather than "holds some subscription".
        let others: Vec<&str> = PARTNER_APP_TYPES
            .iter()
            .copied()
            .filter(|held| *held != app_type)
            .collect();
        for (label, held) in [
            ("no subscription at all", Vec::new()),
            ("subscribed to the other types only", others),
        ] {
            let case = format!("type={app_type} held={label}");
            let emulator = Emulator::new();
            subscribe(&emulator, &held);

            let body = create_body("MyPartnerApp1", app_type, &json!({}));
            // Twice: a refusal is not a step towards a create, so the second call answers
            // exactly what the first did.
            for attempt in ["first attempt", "second attempt"] {
                let (status, refused) = emulator.call("CreatePartnerApp", &body).await;
                assert_eq!(
                    (status, error_code(&refused).as_str()),
                    (400, "ValidationException"),
                    "case {case}/{attempt}: expected 400 ValidationException, got {status} \
                     with {refused}"
                );
                assert_eq!(
                    message(&refused),
                    sm::PARTNER_APP_NOT_SUBSCRIBED,
                    "case {case}/{attempt}: the refusal must state the missing AWS \
                     Marketplace entitlement in the service's own fixed wording, got \
                     {refused}"
                );
                assert!(
                    refused.get("Arn").is_none(),
                    "case {case}/{attempt}: a refused create mints no ARN, got {refused}"
                );
                assert!(
                    listed_arns(&emulator).await.is_empty(),
                    "case {case}/{attempt}: a refused create persists nothing"
                );
            }

            // The subscription is the only thing that was missing.
            subscribe(&emulator, &[app_type]);
            let (status, created) = emulator.call("CreatePartnerApp", &body).await;
            assert_eq!(
                status, 200,
                "case {case}: the same request must succeed once the subscription is held, \
                 got {status} with {created}"
            );
            assert_partner_app_arn(&case, &str_field(&created, "Arn"), "us-east-1");
        }
    }
}

/// RULE: `Name` is the one identity a caller chooses for an app, so a second create under
/// a name this account already holds an app under is refused with the modelled
/// `ConflictException` and the store is left as it was — while another name is created.
#[tokio::test]
async fn a_name_this_account_already_holds_is_refused_as_a_conflict() {
    let emulator = Emulator::new();
    subscribe(&emulator, &PARTNER_APP_TYPES);
    let first = emulator
        .ok(
            "CreatePartnerApp",
            &create_body("MyPartnerApp1", "comet", &json!({})),
        )
        .await;
    let held = str_field(&first, "Arn");

    // Rows differ in what ELSE the second create changes: nothing, the type, the Region it
    // signs for, and the tier — the name is what the conflict turns on. The Region row is
    // deliberate: the store this service keeps, and the listing that pages it, are
    // account-wide rather than per Region, so a name held anywhere in the account is held
    // (see `state::partner_app_named`).
    let cases = [
        ("the same request again", "us-east-1", json!({})),
        ("another type", "us-east-1", json!({"Type": "fiddler"})),
        ("another Region", "eu-west-1", json!({})),
        ("another tier", "us-east-1", json!({"Tier": "large"})),
    ];
    for (case, region, extras) in cases {
        let body = create_body("MyPartnerApp1", "comet", &extras);
        let (status, refused) = emulator.call_in(region, "CreatePartnerApp", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ConflictException"),
            "case {case}: expected 400 ConflictException, got {status} with {refused}"
        );
        assert!(
            message(&refused).contains(&held) && message(&refused).contains("MyPartnerApp1"),
            "case {case}: the conflict must name the app already holding the name, got \
             {refused}"
        );
        assert_eq!(
            listed_arns(&emulator).await,
            vec![held.clone()],
            "case {case}: a refused create leaves the store as it was"
        );
    }

    // Another name is another app.
    let second = emulator
        .ok(
            "CreatePartnerApp",
            &create_body("MyPartnerApp2", "comet", &json!({})),
        )
        .await;
    let other = str_field(&second, "Arn");
    assert_ne!(
        other, held,
        "case another name: a second app has its own ARN, got {second}"
    );
    let mut listed = listed_arns(&emulator).await;
    listed.sort();
    let mut expected = vec![held, other];
    expected.sort();
    assert_eq!(
        listed, expected,
        "case another name: both apps must be in the store"
    );
}

/// RULE: `ClientToken` makes the call idempotent, so a create presenting a token an app
/// was already minted under answers that app's ARN and mints nothing — while a fresh token
/// mints a new app.
#[tokio::test]
async fn a_repeated_client_token_answers_the_app_it_already_minted() {
    let emulator = Emulator::new();
    subscribe(&emulator, &PARTNER_APP_TYPES);
    // `ClientToken` is modelled `len 1..36, pattern ^[a-zA-Z0-9-]+$`.
    let token = "11111111-2222-3333-4444-555555555555";
    let first = emulator
        .ok(
            "CreatePartnerApp",
            &create_body("MyPartnerApp1", "comet", &json!({"ClientToken": token})),
        )
        .await;
    let minted = str_field(&first, "Arn");

    // Rows differ in how much of the retry's body differs from the original: not at all,
    // then in the name, the type and the tier — the token is what the answer turns on.
    let retries = [
        ("the same request again", json!({"ClientToken": token})),
        (
            "another name",
            json!({"ClientToken": token, "Name": "MyPartnerApp2"}),
        ),
        (
            "another type",
            json!({"ClientToken": token, "Type": "fiddler"}),
        ),
        (
            "another tier",
            json!({"ClientToken": token, "Tier": "large"}),
        ),
    ];
    for (case, extras) in retries {
        let body = create_body("MyPartnerApp1", "comet", &extras);
        let retried = emulator.ok("CreatePartnerApp", &body).await;
        assert_eq!(
            str_field(&retried, "Arn"),
            minted,
            "case {case}: a repeated ClientToken must answer the app it minted, got \
             {retried}"
        );
        assert_eq!(
            listed_arns(&emulator).await,
            vec![minted.clone()],
            "case {case}: a repeated ClientToken mints no second app"
        );
    }

    // A fresh token under another name is another app.
    let fresh = emulator
        .ok(
            "CreatePartnerApp",
            &create_body(
                "MyPartnerApp2",
                "comet",
                &json!({"ClientToken": "66666666-7777-8888-9999-000000000000"}),
            ),
        )
        .await;
    assert_ne!(
        str_field(&fresh, "Arn"),
        minted,
        "case a fresh token: a new app has its own ARN, got {fresh}"
    );
    assert_eq!(
        listed_arns(&emulator).await.len(),
        2,
        "case a fresh token: both apps must be in the store"
    );
}

/// RULE: a created app is `Creating` — and serves nothing — until the instant the clock
/// owes its move to `Available`, whatever the number of reads; from then on it is
/// `Available` and has a presigned URL, and its `CreationTime` never moves.
#[tokio::test]
async fn a_created_app_becomes_available_only_once_the_clock_owes_it() {
    for app_type in PARTNER_APP_TYPES {
        let case = format!("type={app_type}");
        let emulator = Emulator::new();
        subscribe(&emulator, &[app_type]);
        let created = emulator
            .ok(
                "CreatePartnerApp",
                &create_body("MyPartnerApp1", app_type, &json!({})),
            )
            .await;
        let arn = str_field(&created, "Arn");

        // Before the due instant: two reads, neither of which moves the status, and no URL
        // for an app that is not reachable yet.
        let mut creation_times = Vec::new();
        for read in ["first read", "second read"] {
            let described = emulator
                .ok("DescribePartnerApp", &json!({"Arn": arn}))
                .await;
            assert_eq!(
                str_field(&described, "Status"),
                sm::PARTNER_APP_STATUS_CREATING,
                "case {case}/{read}: expected {} before the transition is owed, got \
                 {described}",
                sm::PARTNER_APP_STATUS_CREATING
            );
            creation_times.push(f64_field(&described, "CreationTime"));
        }
        let (status, refused) = emulator
            .call("CreatePartnerAppPresignedUrl", &json!({"Arn": arn}))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case}: an app that is still being created serves no URL, got {status} \
             with {refused}"
        );

        // Past it: the transition the clock owed is materialised, and stays.
        tokio::time::sleep(sm::PARTNER_APP_AVAILABLE_AFTER + Duration::from_millis(300)).await;
        for read in ["first read after due", "second read after due"] {
            let described = emulator
                .ok("DescribePartnerApp", &json!({"Arn": arn}))
                .await;
            assert_eq!(
                str_field(&described, "Status"),
                sm::PARTNER_APP_STATUS_AVAILABLE,
                "case {case}/{read}: expected {} once the transition is owed, got \
                 {described}",
                sm::PARTNER_APP_STATUS_AVAILABLE
            );
            creation_times.push(f64_field(&described, "CreationTime"));
            // `LastModifiedTime` is the instant the transition was DUE, not the instant it
            // was read, so it does not move with the reads either.
            let modified = f64_field(&described, "LastModifiedTime");
            let due = creation_times[0] + sm::PARTNER_APP_AVAILABLE_AFTER.as_secs_f64();
            assert!(
                (modified - due).abs() < 0.05,
                "case {case}/{read}: LastModifiedTime must be the instant the transition \
                 was due ({due}), got {modified}"
            );
        }
        assert!(
            creation_times
                .iter()
                .all(|reported| (reported - creation_times[0]).abs() < f64::EPSILON),
            "case {case}: CreationTime must not move across reads, got {creation_times:?}"
        );

        let minted = emulator
            .ok("CreatePartnerAppPresignedUrl", &json!({"Arn": arn}))
            .await;
        let url = str_field(&minted, "Url");
        // The URL leads back to THIS emulator, at the session address it serves for the
        // app (its own suite is `tests/all/presigned_session.rs`): the app's `BaseUrl` is
        // what the READER reports, as AWS reports it, and no client of this emulator
        // resolves that host.
        let expected = format!(
            "{}/_aws/sagemaker/partner-app/{}/auth?authToken=",
            smithy_server_runtime::UNSERVED_URL_BASE,
            app_id_of(&arn)
        );
        assert!(
            url.starts_with(&expected),
            "case {case}: the URL must address the app's session on this emulator, got \
             {url:?}"
        );
    }
}

/// RULE: an ARN this account holds no app for is the reader's modelled `ResourceNotFound`,
/// naming the ARN the request sent.
#[tokio::test]
async fn an_arn_this_account_holds_no_app_for_is_not_found() {
    let emulator = Emulator::new();
    subscribe(&emulator, &PARTNER_APP_TYPES);
    // One app exists, so the refusals below are about the ARN rather than about an empty
    // store.
    let created = emulator
        .ok(
            "CreatePartnerApp",
            &create_body("MyPartnerApp1", "comet", &json!({})),
        )
        .await;
    let held = str_field(&created, "Arn");

    // Rows: an id no app holds, that same id in another Region, that id under another
    // account, and the held app's id under another account — each a well-formed
    // `PartnerAppArn` naming no app of this account.
    let absent = [
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:partner-app/app-ABCDEFGH1234"),
        format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:partner-app/app-ABCDEFGH1234"),
        "arn:aws:sagemaker:us-east-1:111122223333:partner-app/app-ABCDEFGH1234".to_string(),
        held.replace(ACCOUNT, "111122223333"),
    ];
    for arn in absent {
        let (status, refused) = emulator
            .call("DescribePartnerApp", &json!({"Arn": arn}))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {arn}: expected 400 ResourceNotFound, got {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            // The sentence spelled out, not read from the constant under test: the wire
            // form is the requirement.
            "App entity does not exist.",
            "case {arn}: the refusal must be the service's fixed sentence, naming no \
             identifier, got {refused}"
        );
    }

    // The app this account does hold still reads back.
    let described = emulator
        .ok("DescribePartnerApp", &json!({"Arn": held}))
        .await;
    assert_eq!(
        str_field(&described, "Arn"),
        held,
        "case the held app: it must still read back, got {described}"
    );
}

/// One row of the tag table: what the create carried as `Tags`, and the (key, value)
/// pairs the app must then hold, sorted. A named row rather than a tuple, so the table's
/// type stays readable.
struct TagCase {
    label: &'static str,
    extras: Value,
    expected: &'static [(&'static str, &'static str)],
}

/// RULE: the `Tags` a create carried are the app's resource tags, so the tag operations
/// read and write exactly them.
#[tokio::test]
async fn the_tags_a_create_carried_are_the_apps_resource_tags() {
    // Rows differ in how many tags the create carried: none, one, several.
    let cases = [
        TagCase {
            label: "no Tags member",
            extras: json!({}),
            expected: &[],
        },
        TagCase {
            label: "one tag",
            extras: json!({"Tags": [{"Key": "team", "Value": "ml"}]}),
            expected: &[("team", "ml")],
        },
        TagCase {
            label: "three tags",
            extras: json!({"Tags": [
                {"Key": "team", "Value": "ml"},
                {"Key": "env", "Value": "test"},
                {"Key": "cost-centre", "Value": "4711"},
            ]}),
            expected: &[("cost-centre", "4711"), ("env", "test"), ("team", "ml")],
        },
    ];
    for TagCase {
        label: case,
        extras,
        expected,
    } in cases
    {
        let emulator = Emulator::new();
        subscribe(&emulator, &["comet"]);
        let created = emulator
            .ok(
                "CreatePartnerApp",
                &create_body("MyPartnerApp1", "comet", &extras),
            )
            .await;
        let arn = str_field(&created, "Arn");

        let listed = emulator.ok("ListTags", &json!({"ResourceArn": arn})).await;
        let mut reported: Vec<(String, String)> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| (str_field(tag, "Key"), str_field(tag, "Value")))
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case}: ListTags must report the tags the create carried, got {listed}"
        );

        // And a tag added afterwards joins that same set.
        emulator
            .ok(
                "AddTags",
                &json!({"ResourceArn": arn, "Tags": [{"Key": "owner", "Value": "sm"}]}),
            )
            .await;
        let listed = emulator.ok("ListTags", &json!({"ResourceArn": arn})).await;
        let keys: Vec<String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .map(|tags| tags.iter().map(|tag| str_field(tag, "Key")).collect())
            .unwrap_or_default();
        assert_eq!(
            keys.len(),
            expected.len() + 1,
            "case {case}: an added tag joins the app's set, got {listed}"
        );
        assert!(
            keys.contains(&"owner".to_string()),
            "case {case}: the added tag must be reported, got {listed}"
        );
    }
}

/// RULE: the store that holds the account's apps is bounded, so a create past the bound is
/// the modelled `ResourceLimitExceeded` — while a create that could not add a record anyway
/// (its name is already held, or its type is unsubscribed) is refused for THAT, because
/// both are judged before the bound.
#[tokio::test]
async fn a_store_at_its_bound_refuses_a_further_create() {
    let emulator = Emulator::new();
    subscribe(&emulator, &["comet"]);
    // The bound is far above what a wire test can create one call at a time, so the store
    // is filled through the one map the handler reads — the same seam `partner_apps.rs`
    // seeds through, with its record builder rather than a second one.
    {
        let state =
            sm::service_state(&*emulator.engine).expect("the store is on the shared engine");
        let mut store = state.lock();
        for index in 0..sm::PARTNER_APP_QUOTA {
            let app = crate::partner_apps::stored_app(
                index,
                &format!("Held{index}"),
                "comet",
                sm::PARTNER_APP_STATUS_AVAILABLE,
                Duration::from_secs(60),
                None,
            );
            store.partner_apps.insert(app.app_id.clone(), app);
        }
    }

    let (status, refused) = emulator
        .call(
            "CreatePartnerApp",
            &create_body("MyPartnerApp1", "comet", &json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceLimitExceeded"),
        "case a full store: expected 400 ResourceLimitExceeded, got {status} with {refused}"
    );
    let reported = message(&refused);
    assert!(
        reported.contains(&sm::PARTNER_APP_QUOTA.to_string()) && reported.contains("limit"),
        "case a full store: the refusal must name the limit it is past, got {refused}"
    );

    // A name the account already holds is the conflict, whatever the bound says.
    let (status, refused) = emulator
        .call(
            "CreatePartnerApp",
            &create_body("Held0", "comet", &json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ConflictException"),
        "case a held name on a full store: expected 400 ConflictException, got {status} \
         with {refused}"
    );

    // And an unsubscribed type is refused for the subscription it lacks.
    let (status, refused) = emulator
        .call(
            "CreatePartnerApp",
            &create_body("MyPartnerApp2", "fiddler", &json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case an unsubscribed type on a full store: expected 400 ValidationException, got \
         {status} with {refused}"
    );
    assert_eq!(
        message(&refused),
        sm::PARTNER_APP_NOT_SUBSCRIBED,
        "case an unsubscribed type on a full store: the refusal must be the subscription \
         one, got {refused}"
    );
}

/// RULE: a created app's `CreationTime` is the instant it was created, which the listing
/// and the reader report as the same value — a resource has ONE creation instant.
#[tokio::test]
async fn the_creation_time_the_reader_and_the_listing_report_is_one_instant() {
    let emulator = Emulator::new();
    subscribe(&emulator, &PARTNER_APP_TYPES);
    let before = epoch_seconds(std::time::SystemTime::now());
    let created = emulator
        .ok(
            "CreatePartnerApp",
            &create_body("MyPartnerApp1", "lakera-guard", &json!({})),
        )
        .await;
    let after = epoch_seconds(std::time::SystemTime::now());
    let arn = str_field(&created, "Arn");

    let described = emulator
        .ok("DescribePartnerApp", &json!({"Arn": arn}))
        .await;
    let reported = f64_field(&described, "CreationTime");
    assert!(
        reported >= before && reported <= after,
        "case the created app: CreationTime must be the instant of the create (between \
         {before} and {after}), got {reported}"
    );

    let page = emulator.ok("ListPartnerApps", &json!({})).await;
    let summary = page
        .get("Summaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .unwrap_or_else(|| panic!("expected the created app in {page}"))
        .clone();
    assert!(
        (f64_field(&summary, "CreationTime") - reported).abs() < f64::EPSILON,
        "case the created app: the listing and the reader must report one instant, got \
         {summary} against {reported}"
    );
}
