// SPDX-License-Identifier: CC0-1.0

//! The ENTITY TYPES a DataAutomationLibrary holds, which is what
//! `GetDataAutomationLibrary` reports in `entityTypes`.
//!
//! RULE. A library holds an entity type from the moment the service has somewhere to resolve
//! entities of that type in it: the type [`PROVISIONED_ENTITY_TYPE`] the create stands every
//! library up with, plus every type an accepted `UPSERT` ingestion job has populated
//! ([`UPSERT_OPERATION`]). Each type is reported ONCE however many jobs have populated it,
//! and carries the `entityMetadata` derived from the jobs this store holds for that (library,
//! type) pair — so the list and its contents are state-derived: they grow as the library is
//! ingested into and go with the library when it is released.
//!
//! RECORDED for the provisioned type: the read-back of a freshly created library reports one
//! `entityTypes` element, `VOCABULARY`, carrying an `entityMetadata` value — so a library
//! holds that type before anything has been ingested into it. A type the model declares
//! beside it (`smithy_query(query="shape",shape="EntityType")` -> `["VOCABULARY"]`) appears
//! only once an ingestion has populated it.
//!
//! Single-homed here because the create and the ingestion submission both write this list and
//! the library's read projects it: three readings of what a library holds are three answers
//! waiting to drift.

use super::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use crate::ops::types::EntityTypeInfo;

/// The entity type every created library is stood up holding.
///
/// Source: the recording of a library's read-back right after its create (see the module
/// docs) — a Data Automation Library is provisioned with its vocabulary, so a caller can
/// resolve against that type without ingesting first.
pub const PROVISIONED_ENTITY_TYPE: &str = "VOCABULARY";

/// The `operationType` whose submissions POPULATE a library with entities of the entity type
/// they name, and so the one that gives the library that type.
///
/// Source: `smithy_query(query="shape",shape="LibraryIngestionJobOperationType")` ->
/// `["DELETE", "UPSERT"]`. The other value takes entities OUT of the library; this store
/// holds the entity TYPES a library has acquired rather than the individual entities under
/// them, so it cannot tell when a delete has emptied a type — and a delete therefore takes no
/// type away.
pub const UPSERT_OPERATION: &str = "UPSERT";

/// The entity types a create stands a library up holding: the provisioned type, with the
/// metadata of a type nothing has been ingested into yet.
#[must_use]
pub fn provisioned_entity_types() -> Vec<EntityTypeInfo> {
    vec![EntityTypeInfo {
        entity_type: PROVISIONED_ENTITY_TYPE.to_string(),
        entity_metadata: Some(entity_type_metadata(0, None)),
    }]
}

/// The `entityMetadata` a library reports for one of the entity types it holds.
///
/// `smithy_query(query="shape",shape="EntityMetadata")` documents the member as "JSON string
/// representing relevant metadata for the entity type" and states no schema for it, so the
/// value is a JSON object built from what this store KNOWS about that type of that library:
/// how many ingestion jobs have populated it, and — once one has — when the latest of them
/// was submitted. Nothing in it comes from the request.
#[must_use]
fn entity_type_metadata(
    ingestion_jobs: usize,
    last_ingestion: Option<::aws_smithy_types::DateTime>,
) -> String {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "ingestionJobCount".to_string(),
        ::serde_json::Value::from(ingestion_jobs),
    );
    if let Some(at) = last_ingestion {
        if let Ok(text) = at.fmt(::aws_smithy_types::date_time::Format::DateTime) {
            document.insert("lastIngestionTime".to_string(), text.into());
        }
    }
    ::serde_json::Value::Object(document).to_string()
}

impl ServiceState {
    /// Record that the library `library_arn` names now HOLDS the entity type `entity_type` —
    /// what an accepted `UPSERT` ingestion job against it does to the library.
    ///
    /// The type is listed once however many jobs have populated it, and its metadata is
    /// recomputed from the jobs this store holds for the pair, so what a read reports is
    /// derived from state rather than from whichever submission arrived last. Called AFTER the
    /// job is stored, so the submission being accepted is one of the jobs counted.
    ///
    /// A library this store does not hold acquires nothing: a submission naming an unresolved
    /// `libraryArn` is refused before it reaches here.
    pub fn record_library_entity_type(&mut self, library_arn: &str, entity_type: &str) {
        let populating: Vec<::aws_smithy_types::DateTime> = self
            .ingestion_jobs
            .values()
            .filter(|job| {
                job.library_arn == library_arn
                    && job.entity_type == entity_type
                    && job.operation_type == UPSERT_OPERATION
            })
            .map(|job| job.creation_time)
            .collect();
        let latest = populating
            .iter()
            .copied()
            .max_by_key(|at| (at.secs(), at.subsec_nanos()));
        let metadata = entity_type_metadata(populating.len(), latest);
        let Some(library) = self.libraries.get_mut(library_arn) else {
            return;
        };
        match library
            .entity_types
            .iter_mut()
            .find(|held| held.entity_type == entity_type)
        {
            Some(held) => held.entity_metadata = Some(metadata),
            None => library.entity_types.push(EntityTypeInfo {
                entity_type: entity_type.to_string(),
                entity_metadata: Some(metadata),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a created library holds the provisioned type, with the metadata of a type no
    /// ingestion has populated.
    #[test]
    fn a_created_library_holds_the_provisioned_type() {
        let held = provisioned_entity_types();
        let named: Vec<&str> = held
            .iter()
            .map(|info| info.entity_type.as_str())
            .collect();
        assert_eq!(
            named,
            vec![PROVISIONED_ENTITY_TYPE],
            "case \"a created library\": expected the provisioned type alone, got {named:?}"
        );
        let metadata = held[0]
            .entity_metadata
            .clone()
            .expect("case \"a created library\": the provisioned type carries metadata");
        let document: ::serde_json::Value = ::serde_json::from_str(&metadata)
            .expect("case \"a created library\": the metadata is a JSON document");
        assert_eq!(
            document.get("ingestionJobCount").and_then(|v| v.as_u64()),
            Some(0),
            "case \"a created library\": expected no ingestion counted, got {document:?}"
        );
        assert!(
            document.get("lastIngestionTime").is_none(),
            "case \"a created library\": a type nothing has ingested into has no ingestion \
             instant, got {document:?}"
        );
    }

    /// RULE: the metadata counts the jobs that have populated the type and names the latest
    /// of their instants, in the modelled `date-time` form.
    #[test]
    fn the_metadata_counts_the_jobs_and_names_the_latest_instant() {
        // (case, how many jobs have populated the type, the latest instant among them)
        let rows: [(&str, usize, Option<i64>); 3] = [
            ("nothing ingested", 0, None),
            ("one job", 1, Some(1_700_000_000)),
            ("several jobs", 4, Some(1_700_000_900)),
        ];
        for (case, jobs, latest) in rows {
            let metadata = entity_type_metadata(
                jobs,
                latest.map(::aws_smithy_types::DateTime::from_secs),
            );
            let document: ::serde_json::Value = ::serde_json::from_str(&metadata)
                .unwrap_or_else(|e| panic!("case {case:?}: {metadata:?} is not JSON: {e}"));
            assert_eq!(
                document.get("ingestionJobCount").and_then(|v| v.as_u64()),
                Some(jobs as u64),
                "case {case:?}: expected {jobs} counted, got {document:?}"
            );
            let reported = document
                .get("lastIngestionTime")
                .and_then(|v| v.as_str())
                .map(|text| {
                    ::aws_smithy_types::DateTime::from_str(
                        text,
                        ::aws_smithy_types::date_time::Format::DateTime,
                    )
                    .unwrap_or_else(|e| panic!("case {case:?}: {text:?} is no date-time: {e}"))
                    .secs()
                });
            assert_eq!(
                reported, latest,
                "case {case:?}: expected the latest instant {latest:?}, got {reported:?}"
            );
        }
    }
}
