// SPDX-License-Identifier: CC0-1.0

//! What `UpdateBucketMetadataAnnotationTableConfiguration` REFUSES, and what its refusals
//! say.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and metadata-configuration suites in this crate own), so what is pinned
//! is the wire behaviour — status, error code, `<Message>`, the members of the `<Error>`
//! document — and what the configuration reads back as afterwards.
//!
//! The two rules under test:
//!
//! * A table of an S3 Metadata configuration that DEPENDS on that configuration's journal
//!   table cannot be reconfigured while the journal table is still being created: there is
//!   no journal table yet for it to read from, so the update is refused with HTTP 400
//!   `InvalidRequest` and the fixed sentence of that error class, writes nothing, and the
//!   very same request is served once the journal table is in service. The annotation table
//!   is such a table — it belongs to the same configuration — and so is the inventory table,
//!   whose records "require a journal table with an `ACTIVE` status".
//! * A refusal of that update that is ABOUT the resource the request addressed — the bucket
//!   this emulator does not hold, or the metadata configuration the bucket does not carry —
//!   carries, beside the fixed `<Code>` and `<Message>` of its class, a `<BucketName>` copied
//!   from the request's `{Bucket}` path label. The refusal that reports the configuration's
//!   LIFECYCLE state instead is not about resolving a resource and names none.
//!
//! The rows differ along what the rules generalise over: which dependent table the update is
//! about and which change it asks for (a disable, an enable re-pointed at another role, an
//! enable naming no role at all), and — for the envelope — the refusal class and the FORM and
//! SIZE of the bucket name the request carries (short, hyphenated with digits, dotted, and
//! one at the ceiling a general purpose bucket name may be written to).
//!
//! The journal table's status is driven through the STORE rather than by waiting for the
//! emulator's activation span to pass: the instant a table falls due is recorded
//! ([`service_com_amazonaws_s3_amazons3::state::MetadataTable::active_at`]), so a row moves
//! that instant and the status every reader computes from it follows. Nothing here sleeps,
//! and no row's outcome depends on how fast the host runs.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_s3_amazons3::service::metadata_tables::JOURNAL_TABLE_BEING_CREATED_MESSAGE;
use service_com_amazonaws_s3_amazons3::state::{
    ComAmazonawsS3Amazons3State, INVALID_REQUEST_CODE, INVALID_REQUEST_STATUS,
    METADATA_CONFIGURATION_NOT_FOUND_CODE, METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
    METADATA_CONFIGURATION_NOT_FOUND_STATUS, NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, router_with_engine, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, text as element_text,
    ANNOTATION_ROLE, XMLNS,
};
use super::object_and_tagging_roundtrip::text;

/// A second IAM role ARN of the same shape as [`ANNOTATION_ROLE`], so a row can ask the
/// update to re-point the annotation table at another role.
const ANOTHER_ANNOTATION_ROLE: &str = "arn:aws:iam::000000000000:role/s3-metadata-annotations-2";

/// How far a row moves the journal table's due instant when it wants that table reported as
/// still being created: long enough that no row can outrun it.
const WELL_INTO_THE_FUTURE: Duration = Duration::from_secs(60 * 60);

/// The `MetadataConfiguration` every row of the journal-state table starts from: a journal
/// table with its record expiration off, and both optional tables enabled, so each update has
/// a table of the configuration to be about.
fn starting_configuration() -> String {
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>DISABLED</Expiration>\
         </RecordExpiration></JournalTableConfiguration>\
         <InventoryTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         </InventoryTableConfiguration>\
         <AnnotationTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         <Role>{ANNOTATION_ROLE}</Role>\
         </AnnotationTableConfiguration>\
         </MetadataConfiguration>"
    )
}

/// The `AnnotationTableConfigurationUpdates` document naming `state`, and the `Role` the call
/// applies when it names one.
fn annotation_updates(state: &str, role: Option<&str>) -> String {
    let role = role.map_or(String::new(), |role| format!("<Role>{role}</Role>"));
    format!(
        "<AnnotationTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>{state}</ConfigurationState>{role}\
         </AnnotationTableConfiguration>"
    )
}

/// The `InventoryTableConfigurationUpdates` document naming `state`.
fn inventory_updates(state: &str) -> String {
    format!(
        "<InventoryTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>{state}</ConfigurationState>\
         </InventoryTableConfiguration>"
    )
}

/// `PUT /<bucket>?<subresource>` — one of the metadata-table updates, with `payload` as its
/// container.
async fn update(
    router: &MultiServiceRouter,
    bucket: &str,
    subresource: &str,
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?{subresource}"),
        &[],
        payload,
    )
    .await
}

/// Move the due instant of the journal table of `bucket`'s metadata configuration to
/// `active_at`, through the same store the service reads.
///
/// Not over the wire, because no request moves it: the instant is recorded by the create that
/// asked for the table, and what every reader reports is the status it computes from that
/// instant at the time of the call. Seeding it is what lets a row observe the journal table as
/// being created, and then as in service, with nothing waiting for a span to pass.
fn given_the_journal_table_falls_due_at(
    engine: &Arc<engine::InMemoryEngine>,
    bucket: &str,
    active_at: SystemTime,
) {
    let state = engine.state::<ComAmazonawsS3Amazons3State>();
    let mut store = state.lock();
    let held = store
        .bucket_mut(bucket)
        .unwrap_or_else(|| panic!("the bucket {bucket:?} must be held before it is seeded"));
    let mut configuration = held
        .metadata_configuration()
        .cloned()
        .unwrap_or_else(|| panic!("the bucket {bucket:?} must carry a metadata configuration"));
    configuration.journal.active_at = active_at;
    held.set_metadata_configuration(configuration);
}

/// The `ConfigurationState` and `Role` the read `body` reports inside `container`.
fn reported_state(body: &str, container: &str) -> (Option<String>, Option<String>) {
    let inner = inside(body, container);
    (
        inner.and_then(|inner| element_text(inner, "ConfigurationState")),
        inner.and_then(|inner| element_text(inner, "Role")),
    )
}

/// One row of the journal-state table: which dependent table the update is about, the change
/// it asks for, and what the configuration reports for that table before and after.
struct DependentTableRow {
    case: &'static str,
    subresource: &'static str,
    payload: String,
    /// The container of the read that reports the table this row updates.
    container: &'static str,
    /// The `ConfigurationState` and `Role` that container reports while nothing has been
    /// written.
    untouched: (Option<String>, Option<String>),
    /// The same two, once the update has been served.
    updated: (Option<String>, Option<String>),
}

fn dependent_table_rows() -> Vec<DependentTableRow> {
    let annotation = "AnnotationTableConfigurationResult";
    let created_annotation = (
        Some("ENABLED".to_string()),
        Some(ANNOTATION_ROLE.to_string()),
    );
    vec![
        DependentTableRow {
            case: "the annotation table, disabled",
            subresource: "metadataAnnotationTable",
            payload: annotation_updates("DISABLED", None),
            container: annotation,
            untouched: created_annotation.clone(),
            // The update named no role, so the one the configuration carried is still the one
            // it reports.
            updated: (
                Some("DISABLED".to_string()),
                Some(ANNOTATION_ROLE.to_string()),
            ),
        },
        DependentTableRow {
            case: "the annotation table, re-pointed at another role",
            subresource: "metadataAnnotationTable",
            payload: annotation_updates("ENABLED", Some(ANOTHER_ANNOTATION_ROLE)),
            container: annotation,
            untouched: created_annotation.clone(),
            updated: (
                Some("ENABLED".to_string()),
                Some(ANOTHER_ANNOTATION_ROLE.to_string()),
            ),
        },
        DependentTableRow {
            case: "the annotation table, enabled again naming no role",
            subresource: "metadataAnnotationTable",
            payload: annotation_updates("ENABLED", None),
            container: annotation,
            untouched: created_annotation.clone(),
            updated: created_annotation.clone(),
        },
        DependentTableRow {
            case: "the inventory table, the sibling reading from the same journal table",
            subresource: "metadataInventoryTable",
            payload: inventory_updates("DISABLED"),
            container: "InventoryTableConfigurationResult",
            untouched: (Some("ENABLED".to_string()), None),
            updated: (Some("DISABLED".to_string()), None),
        },
    ]
}

#[tokio::test]
async fn an_update_of_a_dependent_table_waits_for_the_journal_table() {
    let (router, engine) = router_with_engine();

    for (index, row) in dependent_table_rows().iter().enumerate() {
        let DependentTableRow {
            case,
            subresource,
            payload,
            container,
            untouched,
            updated,
        } = row;
        // A bucket per row, so each row meets a configuration of its own.
        let bucket = format!("journal-pending-{index}");
        create_bucket(&router, &bucket, None, None).await;
        let created =
            create_metadata_configuration(&router, &bucket, &[], &starting_configuration()).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the configuration must be created first, got {} with body {:?}",
            created.status,
            text(&created)
        );

        // The journal table is still being created, whatever the host has got through in the
        // meantime.
        given_the_journal_table_falls_due_at(
            &engine,
            &bucket,
            SystemTime::now() + WELL_INTO_THE_FUTURE,
        );

        let refused = update(&router, &bucket, subresource, payload).await;
        assert_eq!(
            refused.status,
            INVALID_REQUEST_STATUS,
            "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS} while the journal table is \
             being created, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_REQUEST_CODE,
            "case {case:?}: expected the error code {INVALID_REQUEST_CODE:?}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            JOURNAL_TABLE_BEING_CREATED_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
        // This refusal reports the state the configuration's lifecycle is in rather than a
        // resource that could not be resolved, so it names no resource.
        assert_eq!(
            refused.element("BucketName"),
            None,
            "case {case:?}: a lifecycle-state refusal names no resource, got {:?} in {:?}",
            refused.element("BucketName"),
            text(&refused)
        );

        // Nothing was written: the table this row updates reports exactly what the create
        // left it reporting.
        let body = text(&get_metadata_configuration(&router, &bucket, &[]).await);
        assert_eq!(
            reported_state(&body, container),
            *untouched,
            "case {case:?}: a refused update must leave the table exactly as it was, got {body:?}"
        );

        // Once the journal table is in service the SAME request is served, so the refusal was
        // about that table's status and not about the payload.
        given_the_journal_table_falls_due_at(
            &engine,
            &bucket,
            SystemTime::now() - Duration::from_secs(1),
        );
        let served = update(&router, &bucket, subresource, payload).await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: the same update must be served once the journal table is in \
             service, got {} with body {:?}",
            served.status,
            text(&served)
        );
        assert!(
            served.body.is_empty(),
            "case {case:?}: the served update answers an empty body, got {:?}",
            text(&served)
        );

        let after = text(&get_metadata_configuration(&router, &bucket, &[]).await);
        assert_eq!(
            reported_state(&after, container),
            *updated,
            "case {case:?}: the served update must be what the read reports, got {after:?}"
        );
    }
}

/// How the bucket a row addresses is set up before the refused update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Never created, so the update is refused on the bucket itself.
    NotHeld,
    /// Created and carrying no metadata configuration, so the update has no annotation table
    /// to be about.
    WithoutAConfiguration,
}

/// One row of the envelope table: the FORM of the bucket name the request carries, in two
/// names of that form — one no create ever recorded, and one that is held and carries no
/// metadata configuration — so each refusal class is driven on a name of its own.
struct EnvelopeRow {
    case: &'static str,
    not_held: String,
    held: String,
}

impl EnvelopeRow {
    /// The name the request carries for `setup`.
    fn bucket(&self, setup: Setup) -> &str {
        match setup {
            Setup::NotHeld => &self.not_held,
            Setup::WithoutAConfiguration => &self.held,
        }
    }
}

fn envelope_rows() -> Vec<EnvelopeRow> {
    // "Bucket names must be between 3 (min) and 63 (max) characters long" (Amazon S3 User
    // Guide, "General purpose bucket naming rules"), so the longest row's names are of
    // exactly that many characters: a name a create records, which the envelope must carry
    // whole.
    let at_the_ceiling = |tail: &str| {
        let head = "annotation-envelope-";
        format!(
            "{head}{}{tail}",
            "c".repeat(63 - head.len() - tail.len())
        )
    };
    vec![
        EnvelopeRow {
            case: "a short name",
            not_held: "ann".to_string(),
            held: "anh".to_string(),
        },
        EnvelopeRow {
            case: "a hyphenated name carrying digits",
            not_held: "annotation-envelope-2026".to_string(),
            held: "annotation-envelope-2027".to_string(),
        },
        EnvelopeRow {
            case: "a dotted name",
            not_held: "annotation.envelope.absent".to_string(),
            held: "annotation.envelope.held".to_string(),
        },
        EnvelopeRow {
            case: "a name at the published 63-character ceiling",
            not_held: at_the_ceiling("-absent"),
            held: at_the_ceiling("-held"),
        },
    ]
}

#[tokio::test]
async fn a_resource_scoped_refusal_of_the_update_names_the_bucket() {
    let router = router();

    // A neighbour carrying a configuration throughout, so every refusal below is about the
    // bucket the request named rather than about an empty store.
    let neighbour = "annotation-envelope-neighbour";
    create_bucket(&router, neighbour, None, None).await;
    let configured =
        create_metadata_configuration(&router, neighbour, &[], &starting_configuration()).await;
    assert_eq!(
        configured.status,
        200,
        "the neighbour's configuration must be recorded, got {} with body {:?}",
        configured.status,
        text(&configured)
    );

    for row in envelope_rows() {
        let case = row.case;
        for setup in [Setup::NotHeld, Setup::WithoutAConfiguration] {
            let name = row.bucket(setup).to_string();
            assert!(
                (3..=63).contains(&name.chars().count()),
                "case {case:?}/{setup:?}: the row's own bucket name must be one a create \
                 records, got {} characters in {name:?}",
                name.chars().count()
            );
            if setup == Setup::WithoutAConfiguration {
                let created = create_bucket(&router, &name, None, None).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}/{setup:?}: the bucket must be created first, got {} with \
                     body {:?}",
                    created.status,
                    text(&created)
                );
            }

            let (status, code, message) = match setup {
                Setup::NotHeld => (404, NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE),
                Setup::WithoutAConfiguration => (
                    METADATA_CONFIGURATION_NOT_FOUND_STATUS,
                    METADATA_CONFIGURATION_NOT_FOUND_CODE,
                    METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
                ),
            };
            let refused = update(
                &router,
                &name,
                "metadataAnnotationTable",
                &annotation_updates("DISABLED", None),
            )
            .await;
            assert_eq!(
                refused.status, status,
                "case {case:?}/{setup:?}: expected HTTP {status}, got {} with body {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}/{setup:?}: expected the error code {code:?}, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}/{setup:?}: expected the published sentence, got {:?}",
                refused.error_message()
            );
            assert_eq!(
                refused.element("BucketName").as_deref(),
                Some(name.as_str()),
                "case {case:?}/{setup:?}: the error document must name the bucket the request \
                 addressed, got {:?} in {:?}",
                refused.element("BucketName"),
                text(&refused)
            );
            // The naming member belongs to the error document, beside its code and message.
            let document = text(&refused);
            assert!(
                document.find("<BucketName>").is_some_and(|at| document
                    .find("</Message>")
                    .is_some_and(|before| before < at)),
                "case {case:?}/{setup:?}: the naming member must follow the message the way \
                 S3's error document carries it, got {document:?}"
            );

            // The refusal wrote nothing: a bucket that was held still carries no
            // configuration, so a read of it is refused the same way.
            if setup == Setup::WithoutAConfiguration {
                let read = get_metadata_configuration(&router, &name, &[]).await;
                assert_eq!(
                    read.error_code(),
                    METADATA_CONFIGURATION_NOT_FOUND_CODE,
                    "case {case:?}/{setup:?}: the refused update must have recorded no \
                     configuration, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
            }
        }
    }

    // The neighbour still reports the configuration it was given, so no row's refusal touched
    // another bucket's state.
    let held = get_metadata_configuration(&router, neighbour, &[]).await;
    assert_eq!(
        held.status,
        200,
        "the neighbour must still report its configuration, got {} with body {:?}",
        held.status,
        text(&held)
    );
}
