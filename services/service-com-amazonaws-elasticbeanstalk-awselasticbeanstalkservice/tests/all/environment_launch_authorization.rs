// SPDX-License-Identifier: CC0-1.0

//! WHOSE permission an environment LAUNCH needs, and what an environment nobody
//! may launch reports — driven end to end over this service's awsQuery surface
//! with the IAM service mounted beside it on one shared engine.
//!
//! RULE: an environment's Amazon EC2 instances are given the IAM instance
//! profile its configuration names (`aws:autoscaling:launchconfiguration` /
//! `IamInstanceProfile`), or that option's published default when it names
//! none, and handing a role to a resource is the CALLER's act — IAM admits it
//! only to a caller allowed `iam:PassRole` on that role. So a launch IAM will
//! not let the caller pass the profile for cannot be provisioned: the call is
//! still accepted and the environment still starts `Launching`, and the refusal
//! is reported where a client reads a launch's progress — an `ERROR` event
//! carrying IAM's own refusal, the service's `Failed to launch environment.`,
//! and the terminal `Terminated` status rather than `Ready`.
//!
//! The rows span what the rule generalises over: the document in force on the
//! caller (an allow of this service's actions alone, an explicit deny of the
//! pass beside an allow of everything, an allow of the pass on every role, an
//! allow of the pass on exactly the role the launch passes, an allow of the pass
//! on another role), the instance profile the configuration
//! names (none, so the published default, and one of its own), the principal
//! (an assumed-role session and the account's own published credential), and
//! the operation that launches — `CreateEnvironment`, from the request's own
//! configuration, and `ComposeEnvironments`, from an environment manifest's.
//!
//! Every row's caller is one IAM lets launch at all: whether the caller may make
//! the CALL is decided before any of this, and a caller no policy allows is
//! refused it outright (`request_authorization.rs`). So each document below
//! allows this service's actions, and what the row varies is the PASS.
//!
//! The role, its policies and the session live in the IAM store of the SAME
//! shared engine — the records `iam:CreateRole`, `iam:PutRolePolicy` and
//! `sts:AssumeRole` write — seeded the way
//! `create_platform_version_authorization.rs` seeds them. This suite holds its
//! OWN principal rather than sharing that one, so what decides a row here is
//! the document this suite put on its own role and nothing else.

use std::collections::BTreeMap;

use crate::application_version_processing::zip_of;
use crate::harness::{wait_for_status, wait_for_version_status, Answer, Emulator, SOLUTION_STACK};
use crate::lifecycle::{account, ACCOUNT_KEY_ID, TEST_REGION};
use crate::resource_tags::xml_unescaped;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

/// The access key id the assumed-role session every row below signs its launch
/// with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLELAUNCH001";

/// The role that session is assumed from, its session name, and the name of the
/// inline policy it carries.
const ROLE: &str = "eb-environment-launcher";
const SESSION: &str = "launch-session";
const POLICY_NAME: &str = "inline";

/// The application every launch below is made under.
const APPLICATION: &str = "launch-guarded-app";

/// The environment `CreateEnvironment` launches, and the one the manifest a
/// composition reads names.
const ENVIRONMENT: &str = "launch-guarded-env";
/// See [`ENVIRONMENT`].
const COMPOSED_ENVIRONMENT: &str = "composed-guarded-env";

/// The version label a composition names, and where its source bundle is
/// uploaded.
const VERSION_LABEL: &str = "v1";
const BUNDLE_BUCKET: &str = "launch-guarded-bundles";
const BUNDLE_KEY: &str = "bundles/bundle.zip";

/// The configuration-option namespace that carries an environment's launch
/// configuration and the option in it that names the IAM instance profile its
/// instances are given, as Elastic Beanstalk's published
/// configuration-option reference spells them ("`aws:autoscaling:launchconfiguration`
/// / `IamInstanceProfile`: instance profile that gives the environment's
/// instances the permissions they need"). Written out here rather than taken
/// from the service, which keeps them internal to its state module.
const LAUNCH_CONFIGURATION: &str = "aws:autoscaling:launchconfiguration";
/// See [`LAUNCH_CONFIGURATION`].
const IAM_INSTANCE_PROFILE: &str = "IamInstanceProfile";

/// An instance profile a configuration names of its own, and a DIFFERENT one,
/// so a grant scoped to exactly one role can be told from a grant covering the
/// role a launch really passes.
const NAMED_PROFILE: &str = "eb-ec2-role";
/// See [`NAMED_PROFILE`].
const OTHER_PROFILE: &str = "eb-other-role";

/// The `EnvironmentStatus` values the launches under test start in and settle
/// in, taken from the service's own constants so an expectation cannot drift
/// from what it reports.
const LAUNCHING: &str = eb::state::STATUS_LAUNCHING;
/// See [`LAUNCHING`].
const READY: &str = eb::state::STATUS_READY;
/// See [`LAUNCHING`].
const TERMINATED: &str = eb::state::STATUS_TERMINATED;

/// The severity a failure is reported at, and the one every other event of a
/// launch carries.
const ERROR: &str = eb::state::SEVERITY_ERROR;

/// The ARN the account holds the role under — the identity whose policies an
/// assumed-role session's launch is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", account())
}

/// The principal ARN the seeded session speaks for, as `sts:AssumeRole` reports
/// it — what a refusal names.
fn session_arn() -> String {
    format!("arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}", account())
}

/// The ARN of the IAM role a launch passes for the instance profile `profile`:
/// the service's own resolution of it, so the resource this suite expects the
/// decision to be taken on is the one it IS taken on.
fn passed_role_arn(profile: &str) -> String {
    eb::state::instance_profile_role_arn(profile)
}

/// The same instance profile named by its ARN — the other form the option takes
/// ("the instance profile's name or its ARN") — as IAM names an instance profile
/// of this account. What a launch passes for it is still the ROLE that profile
/// carries, which is what [`passed_role_arn`] resolves.
fn instance_profile_arn(profile: &str) -> String {
    format!("arn:aws:iam::{}:instance-profile/{profile}", account())
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for,
/// as `iam:CreateRole` and the mint of a temporary credential record them.
fn given_session(emulator: &Emulator) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEEBLAUNCH1".to_string(),
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

/// Put `document` on the role as its inline policy — what `iam:PutRolePolicy`
/// does; `None` leaves the role carrying none, as a role nothing was put on.
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
    allow_on(actions, "*")
}

/// A policy document allowing this service's actions on everything and
/// `actions` on `resource` alone.
fn allow_on(actions: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"elasticbeanstalk:*","Resource":"*"}},{{"Effect":"Allow","Action":{actions},"Resource":"{resource}"}}]}}"#
    )
}

/// A policy document allowing every action of every service and denying
/// `actions`.
fn allow_everything_but(actions: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},{{"Effect":"Deny","Action":{actions},"Resource":"*"}}]}}"#
    )
}

/// One row: the document in force on the caller, the instance profile the
/// launched configuration names, and whether the launch is provisioned.
struct Row {
    case: &'static str,
    document: Option<String>,
    /// `None` names no profile at all, so the launch passes the role the
    /// option's published default names.
    instance_profile: Option<String>,
    launched: bool,
}

fn rows() -> Vec<Row> {
    let pass_role = "\"iam:PassRole\"";
    vec![
        Row {
            case: "an allow of this service's actions alone",
            document: Some(allow("\"elasticbeanstalk:DescribeEnvironments\"")),
            instance_profile: None,
            launched: false,
        },
        Row {
            case: "an explicit deny of the pass beside an allow of everything",
            document: Some(allow_everything_but(pass_role)),
            instance_profile: None,
            launched: false,
        },
        Row {
            case: "an allow of the pass on every role",
            document: Some(allow(pass_role)),
            instance_profile: None,
            launched: true,
        },
        Row {
            case: "an allow of the pass on exactly the default profile's role",
            document: Some(allow_on(
                pass_role,
                &passed_role_arn(eb::state::DEFAULT_INSTANCE_PROFILE),
            )),
            instance_profile: None,
            launched: true,
        },
        Row {
            case: "an allow of the pass on exactly the named profile's role",
            document: Some(allow_on(pass_role, &passed_role_arn(NAMED_PROFILE))),
            instance_profile: Some(NAMED_PROFILE.to_string()),
            launched: true,
        },
        Row {
            case: "a profile named by its ARN, with the pass allowed on the role it carries",
            document: Some(allow_on(pass_role, &passed_role_arn(NAMED_PROFILE))),
            instance_profile: Some(instance_profile_arn(NAMED_PROFILE)),
            launched: true,
        },
        Row {
            case: "a profile named by its ARN, with the pass allowed on another role",
            document: Some(allow_on(pass_role, &passed_role_arn(OTHER_PROFILE))),
            instance_profile: Some(instance_profile_arn(NAMED_PROFILE)),
            launched: false,
        },
        Row {
            case: "an allow of the pass on another role than the one passed",
            document: Some(allow_on(pass_role, &passed_role_arn(OTHER_PROFILE))),
            instance_profile: Some(NAMED_PROFILE.to_string()),
            launched: false,
        },
        Row {
            case: "an allow of the pass on the default role while the configuration names one",
            document: Some(allow_on(
                pass_role,
                &passed_role_arn(eb::state::DEFAULT_INSTANCE_PROFILE),
            )),
            instance_profile: Some(NAMED_PROFILE.to_string()),
            launched: false,
        },
    ]
}

/// Create the application every launch below is made under, as the account.
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

/// Launch `ENVIRONMENT` as `access_key_id`, with the configuration naming
/// `instance_profile` where the row names one.
async fn create_environment_as(
    emulator: &Emulator,
    access_key_id: &str,
    instance_profile: Option<&str>,
) -> Answer {
    let stack = SOLUTION_STACK.as_str();
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", ENVIRONMENT),
        ("SolutionStackName", stack),
    ];
    if let Some(instance_profile) = instance_profile {
        params.push(("OptionSettings.member.1.Namespace", LAUNCH_CONFIGURATION));
        params.push(("OptionSettings.member.1.OptionName", IAM_INSTANCE_PROFILE));
        params.push(("OptionSettings.member.1.Value", instance_profile));
    }
    emulator
        .call_as(access_key_id, TEST_REGION, "CreateEnvironment", &params)
        .await
}

/// The `(Severity, Message)` pairs `DescribeEvents` reports for `environment`,
/// newest first. A message is read back through the shared XML unescaper, since
/// the harness reads the raw slice between two tags and these messages quote the
/// option and the role they name.
async fn events_of(emulator: &Emulator, case: &str, environment: &str) -> Vec<(String, String)> {
    let described = emulator
        .call("DescribeEvents", &[("EnvironmentName", environment)])
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeEvents, got {} and body {}",
        described.status, described.body
    );
    described
        .elements_at("Events/member")
        .iter()
        .map(|event| {
            (
                event.value("Severity").unwrap_or_default().to_string(),
                xml_unescaped(event.value("Message").unwrap_or_default()),
            )
        })
        .collect()
}

/// Assert what the event stream of a launch that was refused the pass carries:
/// IAM's own refusal, naming the principal, the action and the role the launch
/// would have passed, and the service's statement that the launch failed.
async fn assert_launch_failure_events(emulator: &Emulator, case: &str, environment: &str, profile: &str) {
    let events = events_of(emulator, case, environment).await;
    let failed = (ERROR.to_string(), eb::state::LAUNCH_FAILED_EVENT.to_string());
    assert!(
        events.contains(&failed),
        "case {case:?}: expected the {ERROR} event {:?}, got {events:?}",
        eb::state::LAUNCH_FAILED_EVENT
    );
    let expected = format!(
        "User: {} is not authorized to perform: iam:PassRole on resource: {}",
        session_arn(),
        passed_role_arn(profile)
    );
    assert!(
        events
            .iter()
            .any(|(severity, message)| severity == ERROR && message == &expected),
        "case {case:?}: expected an {ERROR} event stating {expected:?}, got {events:?}"
    );
}

/// Assert that the event stream of a provisioned launch reports no failure at
/// all: the launch was permitted, so nothing refused it.
async fn assert_no_failure_events(emulator: &Emulator, case: &str, environment: &str) {
    let events = events_of(emulator, case, environment).await;
    assert!(
        !events.iter().any(|(severity, _)| severity == ERROR),
        "case {case:?}: a provisioned launch must record no {ERROR} event, got {events:?}"
    );
}

/// RULE, over the documents a caller may carry and the profiles a configuration
/// may name, through `CreateEnvironment`.
#[tokio::test]
async fn a_launch_needs_the_callers_permission_to_pass_the_instance_profile() {
    for row in rows() {
        let case = row.case;
        // One emulator per row, so a provisioned launch cannot make the next
        // row's launch a duplicate name instead of the decision under test.
        let emulator = Emulator::with_identity_services();
        given_application(&emulator, case).await;
        given_session(&emulator);
        given_role_policy(&emulator, row.document.as_deref());

        let launched =
            create_environment_as(&emulator, SESSION_KEY_ID, row.instance_profile.as_deref()).await;
        // The call is ACCEPTED either way: the pass is not the request's to get
        // right, and a launch that cannot be provisioned still starts.
        assert_eq!(
            launched.status, 200,
            "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
            launched.status, launched.body
        );
        assert_eq!(
            launched.value("Status"),
            Some(LAUNCHING),
            "case {case:?}: an accepted launch starts {LAUNCHING}, got body {}",
            launched.body
        );

        let expected = if row.launched { READY } else { TERMINATED };
        wait_for_status(&emulator, case, ENVIRONMENT, expected).await;
        // The profile the launch passed for, in whichever form the row named it
        // — the role the decision was taken on is the service's resolution of
        // it ([`passed_role_arn`]).
        let profile = row
            .instance_profile
            .clone()
            .unwrap_or_else(|| eb::state::DEFAULT_INSTANCE_PROFILE.to_string());
        if row.launched {
            assert_no_failure_events(&emulator, case, ENVIRONMENT).await;
        } else {
            assert_launch_failure_events(&emulator, case, ENVIRONMENT, &profile).await;
        }
    }
}

/// RULE for the caller that holds no IAM policies: the account's own published
/// credential, which every other suite signs with, may pass the profile — so an
/// environment it launches reaches `Ready`.
#[tokio::test]
async fn the_accounts_own_credential_may_pass_the_instance_profile() {
    let case = "the account's own published credential";
    let emulator = Emulator::with_identity_services();
    given_application(&emulator, case).await;
    given_session(&emulator);
    // A policy denying the pass is in force on the ROLE, which this caller is
    // not: the decision is about the principal the credential speaks for.
    given_role_policy(&emulator, Some(&allow_everything_but("\"iam:PassRole\"")));

    let launched = create_environment_as(&emulator, ACCOUNT_KEY_ID, None).await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    wait_for_status(&emulator, case, ENVIRONMENT, READY).await;
    assert_no_failure_events(&emulator, case, ENVIRONMENT).await;
}

/// The environment manifest a composition reads: the configuration-template
/// version, the environment it names, the solution stack to launch it on and —
/// where the row names one — the instance profile its instances are given.
fn manifest(instance_profile: Option<&str>) -> Vec<u8> {
    let mut text = String::from("AWSConfigurationTemplateVersion: 1.1.0.0\n");
    text.push_str(&format!("EnvironmentName: {COMPOSED_ENVIRONMENT}\n"));
    text.push_str(&format!("SolutionStack: {}\n", SOLUTION_STACK.as_str()));
    if let Some(instance_profile) = instance_profile {
        text.push_str(&format!(
            "OptionSettings:\n  {LAUNCH_CONFIGURATION}:\n    {IAM_INSTANCE_PROFILE}: \
             '{instance_profile}'\n"
        ));
    }
    text.into_bytes()
}

/// Upload a source bundle carrying that manifest and create the application
/// version from it with pre-processing turned on, as the account — the state a
/// client that is about to compose has reached.
async fn given_manifest_version(emulator: &Emulator, case: &str, instance_profile: Option<&str>) {
    emulator
        .put_object(
            BUNDLE_BUCKET,
            BUNDLE_KEY,
            &zip_of(&[("env.yaml", &manifest(instance_profile))]),
        )
        .await;
    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", VERSION_LABEL),
                ("SourceBundle.S3Bucket", BUNDLE_BUCKET),
                ("SourceBundle.S3Key", BUNDLE_KEY),
                ("Process", "true"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
        created.status, created.body
    );
    wait_for_version_status(
        emulator,
        case,
        APPLICATION,
        VERSION_LABEL,
        eb::state::VERSION_STATUS_PROCESSED,
    )
    .await;
}

/// The sentence Elastic Beanstalk refuses to provision an environment with no
/// IAM instance profile with, spelled out here rather than taken from the
/// service's own constant: the configuration option it names and the account
/// role it names are fixed parts of it.
const NO_INSTANCE_PROFILE: &str = "Environment must have instance profile associated with it. \
                                   Please specify profile name in configuration option \
                                   'aws:autoscaling:launchconfiguration/IamInstanceProfile' or \
                                   add role with name 'aws-elasticbeanstalk-ec2-role' to this \
                                   account";

/// RULE for the other way a launch can be left with no usable instance profile:
/// a composed environment is configured by its manifest alone, so a manifest
/// that associates none leaves the launch without one and it fails with the
/// service's own instance-profile statement — whatever the caller may pass.
#[tokio::test]
async fn a_manifest_that_associates_no_instance_profile_composes_a_launch_that_fails() {
    let case = "a manifest associating no instance profile";
    let emulator = Emulator::with_identity_services();
    given_application(&emulator, case).await;
    given_manifest_version(&emulator, case, None).await;

    // The account's own published credential, which may pass any role: what
    // refuses this launch is the configuration, not the caller.
    let composed = emulator
        .call(
            "ComposeEnvironments",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", VERSION_LABEL),
            ],
        )
        .await;
    assert_eq!(
        composed.status, 200,
        "case {case:?}: expected 200 from ComposeEnvironments, got {} and body {}",
        composed.status, composed.body
    );
    assert_eq!(
        composed.values_at("Environments/member/Status"),
        vec![LAUNCHING],
        "case {case:?}: an accepted composition starts {LAUNCHING}, got body {}",
        composed.body
    );

    wait_for_status(&emulator, case, COMPOSED_ENVIRONMENT, TERMINATED).await;
    let events = events_of(&emulator, case, COMPOSED_ENVIRONMENT).await;
    for expected in [NO_INSTANCE_PROFILE, eb::state::LAUNCH_FAILED_EVENT] {
        assert!(
            events.contains(&(ERROR.to_string(), expected.to_string())),
            "case {case:?}: expected the {ERROR} event {expected:?}, got {events:?}"
        );
    }
}

/// RULE through the OTHER launch: `ComposeEnvironments` reads the configuration
/// out of the manifest, and the environment it composes needs the same pass as
/// one `CreateEnvironment` launches.
#[tokio::test]
async fn a_composed_launch_needs_the_same_permission() {
    // Two rows: the same composition under a caller that may pass the
    // manifest's profile and one that may not.
    let rows: [(&str, Option<String>, bool); 3] = [
        (
            "a composing caller allowed the pass",
            Some(allow("\"iam:PassRole\"")),
            true,
        ),
        (
            "a composing caller allowed this service alone",
            Some(allow("\"elasticbeanstalk:DescribeEnvironments\"")),
            false,
        ),
        (
            "a composing caller denied the pass",
            Some(allow_everything_but("\"iam:PassRole\"")),
            false,
        ),
    ];
    for (case, document, launched) in rows {
        let emulator = Emulator::with_identity_services();
        given_application(&emulator, case).await;
        given_manifest_version(&emulator, case, Some(NAMED_PROFILE)).await;
        given_session(&emulator);
        given_role_policy(&emulator, document.as_deref());

        let composed = emulator
            .call_as(
                SESSION_KEY_ID,
                TEST_REGION,
                "ComposeEnvironments",
                &[
                    ("ApplicationName", APPLICATION),
                    ("VersionLabels.member.1", VERSION_LABEL),
                ],
            )
            .await;
        assert_eq!(
            composed.status, 200,
            "case {case:?}: expected 200 from ComposeEnvironments, got {} and body {}",
            composed.status, composed.body
        );
        assert_eq!(
            composed.values_at("Environments/member/EnvironmentName"),
            vec![COMPOSED_ENVIRONMENT],
            "case {case:?}: expected the manifest's environment, got body {}",
            composed.body
        );
        assert_eq!(
            composed.values_at("Environments/member/Status"),
            vec![LAUNCHING],
            "case {case:?}: an accepted composition starts {LAUNCHING}, got body {}",
            composed.body
        );

        let expected = if launched { READY } else { TERMINATED };
        wait_for_status(&emulator, case, COMPOSED_ENVIRONMENT, expected).await;
        if launched {
            assert_no_failure_events(&emulator, case, COMPOSED_ENVIRONMENT).await;
        } else {
            assert_launch_failure_events(&emulator, case, COMPOSED_ENVIRONMENT, NAMED_PROFILE)
                .await;
        }
    }
}
