// SPDX-License-Identifier: CC0-1.0

//! REPLICATION of a DB instance's automated backups to another Amazon Web Services Region:
//! what `StartDBInstanceAutomatedBackupsReplication` enables and
//! `StopDBInstanceAutomatedBackupsReplication` stops.
//!
//! `StartDBInstanceAutomatedBackupsReplication` "enables replication of automated backups to
//! a different Amazon Web Services Region", naming the instance whose backups are replicated
//! by its ARN (`SourceDBInstanceArn`) — so the call is made against the DESTINATION Region's
//! endpoint and the source instance lives in another Region, which is why one record is keyed
//! by the destination Region and carries the source instance's own Region inside it.
//!
//! A replicated automated backup is an automated backup like any other, so it is reported by
//! `DescribeDBInstanceAutomatedBackups` in the Region it was replicated TO, under the statuses
//! that report publishes: `creating` while it is "waiting for the first automated snapshot to
//! be available", `active` once it has one, and `retained` — which that report's `status`
//! filter documents as "automated backups for deleted instances AND AFTER BACKUP REPLICATION
//! IS STOPPED" — once the replication has been stopped. Those three statuses are the whole
//! vocabulary; none is invented here.
//!
//! The SOURCE's half of the relationship is not stored on the source: its automated backup
//! reports the replications of it (`DBInstanceAutomatedBackupsReplications`, "the list of
//! replications to different Amazon Web Services Regions associated with the automated
//! backup"), read off the records that name it
//! ([`ComAmazonawsRdsAmazonrdsv19State::automated_backups_replications_of`]), so the two
//! halves can never disagree.
//!
//! The records are bounded: at most [`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`] per
//! destination Region, the same quota the retained backups of deleted instances are held to —
//! "the retained automated backups quota is the same as your DB instance quota" — which is
//! the `DBInstanceAutomatedBackupQuotaExceededFault` this operation models. A record a stop
//! left behind keeps its slot until `DeleteDBInstanceAutomatedBackup` removes it, which is
//! the reclamation path for the backups a stopped replication retains, exactly as it is for
//! the backups a deleted instance leaves.

use super::*;

/// What a `SourceDBInstanceArn` NAMES: the Region the source DB instance is in, and the
/// identifier this store keys it by.
///
/// The member is "the Amazon Resource Name (ARN) of the source DB instance for the replicated
/// automated backups, for example, arn:aws:rds:us-west-2:123456789012:db:mydatabase", and the
/// source is in another Region than the call, so BOTH fields of the ARN are read rather than
/// only the identifier — which is what the folds the same-Region members use read
/// ([`arn_resource_name_of`]).
#[derive(Debug, Clone)]
pub struct SourceDbInstance {
    /// The source instance's Region, from the ARN's Region field, folded the way this store
    /// keys a Region: the published lowercase region-id form every request's own scope is
    /// held to (`request_region`).
    pub region: String,
    /// The source instance's identifier, folded the way this store keys one: "stored as a
    /// lowercase string".
    pub identifier: String,
}

/// The source DB instance `named` names, looked for from a request signed for
/// `request_region`, or `None` when `named` is not the ARN of a DB instance at all — read
/// through the ONE RDS ARN reading this crate has ([`parse_rds_resource_arn`]), so a value
/// this service would not mint an ARN of names no source here either.
///
/// The region is the ARN's own, which is the whole point of naming a cross-region source by
/// ARN; an ARN whose region segment is EMPTY names no region other than the one the request
/// was signed for, so that is where its source is looked for — the rule the copies' source
/// snapshot is resolved by (`snapshot_copy_source_not_found_message`). Both halves of what
/// the ARN names are folded the way this store keys them, so a source is resolved by its ARN
/// however the request spells it.
pub fn source_db_instance_of(named: &str, request_region: &str) -> Option<SourceDbInstance> {
    let parsed = parse_rds_resource_arn(named)?;
    if parsed.resource_type != DB_INSTANCE_ARN_RESOURCE_TYPE {
        return None;
    }
    let region = if parsed.region.is_empty() {
        request_region
    } else {
        parsed.region
    };
    Some(SourceDbInstance {
        region: region.to_ascii_lowercase(),
        identifier: parsed.identifier.to_ascii_lowercase(),
    })
}

/// The ARN resource type of a DB INSTANCE, as Amazon RDS spells it in an ARN
/// (`arn:aws:rds:<region>:<account>:db:<identifier>`).
pub const DB_INSTANCE_ARN_RESOURCE_TYPE: &str = "db";

/// What a request whose `SourceDBInstanceArn` is not a DB instance ARN is refused with: the
/// member is "the Amazon Resource Name (ARN) of the source DB instance", so a value that is
/// no such ARN names no source to replicate, which this Query API answers
/// `InvalidParameterValue`. The sentence names the member and the value the request carried.
pub fn invalid_source_db_instance_arn_message(source_arn: &str) -> String {
    format!(
        "The parameter SourceDBInstanceArn must name the ARN of a DB instance; got \
         {source_arn}."
    )
}

/// What a request that would replicate a source instance's automated backups INTO the Region
/// that instance is already in is refused with.
///
/// `StartDBInstanceAutomatedBackupsReplication` "enables replication of automated backups to a
/// DIFFERENT Amazon Web Services Region", and the Region a call is made to is the destination,
/// so a source in that same Region has nowhere to be replicated to: the automated backup it
/// already keeps there is the one `DescribeDBInstanceAutomatedBackups` reports `active`. The
/// sentence names the ARN the request carried and states the rule.
pub fn same_region_backup_replication_message(source_arn: &str) -> String {
    format!(
        "The source DB instance {source_arn} is located in the Region this request was made \
         to. Automated backups are replicated to a different AWS Region."
    )
}

/// What `StopDBInstanceAutomatedBackupsReplication` refuses a call that names no replication
/// INTO the Region the call was made to with.
///
/// The question that operation asks of the Region of the call — is the instance the ARN names
/// replicating its automated backups into THIS Region? — has one state it can refuse:
/// "nothing is replicating here", which is the modelled `InvalidDBInstanceStateFault` ("the
/// DB instance isn't in a valid state"). It is the answer whether the replication was never
/// enabled, has already been stopped, runs into another Region, or names an instance no Region
/// holds at all: none of them is a replication into this Region, and the question is asked of
/// this Region's own records rather than of the source instance — which is why an ARN naming
/// nothing is answered here and not with the `DBInstanceNotFoundFault` the START leg, which
/// has to read that instance, answers ([`source_db_instance_not_found_message`]). The
/// sentence's one variable part is the request's `SourceDBInstanceArn`, reproduced whole.
pub fn backup_replication_not_to_this_region_message(source_arn: &str) -> String {
    format!("DB Instance {source_arn} is not replicating to the current region.")
}

/// A REPLICATED automated backup: the automated backups of one DB instance, replicated into
/// another Amazon Web Services Region.
///
/// It is the destination Region's record of the replication — the Region's own copy of the
/// backups — so the properties it reports are the SOURCE instance's as of the instant
/// replication was enabled, held in the one copy of an instance's backup properties this
/// crate has ([`RetainedInstanceBackupRecord`]), and the members that belong to the
/// replication itself are its own.
#[derive(Debug, Clone)]
pub struct ReplicatedInstanceBackupRecord {
    /// The DESTINATION Region: the one the backups are replicated to, which is the Region the
    /// enabling call was made to and the Region this record is keyed by. The backup's own
    /// `DBInstanceAutomatedBackupsArn` is in it.
    pub destination_region: String,
    /// The SOURCE instance's properties, copied when replication was enabled — the same copy
    /// a delete that retains an instance's backup keeps, for the same reason: the backup must
    /// still report the instance it is of after that instance is gone. Its `retained_at` is
    /// the instant of the copy, which is the instant this replication was enabled
    /// ([`Self::started_at`]).
    pub source: RetainedInstanceBackupRecord,
    /// `BackupRetentionPeriod`: "the retention period for the replicated automated backups",
    /// as the request named it, else the source instance's own period.
    pub backup_retention_period: i32,
    /// `KmsKeyId`: "the Amazon Web Services KMS key identifier for encryption of the
    /// replicated automated backups ... in the destination Amazon Web Services Region", as the
    /// request named it, else the key the source instance's storage is encrypted under.
    /// `None` for a source whose storage is not encrypted, which has no key encrypting it.
    pub kms_key_id: Option<String>,
    /// `Tags`: "a list of tags to associate with the replicated automated backups" — the
    /// replicated backups' own tags, which is why they are not the source instance's.
    pub tags: Vec<Tag>,
    /// The status this backup reports, one of the three
    /// `DescribeDBInstanceAutomatedBackups` publishes: [`DB_INSTANCE_AUTOMATED_BACKUP_STATUS_CREATING`]
    /// while it waits for its first snapshot, [`DB_INSTANCE_AUTOMATED_BACKUP_STATUS_ACTIVE`]
    /// once it has one, and [`DB_INSTANCE_AUTOMATED_BACKUP_STATUS_RETAINED`] after the
    /// replication is stopped.
    pub status: String,
    /// The transition the clock owes this record: `creating` -> `active`, once the first
    /// replicated snapshot is due. `None` for a record in a settled status.
    pub pending_status: Option<PendingStatus>,
    /// When the replication was STOPPED, for a record whose replication has been: from then
    /// on nothing is replicated into it, so its restore window reaches no further than this
    /// instant. `None` while it is still replicating.
    pub stopped_at: Option<::aws_smithy_types::DateTime>,
}

impl ReplicatedInstanceBackupRecord {
    /// The record a `StartDBInstanceAutomatedBackupsReplication` into `destination_region`
    /// stores for `instance`: the source's properties as of now, the settings the request
    /// carried, and the `creating` status a backup "waiting for the first automated snapshot
    /// to be available" reports — which the clock owes `active`.
    pub fn started(
        destination_region: &str,
        instance: &DbInstanceRecord,
        backup_retention_period: Option<i32>,
        kms_key_id: Option<String>,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            destination_region: destination_region.to_string(),
            source: RetainedInstanceBackupRecord::of(instance),
            // "Default: Inherits from the source DB instance" is how this member's siblings
            // are documented across the API, and the backups being replicated are the ones
            // the source keeps, so the source's own period is what they are retained for
            // when the request names none.
            backup_retention_period: backup_retention_period
                .unwrap_or(instance.backup_retention_period),
            // A replicated backup of an encrypted source is encrypted; the request names the
            // key in the destination Region, and an unencrypted source has no key at all.
            kms_key_id: kms_key_id.or_else(|| instance.kms_key_id.clone()),
            tags,
            status: DB_INSTANCE_AUTOMATED_BACKUP_STATUS_CREATING.to_string(),
            pending_status: Some(PendingStatus::in_a_moment(
                DB_INSTANCE_AUTOMATED_BACKUP_STATUS_ACTIVE,
            )),
            stopped_at: None,
        }
    }

    /// When this replication was enabled: the instant the source's properties were copied,
    /// which is the earliest instant the destination Region holds any of its data for.
    pub fn started_at(&self) -> ::aws_smithy_types::DateTime {
        self.source.retained_at
    }

    /// Stop replicating into this Region: the backups already replicated stay, "retained"
    /// being the status the report documents "after backup replication is stopped", and the
    /// window they cover reaches no further than this instant because nothing is replicated
    /// after it. `DeleteDBInstanceAutomatedBackup` is what removes them from then on.
    pub fn stop(&mut self) {
        self.status = DB_INSTANCE_AUTOMATED_BACKUP_STATUS_RETAINED.to_string();
        self.pending_status = None;
        self.stopped_at = Some(now_as_wire_timestamp());
    }

    /// Whether this record is a replication IN FORCE — one the source instance's automated
    /// backup reports as a replication of itself — rather than the backups a stopped
    /// replication left behind.
    pub fn is_replicating(&self) -> bool {
        self.stopped_at.is_none()
    }

    /// Apply a change whose due instant has passed.
    ///
    /// Unlike the records that can be removed by the clock, this one never is: stopping a
    /// replication RETAINS the backups it had taken, and only
    /// `DeleteDBInstanceAutomatedBackup` removes an automated backup — so nothing is answered
    /// back.
    pub(super) fn settle(&mut self, now: Instant) {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            // The clock owes this record no removal, and the change was there a line ago
            // under this one `&mut self`: both are answered as "nothing was due" rather than
            // with a panic in a request path.
            Some(PendingChange::Removal) | None => {}
        }
    }

    /// `DBInstanceAutomatedBackupsArn`: "the Amazon Resource Name (ARN) for the replicated
    /// automated backups" — the ARN of THESE backups, which are in the DESTINATION Region,
    /// derived from the source instance's resource id through the one auto-backup ARN form
    /// this crate has ([`instance_automated_backups_arn`]).
    pub fn automated_backups_arn(&self) -> String {
        instance_automated_backups_arn(&self.destination_region, &self.source.dbi_resource_id)
    }

    /// `RestoreWindow` for these backups: "the earliest and latest time a DB instance can be
    /// restored to", in the destination Region.
    ///
    /// The window reaches back over the retention period these backups are kept for, never
    /// before replication began — the destination Region holds nothing from before that — and
    /// up to the last instant anything was replicated into it, which is the instant the
    /// replication was stopped or, for one still replicating, now.
    ///
    /// `None` while the backups are still `creating`, "waiting for the first automated
    /// snapshot to be available": there is no instant they can be restored to yet, so no
    /// window is reported rather than one nothing covers. The member itself is still published
    /// — EMPTY, carrying neither time — which is the wire form a backup with nothing to restore
    /// to takes ([`DbInstanceRecord::automated_backup_to_wire`]).
    pub fn restore_window(&self) -> Option<RestoreWindow> {
        if self.status == DB_INSTANCE_AUTOMATED_BACKUP_STATUS_CREATING {
            return None;
        }
        let latest = self.stopped_at.unwrap_or_else(now_as_wire_timestamp);
        let window = restore_window(latest, self.started_at(), self.backup_retention_period);
        let covers_nothing = window
            .earliest_time
            .is_some_and(|earliest| earliest.as_secs_f64() >= latest.as_secs_f64());
        match covers_nothing {
            true => None,
            false => Some(window),
        }
    }

    /// Project these backups onto the modelled `DBInstanceAutomatedBackup` shape.
    ///
    /// Every property of the DB instance the backups are OF is read from the one copy of them
    /// this record holds, so a replicated backup and the retained backup of the same instance
    /// cannot disagree about the instance; the members that belong to the REPLICATION — its
    /// own ARN in the destination Region, its status, the window it covers, the period it is
    /// kept for, the key it is encrypted under and its own tags — are this record's.
    pub fn to_wire(&self) -> DBInstanceAutomatedBackup {
        DBInstanceAutomatedBackup {
            db_instance_automated_backups_arn: Some(self.automated_backups_arn()),
            status: Some(self.status.clone()),
            restore_window: Some(self.restore_window().unwrap_or_default()),
            backup_retention_period: Some(self.backup_retention_period),
            kms_key_id: self.kms_key_id.clone(),
            tag_list: Some(self.tags.clone()),
            // `Region` stays the source instance's — "the Amazon Web Services Region
            // associated with the automated backup" is the Region the instance the backups
            // are of is in — and so does `DBInstanceArn`, which names that instance. Both
            // come from the source copy below, as does every storage and engine property.
            ..self.source.to_wire()
        }
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The automated backups REPLICATED INTO `region`, in key order — the order
    /// `DescribeDBInstanceAutomatedBackups` pages them in.
    ///
    /// Reads one Region's ordered range, bounded by
    /// [`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`].
    pub fn replicated_instance_backups_in(
        &self,
        region: &str,
    ) -> Vec<&ReplicatedInstanceBackupRecord> {
        range_in(&self.db_instance_automated_backup_replications, region)
    }

    /// The automated backups replicated into `region` of the source instance whose resource
    /// id is `dbi_resource_id` — the ONE record a source instance has in one destination
    /// Region, since that id "can't be changed and ... is unique to an Amazon Web Services
    /// Region".
    pub fn replicated_instance_backup_of(
        &self,
        region: &str,
        dbi_resource_id: &str,
    ) -> Option<&ReplicatedInstanceBackupRecord> {
        self.db_instance_automated_backup_replications
            .get(&(region.to_string(), dbi_resource_id.to_string()))
    }

    /// The automated backups replicated into `region` FROM the source DB instance a
    /// `SourceDBInstanceArn` names, resolved from the ARN alone: the record's own copy of the
    /// source carries that instance's Region and identifier
    /// ([`RetainedInstanceBackupRecord`]), so a replication into this Region is found from the
    /// ARN itself rather than from the source instance's record — which is what lets a Region
    /// answer "nothing is replicating here" for a source instance that exists and replicates
    /// nowhere.
    ///
    /// Both halves of what the ARN names — the Region segment and the identifier — are matched
    /// without regard to case, as every name this service resolves against stored state is, so
    /// a replication in force is found by the ARN however the request spells it.
    pub fn replicated_instance_backup_from(
        &self,
        region: &str,
        source: &SourceDbInstance,
    ) -> Option<&ReplicatedInstanceBackupRecord> {
        self.replicated_instance_backups_in(region)
            .into_iter()
            .find(|backup| {
                backup.source.region.eq_ignore_ascii_case(&source.region)
                    && backup
                        .source
                        .db_instance_identifier
                        .eq_ignore_ascii_case(&source.identifier)
            })
    }

    /// Whether this store has ROOM to replicate the backups of the instance whose resource id
    /// is `dbi_resource_id` into `region` ([`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`] per
    /// Region, in at most [`MAX_REGIONS`] Regions). A Region already holding that instance's
    /// replicated backups has room for them, since replicating again writes that one record
    /// rather than adding another.
    ///
    /// Read BEFORE the replication is enabled, so a call the quota refuses changes nothing:
    /// the refusal is the modelled `DBInstanceAutomatedBackupQuotaExceededFault`.
    pub fn has_room_for_replicated_instance_backup(
        &self,
        region: &str,
        dbi_resource_id: &str,
    ) -> bool {
        if self
            .replicated_instance_backup_of(region, dbi_resource_id)
            .is_some()
        {
            return true;
        }
        self.replicated_instance_backups_in(region).len()
            < RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION
            && self.has_room_for_region(region)
    }

    /// Replicate `backup` into its destination Region: what
    /// `StartDBInstanceAutomatedBackupsReplication` does.
    ///
    /// Call it with [`Self::has_room_for_replicated_instance_backup`] already answered, which
    /// is where the destination Region's quota is enforced.
    pub fn replicate_instance_automated_backup(&mut self, backup: ReplicatedInstanceBackupRecord) {
        self.db_instance_automated_backup_replications.insert(
            (
                backup.destination_region.clone(),
                backup.source.dbi_resource_id.clone(),
            ),
            backup,
        );
    }

    /// The automated backups replicated into `region` that `reference` names, by either of
    /// the members that may name an automated backup ([`AutomatedBackupReference`]) — the
    /// source instance's resource id, or the backups' own ARN in this Region.
    ///
    /// The one resolution `DeleteDBInstanceAutomatedBackup` resolves a replicated backup
    /// through, so the report and the delete cannot disagree about which backup a value names.
    pub fn replicated_instance_backup_named(
        &self,
        region: &str,
        reference: &AutomatedBackupReference,
    ) -> Option<&ReplicatedInstanceBackupRecord> {
        self.replicated_instance_backups_in(region)
            .into_iter()
            .find(|backup| {
                reference.names(
                    &backup.source.dbi_resource_id,
                    &backup.automated_backups_arn(),
                )
            })
    }

    /// Remove those backups from this Region, answering the record removed: what
    /// `DeleteDBInstanceAutomatedBackup` does to the backups a stopped replication left, after
    /// which this Region reports them no more and the Region's allowance has room again.
    pub fn remove_replicated_instance_backup(
        &mut self,
        region: &str,
        dbi_resource_id: &str,
    ) -> Option<ReplicatedInstanceBackupRecord> {
        let key = self
            .replicated_instance_backups_in(region)
            .into_iter()
            .find(|backup| {
                backup
                    .source
                    .dbi_resource_id
                    .eq_ignore_ascii_case(dbi_resource_id)
            })
            .map(|backup| (region.to_string(), backup.source.dbi_resource_id.clone()))?;
        self.db_instance_automated_backup_replications.remove(&key)
    }

    /// `DBInstanceAutomatedBackupsReplications` for the automated backup of the instance whose
    /// resource id is `dbi_resource_id`: "the list of replications to different Amazon Web
    /// Services Regions associated with the automated backup", each naming the replicated
    /// backups' own ARN.
    ///
    /// Read off the destination Regions' records rather than stored on the source, so the two
    /// halves of one replication cannot disagree, and only the replications IN FORCE are in
    /// it: a stopped one left backups the destination Region retains, which are no longer a
    /// replication of this backup. Walks every destination Region's records, bounded by
    /// [`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`] in each of at most [`MAX_REGIONS`]
    /// Regions.
    pub fn automated_backups_replications_of(
        &self,
        dbi_resource_id: &str,
    ) -> Vec<DBInstanceAutomatedBackupsReplication> {
        self.db_instance_automated_backup_replications
            .values()
            .filter(|backup| {
                backup.is_replicating() && backup.source.dbi_resource_id == dbi_resource_id
            })
            .map(|backup| DBInstanceAutomatedBackupsReplication {
                db_instance_automated_backups_arn: Some(backup.automated_backups_arn()),
            })
            .collect()
    }

    /// `DBInstanceAutomatedBackup` for the automated backup of `instance`, with the
    /// replications of it to other Regions this store holds.
    ///
    /// The one projection of a live instance's automated backup: the instance's own
    /// ([`DbInstanceRecord::automated_backup_to_wire`]) cannot report the replications,
    /// because they are records of OTHER Regions and the instance record holds no half of
    /// them.
    pub fn instance_automated_backup_to_wire(
        &self,
        instance: &DbInstanceRecord,
    ) -> DBInstanceAutomatedBackup {
        DBInstanceAutomatedBackup {
            db_instance_automated_backups_replications: Some(
                self.automated_backups_replications_of(&instance.dbi_resource_id),
            ),
            ..instance.automated_backup_to_wire()
        }
    }

    /// Materialise the transitions the clock has made due for the automated backups replicated
    /// into `region`: the `creating` -> `active` each one owes once its first replicated
    /// snapshot is due. Called by [`Self::settle_due_transitions`], which is where every
    /// handler materialises what the clock already owed.
    pub(super) fn settle_replicated_instance_backups(&mut self, region: &str, now: Instant) {
        for (key, backup) in self
            .db_instance_automated_backup_replications
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            backup.settle(now);
        }
    }
}
