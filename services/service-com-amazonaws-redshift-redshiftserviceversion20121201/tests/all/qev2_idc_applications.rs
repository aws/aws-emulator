// SPDX-License-Identifier: CC0-1.0

//! The Query Editor (QEV2) IAM Identity Center application lifecycle, end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created application is stored exactly as the request configured
//! it, with a stable `Qev2IdcApplicationArn` later calls reach it by and the
//! `IdcManagedApplicationArn` of the IAM Identity Center instance it names; the
//! application NAME being what one account's applications are unique on, and a
//! delete freeing it; an ARN that names no application being refused by every
//! operation that takes one, with one fixed sentence stating the precondition
//! that operation enforces — existence for the read and the delete, existence
//! and finished onboarding for the modify; a modify changing only the display name; a delete
//! making the application absent from every later read; the service's
//! `MaxRecords` window and `Marker` cursor; an `IdcInstanceArn` that is not an
//! ARN (or is longer than one can be) being refused with nothing stored; and the
//! documented tag limit and the store's own bound on the collection.

use std::collections::BTreeSet;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Qev2IdcApplication;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// An IAM Identity Center instance ARN the ACCOUNT holds, read from the shared
/// harness: the instance is a global IAM Identity Center resource, so its ARN
/// carries no Region or account, and the managed application a create registers
/// can only be created in an instance the account holds.
const IDC_INSTANCE_ARN: &str = crate::cluster_lifecycle::IDENTITY_CENTER_INSTANCES[0];

/// A SECOND instance the account holds, so a row can vary the instance a create
/// names.
const OTHER_IDC_INSTANCE_ARN: &str = crate::cluster_lifecycle::IDENTITY_CENTER_INSTANCES[1];

/// The members of a `Qev2IdcApplication` this suite compares between a write's
/// answer and a later read's, so the two records cannot disagree.
///
/// Source: `smithy_query(query="shape", shape="Qev2IdcApplication")` — every
/// member of the shape except `Tags`, which is a list and is compared on its own.
const APPLICATION_MEMBERS: [&str; 6] = [
    "Qev2IdcApplicationArn",
    "Qev2IdcApplicationName",
    "IdcInstanceArn",
    "IdcManagedApplicationArn",
    "IdcOnboardStatus",
    "IdcDisplayName",
];

/// One row of the create table: what the request carries.
struct CreateCase {
    case: &'static str,
    name: String,
    display_name: String,
    instance_arn: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// `CreateQev2IdcApplication` with the three required members plus `tags`.
async fn create_application(
    emulator: &Emulator,
    name: &str,
    display_name: &str,
    instance_arn: &str,
    tags: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("IdcInstanceArn".to_string(), instance_arn.to_string()),
        ("Qev2IdcApplicationName".to_string(), name.to_string()),
        ("IdcDisplayName".to_string(), display_name.to_string()),
    ];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("CreateQev2IdcApplication", &borrowed)
        .await
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Every compared member of the single application a body carries.
fn application_members(body: &str) -> Vec<(&'static str, Option<String>)> {
    APPLICATION_MEMBERS
        .into_iter()
        .map(|member| (member, all_text(body, member).into_iter().next()))
        .collect()
}

/// The `<Qev2IdcApplicationName>` of every application a body carries, in the
/// order the page lists them.
fn listed_applications(body: &str) -> Vec<String> {
    all_text(body, "Qev2IdcApplicationName")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// A name of `len` characters that satisfies `Qev2IdcApplicationName`'s pattern
/// (`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
fn name_of_length(len: usize) -> String {
    std::iter::once('a')
        .chain(std::iter::repeat_n('b', len - 1))
        .collect()
}

/// A display name of `len` characters that satisfies `IdcDisplayNameString`'s
/// pattern (`^[\w+=,.@-]+$`).
fn display_name_of_length(len: usize) -> String {
    std::iter::repeat_n('x', len).collect()
}

/// The table every create case walks: rows differ along what the rule
/// generalises over — the length of the name (both modelled bounds) and of the
/// display name, the punctuation each pattern admits, the number of tags, and
/// the form of the IAM Identity Center instance ARN.
///
/// Bounds source: `smithy_query(query="operation",
/// operation="CreateQev2IdcApplication")` — `Qev2IdcApplicationName` len 1..63
/// pattern `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`, `IdcDisplayName` len 1..127 pattern
/// `^[\w+=,.@-]+$`.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "the three required members alone",
            name: "qev2-app".to_string(),
            display_name: "QueryEditor".to_string(),
            instance_arn: IDC_INSTANCE_ARN,
            tags: Vec::new(),
        },
        CreateCase {
            case: "the shortest name and display name",
            name: name_of_length(1),
            display_name: display_name_of_length(1),
            instance_arn: IDC_INSTANCE_ARN,
            tags: Vec::new(),
        },
        CreateCase {
            case: "the longest name and display name",
            name: name_of_length(63),
            display_name: display_name_of_length(127),
            instance_arn: IDC_INSTANCE_ARN,
            tags: Vec::new(),
        },
        CreateCase {
            case: "a hyphenated name and every punctuation the display name admits",
            name: "my-qev2-app-1".to_string(),
            display_name: "Team+Editor=1,v.2@aws-x".to_string(),
            instance_arn: IDC_INSTANCE_ARN,
            tags: Vec::new(),
        },
        CreateCase {
            case: "one tag",
            name: "tagged-once".to_string(),
            display_name: "TaggedOnce".to_string(),
            instance_arn: IDC_INSTANCE_ARN,
            tags: vec![("owner", "alice")],
        },
        CreateCase {
            case: "three tags",
            name: "tagged-thrice".to_string(),
            display_name: "TaggedThrice".to_string(),
            instance_arn: IDC_INSTANCE_ARN,
            tags: vec![("owner", "alice"), ("env", "prod"), ("cost-center", "42")],
        },
        CreateCase {
            case: "another instance the account holds",
            name: "second-instance-app".to_string(),
            display_name: "SecondInstance".to_string(),
            instance_arn: OTHER_IDC_INSTANCE_ARN,
            tags: Vec::new(),
        },
    ]
}

/// Each create stores the application exactly as the request configured it,
/// mints an ARN of its own that a later read reaches it by, and reports the IAM
/// Identity Center managed application of the instance the request named.
#[tokio::test]
async fn every_created_application_is_stored_exactly_as_the_request_configured_it() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for row in create_cases() {
        let CreateCase {
            case,
            name,
            display_name,
            instance_arn,
            tags,
        } = row;
        let created =
            create_application(&emulator, &name, &display_name, instance_arn, &tags).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );

        assert_eq!(
            created.text("Qev2IdcApplicationName").as_deref(),
            Some(name.as_str()),
            "case {case}: expected the requested name, got {:?}",
            created.text("Qev2IdcApplicationName")
        );
        assert_eq!(
            created.text("IdcDisplayName").as_deref(),
            Some(display_name.as_str()),
            "case {case}: expected the requested display name, got {:?}",
            created.text("IdcDisplayName")
        );
        assert_eq!(
            created.text("IdcInstanceArn").as_deref(),
            Some(instance_arn),
            "case {case}: expected the requested instance ARN, got {:?}",
            created.text("IdcInstanceArn")
        );
        assert_eq!(
            created.text("IdcOnboardStatus").as_deref(),
            Some(state::QEV2_IDC_ONBOARD_STATUS),
            "case {case}: expected the registered onboard status, got {:?}",
            created.text("IdcOnboardStatus")
        );

        let arn = created
            .text("Qev2IdcApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries an ARN"));
        // The ARN this emulator mints for a Redshift resource of the Region the
        // shared harness signs every request for, with the canonical emulator
        // account.
        let expected_prefix =
            engine::emulator_arn("redshift", "us-west-2", "qev2idcapplication:");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected an ARN of its own, got the already-minted {arn:?}"
        );

        let managed = created
            .text("IdcManagedApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries a managed ARN"));
        let instance_id = instance_arn
            .split('/')
            .next_back()
            .expect("the case's instance ARN carries an id");
        assert!(
            managed.starts_with("arn:aws:sso::")
                && managed.contains(&format!("application/{instance_id}/apl-")),
            "case {case}: expected the managed application of instance {instance_id:?}, got {managed:?}"
        );

        // The round trip: the application the create answered is exactly what a
        // later read of that ARN reports, tags included.
        let described = emulator
            .call(
                "DescribeQev2IdcApplications",
                &[("Qev2IdcApplicationArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the created application readable, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            listed_applications(&described.body),
            vec![name.clone()],
            "case {case}: expected exactly the created application, got {:?}",
            listed_applications(&described.body)
        );
        assert_eq!(
            application_members(&described.body),
            application_members(&created.body),
            "case {case}: expected the read to report the created record, got {:?}",
            application_members(&described.body)
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&described.body),
            expected_tags,
            "case {case}: expected the requested tags, got {:?}",
            listed_tags(&described.body)
        );
    }
}

/// An application NAME is what one account's applications are unique on — "use a
/// different application name or describe existing applications to find the
/// ARN" — and a delete frees the name for a new application.
#[tokio::test]
async fn an_application_name_is_unique_and_a_delete_frees_it() {
    let emulator = Emulator::new();

    // The rows differ along the spelling and the length of the name a second
    // create re-uses, and each is followed by a name one character off it, which
    // must be admitted: the refusal is keyed on the whole name, not a prefix.
    for (case, name) in [
        ("a plain name", "unique-app".to_string()),
        ("the shortest name", name_of_length(1)),
        ("the longest name", name_of_length(63)),
        ("a name of digits and hyphens", "app-2-0".to_string()),
    ] {
        let first = create_application(&emulator, &name, "First", IDC_INSTANCE_ARN, &[]).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected the first create served with 200, got {} with body {}",
            first.status, first.body
        );
        let arn = first
            .text("Qev2IdcApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries an ARN"));

        // A second create of the SAME name registers nothing, whatever else it
        // carries: a different display name does not make it a new application.
        let again = create_application(&emulator, &name, "Second", IDC_INSTANCE_ARN, &[]).await;
        assert_refused(case, &again, 400, "Qev2IdcApplicationAlreadyExists");
        let described = emulator
            .call(
                "DescribeQev2IdcApplications",
                &[("Qev2IdcApplicationArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            described.text("IdcDisplayName").as_deref(),
            Some("First"),
            "case {case}: expected the refused create to change nothing, got {:?}",
            described.text("IdcDisplayName")
        );

        // A name one character longer is a different application. The longest
        // row is already at the modelled maximum, so its neighbour is built to
        // stay inside the bound rather than past it.
        let neighbour = if name.len() + 2 > 63 {
            format!("{}-x", &name[..61])
        } else {
            format!("{name}-x")
        };
        let other =
            create_application(&emulator, &neighbour, "Neighbour", IDC_INSTANCE_ARN, &[]).await;
        assert_eq!(
            other.status, 200,
            "case {case}: expected the name {neighbour:?} admitted, got {} with body {}",
            other.status, other.body
        );

        // The delete frees the name: the same create now succeeds again.
        let deleted = emulator
            .call(
                "DeleteQev2IdcApplication",
                &[("Qev2IdcApplicationArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete served with 200, got {} with body {}",
            deleted.status, deleted.body
        );
        let recreated =
            create_application(&emulator, &name, "Third", IDC_INSTANCE_ARN, &[]).await;
        assert_eq!(
            recreated.status, 200,
            "case {case}: expected the freed name re-usable, got {} with body {}",
            recreated.status, recreated.body
        );
    }
}

/// An ARN that names no application is refused with the modelled 404 fault by
/// every operation that takes one, and an ARN that names one is served. Every
/// such refusal carries the fault's one fixed sentence, whatever ARN the
/// request supplied.
#[tokio::test]
async fn an_arn_naming_no_application_is_refused_by_every_operation_that_takes_one() {
    let emulator = Emulator::new();
    let created = create_application(&emulator, "present-app", "Present", IDC_INSTANCE_ARN, &[])
        .await;
    assert_eq!(
        created.status, 200,
        "case seed present-app: expected 200, got {} with body {}",
        created.status, created.body
    );
    let present = created
        .text("Qev2IdcApplicationArn")
        .expect("case seed present-app: a created application carries an ARN");

    // The rows differ along the form of the ARN that resolves to nothing: a
    // well-formed ARN of this very resource type, one of another type, one of
    // another Region, a stored ARN one character off, and a value that is no ARN
    // at all. The last row is the stored ARN, which must be served.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "an unknown application of this resource type",
            engine::emulator_arn(
                "redshift",
                "us-west-2",
                "qev2idcapplication:00000000-0000-0000-0000-000000000000",
            ),
            false,
        ),
        (
            "an ARN of another Redshift resource type",
            engine::emulator_arn("redshift", "us-west-2", "cluster:present-app"),
            false,
        ),
        (
            "the stored ARN in another Region",
            present.replace("us-west-2", "eu-west-1"),
            false,
        ),
        (
            "the stored ARN one character off",
            format!("{present}0"),
            false,
        ),
        ("a value that is no ARN at all", "present-app".to_string(), false),
        ("the stored ARN", present.clone(), true),
    ];

    for (case, arn, admitted) in rows {
        for action in [
            "DescribeQev2IdcApplications",
            "ModifyQev2IdcApplication",
            "DeleteQev2IdcApplication",
        ] {
            // A fresh store per row and action, so one row's delete cannot be
            // what the next row observes. The seeded application's ARN is the
            // same in every store: it is derived from the name and the Region,
            // not drawn per run.
            let emulator = Emulator::new();
            let seeded =
                create_application(&emulator, "present-app", "Present", IDC_INSTANCE_ARN, &[])
                    .await;
            assert_eq!(
                seeded.text("Qev2IdcApplicationArn").as_deref(),
                Some(present.as_str()),
                "case {case}: expected the ARN derived from the name and Region, got {:?}",
                seeded.text("Qev2IdcApplicationArn")
            );

            let answer = emulator
                .call(action, &[("Qev2IdcApplicationArn", arn.as_str())])
                .await;
            let case = format!("{case} on {action}");
            if admitted {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with body {}",
                    answer.status, answer.body
                );
            } else {
                assert_refused(&case, &answer, 404, "Qev2IdcApplicationNotExists");
                // The refusal carries ONE fixed sentence, stating the whole
                // precondition the call failed: existence for the read and the
                // delete, existence AND finished onboarding for the modify. The
                // wording is the same for every form of ARN and carries nothing
                // the request supplied, so a refusal reflects no caller value
                // back.
                let expected = expected_refusal(action);
                assert_eq!(
                    answer.text("Message").as_deref(),
                    Some(expected),
                    "case {case}: expected the fault's fixed wording {expected:?}, got {:?}",
                    answer.text("Message")
                );
                assert!(
                    !answer
                        .text("Message")
                        .is_some_and(|message| message.contains(arn.as_str())),
                    "case {case}: expected the refusal to carry none of the requested ARN {arn:?}, got {:?}",
                    answer.text("Message")
                );
            }
        }
    }
}

/// The sentence the `Qev2IdcApplicationNotExists` fault carries, which states
/// the precondition the operation enforces: existence for the read and the
/// delete, existence and finished onboarding for the modify.
///
/// Spelt out as a literal here rather than read from the crate's own constants,
/// so an edit to a constant changes the wording ON THE WIRE and this suite
/// fails; the constants the handlers emit are tied to these literals below.
fn expected_refusal(action: &str) -> &'static str {
    if action == "ModifyQev2IdcApplication" {
        "The Qev2IdcApplication you specified doesn't exist or the status isn't Completed."
    } else {
        "The Qev2IdcApplication you specified doesn't exist."
    }
}

/// The wording the crate serves is the wording this suite pins, so neither can
/// drift without the other: every refusal row compares the wire `Message`
/// against [`expected_refusal`]'s literals, and this case ties the constants the
/// handlers emit to the same text.
#[test]
fn the_refusal_wording_the_handlers_emit_is_the_pinned_text() {
    for (case, action, constant) in [
        (
            "the read's and the delete's wording",
            "DescribeQev2IdcApplications",
            state::QEV2_IDC_APPLICATION_NOT_EXISTS,
        ),
        (
            "the modify's wording",
            "ModifyQev2IdcApplication",
            state::QEV2_IDC_APPLICATION_NOT_MODIFIABLE,
        ),
    ] {
        assert_eq!(
            constant,
            expected_refusal(action),
            "case {case}: expected {:?}, got {constant:?}",
            expected_refusal(action)
        );
    }
}

/// An application whose onboarding has NOT finished is reported by the read and
/// deleted by the delete, but refused by the modify: a modify changes the IAM
/// Identity Center managed application that onboarding mints, so its
/// precondition is wider than the other two.
///
/// Every status but [`state::QEV2_IDC_ONBOARD_STATUS`] is a row here. No
/// implemented operation mints one — a create's application is onboarded by the
/// time it returns — so the record is seeded straight into the same shared store
/// the handlers read, exactly as the harness seeds an automated snapshot.
#[tokio::test]
async fn an_application_that_has_not_finished_onboarding_is_read_and_deleted_but_not_modified() {
    // The rows differ along the onboarding status that is not the finished one:
    // a status the service could report while onboarding, one spelt in another
    // case, and an empty one.
    for status in ["InProgress", "Failed", "completed", ""] {
        for action in ["ModifyQev2IdcApplication", "DeleteQev2IdcApplication"] {
            let emulator = Emulator::new();
            let arn = state::qev2_idc_application_arn("onboarding-app", "us-west-2");
            {
                let store = state::service_state(&*emulator.engine).expect("the Redshift store");
                let mut store = store.lock();
                store.qev2_idc_applications.push(Qev2IdcApplication {
                    qev2_idc_application_arn: Some(arn.clone()),
                    qev2_idc_application_name: Some("onboarding-app".to_string()),
                    idc_instance_arn: Some(IDC_INSTANCE_ARN.to_string()),
                    idc_managed_application_arn: Some(state::idc_managed_application_arn(
                        IDC_INSTANCE_ARN,
                        "onboarding-app",
                    )),
                    idc_display_name: Some("Onboarding".to_string()),
                    idc_onboard_status: Some(status.to_string()),
                    tags: Some(Vec::new()),
                });
            }
            let case = format!("status {status:?} on {action}");

            // The read reports it, status and all: a read's only precondition is
            // that the application exists.
            let described = emulator
                .call(
                    "DescribeQev2IdcApplications",
                    &[("Qev2IdcApplicationArn", arn.as_str())],
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected the read to report it with 200, got {} with body {}",
                described.status, described.body
            );
            assert_eq!(
                described.text("IdcOnboardStatus").as_deref(),
                Some(status),
                "case {case}: expected the stored onboarding status, got {:?}",
                described.text("IdcOnboardStatus")
            );

            // The modify is refused and changes nothing; the delete is served
            // and makes the application absent from every later read.
            if action == "ModifyQev2IdcApplication" {
                let answer = emulator
                    .call(
                        action,
                        &[
                            ("Qev2IdcApplicationArn", arn.as_str()),
                            ("IdcDisplayName", "Renamed"),
                        ],
                    )
                    .await;
                assert_refused(&case, &answer, 404, "Qev2IdcApplicationNotExists");
                assert_eq!(
                    answer.text("Message").as_deref(),
                    Some(expected_refusal(action)),
                    "case {case}: expected the modify's fixed wording {:?}, got {:?}",
                    expected_refusal(action),
                    answer.text("Message")
                );

                let after = emulator
                    .call(
                        "DescribeQev2IdcApplications",
                        &[("Qev2IdcApplicationArn", arn.as_str())],
                    )
                    .await;
                assert_eq!(
                    application_members(&after.body),
                    application_members(&described.body),
                    "case {case}: expected the refused modify to change nothing, got {:?}",
                    application_members(&after.body)
                );
            } else {
                let answer = emulator
                    .call(action, &[("Qev2IdcApplicationArn", arn.as_str())])
                    .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the delete to be served with 200, got {} with body {}",
                    answer.status, answer.body
                );

                let after = emulator
                    .call(
                        "DescribeQev2IdcApplications",
                        &[("Qev2IdcApplicationArn", arn.as_str())],
                    )
                    .await;
                assert_refused(&case, &after, 404, "Qev2IdcApplicationNotExists");
            }
        }
    }
}

/// `ModifyQev2IdcApplication` changes the one member it takes and NOTHING else,
/// and a modify that names no display name changes nothing at all.
#[tokio::test]
async fn a_modify_changes_only_the_display_name() {
    let emulator = Emulator::new();
    let created = create_application(
        &emulator,
        "modified-app",
        "Original",
        IDC_INSTANCE_ARN,
        &[("owner", "alice")],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed modified-app: expected 200, got {} with body {}",
        created.status, created.body
    );
    let arn = created
        .text("Qev2IdcApplicationArn")
        .expect("case seed modified-app: a created application carries an ARN");

    // The rows differ along the length of the new display name (both modelled
    // bounds) and the punctuation its pattern admits.
    let rows: Vec<(&str, Option<String>)> = vec![
        ("a plain display name", Some("Renamed".to_string())),
        ("the shortest display name", Some(display_name_of_length(1))),
        ("the longest display name", Some(display_name_of_length(127))),
        (
            "every punctuation the pattern admits",
            Some("Team+Editor=1,v.2@aws-x".to_string()),
        ),
        ("no display name at all", None),
    ];

    let mut expected_display = "Original".to_string();
    for (case, display_name) in rows {
        let mut params: Vec<(&str, &str)> = vec![("Qev2IdcApplicationArn", arn.as_str())];
        if let Some(display_name) = display_name.as_deref() {
            params.push(("IdcDisplayName", display_name));
            expected_display = display_name.to_string();
        }
        let modified = emulator.call("ModifyQev2IdcApplication", &params).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: expected 200, got {} with body {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.text("IdcDisplayName").as_deref(),
            Some(expected_display.as_str()),
            "case {case}: expected the display name {expected_display:?}, got {:?}",
            modified.text("IdcDisplayName")
        );

        // Everything else is the record the create left: the identity members,
        // the onboard status and the tags.
        let described = emulator
            .call(
                "DescribeQev2IdcApplications",
                &[("Qev2IdcApplicationArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            application_members(&described.body),
            application_members(&modified.body),
            "case {case}: expected the read to report the modified record, got {:?}",
            application_members(&described.body)
        );
        for member in ["Qev2IdcApplicationArn", "Qev2IdcApplicationName", "IdcInstanceArn", "IdcManagedApplicationArn", "IdcOnboardStatus"] {
            assert_eq!(
                described.text(member),
                created.text(member),
                "case {case}: expected {member} unchanged at {:?}, got {:?}",
                created.text(member),
                described.text(member)
            );
        }
        assert_eq!(
            listed_tags(&described.body),
            vec![("owner".to_string(), "alice".to_string())],
            "case {case}: expected the tags unchanged, got {:?}",
            listed_tags(&described.body)
        );
    }
}

/// `DeleteQev2IdcApplication` makes the application absent from every later
/// read, leaves every other application where it was, and a second delete of the
/// same ARN names nothing.
#[tokio::test]
async fn a_delete_makes_the_application_absent_from_every_later_read() {
    let emulator = Emulator::new();
    let mut arns: Vec<(String, String)> = Vec::new();
    for name in ["first-app", "second-app", "third-app"] {
        let created = create_application(&emulator, name, "Listed", IDC_INSTANCE_ARN, &[]).await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} with body {}",
            created.status, created.body
        );
        arns.push((
            name.to_string(),
            created
                .text("Qev2IdcApplicationArn")
                .unwrap_or_else(|| panic!("case seed {name}: a created application carries an ARN")),
        ));
    }

    // Delete the MIDDLE record, so the case cannot pass by truncating the
    // collection.
    let (deleted_name, deleted_arn) = arns[1].clone();
    let deleted = emulator
        .call(
            "DeleteQev2IdcApplication",
            &[("Qev2IdcApplicationArn", deleted_arn.as_str())],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete {deleted_name}: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    let listed = emulator.call("DescribeQev2IdcApplications", &[]).await;
    assert_eq!(
        listed_applications(&listed.body),
        vec!["first-app".to_string(), "third-app".to_string()],
        "case after the delete: expected the surviving applications in creation order, got {:?}",
        listed_applications(&listed.body)
    );

    // The deleted ARN now names nothing, for a read and for a second delete.
    for action in [
        "DescribeQev2IdcApplications",
        "ModifyQev2IdcApplication",
        "DeleteQev2IdcApplication",
    ] {
        let answer = emulator
            .call(action, &[("Qev2IdcApplicationArn", deleted_arn.as_str())])
            .await;
        assert_refused(
            &format!("the deleted ARN on {action}"),
            &answer,
            404,
            "Qev2IdcApplicationNotExists",
        );
    }
}

/// `DescribeQev2IdcApplications` hands out one page of the service's documented
/// `MaxRecords` window at a time, with a `Marker` cursor a later call resumes
/// the same sequence from.
///
/// The window is the one every paginated `Describe*` of this service shares
/// (`state::MAX_RECORDS_MIN` ..= `state::MAX_RECORDS_MAX`, applied by
/// `state::page_bounds`), tested on each bound and one past it.
#[tokio::test]
async fn applications_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();
    // One application more than the smallest page the window admits, so a page
    // of that size leaves a cursor.
    let total = state::MAX_RECORDS_MIN as usize + 1;
    for index in 0..total {
        let name = format!("paged-app-{index}");
        let created = create_application(&emulator, &name, "Paged", IDC_INSTANCE_ARN, &[]).await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    let minimum = state::MAX_RECORDS_MIN;
    let maximum = state::MAX_RECORDS_MAX;
    for (case, max_records, accepted) in [
        ("one below the window", (minimum - 1).to_string(), false),
        ("at the window's minimum", minimum.to_string(), true),
        ("inside the window", ((minimum + maximum) / 2).to_string(), true),
        ("at the window's maximum", maximum.to_string(), true),
        ("one past the window, which is served", (maximum + 1).to_string(), true),
        ("zero", "0".to_string(), false),
    ] {
        let answer = emulator
            .call(
                "DescribeQev2IdcApplications",
                &[("MaxRecords", max_records.as_str())],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // The default window takes every application in one page, with no cursor
    // left.
    let unpaged = emulator.call("DescribeQev2IdcApplications", &[]).await;
    assert_eq!(
        listed_applications(&unpaged.body).len(),
        total,
        "case the default window: expected all {total} applications in one page, got {:?}",
        listed_applications(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page at the window's minimum leaves a cursor the next call resumes from,
    // and the two pages are the unpaged sequence split in order.
    let page_size = minimum.to_string();
    let first_page = emulator
        .call(
            "DescribeQev2IdcApplications",
            &[("MaxRecords", page_size.as_str())],
        )
        .await;
    let first_names = listed_applications(&first_page.body);
    assert_eq!(
        first_names.len(),
        minimum as usize,
        "case the first page: expected {minimum} applications, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page: records remain, so the page carries a cursor");
    let second_page = emulator
        .call(
            "DescribeQev2IdcApplications",
            &[
                ("MaxRecords", page_size.as_str()),
                ("Marker", marker.as_str()),
            ],
        )
        .await;
    let second_names = listed_applications(&second_page.body);
    assert_eq!(
        second_names.len(),
        total - minimum as usize,
        "case the second page: expected the remaining {} applications, got {second_names:?}",
        total - minimum as usize
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked,
        listed_applications(&unpaged.body),
        "case the walked pages: expected the unpaged sequence, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    for (case, cursor) in [
        ("not a cursor at all", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeQev2IdcApplications", &[("Marker", cursor)])
            .await;
        assert_refused(case, &bogus, 400, "InvalidParameterValue");
    }
}

/// One row of the instance-ARN table: the value the create names its instance by,
/// and the answer it earns — `code` is the error code it is refused with, or
/// `None` when it is served.
struct InstanceCase {
    case: &'static str,
    instance_arn: String,
    status: u16,
    code: Option<&'static str>,
}

impl InstanceCase {
    /// A row refused for the FORM of its value, which is this service's own
    /// `InvalidParameterValue`.
    fn refused(case: &'static str, instance_arn: String) -> Self {
        Self {
            case,
            instance_arn,
            status: 400,
            code: Some("InvalidParameterValue"),
        }
    }
}

/// `IdcInstanceArn` names an IAM Identity Center instance, so a value that is
/// not an ARN at all — or is longer than an ARN can be — is refused and nothing
/// is stored.
#[tokio::test]
async fn an_idc_instance_arn_that_is_not_an_arn_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // An instance ARN of exactly the published ARN length bound, and one
    // character past it (`state::MAX_ARN_LENGTH`).
    let prefix = "arn:aws:sso:::instance/";
    let at_bound = format!(
        "{prefix}{}",
        "a".repeat(state::MAX_ARN_LENGTH - prefix.len())
    );
    let past_bound = format!(
        "{prefix}{}",
        "a".repeat(state::MAX_ARN_LENGTH + 1 - prefix.len())
    );

    // The rows differ along the form and the size of the value: empty (the
    // modelled minimum length is 0, so the boundary validator admits it), a bare
    // word, an ARN missing its resource portion, an ARN missing a partition, an
    // ARN at the length bound and one past it, and a well-formed instance ARN of
    // an instance the account holds, which must be admitted. Each row carries the
    // refusal it earns, or `None` when it is served: a value that is no ARN at all
    // is refused for its FORM, while an ARN the form rule admits that names no
    // instance of the account passes that rule and is then denied by the IAM
    // Identity Center registration the create performs (the rule
    // [`crate::identity_center_dependency`] is about).
    let rows: Vec<InstanceCase> = vec![
        InstanceCase::refused("an empty instance ARN", String::new()),
        InstanceCase::refused("a bare word", "ssoins-1111111111111111".to_string()),
        InstanceCase::refused("an ARN with too few segments", "arn:aws:sso".to_string()),
        InstanceCase::refused(
            "an ARN with an empty resource",
            "arn:aws:sso::::".to_string(),
        ),
        InstanceCase::refused(
            "an ARN with no partition",
            "arn::sso:::instance/ssoins-1".to_string(),
        ),
        InstanceCase {
            case: "an ARN at the length bound",
            instance_arn: at_bound,
            status: 403,
            code: Some("DependentServiceAccessDenied"),
        },
        InstanceCase::refused("an ARN one character past the length bound", past_bound),
        InstanceCase {
            case: "a well-formed instance ARN of an instance the account holds",
            instance_arn: IDC_INSTANCE_ARN.to_string(),
            status: 200,
            code: None,
        },
    ];

    let mut admitted_names: Vec<String> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let InstanceCase {
            case,
            instance_arn,
            status,
            code,
        } = row;
        let name = format!("instance-case-{index}");
        let answer = create_application(&emulator, &name, "Instance", &instance_arn, &[]).await;
        match code {
            None => {
                assert_eq!(
                    answer.status, status,
                    "case {case}: expected {status}, got {} with body {}",
                    answer.status, answer.body
                );
                admitted_names.push(name);
            }
            Some(code) => assert_refused(case, &answer, status, code),
        }
    }

    // Only the admitted rows are stored: the refused creates registered nothing.
    let listed = emulator.call("DescribeQev2IdcApplications", &[]).await;
    assert_eq!(
        listed_applications(&listed.body),
        admitted_names,
        "case after the refusals: expected only the admitted applications, got {:?}",
        listed_applications(&listed.body)
    );
}

/// The documented tag limit and the store's own bound on the collection: a
/// create past either is refused with the modelled fault and registers nothing,
/// and a delete makes room again.
///
/// Tag bound source: `smithy_query(query="doc", operation="CreateTags")` — "A
/// resource can have up to 50 tags" (`state::MAX_TAGS_PER_RESOURCE`). The store
/// bound is this emulator's own (`state::QEV2_IDC_APPLICATION_QUOTA`).
#[tokio::test]
async fn a_create_past_the_tag_limit_or_the_store_bound_is_refused() {
    let emulator = Emulator::new();

    // The rows differ along the number of tags: none, one, the documented bound
    // and one past it.
    let mut tagged_names: Vec<String> = Vec::new();
    for (case, count, admitted) in [
        ("no tags", 0usize, true),
        ("one tag", 1, true),
        ("at the documented tag limit", state::MAX_TAGS_PER_RESOURCE, true),
        (
            "one tag past the documented limit",
            state::MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
    ] {
        let owned: Vec<(String, String)> = (0..count)
            .map(|index| (format!("key-{index}"), format!("value-{index}")))
            .collect();
        let tags: Vec<(&str, &str)> = owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let name = format!("tagged-with-{count}");
        let answer =
            create_application(&emulator, &name, "Tagged", IDC_INSTANCE_ARN, &tags).await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_tags(&answer.body).len(),
                count,
                "case {case}: expected {count} tags on the created application, got {:?}",
                listed_tags(&answer.body)
            );
            tagged_names.push(name);
        } else {
            assert_refused(case, &answer, 400, "TagLimitExceededFault");
        }
    }

    // The refused create registered nothing: only the admitted rows are stored.
    let listed = emulator.call("DescribeQev2IdcApplications", &[]).await;
    assert_eq!(
        listed_applications(&listed.body),
        tagged_names,
        "case after the tag rows: expected only the admitted applications, got {:?}",
        listed_applications(&listed.body)
    );

    // The store bound is thousands of records past what a suite can reasonably
    // create over the wire, and the bound is a property of the COLLECTION, so
    // the records it is filled with are seeded straight into the same shared
    // store the handlers read.
    let seeded_arn = {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        while store.qev2_idc_applications.len() < state::QEV2_IDC_APPLICATION_QUOTA {
            let index = store.qev2_idc_applications.len();
            store.qev2_idc_applications.push(Qev2IdcApplication {
                idc_instance_arn: Some(IDC_INSTANCE_ARN.to_string()),
                qev2_idc_application_name: Some(format!("seeded-{index:06}")),
                qev2_idc_application_arn: Some(state::qev2_idc_application_arn(
                    &format!("seeded-{index:06}"),
                    "us-west-2",
                )),
                idc_managed_application_arn: Some(state::idc_managed_application_arn(
                    IDC_INSTANCE_ARN,
                    &format!("seeded-{index:06}"),
                )),
                idc_onboard_status: Some(state::QEV2_IDC_ONBOARD_STATUS.to_string()),
                idc_display_name: Some("Seeded".to_string()),
                tags: Some(Vec::new()),
            });
        }
        store
            .qev2_idc_applications
            .last()
            .and_then(|application| application.qev2_idc_application_arn.clone())
            .expect("the seeded applications carry ARNs")
    };

    let full = create_application(
        &emulator,
        "one-too-many",
        "OneTooMany",
        IDC_INSTANCE_ARN,
        &[],
    )
    .await;
    assert_refused("one application past the store bound", &full, 400, "LimitExceededFault");

    // A delete makes room again: the same create now succeeds.
    let deleted = emulator
        .call(
            "DeleteQev2IdcApplication",
            &[("Qev2IdcApplicationArn", seeded_arn.as_str())],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case freeing one record: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let admitted = create_application(
        &emulator,
        "one-too-many",
        "OneTooMany",
        IDC_INSTANCE_ARN,
        &[],
    )
    .await;
    assert_eq!(
        admitted.status, 200,
        "case after freeing one record: expected 200, got {} with body {}",
        admitted.status, admitted.body
    );
}
