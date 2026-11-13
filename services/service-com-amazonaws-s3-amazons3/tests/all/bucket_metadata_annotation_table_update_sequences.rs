// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a SEQUENCE of
//! `UpdateBucketMetadataAnnotationTableConfiguration` calls leaves on the S3 Metadata
//! configuration a general purpose bucket carries, read back through
//! `GetBucketMetadataConfiguration` after every call.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns, with the metadata-configuration suite's own
//! helpers for the create, the read and for slicing a response), so what is pinned is the
//! wire behaviour — status, body, error code — rather than any handler's internals.
//!
//! Where the single-call table in [`super::bucket_metadata_annotation_table_update`] pins one
//! update of a configuration, this suite pins what holds over a SEQUENCE of them:
//!
//! * The operation is used "to enable or disable the annotation table, or to update its
//!   associated IAM role" (its own documentation), so each call REPLACES what the one before
//!   it left: after any sequence the configuration reports the `ConfigurationState` the LAST
//!   call named, however many calls preceded it and whichever states they named.
//! * A table stands behind that state exactly while it is `ENABLED` — which is why
//!   `AnnotationTableConfigurationResult` requires its `ConfigurationState` and leaves its
//!   `TableName`, `TableStatus` and `TableArn` optional — and the table is always the one
//!   S3 Metadata names a configuration's annotation table, in the general purpose bucket's
//!   own namespace, so a table re-enabled after a disable carries the same name and ARN as
//!   the one before it.
//! * A table that is there is in one of the states its "provisioning status" publishes
//!   (`AnnotationTableConfigurationResult$TableStatus`: `CREATING`, `BACKFILLING`, `ACTIVE`,
//!   `FAILED`), and one a call in the sequence created comes into service, so the read
//!   settles on `ACTIVE`.
//! * `Role` is "the new IAM role ARN to apply"
//!   (`AnnotationTableConfigurationUpdates$Role`) and is optional, so what the configuration
//!   reports after a sequence is the role the LAST call that named one named — including a
//!   call that only disabled the table — and never a role no call named.
//! * The update is scoped to the bucket it names and to that bucket's annotation table: the
//!   journal and inventory halves of the same configuration, and another bucket's
//!   configuration entirely, read back exactly as they did before the sequence.
//! * The one caller-supplied string a configuration retains is bounded by the ceiling the
//!   create that first records a role applies
//!   ([`service_com_amazonaws_s3_amazons3::state::MAX_METADATA_TABLE_ROLE_CHARS`]): a role
//!   exactly that long round-trips through the read, and one character past it is refused
//!   with HTTP 400 and records nothing.

use std::time::{Duration, Instant};

use service_com_amazonaws_s3_amazons3::state::MAX_METADATA_TABLE_ROLE_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, elements_of, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, table_arn, text,
    ANNOTATION_ROLE, ANNOTATION_TABLE, XMLNS,
};

/// A second IAM role ARN, of the same shape as [`ANNOTATION_ROLE`], so a step of a sequence
/// can re-point the annotation table at another role and the read-back is pinned to the role
/// the call named rather than to any role being reported at all.
const ANOTHER_ROLE: &str = "arn:aws:iam::000000000000:role/s3-metadata-annotations-second";

/// A third one, so a sequence can re-point the table twice and then back.
const A_THIRD_ROLE: &str = "arn:aws:iam::000000000000:role/s3-metadata-annotations-third";

/// The `TableStatus` a table whose creation is still under way is reported in, and the one it
/// is reported in once it is in service: two of the four values that member's "provisioning
/// status" publishes (`AnnotationTableConfigurationResult$TableStatus`: `CREATING`,
/// `BACKFILLING`, `ACTIVE`, `FAILED`) — spelled out here rather than read from the code under
/// test.
const CREATING: &str = "CREATING";
/// See [`CREATING`] — the status of a table that is in service.
const ACTIVE: &str = "ACTIVE";

/// How long a poll waits for every table of a configuration to come into service before it
/// gives up: generous, so a loaded or slow host still reaches the condition — what is waited
/// for is the CONDITION, read again after [`LOOK_AGAIN_AFTER`], never a fixed span.
const TABLES_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two reads of the configuration.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(20);

/// `PUT /<bucket>?metadataAnnotationTable` —
/// `UpdateBucketMetadataAnnotationTableConfiguration`, with `payload` as the
/// `AnnotationTableConfiguration` container.
async fn update_annotation_table(
    router: &MultiServiceRouter,
    bucket: &str,
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?metadataAnnotationTable"),
        &[],
        payload,
    )
    .await
}

/// The `AnnotationTableConfigurationUpdates` document naming `state`, and the `role` the call
/// applies when it names one.
fn updates(state: &str, role: Option<&str>) -> String {
    let role = role.map_or(String::new(), |role| format!("<Role>{role}</Role>"));
    format!(
        "<AnnotationTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>{state}</ConfigurationState>\
         {role}\
         </AnnotationTableConfiguration>"
    )
}

/// The `MetadataConfiguration` a sequence starts from: a journal table retaining records for
/// a week, an enabled inventory table, and the annotation table configuration `annotation`
/// names (its state and the role the create named for it) — or none at all when the sequence
/// starts from a configuration naming none.
fn starting_configuration(annotation: Option<(&str, Option<&str>)>) -> String {
    let annotation = match annotation {
        None => String::new(),
        Some((state, role)) => {
            let role = role.map_or(String::new(), |role| format!("<Role>{role}</Role>"));
            format!(
                "<AnnotationTableConfiguration>\
                 <ConfigurationState>{state}</ConfigurationState>\
                 {role}\
                 </AnnotationTableConfiguration>"
            )
        }
    };
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days>\
         </RecordExpiration></JournalTableConfiguration>\
         <InventoryTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         </InventoryTableConfiguration>\
         {annotation}\
         </MetadataConfiguration>"
    )
}

/// The bucket's metadata configuration as the read reports it, once no table of it is still
/// being created — the state in which every table's reported status is settled, so a snapshot
/// taken here can be compared with a later one.
///
/// Looks again every [`LOOK_AGAIN_AFTER`] until the condition holds, giving up only after
/// [`TABLES_DEADLINE`]: nothing here waits a fixed span for a table to fall due.
async fn configuration_with_its_tables_in_service(
    router: &MultiServiceRouter,
    bucket: &str,
    case: &str,
) -> String {
    let deadline = Instant::now() + TABLES_DEADLINE;
    loop {
        let answer = get_metadata_configuration(router, bucket, &[]).await;
        let body = String::from_utf8_lossy(&answer.body).to_string();
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the read of {bucket:?} to answer 200, got {} with body \
             {body:?}",
            answer.status
        );
        let statuses = elements_of(&body, "TableStatus");
        if statuses.iter().all(|status| status == ACTIVE) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the tables of {bucket:?} must come into service, got statuses \
             {statuses:?}"
        );
        std::thread::sleep(LOOK_AGAIN_AFTER);
    }
}

/// What the read must report for the annotation table container after a step that named
/// `state` and left `role` in effect, with `body` the configuration the read answered.
fn assert_the_step_was_recorded(
    case: &str,
    step: usize,
    body: &str,
    state: &str,
    role: Option<&str>,
    namespace: &str,
) {
    let container = inside(body, "AnnotationTableConfigurationResult").unwrap_or_else(|| {
        panic!(
            "case {case:?} step {step}: an update names an annotation table configuration, so \
             the read must report one, got {body:?}"
        )
    });
    assert_eq!(
        text(container, "ConfigurationState").as_deref(),
        Some(state),
        "case {case:?} step {step}: expected ConfigurationState {state:?}, got {:?}",
        text(container, "ConfigurationState")
    );
    assert_eq!(
        text(container, "Role").as_deref(),
        role,
        "case {case:?} step {step}: expected Role {role:?}, got {:?}",
        text(container, "Role")
    );

    // The table stands behind the state exactly while it is enabled, under the name and the
    // ARN S3 Metadata gives a configuration's annotation table.
    let expected_name = (state == "ENABLED").then_some(ANNOTATION_TABLE);
    assert_eq!(
        text(container, "TableName").as_deref(),
        expected_name,
        "case {case:?} step {step}: expected TableName {expected_name:?}, got {:?}",
        text(container, "TableName")
    );
    let expected_arn = expected_name.map(|table| table_arn(namespace, table));
    assert_eq!(
        text(container, "TableArn"),
        expected_arn,
        "case {case:?} step {step}: expected TableArn {expected_arn:?}, got {:?}",
        text(container, "TableArn")
    );
    let status = text(container, "TableStatus");
    match expected_name {
        None => assert_eq!(
            status, None,
            "case {case:?} step {step}: a state with no table behind it reports no \
             TableStatus, got {status:?}"
        ),
        Some(_) => assert!(
            status.as_deref() == Some(CREATING) || status.as_deref() == Some(ACTIVE),
            "case {case:?} step {step}: a table of an enabled annotation table is being \
             created or in service, got {status:?}"
        ),
    }
}

/// That the journal and inventory halves of the configuration, and its destination, read back
/// exactly as they did before the sequence ran — each update is scoped to the annotation
/// table.
fn assert_the_rest_is_untouched(case: &str, before: &str, after: &str) {
    for container in [
        "JournalTableConfigurationResult",
        "InventoryTableConfigurationResult",
        "DestinationResult",
    ] {
        assert_eq!(
            inside(after, container),
            inside(before, container),
            "case {case:?}: the sequence must leave {container} exactly as it was, got {:?} \
             where the read before it reported {:?}",
            inside(after, container),
            inside(before, container)
        );
    }
}

/// One row: a bucket, the annotation table configuration its create named (or none at all),
/// and the sequence of updates driven against it — each step a `ConfigurationState` and the
/// `Role` that step names, when it names one.
#[derive(Debug, Clone)]
struct Sequence {
    /// The case name every assertion carries.
    name: &'static str,
    /// The general purpose bucket the configuration lives on.
    bucket: &'static str,
    /// The namespace that bucket's tables live in, as a table ARN's id carries it (the
    /// bucket's name written in the character set an S3 Tables name is written in) — spelled
    /// out, so what the id is made of is pinned by hand rather than by the code under test.
    namespace: &'static str,
    /// The annotation `ConfigurationState` the configuration was CREATED with and the role the
    /// create named for it, or `None` when the create named no annotation table configuration
    /// at all.
    start: Option<(&'static str, Option<&'static str>)>,
    /// The steps, in order.
    steps: &'static [(&'static str, Option<&'static str>)],
}

/// Every row: they differ along the length of the sequence (two to four calls), the order of
/// the states it names, whether a step repeats the state already in effect, whether a step
/// names a role and whether that role is the one in effect, whether the configuration it
/// starts from names an annotation table at all, and the FORM of the bucket's name the
/// namespace is folded from.
fn sequences() -> Vec<Sequence> {
    vec![
        Sequence {
            name: "enabled, disabled, then enabled again naming no role",
            bucket: "annotation-sequence-cycle",
            namespace: "b_annotation_sequence_cycle",
            start: None,
            steps: &[
                ("ENABLED", Some(ANNOTATION_ROLE)),
                ("DISABLED", None),
                ("ENABLED", None),
            ],
        },
        Sequence {
            name: "re-pointed twice and then back, staying enabled throughout",
            bucket: "annotation-sequence-roles",
            namespace: "b_annotation_sequence_roles",
            start: Some(("ENABLED", Some(ANNOTATION_ROLE))),
            steps: &[
                ("ENABLED", Some(ANOTHER_ROLE)),
                ("ENABLED", Some(A_THIRD_ROLE)),
                ("ENABLED", None),
                ("ENABLED", Some(ANNOTATION_ROLE)),
            ],
        },
        Sequence {
            name: "disabled twice over, the second call naming another role",
            bucket: "annotation.sequence.2026",
            namespace: "b_annotation_sequence_2026",
            start: Some(("ENABLED", Some(ANNOTATION_ROLE))),
            steps: &[("DISABLED", None), ("DISABLED", Some(ANOTHER_ROLE))],
        },
        Sequence {
            name: "enabled, disabled, enabled at another role, then disabled again",
            bucket: "annotation-sequence-long",
            namespace: "b_annotation_sequence_long",
            start: Some(("DISABLED", None)),
            steps: &[
                ("ENABLED", Some(ANOTHER_ROLE)),
                ("DISABLED", None),
                ("ENABLED", Some(ANNOTATION_ROLE)),
                ("DISABLED", None),
            ],
        },
    ]
}

/// The bucket whose configuration no update in this suite names, so what is pinned is that an
/// update reaches the bucket it addresses and no other.
const UNTOUCHED_BUCKET: &str = "annotation-sequence-untouched";

#[tokio::test]
async fn every_update_replaces_what_the_one_before_it_left() {
    let router = router();

    // A configuration no step of any sequence names, snapshotted once its tables are in
    // service so the comparison at the end is against a settled read.
    create_bucket(&router, UNTOUCHED_BUCKET, None, None).await;
    create_metadata_configuration(
        &router,
        UNTOUCHED_BUCKET,
        &[],
        &starting_configuration(Some(("ENABLED", Some(ANNOTATION_ROLE)))),
    )
    .await;
    let untouched_before =
        configuration_with_its_tables_in_service(&router, UNTOUCHED_BUCKET, "untouched bucket")
            .await;

    for sequence in sequences() {
        let case = sequence.name;
        create_bucket(&router, sequence.bucket, None, None).await;
        let created = create_metadata_configuration(
            &router,
            sequence.bucket,
            &[],
            &starting_configuration(sequence.start),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the configuration this row starts from must be created, got {} \
             with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The tables the CREATE asked for are in service before the sequence runs, so the
        // snapshot the scoping assertion compares against is settled.
        let before =
            configuration_with_its_tables_in_service(&router, sequence.bucket, case).await;

        // The role in effect walks with the sequence: the one the last call that named one
        // named, starting from the role the create named.
        let mut role_in_effect = sequence.start.and_then(|(_, role)| role);
        for (step, (state, role)) in sequence.steps.iter().enumerate() {
            let answer = update_annotation_table(&router, sequence.bucket, &updates(state, *role))
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?} step {step}: expected the modelled 200, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.body.is_empty(),
                "case {case:?} step {step}: the update models a Unit output, so its body must \
                 be empty, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
            role_in_effect = role.or(role_in_effect);

            let reading = get_metadata_configuration(&router, sequence.bucket, &[]).await;
            assert_eq!(
                reading.status,
                200,
                "case {case:?} step {step}: expected the read to answer 200, got {} with body \
                 {:?}",
                reading.status,
                String::from_utf8_lossy(&reading.body)
            );
            let after = String::from_utf8_lossy(&reading.body).to_string();
            assert_the_step_was_recorded(
                case,
                step,
                &after,
                state,
                role_in_effect,
                sequence.namespace,
            );
            assert_the_rest_is_untouched(case, &before, &after);
        }

        // A table the sequence left behind comes into service, and the read then reports the
        // final state with that table under it.
        let settled = configuration_with_its_tables_in_service(&router, sequence.bucket, case)
            .await;
        let (final_state, _) = sequence.steps.last().expect("every row names a step");
        assert_the_step_was_recorded(
            case,
            sequence.steps.len(),
            &settled,
            final_state,
            role_in_effect,
            sequence.namespace,
        );
        assert_the_rest_is_untouched(case, &before, &settled);
    }

    // No update named this bucket, so its configuration reads back exactly as it did.
    let untouched_after =
        configuration_with_its_tables_in_service(&router, UNTOUCHED_BUCKET, "untouched bucket")
            .await;
    assert_eq!(
        untouched_after, untouched_before,
        "an update must reach the bucket it addresses and no other: expected {untouched_before:?}, \
         got {untouched_after:?}"
    );
}

#[tokio::test]
async fn a_role_past_the_recorded_ceiling_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "annotation-sequence-role-length";
    create_bucket(&router, bucket, None, None).await;
    create_metadata_configuration(
        &router,
        bucket,
        &[],
        &starting_configuration(Some(("DISABLED", Some(ANNOTATION_ROLE)))),
    )
    .await;
    let before = configuration_with_its_tables_in_service(&router, bucket, "role length").await;

    // A role ARN of exactly the recorded ceiling, and one character past it: the same ARN
    // prefix with as many trailing characters in the role's path as each length needs.
    let prefix = "arn:aws:iam::000000000000:role/";
    let at_the_ceiling = format!(
        "{prefix}{}",
        "a".repeat(MAX_METADATA_TABLE_ROLE_CHARS - prefix.chars().count())
    );
    let past_the_ceiling = format!("{at_the_ceiling}a");

    let refused =
        update_annotation_table(&router, bucket, &updates("ENABLED", Some(&past_the_ceiling)))
            .await;
    assert_eq!(
        refused.status,
        400,
        "a role past the recorded ceiling must be refused, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    let unchanged = configuration_with_its_tables_in_service(&router, bucket, "role length").await;
    assert_eq!(
        unchanged, before,
        "a refused update must leave the stored configuration exactly as it was: expected \
         {before:?}, got {unchanged:?}"
    );

    // The same update naming a role of exactly the ceiling is accepted and round-trips, so
    // what is pinned is the bound and not the length of the string.
    let accepted =
        update_annotation_table(&router, bucket, &updates("ENABLED", Some(&at_the_ceiling))).await;
    assert_eq!(
        accepted.status,
        200,
        "a role of exactly the recorded ceiling must be accepted, got {} with body {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let reading = get_metadata_configuration(&router, bucket, &[]).await;
    let body = String::from_utf8_lossy(&reading.body).to_string();
    let container = inside(&body, "AnnotationTableConfigurationResult")
        .expect("the accepted update must leave an annotation table configuration behind");
    assert_eq!(
        text(container, "Role").as_deref(),
        Some(at_the_ceiling.as_str()),
        "the role the accepted update named must round-trip through the read, got {:?}",
        text(container, "Role")
    );
}
