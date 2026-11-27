// SPDX-License-Identifier: CC0-1.0

//! Merging one case into another: the record a merge is started as, the
//! completion the clock owes it, and what a request against a case taking part
//! in one is refused with.
//!
//! A merge is a resource of the domain the two cases belong to, exactly as
//! those cases are, so the record lives under [`Domain`] and everything a merge
//! needs from the store lives here rather than in one operation's file: the
//! merge starts in one operation, is REPORTED by another, and is what three
//! other operations refuse a request against either case with.
//!
//! The merge "runs asynchronously"
//! (`smithy_query(query="doc", shape="StartMergeCases")`), so its completion is
//! a transition the CLOCK owes: the start stores the instant it falls due
//! ([`MERGE_RUNS_FOR`] after the call) and nothing sleeps for it. Every
//! operation that reads or writes a case materialises the merges already due
//! ([`Domain::settle_merges`]) before it answers, so what a caller sees never
//! depends on who looked first.

use super::*;

use std::time::Duration;

use crate::ops::types::{
    AuditMergeMetadata, CaseIdentifier, ConnectCaseInputContent, EmptyFieldValue,
    RelatedItemsConfiguration,
};

/// How long a merge runs before the service owes its completion.
///
/// The documentation states that the merge is asynchronous and gives no
/// duration, so this span is the emulator's: long enough that a caller which
/// looks straight after starting one still sees the running merge and the
/// refusals it carries, short enough that a caller waiting for it does not
/// wait long. The instant it falls due is stored on the record, so no request
/// ever waits for the clock.
pub const MERGE_RUNS_FOR: Duration = Duration::from_secs(3);

/// `AuditEventType` of the event a finished merge records on each of the two
/// cases that took part in it: "`Case.Merged` summarizes a merge on each of the
/// two cases that took part in it"
/// (`smithy_query(query="shape", shape="AuditEvent")`, member `type`). One of
/// `AUDIT_EVENT_TYPE_VALUES`.
pub const AUDIT_EVENT_CASE_MERGED: &str = "Case.Merged";

/// `MergeStatus` of a merge that is still running. One of
/// `MERGE_STATUS_VALUES`.
pub const MERGE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `MergeStatus` a case that was merged into another one as the secondary case
/// reports. One of `MERGE_STATUS_VALUES`.
pub const MERGE_STATUS_COMPLETED: &str = "COMPLETED";

/// The `System`-namespace fields a merge may take the secondary case's value
/// for: "You can only merge custom fields and the following system fields:
/// assigned_queue, assigned_user, summary, and case_reason"
/// (`smithy_query(query="operation", operation="StartMergeCases")`, member
/// `fieldsConfiguration`). Every other reserved field of
/// [`RESERVED_SYSTEM_FIELDS`] is the service's own to maintain, so a
/// configuration naming one is refused rather than applied.
pub const MERGEABLE_SYSTEM_FIELDS: &[&str] =
    &["assigned_queue", "assigned_user", "case_reason", "summary"];

/// The reserved field a case's status is held in, one of
/// [`RESERVED_SYSTEM_FIELDS`]. It is what a merge closes the secondary case
/// through, and what tells a closed case from an open one.
pub const CASE_STATUS_FIELD_ID: &str = "status";

/// The reserved field a case's title is held in, one of
/// [`RESERVED_SYSTEM_FIELDS`].
pub const CASE_TITLE_FIELD_ID: &str = "title";

/// The reserved field a case's customer reference is held in, one of
/// [`RESERVED_SYSTEM_FIELDS`]: what a merge compares the two cases on.
pub const CASE_CUSTOMER_FIELD_ID: &str = "customer_id";

/// The reserved field a case's caller-chosen reference id is held in, one of
/// [`RESERVED_SYSTEM_FIELDS`]: what a `CaseIdentifierUnion$referenceId`
/// resolves a case by.
pub const CASE_REFERENCE_ID_FIELD_ID: &str = "reference_id";

/// The reserved field a case's reference number is held in, one of
/// [`RESERVED_SYSTEM_FIELDS`]: the other identifier a `CaseIdentifier` reports.
pub const CASE_REFERENCE_NUMBER_FIELD_ID: &str = "reference_number";

/// The `status` value a case closed by a merge holds: "the secondary case is
/// closed" when the merge finishes
/// (`smithy_query(query="doc", shape="StartMergeCases")`).
pub const CASE_STATUS_CLOSED: &str = "Closed";

/// Most merges one domain remembers. EMULATOR-OWNED ceiling on request-driven
/// growth, as [`MAX_DOMAINS`]: each accepted `StartMergeCases` stores one more
/// record, and a case may be the primary case of any number of merges over its
/// life, so past this many a further merge is refused with the modelled quota
/// error rather than the store growing with the number of merges one caller
/// starts.
pub const MAX_MERGES_PER_DOMAIN: usize = 1_000;

/// Most field values a case may hold once a merge has written to it: "The
/// request fails with a `ValidationException`, and no merge starts, if ...
/// After the merge, the primary case would have more than 500 field values"
/// (`smithy_query(query="doc", shape="StartMergeCases")`).
pub const MAX_CASE_FIELD_VALUES: usize = 500;

/// One merge of a secondary case into a primary case, as `StartMergeCases`
/// records it and `GetMergeCasesStatus` reports it.
///
/// The record carries what the merge will write when it falls due, rather than
/// the request it came from: the field ids whose value the primary case takes
/// from the secondary case, and the related-item types that move with them.
#[derive(Debug, Clone)]
pub struct CaseMerge {
    /// The merge's own identifier: "All audit events produced by the same
    /// merge, on both cases, have the same value"
    /// (`smithy_query(query="shape", shape="AuditMergeMetadata")`).
    pub merge_event_id: String,
    pub primary_case_id: String,
    pub secondary_case_id: String,
    /// The instant the completion of this merge falls due: [`MERGE_RUNS_FOR`]
    /// after the call that started it. Stored rather than waited for.
    pub completes_at: SystemTime,
    /// Whether the completion has been materialised
    /// ([`Domain::settle_merges`]).
    pub completed: bool,
    /// The fields whose value the primary case takes from the secondary case,
    /// in the order the request selected them. Bounded by
    /// [`MAX_CASE_FIELD_IDS`], the cap on the field list of the request it was
    /// built from.
    pub merged_field_ids: Vec<String>,
    /// The `RelatedItemType`s that move from the secondary case to the primary
    /// case, from the request's `relatedItemsConfiguration`. Values of
    /// `RELATED_ITEM_TYPE_VALUES`.
    pub moved_item_types: Vec<String>,
    /// The entity the request named as having performed the merge.
    pub performed_by: Option<UserUnion>,
    /// The IAM principal whose credentials started the merge
    /// ([`caller_principal_arn`]), recorded against both audit events the
    /// completion writes.
    pub iam_principal_arn: String,
    /// The region the start was addressed to, so the related items the
    /// completion moves and links are addressed under the same region their
    /// case is.
    pub region: String,
}

impl CaseMerge {
    /// Does this merge move related items of `item_type`?
    ///
    /// "Related items of a type that you don't select, or set to false, stay on
    /// the closed secondary case. If you omit this parameter, no related items
    /// are moved"
    /// (`smithy_query(query="operation", operation="StartMergeCases")`, member
    /// `relatedItemsConfiguration`), so an empty selection moves nothing.
    pub fn moves_item_type(&self, item_type: &str) -> bool {
        self.moved_item_types.iter().any(|moved| moved == item_type)
    }

    /// Is `case_id` one of the two cases this merge is about?
    pub fn is_about(&self, case_id: &str) -> bool {
        self.primary_case_id == case_id || self.secondary_case_id == case_id
    }

    /// The merge's metadata as an audit event reports it.
    fn metadata(&self) -> AuditMergeMetadata {
        AuditMergeMetadata {
            merge_event_id: self.merge_event_id.clone(),
            primary_case_id: self.primary_case_id.clone(),
            secondary_case_id: self.secondary_case_id.clone(),
        }
    }
}

impl Case {
    /// The string this case holds for the reserved field `field_id`, or `None`
    /// when it holds no value for it or holds one of another variant.
    ///
    /// A stored `emptyValue` is "no value" (`state::related_items`), so it
    /// reads back as none.
    pub fn text_field(&self, field_id: &str) -> Option<&str> {
        match self.field_values.get(field_id) {
            Some(FieldValueUnion::stringValue(text)) => Some(text.as_str()),
            _ => None,
        }
    }

    /// Is this case closed?
    ///
    /// A case's status is a value of its `status` field, which is the field a
    /// merge closes the secondary case through, so "either case is closed"
    /// (`smithy_query(query="doc", shape="StartMergeCases")`) is read off the
    /// same field a caller sets through `UpdateCase`. The comparison ignores
    /// case, since the status is a caller-written value of a `SingleSelect`
    /// field rather than a modelled enum.
    pub fn is_closed(&self) -> bool {
        self.text_field(CASE_STATUS_FIELD_ID)
            .is_some_and(|status| status.eq_ignore_ascii_case(CASE_STATUS_CLOSED))
    }

    /// The customer this case references, or `None` when it references none:
    /// what the two cases of a merge are compared on.
    pub fn customer(&self) -> Option<&str> {
        self.text_field(CASE_CUSTOMER_FIELD_ID)
    }

    /// This case's identity as a batch answer reports it: its own id, plus the
    /// reference values the case HOLDS, so an entry reports what the store
    /// says rather than only what the request named.
    pub fn identifier(&self) -> CaseIdentifier {
        CaseIdentifier {
            case_id: Some(self.case_id.clone()),
            reference_number: self
                .text_field(CASE_REFERENCE_NUMBER_FIELD_ID)
                .map(str::to_string),
            reference_id: self
                .text_field(CASE_REFERENCE_ID_FIELD_ID)
                .map(str::to_string),
        }
    }

    /// Record the audit event one finished merge writes on this case: an event
    /// of the merge kind, carrying the merge's own metadata so "all audit
    /// events produced by the same merge, on both cases" can be grouped
    /// (`smithy_query(query="shape", shape="AuditMergeMetadata")`), and the
    /// field values the merge wrote on this case.
    fn record_merge_event(
        &mut self,
        merge: &CaseMerge,
        performed_time: SystemTime,
        fields: Vec<CaseAuditEventField>,
    ) {
        self.record_audit_event(
            AUDIT_EVENT_CASE_MERGED,
            // An edit of the case itself, about no related item.
            None,
            performed_time,
            merge.iam_principal_arn.clone(),
            merge.performed_by.clone(),
            fields,
        );
        if let Some(event) = self.audit_events.back_mut() {
            event.merge_metadata = Some(merge.metadata());
        }
    }
}

impl Domain {
    /// Refuse a further merge once the domain remembers
    /// [`MAX_MERGES_PER_DOMAIN`] of them.
    pub fn merge_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.merges.len(),
            MAX_MERGES_PER_DOMAIN,
            "merges in this domain",
        )
    }

    /// Remember `merge` as started.
    pub fn start_merge(&mut self, merge: CaseMerge) {
        self.merges.push(merge);
    }

    /// The merge that is still running and that `case_id` takes part in, either
    /// as the primary or as the secondary case.
    pub fn running_merge(&self, case_id: &str) -> Option<&CaseMerge> {
        self.merges
            .iter()
            .find(|merge| !merge.completed && merge.is_about(case_id))
    }

    /// The finished merge `case_id` was the SECONDARY case of: the one state a
    /// case keeps for the rest of its life, since "after the merge, the
    /// secondary case ... can't be merged again"
    /// (`smithy_query(query="doc", shape="StartMergeCases")`). The most recent
    /// one, which is the only one a case can have.
    pub fn completed_merge_of_secondary(&self, case_id: &str) -> Option<&CaseMerge> {
        self.merges
            .iter()
            .rev()
            .find(|merge| merge.completed && merge.secondary_case_id == case_id)
    }

    /// The modelled 409 a request that MODIFIES `case_id` — its own values or
    /// its related items — is refused with while a merge of it runs, or `None`
    /// when no merge blocks it.
    ///
    /// "While the merge runs, you can't modify the secondary case or its
    /// related items" (`smithy_query(query="doc", shape="StartMergeCases")`),
    /// so the primary case stays writable while its merge runs and only the
    /// secondary case is held.
    pub fn merge_modification_conflict(&self, case_id: &str) -> Option<ConflictException> {
        self.running_merge(case_id)
            .filter(|merge| merge.secondary_case_id == case_id)
            .map(|_| ConflictException {
                message: format!(
                    "Case '{case_id}' is the secondary case of a merge that is in progress and \
                     cannot be modified until the merge finishes."
                ),
            })
    }

    /// The modelled 409 a DELETE of `case_id` is refused with while a merge of
    /// it runs, or `None` when no merge blocks it.
    ///
    /// Deleting the primary case is named on its own — "you can't delete the
    /// primary case" — and deleting the secondary case is a modification of it,
    /// so a running merge holds both of its cases against a delete.
    pub fn merge_deletion_conflict(&self, case_id: &str) -> Option<ConflictException> {
        self.running_merge(case_id).map(|_| ConflictException {
            message: format!(
                "Case '{case_id}' takes part in a merge that is in progress and cannot be \
                 deleted until the merge finishes."
            ),
        })
    }

    /// Materialise every merge of this domain whose completion the clock
    /// already owes, oldest first.
    ///
    /// A merge runs asynchronously, so its completion is owed at
    /// [`CaseMerge::completes_at`] whether or not anybody calls: this is the
    /// one place it is applied, and every operation that reads or writes a case
    /// runs it first, so the same store answers the same way however it is
    /// reached. The writes are the merge's own — they were due at that instant
    /// — and they are stamped with it rather than with the instant someone
    /// looked, so two callers reading at different times read one history.
    pub fn settle_merges(&mut self, now: SystemTime) {
        let due: Vec<usize> = self
            .merges
            .iter()
            .enumerate()
            .filter(|(_, merge)| !merge.completed && merge.completes_at <= now)
            .map(|(index, _)| index)
            .collect();
        for index in due {
            self.complete_merge(index);
        }
    }

    /// Apply the merge at `index`: the selected field values and the selected
    /// related items move from the secondary case to the primary case, the two
    /// cases are linked, the secondary case is closed, and each case records
    /// the merge in its audit history.
    fn complete_merge(&mut self, index: usize) {
        let merge = self.merges[index].clone();
        self.merges[index].completed = true;
        let at = merge.completes_at;
        let domain_id = self.domain_id.clone();
        // The two links the merge takes out, minted before either case is
        // reached so each link is a resource of its own with its own id.
        let link_on_secondary = new_id();
        let link_on_primary = new_id();

        // What the secondary case gives up, and the close that ends its part
        // in the merge.
        let Some(secondary) = self.cases.get_mut(&merge.secondary_case_id) else {
            return;
        };
        let taken: Vec<(String, FieldValueUnion)> = merge
            .merged_field_ids
            .iter()
            .map(|field_id| {
                (
                    field_id.clone(),
                    // "If the secondary case's value is empty, the primary
                    // case's value is cleared", and the union's own empty
                    // member is what a cleared value reads back as.
                    secondary
                        .field_values
                        .get(field_id)
                        .cloned()
                        .unwrap_or(FieldValueUnion::emptyValue(EmptyFieldValue {})),
                )
            })
            .collect();
        let moving: Vec<String> = secondary
            .related_items
            .values()
            .filter(|item| merge.moves_item_type(&item.item_type))
            .map(|item| item.related_item_id.clone())
            .collect();
        let mut moved: Vec<RelatedItem> = Vec::with_capacity(moving.len());
        for related_item_id in moving {
            if let Some(mut item) = secondary.related_items.remove(&related_item_id) {
                // The item is a resource under the case it is associated with,
                // so moving it re-addresses it under its new case.
                item.case_id = merge.primary_case_id.clone();
                item.related_item_arn = related_item_arn(
                    &merge.region,
                    &domain_id,
                    &merge.primary_case_id,
                    &item.related_item_id,
                );
                moved.push(item);
            }
        }
        let status_before = secondary.field_values.get(CASE_STATUS_FIELD_ID).cloned();
        let closed = FieldValueUnion::stringValue(CASE_STATUS_CLOSED.to_string());
        secondary
            .field_values
            .insert(CASE_STATUS_FIELD_ID.to_string(), closed.clone());
        secondary.related_items.insert(
            link_on_secondary.clone(),
            case_link(
                &merge,
                &domain_id,
                &merge.secondary_case_id,
                &merge.primary_case_id,
                link_on_secondary,
            ),
        );
        secondary.last_modified_time = at;
        secondary.record_merge_event(
            &merge,
            at,
            vec![CaseAuditEventField {
                field_id: CASE_STATUS_FIELD_ID.to_string(),
                old_value: status_before,
                new_value: closed,
            }],
        );

        // What the primary case receives.
        let Some(primary) = self.cases.get_mut(&merge.primary_case_id) else {
            return;
        };
        let written: Vec<CaseAuditEventField> = taken
            .into_iter()
            .map(|(field_id, value)| CaseAuditEventField {
                old_value: primary.field_values.insert(field_id.clone(), value.clone()),
                field_id,
                new_value: value,
            })
            .collect();
        for item in moved {
            primary
                .related_items
                .insert(item.related_item_id.clone(), item);
        }
        primary.related_items.insert(
            link_on_primary.clone(),
            case_link(
                &merge,
                &domain_id,
                &merge.primary_case_id,
                &merge.secondary_case_id,
                link_on_primary,
            ),
        );
        primary.last_modified_time = at;
        primary.record_merge_event(&merge, at, written);
    }
}

/// The related item a merge links one of its cases to the other with: a
/// `ConnectCase` association, which is the modelled type for a link to another
/// case of the same domain, associated with `case_id` and naming `linked_to`.
fn case_link(
    merge: &CaseMerge,
    domain_id: &str,
    case_id: &str,
    linked_to: &str,
    related_item_id: String,
) -> RelatedItem {
    let content = RelatedItemInputContent::connectCase(ConnectCaseInputContent {
        case_id: linked_to.to_string(),
    });
    RelatedItem {
        related_item_arn: related_item_arn(&merge.region, domain_id, case_id, &related_item_id),
        related_item_id,
        case_id: case_id.to_string(),
        item_type: related_item_content_type(&content).to_string(),
        content,
        association_time: merge.completes_at,
        performed_by: merge.performed_by.clone(),
        tags: HashMap::new(),
    }
}

/// Is `field_id` a field a merge may take the secondary case's value for?
///
/// "You can only merge custom fields and the following system fields"
/// (`StartMergeCases`, member `fieldsConfiguration`): a field of the `Custom`
/// namespace is a caller's own, and of the reserved ones only
/// [`MERGEABLE_SYSTEM_FIELDS`] may be merged. A field the domain holds no
/// record for is left to the caller's own reference check, which names it as
/// unresolvable rather than as unmergeable.
pub fn field_is_mergeable(field: &Field) -> bool {
    field.namespace != FIELD_NAMESPACE_SYSTEM || MERGEABLE_SYSTEM_FIELDS.contains(&field.field_id.as_str())
}

/// The modelled 402 for a merge that would leave one of its cases holding more
/// than [`MAX_RELATED_ITEMS_PER_CASE`] related items: "The request fails with a
/// `ServiceQuotaExceededException` if, after the merge, either case would
/// exceed the quota for related items per case"
/// (`smithy_query(query="doc", shape="StartMergeCases")`).
pub fn too_many_related_items_after_a_merge(
    case_id: &str,
    items: usize,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "After the merge case '{case_id}' would hold {items} related items; a case holds at \
             most {MAX_RELATED_ITEMS_PER_CASE}."
        ),
    }
}

/// The `RelatedItemType`s a merge's `relatedItemsConfiguration` selects to move
/// from the secondary case to the primary case.
///
/// "Related items of a type that you don't select, or set to false, stay on the
/// closed secondary case. If you omit this parameter, no related items are
/// moved. File, SLA, and work plan related items are never moved"
/// (`smithy_query(query="operation", operation="StartMergeCases")`, member
/// `relatedItemsConfiguration`) — and the four members the configuration
/// declares are exactly the four types that CAN move, so the types it never
/// moves need no exclusion here. Each name is a value of
/// `RELATED_ITEM_TYPE_VALUES`, as [`related_item_content_type`] spells them.
pub fn moved_item_types(configuration: Option<&RelatedItemsConfiguration>) -> Vec<String> {
    let Some(configuration) = configuration else {
        return Vec::new();
    };
    [
        (configuration.comment, "Comment"),
        (configuration.connect_case, "ConnectCase"),
        (configuration.contact, "Contact"),
        (configuration.custom, "Custom"),
    ]
    .into_iter()
    .filter(|(selected, _)| *selected == Some(true))
    .map(|(_, item_type)| item_type.to_string())
    .collect()
}

/// The modelled 400 for a merge of a case into itself.
pub fn merge_of_one_case(case_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "The primary case and the secondary case of a merge must be different cases, but \
             both are '{case_id}'."
        ),
    }
}

/// The modelled 400 for a merge naming a case that is already closed.
pub fn merge_of_a_closed_case(case_id: &str) -> ValidationException {
    ValidationException {
        message: format!("Case '{case_id}' is closed and cannot take part in a merge."),
    }
}

/// The modelled 400 for a merge of two cases that reference different
/// customers: "Both cases must have the same customer, or at least one of them
/// must have no customer" (`StartMergeCases`).
pub fn merge_of_different_customers() -> ValidationException {
    ValidationException {
        message: "The primary case and the secondary case of a merge must have the same \
                  customer, or at least one of them must have no customer."
            .to_string(),
    }
}

/// The modelled 400 for a `fieldsConfiguration` selecting a field a merge may
/// not take ([`field_is_mergeable`]).
pub fn field_cannot_be_merged(field_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Field '{field_id}' cannot be merged; a merge takes the value of a Custom field or \
             of one of the system fields {}.",
            MERGEABLE_SYSTEM_FIELDS.join(", ")
        ),
    }
}

/// The modelled 400 for a `fieldsConfiguration` selecting a field the case it
/// takes values from holds no value for.
pub fn field_not_held_by_case(field_id: &str, case_id: &str) -> ValidationException {
    ValidationException {
        message: format!("Case '{case_id}' does not have field '{field_id}'."),
    }
}

/// The modelled 400 for a `fieldsConfiguration` whose `sourceCaseId` is not a
/// case the merge takes values from: a merge takes values from its secondary
/// case, so a configuration naming any other case selects nothing.
pub fn field_source_is_not_the_secondary_case(source_case_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "fieldsConfiguration.sourceCaseId must name a secondary case of the merge, but names \
             '{source_case_id}'."
        ),
    }
}

/// The modelled 400 for a merge that would leave the primary case holding more
/// than [`MAX_CASE_FIELD_VALUES`] field values.
pub fn too_many_field_values_after_a_merge(case_id: &str, values: usize) -> ValidationException {
    ValidationException {
        message: format!(
            "After the merge case '{case_id}' would hold {values} field values; a case holds at \
             most {MAX_CASE_FIELD_VALUES}."
        ),
    }
}

/// The modelled 409 for a merge naming a case that already takes part in a
/// merge that is running.
pub fn merge_already_running(case_id: &str) -> ConflictException {
    ConflictException {
        message: format!("Case '{case_id}' already takes part in a merge that is in progress."),
    }
}

/// The modelled 409 for a merge naming a case that was already merged into
/// another case as the secondary case: "after the merge, the secondary case
/// ... can't be merged again" (`StartMergeCases`).
pub fn merge_already_completed(case_id: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "Case '{case_id}' was already merged into another case as the secondary case and \
             cannot be merged again."
        ),
    }
}
