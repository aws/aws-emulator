// SPDX-License-Identifier: CC0-1.0

//! Cross-account replication: the link between a PRIMARY secret and the replica
//! secret of it another account holds, and what that link replicates.
//!
//! A cross-account replica is a secret of its own, created by the replica
//! account with `CreateSecret` naming the primary it is to receive from
//! (`CreateSecretRequest$CrossAccountPrimaryARN`, this service's Smithy model):
//! it carries its own name, its own ARN and its own KMS key, and until the
//! primary's owner calls `AssociateCrossAccountReplica` nothing replicates into
//! it — "Pending, which indicates that replication has not yet started"
//! (`StatusType`).
//!
//! So the link is held on BOTH records, each side reporting what that side knows:
//!
//! * the replica holds [`CrossAccountPrimaryLink`], which is the
//!   `CrossAccountPrimary` a read of the replica reports — the primary's ARN and
//!   the status of replication INTO this replica;
//! * the primary holds [`CrossAccountReplicaLink`], which is the one
//!   `CrossAccountReplicas` entry a read of the primary reports — the replica's
//!   ARN, the role replication assumes and the status of replication OUT to it.
//!   "A primary secret can have at most one cross-account replica"
//!   (`AssociateCrossAccountReplica`), so there is at most one.
//!
//! What the association replicates is the primary's VERSIONS: once the link is
//! `InSync` the replica holds the primary's values as its own, re-copied whenever
//! the primary changes ([`ComAmazonawsSecretsmanagerSecretsmanagerState::settle_cross_account_due`]),
//! which is the change the clock owes rather than anything a caller performs. The
//! replica keeps everything that is its OWN — its description, its tags and its
//! resource policy — because it is a secret in another account.
//!
//! Disassociation stops replication and leaves both secrets standing:
//! "Disassociating the replica from the primary does not delete the replica
//! secret or convert the replica secret to a standalone secret. The replica
//! secret remains in the other account and its replication status changes to
//! Detached" (`DisassociateCrossAccountReplica`), so the replica keeps the values
//! already replicated into it and reports `Detached`.

use super::*;

/// `StatusType` for a link no replication has started for yet: "Pending, which
/// indicates that replication has not yet started" (`StatusType`, this service's
/// Smithy model) — the status a cross-account replica carries from its creation
/// until the primary's owner associates it.
pub const CROSS_ACCOUNT_STATUS_PENDING: &str = "Pending";

/// `StatusType` for a link replication no longer flows over: "Detached, which
/// indicates that primary secret is no longer linked to the replica secret"
/// (`StatusType`, this service's Smithy model).
pub const CROSS_ACCOUNT_STATUS_DETACHED: &str = "Detached";

/// The `InvalidParameterException` message for a `CrossAccountReplicaARN` that
/// names a secret outside the primary's Region or partition: "The primary and
/// replica secrets must be in the same Amazon Web Services Region and partition"
/// (`AssociateCrossAccountReplica`, this service's Smithy model).
///
/// ONE fixed text: neither ARN is interpolated into it.
pub const CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE: &str =
    "The primary and replica secrets must be in the same Amazon Web Services Region and \
     partition.";

/// The `LimitExceededException` message for a second cross-account replica: "A
/// primary secret can have at most one cross-account replica"
/// (`AssociateCrossAccountReplica`, this service's Smithy model).
pub const ONE_CROSS_ACCOUNT_REPLICA_MESSAGE: &str =
    "A primary secret can have at most one cross-account replica.";

/// The `ResourceNotFoundException` message for a `CrossAccountReplicaARN` the
/// primary has no association with, which is the resource the disassociation asks
/// about ("Secrets Manager can't find the resource that you asked for",
/// `ResourceNotFoundException`, this service's Smithy model).
///
/// The same form as [`SECRET_NOT_FOUND_MESSAGE`], for the same class of answer:
/// one fixed sentence naming the kind of resource that could not be found.
pub const CROSS_ACCOUNT_REPLICA_NOT_FOUND_MESSAGE: &str =
    "Secrets Manager can't find the specified cross-account replica.";

/// The `InvalidParameterException` message for an operation addressed to a
/// cross-account REPLICA that only the primary's owner can perform: "you must
/// call this operation from the account that owns the primary secret"
/// (`AssociateCrossAccountReplica`, this service's Smithy model).
///
/// The mirror of [`NOT_PERMITTED_ON_REPLICA_MESSAGE`] for the cross-account link:
/// one fixed text naming the class of secret the call is not permitted on and the
/// secret it has to be made on instead.
pub const NOT_PERMITTED_ON_CROSS_ACCOUNT_REPLICA_MESSAGE: &str =
    "Operation not permitted on a cross-account replica secret. Call must be made on the \
     primary secret.";

/// The `InvalidRequestException` message for a `CrossAccountReplicaARN` that names
/// a secret which is not a replica OF THIS primary.
///
/// "The replica secret must already exist in the other Amazon Web Services
/// account. Create the replica with CreateSecret and set its
/// CrossAccountPrimaryARN field set to this primary secret's ARN"
/// (`AssociateCrossAccountReplica`, this service's Smithy model), so the refusal
/// names the two secrets the request put in that relation and what the replica
/// account has to do for it to hold.
#[must_use]
pub fn not_a_cross_account_replica_message(replica_arn: &str, primary_arn: &str) -> String {
    format!(
        "The secret {replica_arn} is not a cross-account replica of the secret {primary_arn}. \
         Create the replica with CreateSecret and set its CrossAccountPrimaryARN to the primary \
         secret's ARN."
    )
}

/// The `InvalidParameterException` message `DeleteSecret` refuses a primary that
/// still has a cross-account replica associated with: "You must disassociate a
/// cross-account replica before you call DeleteSecret for the primary secret. The
/// DeleteSecret call fails if any replicas are still associated with the primary
/// secret" (`DisassociateCrossAccountReplica`, this service's Smithy model).
///
/// Like [`primary_is_replicated_message`], the refusal names the secret it is
/// about and the replica that is in the way, since that is what the caller has to
/// disassociate.
#[must_use]
pub fn primary_has_cross_account_replica_message(arn: &str, replica_arn: &str) -> String {
    format!(
        "You can't delete secret {arn} that still has the cross-account replica {replica_arn} \
         associated with it. You must first disassociate the replica using \
         DisassociateCrossAccountReplica, and then delete the primary secret."
    )
}

/// The `InvalidParameterException` message a create that names a primary to
/// replicate from is refused with when it carries no customer managed key: "you
/// must specify a customer managed KMS key in KmsKeyId"
/// (`CreateSecretRequest$CrossAccountPrimaryARN`, this service's Smithy model).
pub const CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE: &str =
    "A cross-account replica secret must specify a customer managed KMS key in KmsKeyId.";

/// The `InvalidParameterException` message such a create is refused with when it
/// carries a member a cross-account replica cannot have: "You can't include
/// SecretString, SecretBinary, Description, AddReplicaRegions, or Type"
/// (`CreateSecretRequest$CrossAccountPrimaryARN`, this service's Smithy model) —
/// the replica receives all of that from the primary it replicates.
pub const CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE: &str =
    "You can't include SecretString, SecretBinary, Description, AddReplicaRegions, or Type when \
     you create a cross-account replica secret.";

/// Whether two secret ARNs are in the same Region AND partition, which is what
/// cross-account replication requires of the pair
/// ([`CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE`]).
///
/// Read through the tree's ONE ARN parse
/// ([`::service_common::arn::parse`]), so this service cannot disagree with
/// another about what an ARN's partition and Region are. Text that is not a
/// well-formed ARN names no Region or partition to agree with, so it never
/// matches.
#[must_use]
pub fn same_region_and_partition(one: &str, other: &str) -> bool {
    match (
        ::service_common::arn::parse(one),
        ::service_common::arn::parse(other),
    ) {
        (Some(one), Some(other)) => {
            one.partition == other.partition && one.region == other.region
        }
        _ => false,
    }
}

/// The link a cross-account REPLICA holds to the primary it receives from, which
/// is the `CrossAccountPrimary` a read of that replica reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossAccountPrimaryLink {
    /// "The ARN of the primary secret replicating into this replica secret."
    pub primary_secret_arn: String,
    /// The status of replication INTO this replica: [`CROSS_ACCOUNT_STATUS_PENDING`]
    /// until the primary's owner associates it, then
    /// [`REPLICA_STATUS_IN_PROGRESS`] and [`REPLICA_STATUS_IN_SYNC`], and
    /// [`CROSS_ACCOUNT_STATUS_DETACHED`] once the link is gone.
    pub status: String,
    /// "A message that describes the replication status, if Secrets Manager has
    /// more information to provide."
    pub status_message: Option<String>,
    /// The instant the `InProgress` -> `InSync` move the clock owes this link
    /// becomes due; `None` once it has been made and while nothing is replicating.
    /// Stored rather than slept on, so nothing blocks a request.
    pub in_sync_due: Option<SystemTime>,
    /// The primary's `LastChangedDate` as of the last copy made into this replica,
    /// which is what tells a replica that is up to date from one the clock owes a
    /// copy to ([`ComAmazonawsSecretsmanagerSecretsmanagerState::settle_cross_account_due`]);
    /// `None` while nothing has been replicated into it.
    pub replicated_through: Option<SystemTime>,
}

impl CrossAccountPrimaryLink {
    /// The link a replica is CREATED with: the primary is named, and nothing
    /// replicates until that primary's owner associates this replica.
    #[must_use]
    pub fn pending(primary_secret_arn: String) -> Self {
        Self {
            primary_secret_arn,
            status: CROSS_ACCOUNT_STATUS_PENDING.to_string(),
            status_message: None,
            in_sync_due: None,
            replicated_through: None,
        }
    }

    /// Whether the primary is linked to this replica right now: before the
    /// association there is nothing to replicate, and after a disassociation
    /// there is nothing left to replicate.
    #[must_use]
    pub fn is_associated(&self) -> bool {
        self.status != CROSS_ACCOUNT_STATUS_DETACHED && self.status != CROSS_ACCOUNT_STATUS_PENDING
    }

    /// Whether replication into this replica has COMPLETED, which is what makes a
    /// copy of the primary's versions due: a link still `InProgress` is one the
    /// clock has not finished yet, and the replica holds what it held before.
    #[must_use]
    pub fn is_in_sync(&self) -> bool {
        self.status == REPLICA_STATUS_IN_SYNC
    }

    /// Materialise the move to `InSync` once the clock has made it due,
    /// reporting whether anything changed.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.in_sync_due.is_none_or(|due| due > now) {
            return false;
        }
        self.status = REPLICA_STATUS_IN_SYNC.to_string();
        self.status_message = Some(REPLICATION_SUCCEEDED_MESSAGE.to_string());
        self.in_sync_due = None;
        true
    }

    /// The link for the wire.
    #[must_use]
    pub fn to_modeled(&self) -> CrossAccountPrimaryType {
        CrossAccountPrimaryType {
            primary_secret_arn: Some(self.primary_secret_arn.clone()),
            replication_status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
        }
    }
}

/// The link a PRIMARY holds to the cross-account replica it replicates to, which
/// is the one `CrossAccountReplicas` entry a read of the primary reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossAccountReplicaLink {
    /// "The ARN of the replica secret."
    pub replica_secret_arn: String,
    /// "The ARN of the IAM role that Secrets Manager assumes to replicate to this
    /// replica, if one was specified when the replica was associated."
    pub replication_role_arn: Option<String>,
    /// "Whether the replica account must belong to the same Amazon Web Services
    /// organization as the primary account"
    /// (`AssociateCrossAccountReplicaRequest$ReplicaRestriction`), as the
    /// association stated it.
    pub replica_restriction: Option<String>,
    /// The status of replication OUT to the replica.
    pub status: String,
    /// "A message that describes the replication status, if Secrets Manager has
    /// more information to provide."
    pub status_message: Option<String>,
    /// The instant the `InProgress` -> `InSync` move the clock owes this link
    /// becomes due; `None` once it has been made.
    pub in_sync_due: Option<SystemTime>,
}

impl CrossAccountReplicaLink {
    /// The entry an association writes: replication to the replica has started,
    /// which the clock moves to `InSync` ([`settle`]).
    ///
    /// [`settle`]: CrossAccountReplicaLink::settle
    #[must_use]
    pub fn replicating(
        replica_secret_arn: String,
        replication_role_arn: Option<String>,
        replica_restriction: Option<String>,
        at: SystemTime,
    ) -> Self {
        Self {
            replica_secret_arn,
            replication_role_arn,
            replica_restriction,
            status: REPLICA_STATUS_IN_PROGRESS.to_string(),
            status_message: None,
            in_sync_due: at.checked_add(REPLICA_SYNC_DELAY),
        }
    }

    /// Whether the primary is still linked to this replica: an entry the replica
    /// itself detached is kept for reporting and is no longer an association.
    #[must_use]
    pub fn is_associated(&self) -> bool {
        self.status != CROSS_ACCOUNT_STATUS_DETACHED
    }

    /// Materialise the move to `InSync` once the clock has made it due,
    /// reporting whether anything changed.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.in_sync_due.is_none_or(|due| due > now) {
            return false;
        }
        self.status = REPLICA_STATUS_IN_SYNC.to_string();
        self.status_message = Some(REPLICATION_SUCCEEDED_MESSAGE.to_string());
        self.in_sync_due = None;
        true
    }

    /// The entry for the wire.
    #[must_use]
    pub fn to_modeled(&self) -> CrossAccountReplicaEntry {
        CrossAccountReplicaEntry {
            replica_secret_arn: Some(self.replica_secret_arn.clone()),
            replication_role_arn: self.replication_role_arn.clone(),
            replication_status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
        }
    }
}

impl Secret {
    /// Whether this secret is a cross-account replica of a primary another
    /// account holds.
    #[must_use]
    pub fn is_cross_account_replica(&self) -> bool {
        self.cross_account_primary.is_some()
    }

    /// The replica this secret is associated with RIGHT NOW, if any: an entry the
    /// replica detached itself is reported but is no longer an association.
    #[must_use]
    pub fn associated_cross_account_replica(&self) -> Option<&CrossAccountReplicaLink> {
        self.cross_account_replica
            .as_ref()
            .filter(|link| link.is_associated())
    }

    /// Start — or, for a replica already associated, restart — replication to the
    /// replica `replica_secret_arn` names.
    ///
    /// "This operation is idempotent, so you can call it again for the same
    /// replica to repair replication" (`AssociateCrossAccountReplica`, this
    /// service's Smithy model): a repeat replaces the entry, which starts
    /// replication over rather than adding a second one.
    pub fn associate_cross_account_replica(
        &mut self,
        replica_secret_arn: String,
        replication_role_arn: Option<String>,
        replica_restriction: Option<String>,
        at: SystemTime,
    ) {
        self.cross_account_replica = Some(CrossAccountReplicaLink::replicating(
            replica_secret_arn,
            replication_role_arn,
            replica_restriction,
            at,
        ));
        self.last_changed_date = at;
    }

    /// Record that this REPLICA is now receiving from its primary, which is what
    /// the association on the primary's side starts.
    pub fn start_receiving_cross_account(&mut self, at: SystemTime) {
        if let Some(link) = self.cross_account_primary.as_mut() {
            link.status = REPLICA_STATUS_IN_PROGRESS.to_string();
            link.status_message = None;
            link.in_sync_due = at.checked_add(REPLICA_SYNC_DELAY);
        }
        self.last_changed_date = at;
    }

    /// Record that nothing replicates into this REPLICA any more: "its
    /// replication status changes to Detached"
    /// (`DisassociateCrossAccountReplica`, this service's Smithy model). The
    /// values already replicated into it stay: the replica secret remains.
    pub fn stop_receiving_cross_account(&mut self, at: SystemTime) {
        if let Some(link) = self.cross_account_primary.as_mut() {
            link.status = CROSS_ACCOUNT_STATUS_DETACHED.to_string();
            link.status_message = None;
            link.in_sync_due = None;
        }
        self.last_changed_date = at;
    }

    /// Materialise every cross-account transition the clock has already made due
    /// on this secret's own links — the move from `InProgress` to `InSync` on
    /// either side — reporting whether any moved.
    pub fn settle_cross_account(&mut self, now: SystemTime) -> bool {
        let mut settled = false;
        if let Some(link) = self.cross_account_replica.as_mut() {
            settled |= link.settle(now);
        }
        if let Some(link) = self.cross_account_primary.as_mut() {
            settled |= link.settle(now);
        }
        settled
    }

    /// The `CrossAccountPrimary` a read of this secret reports, which only a
    /// cross-account replica has: "This information is only available for
    /// cross-account replica secrets" (`CrossAccountPrimaryType`, this service's
    /// Smithy model).
    #[must_use]
    pub fn cross_account_primary_out(&self) -> Option<CrossAccountPrimaryType> {
        self.cross_account_primary
            .as_ref()
            .map(CrossAccountPrimaryLink::to_modeled)
    }

    /// The `CrossAccountReplicas` a read of this secret reports, which only a
    /// cross-account primary has: "This information is only available for
    /// cross-account primary secrets" (`CrossAccountReplicaEntry`, this service's
    /// Smithy model). Omitted — rather than reported empty — for a secret that
    /// takes no part in cross-account replication, since "Secrets Manager only
    /// returns fields that have a value in the response" (`DescribeSecret`).
    #[must_use]
    pub fn cross_account_replicas_out(&self) -> Option<Vec<CrossAccountReplicaEntry>> {
        self.cross_account_replica
            .as_ref()
            .map(|link| vec![link.to_modeled()])
    }
}

impl ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// The secret an ARN names, wherever it is homed: the one lookup a
    /// cross-account link resolves through, since a link names the other secret by
    /// ARN alone.
    #[must_use]
    pub fn resolve_arn(&self, arn: &str) -> Option<SecretKey> {
        self.arn_index.get(arn).cloned()
    }

    /// Associate the replica at `replica` with the primary at `primary`, writing
    /// the link both records report.
    ///
    /// Both sides are written here, so no path can leave one secret claiming a
    /// link the other does not.
    pub fn associate_cross_account(
        &mut self,
        primary: &SecretKey,
        replica: &SecretKey,
        replication_role_arn: Option<String>,
        replica_restriction: Option<String>,
        at: SystemTime,
    ) -> Option<CrossAccountReplicaLink> {
        let replica_arn = self.secrets.get(replica)?.arn.clone();
        self.secrets.get_mut(primary)?.associate_cross_account_replica(
            replica_arn,
            replication_role_arn,
            replica_restriction,
            at,
        );
        self.secrets
            .get_mut(replica)?
            .start_receiving_cross_account(at);
        self.secrets
            .get(primary)?
            .cross_account_replica
            .clone()
    }

    /// Remove the association the primary at `primary` holds, and record on the
    /// replica that nothing replicates into it any more.
    ///
    /// The replica is left standing with the values already replicated into it:
    /// "Disassociating the replica from the primary does not delete the replica
    /// secret" (`DisassociateCrossAccountReplica`, this service's Smithy model).
    pub fn disassociate_cross_account(&mut self, primary: &SecretKey, at: SystemTime) -> bool {
        let Some(link) = self
            .secrets
            .get_mut(primary)
            .and_then(|secret| secret.cross_account_replica.take())
        else {
            return false;
        };
        if let Some(secret) = self.secrets.get_mut(primary) {
            secret.last_changed_date = at;
        }
        if let Some(key) = self.resolve_arn(&link.replica_secret_arn) {
            if let Some(replica) = self.secrets.get_mut(&key) {
                if replica.cross_account_primary.is_some() {
                    replica.stop_receiving_cross_account(at);
                }
            }
        }
        true
    }

    /// Unlink the cross-account REPLICA at `replica` from its primary, answering
    /// its ARN; `None` for a key that names no cross-account replica.
    ///
    /// "To convert a detached replica into a standalone secret, call
    /// StopReplicationToReplica on the replica secret"
    /// (`DisassociateCrossAccountReplica`, this service's Smithy model): the
    /// secret keeps its ARN and the values replicated into it, and stops being a
    /// replica. The primary's entry for it records that replication "was stopped
    /// by the replica" ([`CROSS_ACCOUNT_STATUS_DETACHED`]) rather than
    /// disappearing, so the primary's owner can still see what became of it.
    pub fn detach_cross_account_replica(
        &mut self,
        replica: &SecretKey,
        at: SystemTime,
    ) -> Option<String> {
        let link = self.secrets.get_mut(replica)?.cross_account_primary.take()?;
        let replica_secret = self.secrets.get_mut(replica)?;
        replica_secret.last_changed_date = at;
        let arn = replica_secret.arn.clone();
        if let Some(key) = self.resolve_arn(&link.primary_secret_arn) {
            if let Some(primary) = self.secrets.get_mut(&key) {
                if let Some(entry) = primary
                    .cross_account_replica
                    .as_mut()
                    .filter(|entry| entry.replica_secret_arn == arn)
                {
                    entry.status = CROSS_ACCOUNT_STATUS_DETACHED.to_string();
                    entry.status_message = None;
                    entry.in_sync_due = None;
                    primary.last_changed_date = at;
                }
            }
        }
        Some(arn)
    }

    /// Materialise every cross-account replication the clock has already made
    /// due, across every secret the store holds: the status moves on both sides of
    /// a link, and the COPY of the primary's versions into a replica that is
    /// `InSync` and has not received the primary's latest change yet.
    ///
    /// Like [`expire_due`], this is a change that was owed at this instant whether
    /// or not anybody called — `AssociateCrossAccountReplica` "starts replication
    /// to this replica", and replication is what carries every later change of the
    /// primary — so an operation that reports a secret performs it before it
    /// answers and never reports a replica as still receiving the previous value.
    ///
    /// The values stay sealed for the ARN they were sealed under
    /// ([`Secret::sealed_under`]), which is the only encryption context they open
    /// under, so the replica serves exactly what the primary holds.
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub fn settle_cross_account_due(&mut self, now: SystemTime) {
        for secret in self.secrets.values_mut() {
            secret.settle_cross_account(now);
        }
        // The pairs a copy may be owed for, read before anything is written: a
        // replica and the primary it receives from are two records of this store,
        // and only one of them can be borrowed mutably at a time.
        let receiving: Vec<(SecretKey, String)> = self
            .secrets
            .iter()
            .filter(|(_, secret)| {
                secret
                    .cross_account_primary
                    .as_ref()
                    .is_some_and(CrossAccountPrimaryLink::is_in_sync)
            })
            .filter_map(|(key, secret)| {
                secret
                    .cross_account_primary
                    .as_ref()
                    .map(|link| (key.clone(), link.primary_secret_arn.clone()))
            })
            .collect();
        for (replica_key, primary_arn) in receiving {
            let Some(primary_key) = self.resolve_arn(&primary_arn) else {
                continue;
            };
            let Some(primary) = self.secrets.get(&primary_key) else {
                continue;
            };
            let changed_at = primary.last_changed_date;
            let already = self
                .secrets
                .get(&replica_key)
                .and_then(|replica| replica.cross_account_primary.as_ref())
                .and_then(|link| link.replicated_through);
            if already == Some(changed_at) {
                continue;
            }
            let versions = primary.versions.clone();
            let sealed_under = primary.value_context_arn().to_string();
            let Some(replica) = self.secrets.get_mut(&replica_key) else {
                continue;
            };
            replica.versions = versions;
            replica.sealed_under = Some(sealed_under);
            // The replica was modified — by the replication, at this instant —
            // which is what `LastChangedDate` reports ("the last date and time
            // that this secret was modified in any way", this service's Smithy
            // model). The primary's own stamp is what the copy is MARKED with
            // instead, so a change already replicated is not copied again.
            replica.last_changed_date = now;
            if let Some(link) = replica.cross_account_primary.as_mut() {
                link.replicated_through = Some(changed_at);
            }
        }
    }
}
