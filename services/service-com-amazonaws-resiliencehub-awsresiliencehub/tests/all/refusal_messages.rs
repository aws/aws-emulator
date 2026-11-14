// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, end to end over HTTP.
//!
//! The `message` of an error is part of the contract a client reads — it branches on
//! the text to tell one 404 or 409 from another — so each suite below asserts the
//! whole sentence, not just the status and the error code.
//!
//! Four rules are under test, each over a TABLE that spans its class rather than the
//! one request that exposed it:
//!
//! 1. a value outside the `@pattern` of the member it was sent for is refused with the
//!    service's bracketed constraint report, naming the expression and the value —
//!    whichever operation and whichever member carried it; every OTHER modelled
//!    constraint keeps the common framework wording;
//! 2. an `appArn` under which nothing is stored is the same sentence from every
//!    operation that resolves one, and likewise a `policyArn`;
//! 3. a `CreateApp`/`UpdateApp` that cannot attach the policy it names states the
//!    attach failure and names the application as well as the policy;
//! 4. a name or a template resource already taken is refused with the conflict
//!    sentence for that kind of duplicate.
//!
//! The harness (`emulator`, `post`, `Reply`, `create_a_policy`) is the one
//! `application_lifecycle` drives every request of this crate's suites through: the
//! real wire path, `MultiServiceRouter::dispatch` with a restJson1 request.

use serde_json::{json, Value};

use super::application_lifecycle::{create_a_policy, emulator, post, required_objectives, Reply, ACCOUNT, REGION};
use smithy_server_runtime::MultiServiceRouter;

/// The `@pattern` of the `Arn` shape, transcribed from the model
/// (`smithy_query(query="shape",shape="Arn")`). Every `appArn` and `policyArn` member
/// targets it.
const ARN_PATTERN: &str = "^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):\
                           [A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:([a-z]{2}-((iso[a-z]{0,1}-)|\
                           (gov-)){0,1}[a-z]+-[0-9]):[0-9]{12}:[A-Za-z0-9/]\
                           [A-Za-z0-9:_/+.-]{0,1023}$";

/// The `@pattern` of the `EntityName` shape, transcribed from the model
/// (`smithy_query(query="shape",shape="EntityName")`): `CreateApp$name`,
/// `CreateResiliencyPolicy$policyName`, `StartAppAssessment$assessmentName` and
/// `CreateAppVersionResource$resourceName` all target it.
const ENTITY_NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$";

/// The `@pattern` of the `EntityVersion` shape, transcribed from the model
/// (`smithy_query(query="operation",operation="DescribeAppVersionTemplate")`, whose
/// `appVersion` targets it).
const ENTITY_VERSION_PATTERN: &str = r"^\S{1,50}$";

/// The `@pattern` of the `CustomerId` shape, transcribed from the model
/// (`smithy_query(query="operation",operation="CreateAppVersionResource")`, whose
/// `awsAccountId` targets it).
const CUSTOMER_ID_PATTERN: &str = "^[0-9]{12}$";

/// The `@pattern` of the `AwsRegion` shape, transcribed from the model
/// (`smithy_query(query="operation",operation="CreateAppVersionResource")`, whose
/// `awsRegion` targets it).
const AWS_REGION_PATTERN: &str = "^[a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]$";

/// The `@pattern` of the `NextToken` shape, transcribed from the model
/// (`smithy_query(query="operation",operation="ListAppVersions")`, whose `nextToken`
/// targets it).
const NEXT_TOKEN_PATTERN: &str = r"^\S{1,2000}$";

/// The widest `message` any of this service's errors carries: every one of them
/// targets `String500` (`smithy_query(query="shape",shape="ValidationException")`).
const MAX_MESSAGE_LENGTH: usize = 500;

/// An `appArn` of the right shape that nothing was ever stored under.
fn unknown_app_arn(id: &str) -> String {
    format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/{id}")
}

/// A `policyArn` of the right shape that nothing was ever stored under.
fn unknown_policy_arn(id: &str) -> String {
    format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/{id}")
}

/// Create an application named `name` and answer its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// The `message` the refusal carried.
fn message(reply: &Reply) -> String {
    reply.body["message"].as_str().unwrap_or_default().to_string()
}

/// The constraint report this service states for one value outside one member's
/// `@pattern`.
fn pattern_report(pattern: &str, value: &str) -> String {
    format!("[ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"]")
}

/// One row of the constraint-report table: a request carrying one value outside the
/// `@pattern` of the member it was sent for.
struct PatternCase {
    label: &'static str,
    target: &'static str,
    body: Value,
    /// The expression the value had to satisfy.
    pattern: &'static str,
    /// The value that did not satisfy it, as the report echoes it.
    value: String,
}

/// A value outside a member's modelled `@pattern` is refused with this service's own
/// constraint report — the expression and the value, bracketed — whichever operation
/// and whichever member carried it.
#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_with_the_constraint_report() {
    let router = emulator();
    let app_arn = create_an_app(&router, "pattern-report-app").await;
    // 61 characters: one past `EntityName`'s `[A-Za-z0-9]` head plus 59 tail
    // characters.
    let too_long_name = format!("a{}", "b".repeat(60));

    let cases = vec![
        PatternCase {
            label: "CreateApp name holding spaces",
            target: "/create-app",
            body: json!({"name": "a name with spaces"}),
            pattern: ENTITY_NAME_PATTERN,
            value: "a name with spaces".to_string(),
        },
        PatternCase {
            label: "CreateApp name one character short of the pattern's tail",
            target: "/create-app",
            body: json!({"name": "a"}),
            pattern: ENTITY_NAME_PATTERN,
            value: "a".to_string(),
        },
        PatternCase {
            label: "CreateApp name one character past the pattern's tail",
            target: "/create-app",
            body: json!({"name": too_long_name.clone()}),
            pattern: ENTITY_NAME_PATTERN,
            value: too_long_name.clone(),
        },
        PatternCase {
            label: "CreateApp awsApplicationArn that is not an ARN",
            target: "/create-app",
            body: json!({"name": "another-app", "awsApplicationArn": "arn:aws:bogus"}),
            pattern: ARN_PATTERN,
            value: "arn:aws:bogus".to_string(),
        },
        PatternCase {
            label: "DescribeApp appArn that is not an ARN",
            target: "/describe-app",
            body: json!({"appArn": "not-an-arn"}),
            pattern: ARN_PATTERN,
            value: "not-an-arn".to_string(),
        },
        PatternCase {
            label: "DescribeResiliencyPolicy policyArn that is not an ARN",
            target: "/describe-resiliency-policy",
            body: json!({"policyArn": "not-an-arn"}),
            pattern: ARN_PATTERN,
            value: "not-an-arn".to_string(),
        },
        PatternCase {
            label: "DeleteResiliencyPolicy policyArn that is empty",
            target: "/delete-resiliency-policy",
            body: json!({"policyArn": ""}),
            pattern: ARN_PATTERN,
            value: String::new(),
        },
        PatternCase {
            label: "CreateResiliencyPolicy policyName holding a punctuation character",
            target: "/create-resiliency-policy",
            body: json!({
                "policyName": "policy!name",
                "tier": "Important",
                "policy": required_objectives(3600),
            }),
            pattern: ENTITY_NAME_PATTERN,
            value: "policy!name".to_string(),
        },
        PatternCase {
            label: "DescribeAppVersionTemplate appVersion holding whitespace",
            target: "/describe-app-version-template",
            body: json!({"appArn": app_arn, "appVersion": "draft version"}),
            pattern: ENTITY_VERSION_PATTERN,
            value: "draft version".to_string(),
        },
        PatternCase {
            label: "StartAppAssessment assessmentName holding a punctuation character",
            target: "/start-app-assessment",
            body: json!({
                "appArn": app_arn,
                "appVersion": "release",
                "assessmentName": "assessment$1",
            }),
            pattern: ENTITY_NAME_PATTERN,
            value: "assessment$1".to_string(),
        },
        PatternCase {
            label: "CreateAppVersionResource awsAccountId that is not twelve digits",
            target: "/create-app-version-resource",
            body: json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": "Probe"},
                "physicalResourceId": "probe",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["probe-tier"],
                "awsAccountId": "12",
            }),
            pattern: CUSTOMER_ID_PATTERN,
            value: "12".to_string(),
        },
        PatternCase {
            label: "CreateAppVersionResource awsRegion in the wrong case",
            target: "/create-app-version-resource",
            body: json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": "Probe"},
                "physicalResourceId": "probe",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["probe-tier"],
                "awsRegion": "US-East-1",
            }),
            pattern: AWS_REGION_PATTERN,
            value: "US-East-1".to_string(),
        },
        PatternCase {
            label: "ListAppVersions nextToken holding whitespace",
            target: "/list-app-versions",
            body: json!({"appArn": app_arn, "nextToken": "page two"}),
            pattern: NEXT_TOKEN_PATTERN,
            value: "page two".to_string(),
        },
    ];

    for case in cases {
        let refused = post(&router, case.target, &case.body).await;
        refused.refused(case.label, 400, "ValidationException");
        let expected = pattern_report(case.pattern, &case.value);
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the constraint report {expected:?}, got {actual:?}",
            case.label
        );
        assert!(
            actual.chars().count() <= MAX_MESSAGE_LENGTH,
            "case {:?}: expected a message within the modelled {MAX_MESSAGE_LENGTH} \
             characters, got {} ({actual:?})",
            case.label,
            actual.chars().count()
        );
    }
}

/// A request breaking the `@pattern` of SEVERAL members is reported once, as one
/// bracketed list holding an entry per violated member.
#[tokio::test]
async fn one_report_holds_an_entry_per_member_outside_its_pattern() {
    let router = emulator();
    let refused = post(
        &router,
        "/create-app",
        &json!({"name": "bad name", "policyArn": "not-an-arn"}),
    )
    .await;
    refused.refused("two members outside their patterns", 400, "ValidationException");
    let actual = message(&refused);
    // The order the walk finds the members in is the framework's, so the entries are
    // asserted as a set inside the one report.
    assert!(
        actual.starts_with('[') && actual.ends_with(']'),
        "case {:?}: expected one bracketed report, got {actual:?}",
        "two members outside their patterns"
    );
    for entry in [
        format!("ECMA 262 regex \"{ENTITY_NAME_PATTERN}\" does not match input string \"bad name\""),
        format!("ECMA 262 regex \"{ARN_PATTERN}\" does not match input string \"not-an-arn\""),
    ] {
        assert!(
            actual.contains(&entry),
            "case {:?}: expected the report to hold {entry:?}, got {actual:?}",
            "two members outside their patterns"
        );
    }
}

/// Only the `@pattern` class is reported that way: a value breaking any other
/// modelled constraint keeps the framework wording this service's handler-side
/// refusals also state, so one rule is not reported in two formats.
#[tokio::test]
async fn a_violation_of_another_constraint_keeps_the_framework_wording() {
    let router = emulator();
    let app_arn = create_an_app(&router, "other-constraints-app").await;

    for (case, target, body) in [
        (
            "maxResults below its modelled range",
            "/list-app-versions",
            json!({"appArn": app_arn, "maxResults": 0}),
        ),
        (
            "clientToken shorter than its modelled length",
            "/delete-app",
            json!({"appArn": app_arn, "clientToken": ""}),
        ),
        (
            "a required member the request left out",
            "/create-app",
            json!({"description": "no name at all"}),
        ),
        (
            "assessmentSchedule outside its modelled value set",
            "/create-app",
            json!({"name": "scheduled-app", "assessmentSchedule": "Hourly"}),
        ),
    ] {
        let refused = post(&router, target, &body).await;
        refused.refused(case, 400, "ValidationException");
        let actual = message(&refused);
        assert!(
            actual.contains("validation error") && !actual.starts_with('['),
            "case {case:?}: expected the framework's validation wording, got {actual:?}"
        );
    }
}

/// An `appArn` nothing is stored under is the SAME sentence from every operation that
/// resolves one — whether the application was never created or has been deleted.
#[tokio::test]
async fn an_unknown_app_arn_is_the_same_not_found_sentence_everywhere() {
    let router = emulator();
    let deleted = create_an_app(&router, "app-to-delete").await;
    post(&router, "/delete-app", &json!({"appArn": deleted.clone()}))
        .await
        .ok("deleting the application");
    let never_minted = unknown_app_arn("11111111-1111-4111-8111-111111111111");

    for app_arn in [never_minted, deleted] {
        for (case, target, body) in [
            ("DescribeApp", "/describe-app", json!({"appArn": app_arn})),
            ("DeleteApp", "/delete-app", json!({"appArn": app_arn})),
            (
                "UpdateApp",
                "/update-app",
                json!({"appArn": app_arn, "description": "anything"}),
            ),
            (
                "ListAppVersions",
                "/list-app-versions",
                json!({"appArn": app_arn}),
            ),
            (
                "PublishAppVersion",
                "/publish-app-version",
                json!({"appArn": app_arn}),
            ),
            (
                "DescribeAppVersionTemplate",
                "/describe-app-version-template",
                json!({"appArn": app_arn, "appVersion": "draft"}),
            ),
            (
                "ListAppVersionResources",
                "/list-app-version-resources",
                json!({"appArn": app_arn, "appVersion": "draft"}),
            ),
            (
                "ResolveAppVersionResources",
                "/resolve-app-version-resources",
                json!({"appArn": app_arn, "appVersion": "draft"}),
            ),
            (
                "StartAppAssessment",
                "/start-app-assessment",
                json!({
                    "appArn": app_arn,
                    "appVersion": "release",
                    "assessmentName": "probe-assessment",
                }),
            ),
            (
                "CreateAppVersionAppComponent",
                "/create-app-version-app-component",
                json!({"appArn": app_arn, "name": "probe-tier", "type": "AWS::ResilienceHub::ComputeAppComponent"}),
            ),
            (
                "CreateAppVersionResource",
                "/create-app-version-resource",
                json!({
                    "appArn": app_arn,
                    "logicalResourceId": {"identifier": "Probe"},
                    "physicalResourceId": "probe",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["probe-tier"],
                }),
            ),
        ] {
            let refused = post(&router, target, &body).await;
            refused.refused(case, 404, "ResourceNotFoundException");
            let expected = format!("App not found for appArn {app_arn}");
            let actual = message(&refused);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// A `policyArn` nothing is stored under is the SAME sentence from every operation
/// that resolves one.
#[tokio::test]
async fn an_unknown_policy_arn_is_the_same_not_found_sentence_everywhere() {
    let router = emulator();
    let deleted = create_a_policy(&router, "policy-to-delete").await;
    post(
        &router,
        "/delete-resiliency-policy",
        &json!({"policyArn": deleted.clone()}),
    )
    .await
    .ok("deleting the policy");
    let never_minted = unknown_policy_arn("22222222-2222-4222-8222-222222222222");

    for policy_arn in [never_minted, deleted] {
        for (case, target, body) in [
            (
                "DescribeResiliencyPolicy",
                "/describe-resiliency-policy",
                json!({"policyArn": policy_arn}),
            ),
            (
                "DeleteResiliencyPolicy",
                "/delete-resiliency-policy",
                json!({"policyArn": policy_arn}),
            ),
            (
                "UpdateResiliencyPolicy",
                "/update-resiliency-policy",
                json!({"policyArn": policy_arn, "policyDescription": "anything"}),
            ),
        ] {
            let refused = post(&router, target, &body).await;
            refused.refused(case, 404, "ResourceNotFoundException");
            let expected = format!("ResiliencyPolicy {policy_arn} not found");
            let actual = message(&refused);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The attach-failure sentence, transcribed here from the service's own fixed text
/// so that a drift in how a handler words it fails this suite: the sentence
/// TERMINATES with a period followed by a single apostrophe, and only the policy ARN
/// (twice) and the application's name are interpolated.
fn attach_failure_message(policy_arn: &str, app_name: &str) -> String {
    format!(
        "Cannot attach policy {policy_arn} to app {app_name}. \
         Policy {policy_arn} does not exist.'"
    )
}

/// A create or update that cannot ATTACH the policy it names states the attach
/// failure and names the application as well as the policy — and stores nothing.
///
/// The table spans what the sentence interpolates: the application name's form
/// (hyphenated, underscored, the shortest and the widest `EntityName` admits) and the
/// policy ARN's resource id. The fixed text around them is the same for every row.
#[tokio::test]
async fn a_policy_that_cannot_be_attached_names_the_application() {
    let router = emulator();

    // 60 characters: `EntityName`'s `[A-Za-z0-9]` head plus the 59 tail characters
    // its `{1,59}` admits — the widest name the refusal can echo.
    let widest = format!("a{}", "b".repeat(59));
    // CreateApp: the application is named by the `name` the request carried, since
    // nothing was minted for it.
    for (case, name, policy_id) in [
        (
            "a policy id nothing minted",
            "attach-create-one".to_string(),
            "33333333-3333-4333-8333-333333333333",
        ),
        (
            "an underscored app name and another policy id",
            "attach_create_two".to_string(),
            "44444444-4444-4444-8444-444444444444",
        ),
        (
            "the shortest name EntityName admits",
            "ab".to_string(),
            "66666666-6666-4666-8666-666666666666",
        ),
        (
            "the widest name EntityName admits",
            widest,
            "77777777-7777-4777-8777-777777777777",
        ),
    ] {
        let policy_arn = unknown_policy_arn(policy_id);
        let refused = post(
            &router,
            "/create-app",
            &json!({"name": name, "policyArn": policy_arn}),
        )
        .await;
        refused.refused(case, 404, "ResourceNotFoundException");
        let expected = attach_failure_message(&policy_arn, &name);
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // Nothing was stored for the refused create: the name is still free, so the
        // same create without a policy succeeds.
        let created = post(&router, "/create-app", &json!({"name": name})).await;
        created.ok(case);
    }

    // UpdateApp: the application exists, and is named by the name it answers to. Both
    // ways a policy ARN can resolve to nothing are covered — never minted, and
    // deleted since.
    let deleted = create_a_policy(&router, "attach-policy-deleted").await;
    post(
        &router,
        "/delete-resiliency-policy",
        &json!({"policyArn": deleted.clone()}),
    )
    .await
    .ok("deleting the policy");

    for (case, name, policy_arn) in [
        (
            "UpdateApp with a policy nothing minted",
            "attach-update-app",
            unknown_policy_arn("55555555-5555-4555-8555-555555555555"),
        ),
        (
            "UpdateApp with a policy deleted since",
            "attach_update_app_2",
            deleted,
        ),
    ] {
        let app_arn = create_an_app(&router, name).await;
        let refused = post(
            &router,
            "/update-app",
            &json!({"appArn": app_arn, "policyArn": policy_arn}),
        )
        .await;
        refused.refused(case, 404, "ResourceNotFoundException");
        let expected = attach_failure_message(&policy_arn, name);
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // The refused update changed nothing: the application still carries no policy.
        let described = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        assert!(
            described.ok(case)["app"]["policyArn"].is_null(),
            "case {case:?}: expected the application to carry no policy, got {:?}",
            described.body
        );
    }
}

/// A `CreateApp` whose `name` another application already answers to is refused with
/// the duplicate-name conflict, naming the NAME the request asked for.
#[tokio::test]
async fn a_name_already_taken_is_refused_with_the_duplicate_name_conflict() {
    let router = emulator();
    // 60 characters: `EntityName`'s `[A-Za-z0-9]` head plus the 59 tail characters
    // its `{1,59}` admits — the widest name a duplicate can be reported for.
    let widest = format!("a{}", "b".repeat(59));

    for name in [
        "duplicate-app".to_string(),
        "duplicate_app_2".to_string(),
        "ab".to_string(),
        widest,
    ] {
        post(&router, "/create-app", &json!({"name": name}))
            .await
            .ok(&name);
        let refused = post(&router, "/create-app", &json!({"name": name})).await;
        refused.refused(&name, 409, "ConflictException");
        let expected = format!("App with name {name} already exists.");
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            actual.chars().count() <= MAX_MESSAGE_LENGTH,
            "case {name:?}: expected a message within the modelled {MAX_MESSAGE_LENGTH} \
             characters, got {}",
            actual.chars().count()
        );
    }
}

/// A resource the app template already holds is refused with the duplicate-resource
/// conflict, naming the resource by the id the template addresses it by and stating
/// the remediation — and the template still holds exactly the resources that were
/// created.
#[tokio::test]
async fn a_resource_already_in_the_template_is_refused_with_the_duplicate_conflict() {
    let router = emulator();
    let app_arn = create_an_app(&router, "duplicate-resource-app").await;

    // Each row differs in what the template addresses the resource by: a name the
    // caller chose, a name it chose for a resource whose physical id is an ARN, and
    // no name at all — in which case the logical identifier is the id it is held
    // under.
    for (case, body, expected_id) in [
        (
            "a named resource re-added",
            json!({
                "appArn": app_arn,
                "resourceName": "first-resource",
                "logicalResourceId": {"identifier": "FirstResource"},
                "physicalResourceId": "first-queue",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
            "first-resource",
        ),
        (
            "a named resource whose physical id is an ARN",
            json!({
                "appArn": app_arn,
                "resourceName": "second-resource",
                "logicalResourceId": {"identifier": "SecondResource"},
                "physicalResourceId": "arn:aws:sqs:us-east-1:000000000000:second-queue",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
            "second-resource",
        ),
        (
            "an unnamed resource, held under its logical identifier",
            json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": "ThirdResource"},
                "physicalResourceId": "third-queue",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
            "ThirdResource",
        ),
    ] {
        post(&router, "/create-app-version-resource", &body)
            .await
            .ok(case);

        let refused = post(&router, "/create-app-version-resource", &body).await;
        refused.refused(case, 409, "ConflictException");
        let expected = format!(
            "Resource with id {expected_id} exists in the current app template. Please update \
             the the app template to remove the duplicated resource."
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A second resource taking a NAME the template already holds is the same
    // refusal, naming that name: the id is what the template addresses, whichever
    // member of the request carried it.
    let taking_a_held_name = json!({
        "appArn": app_arn,
        "resourceName": "first-resource",
        "logicalResourceId": {"identifier": "FourthResource"},
        "physicalResourceId": "fourth-queue",
        "resourceType": "AWS::SQS::Queue",
        "appComponents": ["queue-tier"],
    });
    let refused = post(&router, "/create-app-version-resource", &taking_a_held_name).await;
    refused.refused("a held name under another logical id", 409, "ConflictException");
    let expected = "Resource with id first-resource exists in the current app template. Please \
                    update the the app template to remove the duplicated resource.";
    let actual = message(&refused);
    assert_eq!(
        actual, expected,
        "case {:?}: expected {expected:?}, got {actual:?}",
        "a held name under another logical id"
    );

    // The refused creates stored nothing: the draft holds one resource per
    // successful create and no more.
    let listed = post(
        &router,
        "/list-app-version-resources",
        &json!({"appArn": app_arn, "appVersion": "draft"}),
    )
    .await;
    let resources = listed.ok("the draft's resources")["physicalResources"]
        .as_array()
        .expect("a list of physical resources");
    assert_eq!(
        resources.len(),
        3,
        "case {:?}: expected the three created resources, got {resources:?}",
        "the draft's resources"
    );
}
