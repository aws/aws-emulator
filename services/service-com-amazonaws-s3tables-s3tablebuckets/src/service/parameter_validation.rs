// SPDX-License-Identifier: CC0-1.0

//! The refusal this service does NOT word itself: a value a modelled constraint
//! rules out on a member the service states no sentence of its own for.
//!
//! Two layers refuse a request before any handler runs, and a caller tells them
//! apart by the error code:
//!
//! * the SERVICE refuses what it validates itself — the names a create gives a
//!   resource, the ARNs an operation is addressed by, the encryption and format
//!   members it will not act on — with the one client-input error its model
//!   declares (`BadRequestException`) and one fixed sentence per class, which
//!   [`super::fixed_sentence`] is the list of;
//! * every OTHER modelled constraint is checked by the parameter validator in
//!   front of the service, which answers under its own code — a code that names
//!   no shape of this service's model, which is exactly what tells a client the
//!   request was refused for the SHAPE of a parameter rather than by a rule of
//!   the service.
//!
//! Nothing about that second refusal is worded here. The code, the envelope, the
//! member paths, the requirement per constraint class, the `@pattern` rendering,
//! the ORDER an enumerated type's value set is listed in and the clause a
//! constraint declared on a COLLECTION's elements is stated in are the whole
//! workspace's ([`service_common::validation_message`]), so no two crates can
//! word one constraint two ways. This module is only WHICH of the choices that
//! module leaves to a service this one makes.

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::Violation;
use smithy_server_runtime::{Refusal, Refused};

/// How this service answers `refusal` when it words none of its violations
/// itself, or `None` when there is nothing to state (which leaves the
/// framework's own wording).
///
/// Three choices are this service's, and each is about one clause rather than
/// about the envelope:
///
/// * EVERY clause withholds the value half: its refusals are recorded stating
///   the member path and the requirement alone — `Value at '<member path>'
///   failed to satisfy constraint: <requirement>` — whatever constraint the
///   value broke, so the same refusal reads identically whatever was sent and
///   nothing of the request comes back;
/// * a constraint declared on a COLLECTION's element shape is stated AT that
///   collection member, carrying the element shape's whole declared constraint
///   set, rather than at the element that happened to break one of them: the
///   constraint belongs to the member the model declares it on, so the clause
///   neither names a position nor moves with one, and the set it lists is read
///   off the model ([`INPUT_SCHEMAS`]) rather than off the one constraint a
///   value failed;
/// * the value set an ENUMERATED type's requirement lists is stated
///   lexicographically ([`sorted_enum_sets`]), whatever order the model declares
///   that type's members in.
pub fn refusal(refusal: &Refusal) -> Option<Refused> {
    service_common::validation_message::aws_validation_refusal_over_elements_withholding(
        &sorted_enum_sets(refusal),
        INPUT_SCHEMAS,
        // The `@pattern` rendering this service's refusals are recorded in: the
        // expression without the anchors that state the member's position, which
        // is the dialect the shared layer's `unanchored` states.
        service_common::validation::unanchored,
        |_| true,
    )
}

/// Every operation INPUT shape of this service, as the schemas codegen derives
/// from the Smithy model.
///
/// What they are read for: the clause stated at a collection member lists the
/// whole constraint set the model declares on that collection's ELEMENT shape,
/// and a refusal carries only the constraints a value actually broke. Codegen
/// attaches a collection member's element constraints to that member's schema,
/// which is where the shared rendering reads them — and where the validating
/// walk read them to refuse the request.
///
/// ALL of them, not the ones one refusal happens to name: the rule holds for
/// every constrained collection of every operation. `grep -h
/// 'super::types::.*_INPUT_SCHEMA' src/ops/*.rs` names exactly these, one per
/// operation file; one missing from this list is not a wrong clause, only a
/// clause stating the constraint that failed rather than the element shape's
/// whole set.
const INPUT_SCHEMAS: &[&Schema] = {
    use crate::ops::types;

    &[
        types::COMPLETE_TABLE_MIGRATION_INPUT_SCHEMA,
        types::CREATE_NAMESPACE_INPUT_SCHEMA,
        types::CREATE_TABLE_INPUT_SCHEMA,
        types::CREATE_TABLE_BUCKET_INPUT_SCHEMA,
        types::DELETE_NAMESPACE_INPUT_SCHEMA,
        types::DELETE_TABLE_INPUT_SCHEMA,
        types::DELETE_TABLE_BUCKET_INPUT_SCHEMA,
        types::DELETE_TABLE_BUCKET_ENCRYPTION_INPUT_SCHEMA,
        types::DELETE_TABLE_BUCKET_METRICS_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_TABLE_BUCKET_POLICY_INPUT_SCHEMA,
        types::DELETE_TABLE_BUCKET_REPLICATION_INPUT_SCHEMA,
        types::DELETE_TABLE_POLICY_INPUT_SCHEMA,
        types::DELETE_TABLE_REPLICATION_INPUT_SCHEMA,
        types::DISASSOCIATE_TABLE_REPLICATION_INPUT_SCHEMA,
        types::GET_NAMESPACE_INPUT_SCHEMA,
        types::GET_TABLE_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_ENCRYPTION_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_LOGGING_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_MAINTENANCE_CONFIGURATION_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_METRICS_CONFIGURATION_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_POLICY_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_REPLICATION_INPUT_SCHEMA,
        types::GET_TABLE_BUCKET_STORAGE_CLASS_INPUT_SCHEMA,
        types::GET_TABLE_ENCRYPTION_INPUT_SCHEMA,
        types::GET_TABLE_MAINTENANCE_CONFIGURATION_INPUT_SCHEMA,
        types::GET_TABLE_MAINTENANCE_JOB_STATUS_INPUT_SCHEMA,
        types::GET_TABLE_METADATA_LOCATION_INPUT_SCHEMA,
        types::GET_TABLE_MIGRATION_INPUT_SCHEMA,
        types::GET_TABLE_POLICY_INPUT_SCHEMA,
        types::GET_TABLE_RECORD_EXPIRATION_CONFIGURATION_INPUT_SCHEMA,
        types::GET_TABLE_RECORD_EXPIRATION_JOB_STATUS_INPUT_SCHEMA,
        types::GET_TABLE_REPLICATION_INPUT_SCHEMA,
        types::GET_TABLE_REPLICATION_STATUS_INPUT_SCHEMA,
        types::GET_TABLE_STORAGE_CLASS_INPUT_SCHEMA,
        types::LIST_NAMESPACES_INPUT_SCHEMA,
        types::LIST_TABLES_INPUT_SCHEMA,
        types::LIST_TABLE_BUCKETS_INPUT_SCHEMA,
        types::LIST_TABLE_MIGRATIONS_INPUT_SCHEMA,
        types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_ENCRYPTION_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_LOGGING_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_MAINTENANCE_CONFIGURATION_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_METRICS_CONFIGURATION_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_POLICY_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_REPLICATION_INPUT_SCHEMA,
        types::PUT_TABLE_BUCKET_STORAGE_CLASS_INPUT_SCHEMA,
        types::PUT_TABLE_MAINTENANCE_CONFIGURATION_INPUT_SCHEMA,
        types::PUT_TABLE_POLICY_INPUT_SCHEMA,
        types::PUT_TABLE_RECORD_EXPIRATION_CONFIGURATION_INPUT_SCHEMA,
        types::PUT_TABLE_REPLICATION_INPUT_SCHEMA,
        types::RENAME_TABLE_INPUT_SCHEMA,
        types::START_TABLE_MIGRATION_INPUT_SCHEMA,
        types::SYNC_TABLE_MIGRATION_INPUT_SCHEMA,
        types::TAG_RESOURCE_INPUT_SCHEMA,
        types::UNTAG_RESOURCE_INPUT_SCHEMA,
        types::UPDATE_TABLE_METADATA_LOCATION_INPUT_SCHEMA,
        types::UPDATE_TABLE_MIGRATION_INPUT_SCHEMA,
    ]
};

/// `refusal` with every enumerated type's value set stated in lexicographic
/// order, and anything else handed on unchanged.
///
/// A refusal of a value outside an enumerated type names the values that type
/// admits, and this service states that set sorted rather than in the order the
/// model happens to declare it in. The order belongs to the TYPE, so one enum
/// reads the same whichever member, operation or depth carried the value, and a
/// type the model gains or reorders needs nothing here.
///
/// The re-ordering happens BEFORE the envelope is rendered, so the clause — the
/// member path, the requirement's wording and the brackets around the set — is
/// still written in the one place the whole workspace writes it
/// (`service_common::validation_message::in_sorted_enum_order`). It is applied on
/// THIS path alone: the sentences the service words itself
/// ([`super::stated_sentence`]) read the class off the set the MODEL declares.
fn sorted_enum_sets(refusal: &Refusal) -> Refusal {
    match refusal {
        Refusal::Invalid(violations) => Refusal::Invalid(
            violations
                .iter()
                .map(service_common::validation_message::in_sorted_enum_order)
                .collect::<Vec<Violation>>(),
        ),
        other => other.clone(),
    }
}
