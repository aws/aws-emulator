// SPDX-License-Identifier: CC0-1.0

//! The ENTITIES a DataAutomationLibrary holds — what `GetDataAutomationLibraryEntity`
//! resolves one of and `ListDataAutomationLibraryEntities` enumerates. The entity TYPES a
//! library has acquired are [`super::library_entity_types`]; the jobs that put entities
//! there are [`super::library_ingestion`].
//!
//! RULE. The entities of a (library, entity type) pair are what the ingestion jobs accepted
//! against that library have made of it: a job's payload applies when the clock has made
//! that job's completion due — the submission is asynchronous, so nothing it carries is in
//! the library before it finishes — and the jobs apply in the order they were submitted
//! ([`super::in_listing_order`]). An `UPSERT` entry stands the entity its `entityId` names
//! up, or replaces the entity already stored under it with what that entry carries; a
//! `DELETE` entry takes the entities its `entityIds` name out. An entity is therefore
//! resolvable from the instant the job that upserted it completed until a job that deletes
//! it completes, and reports the description, language and phrases of the LATEST entry that
//! upserted it.
//!
//! An entry's own `entityId`, `language` and `phrases` reached the store through the modelled
//! constraint walk the submission ran before its handler, so what is folded here already
//! satisfies the constraints those members carry.
//!
//! DERIVED from the jobs this store already holds rather than stored a second time: the
//! payload a submission persisted is the whole record of what was ingested, so a read folds
//! it rather than writing anything of its own — two reads at one instant cannot disagree,
//! and a library that releases its jobs ([`super::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState::remove_library`])
//! releases its entities with them.
//!
//! Single-homed here because the read of one entity and the entity list of a library are two
//! projections of that same fold, served one page at a time in this service's listing order
//! ([`super::in_listing_order`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::library_entity_types::UPSERT_OPERATION;
use super::library_ingestion::{
    DataAutomationLibraryIngestionJobRecord, INGESTION_JOB_COMPLETED,
};
use super::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use super::ListedResource;
use crate::ops::types::{
    DataAutomationLibraryEntitySummary, EntityDetails, InlinePayload, Phrase, UpsertEntityInfo,
    VocabularyEntity, VocabularyEntitySummary,
};

/// [`super::LIBRARY_ARN_NOT_FOUND_MESSAGE`] for the resource this module holds: what a read
/// answers when the (entity type, entity id) pair it was handed names no entity of the
/// library the request addressed.
///
/// RECORDED behaviour, and entirely FIXED text: the sentence names the two input members that
/// address an entity, interpolates neither of their values and reads nothing from state — so
/// it is spelled here once and never formatted.
pub const ENTITY_NOT_FOUND_MESSAGE: &str =
    "The specified entity ID and entity type is not found.";

/// The largest page `ListDataAutomationLibraryEntities` serves, and the page size it uses
/// when the request names no `maxResults`. The same value as [`super::LIBRARY_PAGE_LIMIT`]
/// because it has the same source: this listing binds `maxResults` to the one modelled
/// `MaxResults` shape
/// (`smithy_query(query="operation",operation="ListDataAutomationLibraryEntities")` ->
/// `@range(min: 1, max: 1000)`), so no caller can ask for more than this and an absent
/// `maxResults` cannot serialise every entity a library holds either.
pub const ENTITY_PAGE_LIMIT: i32 = super::LIBRARY_PAGE_LIMIT;

/// The `operationType` whose submissions take entities OUT of the library they name.
///
/// Source: `smithy_query(query="shape",shape="LibraryIngestionJobOperationType")` ->
/// `["DELETE", "UPSERT"]` — the other half of the pair [`UPSERT_OPERATION`] names.
pub const DELETE_OPERATION: &str = "DELETE";

/// One entity a library holds, as the jobs ingested into it leave it.
#[derive(Debug, Clone)]
pub struct LibraryEntityRecord {
    /// What addresses this entity inside its (library, entity type) pair: the `entityId` the
    /// upsert entry carried, or the one this store minted for an entry that carried none
    /// ([`minted_entity_id`]).
    pub entity_id: String,
    pub description: Option<String>,
    pub language: String,
    pub phrases: Vec<Phrase>,
    /// The instant the ingestion that last wrote this entity completed, which is the instant
    /// from which the entity reads as it does now.
    pub last_modified_time: ::aws_smithy_types::DateTime,
}

impl LibraryEntityRecord {
    /// Project the entity onto the modeled `EntityDetails` union a read serves.
    ///
    /// `smithy_query(query="shape",shape="EntityDetails")` carries the single member
    /// `vocabulary` (`VocabularyEntity`), so that is the arm every entity this store holds is
    /// reported through, with every member of the projection filled from the record.
    #[must_use]
    pub fn to_entity_details(&self) -> EntityDetails {
        EntityDetails::Vocabulary(VocabularyEntity {
            entity_id: Some(self.entity_id.clone()),
            description: self.description.clone(),
            language: Some(self.language.clone()),
            phrases: Some(self.phrases.clone()),
            last_modified_time: Some(self.last_modified_time),
        })
    }

    /// Project the entity onto the NARROW `DataAutomationLibraryEntitySummary` union an item
    /// of a listing is.
    ///
    /// `smithy_query(query="shape",shape="DataAutomationLibraryEntitySummary")` carries the
    /// single member `vocabulary` (`VocabularyEntitySummary`) — the same arm the read's
    /// `EntityDetails` carries — and that summary reports the entity's identity, its
    /// description and language, HOW MANY phrases it holds rather than the phrases themselves,
    /// and the instant it was last written. Every member is filled from the same record the
    /// read projects, so a listing and a read of one entity cannot disagree about it.
    #[must_use]
    pub fn to_entity_summary(&self) -> DataAutomationLibraryEntitySummary {
        DataAutomationLibraryEntitySummary::Vocabulary(VocabularyEntitySummary {
            entity_id: Some(self.entity_id.clone()),
            description: self.description.clone(),
            language: Some(self.language.clone()),
            // `numOfPhrases` carries `@range(min: 1)`
            // (`smithy_query(query="shape",shape="VocabularyEntitySummary")`), so an entity
            // holding no phrase reports NO count rather than a zero the model does not admit.
            num_of_phrases: i32::try_from(self.phrases.len())
                .ok()
                .filter(|count| *count >= 1),
            last_modified_time: Some(self.last_modified_time),
        })
    }
}

impl ListedResource for LibraryEntityRecord {
    /// The instant this entity was last written, then its `entityId` — the same shape of key
    /// every listing of this service orders by ([`super::in_listing_order`]), with the entity's
    /// own instant standing where a resource's `creationTime` does and its identity inside its
    /// (library, entity type) pair breaking the tie between the entities one ingestion wrote.
    fn order_key(&self) -> (i64, u32, &str) {
        (
            self.last_modified_time.secs(),
            self.last_modified_time.subsec_nanos(),
            self.entity_id.as_str(),
        )
    }
}

/// The `entityId` an upsert entry that names none is stored under.
///
/// `VocabularyEntityInfo$entityId` is OPTIONAL
/// (`smithy_query(query="shape",shape="VocabularyEntityInfo")`), so an upsert can describe an
/// entity without naming one — and the service then has to address it by an id of its own.
/// Built from the resource id of the job's own ARN and the entry's position in that job's
/// payload, so it differs for every (job, position) this store can address — the ARN is
/// minted by [`super::library_ingestion::mint_ingestion_job_arn`], whose resource id is
/// hexadecimal with one `-` and so carries over whole — and it is stable: a read derives the
/// same id from the same stored job however often it is asked.
///
/// The result is inside the modelled `EntityId` `@length(1..128)` / `^[a-zA-Z0-9-_]+$` for
/// every value, not only for the ARNs this store mints: a character the class does not admit
/// is substituted rather than dropped, a resource id with nothing to carry over falls back to
/// [`MINTED_ENTITY_ID_STEM`], and the stem is truncated to leave room for the position. Those
/// three only apply to a resource id no minting of this store produces, and they are shape
/// repairs rather than uniqueness ones: an ARN that needed one is outside what this store
/// mints, and two such ARNs can reduce to one id.
#[must_use]
pub fn minted_entity_id(job_arn: &str, index: usize) -> String {
    let resource = job_arn.rsplit('/').next().unwrap_or(job_arn);
    let stem: String = resource
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                MINTED_ENTITY_ID_SUBSTITUTE
            }
        })
        .collect();
    let position = format!("-{index}");
    let room = MINTED_ENTITY_ID_LIMIT.saturating_sub(position.chars().count());
    let stem = if stem.is_empty() {
        MINTED_ENTITY_ID_STEM
    } else {
        &stem
    };
    let stem: String = stem.chars().take(room).collect();
    format!("{stem}{position}")
}

/// What stands in for a character the modelled `EntityId` class does not admit, so a resource
/// id carrying one still mints an id of the same shape and length.
const MINTED_ENTITY_ID_SUBSTITUTE: char = '_';

/// What a minted id is built on when the job ARN's resource id carries nothing at all — the id
/// still has to be non-empty to satisfy the modelled `@length(1..128)`.
const MINTED_ENTITY_ID_STEM: &str = "entity";

/// The longest `entityId` the model admits
/// (`smithy_query(query="shape",shape="EntityId")`: `@length(min: 1, max: 128)`), which a
/// minted id is kept inside however long the ARN it is built from is.
const MINTED_ENTITY_ID_LIMIT: usize = 128;

/// Apply one COMPLETED job's inline payload to the entities `held` so far.
///
/// A job whose source is an `s3Object` applies nothing here: its entities live in an object
/// whose content schema the model does not state, so this store has no reading of it to fold.
/// Neither does a job whose payload arm is not the one its `operationType` performs — an
/// upsert job carrying a delete payload describes no upsert.
fn apply_to_entities(
    held: &mut BTreeMap<String, LibraryEntityRecord>,
    job: &DataAutomationLibraryIngestionJobRecord,
) {
    let Some(payload) = job.input_configuration.inline_payload.as_ref() else {
        return;
    };
    match (job.operation_type.as_str(), payload) {
        (UPSERT_OPERATION, InlinePayload::upsertEntitiesInfo(entries)) => {
            for (index, entry) in entries.iter().enumerate() {
                let UpsertEntityInfo::Vocabulary(info) = entry;
                let entity_id = info
                    .entity_id
                    .clone()
                    .unwrap_or_else(|| minted_entity_id(&job.job_arn, index));
                held.insert(
                    entity_id.clone(),
                    LibraryEntityRecord {
                        entity_id,
                        description: info.description.clone(),
                        language: info.language.clone(),
                        phrases: info.phrases.clone(),
                        last_modified_time: job.completion_time,
                    },
                );
            }
        }
        (DELETE_OPERATION, InlinePayload::deleteEntitiesInfo(info)) => {
            for entity_id in &info.entity_ids {
                held.remove(entity_id);
            }
        }
        _ => {}
    }
}

impl ServiceState {
    /// The entities of one (library, entity type) pair as of `now`, keyed by their
    /// `entityId`.
    ///
    /// A pure function of the jobs this store holds: the jobs of that library and that entity
    /// type whose completion the clock has made due, folded in submission order, so nothing
    /// an in-flight job carries is reported and reporting writes nothing.
    #[must_use]
    pub fn library_entities(
        &self,
        library_arn: &str,
        entity_type: &str,
        now: SystemTime,
    ) -> BTreeMap<String, LibraryEntityRecord> {
        let ingested = super::in_listing_order(self.ingestion_jobs.values().filter(|job| {
            job.library_arn == library_arn
                && job.entity_type == entity_type
                && job.status_at(now) == INGESTION_JOB_COMPLETED
        }));
        let mut held: BTreeMap<String, LibraryEntityRecord> = BTreeMap::new();
        for job in ingested {
            apply_to_entities(&mut held, job);
        }
        held
    }

    /// The one entity `entity_id` names inside the (library, entity type) pair addressed, as
    /// of `now`.
    ///
    /// An entity belongs to one pair, so a request naming another library or another entity
    /// type resolves to nothing: the read is of the triple, never of the id alone.
    #[must_use]
    pub fn library_entity(
        &self,
        library_arn: &str,
        entity_type: &str,
        entity_id: &str,
        now: SystemTime,
    ) -> Option<LibraryEntityRecord> {
        self.library_entities(library_arn, entity_type, now)
            .remove(entity_id)
    }

    /// The entities of one (library, entity type) pair as of `now`, in this service's listing
    /// order ([`LibraryEntityRecord::order_key`]) — what a page of
    /// `ListDataAutomationLibraryEntities` is taken from.
    ///
    /// The same fold [`Self::library_entities`] serves the read of one entity from, so a
    /// listing enumerates exactly the entities a read of each id would resolve.
    #[must_use]
    pub fn library_entities_in_order(
        &self,
        library_arn: &str,
        entity_type: &str,
        now: SystemTime,
    ) -> Vec<LibraryEntityRecord> {
        let held = self.library_entities(library_arn, entity_type, now);
        super::in_listing_order(held.values())
            .into_iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ops::types::{
        DeleteEntitiesInfo, InputConfiguration, OutputConfiguration, VocabularyEntityInfo,
    };

    /// A library every case here ingests into.
    const LIBRARY: &str = "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/abcdefabcdef";

    /// One stored ingestion job: submitted at `submitted_at`, completing one span later.
    fn job(
        job_arn: &str,
        entity_type: &str,
        operation_type: &str,
        payload: Option<InlinePayload>,
        submitted_at: SystemTime,
    ) -> DataAutomationLibraryIngestionJobRecord {
        let completes_at = submitted_at + super::super::library_ingestion::INGESTION_JOB_SPAN;
        DataAutomationLibraryIngestionJobRecord {
            job_arn: job_arn.to_string(),
            library_arn: LIBRARY.to_string(),
            entity_type: entity_type.to_string(),
            operation_type: operation_type.to_string(),
            input_configuration: InputConfiguration {
                s3_object: None,
                inline_payload: payload,
            },
            output_configuration: OutputConfiguration {
                s3_uri: "s3://bucket/out/".to_string(),
            },
            notification_configuration: None,
            creation_time: ::aws_smithy_types::DateTime::from(submitted_at),
            completes_at,
            completion_time: ::aws_smithy_types::DateTime::from(completes_at),
            tags: Vec::new(),
        }
    }

    /// An upsert payload for one vocabulary entity.
    fn upsert(entity_id: Option<&str>, language: &str, phrases: &[&str]) -> InlinePayload {
        InlinePayload::upsertEntitiesInfo(vec![UpsertEntityInfo::Vocabulary(
            VocabularyEntityInfo {
                entity_id: entity_id.map(str::to_string),
                description: None,
                language: language.to_string(),
                phrases: phrases
                    .iter()
                    .map(|text| Phrase {
                        text: (*text).to_string(),
                        display_as_text: None,
                    })
                    .collect(),
            },
        )])
    }

    /// A store holding the jobs handed in.
    fn store_with(jobs: Vec<DataAutomationLibraryIngestionJobRecord>) -> ServiceState {
        let mut store = ServiceState::default();
        for record in jobs {
            store.ingestion_jobs.insert(record.job_arn.clone(), record);
        }
        store
    }

    /// One row of the fold table: the case, the instant read at, the language reported for
    /// the entity, how many phrases it carries, and the instant its ingestion completed.
    type FoldRow<'a> = (
        &'a str,
        SystemTime,
        Option<&'a str>,
        usize,
        Option<SystemTime>,
    );

    /// RULE: a completed upsert puts its entity in the library, a later upsert of the same id
    /// replaces what it carried, and a completed delete takes it out — while an in-flight job
    /// has changed nothing yet.
    #[test]
    fn the_entities_are_what_the_completed_jobs_made_of_the_library() {
        let span = super::super::library_ingestion::INGESTION_JOB_SPAN;
        let first = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let second = first + span * 2;
        let third = second + span * 2;
        let store = store_with(vec![
            job(
                "job/1",
                "VOCABULARY",
                UPSERT_OPERATION,
                Some(upsert(Some("vocab-1"), "EN", &["mockingbird"])),
                first,
            ),
            job(
                "job/2",
                "VOCABULARY",
                UPSERT_OPERATION,
                Some(upsert(Some("vocab-1"), "DE", &["nachtigall", "amsel"])),
                second,
            ),
            job(
                "job/3",
                "VOCABULARY",
                DELETE_OPERATION,
                Some(InlinePayload::deleteEntitiesInfo(DeleteEntitiesInfo {
                    entity_ids: vec!["vocab-1".to_string()],
                })),
                third,
            ),
        ]);

        // (case, the instant read at, the language reported for `vocab-1`, how many phrases,
        // the instant the ingestion that last wrote it completed)
        let rows: [FoldRow<'_>; 5] = [
            ("before the first job completes", first, None, 0, None),
            (
                "the first job has completed",
                first + span,
                Some("EN"),
                1,
                Some(first + span),
            ),
            (
                "the second job is still in flight",
                second,
                Some("EN"),
                1,
                Some(first + span),
            ),
            (
                "the second job has completed",
                second + span,
                Some("DE"),
                2,
                Some(second + span),
            ),
            ("the delete has completed", third + span, None, 0, None),
        ];
        for (case, now, language, phrases, modified) in rows {
            let held = store.library_entity(LIBRARY, "VOCABULARY", "vocab-1", now);
            let reported = held.as_ref().map(|entity| entity.language.clone());
            assert_eq!(
                reported.as_deref(),
                language,
                "case {case:?}: expected language {language:?}, got {held:?}"
            );
            let counted = held.as_ref().map_or(0, |entity| entity.phrases.len());
            assert_eq!(
                counted, phrases,
                "case {case:?}: expected {phrases} phrase(s), got {held:?}"
            );
            let instant = held
                .as_ref()
                .map(|entity| entity.last_modified_time.secs());
            let expected = modified.map(|at| {
                at.duration_since(SystemTime::UNIX_EPOCH)
                    .expect("an instant after the epoch")
                    .as_secs() as i64
            });
            assert_eq!(
                instant, expected,
                "case {case:?}: the entity reports the instant its ingestion completed, got \
                 {held:?}"
            );
        }
    }

    /// RULE: an entity belongs to ONE (library, entity type) pair, and an upsert naming no
    /// `entityId` is stored under an id this service minted for it.
    #[test]
    fn an_entity_belongs_to_one_pair_and_an_unnamed_one_is_minted_an_id() {
        let span = super::super::library_ingestion::INGESTION_JOB_SPAN;
        let submitted = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let now = submitted + span;
        let store = store_with(vec![
            job(
                "job/named",
                "VOCABULARY",
                UPSERT_OPERATION,
                Some(upsert(Some("named_1"), "EN", &["kingfisher"])),
                submitted,
            ),
            job(
                "job/unnamed",
                "VOCABULARY",
                UPSERT_OPERATION,
                Some(upsert(None, "FR", &["martin-pecheur"])),
                submitted,
            ),
        ]);

        // The pair both jobs named holds both entities: the one the upsert named, and the one
        // stored under the id this service minted for the upsert that named none.
        let minted = minted_entity_id("job/unnamed", 0);
        let mut held: Vec<String> = store
            .library_entities(LIBRARY, "VOCABULARY", now)
            .into_keys()
            .collect();
        held.sort();
        let mut expected = vec!["named_1".to_string(), minted.clone()];
        expected.sort();
        assert_eq!(
            held, expected,
            "case \"the ingested pair\": expected {expected:?}, got {held:?}"
        );
        assert!(
            minted
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && (1..=128).contains(&minted.len()),
            "case \"a minted entity id\": {minted:?} must satisfy the modelled EntityId \
             constraints"
        );
        let other_library = store.library_entities(
            "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/0123456789ab",
            "VOCABULARY",
            now,
        );
        assert!(
            other_library.is_empty(),
            "case \"another library\": a job of this library ingests into no other, got \
             {other_library:?}"
        );
    }

    /// RULE: a minted `entityId` satisfies the modelled `EntityId` constraints
    /// (`smithy_query(query="shape",shape="EntityId")`: `@length(min: 1, max: 128)`,
    /// `^[a-zA-Z0-9-_]+$`) whatever ARN and position it is built from, and two different
    /// (job, position) pairs mint two different ids.
    ///
    /// The rows span the ARN this store itself mints, a resource id carrying characters the
    /// class does not admit, one the class admits nothing of, one far longer than the modelled
    /// length, a value that is no ARN at all, and the positions a payload can reach
    /// (`UpsertEntitiesInfo` is `@length(min: 1, max: 10)`).
    #[test]
    fn a_minted_entity_id_satisfies_the_modelled_constraints() {
        let minted = super::super::library_ingestion::mint_ingestion_job_arn(
            "us-east-1",
            ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            7,
        );
        // (case, the job ARN, the position in that job's payload)
        let rows: [(&str, String, usize); 6] = [
            ("an ARN this store minted", minted, 0),
            (
                "a resource id carrying characters the class does not admit",
                "arn:aws:bedrock:us-east-1:000000000000:data-automation-library-ingestion-job/a.b:c"
                    .to_string(),
                9,
            ),
            (
                "a resource id the class admits nothing of",
                "arn:aws:bedrock:us-east-1:000000000000:data-automation-library-ingestion-job/"
                    .to_string(),
                0,
            ),
            (
                "a resource id far longer than the modelled length",
                format!("job/{}", "a".repeat(400)),
                10,
            ),
            ("a value that is no ARN at all", "not-an-arn".to_string(), 3),
            ("the last position a payload can reach", "job/one".to_string(), 10),
        ];
        let mut seen: Vec<String> = Vec::new();
        for (case, job_arn, index) in rows {
            let id = minted_entity_id(&job_arn, index);
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "case {case:?}: expected only characters the modelled EntityId pattern admits, \
                 got {id:?}"
            );
            assert!(
                (1..=128).contains(&id.chars().count()),
                "case {case:?}: expected a length inside the modelled 1..128, got {} in {id:?}",
                id.chars().count()
            );
            assert_eq!(
                id,
                minted_entity_id(&job_arn, index),
                "case {case:?}: the same job and position mint the same id"
            );
            assert!(
                !seen.contains(&id),
                "case {case:?}: expected an id no other (job, position) minted, got {id:?} \
                 among {seen:?}"
            );
            seen.push(id);
        }
        // Two positions of ONE job, and two jobs at one position, mint different ids.
        assert_ne!(
            minted_entity_id("job/one", 0),
            minted_entity_id("job/one", 1),
            "case \"two positions of one job\": expected two ids"
        );
        assert_ne!(
            minted_entity_id("job/one", 0),
            minted_entity_id("job/two", 0),
            "case \"two jobs at one position\": expected two ids"
        );
    }
}
