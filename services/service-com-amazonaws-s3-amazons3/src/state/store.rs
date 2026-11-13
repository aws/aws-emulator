// SPDX-License-Identifier: CC0-1.0

//! The store itself: the bucket map every operation of this service reads and writes, and
//! the operations the store performs on it.

use super::*;

/// Per-service state for com-amazonaws-s3-amazons3. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsS3Amazons3State {
    /// Buckets by name.
    ///
    /// Keyed by name ALONE because that is the scope of S3's bucket namespace: "General
    /// purpose buckets exist in a global namespace, which means that each bucket name
    /// must be unique across all Amazon Web Services accounts in all the Amazon Web
    /// Services Regions within a partition" (`CreateBucket`'s documentation). Sorted,
    /// because S3 lists buckets in name order.
    pub(super) buckets: BTreeMap<String, Bucket>,
    /// How many object versions this store has minted an id for, ever.
    ///
    /// The sequence [`mint_version_id`] derives each id from, so two versions of one key
    /// — and two versions anywhere in the store — can never carry the same id, whatever
    /// the clock does. Only [`Self::put_object`] advances it.
    pub(super) minted_versions: u64,
    /// How many notification configuration ids this store has minted.
    ///
    /// Only ever increases, so [`Self::mint_notification_configuration_id`] cannot hand
    /// out the same id twice however many configurations are put.
    pub(super) minted_notification_configuration_ids: u64,
    /// How many lifecycle rule identifiers this store has minted.
    ///
    /// Only ever increases, so [`Self::mint_lifecycle_rule_id`] cannot hand out the same
    /// identifier twice however many configurations are put — on one bucket or across them.
    pub(super) minted_lifecycle_rule_ids: u64,
    /// How many multipart upload ids this store has minted, ever.
    ///
    /// The sequence [`mint_upload_id`] derives each id from, so two uploads of one key —
    /// and two uploads anywhere in the store — can never carry the same id, whatever the
    /// clock does. Only [`Self::create_multipart_upload`] advances it.
    pub(super) minted_upload_ids: u64,
    /// What the objects AWS publishes ([`aws_published`]) retain, and how many stored
    /// versions they are, measured ONCE when the store was seeded with them.
    ///
    /// The whole-store ceilings ([`MAX_OBJECTS_TOTAL`] / [`MAX_OBJECT_BYTES_TOTAL`]) bound
    /// what this process holds because a CALLER asked it to, and the published catalog is
    /// fixed by the code rather than by anything a caller does — so this fixed amount is
    /// subtracted from the store's footprint ([`Self::caller_retained_bytes`] /
    /// [`Self::caller_stored_versions`]) instead of a whole bucket being skipped. Measured
    /// rather than exempted per bucket so that bytes a caller writes INTO a published
    /// bucket count against those budgets exactly as they would anywhere else.
    ///
    /// Written once, by [`Self::record_published_buckets`]; nothing else moves it, so it is
    /// the CEILING on what the budgets subtract — [`Self::published_allowance`] takes the
    /// smaller of it and what those buckets hold now.
    pub(super) published_retained_bytes: usize,
    /// How many stored versions the published catalog is. See
    /// [`Self::published_retained_bytes`].
    pub(super) published_versions: usize,
    /// The sessions `CreateSession` has vended and not released, as `credential id ->
    /// the session`.
    ///
    /// Kept because a session has a LIFETIME: "temporary security credentials are scoped to
    /// the bucket and expire after 5 minutes" (`CreateSession`'s own documentation), and a
    /// credential nothing releases would be accepted for signing for as long as the process
    /// lives. [`Self::release_expired_sessions`] reports the ones the clock has already
    /// expired so the vending operation can forget them, and
    /// [`Self::record_vended_session`] bounds how many this process holds at once.
    ///
    /// Each record also carries what the session is SCOPED to — "temporary security
    /// credentials are scoped to the bucket" — so a request presenting a session token can
    /// be judged against the bucket that session was created for
    /// ([`Self::session_for_token`]).
    ///
    /// Private: the methods below are the only way in or out, so the quota and the
    /// release cannot be applied in one place and skipped in another.
    pub(super) vended_sessions: BTreeMap<String, VendedSession>,
    /// The renames this store has performed, as `client token -> (the instant it was
    /// recorded, the rename)`.
    ///
    /// Kept because `RenameObject` "supports idempotency using a client token"
    /// (`RenameObjectRequest$ClientToken`): a retry under the same token has to succeed
    /// without renaming anything again, and one under the same token with other parameters
    /// has to be refused — neither of which can be answered without the completed call.
    ///
    /// Private: `state/renames.rs` holds the two methods that read and write it, so the
    /// bound on how many are remembered cannot be applied in one place and skipped in
    /// another.
    pub(super) completed_renames: BTreeMap<String, (SystemTime, CompletedRename)>,
    /// The metadata tables that exist in the managed table bucket, each as its name
    /// qualified by the namespace it lives in ([`metadata_qualified_table_name`]).
    ///
    /// ONE record for the whole store, because that is its scope: the managed table bucket is
    /// one per Region and every bucket's metadata tables are created inside it, in a
    /// namespace of that bucket's own ([`metadata_table_namespace`]) — so a qualified table
    /// name is taken or free for the store rather than for one bucket.
    ///
    /// Kept apart from the bucket's own configuration because its SCOPE is different, and
    /// because a table that came into service OUTLIVES it: the name of a table is taken or
    /// free for the whole namespace, so a create asks this set rather than the bucket whether
    /// the name it would provision under is free — and provisions into
    /// [`METADATA_TABLE_STATUS_FAILED`] with [`METADATA_JOURNAL_TABLE_EXISTS_CODE`] when a
    /// table already holds it. Deleting the configuration that created a table in service does
    /// not delete that table (the published failure above asks the caller to delete it through
    /// the table API that owns it), and neither does deleting the general purpose bucket; what
    /// a configuration going away DOES release is the name of a table whose creation had not
    /// yet fallen due, which left no table behind for anyone to delete
    /// ([`Self::release_unprovisioned_metadata_tables`]). A ceiling bounds the rest
    /// ([`MAX_METADATA_NAMESPACE_TABLES`]).
    pub(super) metadata_namespace_tables: BTreeSet<String>,
    /// The secret THIS store's page tokens are authenticated under — the continuation
    /// token every listing of a bucket's id-addressed configurations hands back (see
    /// `state/configuration_tokens.rs`).
    ///
    /// Drawn from the system's randomness when the store is created and never reported, so
    /// a token this store did not mint is detectable and refused rather than read as a
    /// cursor into a position the caller chose. Private: [`Self::page_token_key`] is the
    /// one way to it, and it hands back a copy of the key rather than the key's bytes.
    pub(super) page_tokens: ::service_common::page_tokens::PageTokenKey,
    /// The ACCESS POINTS that front this store's buckets, by access point ARN.
    ///
    /// An access point is a named data path onto ONE bucket, so the record a request
    /// addressed to it is served and judged by belongs to the store that serves the data
    /// plane rather than to the service that creates the resource — which writes it here,
    /// exactly as it writes a bucket's tag set here. See `state/access_points.rs` for the
    /// record, the addresses that reach one, and the bound on how many are held.
    ///
    /// Private: the methods in that file are the only way in or out, so the ceiling cannot
    /// be applied in one place and skipped in another.
    pub(super) access_points: BTreeMap<String, access_points::AccessPointRoute>,
    /// The index of the ADDRESSES that reach each access point other than its ARN — its
    /// alias and the first label of its endpoint host — as `address -> access point ARN`.
    ///
    /// An index rather than a second record: the route itself is held once, under its ARN,
    /// and this is what lets the address EVERY request carries be resolved by one lookup
    /// instead of a scan. Written only by the two methods in `state/access_points.rs` that
    /// record and forget a route, so it holds exactly the addresses of the routes above and
    /// is bounded with them.
    pub(super) access_point_addresses: BTreeMap<String, String>,
    /// The ACCOUNT-level Block Public Access configurations in force on this store's
    /// buckets, by account id.
    ///
    /// S3 Block Public Access is configured on a bucket and on the ACCOUNT, and the settings
    /// in force on a bucket are the most restrictive of both — so the account-level record
    /// belongs to the store the data plane is served and judged from, which is this one, and
    /// the service that owns the API over it writes it here exactly as it writes an access
    /// point's route. See `state/account_public_access_block.rs` for the record, the union
    /// the guardrails read, and the bound on how many accounts are held.
    ///
    /// Private: the methods in that file are the only way in or out, so the ceiling cannot
    /// be applied in one place and skipped in another.
    pub(super) account_public_access_blocks: BTreeMap<String, BucketPublicAccessBlock>,
}

impl Default for ComAmazonawsS3Amazons3State {
    /// The store a fresh engine hands this service: empty of everything a caller writes,
    /// and already holding the buckets AWS publishes ([`aws_published`]).
    ///
    /// Those buckets are what the store STARTS with because that is what publishing one
    /// means: no client creates `sagemaker-sample-files`, and a client that follows a
    /// documented path reads sample data out of it before it creates a bucket of its own.
    /// Seeding them here — the one place a store comes into being — is what lets every
    /// reader resolve them through the one bucket map, with no operation carrying a case
    /// for a managed name.
    fn default() -> Self {
        let mut store = Self {
            buckets: BTreeMap::new(),
            minted_versions: 0,
            minted_notification_configuration_ids: 0,
            minted_lifecycle_rule_ids: 0,
            minted_upload_ids: 0,
            published_retained_bytes: 0,
            published_versions: 0,
            vended_sessions: BTreeMap::new(),
            completed_renames: BTreeMap::new(),
            metadata_namespace_tables: BTreeSet::new(),
            page_tokens: ::service_common::page_tokens::PageTokenKey::new(),
            access_points: BTreeMap::new(),
            access_point_addresses: BTreeMap::new(),
            account_public_access_blocks: BTreeMap::new(),
        };
        store.record_published_buckets();
        store
    }
}

impl ComAmazonawsS3Amazons3State {
    /// The key this store's continuation tokens are minted and read under.
    ///
    /// `Copy`, so a paginated read takes the key off the store and goes on to borrow the
    /// configurations it is about to report in the same call. Read-only: nothing about the
    /// store moves, which is what keeps a listing a report.
    pub fn page_token_key(&self) -> ::service_common::page_tokens::PageTokenKey {
        self.page_tokens
    }

    /// Forget every session whose expiry the clock has already passed, reporting the
    /// credential ids so the caller can delete them from the credential store too.
    ///
    /// A session "expire[s] after 5 minutes. After the expiration time, any calls that you
    /// make with those credentials will fail" (`CreateSession`'s own documentation), so this
    /// materialises only what the CLOCK already made due: the release was owed at the same
    /// instant whether or not anybody called. Nothing else about the store moves.
    pub fn release_expired_sessions(&mut self, now: SystemTime) -> Vec<String> {
        let expired: Vec<String> = self
            .vended_sessions
            .iter()
            .filter(|(_, session)| session.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.vended_sessions.remove(id);
        }
        expired
    }

    /// Record the session `id` as vended until `expires_at`, or report that this process
    /// already holds as many sessions as it keeps at once ([`MAX_VENDED_SESSIONS`]).
    ///
    /// The bound is this emulator's, not an AWS quota: a caller may ask for a session as
    /// often as it likes, and each one is a credential this process has to keep accepting
    /// until it expires — so the count is what keeps that memory bounded whatever a caller
    /// does. Read and written under the one guard the caller already holds, so two
    /// concurrent vends cannot both pass a bound only one of them fits in.
    pub fn record_vended_session(&mut self, id: &str, expires_at: SystemTime) -> bool {
        self.record_session(
            id,
            VendedSession {
                expires_at,
                scope: None,
            },
        )
    }

    /// [`Self::record_vended_session`] for a session `CreateSession` vended: the same bound,
    /// and the SCOPE the session was created with — the directory bucket it is for and the
    /// token a Zonal endpoint request presents it with.
    ///
    /// The session is recorded in the DEFAULT mode and under no encryption algorithm, which
    /// is what a `CreateSession` naming neither establishes; a vend that names either records
    /// the whole scope through [`Self::record_vended_session_scoped`].
    pub fn record_vended_session_on(
        &mut self,
        id: &str,
        expires_at: SystemTime,
        bucket: &str,
        token: &str,
    ) -> bool {
        self.record_vended_session_scoped(
            id,
            expires_at,
            SessionScope {
                bucket: bucket.to_string(),
                token: token.to_string(),
                mode: SessionMode::default(),
                encryption_algorithm: None,
            },
        )
    }

    /// [`Self::record_vended_session_on`] with the WHOLE scope the vend settled: the bucket,
    /// the token, the mode the session was created in and the encryption algorithm it was
    /// established under.
    pub fn record_vended_session_scoped(
        &mut self,
        id: &str,
        expires_at: SystemTime,
        scope: SessionScope,
    ) -> bool {
        self.record_session(
            id,
            VendedSession {
                expires_at,
                scope: Some(scope),
            },
        )
    }

    /// The one way a session joins the store, so the bound is applied to every kind of
    /// record ([`MAX_VENDED_SESSIONS`]).
    fn record_session(&mut self, id: &str, session: VendedSession) -> bool {
        if self.vended_sessions.len() >= MAX_VENDED_SESSIONS {
            return false;
        }
        self.vended_sessions.insert(id.to_string(), session);
        true
    }

    /// Whether `access_key_id` names a session this service vended and has not released —
    /// so a request presenting it presents a credential the emulator ISSUED.
    pub fn holds_vended_session(&self, access_key_id: &str) -> bool {
        self.vended_sessions.contains_key(access_key_id)
    }

    /// What the session the credential `key_id` belongs to authorizes on the bucket named
    /// `bucket` at `now`, or `None` when `key_id` names no scoped session at all — a
    /// credential this rule has nothing to say about.
    ///
    /// The other half of "temporary security credentials are scoped to the bucket and expire
    /// after 5 minutes": the scope and the expiry belong to the CREDENTIALS and not only to
    /// the token they are presented with, so a request signed with a session's key pair
    /// reaches the bucket that session was created for, while that session lives, and
    /// nothing else. Read-only, and judged by the one verdict
    /// [`Self::session_token_verdict`] answers, so a session is judged the same whichever of
    /// its halves the request presented.
    pub fn session_credential_verdict(
        &self,
        key_id: &str,
        bucket: &str,
        now: SystemTime,
    ) -> Option<SessionTokenVerdict> {
        let session = self.vended_sessions.get(key_id)?;
        let scope = session.scope.as_ref()?;
        if session.expires_at <= now {
            return Some(SessionTokenVerdict::Expired);
        }
        Some(match scope.bucket == bucket {
            true => SessionTokenVerdict::Authorized,
            false => SessionTokenVerdict::OtherBucket,
        })
    }

    /// What the session token `token` authorizes on the bucket named `bucket`, at `now`.
    ///
    /// Read-only, and the expiry is applied HERE rather than by releasing anything: "after
    /// the expiration time, any calls that you make with those credentials will fail", so a
    /// token whose session the clock has expired authorizes nothing, whether or not a later
    /// vend has got round to forgetting it ([`Self::release_expired_sessions`]).
    pub fn session_token_verdict(
        &self,
        token: &str,
        bucket: &str,
        now: SystemTime,
    ) -> SessionTokenVerdict {
        let named = self.vended_sessions.values().find(|session| {
            session
                .scope
                .as_ref()
                .is_some_and(|scope| scope.token == token)
        });
        let Some(session) = named else {
            return SessionTokenVerdict::Unknown;
        };
        if session.expires_at <= now {
            return SessionTokenVerdict::Expired;
        }
        match session
            .scope
            .as_ref()
            .is_some_and(|scope| scope.bucket == bucket)
        {
            true => SessionTokenVerdict::Authorized,
            false => SessionTokenVerdict::OtherBucket,
        }
    }

    /// What the session `presented` names authorizes on the bucket named `bucket`, at `now`,
    /// or `None` when it names no scoped session at all — a credential this rule has nothing
    /// to say about.
    ///
    /// The ONE entry point for judging a presented session, whichever of its two halves the
    /// request carried ([`PresentedSession`]): a token is judged by
    /// [`Self::session_token_verdict`] and a key pair by
    /// [`Self::session_credential_verdict`], so the two cannot drift apart.
    pub fn presented_session_verdict(
        &self,
        presented: &PresentedSession,
        bucket: &str,
        now: SystemTime,
    ) -> Option<SessionTokenVerdict> {
        match presented {
            PresentedSession::Token(token) => {
                Some(self.session_token_verdict(token, bucket, now))
            }
            PresentedSession::Credential(key_id) => {
                self.session_credential_verdict(key_id, bucket, now)
            }
        }
    }

    /// The SCOPE of the live session `presented` names, or `None` when it names none — a
    /// token no session carries, a credential that is not a session's, or a session the clock
    /// has expired ("after the expiration time, any calls that you make with those
    /// credentials will fail").
    ///
    /// What a call acting UNDER a session reads about it: the mode it was created in and the
    /// encryption algorithm it was established under. Read-only; the expiry is applied here
    /// rather than by releasing anything, exactly as the verdicts above apply it.
    pub fn presented_session_scope(
        &self,
        presented: &PresentedSession,
        now: SystemTime,
    ) -> Option<&SessionScope> {
        let session = match presented {
            PresentedSession::Token(token) => self.vended_sessions.values().find(|session| {
                session
                    .scope
                    .as_ref()
                    .is_some_and(|scope| &scope.token == token)
            })?,
            PresentedSession::Credential(key_id) => self.vended_sessions.get(key_id)?,
        };
        if session.expires_at <= now {
            return None;
        }
        session.scope.as_ref()
    }

    /// Whether the managed table bucket already holds the metadata table `table`, named as
    /// [`metadata_qualified_table_name`] qualifies it — see [`Self::metadata_namespace_tables`].
    ///
    /// What a create of a metadata configuration asks before it provisions a table: a name
    /// already in the namespace is one S3 cannot create a second table under, which is the
    /// `*AlreadyExists` provisioning failure the configuration then reports. A name is in this
    /// set while a table stands under it — one still being created as much as one in service —
    /// and is out of it again once a configuration taken away carried off a table that had
    /// never come into service ([`Self::release_unprovisioned_metadata_tables`]).
    pub fn metadata_namespace_holds(&self, table: &str) -> bool {
        self.metadata_namespace_tables.contains(table)
    }

    /// How many more tables the managed table bucket's namespace has room for
    /// ([`MAX_METADATA_NAMESPACE_TABLES`]).
    ///
    /// A table in service outlives the configuration that created it, and only the name of one
    /// that never came into service is released with it
    /// ([`Self::delete_metadata_configuration`]) — so a caller creating configurations for
    /// bucket after bucket would grow the namespace without bound; what keeps it bounded is
    /// this ceiling, measured against the names a create would ADD before it records any of
    /// them: a create that would carry the namespace past the maximum is refused instead, with
    /// nothing recorded. Read under the guard the caller already holds, so two concurrent
    /// creates cannot both fit in room only one of them has.
    pub fn metadata_namespace_room(&self) -> usize {
        MAX_METADATA_NAMESPACE_TABLES.saturating_sub(self.metadata_namespace_tables.len())
    }

    /// Record `table` as a table of the managed table bucket's namespace.
    ///
    /// Called for each table a create really provisions, so the namespace holds exactly the
    /// tables this store has created and not carried away before they came into service
    /// ([`Self::release_unprovisioned_metadata_tables`]): no operation of this service deletes
    /// a table that is really there. A name the namespace already holds is already recorded, so
    /// recording it again changes nothing.
    ///
    /// The CEILING is the caller's: [`MAX_METADATA_NAMESPACE_TABLES`] is measured through
    /// [`Self::metadata_namespace_room`] against the names a create would add, before any of
    /// them is recorded and under the same guard — one place owning the bound, rather than a
    /// second check here that the caller would have to interpret.
    pub fn record_metadata_namespace_table(&mut self, table: &str) {
        self.metadata_namespace_tables.insert(table.to_string());
    }

    /// Take the S3 Metadata configuration of the bucket named `bucket` away, reporting what
    /// that did ([`MetadataConfigurationDeletion`]): the three outcomes the caller answers
    /// differently, because a bucket carrying NO configuration has none to delete — the
    /// delete is not idempotent, so it answers the metadata configuration's own not-there
    /// condition rather than the success a delete that removed one answers.
    ///
    /// The CONFIGURATION is what goes, and with it the names of the tables that never came
    /// into service. A table that DID is a table of an S3 Tables bucket and is deleted through
    /// the table API that owns it — "a journal table already exists in the Amazon Web Services
    /// managed table bucket's namespace. Delete the journal table, and then try again. To
    /// create a new metadata table, you must delete the metadata configuration for this
    /// bucket, and then create a new metadata configuration"
    /// ([`METADATA_JOURNAL_TABLE_EXISTS_MESSAGE`], the provisioning failure the published V2
    /// list carries) states both halves: the configuration delete is a PRECONDITION for a new
    /// configuration, and the leftover table is the separate deletion a caller still owes. So a
    /// create of the same configuration afterwards is accepted and its journal table provisions
    /// into [`METADATA_TABLE_STATUS_FAILED`] carrying that code, exactly as a create colliding
    /// with any other table of the namespace does — while a configuration whose tables were
    /// still being created left no table for a caller to delete, so those names are free again
    /// and the next create provisions afresh under them
    /// ([`Self::release_unprovisioned_metadata_tables`]).
    pub fn delete_metadata_configuration(
        &mut self,
        bucket: &str,
        now: SystemTime,
    ) -> MetadataConfigurationDeletion {
        let Some(held) = self.buckets.get_mut(bucket) else {
            return MetadataConfigurationDeletion::NoSuchBucket;
        };
        let Some(configuration) = held.take_metadata_configuration() else {
            return MetadataConfigurationDeletion::NoConfiguration;
        };
        self.release_unprovisioned_metadata_tables(&configuration, now);
        MetadataConfigurationDeletion::Deleted
    }

    /// Release from the managed table bucket's namespace the name of every table of
    /// `configuration` that no table stands under at `now`.
    ///
    /// The one rule behind every way this service stops recording a configuration's tables —
    /// the configuration deleted, the general purpose bucket carrying it deleted, one of its
    /// optional tables disabled: a name belongs to the namespace for as long as a TABLE holds
    /// it, and a table holds the name it was created under from the instant its creation falls
    /// due ([`MetadataTable::creating_at`]). A table whose creation was still under way leaves
    /// nothing in the namespace for the caller to delete, so its name is free again; a table in
    /// service keeps its name until the S3 Tables API that owns it deletes it, and a table that
    /// FAILED never held its name at all — the table that is really there does — so neither
    /// releases anything here.
    fn release_unprovisioned_metadata_tables(
        &mut self,
        configuration: &BucketMetadataConfiguration,
        now: SystemTime,
    ) {
        for table in configuration.tables() {
            if table.creating_at(now) {
                self.metadata_namespace_tables.remove(&table.qualified_name());
            }
        }
    }

    /// Release the name of the single table `table` the same way, for a call that drops one
    /// table of a configuration rather than the whole configuration — see
    /// [`Self::release_unprovisioned_metadata_tables`], which owns the rule.
    fn release_unprovisioned_metadata_table(&mut self, table: &MetadataTable, now: SystemTime) {
        if table.creating_at(now) {
            self.metadata_namespace_tables.remove(&table.qualified_name());
        }
    }

    /// Record `configuration_state` as the inventory table state of the S3 Metadata
    /// configuration the general purpose bucket `bucket` carries, provisioning or dropping the
    /// inventory table behind it and keeping the managed table bucket's namespace in step.
    ///
    /// The ONE way in, because an inventory table is a table of that namespace rather than of
    /// the bucket: `UpdateBucketMetadataInventoryTableConfiguration` really does PROVISION one
    /// — `s3tables:CreateTable` is among the permissions that operation requires, and none of
    /// the permissions it lists deletes a table (its own documentation) — so the name an
    /// enable creates a table under joins the namespace and stays there, exactly as the names
    /// a create provisions do ([`Self::record_metadata_namespace_table`]). An enable whose
    /// name the namespace ALREADY holds therefore provisions nothing: the table is recorded
    /// [`METADATA_TABLE_STATUS_FAILED`] with the published failure for the name being taken,
    /// the same verdict a create colliding with that name reports
    /// ([`metadata_inventory_table_failure`], the one rule both paths read). Enabling the
    /// table that is already there is not a provisioning at all, so it leaves both the table
    /// and the namespace exactly as they are.
    ///
    /// DISABLING drops the table, which releases its name exactly when nothing stands under it
    /// — a table whose creation had not yet fallen due left nothing behind for the caller to
    /// delete ([`Self::release_unprovisioned_metadata_tables`], the rule the configuration
    /// delete reads too).
    pub fn update_metadata_inventory_table(
        &mut self,
        bucket: &str,
        configuration_state: String,
        now: SystemTime,
    ) -> MetadataInventoryTableUpdate {
        let Some(held) = self.buckets.get(bucket) else {
            return MetadataInventoryTableUpdate::NoSuchBucket;
        };
        let Some(configuration) = held.metadata_configuration() else {
            return MetadataInventoryTableUpdate::NoConfiguration;
        };
        let namespace = metadata_table_namespace(bucket);
        let qualified_name =
            metadata_qualified_table_name(&namespace, METADATA_INVENTORY_TABLE_NAME);
        // Only an enable that finds the configuration carrying no table of its own provisions
        // one; the journal table it would read from is the configuration's own.
        let provisions = configuration_state == METADATA_CONFIGURATION_STATE_ENABLED
            && configuration
                .inventory
                .as_ref()
                .is_none_or(|inventory| inventory.table.is_none());
        let failure = provisions
            .then(|| {
                metadata_inventory_table_failure(
                    self.metadata_namespace_holds(&qualified_name),
                    configuration.journal.failure.is_some(),
                )
            })
            .flatten();
        // A table that could not be provisioned adds nothing to the namespace: either the name
        // is already held by the table that is there, or no table was created under it. Only a
        // table this really creates takes room, measured against the ceiling under the guard
        // the caller already holds — the way the create measures the names it would add — and
        // an enable that would carry the namespace past the maximum writes NOTHING.
        let takes_room = provisions && failure.is_none();
        if takes_room && self.metadata_namespace_room() == 0 {
            return MetadataInventoryTableUpdate::NamespaceFull;
        }
        // The table a DISABLE drops, read before the write that drops it, so its name can be
        // released when no table stood under it.
        let dropped = (configuration_state != METADATA_CONFIGURATION_STATE_ENABLED)
            .then(|| {
                configuration
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.table.clone())
            })
            .flatten();
        let Some(held) = self.buckets.get_mut(bucket) else {
            // Unreachable: the lookup above ran against this same map, under this same guard.
            return MetadataInventoryTableUpdate::NoSuchBucket;
        };
        if !held.update_metadata_inventory_table(configuration_state, &namespace, failure, now) {
            // Unreachable for the same reason: the configuration was read above.
            return MetadataInventoryTableUpdate::NoConfiguration;
        }
        if takes_room {
            self.record_metadata_namespace_table(&qualified_name);
        }
        if let Some(table) = dropped {
            self.release_unprovisioned_metadata_table(&table, now);
        }
        MetadataInventoryTableUpdate::Recorded
    }

    /// Every bucket this account holds, as `(name, bucket)` in bucket-name order — the
    /// order S3 lists buckets in.
    ///
    /// The read seam for any operation that reports the whole account's buckets, S3's own
    /// `ListBuckets` included, so none of them keeps a second copy of this map.
    pub fn buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.buckets
            .iter()
            .map(|(name, bucket)| (name.as_str(), bucket))
    }

    /// Every bucket of the CALLER'S OWN S3 account, as `(name, bucket)` in bucket-name
    /// order.
    ///
    /// The read seam for any operation that reports "the buckets in your account" — S3's
    /// own `ListBuckets` and the peer services that report the account's buckets. It is
    /// [`Self::buckets`] LESS the buckets another emulated service provisioned as the
    /// object store of one of its own resources ([`Bucket::provisioned_by`]): those are
    /// served by this service's object data plane, because that is what they are for, but
    /// they are not buckets of the caller's S3 account — AWS holds a Lightsail bucket in a
    /// service-managed account, so it is not in the account's own `ListBuckets` either.
    pub fn account_buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.buckets()
            .filter(|(_, bucket)| bucket.provisioned_by.is_none() && !bucket.aws_published)
    }

    /// Every GENERAL PURPOSE bucket of the caller's own S3 account, as `(name, bucket)` in
    /// bucket-name order.
    ///
    /// [`Self::account_buckets`] less the DIRECTORY buckets: the two kinds of bucket are
    /// listed by two different operations, each on its own endpoint — `ListBuckets`
    /// "returns a list of all buckets owned by the authenticated sender of the request" and
    /// "is not supported for directory buckets", while `ListDirectoryBuckets` "returns a
    /// list of all Amazon S3 directory buckets owned by the authenticated sender" (their own
    /// documentation) — so neither listing reports the other's buckets.
    pub fn general_purpose_account_buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.account_buckets()
            .filter(|(_, bucket)| !bucket.is_directory())
    }

    /// Every DIRECTORY bucket of the caller's own S3 account, as `(name, bucket)` in
    /// bucket-name order. See [`Self::general_purpose_account_buckets`].
    pub fn directory_account_buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.account_buckets()
            .filter(|(_, bucket)| bucket.is_directory())
    }

    /// How many stored versions the caller-stored half of this store holds, which is what
    /// [`MAX_OBJECTS_TOTAL`] bounds: every version in the store, less the published
    /// catalog's own ([`Self::published_allowance`]).
    pub(super) fn caller_stored_versions(&self) -> usize {
        let (_, versions) = self.published_allowance();
        self.buckets
            .values()
            .map(Bucket::stored_versions)
            .sum::<usize>()
            .saturating_sub(versions)
    }

    /// How many bytes the caller-stored half of this store retains, which is what
    /// [`MAX_OBJECT_BYTES_TOTAL`] bounds: everything the store retains, less the published
    /// catalog's own ([`Self::published_allowance`]).
    pub(super) fn caller_retained_bytes(&self) -> usize {
        let (bytes, _) = self.published_allowance();
        self.buckets
            .values()
            .map(Bucket::retained_bytes)
            .sum::<usize>()
            .saturating_sub(bytes)
    }

    /// What the whole-store budgets subtract for the published catalog, as
    /// `(bytes, versions)`: the seeded amount ([`Self::published_retained_bytes`]) or what
    /// the published buckets hold NOW, whichever is SMALLER.
    ///
    /// Both directions matter, which is why it is neither the cached amount alone nor the
    /// live footprint alone. A caller that writes into a published bucket pushes the live
    /// footprint ABOVE the seeded amount, so the subtraction stays at the seeded amount and
    /// everything it wrote counts against the budgets like a write anywhere else. A caller
    /// that deletes a published object pushes it BELOW, so the subtraction shrinks with it
    /// and no room is handed out for bytes the store no longer holds.
    pub(super) fn published_allowance(&self) -> (usize, usize) {
        let (mut bytes, mut versions) = (0usize, 0usize);
        for bucket in self.buckets.values().filter(|held| held.aws_published) {
            bytes += bucket.retained_bytes();
            versions += bucket.stored_versions();
        }
        (
            bytes.min(self.published_retained_bytes),
            versions.min(self.published_versions),
        )
    }

    /// The bucket named `name`, or `None` when this emulator holds no such bucket.
    pub fn bucket(&self, name: &str) -> Option<&Bucket> {
        self.buckets.get(name)
    }

    /// The bucket named `name` for writing — a subresource (a policy or a tag set) or its
    /// objects — or `None` when this emulator holds no such bucket, so such a write
    /// refuses a name that is not a bucket instead of creating one.
    pub fn bucket_mut(&mut self, name: &str) -> Option<&mut Bucket> {
        self.buckets.get_mut(name)
    }

    /// Store `object` at `key` in the bucket named `bucket`, reporting what that did, or
    /// `None` when this emulator holds no such bucket (so an object write refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// This is also where an object's VERSION ID is minted, because this is the only place
    /// that sees both the bucket's versioning state and the store's version sequence: a
    /// write into a bucket whose status is `Enabled` mints a fresh id and keeps the version
    /// it supersedes ("All objects added to the bucket receive a unique version ID" —
    /// `PutBucketVersioning`), and a write anywhere else mints none and replaces what the
    /// key held, which is the `null` version. The id the write stored is read back off the
    /// stored object, so nothing reports an id the store did not record.
    ///
    /// This is the ONLY way an object is recorded, because it is the only place that can
    /// see BOTH the per-bucket ceilings and the service-wide ones: the per-bucket pair
    /// alone would leave the store bounded only by their product with
    /// [`MAX_BUCKETS_PER_ACCOUNT`], so the whole-store checks here ([`MAX_OBJECTS_TOTAL`]
    /// and [`MAX_OBJECT_BYTES_TOTAL`]) are what bound this process's object memory
    /// whatever a caller does. Nothing is written unless the answer is
    /// [`ObjectWrite::Created`] or [`ObjectWrite::Replaced`]; a refusal leaves every
    /// stored object exactly as it was, and mints nothing.
    pub fn put_object(&mut self, bucket: &str, key: &str, object: Object) -> Option<ObjectWrite> {
        self.record_object(bucket, key, object, None)
    }

    /// Store the REPLICA `object` at `key` in the bucket named `bucket`, under
    /// `replicated_version` — the id of the SOURCE version it is a copy of — rather than an
    /// id of its own.
    ///
    /// A replica keeps its source's version id: the copy is the same version of the same
    /// object in another bucket, which is what lets a reader address it there by the id the
    /// source reported. Everything else is [`Self::put_object`]'s: the same per-bucket and
    /// service-wide ceilings bound a replica, so a destination that is full refuses the copy
    /// rather than growing past them — which is what the source version then reports as a
    /// replication that did not happen (see [`Self::replicate_write`]).
    pub fn put_replica(
        &mut self,
        bucket: &str,
        key: &str,
        object: Object,
        replicated_version: Option<&str>,
    ) -> Option<ObjectWrite> {
        self.record_object(bucket, key, object, replicated_version)
    }

    /// [`Self::put_object`] and [`Self::put_replica`] in one: `replicated_version` is the
    /// version id a REPLICA keeps, and `None` asks for the id the bucket's versioning state
    /// calls for.
    pub(super) fn record_object(
        &mut self,
        bucket: &str,
        key: &str,
        object: Object,
        replicated_version: Option<&str>,
    ) -> Option<ObjectWrite> {
        let held_bucket = self.buckets.get(bucket)?;
        let versioned = held_bucket.versioning_enabled();
        // Whether this write keeps the version it supersedes, which is what decides
        // whether it adds a stored version to the store's counts and budgets or replaces
        // one in place. It turns on the superseded version, not on the bucket's status.
        let keeps = held_bucket.keeps_superseded(key);
        let held = held_bucket.object(key).is_some();

        // What the whole store would hold after this write: every other bucket's bytes,
        // plus what this bucket would hold with the object at `key` written. The counts
        // work the same way — a key the bucket already holds adds no entry, unless the
        // version there is kept rather than replaced. Measured over the CALLER-stored half
        // of the store ([`Self::caller_retained_bytes`]), which is what these budgets
        // bound; a write into a bucket AWS publishes counts here like any other.
        let held_versions: usize = self.caller_stored_versions();
        if (keeps || !held) && held_versions >= MAX_OBJECTS_TOTAL {
            return Some(ObjectWrite::TotalObjectCountReached);
        }
        // The id is minted before the budgets are measured, so the bytes it retains are
        // counted; a refusal below leaves it unused, which costs nothing but the sequence
        // number it consumed (ids are opaque, and nothing numbers them). A REPLICA mints
        // none: it keeps the id of the source version it copies.
        let minted = match replicated_version {
            Some(replicated) => Some(replicated.to_string()),
            None => versioned.then(|| {
                self.minted_versions = self.minted_versions.saturating_add(1);
                mint_version_id(bucket, key, self.minted_versions)
            }),
        };
        let incoming = object.retained_bytes() + minted.as_ref().map_or(0, String::len);
        // The same allowance the five other whole-store checks subtract
        // ([`Self::published_allowance`]), so this path hands out no room for published
        // bytes the store no longer holds either.
        let (published_bytes, _) = self.published_allowance();
        let would_hold: usize = self
            .buckets
            .iter()
            .map(|(name, held)| {
                if name == bucket {
                    held.retained_bytes_after_write(key, keeps)
                } else {
                    held.retained_bytes()
                }
            })
            .sum::<usize>()
            .saturating_sub(published_bytes)
            + incoming;
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectWrite::TotalByteBudgetReached { would_hold });
        }

        // Then the per-bucket ceilings, which also perform the write.
        self.buckets
            .get_mut(bucket)
            .map(|held| held.put_object(key, object, minted))
    }

    /// Carry out the replication the bucket named `bucket` owes for the version its last
    /// write stored at `key`, reporting what that version now carries — or `None` when no
    /// enabled rule of that bucket covers the key, which is the state of every object in a
    /// bucket with no replication configuration (and so of every object whose reader reports
    /// no `ReplicationStatus` at all).
    ///
    /// What it does, for each destination `state::replication::destinations` names: the
    /// stored version is copied there byte-identically, under the same key and the same
    /// version id ([`Self::put_replica`]), where it is a replica. The source version then
    /// carries [`REPLICATION_STATUS_COMPLETED`] when every one of those copies was made and
    /// [`REPLICATION_STATUS_FAILED`] when one could not be — a destination this emulator
    /// does not hold, one whose versioning is not enabled (replication copies object
    /// versions, so a destination without them cannot hold one), or one already at a
    /// ceiling. Either way the status is reported from
    /// [`REPLICATION_SETTLES_AFTER`] past the write ([`ObjectReplication::status_at`]).
    ///
    /// Every one of those copies is made under the role the configuration names, so `access`
    /// — what IAM answered about that role ([`replication::role_access`]) — decides whether
    /// any of them can be made at all: a role S3 cannot act as copies NOTHING, to no
    /// destination, and a role whose permissions do not let it write ONE destination's objects
    /// leaves that destination untouched while the others are still copied to. Either way the
    /// source version reports [`REPLICATION_STATUS_FAILED`], which is the terminal outcome of a
    /// replication whose role withholds any part of it.
    ///
    /// Called by the write that recorded the version, AFTER the store accepted it, so a
    /// refused write replicates nothing. A REPLICA is never replicated again, because the
    /// copy is written through [`Self::put_replica`] — which records it and replicates
    /// nothing itself — rather than through the write path that calls this: a configuration
    /// pointing back at the source therefore cannot copy a replica home.
    ///
    /// The WORK one write may do here is bounded, because both of its factors are a
    /// caller's to choose: how many destinations the stored configuration names and how
    /// large the object is. At most
    /// [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`] copies are made and at most
    /// [`MAX_REPLICATION_BYTES_PER_WRITE`] bytes are copied in one call, and the FIRST copy
    /// the store refuses ends the fan-out rather than letting every further destination pay
    /// for a copy that cannot be stored. A write past either ceiling records
    /// [`REPLICATION_STATUS_FAILED`], which is what the source version then reports: the
    /// copies past it were not made.
    pub fn replicate_write(
        &mut self,
        bucket: &str,
        key: &str,
        access: &replication::RoleAccess,
    ) -> Option<ObjectReplication> {
        // The tag set alone, because that is all the SELECTION needs: a bucket with no
        // configuration — and a key no rule covers — pays no copy of the object at all.
        let tags = self.bucket(bucket)?.object(key)?.tags.clone();
        let destinations = replication::destinations(self, bucket, key, &tags);
        if destinations.is_empty() {
            return None;
        }
        let asked = destinations.len();
        let source_version = self.bucket(bucket)?.object(key)?.clone();
        let body_bytes = source_version.body.len();

        // More destinations than one write replicates to is work this emulator does not do:
        // the copies past the ceiling are not made, and the source version says so.
        let mut every_copy_made = asked <= MAX_REPLICATION_DESTINATIONS_PER_WRITE;
        // And a role S3 cannot act as performs no copy at all: the destinations the
        // configuration names are left untouched, whatever they could have held, because
        // the copy into each of them would have been made under that role.
        let targets: Vec<replication::ReplicationTarget> = if access.copies_nothing() {
            every_copy_made = false;
            Vec::new()
        } else {
            destinations
                .into_iter()
                .take(MAX_REPLICATION_DESTINATIONS_PER_WRITE)
                .collect()
        };
        let mut copied_bytes = 0usize;
        for target in targets {
            // A destination the ROLE may not write a replica into holds none: the copy would
            // have been made under that role, and IAM answered that it may not make it there.
            // The other destinations are still copied to, so one denied destination fails the
            // replication without hiding what the rest of the configuration did.
            if !access.may_replicate_object(&target.bucket) {
                every_copy_made = false;
                continue;
            }
            // A destination this emulator cannot hold a version in replicates nothing: a
            // bucket it does not hold, one whose versioning is not enabled, and the SOURCE
            // bucket itself (a bucket cannot be its own destination, which would replace
            // the very version being replicated with a replica of it).
            let writable = self
                .bucket(&target.bucket)
                .is_some_and(Bucket::versioning_enabled);
            if !writable || target.bucket == bucket {
                every_copy_made = false;
                continue;
            }
            // The byte ceiling is consulted BEFORE the copy is made, so the bytes this one
            // call copies are bounded whatever the configuration asks for.
            if copied_bytes.saturating_add(body_bytes) > MAX_REPLICATION_BYTES_PER_WRITE {
                every_copy_made = false;
                break;
            }
            let replica = replication::replica_of(
                &source_version,
                target.storage_class.as_deref(),
                SystemTime::now(),
            );
            let written = self.put_replica(
                &target.bucket,
                key,
                replica,
                source_version.version_id.as_deref(),
            );
            if matches!(
                written,
                Some(ObjectWrite::Created) | Some(ObjectWrite::Replaced)
            ) {
                copied_bytes = copied_bytes.saturating_add(body_bytes);
            } else {
                // A store that cannot hold this copy cannot hold the next one either, so
                // the fan-out ends here instead of copying a body per remaining
                // destination only to have each one refused.
                every_copy_made = false;
                break;
            }
        }

        let settled = if every_copy_made {
            REPLICATION_STATUS_COMPLETED
        } else {
            REPLICATION_STATUS_FAILED
        };
        let record = ObjectReplication::source(settled, source_version.last_modified);
        self.buckets
            .get_mut(bucket)?
            .addressed_object_mut(key, source_version.version_id.as_deref())?
            .replication = Some(record.clone());
        Some(record)
    }

    /// Whether the bucket named `bucket` carries a replication configuration at all — the
    /// ONE question a caller with many keys asks once, instead of asking after each of them
    /// what a bucket that replicates nothing owes.
    ///
    /// Read-only, and cheap: it looks at the stored container rather than at any rule.
    pub fn replicates_objects(&self, bucket: &str) -> bool {
        self.bucket(bucket)
            .and_then(Bucket::replication)
            .is_some()
    }

    /// Replicate the DELETE MARKER a delete just laid on `key` in the bucket named
    /// `bucket`, reporting how many destinations it was laid in.    ///
    /// "Specifies whether Amazon S3 replicates delete markers"
    /// (`DeleteMarkerReplication$Status`): a marker goes only to the destinations of the
    /// covering rules that ask for it, so a rule carrying no `DeleteMarkerReplication` — or
    /// one whose status is not `Enabled` — leaves its destination unmarked. The marker laid
    /// in the destination is that bucket's own ([`Self::remove_object`] mints its id there),
    /// because a delete marker carries no bytes to copy.
    ///
    /// A marker is replicated under the configuration's role like every other copy, so
    /// `access` ([`replication::role_access`]) decides whether it is laid at all: a role S3
    /// cannot act as marks no destination, and one denied the replicated delete at a
    /// destination leaves that destination unmarked.
    ///
    /// Called by the delete that laid the marker. The PERMANENT delete of one version never
    /// reaches here, which is the published rule: a delete naming a version id is not
    /// replicated. A batch delete asks [`Self::replicates_objects`] once and calls this only
    /// for a bucket that has a configuration at all, so a bucket with none pays nothing per
    /// key; and one call marks at most [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`]
    /// destinations, the same ceiling a write's fan-out is bounded by.
    pub fn replicate_delete_marker(
        &mut self,
        bucket: &str,
        key: &str,
        access: &replication::RoleAccess,
    ) -> usize {
        if access.copies_nothing() {
            return 0;
        }
        let marked: Vec<String> = replication::destinations(self, bucket, key, &[])
            .into_iter()
            .filter(|target| target.delete_markers && target.bucket != bucket)
            // A destination the ROLE may not lay a replicated delete in is left unmarked, the
            // same reading the copy of a version takes of that role's permissions.
            .filter(|target| access.may_replicate_delete(&target.bucket))
            .map(|target| target.bucket)
            .take(MAX_REPLICATION_DESTINATIONS_PER_WRITE)
            .collect();
        let mut laid = 0usize;
        for destination in marked {
            if !self
                .bucket(&destination)
                .is_some_and(Bucket::versioning_enabled)
            {
                continue;
            }
            if matches!(
                self.remove_object(&destination, key),
                Some(ObjectDeletion::MarkerLaid { .. })
            ) {
                laid += 1;
            }
        }
        laid
    }

    /// Record `upload` as a new multipart upload in progress in the bucket named
    /// `bucket`, reporting what that did and the id the upload is addressed by — or
    /// `None` when this emulator holds no such bucket (so an initiation refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// This is also where the UPLOAD ID is minted, because this is the only place that
    /// sees the store's mint sequence: "This action initiates a multipart upload and
    /// returns an upload ID" (`CreateMultipartUpload`'s own documentation), and the id is
    /// recorded with the upload, so every later reader reports the id the upload really
    /// is addressed by rather than one re-derived from the request. Two initiations of one
    /// key are two uploads, each with its own id — "You specify this upload ID in each of
    /// your subsequent upload part requests" — so nothing here replaces an upload the
    /// bucket already has in progress.
    ///
    /// This is the ONLY way an upload is recorded, because it is the only place that can
    /// see BOTH the per-bucket ceiling ([`MAX_MULTIPART_UPLOADS_PER_BUCKET`]) and the
    /// service-wide one ([`MAX_MULTIPART_UPLOADS_TOTAL`]), which together bound the
    /// uploads this process holds whatever a caller does. Nothing is written unless the
    /// answer is [`MultipartUploadCreation::Initiated`]: an over-long key, metadata past
    /// [`MAX_OBJECT_METADATA_BYTES`] and either ceiling are each refused here, leaving
    /// every recorded upload exactly as it was.
    pub fn create_multipart_upload(
        &mut self,
        bucket: &str,
        upload: MultipartUpload,
    ) -> Option<MultipartUploadCreation> {
        let held_bucket = self.buckets.get(bucket)?;
        if upload.key.len() > MAX_OBJECT_KEY_BYTES {
            return Some(MultipartUploadCreation::KeyTooLong {
                bytes: upload.key.len(),
            });
        }
        let metadata_bytes = upload.metadata_bytes();
        if metadata_bytes > MAX_OBJECT_METADATA_BYTES {
            return Some(MultipartUploadCreation::MetadataTooLarge {
                bytes: metadata_bytes,
            });
        }
        if held_bucket.multipart_uploads.len() >= MAX_MULTIPART_UPLOADS_PER_BUCKET {
            return Some(MultipartUploadCreation::UploadCountReached);
        }
        let held_total: usize = self
            .buckets
            .values()
            .map(|held| held.multipart_uploads.len())
            .sum();
        if held_total >= MAX_MULTIPART_UPLOADS_TOTAL {
            return Some(MultipartUploadCreation::TotalUploadCountReached);
        }

        self.minted_upload_ids = self.minted_upload_ids.saturating_add(1);
        let upload_id = mint_upload_id(bucket, &upload.key, self.minted_upload_ids);
        self.buckets
            .get_mut(bucket)?
            .multipart_uploads
            .insert(upload_id.clone(), upload);
        Some(MultipartUploadCreation::Initiated { upload_id })
    }

    /// Record `part` against the multipart upload `upload_id` names in the bucket named
    /// `bucket`, reporting what that did — or `None` when this emulator holds no such
    /// bucket (so a part upload refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, so a request
    /// naming the id under another key names no upload
    /// ([`PartUpload::NoSuchUpload`]) — the same address a read of the upload is answered
    /// by, so neither can reach an upload the other cannot.
    ///
    /// A part REPLACES what its number held: "If you upload a new part using the same part
    /// number that was used with a previous part, the previously uploaded part is
    /// overwritten" (`UploadPart`'s own documentation), and the bytes the overwritten part
    /// retained are released with it.
    ///
    /// This is the ONLY way a part is recorded, because it is the only place that can see
    /// BOTH the per-bucket byte budget ([`MAX_OBJECT_BYTES_PER_BUCKET`]) and the
    /// service-wide one ([`MAX_OBJECT_BYTES_TOTAL`]) — the same two budgets a stored
    /// object is measured against, since a retained part holds this process's memory the
    /// same way. Nothing is written unless the answer is [`PartUpload::Stored`].
    pub fn upload_part(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
        part: UploadedPart,
    ) -> Option<PartUpload> {
        let held_bucket = self.buckets.get(bucket)?;
        let Some(upload) = held_bucket
            .multipart_upload(upload_id)
            .filter(|upload| upload.key == key)
        else {
            return Some(PartUpload::NoSuchUpload);
        };

        // What this write releases: the part the number already held, whose bytes go away
        // as it is overwritten — so the budgets are over what the store WOULD hold, not
        // over everything ever uploaded.
        let released = upload
            .part(part.part_number)
            .map_or(0, UploadedPart::retained_bytes);
        let incoming = part.retained_bytes();

        let would_hold_in_bucket = held_bucket.retained_bytes().saturating_sub(released) + incoming;
        if would_hold_in_bucket > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(PartUpload::ByteBudgetReached {
                would_hold: would_hold_in_bucket,
            });
        }
        // The whole-store budget measures the caller-stored half of the store
        // ([`Self::caller_retained_bytes`]); a part uploaded into a bucket AWS publishes
        // counts against it like any other.
        let would_hold: usize = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + incoming;
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(PartUpload::TotalByteBudgetReached { would_hold });
        }

        let replaced = self
            .buckets
            .get_mut(bucket)?
            .multipart_uploads
            .get_mut(upload_id)?
            .parts
            .insert(part.part_number, part)
            .is_some();
        Some(PartUpload::Stored { replaced })
    }

    /// Discard the multipart upload `upload_id` names in the bucket named `bucket`,
    /// reporting what that did — or `None` when this emulator holds no such bucket (so an
    /// abort refuses a name that is not a bucket instead of reporting success for it).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, exactly as
    /// [`Self::upload_part`] and a read of the upload address it, so an id named under
    /// another key names no upload.
    ///
    /// What the address holds decides which of the three outcomes goes back: the upload
    /// itself ([`UploadAbort::Discarded`]), an upload this bucket HELD there and a completion
    /// or an earlier abort has taken away ([`UploadAbort::AlreadyDiscarded`], the state an
    /// abort asks for), or nothing this bucket ever held there
    /// ([`UploadAbort::NoSuchUpload`]).
    ///
    /// The upload goes away with every part it held — "the storage consumed by any
    /// previously uploaded parts will be freed" (`AbortMultipartUpload`'s own
    /// documentation) — so the bytes those parts retained stop counting against the
    /// bucket's budget ([`Bucket::retained_bytes`]) and the upload stops being listed. ONE
    /// upload is discarded: every other upload the bucket has in progress, including
    /// another upload of the same key, is left exactly as it was.
    pub fn abort_multipart_upload(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
    ) -> Option<UploadAbort> {
        let held_bucket = self.buckets.get_mut(bucket)?;
        if held_bucket
            .multipart_upload(upload_id)
            .is_none_or(|upload| upload.key != key)
        {
            return Some(match held_bucket.held_multipart_upload(key, upload_id) {
                true => UploadAbort::AlreadyDiscarded,
                false => UploadAbort::NoSuchUpload,
            });
        }
        held_bucket.multipart_uploads.remove(upload_id);
        // The address is retired as the upload leaves, so a second abort of the same id sees
        // an upload that is already gone rather than an id that names nothing.
        held_bucket.retire_multipart_upload(key, upload_id);
        Some(UploadAbort::Discarded)
    }

    /// Turn the multipart upload `upload_id` names in the bucket named `bucket` into
    /// `object` at `key`, reporting what that did — or `None` when this emulator holds no
    /// such bucket (so a completion refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, exactly as
    /// [`Self::upload_part`] and [`Self::abort_multipart_upload`] address it, so an id named
    /// under another key names no upload ([`UploadCompletion::NoSuchUpload`]) and nothing is
    /// written.
    ///
    /// The upload is taken out BEFORE the object is written, because the parts BECOME the
    /// object: "upon receiving this request, Amazon S3 concatenates all the parts in
    /// ascending order by part number to create a new object"
    /// (`CompleteMultipartUpload`'s own documentation), and the storage the parts held is
    /// not held twice. That is also what keeps the completion inside the byte budgets a
    /// store this size has room for: the bytes the parts retained are released as the same
    /// bytes are recorded under the key.
    ///
    /// The write goes through [`Self::put_object`], the one place an object is recorded, so
    /// a completed upload is subject to exactly the ceilings a one-shot write is and mints
    /// its version id the same way. A write that is REFUSED leaves the upload in progress
    /// with every part it held, so a caller can retry it rather than losing the parts.
    pub fn complete_multipart_upload(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
        object: Object,
    ) -> Option<UploadCompletion> {
        let held_bucket = self.buckets.get_mut(bucket)?;
        if held_bucket
            .multipart_upload(upload_id)
            .is_none_or(|upload| upload.key != key)
        {
            return Some(UploadCompletion::NoSuchUpload);
        }
        let Some(upload) = held_bucket.multipart_uploads.remove(upload_id) else {
            return Some(UploadCompletion::NoSuchUpload);
        };

        let write = self.put_object(bucket, key, object);
        match write {
            Some(write @ (ObjectWrite::Created | ObjectWrite::Replaced)) => {
                // The address is retired as the upload becomes the object, so a cleanup
                // abort of the id the completion consumed sees an upload that is already
                // gone rather than an id that names nothing.
                if let Some(held_bucket) = self.buckets.get_mut(bucket) {
                    held_bucket.retire_multipart_upload(key, upload_id);
                }
                Some(UploadCompletion::Completed(write))
            }
            // Either refusal leaves the upload where it was: the bucket is reached again
            // because the write borrowed the whole store, and the upload goes back under
            // the id it was addressed by. The bucket is still held — the write above was
            // measured against it and nothing in between removes a bucket — so the upload
            // really does go back; the conditional is what keeps a store that somehow no
            // longer held it from panicking on a request path, there being nothing left to
            // put the upload into.
            refused => {
                if let Some(held_bucket) = self.buckets.get_mut(bucket) {
                    held_bucket
                        .multipart_uploads
                        .insert(upload_id.to_string(), upload);
                }
                refused.map(UploadCompletion::Refused)
            }
        }
    }

    /// Record `configuration` as the whole notification configuration of the bucket named
    /// `bucket`, reporting `None` when this emulator holds no such bucket (so a put
    /// refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT, not a merge: "This action replaces the existing notification
    /// configuration with the configuration you include in the request body"
    /// (`PutBucketNotificationConfiguration`'s own documentation), so what the bucket
    /// carries afterwards is exactly what this request carried — an empty container
    /// included, which turns notifications off.
    ///
    /// Every record that arrives without an `Id` is given one here, because "if you don't
    /// provide one, Amazon S3 will assign an ID" (`NotificationId`). A minted id is
    /// recorded with the configuration, so every later read reports the same one, and it
    /// never collides with an id the request named itself.
    pub fn put_notification_configuration(
        &mut self,
        bucket: &str,
        mut configuration: BucketNotificationConfiguration,
    ) -> Option<()> {
        if !self.buckets.contains_key(bucket) {
            return None;
        }

        let mut taken: BTreeSet<String> = configuration
            .targets()
            .filter_map(|(_, target)| target.id.clone())
            .collect();
        for target in configuration.targets_mut() {
            if target.id.is_none() {
                let id = self.mint_notification_configuration_id(&taken);
                taken.insert(id.clone());
                target.id = Some(id);
            }
        }

        let held = self.buckets.get_mut(bucket)?;
        held.record_configuration_write();
        held.notification = configuration;
        Some(())
    }

    /// Record `rules` as the whole lifecycle configuration of the bucket named `bucket`,
    /// under the `transition_default_minimum_object_size` behaviour the put applied,
    /// reporting `None` when this emulator holds no such bucket (so a put refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT, not a merge: "this will overwrite an existing lifecycle
    /// configuration, so if you want to retain any configuration details, they must be
    /// included in the new lifecycle configuration"
    /// (`PutBucketLifecycleConfiguration`'s own documentation), so what the bucket carries
    /// afterwards is exactly what this request carried.
    ///
    /// Every rule that arrives without an `ID` is given one here, because the member is the
    /// rule's "unique identifier" and a read reports one for every rule. A minted
    /// identifier is recorded with the configuration, so every later read reports the same
    /// one, and it never collides with an identifier the request named itself.
    pub fn put_lifecycle_configuration(
        &mut self,
        bucket: &str,
        mut rules: Vec<crate::ops::types::LifecycleRule>,
        transition_default_minimum_object_size: String,
    ) -> Option<()> {
        if !self.buckets.contains_key(bucket) {
            return None;
        }

        let mut taken: BTreeSet<String> = rules.iter().filter_map(|rule| rule.id.clone()).collect();
        for rule in &mut rules {
            if rule.id.is_none() {
                let id = self.mint_lifecycle_rule_id(&taken);
                taken.insert(id.clone());
                rule.id = Some(id);
            }
        }

        let held = self.buckets.get_mut(bucket)?;
        held.record_configuration_write();
        held.lifecycle = Some(BucketLifecycle {
            rules,
            transition_default_minimum_object_size,
        });
        Some(())
    }

    /// An opaque rule identifier no rule of this configuration carries.
    ///
    /// S3's own assigned identifiers are opaque strings, so this is derived the way
    /// [`Self::mint_notification_configuration_id`] derives a notification id: the MD5
    /// digest of the store's monotonic counter, rendered as hex. Uniqueness comes from the
    /// counter, not from the digest, and an identifier a request named itself (`taken`) is
    /// stepped over rather than reused.
    pub(super) fn mint_lifecycle_rule_id(&mut self, taken: &BTreeSet<String>) -> String {
        loop {
            self.minted_lifecycle_rule_ids += 1;
            let mut digest = Md5::new();
            digest.update(b"s3-lifecycle-rule-id:");
            digest.update(self.minted_lifecycle_rule_ids.to_be_bytes());
            let id = hex::encode(digest.finalize());
            if !taken.contains(&id) {
                return id;
            }
        }
    }

    /// An opaque configuration id no configuration in this store carries.
    ///
    /// S3's own assigned ids are opaque strings, so this is the MD5 digest of the counter
    /// above rendered as hex — the digest helper this module already uses for an object's
    /// ETag. Uniqueness comes from the counter, not from the digest, and an id a request
    /// named itself (`taken`) is stepped over rather than reused.
    pub(super) fn mint_notification_configuration_id(&mut self, taken: &BTreeSet<String>) -> String {
        loop {
            self.minted_notification_configuration_ids += 1;
            let mut digest = Md5::new();
            digest.update(b"s3-notification-configuration-id:");
            digest.update(self.minted_notification_configuration_ids.to_be_bytes());
            let id = hex::encode(digest.finalize());
            if !taken.contains(&id) {
                return id;
            }
        }
    }

    /// Replace the tag set of the object version `version_id` names at `key` in the bucket
    /// named `bucket` — the key's CURRENT object when it names none — reporting what that did,
    /// or `None` when this emulator holds no such bucket (so a tagging write refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole tag set, the way [`Bucket::replace_tags`] replaces a
    /// bucket's: `Tagging` is the complete container the request carries, so what the
    /// object holds afterwards is exactly what this call carried and nothing of the set
    /// before it — an empty `TagSet` therefore leaves the object with no tags.
    ///
    /// Version-scoped, because a tag set is a VERSION's: "to add tags to the current version,
    /// use the versionId query parameter" and "tags are stored at the version level"
    /// (`PutObjectTagging` and `GetObjectTagging`), so a write that names a version leaves
    /// every other version's tag set exactly as it was — the way
    /// [`Self::replace_object_retention`] and [`Self::replace_object_legal_hold`] address a
    /// version.
    ///
    /// This is the ONLY way an object's tag set is recorded, because it is the only place
    /// that can see BOTH the per-bucket byte budget and the service-wide one: a tag set is
    /// retained with the object ([`Object::retained_bytes`]) and is not bounded by
    /// anything [`Self::put_object`] checked, so a tag set that would take the store past
    /// either budget is refused rather than recorded. Nothing is written unless the answer
    /// is [`ObjectTagWrite::Applied`].
    pub fn replace_object_tags(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        tags: Vec<ResourceTag>,
    ) -> Option<ObjectTagWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectTagWrite::NoSuchKey);
        };

        // What the bucket and the whole store would retain with this tag set in place of
        // the object's current one: everything they retain now, less the bytes the tag set
        // being replaced holds, plus the bytes of the replacement.
        let released = tag_set_bytes(&object.tags);
        let taken = tag_set_bytes(&tags);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectTagWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectTagWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.tags = tags;
                ObjectTagWrite::Applied
            })
    }

    /// Replace the Object Lock retention of the object version `version_id` names in
    /// `bucket` — the key's current object when it names none — with `retention`, or take
    /// the retention away when that is `None`. Reports what the write did, or `None` when
    /// this emulator holds no such bucket (so a retention write refuses a name that is not
    /// a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_tags`]
    /// replaces a tag set: `Retention` is "the container element for the Object Retention
    /// configuration" (`PutObjectRetentionRequest$Retention`), so what the version carries
    /// afterwards is exactly what this call carried and nothing of the configuration
    /// before it.
    ///
    /// This decides NOTHING about whether the write is ALLOWED: whether a retention in
    /// force bars it, and whether the caller asked to bypass a governance one, is the
    /// handler's judgement over [`ObjectRetention::in_force_at`] and
    /// [`ObjectRetention::weakens`] — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an object's retention is recorded, because it is the only place
    /// that can see BOTH the per-bucket byte budget and the service-wide one: a retention
    /// is retained with the object ([`Object::retained_bytes`]) and is not bounded by
    /// anything [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectRetentionWrite::Applied`].
    pub fn replace_object_retention(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        retention: Option<ObjectRetention>,
    ) -> Option<ObjectRetentionWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectRetentionWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this retention in place of
        // the version's current one: everything they retain now, less the bytes the record
        // being replaced holds, plus the bytes of the replacement.
        let released = object
            .retention
            .as_ref()
            .map_or(0, ObjectRetention::retained_bytes);
        let taken = retention
            .as_ref()
            .map_or(0, ObjectRetention::retained_bytes);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectRetentionWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectRetentionWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.retention = retention;
                ObjectRetentionWrite::Applied
            })
    }

    /// Record `encryption` as the server-side encryption configuration of the object
    /// version `version_id` names in `bucket` — the key's current object when it names
    /// none. Reports what the write did, or `None` when this emulator holds no such bucket
    /// (so an encryption update refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// A REPLACEMENT of the object's whole encryption record, the way
    /// [`Self::replace_object_retention`] replaces a retention: `ObjectEncryption` is "the
    /// updated server-side encryption type for this object"
    /// (`UpdateObjectEncryptionRequest$ObjectEncryption`), so what the version carries
    /// afterwards is exactly what this call carried. Nothing ELSE about the object changes:
    /// "the UpdateObjectEncryption operation preserves all object metadata properties,
    /// including the storage class, creation date, last modified date, ETag, and checksum
    /// properties" (that operation's own documentation).
    ///
    /// It is the ONLY way an existing object's encryption is re-recorded, because it is the
    /// only place that can see BOTH the per-bucket byte budget and the service-wide one: an
    /// encryption configuration is retained with the object
    /// ([`ObjectEncryption::retained_bytes`]). Nothing is written unless the answer is
    /// [`ObjectEncryptionWrite::Applied`].
    pub fn replace_object_encryption(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        encryption: ObjectEncryption,
    ) -> Option<ObjectEncryptionWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectEncryptionWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this configuration in place
        // of the object's current one, measured exactly as a retention replacement is.
        let released = object.encryption.retained_bytes();
        let taken = encryption.retained_bytes();
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectEncryptionWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self.caller_retained_bytes().saturating_sub(released) + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectEncryptionWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.encryption = encryption;
                ObjectEncryptionWrite::Applied
            })
    }

    /// Record `legal_hold` as the Object Lock legal hold status of the object version
    /// `version_id` names in `bucket` — the key's current object when it names none.
    /// Reports what the write did, or `None` when this emulator holds no such bucket (so a
    /// legal hold write refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_retention`]
    /// replaces a retention: `LegalHold` is the "container element for the legal hold
    /// configuration you want to apply to the specified object"
    /// (`PutObjectLegalHoldRequest$LegalHold`), which holds one status, so what the version
    /// carries afterwards is exactly the status this call named.
    ///
    /// No byte budget is consulted, unlike the retention seam: what is kept is a flag, not
    /// a caller-supplied string, so a hold adds nothing to what the bucket retains.
    pub fn replace_object_legal_hold(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        legal_hold: bool,
    ) -> Option<ObjectLegalHoldWrite> {
        let held = self.buckets.get_mut(bucket)?;
        let Some(object) = held.addressed_object_mut(key, version_id) else {
            return Some(ObjectLegalHoldWrite::NoSuchObject);
        };
        object.legal_hold = Some(legal_hold);
        Some(ObjectLegalHoldWrite::Applied)
    }

    /// Replace the access control list of the object version `version_id` names in
    /// `bucket` — the key's current object when it names none — with `grants`. Reports what
    /// the write did, or `None` when this emulator holds no such bucket (so an ACL write
    /// refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_tags`]
    /// replaces a tag set: `PutObjectAcl` "uses the acl subresource to set the access
    /// control list (ACL) permissions for a new or existing object" from the ONE ACL the
    /// request names, so what the version carries afterwards is exactly those grants and
    /// nothing of the ACL before it.
    ///
    /// This decides NOTHING about whether the write is ALLOWED: whether the bucket's Object
    /// Ownership setting disables ACLs ([`Bucket::acls_disabled`]) and whether its Block
    /// Public Access settings bar a public one ([`acl_grants_public_access`]) are the
    /// handler's judgement — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an object's ACL is recorded, because it is the only place that
    /// can see BOTH the per-bucket byte budget and the service-wide one: an ACL is retained
    /// with the object ([`Object::retained_bytes`]) and is not bounded by anything
    /// [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectAclWrite::Applied`].
    pub fn replace_object_acl(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        grants: Vec<AclGrant>,
    ) -> Option<ObjectAclWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectAclWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this ACL in place of the
        // version's current one: everything they retain now, less the bytes the ACL being
        // replaced holds, plus the bytes of the replacement.
        let released = acl_grants_bytes(object.acl.as_deref().unwrap_or_default());
        let taken = acl_grants_bytes(&grants);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectAclWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self.caller_retained_bytes().saturating_sub(released) + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectAclWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.acl = Some(grants);
                ObjectAclWrite::Applied
            })
    }

    /// Record `restore` on the object version `version_id` names in `bucket` — the key's
    /// current object when it names none. Reports what the write did, or `None` when this
    /// emulator holds no such bucket (so a restore refuses a name that is not a bucket
    /// instead of creating one).
    ///
    /// A REPLACEMENT of the whole record, the way [`Self::replace_object_retention`]
    /// replaces a retention: a second `RestoreObject` on a version states the lifetime and
    /// the tier of the copy afresh, so what the version carries afterwards is exactly what
    /// this call carried.
    ///
    /// It decides NOTHING about whether the restore is ALLOWED: whether the version is
    /// archived at all, and whether a retrieval is already running on it, is the handler's
    /// judgement over [`ARCHIVED_STORAGE_CLASSES`] and [`ObjectRestore::in_progress_at`] —
    /// this is the storage seam those answers end at. It restores no BYTES either: an
    /// archived version's bytes are the ones this store already holds, and what a restore
    /// changes is the state every reader reports through `x-amz-restore`.
    ///
    /// It is the ONLY way a restore is recorded, because it is the only place that can see
    /// BOTH the per-bucket byte budget and the service-wide one: the record is retained
    /// with the object ([`Object::retained_bytes`]) and is not bounded by anything
    /// [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectRestoreWrite::Applied`].
    pub fn restore_object_version(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        restore: ObjectRestore,
    ) -> Option<ObjectRestoreWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectRestoreWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this record in place of the
        // version's current one: everything they retain now, less the bytes the record
        // being replaced holds, plus the bytes of the replacement.
        let released = object
            .restore
            .as_ref()
            .map_or(0, ObjectRestore::retained_bytes);
        let taken = restore.retained_bytes();
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectRestoreWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectRestoreWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.restore = Some(restore);
                ObjectRestoreWrite::Applied
            })
    }

    /// Record `annotation` under `name` on the object version `version_id` names in
    /// `bucket` — the key's current object when it names none. Reports what the write did,
    /// or `None` when this emulator holds no such bucket (so an annotation write refuses a
    /// name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the one annotation `name` addresses, and of nothing else: an
    /// annotation is "a named payload … that you can associate with a specific object or
    /// object version" (`PutObjectAnnotation`'s own documentation), so a second write under
    /// the same name is the whole new payload of that name and leaves every other
    /// annotation of the object where it was.
    ///
    /// It decides NOTHING about whether the name or the payload is acceptable: the name's
    /// bounds are [`annotation_name_rejection`]'s judgement and the payload's are the
    /// handler's over [`MIN_ANNOTATION_PAYLOAD_BYTES`] and
    /// [`MAX_ANNOTATION_PAYLOAD_BYTES`] — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an annotation is recorded, because it is the only place that can
    /// see BOTH the per-object ceiling ([`MAX_ANNOTATIONS_PER_OBJECT`]) and the two byte
    /// budgets: an annotation is retained with the object ([`Object::retained_bytes`]) and
    /// is not bounded by anything [`Self::put_object`] checked. Nothing is written unless
    /// the answer is [`AnnotationWrite::Applied`].
    pub fn put_object_annotation(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        name: &str,
        annotation: Annotation,
    ) -> Option<AnnotationWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(AnnotationWrite::NoSuchObject);
        };

        // "Each object can have up to 1,000 annotations", so only a write that names a
        // name the object does not carry yet can take it past the ceiling: one that
        // replaces an existing annotation leaves the count where it is.
        let replaces = object.annotation(name);
        if replaces.is_none() && object.annotations.len() >= MAX_ANNOTATIONS_PER_OBJECT {
            return Some(AnnotationWrite::LimitReached);
        }

        // What the bucket and the whole store would retain with this annotation in place
        // of the one that name carries: everything they retain now, less the bytes that
        // annotation holds, plus the bytes of the replacement — and the NAME's own bytes
        // too when the name is new, since those are retained with it.
        let released = replaces.map_or(0, Annotation::retained_bytes);
        let taken = annotation.retained_bytes()
            + if replaces.is_none() { name.len() } else { 0 };
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(AnnotationWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(AnnotationWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.annotations.insert(name.to_string(), annotation);
                AnnotationWrite::Applied
            })
    }

    /// Remove the annotation `name` addresses from the object version `version_id` names
    /// in `bucket` — the key's current object when it names none. Reports what the removal
    /// did, or `None` when this emulator holds no such bucket (so a delete refuses a name
    /// that is not a bucket instead of reporting a removal from nothing).
    ///
    /// PERMANENT, and of the one annotation `name` addresses alone: "Deleting an annotation
    /// is permanent. Annotations are not independently versioned, so there is no delete
    /// marker or way to recover a deleted annotation" (`DeleteObjectAnnotation`'s own
    /// documentation), so the payload and everything it retained are released and every
    /// other annotation of the object stays where it was.
    ///
    /// The counterpart of [`Self::put_object_annotation`] and the ONLY way an annotation is
    /// taken away, so the bytes an annotation retains are released in exactly one place;
    /// unlike a write it is bounded by nothing, because a removal only gives room back.
    pub fn remove_object_annotation(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        name: &str,
    ) -> Option<AnnotationRemoval> {
        let held = self.buckets.get_mut(bucket)?;
        let Some(object) = held.addressed_object_mut(key, version_id) else {
            return Some(AnnotationRemoval::NoSuchObject);
        };
        Some(match object.annotations.remove(name) {
            Some(_) => AnnotationRemoval::Removed,
            None => AnnotationRemoval::NoSuchAnnotation,
        })
    }

    /// Record a bucket named `name` in `region`, reporting what happened.
    ///
    /// Nothing is written unless the answer is [`BucketCreation::Created`]: a taken name
    /// leaves the existing bucket alone, and a create past
    /// [`MAX_BUCKETS_PER_ACCOUNT`] is refused, which is what bounds this map.
    pub fn create_bucket(&mut self, name: &str, region: &str) -> BucketCreation {
        self.record_bucket(name, region, None, false)
    }

    /// Record a bucket named `name` in `region` that `provisioned_by` — another emulated
    /// service — stands up as the object store of one of ITS resources, reporting what
    /// happened.
    ///
    /// This is the one way a service other than S3 gives a resource of its own an
    /// S3-compatible object data plane: every object operation here serves the bucket
    /// (that is the point of provisioning it), while [`Self::account_buckets`] leaves it
    /// out. See [`Bucket::provisioned_by`].
    ///
    /// The same guarantees as [`Self::create_bucket`], which it shares its body with: a
    /// name this store already holds is [`BucketCreation::NameTaken`] and writes NOTHING,
    /// so a provisioning service learns the name is taken instead of taking over the
    /// bucket that holds it — one bucket namespace, whoever provisioned the bucket in it.
    /// What it does NOT share is the account's bucket quota: see
    /// [`MAX_BUCKETS_PER_ACCOUNT`].
    pub fn provision_bucket(
        &mut self,
        name: &str,
        region: &str,
        provisioned_by: &str,
    ) -> BucketCreation {
        self.record_bucket(name, region, Some(provisioned_by), false)
    }

    /// Record every bucket AWS publishes ([`aws_published::PUBLISHED_BUCKETS`]) and store
    /// the objects it publishes under it.
    ///
    /// Called from [`Default`] and nowhere else: a published bucket is in the store before
    /// the first request is served, because nothing creates it. Each object goes in through
    /// [`Self::put_object`], the one way an object is recorded, so a published object is
    /// held exactly the way a caller's is and every reader serves it through the same path.
    ///
    /// Nothing here can refuse: the catalog is fixed at compile time, so whether each entry
    /// is recorded is settled by the catalog rather than by anything a caller does — which
    /// is why the outcomes are dropped instead of raising anything on a request's path. That
    /// every entry IS recorded, with the bytes the catalog publishes, is pinned by
    /// `the_default_store_holds_every_published_object` below and over the wire by
    /// `tests/all/aws_published_sample_data.rs`.
    pub(super) fn record_published_buckets(&mut self) {
        for published in aws_published::PUBLISHED_BUCKETS {
            let _ = self.record_bucket(published.name, published.region, None, true);
            for object in published.objects {
                let body = object.body.as_bytes().to_vec();
                let stored = Object {
                    etag: object_etag(&body),
                    body,
                    // The instant the store came into being, which is the only instant
                    // this emulator can report for a published object; nothing moves it
                    // afterwards, so every read reports the same `Last-Modified`.
                    last_modified: SystemTime::now(),
                    storage_class: DEFAULT_STORAGE_CLASS.to_string(),
                    content_type: Some(object.content_type.to_string()),
                    cache_control: None,
                    content_disposition: None,
                    content_encoding: None,
                    content_language: None,
                    expires: None,
                    website_redirect_location: None,
                    metadata: BTreeMap::new(),
                    tags: Vec::new(),
                    encryption: ObjectEncryption::default(),
                    // The store mints a version id only for a versioning-enabled bucket,
                    // and a published bucket carries no versioning state.
                    version_id: None,
                    // No Object Lock retention, no legal hold, no restore and no
                    // annotations: nothing placed one, and a published object is readable
                    // without one being asked for.
                    retention: None,
                    legal_hold: None,
                    restore: None,
                    annotations: BTreeMap::new(),
                    // Nor an ACL: a published object carries the ACL every object starts
                    // with, the owner's `FULL_CONTROL`, which a reader derives rather than
                    // this recording it.
                    acl: None,
                    // And no checksum: the catalog publishes bytes and a content type, so
                    // nothing named a checksum algorithm for these objects to be stored
                    // under.
                    checksum: None,
                    // Nor parts: a published object is stored whole.
                    parts: Vec::new(),
                    // And no replication: a published bucket carries no replication
                    // configuration, so nothing replicates its objects.
                    replication: None,
                };
                let _ = self.put_object(published.name, object.key, stored);
            }
        }

        // What the catalog itself holds, measured once now that it is all in: the fixed
        // amount the whole-store budgets subtract, so the catalog takes no room from a
        // caller's own objects while anything a caller writes into a published bucket
        // still counts. See [`Self::published_retained_bytes`].
        self.published_retained_bytes = self
            .buckets
            .values()
            .filter(|bucket| bucket.aws_published)
            .map(Bucket::retained_bytes)
            .sum();
        self.published_versions = self
            .buckets
            .values()
            .filter(|bucket| bucket.aws_published)
            .map(Bucket::stored_versions)
            .sum();
    }

    /// Record a bucket named `name` in `region`, provisioned by `provisioned_by` (`None`
    /// for one created over S3's own API) and published by AWS when `aws_published`,
    /// reporting what happened.
    ///
    /// Private: a create comes in through [`Self::create_bucket`] or
    /// [`Self::provision_bucket`], which is what keeps both subject to the same taken-name
    /// and quota checks, and the published catalog through
    /// [`Self::record_published_buckets`], which runs before any request does.
    pub(super) fn record_bucket(
        &mut self,
        name: &str,
        region: &str,
        provisioned_by: Option<&str>,
        aws_published: bool,
    ) -> BucketCreation {
        if self.buckets.contains_key(name) {
            return BucketCreation::NameTaken;
        }
        // The quota is the ACCOUNT's, so it bounds the account's own buckets and is
        // measured over them: a bucket another service provisioned is not one of them (it
        // is not even reported among them, see [`Self::account_buckets`]) and is bounded
        // by that service's own limit on the resource it belongs to, so it neither
        // consumes this quota nor is refused by it. Neither is a bucket AWS publishes,
        // which is nobody's account's and is fixed by the catalog.
        if provisioned_by.is_none()
            && !aws_published
            && self.account_buckets().count() >= MAX_BUCKETS_PER_ACCOUNT
        {
            return BucketCreation::QuotaReached;
        }
        self.buckets.insert(
            name.to_string(),
            Bucket {
                region: region.to_string(),
                provisioned_by: provisioned_by.map(str::to_string),
                aws_published,
                // The creation instant is read here, where the bucket comes into being,
                // so `CreationDate` is the moment the store accepted it.
                created_at: Some(SystemTime::now()),
                // "Amazon S3 automatically enables S3 Block Public Access … for new
                // buckets" (Amazon S3 User Guide, Blocking public access to your Amazon S3
                // storage): a bucket is fully blocked from the moment it exists, so the
                // subresource is associated with it here — the same way its Object
                // Ownership setting is — and a read of it reports four switches rather than
                // "there is none". A bucket from AWS's published catalog predates that
                // default and is public, so it carries no configuration.
                public_access_block: (!aws_published)
                    .then(BucketPublicAccessBlock::all_enabled),
                // A fresh bucket carries no policy, no tags and no ownership controls, and
                // holds no objects; `PutBucketPolicy`, `PutBucketTagging` and
                // `PutBucketOwnershipControls` are what associate those subresources with
                // it and `PutObject` what puts an object in it.
                ..Bucket::default()
            },
        );
        BucketCreation::Created
    }

    /// Delete `key` in the bucket named `bucket` the way a `DeleteObject` that named no
    /// `versionId` does, reporting what that did — or `None` when this emulator holds no
    /// such bucket, so a delete can tell "no such bucket" from "nothing at that key".
    ///
    /// What it does turns on the bucket's versioning state, which [`Bucket::remove_object`]
    /// documents case by case. This is where the DELETE MARKER's version id is minted,
    /// because this is the only place that sees both that state and the store's version
    /// sequence: a delete on a bucket whose status is `Enabled` mints a fresh id for the
    /// marker it lays, exactly as a write mints one for the version it stores.
    ///
    /// A marker is a stored version, so it is bounded like one: this refuses to lay it
    /// once the whole store holds [`MAX_OBJECTS_TOTAL`] versions, and the per-bucket
    /// ceiling is checked inside. Nothing is written when the answer is a ceiling.
    pub fn remove_object(&mut self, bucket: &str, key: &str) -> Option<ObjectDeletion> {
        let held = self.buckets.get(bucket)?;
        let versioned = held.versioning_enabled();
        // Only a delete that lays a marker the store does not already hold a slot for adds
        // an entry: a bucket with no versioning state takes one away, and a `null` marker
        // displaces the `null` version it covers. Just that case is bounded here, and only
        // that case pays for the pass over every bucket.
        let adds_version = held.versioning.is_some()
            && (versioned || held.null_version_position(key).is_none());
        if adds_version {
            let held_versions: usize = self.caller_stored_versions();
            if held_versions >= MAX_OBJECTS_TOTAL {
                return Some(ObjectDeletion::TotalVersionCountReached);
            }
        }
        // Minted before the ceilings below are measured, the way a write's id is; a
        // refusal leaves it unused, which costs nothing but the sequence number.
        let minted = versioned.then(|| {
            self.minted_versions = self.minted_versions.saturating_add(1);
            mint_version_id(bucket, key, self.minted_versions)
        });
        self.buckets
            .get_mut(bucket)
            .map(|held| held.remove_object(key, minted))
    }

    /// Remove the version of `key` that `version_id` names from the bucket named
    /// `bucket`, reporting whether the bucket held such a version — or `None` when this
    /// emulator holds no such bucket, so a delete can tell "no such bucket" from "no such
    /// version".
    ///
    /// This is the permanent delete a versioned bucket needs ("To permanently delete an
    /// object in a versioned bucket, you must include the object's versionId in the
    /// request"), and the only way a stored version's bytes are released, which is what
    /// lets a later write back into the byte budgets [`Self::put_object`] checks.
    pub fn remove_object_version(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Option<bool> {
        self.buckets
            .get_mut(bucket)
            .map(|held| held.remove_version(key, version_id))
    }

    /// Remove the bucket named `bucket_name`, releasing everything it still held — its
    /// objects, and from the managed table bucket's namespace the names of its metadata
    /// configuration's tables that never came into service
    /// ([`Self::release_unprovisioned_metadata_tables`]).
    /// `false` when this emulator holds no such bucket.
    pub fn remove_bucket(&mut self, bucket_name: &str, now: SystemTime) -> bool {
        let Some(mut held) = self.buckets.remove(bucket_name) else {
            return false;
        };
        if let Some(configuration) = held.take_metadata_configuration() {
            self.release_unprovisioned_metadata_tables(&configuration, now);
        }
        true
    }
}
