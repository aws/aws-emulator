// SPDX-License-Identifier: CC0-1.0

//! WHO may create a custom platform version, and with WHOSE permissions the
//! platform definition archive is read, driven end to end over this service's
//! awsQuery surface with the IAM service mounted beside it on one shared engine.
//!
//! RULE 1 (the action is authorized): `CreatePlatformVersion` is decided against
//! the identity-based IAM policies in force on the principal the request's access
//! key id speaks for, as the action `elasticbeanstalk:CreatePlatformVersion` on
//! the platform version the request names, BEFORE anything is read or written —
//! so a caller no readable statement allows, or one an explicit `Deny` covers, is
//! refused `AccessDenied` with HTTP 403 and no platform version is recorded,
//! while a caller a matching `Allow` covers (and the account's own published
//! credential, which holds no policies) is served.
//!
//! RULE 2 (the archive read is the caller's): the archive a create names is
//! downloaded FOR the caller, so a caller whose policies do not allow reading
//! that object is refused exactly as object storage's own 403 refuses the read —
//! `InvalidParameterValue` naming the bucket, the key and the reason — and
//! nothing is recorded for it.
//!
//! The role, its policies and the session live in the IAM store of the SAME
//! shared engine — the records `iam:CreateRole`, `iam:PutRolePolicy` and
//! `sts:AssumeRole` write — so the documents under test are the ones that service
//! holds and the principal is the one the credential was minted for.
//!
//! The rows span what rule 1 generalises over: the document in force (none at
//! all, an allow of this service's other actions, an explicit deny beside an
//! allow of everything, an allow of exactly the action, an allow of the whole
//! service), and the principal (an assumed-role session and the account's own
//! published credential).

use std::collections::BTreeMap;

use crate::lifecycle::{account, Emulator, ACCOUNT_KEY_ID, TEST_REGION};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

/// The access key id the assumed-role session every row below signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, its session name, and the name of the
/// inline policy it carries.
const ROLE: &str = "eb-platform-author";
const SESSION: &str = "platform-session";
const POLICY_NAME: &str = "inline";

/// The bundle every create in this suite names.
const BUNDLE_BUCKET: &str = "platform-bundles";
const BUNDLE_KEY: &str = "platform.zip";

/// The platform version the creates under test ask for.
const PLATFORM_NAME: &str = "guarded-platform";
const PLATFORM_VERSION: &str = "1.0.0";

/// The ARN the account holds the role under — the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", account())
}

/// The principal ARN the seeded session speaks for, as `sts:AssumeRole` reports
/// it — what a refusal names.
fn session_arn() -> String {
    format!("arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}", account())
}

/// The ARN of the platform version the creates ask for, as the service builds it.
fn platform_arn() -> String {
    format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:platform/{PLATFORM_NAME}/{PLATFORM_VERSION}",
        account()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and the mint of a temporary credential record them.
fn given_session(emulator: &Emulator) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEEBROLE0001".to_string(),
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
            scope: iam::SessionPolicies::default(),
            tags: Vec::new(),
        },
        now,
    );
}

/// Put `document` on the role as its inline policy, replacing whatever it carried
/// — what `iam:PutRolePolicy` does; `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
fn given_role_policy(emulator: &Emulator, document: Option<&str>) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
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
    }
}

/// A policy document allowing `actions` on every resource.
fn allow(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":{actions},"Resource":"*"}}]}}"#
    )
}

/// A policy document allowing `actions` and the object read every create's
/// archive needs (rule 2), so what decides such a row is the Elastic Beanstalk
/// action alone.
fn allow_with_object_read(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":{actions},"Resource":"*"}},{{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}}]}}"#
    )
}

/// A policy document allowing every action of every service and denying
/// `actions`.
fn allow_everything_but(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},{{"Effect":"Deny","Action":{actions},"Resource":"*"}}]}}"#
    )
}

/// Call `CreatePlatformVersion` as `access_key_id`.
async fn create_as(emulator: &Emulator, access_key_id: &str) -> crate::lifecycle::Answer {
    emulator
        .call_as(
            access_key_id,
            TEST_REGION,
            "CreatePlatformVersion",
            &[
                ("PlatformName", PLATFORM_NAME),
                ("PlatformVersion", PLATFORM_VERSION),
                ("PlatformDefinitionBundle.S3Bucket", BUNDLE_BUCKET),
                ("PlatformDefinitionBundle.S3Key", BUNDLE_KEY),
            ],
        )
        .await
}

/// Whether the store holds the platform version the creates ask for: the
/// read-back that says whether a refused create recorded anything.
async fn version_exists(emulator: &Emulator) -> bool {
    let described = emulator
        .call(
            "DescribePlatformVersion",
            &[("PlatformArn", platform_arn().as_str())],
        )
        .await;
    described.status == 200
}

/// One row: the document in force on the role and whether the create is served.
struct Row {
    case: &'static str,
    document: Option<String>,
    served: bool,
}

fn rows() -> Vec<Row> {
    let action = "\"elasticbeanstalk:CreatePlatformVersion\"";
    vec![
        Row {
            case: "no policy at all",
            document: None,
            served: false,
        },
        Row {
            case: "an allow of this service's other actions",
            document: Some(allow_with_object_read(
                "[\"elasticbeanstalk:DescribePlatformVersion\",\"elasticbeanstalk:ListPlatformVersions\"]",
            )),
            served: false,
        },
        Row {
            case: "another service's actions only",
            document: Some(allow("\"s3:*\"")),
            served: false,
        },
        Row {
            case: "an explicit deny beside an allow of everything",
            document: Some(allow_everything_but(action)),
            served: false,
        },
        Row {
            case: "an allow of exactly the action",
            document: Some(allow_with_object_read(action)),
            served: true,
        },
        Row {
            case: "an allow of the whole service",
            document: Some(allow_with_object_read("\"elasticbeanstalk:*\"")),
            served: true,
        },
    ]
}

/// RULE 1, over the documents a role may carry.
#[tokio::test]
async fn creating_a_platform_version_is_authorized_against_the_callers_policies() {
    for row in rows() {
        let case = row.case;
        // One emulator per row, so a served create cannot make the next row's
        // create a duplicate instead of the authorization decision under test.
        let emulator = Emulator::with_identity_services();
        emulator.put_object(BUNDLE_BUCKET, BUNDLE_KEY, b"platform bundle").await;
        given_session(&emulator);
        given_role_policy(&emulator, row.document.as_deref());

        let answer = create_as(&emulator, SESSION_KEY_ID).await;
        if row.served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.expect("PlatformSummary/PlatformArn", case),
                platform_arn(),
                "case {case:?}: unexpected PlatformArn in {}",
                answer.body
            );
            assert!(
                version_exists(&emulator).await,
                "case {case:?}: a served create must have recorded the version"
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
        let message = answer.error_message();
        let expected_prefix = format!(
            "User: {} is not authorized to perform: elasticbeanstalk:CreatePlatformVersion \
             on resource: {} ",
            session_arn(),
            platform_arn()
        );
        assert!(
            message.starts_with(&expected_prefix),
            "case {case:?}: expected a refusal naming the principal, the action and the \
             resource ({expected_prefix:?}), got {message:?}"
        );
        assert!(
            !version_exists(&emulator).await,
            "case {case:?}: a refused create must record nothing"
        );
    }
}

/// RULE 1 for the caller that holds no IAM policies: the account's own published
/// credential, which is what every other suite signs with.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let case = "the account's own published credential";
    let emulator = Emulator::with_identity_services();
    emulator.put_object(BUNDLE_BUCKET, BUNDLE_KEY, b"platform bundle").await;
    given_session(&emulator);
    // A policy denying the action is in force on the ROLE, which this caller is
    // not: the decision is about the principal the credential speaks for.
    given_role_policy(
        &emulator,
        Some(&allow_everything_but(
            "\"elasticbeanstalk:CreatePlatformVersion\"",
        )),
    );

    let answer = create_as(&emulator, ACCOUNT_KEY_ID).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the create to be served, got {} and body {}",
        answer.status, answer.body
    );
    assert!(
        version_exists(&emulator).await,
        "case {case:?}: the served create must have recorded the version"
    );
}

/// RULE 2: the archive read is the caller's, so a caller that may create the
/// version but may not read the bundle is refused the download.
#[tokio::test]
async fn the_archive_read_is_authorized_as_the_caller() {
    let case = "a caller that may create but may not read the bundle";
    let emulator = Emulator::with_identity_services();
    emulator
        .put_object(BUNDLE_BUCKET, BUNDLE_KEY, b"platform bundle")
        .await;
    given_session(&emulator);
    given_role_policy(&emulator, Some(&allow("\"elasticbeanstalk:*\"")));

    let answer = create_as(&emulator, SESSION_KEY_ID).await;
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
    // AWS's own sentence for an archive read it would not permit, spelled out
    // here rather than taken from the service's own template: the bucket and the
    // key are the request's, and the rest is fixed text.
    let expected = format!(
        "Unable to download from S3 location (Bucket: {BUNDLE_BUCKET}  Key: {BUNDLE_KEY}). \
         Reason: Forbidden: S3Bucket={BUNDLE_BUCKET}, S3Key={BUNDLE_KEY}"
    );
    assert_eq!(
        answer.error_message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.error_message()
    );
    assert!(
        !version_exists(&emulator).await,
        "case {case:?}: a refused create must record nothing"
    );

    // The same caller, once its policies allow reading that object, is served —
    // so the refusal above was the archive read's permission and nothing else.
    given_role_policy(
        &emulator,
        Some(&allow("[\"elasticbeanstalk:*\",\"s3:GetObject\"]")),
    );
    let served = create_as(&emulator, SESSION_KEY_ID).await;
    assert_eq!(
        served.status, 200,
        "case \"the same caller granted the object read\": expected 200, got {} and body {}",
        served.status, served.body
    );
    assert!(
        version_exists(&emulator).await,
        "case \"the same caller granted the object read\": the version must be recorded"
    );
}
