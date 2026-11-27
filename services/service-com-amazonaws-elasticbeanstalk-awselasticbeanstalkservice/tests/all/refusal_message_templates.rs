// SPDX-License-Identifier: CC0-1.0

//! How Elastic Beanstalk WORDS six classes of refusal, driven end to end over
//! its awsQuery HTTP surface with the shared harness in `crate::harness`.
//!
//! Each rule is the service's TEMPLATE for one error class — the fixed text, and
//! the parts taken from the request and from the store — asserted over a table
//! of inputs rather than over one request:
//!
//! RULE 1 (no environment selector): the operations whose model documents no
//! code for an absent `EnvironmentName`/`EnvironmentId` pair refuse a request
//! that names neither with `MissingParameter` (400) and WHOLLY FIXED text:
//! `You must specify either EnvironmentName or EnvironmentId.` Nothing of the
//! request appears in it.
//!
//! RULE 2 (a terminated environment): `AssociateEnvironmentOperationsRole`
//! resolves the name against the LIVE environments, so one whose termination has
//! completed is refused exactly as a name that was never created is — the shared
//! not-found template naming the member the request used — and the refusal
//! reports no status.
//!
//! RULE 3 (an environment that is not Ready): an operation that acts on a
//! running environment refuses one in any other status with
//! `Environment named <EnvironmentName> is in an invalid state for this
//! operation. Must be Ready.` — the name is the request's, the required status is
//! the operation's, and the status the environment is IN is not disclosed.
//!
//! RULE 4 (an ARN of a type that is not tagged): both tag operations refuse one
//! with `Tags aren't supported for the resource type of the ARN '<ResourceArn>'.`
//!
//! RULE 5 (an application version that does not exist): an operation that acts ON
//! a version refuses a `(ApplicationName, VersionLabel)` pair no version answers
//! to with `Application version with label '<VersionLabel>' not found in
//! application '<ApplicationName>'` — both halves named, no trailing period.
//!
//! RULE 6 (a compose that resolves no usable version): `ComposeEnvironments`
//! refuses a request none of whose labels names a version of the application
//! with the fixed sentence `No valid application version found.`, and one that
//! names a version of it that has not been successfully pre-processed with
//! `ApplicationVersion <VersionLabel> has not been successfully processed.
//! Current status: <Status>` — the label is the request's and the status is the
//! version's own.
//!
//! The templates are written out HERE rather than imported from the service, so a
//! change to the handlers' wording fails this suite instead of moving with it.
//! (Two of them are also asserted to equal the service's own shared statement, so
//! the operations that answer in them cannot drift apart.)
//!
//! The enum-constraint validation message — the seventh class — is asserted in
//! `crate::instances_health_validation_message`, which owns the published value
//! set it lists.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{wait_for_status, Emulator, SOLUTION_STACK};
use crate::lifecycle::{account, TEST_REGION};

/// RULE 1's sentence, as AWS words it: wholly fixed text.
const MISSING_SELECTOR_SENTENCE: &str = "You must specify either EnvironmentName or EnvironmentId.";

/// RULE 2's template: the member the request named the environment by and the
/// value it carried.
fn no_environment_found(member: &str, value: &str) -> String {
    format!("No Environment found for {member} = '{value}'.")
}

/// RULE 3's template: the environment the request named and the status the
/// operation requires.
fn invalid_state_for(environment_name: &str) -> String {
    format!(
        "Environment named {environment_name} is in an invalid state for this operation. \
         Must be Ready."
    )
}

/// RULE 4's template: the ARN the request carried.
fn tags_not_supported_for(resource_arn: &str) -> String {
    format!("Tags aren't supported for the resource type of the ARN '{resource_arn}'.")
}

/// RULE 5's template: the label and the application that together name a
/// version. No trailing period, the way the service words this one.
fn version_not_found(application_name: &str, version_label: &str) -> String {
    format!("Application version with label '{version_label}' not found in application '{application_name}'")
}

/// RULE 6's sentence for a request that resolves no version of the application:
/// wholly fixed text.
const NO_VALID_VERSION_SENTENCE: &str = "No valid application version found.";

/// RULE 6's template for a version of the application that has not been
/// successfully pre-processed: the label the request supplied and the status the
/// version is in, as the service puts an `ApplicationVersionStatus` on the wire.
fn not_successfully_processed(version_label: &str, status: &str) -> String {
    format!(
        "ApplicationVersion {version_label} has not been successfully processed. Current status: \
         {status}"
    )
}

/// Create the application every fixture of a test belongs to.
async fn application(emulator: &Emulator, name: &str) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", name)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture: CreateApplication {name:?} answered {} with body {}",
        created.status, created.body
    );
}

/// Launch an environment of `application` and answer its `EnvironmentId`. The
/// launch is NOT waited for here: a test that needs it settled waits for the
/// status it needs.
async fn launch(emulator: &Emulator, application_name: &str, name: &str) -> String {
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", application_name),
                ("EnvironmentName", name),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "fixture: CreateEnvironment {name:?} answered {} with body {}",
        launched.status, launched.body
    );
    launched
        .value("EnvironmentId")
        .expect("fixture: a launch reports the environment's id")
        .to_string()
}

/// Assert `answer` is a 400 carrying `code` and exactly `message`.
fn assert_refused(
    answer: &crate::harness::Answer,
    case: &str,
    code: &str,
    message: &str,
) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected error code {code}, got {:?} in body {}",
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.error_message(),
        message,
        "case {case:?}: expected {message:?}, got {:?}",
        answer.error_message()
    );
}

/// One row of RULE 1: the operation called, and what the request carries beside
/// the absent selector pair.
struct SelectorCase {
    case: &'static str,
    action: &'static str,
    other: &'static [(&'static str, &'static str)],
}

/// RULE 1's rows: every operation in the class, and what else the request
/// carries (a required member, a legal enum element of a list, a filter, a page
/// bound) — the sentence is the same whatever else was sent.
const SELECTOR_CASES: &[SelectorCase] = &[
    SelectorCase {
        case: "ApplyEnvironmentManagedAction with its required ActionId",
        action: "ApplyEnvironmentManagedAction",
        other: &[("ActionId", "7f1d3b2a-0c4e-4d6f-8a91-2b3c4d5e6f70")],
    },
    SelectorCase {
        case: "DescribeInstancesHealth with nothing else",
        action: "DescribeInstancesHealth",
        other: &[],
    },
    SelectorCase {
        case: "DescribeInstancesHealth with a legal AttributeNames element",
        action: "DescribeInstancesHealth",
        other: &[("AttributeNames.member.1", "All")],
    },
    SelectorCase {
        case: "DescribeEnvironmentHealth with a legal AttributeNames element",
        action: "DescribeEnvironmentHealth",
        other: &[("AttributeNames.member.1", "All")],
    },
    SelectorCase {
        case: "DescribeEnvironmentManagedActions with nothing else",
        action: "DescribeEnvironmentManagedActions",
        other: &[],
    },
    SelectorCase {
        case: "DescribeEnvironmentManagedActions filtered by Status",
        action: "DescribeEnvironmentManagedActions",
        other: &[("Status", "Scheduled")],
    },
    SelectorCase {
        case: "DescribeEnvironmentManagedActionHistory with a page bound",
        action: "DescribeEnvironmentManagedActionHistory",
        other: &[("MaxItems", "10")],
    },
];

/// RULE 1, over every operation that answers in it and over requests that carry
/// other members beside the absent pair.
#[tokio::test]
async fn a_request_naming_neither_environment_selector_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();

    for SelectorCase {
        case,
        action,
        other,
    } in SELECTOR_CASES
    {
        let refused = emulator.call(action, other).await;
        assert_refused(&refused, case, "MissingParameter", MISSING_SELECTOR_SENTENCE);
        // Fixed text: nothing the request carried is quoted back.
        for (_, value) in *other {
            assert!(
                !refused.error_message().contains(value),
                "case {case:?}: the fixed sentence must not quote {value:?}, got {:?}",
                refused.error_message()
            );
        }
    }

    // Stated by the service in exactly one place, so the operations above cannot
    // word one refusal several ways.
    assert_eq!(
        eb::state::MISSING_ENVIRONMENT_SELECTOR_MESSAGE, MISSING_SELECTOR_SENTENCE,
        "the service's own statement must be AWS's sentence"
    );
}

/// RULE 2: a terminated environment is no environment an operations role can be
/// associated with, and the refusal is the not-found one — not a state error.
#[tokio::test]
async fn a_terminated_environment_is_absent_to_the_operations_role_calls() {
    const APPLICATION: &str = "role-message-app";
    let emulator = Emulator::new();
    application(&emulator, APPLICATION).await;
    let role = format!("arn:aws:iam::{}:role/eb-operations", account());

    // Rows differ along what the rule generalises over: whether the environment
    // carried an operations role while it lived, and whether that role was put
    // on it by the launch or by a later association. Every row terminates from
    // `Ready`: a launch that has not got past its initial phase is one no
    // termination can take down.
    let rows: [(&str, &str, bool, bool); 3] = [
        ("terminated from Ready", "role-env-ready", true, false),
        ("terminated carrying no role", "role-env-launching", true, false),
        (
            "terminated after carrying a role",
            "role-env-associated",
            true,
            true,
        ),
    ];

    for (case, name, settle_first, associate_first) in rows {
        launch(&emulator, APPLICATION, name).await;
        if settle_first {
            wait_for_status(&emulator, case, name, "Ready").await;
        }
        if associate_first {
            let associated = emulator
                .call(
                    "AssociateEnvironmentOperationsRole",
                    &[("EnvironmentName", name), ("OperationsRole", role.as_str())],
                )
                .await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: a live environment takes the role, got {} and body {}",
                associated.status, associated.body
            );
        }
        let terminated = emulator
            .call("TerminateEnvironment", &[("EnvironmentName", name)])
            .await;
        assert_eq!(
            terminated.status, 200,
            "case {case:?}: TerminateEnvironment answered {} with body {}",
            terminated.status, terminated.body
        );
        wait_for_status(&emulator, case, name, "Terminated").await;

        let refused = emulator
            .call(
                "AssociateEnvironmentOperationsRole",
                &[("EnvironmentName", name), ("OperationsRole", role.as_str())],
            )
            .await;
        let expected = no_environment_found("EnvironmentName", name);
        assert_refused(&refused, case, "InvalidParameterValue", &expected);
        // The refusal says the environment is gone, not that it is busy: no
        // status is reported in it.
        for leaked in ["Terminated", "status", "state"] {
            assert!(
                !refused.error_message().contains(leaked),
                "case {case:?}: the not-found refusal must not report {leaked:?}, got {:?}",
                refused.error_message()
            );
        }
        // The companion call resolves the same name the same way, so the pair
        // cannot disagree about which environments it can see.
        let disassociated = emulator
            .call(
                "DisassociateEnvironmentOperationsRole",
                &[("EnvironmentName", name)],
            )
            .await;
        assert_refused(
            &disassociated,
            &format!("{case} / disassociate"),
            "InvalidParameterValue",
            &expected,
        );
    }

    // A name that was NEVER created is refused in exactly the same words, which
    // is the rule: a terminated environment is absent.
    let ghost = emulator
        .call(
            "AssociateEnvironmentOperationsRole",
            &[
                ("EnvironmentName", "role-env-ghost"),
                ("OperationsRole", role.as_str()),
            ],
        )
        .await;
    assert_refused(
        &ghost,
        "a name that was never created",
        "InvalidParameterValue",
        &no_environment_found("EnvironmentName", "role-env-ghost"),
    );
}

/// One row of RULE 3: which operation is called, which member names the
/// environment, and whether the environment is on its way out rather than still
/// launching.
struct StateCase {
    case: &'static str,
    action: &'static str,
    /// Name it by `EnvironmentId` rather than by `EnvironmentName`.
    by_id: bool,
    /// Shutting down (`Terminating`) rather than still coming up
    /// (`Launching`) — the two statuses an environment is refused in here.
    terminating: bool,
}

/// RULE 3's rows: every operation that requires a Ready environment, in each of
/// the two statuses one can be refused in, named by each member in turn.
const STATE_CASES: &[StateCase] = &[
    StateCase {
        case: "UpdateEnvironment on a launching environment by name",
        action: "UpdateEnvironment",
        by_id: false,
        terminating: false,
    },
    StateCase {
        case: "UpdateEnvironment on a terminating environment by id",
        action: "UpdateEnvironment",
        by_id: true,
        terminating: true,
    },
    StateCase {
        case: "RebuildEnvironment on a launching environment by id",
        action: "RebuildEnvironment",
        by_id: true,
        terminating: false,
    },
    StateCase {
        case: "RebuildEnvironment on a terminating environment by name",
        action: "RebuildEnvironment",
        by_id: false,
        terminating: true,
    },
    StateCase {
        case: "RestartAppServer on a launching environment by name",
        action: "RestartAppServer",
        by_id: false,
        terminating: false,
    },
    StateCase {
        case: "RestartAppServer on a terminating environment by id",
        action: "RestartAppServer",
        by_id: true,
        terminating: true,
    },
    StateCase {
        case: "SwapEnvironmentCNAMEs with a launching source by name",
        action: "SwapEnvironmentCNAMEs",
        by_id: false,
        terminating: false,
    },
    StateCase {
        case: "SwapEnvironmentCNAMEs with a terminating source by id",
        action: "SwapEnvironmentCNAMEs",
        by_id: true,
        terminating: true,
    },
    // An environment still shutting down EXISTS — the reads report it — so this
    // operation refuses it for its status, where one whose termination has
    // COMPLETED is absent to it (RULE 2 above). `EnvironmentName` is the only
    // member it takes.
    StateCase {
        case: "AssociateEnvironmentOperationsRole on a terminating environment by name",
        action: "AssociateEnvironmentOperationsRole",
        by_id: false,
        terminating: true,
    },
];

/// RULE 3, over the operations that act on a running environment and over the
/// statuses an environment can be refused in.
///
/// Every row gets its OWN fixture environment, brought into the status the row
/// needs immediately before the row's one call: a transient status is a status,
/// not a stopwatch, and nothing here waits a fixed time for one.
#[tokio::test]
async fn an_environment_that_is_not_ready_is_refused_naming_the_status_required() {
    const APPLICATION: &str = "state-message-app";
    let emulator = Emulator::new();
    application(&emulator, APPLICATION).await;
    let role = format!("arn:aws:iam::{}:role/eb-operations", account());

    // The destination of the swap rows: an environment that IS ready, so such a
    // refusal can only be about the other side. Launched together with the
    // fixtures the terminating rows terminate, so the suite waits for one span
    // of launches rather than one per row.
    let ready = "state-env-ready";
    let ready_id = launch(&emulator, APPLICATION, ready).await;
    let mut settled: Vec<(String, String)> = Vec::new();
    for (index, row) in STATE_CASES.iter().enumerate() {
        if row.terminating {
            let name = format!("state-env-t{index}");
            let environment_id = launch(&emulator, APPLICATION, &name).await;
            settled.push((name, environment_id));
        }
    }
    wait_for_status(&emulator, "fixture", ready, "Ready").await;
    for (name, _) in &settled {
        wait_for_status(&emulator, "fixture", name, "Ready").await;
    }
    let mut settled = settled.into_iter();

    for (index, row) in STATE_CASES.iter().enumerate() {
        let case = row.case;
        // The environment this row refuses, put into the row's status right now.
        let (name, environment_id, status) = if row.terminating {
            let (name, environment_id) = settled
                .next()
                .expect("fixture: one settled environment per terminating row");
            let terminated = emulator
                .call("TerminateEnvironment", &[("EnvironmentName", name.as_str())])
                .await;
            assert_eq!(
                terminated.status, 200,
                "case {case:?}: TerminateEnvironment answered {} with body {}",
                terminated.status, terminated.body
            );
            (name, environment_id, "Terminating")
        } else {
            let name = format!("state-env-l{index}");
            let environment_id = launch(&emulator, APPLICATION, &name).await;
            (name, environment_id, "Launching")
        };
        let (member, value) = if row.by_id {
            ("EnvironmentId", environment_id.as_str())
        } else {
            ("EnvironmentName", name.as_str())
        };
        let params: Vec<(&str, &str)> = match row.action {
            "UpdateEnvironment" => vec![(member, value), ("Description", "changed")],
            "AssociateEnvironmentOperationsRole" => {
                vec![(member, value), ("OperationsRole", role.as_str())]
            }
            "SwapEnvironmentCNAMEs" => {
                if row.by_id {
                    vec![
                        ("SourceEnvironmentId", value),
                        ("DestinationEnvironmentId", ready_id.as_str()),
                    ]
                } else {
                    vec![
                        ("SourceEnvironmentName", value),
                        ("DestinationEnvironmentName", ready),
                    ]
                }
            }
            _ => vec![(member, value)],
        };
        let refused = emulator.call(row.action, &params).await;
        assert_refused(
            &refused,
            case,
            "InvalidParameterValue",
            &invalid_state_for(&name),
        );
        assert!(
            !refused.error_message().contains(status),
            "case {case:?}: the refusal must not report the current status, got {:?}",
            refused.error_message()
        );
    }

    // The boundary of the rule: an environment that IS Ready is not refused for
    // its status at all.
    let accepted = emulator
        .call(
            "UpdateEnvironment",
            &[("EnvironmentName", ready), ("Description", "changed")],
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a Ready environment\": expected the update to be taken, got {} and body {}",
        accepted.status, accepted.body
    );
}

/// RULE 4, over both tag operations and over the ARN forms whose type this
/// service does not tag.
#[tokio::test]
async fn an_arn_of_an_untagged_type_is_refused_with_one_request_derived_sentence() {
    let emulator = Emulator::new();

    // `eb::state::TAGGABLE_RESOURCE_TYPES` is the set of types this service DOES
    // tag (application, applicationversion, configurationtemplate, environment,
    // platform); every row below carries a type outside it, in the ARN forms this
    // service's own resources use (with and without an account, with a path under
    // the type).
    let cases: Vec<(&str, String)> = vec![
        (
            "a solution stack, which belongs to no account",
            format!("arn:aws:elasticbeanstalk:{TEST_REGION}::solutionstack/64bit Amazon Linux"),
        ),
        (
            "a platform branch",
            format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platformbranch/Python 3.11"),
        ),
        (
            "an instance of this account",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:instance/i-0123456789abcdef0",
                account()
            ),
        ),
        (
            "a type with no resource path at all",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationmetrics",
                account()
            ),
        ),
        (
            "a taggable type's name in the wrong case",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:Application/app",
                account()
            ),
        ),
    ];

    for (case, resource_arn) in cases {
        let expected = tags_not_supported_for(&resource_arn);
        // Stated by the service in exactly one place, so the two operations
        // cannot word it differently.
        assert_eq!(
            eb::state::unsupported_tag_resource_type_message(&resource_arn),
            expected,
            "case {case:?}: the service's own template must be AWS's sentence"
        );
        for action in ["ListTagsForResource", "UpdateTagsForResource"] {
            let params = if action == "ListTagsForResource" {
                vec![("ResourceArn", resource_arn.as_str())]
            } else {
                vec![
                    ("ResourceArn", resource_arn.as_str()),
                    ("TagsToAdd.member.1.Key", "env"),
                    ("TagsToAdd.member.1.Value", "prod"),
                ]
            };
            let refused = emulator.call(action, &params).await;
            assert_refused(
                &refused,
                &format!("{case} / {action}"),
                "ResourceTypeNotSupportedException",
                &expected,
            );
        }
    }

    // The boundary of the rule: a TAGGABLE type whose resource does not exist is
    // a different refusal, and so is an ARN that is not this service's.
    let missing = format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/no-such-app",
        account()
    );
    let answered = emulator
        .call("ListTagsForResource", &[("ResourceArn", missing.as_str())])
        .await;
    assert_refused(
        &answered,
        "a taggable type whose resource does not exist",
        "ResourceNotFoundException",
        &format!("Resource not found for ARN '{missing}'."),
    );
}

/// RULE 5, over the pairs that name no stored application version.
#[tokio::test]
async fn an_application_version_that_does_not_exist_is_refused_naming_both_halves() {
    const APPLICATION: &str = "version-message-app";
    const OTHER_APPLICATION: &str = "version-message-other";
    let emulator = Emulator::new();
    application(&emulator, APPLICATION).await;
    application(&emulator, OTHER_APPLICATION).await;

    // A version under EACH application, so a row can name a label that exists
    // only under the other one.
    for (application_name, version_label) in [(APPLICATION, "v-kept"), (OTHER_APPLICATION, "v-elsewhere")] {
        let created = emulator
            .call(
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", application_name),
                    ("VersionLabel", version_label),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "fixture: CreateApplicationVersion {version_label:?} answered {} with body {}",
            created.status, created.body
        );
    }
    // One version created and then deleted, which is the state a stale label of
    // a client that kept it is in.
    let deleted_label = "v-deleted";
    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", deleted_label),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "fixture: CreateApplicationVersion {deleted_label:?} answered {} with body {}",
        created.status, created.body
    );
    let removed = emulator
        .call(
            "DeleteApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", deleted_label),
            ],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "fixture: DeleteApplicationVersion answered {} with body {}",
        removed.status, removed.body
    );

    // Rows span the ways a pair can name no version: a label nothing ever
    // carried, a label that was deleted, a label that exists only under another
    // application, and a label differing from a real one only in case.
    let cases: [(&str, &str, &str); 4] = [
        ("a label nothing ever carried", APPLICATION, "v-never"),
        ("a label that was deleted", APPLICATION, deleted_label),
        (
            "a label that exists only under another application",
            APPLICATION,
            "v-elsewhere",
        ),
        (
            "a label differing from a real one only in case",
            APPLICATION,
            "V-KEPT",
        ),
    ];

    for (case, application_name, version_label) in cases {
        let refused = emulator
            .call(
                "UpdateApplicationVersion",
                &[
                    ("ApplicationName", application_name),
                    ("VersionLabel", version_label),
                    ("Description", "changed"),
                ],
            )
            .await;
        let expected = version_not_found(application_name, version_label);
        assert_refused(&refused, case, "InvalidParameterValue", &expected);
        let message = refused.error_message();
        assert!(
            message.contains(application_name) && message.contains(version_label),
            "case {case:?}: the refusal must name both halves of the pair, got {message:?}"
        );
        assert!(
            !message.ends_with('.'),
            "case {case:?}: this refusal carries no trailing period, got {message:?}"
        );
        // Stated by the service in exactly one place.
        assert_eq!(
            eb::state::missing_application_version_message(application_name, version_label),
            expected,
            "case {case:?}: the service's own template must be AWS's sentence"
        );
    }

    // The boundary of the rule: an APPLICATION that does not exist is reported as
    // that, and a pair that DOES name a version is updated — read back through
    // the version's own describe.
    let unknown_application = emulator
        .call(
            "UpdateApplicationVersion",
            &[
                ("ApplicationName", "version-message-ghost"),
                ("VersionLabel", "v-kept"),
                ("Description", "changed"),
            ],
        )
        .await;
    assert_refused(
        &unknown_application,
        "an application that does not exist",
        "InvalidParameterValue",
        "No Application named 'version-message-ghost' found.",
    );

    let updated = emulator
        .call(
            "UpdateApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", "v-kept"),
                ("Description", "the new description"),
            ],
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"a pair that names a version\": expected 200, got {} and body {}",
        updated.status, updated.body
    );
    let described = emulator
        .call(
            "DescribeApplicationVersions",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", "v-kept"),
            ],
        )
        .await;
    assert_eq!(
        described.values_at("ApplicationVersions/member/Description"),
        vec!["the new description"],
        "case \"a pair that names a version\": the update must be readable back, got body {}",
        described.body
    );
}

/// RULE 6, over the version labels a compose can resolve to no usable version.
#[tokio::test]
async fn a_compose_that_resolves_no_usable_version_is_refused_with_one_fixed_sentence() {
    const APPLICATION: &str = "compose-message-app";
    const OTHER_APPLICATION: &str = "compose-message-other";
    let emulator = Emulator::new();
    application(&emulator, APPLICATION).await;
    application(&emulator, OTHER_APPLICATION).await;
    for (application_name, version_label) in [(APPLICATION, "v-own"), (OTHER_APPLICATION, "v-foreign")] {
        let created = emulator
            .call(
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", application_name),
                    ("VersionLabel", version_label),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "fixture: CreateApplicationVersion {version_label:?} answered {} with body {}",
            created.status, created.body
        );
    }

    // Rows span how many labels the request carries, where each one lives — a
    // label of another application, a label nothing carries, a label of the named
    // application — and whether the request also names a group. The versions
    // these fixtures create ask for no pre-processing, so the application's own
    // version sits at the status a version nothing validated reports.
    const UNPROCESSED: &str = "UNPROCESSED";
    const NOT_RESOLVED: &str = "MissingParameter";
    const NOT_PROCESSED: &str = "ConfigurationValidationException";
    /// One request this rule refuses, and the refusal it must carry.
    struct Case {
        case: &'static str,
        params: Vec<(&'static str, &'static str)>,
        code: &'static str,
        message: String,
    }
    let cases: Vec<Case> = vec![
        Case {
            case: "a label that belongs to another application",
            params: vec![
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", "v-foreign"),
            ],
            code: NOT_RESOLVED,
            message: NO_VALID_VERSION_SENTENCE.to_string(),
        },
        Case {
            case: "a label nothing carries",
            params: vec![
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", "v-never"),
            ],
            code: NOT_RESOLVED,
            message: NO_VALID_VERSION_SENTENCE.to_string(),
        },
        Case {
            case: "a label of the named application",
            params: vec![
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", "v-own"),
            ],
            code: NOT_PROCESSED,
            message: not_successfully_processed("v-own", UNPROCESSED),
        },
        Case {
            case: "two labels at once",
            params: vec![
                ("ApplicationName", APPLICATION),
                ("VersionLabels.member.1", "v-own"),
                ("VersionLabels.member.2", "v-foreign"),
            ],
            code: NOT_PROCESSED,
            message: not_successfully_processed("v-own", UNPROCESSED),
        },
        Case {
            case: "a group name beside the labels",
            params: vec![
                ("ApplicationName", APPLICATION),
                ("GroupName", "dev"),
                ("VersionLabels.member.1", "v-own"),
            ],
            code: NOT_PROCESSED,
            message: not_successfully_processed("v-own", UNPROCESSED),
        },
    ];

    for Case {
        case,
        params,
        code,
        message,
    } in cases
    {
        let refused = emulator.call("ComposeEnvironments", &params).await;
        assert_refused(&refused, case, code, &message);
        if message == NO_VALID_VERSION_SENTENCE {
            // Fixed text: no label, no application and no diagnosis of a source
            // bundle appears in it.
            for (_, value) in &params {
                assert!(
                    !refused.error_message().contains(value),
                    "case {case:?}: the fixed sentence must not quote {value:?}, got {:?}",
                    refused.error_message()
                );
            }
        }
    }

    // The boundary of the rule: the APPLICATION is resolved first, so a name no
    // application exists under is reported as that instead.
    let unknown_application = emulator
        .call(
            "ComposeEnvironments",
            &[
                ("ApplicationName", "compose-message-ghost"),
                ("VersionLabels.member.1", "v-own"),
            ],
        )
        .await;
    assert_refused(
        &unknown_application,
        "an application that does not exist",
        "MissingParameter",
        "Application could not be found",
    );
}
