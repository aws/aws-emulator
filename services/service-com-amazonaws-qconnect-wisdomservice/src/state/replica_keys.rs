// SPDX-License-Identifier: CC0-1.0

//! The replica-Region KMS keys a resource's encryption configuration carries, and the one
//! place this service registers one.
//!
//! `RegisterAssistantReplicaKey` and `RegisterKnowledgeBaseReplicaKey` write the same
//! member of the same shape — `ServerSideEncryptionConfiguration$replicaKmsKeys`, "the
//! list of KMS keys used to encrypt data for this resource's replicas in other Amazon Web
//! Services Regions"
//! (`smithy_query(query="shape",shape="ServerSideEncryptionConfiguration")`) — so both
//! register one through [`register_replica_kms_key`] and neither decides on its own what
//! a registration does.

use super::*;
use crate::ops::types::ReplicaKmsKey;

/// The most replica-Region KMS keys one resource's encryption configuration carries: the
/// bound the model puts on `ServerSideEncryptionConfiguration$replicaKmsKeys`
/// (`smithy_query(query="shape",shape="ServerSideEncryptionConfiguration")`: a list of 1
/// to 5 entries), so a resource past it could not be reported at all.
pub const MAX_REPLICA_KMS_KEYS: usize = 5;

/// The sentence a registration past [`MAX_REPLICA_KMS_KEYS`] is refused with: the RULE —
/// the ceiling and the resource it is per — carrying nothing from the request, the way
/// [`message_ai_prompt_quota`] states the sibling ceiling. `resource` is the resource the
/// keys are registered on, spelled as the API names it.
pub fn message_replica_kms_key_quota(resource: &str) -> String {
    format!(
        "The maximum number of replica Region KMS keys ({MAX_REPLICA_KMS_KEYS}) for this \
         {resource} has been reached."
    )
}

/// Register `key` as a replica-Region KMS key of `configuration`, answering whether it is
/// registered afterwards.
///
/// A key already registered is left where it is: the operations that call this are
/// modelled `@idempotent`, so registering the same replica key again registers it once and
/// keeps the order the keys were first registered in, rather than reporting the same ARN
/// twice. A resource whose create named no encryption configuration gains one carrying the
/// registered key alone — `kmsKeyId` is an optional member of the shape, and the key the
/// caller registered is what the resource now holds.
///
/// `false` — nothing registered — when the configuration already carries
/// [`MAX_REPLICA_KMS_KEYS`] other keys, which is what keeps the stored list inside the
/// bound the model reports it under.
pub fn register_replica_kms_key(
    configuration: &mut Option<ServerSideEncryptionConfiguration>,
    key: &ReplicaKmsKey,
) -> bool {
    let configuration = configuration.get_or_insert_with(ServerSideEncryptionConfiguration::default);
    let keys = configuration.replica_kms_keys.get_or_insert_with(Vec::new);
    if keys.iter().any(|registered| registered.arn == key.arn) {
        return true;
    }
    if keys.len() >= MAX_REPLICA_KMS_KEYS {
        return false;
    }
    keys.push(key.clone());
    true
}
