// SPDX-License-Identifier: CC0-1.0

//! The restore METADATA a start has to carry for the type of resource it restores.
//!
//! # The rule this module implements
//!
//! `StartRestoreJob`'s `Metadata` is "a set of metadata key-value pairs ... values in
//! addition to those provided by `GetRecoveryPointRestoreMetadata` might be required to
//! restore a resource. For example, you might need to provide a new resource name if the
//! original already exists" (the model's own doc for the member). For a resource type whose
//! restore cannot be performed without such a value, metadata that carries no entry for it
//! is a value AWS Backup REFUSES synchronously, with the parameter-value class and the
//! sentence for that key: the restore is not started, so nothing is stored and no restore job
//! is listed for the recovery point.
//!
//! The refusal is taken BEFORE the job is stored, which is what keeps it a refusal rather
//! than a failed restore: a start the service accepts stores a job every read answers from,
//! while a start it refuses leaves no trace at all.
//!
//! One entry per resource type that requires a metadata key; the first is Amazon DynamoDB,
//! whose restore creates a table under the name the metadata gives
//! ([`super::METADATA_TARGET_TABLE_NAME`]). A type with no entry here requires no particular
//! key, so its metadata is whatever the restore of that type reads.

use std::collections::BTreeMap;

use crate::ops::types::InvalidParameterValueException;

/// The lead every restore-metadata refusal opens with.
const MESSAGE_INVALID_RESTORE_METADATA: &str = "Invalid restore metadata.";

/// What a restore of one resource type cannot be started without: the metadata key, and the
/// sentence AWS words its absence with.
///
/// SOURCE: the per-resource metadata AWS publishes for a restore (AWS Backup, *Metadata for
/// Amazon DynamoDB*, which the model's doc for `StartRestoreJob$Metadata` links) — the key a
/// restore of that type has no target without.
const REQUIRED_RESTORE_METADATA: &[(&str, &str, &str)] = &[(
    super::RESOURCE_TYPE_DYNAMODB,
    super::METADATA_TARGET_TABLE_NAME,
    "Target table name must be provided with restore metadata.",
)];

/// Why the `Metadata` of a restore of `resource_type` from `recovery_point_arn` cannot start
/// one, or `None` when it carries everything that type requires.
///
/// The keys are read through the same case-insensitive step the restore itself reads them
/// with ([`super::metadata_value`]), so a request this accepts is one the restore can perform
/// and a request it refuses is one the restore could not.
#[must_use]
pub fn rejected_restore_metadata(
    resource_type: Option<&str>,
    recovery_point_arn: &str,
    metadata: &BTreeMap<String, String>,
) -> Option<InvalidParameterValueException> {
    let resource_type = resource_type?;
    let (_, key, sentence) = REQUIRED_RESTORE_METADATA
        .iter()
        .find(|(named, _, _)| *named == resource_type)?;
    if super::metadata_value(metadata, key).is_some() {
        return None;
    }
    Some(InvalidParameterValueException {
        message: Some(format!(
            "{MESSAGE_INVALID_RESTORE_METADATA} {sentence} Restore metadata validation failed \
             for {resource_type} with recoveryPoint {recovery_point_arn}."
        )),
        ..Default::default()
    })
}
