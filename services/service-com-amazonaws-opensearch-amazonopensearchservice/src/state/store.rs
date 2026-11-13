// SPDX-License-Identifier: CC0-1.0

//! The store itself, split out of [`crate::state`] unchanged: the struct every
//! operation reaches this service's records through, the ids and tokens it mints, and
//! the settling each record kind is materialised by.

use super::*;

/// Per-service state for com-amazonaws-opensearch-amazonopensearchservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Every created domain, keyed by its name — unique per account and Region, so
    /// the name IS the key. Iterating a `BTreeMap` is order-stable, which is what
    /// lets `ListDomainNames` report the same order twice.
    pub domains: BTreeMap<String, Domain>,
    /// Every cross-cluster connection, keyed by its `ConnectionId` — unique per
    /// connection, so the id IS the key. Iterating a `BTreeMap` is order-stable,
    /// which is what lets `DescribeOutboundConnections` report the same order twice
    /// and page through the set without repeating or skipping a record.
    pub connections: BTreeMap<String, Connection>,
    /// Drives [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_connection_id`].
    pub connection_seq: u64,
    /// Drives [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_endpoint_suffix`].
    pub endpoint_suffix_seq: u64,
    /// The Reserved Instance offering catalog this emulator publishes, keyed by
    /// `ReservedInstanceOfferingId`. Seeded by [`seed_reserved_instance_offerings`]
    /// and never written again: an offering is published by the service, so no
    /// operation creates, changes or removes one. The id is the key so a purchase and
    /// the offering-id filter resolve one entry by it; the ORDER a listing publishes the
    /// catalog in is
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::published_reserved_instance_offerings`]'s,
    /// which is what lets a paged read resume where the previous page stopped.
    pub reserved_instance_offerings: BTreeMap<String, ReservedInstanceOffering>,
    /// The default-application setting: the ARN of the application that is the
    /// account's default, or `None` while no default is set.
    ///
    /// ONE account-scoped value rather than a flag on a per-application record:
    /// `PutDefaultApplicationSetting` sets or clears it and
    /// `GetDefaultApplicationSetting` reports it, so the two cannot disagree about
    /// which application is the default.
    pub default_application_arn: Option<String>,
    /// Every purchased Reserved Instance, keyed by its `ReservedInstanceId` — "the
    /// unique identifier for the reservation", so the id IS the key.
    /// `PurchaseReservedInstanceOffering` mints one and
    /// `DescribeReservedInstances` reports it, from THIS one record.
    pub reserved_instances: BTreeMap<String, ReservedInstanceRecord>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_billing_subscription_id`].
    pub billing_subscription_seq: i64,
    /// Every created package, keyed by its `PackageID` — unique per package, so the
    /// id IS the key. `CreatePackage` mints one and `DescribePackages` reports it,
    /// from THIS one record. Iterating a `BTreeMap` is order-stable, which is what
    /// lets a paged read of the set resume where the previous page stopped without
    /// repeating or skipping a record.
    pub packages: BTreeMap<String, Package>,
    /// The insights held for the ACCOUNT itself — the ones an `Account` entity and
    /// no domain answers for — keyed by their `InsightId`. The insights of a domain
    /// live on that domain ([`Domain::insights`]), so an insight has exactly one
    /// home whichever entity holds it. See [`InsightRecord`].
    pub account_insights: BTreeMap<String, InsightRecord>,
    /// Whether this account may share its packages with other users at all — the
    /// entitlement `UpdatePackageScope` needs before it can grant one.
    ///
    /// ONE account-scoped value rather than a flag per package: it says what the
    /// ACCOUNT may do, so every package of the account is judged by the same value. No
    /// operation of this API carries it and no setting seeds it — a package vendor is
    /// admitted to the programme out of band — so it starts `false`, which is where an
    /// account that has not been admitted stands, and every scope update from such an
    /// account is refused. There is therefore no caller-visible seam that turns it on:
    /// over the wire the refusal is all `UpdatePackageScope` can answer, and the grant
    /// path is reached by a caller holding this store (a test, or a future operation
    /// for an admitted account).
    pub package_vending_entitled: bool,
    /// Every service-managed VPC endpoint of this account, keyed by its
    /// `VpcEndpointId` — unique per endpoint, so the id IS the key.
    /// `CreateVpcEndpoint` mints one and `DescribeVpcEndpoints` / `ListVpcEndpoints`
    /// report it, from THIS one record. Iterating a `BTreeMap` is order-stable, which
    /// is what lets a paged listing resume where the previous page stopped without
    /// repeating or skipping a record. Bounded by [`VPC_ENDPOINT_QUOTA`].
    pub vpc_endpoints: BTreeMap<String, VpcEndpointRecord>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_vpc_endpoint_id`].
    pub vpc_endpoint_seq: u64,
    /// Every OpenSearch UI application of this account, keyed by its `Id` — unique
    /// per application, so the id IS the key. `CreateApplication` mints one and
    /// `GetApplication` reports it, from THIS one record. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads of the set report it in the same
    /// order.
    pub applications: BTreeMap<String, Application>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_application_id`].
    pub application_seq: u64,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_attachment_id`].
    pub attachment_seq: u64,
    /// Every direct query data source of this account, keyed by its
    /// `DataSourceName` — "a unique, user-defined label to identify the data source
    /// within your OpenSearch Service environment", so the name IS the key.
    /// `AddDirectQueryDataSource` mints one and `GetDirectQueryDataSource` reports
    /// it, from THIS one record. Iterating a `BTreeMap` is order-stable, which is
    /// what lets two reads of the set report it in the same order. Bounded by
    /// [`DIRECT_QUERY_DATA_SOURCE_QUOTA`].
    pub direct_query_data_sources: BTreeMap<String, DirectQueryDataSourceRecord>,
    /// Every saved-object migration job of this account, keyed by its `migrationId` —
    /// unique per job, so the id IS the key. `StartMigration` mints one and
    /// `GetMigration` reports it, from THIS one record. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads of the set report it in the same
    /// order. Bounded per application by [`MIGRATION_QUOTA`].
    pub migrations: BTreeMap<String, Migration>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_migration_id`].
    pub migration_seq: u64,
}

impl Default for ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// The store a fresh engine hands this service: no account resources at all,
    /// and the published Reserved Instance offering catalog already seeded,
    /// because the catalog is service content rather than something a call
    /// creates.
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
            connections: BTreeMap::new(),
            connection_seq: 0,
            endpoint_suffix_seq: 0,
            reserved_instance_offerings: seed_reserved_instance_offerings(),
            default_application_arn: None,
            reserved_instances: BTreeMap::new(),
            billing_subscription_seq: 0,
            packages: BTreeMap::new(),
            account_insights: BTreeMap::new(),
            package_vending_entitled: false,
            vpc_endpoints: BTreeMap::new(),
            vpc_endpoint_seq: 0,
            applications: BTreeMap::new(),
            application_seq: 0,
            attachment_seq: 0,
            direct_query_data_sources: BTreeMap::new(),
            migrations: BTreeMap::new(),
            migration_seq: 0,
        }
    }
}

/// The characters a domain endpoint's opaque suffix is built from.
pub(super) const SUFFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The length of a domain endpoint's opaque suffix, from `ServiceUrl`'s own example
/// `search-imdb-movies-oopcnjfn6ugo.eu-west-1.es.amazonaws.com`.
pub(super) const SUFFIX_LEN: u32 = 12;

/// The number of distinct suffixes [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_endpoint_suffix`] can
/// render: `36^12`.
pub(super) const SUFFIX_SPACE: u128 = 36u128.pow(SUFFIX_LEN);

/// Scrambles consecutive seeds across [`SUFFIX_SPACE`]. Prime, so it is coprime with
/// `36^12 = 2^24 * 3^24`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod
/// SUFFIX_SPACE)` a BIJECTION: distinct seeds can never mint the same suffix.
pub(super) const SUFFIX_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term for a domain endpoint's suffix, so the first suffix
/// minted is not a run of leading `a`s.
pub(super) const SUFFIX_OFFSET: u128 = 734_291;

/// The bijection's constant term for a connection id. Different from
/// [`SUFFIX_OFFSET`], so the nth connection id and the nth endpoint suffix — minted
/// off two counters that both start at zero — do not carry the same token.
pub(super) const CONNECTION_ID_OFFSET: u128 = 51_539_607_551;

/// The bijection's constant term for a service-managed VPC endpoint's id. Different
/// from the two above for the same reason: three counters that start at zero must not
/// render the same token.
pub(super) const VPC_ENDPOINT_ID_OFFSET: u128 = 8_589_934_591;

/// The bijection's constant term for an OpenSearch UI application's id. Different
/// from the three above for the same reason: counters that all start at zero must not
/// render the same token.
pub(super) const APPLICATION_ID_OFFSET: u128 = 137_438_953_471;

/// The bijection's constant term for a data source attachment's id. Different from the
/// four above for the same reason: counters that all start at zero must not render the
/// same token.
pub(super) const ATTACHMENT_ID_OFFSET: u128 = 2_199_023_255_551;

/// The bijection's constant term for a saved-object migration job's id. Different from
/// the five above for the same reason: counters that all start at zero must not render
/// the same token.
pub(super) const MIGRATION_ID_OFFSET: u128 = 35_184_372_088_831;

/// The opaque lowercase-alphanumeric token seed `seq` scrambles to, through the
/// bijection described on [`SUFFIX_MULTIPLIER`]: distinct seeds under one `offset`
/// can never render the same token.
pub(super) fn opaque_token(seq: u64, offset: u128) -> String {
    let mut scrambled = (u128::from(seq) * SUFFIX_MULTIPLIER + offset) % SUFFIX_SPACE;
    let mut token = Vec::with_capacity(SUFFIX_LEN as usize);
    for _ in 0..SUFFIX_LEN {
        let digit = usize::try_from(scrambled % 36).unwrap_or(0);
        token.push(SUFFIX_ALPHABET[digit]);
        scrambled /= 36;
    }
    String::from_utf8(token).unwrap_or_default()
}

impl ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Mint the opaque suffix a domain's endpoint host carries, so two domains of
    /// the same name in different runs do not share an endpoint.
    ///
    /// Derived from a counter through the bijection described on
    /// [`SUFFIX_MULTIPLIER`], so it is unpredictable-looking yet never repeats.
    pub fn mint_endpoint_suffix(&mut self) -> String {
        self.endpoint_suffix_seq = self.endpoint_suffix_seq.wrapping_add(1);
        opaque_token(self.endpoint_suffix_seq, SUFFIX_OFFSET)
    }

    /// Mint the identifier a cross-cluster connection is addressed by, so two
    /// connections never share one.
    ///
    /// `ConnectionId` is `@length(min: 10, max: 256)` with pattern
    /// `^[a-z][a-z0-9\-]+$` — an opaque lowercase token. It is minted off its own
    /// counter through the same bijection as [`Self::mint_endpoint_suffix`], behind a
    /// `cc-` prefix for the cross-cluster resource it names, which is also what
    /// satisfies the pattern's leading letter and carries the token past the
    /// 10-character minimum.
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seq = self.connection_seq.wrapping_add(1);
        format!(
            "cc-{}",
            opaque_token(self.connection_seq, CONNECTION_ID_OFFSET)
        )
    }

    /// Mint the identifier a service-managed VPC endpoint is addressed by, so two
    /// endpoints never share one.
    ///
    /// `VpcEndpointId` is `@length(min: 5, max: 256)` with pattern
    /// `^aos-[a-zA-Z0-9]*$`: the `aos-` prefix is the service's own, and the body is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], so it is unpredictable-looking yet never
    /// repeats.
    pub fn mint_vpc_endpoint_id(&mut self) -> String {
        self.vpc_endpoint_seq = self.vpc_endpoint_seq.wrapping_add(1);
        format!(
            "aos-{}",
            opaque_token(self.vpc_endpoint_seq, VPC_ENDPOINT_ID_OFFSET)
        )
    }

    /// Mint the identifier an OpenSearch UI application is addressed by, so two
    /// applications never share one.
    ///
    /// `Id` is `^[a-z0-9]{3,30}$` — an opaque lowercase token — so the whole id is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], which renders exactly that alphabet and never
    /// repeats.
    pub fn mint_application_id(&mut self) -> String {
        self.application_seq = self.application_seq.wrapping_add(1);
        opaque_token(self.application_seq, APPLICATION_ID_OFFSET)
    }

    /// Materialise the status move the clock owes every stored application, for a
    /// caller about to report on any of them — and take away the record of every
    /// application whose removal has fallen due.
    pub fn settle_applications(&mut self, now: SystemTime) {
        for application in self.applications.values_mut() {
            application.settle(now);
        }
        self.remove_deleted_applications(now);
    }

    /// Take the record of every application whose removal the clock has made due away,
    /// together with the records held FOR such an application: a saved-object
    /// migration job belongs to the application it migrates into, so a job of an
    /// application the account no longer holds is no longer a job either.
    ///
    /// An application's data source attachments live ON its record, so they go with it
    /// here rather than being hunted down.
    pub(super) fn remove_deleted_applications(&mut self, now: SystemTime) {
        let removed: Vec<String> = self
            .applications
            .values()
            .filter(|application| application.removal_landed(now))
            .map(|application| application.id.clone())
            .collect();
        for id in &removed {
            self.applications.remove(id);
            self.migrations
                .retain(|_, migration| &migration.application_id != id);
        }
    }

    /// The application a create carrying `client_token` already minted, if this
    /// account holds one: "unique, case-sensitive identifier to ensure idempotency of
    /// the request", so a repeat of the same create is answered with the record the
    /// first one made rather than minting a second application.
    ///
    /// Reads ONLY.
    pub fn application_of_client_token(&self, client_token: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.client_token.as_deref() == Some(client_token))
    }

    /// The application `name` names, if this account holds one. An application name is
    /// unique per account and Region, so this is the one lookup a create resolves its
    /// `name` against.
    ///
    /// Reads ONLY.
    pub fn application_named(&self, name: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.name == name)
    }

    /// The application `arn` addresses, if this account holds one.
    ///
    /// An application's ARN is minted with the record and never changes, so it addresses
    /// exactly one application — which is what lets a caller that holds only the ARN (the
    /// account's default-application setting is written by ARN) resolve the application it
    /// names. A value that is no ARN of this account's applications resolves to nothing.
    ///
    /// Reads ONLY.
    pub fn application_of_arn(&self, arn: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.arn == arn)
    }

    /// Store `application` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    ///
    /// The id comes from [`Self::mint_application_id`], a bijection over its own
    /// counter, so the key cannot already be taken; the entry API is used so no path
    /// can overwrite a record this store already holds under one.
    pub fn insert_application(&mut self, application: Application) -> &Application {
        let id = application.id.clone();
        self.applications.entry(id).or_insert(application)
    }

    /// Every application of this account in the order a listing reports them: the order
    /// they were created in, oldest first.
    ///
    /// Two applications created at one instant are ordered by the `Id` they were minted
    /// under, which is unique per application — so the order is TOTAL, and the same set
    /// reports the same order twice however the records are held. That is what lets a
    /// page token name a place in the set.
    ///
    /// Reads ONLY.
    pub fn applications_in_creation_order(&self) -> Vec<&Application> {
        let mut applications: Vec<&Application> = self.applications.values().collect();
        applications.sort_by(|one, other| {
            one.created_at
                .cmp(&other.created_at)
                .then_with(|| one.id.cmp(&other.id))
        });
        applications
    }

    /// Mint the identifier a saved-object migration job is addressed by, so two jobs
    /// never share one.
    ///
    /// `migrationId` is an opaque string the model constrains not at all, and a read is
    /// addressed by it ALONE, so it names the application whose workspace the job
    /// imports into and then an opaque part minted off its own counter through the same
    /// bijection as [`Self::mint_endpoint_suffix`]. The application's `Id` carries no
    /// hyphen (`^[a-z0-9]{3,30}$`), so the hyphen between the two parts is what
    /// [`migration_application_id`] reads the application back out of.
    pub fn mint_migration_id(&mut self, application_id: &str) -> String {
        self.migration_seq = self.migration_seq.wrapping_add(1);
        format!(
            "{application_id}-{}",
            opaque_token(self.migration_seq, MIGRATION_ID_OFFSET)
        )
    }

    /// Materialise every status move the clock owes every stored migration job, for a
    /// caller about to report on any of them.
    pub fn settle_migrations(&mut self, now: SystemTime) {
        for migration in self.migrations.values_mut() {
            migration.settle(now);
        }
    }

    /// The migration job `migration_id` names, with every status move the clock owes it
    /// already materialised — the one lookup a read of a single job goes through.
    pub fn settled_migration_mut(
        &mut self,
        migration_id: &str,
        now: SystemTime,
    ) -> Option<&mut Migration> {
        let migration = self.migrations.get_mut(migration_id)?;
        migration.settle(now);
        Some(migration)
    }

    /// The migration job a start carrying `client_token` already minted, if this account
    /// holds one: "if this token matches a previous request, Amazon OpenSearch Service
    /// ignores the request but does not return an error", so a repeat of the same start
    /// is answered with the record the first one made rather than starting a second job.
    ///
    /// Reads ONLY.
    pub fn migration_of_client_token(&self, client_token: &str) -> Option<&Migration> {
        self.migrations
            .values()
            .find(|migration| migration.client_token.as_deref() == Some(client_token))
    }

    /// How many migration jobs this account holds for one application — what
    /// [`MIGRATION_QUOTA`] bounds.
    ///
    /// Counted from the ONE statement of which jobs belong to an application, so the
    /// bound a start is refused by and the set a listing reports are the same set.
    ///
    /// Reads ONLY.
    pub fn migrations_of_application(&self, application_id: &str) -> usize {
        self.migrations_of_application_in_creation_order(application_id)
            .len()
    }

    /// This application's migration jobs in the order a listing reports them: the order
    /// they were started in, oldest first.
    ///
    /// Two jobs started at one instant are ordered by the `migrationId` they were minted
    /// under, which is unique per job — so the order is TOTAL, and the same set reports
    /// the same order twice however the records are held. That is what lets a page token
    /// name a place in the set.
    ///
    /// Reads ONLY.
    pub fn migrations_of_application_in_creation_order(
        &self,
        application_id: &str,
    ) -> Vec<&Migration> {
        let mut migrations: Vec<&Migration> = self
            .migrations
            .values()
            .filter(|migration| migration.application_id == application_id)
            .collect();
        migrations.sort_by(|one, other| {
            one.created_at
                .cmp(&other.created_at)
                .then_with(|| one.migration_id.cmp(&other.migration_id))
        });
        migrations
    }

    /// Store `migration` and hand back the stored copy, so a start answers with the
    /// record this service persisted rather than a copy beside it.
    ///
    /// The id comes from [`Self::mint_migration_id`], a bijection over its own counter,
    /// so the key cannot already be taken; the entry API is used so no path can
    /// overwrite a record this store already holds under one.
    pub fn insert_migration(&mut self, migration: Migration) -> &Migration {
        let id = migration.migration_id.clone();
        self.migrations.entry(id).or_insert(migration)
    }

    /// The application `id` names, with any status move the clock owes it already
    /// materialised — the one lookup a read of a single application goes through.
    ///
    /// An application whose removal has fallen due names nothing: its record is taken
    /// away here, so a read of it finds no such application rather than one resting in
    /// its deleting status for ever.
    pub fn settled_application_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut Application> {
        let application = self.applications.get_mut(id)?;
        application.settle(now);
        if application.removal_landed(now) {
            self.remove_deleted_applications(now);
            return None;
        }
        self.applications.get_mut(id)
    }

    /// Mint the identifier a data source attachment is addressed by, so two
    /// attachments never share one.
    ///
    /// `attachmentId` is an opaque string the model constrains not at all, so it is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], behind the `dsa-` prefix that names the resource
    /// it identifies the way `cc-` names a cross-cluster connection.
    pub fn mint_attachment_id(&mut self) -> String {
        self.attachment_seq = self.attachment_seq.wrapping_add(1);
        format!(
            "dsa-{}",
            opaque_token(self.attachment_seq, ATTACHMENT_ID_OFFSET)
        )
    }

    /// Materialise the status move the clock owes every data source attachment of every
    /// stored application, for a caller about to report on any of them.
    ///
    /// The data sources the account still holds are resolved FIRST, because that is
    /// what decides whether a pending attachment that has fallen due completes or
    /// fails: an attachment whose data source is gone can never complete. Each DISTINCT
    /// ARN the attachments name is resolved once, so two applications attached to one
    /// data source cost one resolution rather than two.
    pub fn settle_attachments(&mut self, now: SystemTime) {
        let mut data_sources: BTreeSet<String> = self
            .applications
            .values()
            .flat_map(|application| application.attachments.keys())
            .cloned()
            .collect();
        data_sources.retain(|arn| self.holds_data_source(arn));
        for application in self.applications.values_mut() {
            for attachment in application.attachments.values_mut() {
                attachment.settle(now, &data_sources);
            }
        }
    }

    /// Materialise everything the clock owes the records an attachment operation reads:
    /// the applications the attachments belong to, and the attachments' own moves.
    ///
    /// The ONE place the attachment operations settle, so the four of them cannot drift
    /// apart over which records they report at `now`.
    pub fn settle_for_attachments(&mut self, now: SystemTime) {
        self.settle_applications(now);
        self.settle_attachments(now);
    }

    /// The domain `arn` names, if this account holds one — how an operation that takes
    /// a domain's ARN rather than its name resolves it.
    ///
    /// Reads ONLY.
    pub fn domain_of_arn(&self, arn: &str) -> Option<&Domain> {
        self.domains.values().find(|domain| domain.arn == arn)
    }

    /// When the data source `arn` names becomes active, and whether this account holds
    /// one under that ARN at all — see [`DataSourceActivation`].
    ///
    /// An application's data source is any of the kinds this service attaches to one, so
    /// the ARN is resolved against every kind the account can hold rather than against
    /// its domains alone: a domain of this account, a direct query data source of it, or
    /// this account's AWS Glue Data Catalog. Anything else names no data source of this
    /// account, which is what an attachment can never complete against.
    ///
    /// The ONE resolution of a data source ARN, so the attach that decides an
    /// attachment's outcome and the settlement that lands it cannot disagree about what
    /// the account holds.
    ///
    /// Reads ONLY.
    pub fn data_source_activation(&self, arn: &str) -> DataSourceActivation {
        // A domain is provisioned, so the state of the domain the ARN names is what
        // decides whether its data is reachable yet.
        if let Some(domain) = self.domain_of_arn(arn) {
            return match domain.cluster_up_at() {
                Some(due) => DataSourceActivation::ActiveAt(due),
                None => DataSourceActivation::Active,
            };
        }
        // A direct query data source is queryable from the moment the add that minted it
        // answered: nothing is provisioned for it and no status moves on it.
        if self
            .direct_query_data_sources
            .values()
            .any(|held| held.arn == arn)
        {
            return DataSourceActivation::Active;
        }
        // The account's Data Catalog is not a resource anything provisions, so it is
        // active whenever it is named.
        if is_account_glue_data_catalog_arn(arn) {
            return DataSourceActivation::Active;
        }
        DataSourceActivation::Absent
    }

    /// Whether this account holds a data source under `arn` — what decides whether a
    /// pending attachment that has fallen due completes or fails.
    pub fn holds_data_source(&self, arn: &str) -> bool {
        self.data_source_activation(arn) != DataSourceActivation::Absent
    }

    /// Materialise the status move the clock owes every stored VPC endpoint, for a
    /// caller about to report on any of them.
    pub fn settle_vpc_endpoints(&mut self, now: SystemTime) {
        for endpoint in self.vpc_endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// Give `domain` the opaque host id its VPC endpoints are named by, if it is a
    /// VPC-access domain that has none yet.
    ///
    /// The ONE place that decides a domain needs one, so a domain reaches it however
    /// it came to hold `VPCOptions` — created with them, or given them by a later
    /// update — and a domain that already holds one keeps it, which is what makes the
    /// hosts [`Domain::vpc_access_endpoints`] publishes stable across calls. A domain
    /// with no `VPCOptions` is reached on its public endpoint and gets none.
    pub fn provision_vpc_access_host(&mut self, domain: &mut Domain) {
        if domain.vpc_options.is_some() && domain.vpc_access_suffix.is_none() {
            domain.vpc_access_suffix = Some(self.mint_endpoint_suffix());
        }
    }

    /// The domain a `CreateVpcEndpoint` can mint an endpoint for, given the
    /// `DomainArn` the request named.
    ///
    /// TWO conditions, and the refusal does not say which failed
    /// ([`vpc_endpoint_domain_unavailable`]): the ARN must name a domain this account
    /// holds, and that domain must be one an endpoint can be created for — a domain
    /// provisioned inside a VPC, because the endpoint exists to reach a cluster that
    /// has no public endpoint of its own. A domain with no `VPCOptions` has nothing
    /// for an endpoint to proxy into.
    ///
    /// Reads ONLY.
    pub fn vpc_endpoint_domain(&self, domain_arn: &str) -> Option<&Domain> {
        self.domains
            .values()
            .find(|domain| domain.arn == domain_arn && domain.vpc_options.is_some())
    }

    /// The endpoint a create carrying `client_token` already minted, if this account
    /// holds one.
    ///
    /// `ClientToken` is "a unique, case-sensitive identifier to ensure idempotency of
    /// the request", so a repeat of the same create is answered with the record the
    /// first one made rather than minting a second endpoint.
    ///
    /// Reads ONLY.
    pub fn vpc_endpoint_of_client_token(&self, client_token: &str) -> Option<&VpcEndpointRecord> {
        self.vpc_endpoints
            .values()
            .find(|endpoint| endpoint.client_token.as_deref() == Some(client_token))
    }

    /// Store `endpoint` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    pub fn insert_vpc_endpoint(&mut self, endpoint: VpcEndpointRecord) -> &VpcEndpointRecord {
        let id = endpoint.vpc_endpoint_id.clone();
        self.vpc_endpoints.entry(id).or_insert(endpoint)
    }

    /// The domain `name` names, with any configuration change the clock owes it
    /// already materialised.
    pub fn settled_mut(&mut self, name: &str, now: SystemTime) -> Option<&mut Domain> {
        let domain = self.domains.get_mut(name)?;
        domain.settle(now);
        Some(domain)
    }

    /// Materialise every configuration change the clock owes, for a caller about to
    /// report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for domain in self.domains.values_mut() {
            domain.settle(now);
        }
    }

    /// Materialise the status move the clock owes every stored cross-cluster
    /// connection, for a caller about to report on all of them.
    pub fn settle_connections(&mut self, now: SystemTime) {
        for connection in self.connections.values_mut() {
            connection.settle(now);
        }
    }

    /// Keep the records of deleted connections within [`CONNECTION_HISTORY`],
    /// releasing the oldest first.
    ///
    /// A deleted connection's record stays addressable rather than being taken out on
    /// a timer, so what bounds the store is a COUNT: past the cap the oldest deleted
    /// record is released, which is the only thing that releases one. Called where the
    /// store grows — a create — so the cap is applied before another record is added
    /// and the set of records cannot grow with the number of creates and deletes a
    /// caller makes.
    pub fn trim_connection_history(&mut self) {
        let mut deleted: Vec<(u64, String)> = self
            .connections
            .values()
            .filter(|connection| connection.deleted())
            .map(|connection| (connection.sequence, connection.connection_id.clone()))
            .collect();
        if deleted.len() <= CONNECTION_HISTORY {
            return;
        }
        deleted.sort_unstable();
        let over = deleted.len() - CONNECTION_HISTORY;
        for (_, connection_id) in deleted.into_iter().take(over) {
            self.connections.remove(&connection_id);
        }
    }

    /// The cross-cluster connection `connection_id` names, with every status move the
    /// clock owes it already materialised — so an operation on one connection decides
    /// from the status the record is in at `now`, exactly as a report of the whole set
    /// would have shown it.
    ///
    /// `None` only where the id addresses no record at all: a connection that was
    /// deleted comes to rest in [`CONNECTION_STATUS_DELETED`] and its id goes on
    /// resolving to that record, so an id this service minted for this account stops
    /// resolving only once the store has released the record
    /// ([`Self::trim_connection_history`]).
    pub fn settled_connection_mut(
        &mut self,
        connection_id: &str,
        now: SystemTime,
    ) -> Option<&mut Connection> {
        let connection = self.connections.get_mut(connection_id)?;
        connection.settle(now);
        Some(connection)
    }

    /// Materialise the status move the clock owes every stored package, for a caller
    /// about to report on all of them — and release the record of every package whose
    /// deletion has fallen due.
    pub fn settle_packages(&mut self, now: SystemTime) {
        for package in self.packages.values_mut() {
            package.settle(now);
        }
        // A package the clock owed a removal is no longer one the account holds, so no
        // read reports it again and the quota slot it counted is free. Done here rather
        // than in [`Package::settle`] because a record cannot take itself out of the
        // map that holds it.
        self.packages.retain(|_, package| !package.removal_landed(now));
    }

    /// The insights the entity a request names holds.
    ///
    /// "An entity can be an Amazon OpenSearch Service domain or an Amazon Web
    /// Services account": a `DomainName` entity holds the insights of that one
    /// domain, and an `Account` entity holds the account's insights — its own and
    /// those of every domain it holds, because an insight about a domain is an
    /// insight of the account the domain belongs to.
    ///
    /// Reads ONLY, and refuses rather than reporting an empty set where the entity
    /// names nothing this account holds, so every operation that resolves an
    /// `InsightEntity` resolves it the same way.
    pub fn insights_of_entity(
        &self,
        entity: &InsightEntity,
    ) -> Result<Vec<&InsightRecord>, InsightEntityRefusal> {
        match entity.r#type.as_str() {
            INSIGHT_ENTITY_DOMAIN_NAME => {
                // "For `DomainName`, this is the domain name": an entity of this type
                // names its domain in `Value`, so one that names none resolves to no
                // entity at all rather than to every domain.
                let name = insight_entity_value(entity)
                    .ok_or_else(|| InsightEntityRefusal::Validation(insight_value_required()))?;
                let domain = self
                    .domains
                    .get(name)
                    .ok_or_else(|| InsightEntityRefusal::NotFound(domain_not_found(name)))?;
                Ok(domain.insights.values().collect())
            }
            INSIGHT_ENTITY_ACCOUNT => {
                // "For `Account`, this is the Amazon Web Services account ID": the
                // only account whose insights this emulator holds is the one its
                // requests are made for, so any other account id names an entity it
                // cannot report on.
                if let Some(value) = insight_entity_value(entity) {
                    if value != ::engine::emulator_account_id() {
                        return Err(InsightEntityRefusal::Validation(foreign_account_entity(
                            value,
                        )));
                    }
                }
                Ok(self
                    .account_insights
                    .values()
                    .chain(
                        self.domains
                            .values()
                            .flat_map(|domain| domain.insights.values()),
                    )
                    .collect())
            }
            other => Err(InsightEntityRefusal::Validation(unknown_insight_entity(
                other,
            ))),
        }
    }

    /// The insights a LISTING reports for the entity a request names.
    ///
    /// Resolves the entity exactly as [`Self::insights_of_entity`] does — the same ONE
    /// decision — except that a `DomainName` this account holds no domain under selects
    /// no insight rather than refusing: a listing reports the set an entity holds, and
    /// an entity holding none reports the EMPTY set however it came to be empty. The
    /// entity itself is still a requirement, so one whose type is outside
    /// `InsightEntityType`, one that names no value where its type needs one, and one
    /// naming another account are refused as the malformed requests they are.
    ///
    /// Reads ONLY.
    pub fn insights_listed_for_entity(
        &self,
        entity: &InsightEntity,
    ) -> Result<Vec<&InsightRecord>, ValidationException> {
        match self.insights_of_entity(entity) {
            Ok(held) => Ok(held),
            Err(InsightEntityRefusal::NotFound(_)) => Ok(Vec::new()),
            Err(InsightEntityRefusal::Validation(payload)) => Err(payload),
        }
    }

    /// The one insight the entity a request names holds under `insight_id`, or
    /// `None` where that entity holds none under it.
    ///
    /// An insight is addressed by its entity and its id together, so this resolves
    /// the entity exactly as [`Self::insights_of_entity`] does — the same ONE
    /// decision, and the only one that can refuse — and then looks the id up among
    /// the insights THAT entity holds, so an insight another entity holds is never
    /// reported here. An id the entity holds no insight under is not a refusal: it
    /// names no insight of this entity, which the caller reports as it reports any
    /// other id it resolves nothing for.
    ///
    /// Reads ONLY.
    pub fn insight_of_entity(
        &self,
        entity: &InsightEntity,
        insight_id: &str,
    ) -> Result<Option<&InsightRecord>, InsightEntityRefusal> {
        Ok(self
            .insights_of_entity(entity)?
            .into_iter()
            .find(|record| record.insight_id == insight_id))
    }

    /// The one insight a feedback submission addresses, ready to be written.
    ///
    /// Feedback "submits feedback for an EXISTING insight in an Amazon OpenSearch
    /// Service domain", and `InsightFeedbackEntityType` admits `DomainName` alone, so
    /// a submission is addressed by the domain that holds the insight and the
    /// insight's own id together: BOTH have to resolve, because a domain this account
    /// does not hold and an id that domain holds no insight under alike leave no
    /// insight for the rating to be about. Which of the two failed to resolve is not
    /// reported — the submission named a resource this service cannot resolve either
    /// way ([`insight_resource_not_found`]) — while an entity whose type is outside
    /// the enum names no kind of resource at all and is refused as a malformed
    /// request.
    pub fn insight_for_feedback_mut(
        &mut self,
        entity: &InsightFeedbackEntity,
        insight_id: &str,
    ) -> Result<&mut InsightRecord, InsightEntityRefusal> {
        if entity.r#type != INSIGHT_ENTITY_DOMAIN_NAME {
            return Err(InsightEntityRefusal::Validation(
                unknown_insight_feedback_entity(&entity.r#type),
            ));
        }
        // "The value of the entity, such as a domain name": an entity of this type
        // names its domain in `Value`, and `Value` is `@required` here, so one that
        // names no name at all resolves to no domain rather than to every domain.
        let name = entity.value.trim();
        self.domains
            .get_mut(name)
            .and_then(|domain| domain.insights.get_mut(insight_id))
            .ok_or_else(|| InsightEntityRefusal::NotFound(insight_resource_not_found()))
    }

    /// The tag set of the resource `arn` names, for a caller about to READ it.
    ///
    /// A domain, an application and a direct query data source are the taggable
    /// resources this store persists an ARN for ([`domain_arn`], [`application_arn`],
    /// [`direct_query_data_source_arn`]); the data source of a DOMAIN that the tagging
    /// operations also name has no stored ARN to resolve against (it is addressed by
    /// its domain and name), so an ARN naming none of them resolves to nothing and the
    /// caller refuses it.
    pub fn tagged_resource(&self, arn: &str) -> Option<&TagSet> {
        self.domains
            .values()
            .find(|domain| domain.arn == arn)
            .map(|domain| &domain.tags)
            .or_else(|| {
                self.applications
                    .values()
                    .find(|application| application.arn == arn)
                    .map(|application| &application.tags)
            })
            .or_else(|| {
                self.direct_query_data_sources
                    .values()
                    .find(|data_source| data_source.arn == arn)
                    .map(|data_source| &data_source.tags)
            })
    }

    /// The tag set of the resource `arn` names, for a caller about to WRITE it —
    /// resolved exactly as [`Self::tagged_resource`] resolves it, so a tag write and
    /// a tag read can never disagree about which resource an ARN names.
    pub fn tagged_resource_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(domain) = self.domains.values_mut().find(|domain| domain.arn == arn) {
            return Some(&mut domain.tags);
        }
        if let Some(application) = self
            .applications
            .values_mut()
            .find(|application| application.arn == arn)
        {
            return Some(&mut application.tags);
        }
        self.direct_query_data_sources
            .values_mut()
            .find(|data_source| data_source.arn == arn)
            .map(|data_source| &mut data_source.tags)
    }

    /// The package `package_name` names, if the account holds one.
    ///
    /// The store is keyed by `PackageID`, but a package NAME is unique across the
    /// account's packages, so this is the one lookup every operation that resolves a
    /// name goes through.
    pub fn package_named(&self, package_name: &str) -> Option<&Package> {
        self.packages
            .values()
            .find(|package| package.package_name == package_name)
    }

    /// Store `package` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    pub fn insert_package(&mut self, package: Package) -> &Package {
        let id = package.package_id.clone();
        self.packages.entry(id).or_insert(package)
    }

    /// Whether any domain of this account holds the package `package_id` — the
    /// condition that keeps the package from being deleted.
    ///
    /// A link is held ON the domain under the package's id, so this is the same set
    /// `ListDomainsForPackage` reports: an association the clock has not taken off the
    /// domain yet still counts, because the package is still in place there.
    ///
    /// Reads ONLY, and is read AFTER [`Domain::settle_package_associations`], so a link
    /// whose removal has already fallen due is not counted.
    pub fn package_is_associated(&self, package_id: &str) -> bool {
        self.domains
            .values()
            .any(|domain| domain.package_associations.contains_key(package_id))
    }

    /// Decide whether `request` can be served on `domain_name` and, where it can,
    /// hand back the stored [`Package`] the association will link to.
    ///
    /// Reads ONLY: this is the whole decision for one link, so a caller making
    /// several of them can decide on all of them before writing any — which is what
    /// lets a refused request leave the domain's associations exactly as they were.
    ///
    /// `associated_by_request` names the packages the SAME request associates, which
    /// is what satisfies a prerequisite "linked to the domain before or
    /// simultaneously with the package association"; a request that links a single
    /// package passes an empty set, because such a call associates nothing else.
    ///
    /// Call AFTER [`Self::settle_packages`] and the domain's own settling, so a
    /// package whose copy has finished and an association whose move has landed are
    /// judged in the status the clock already left them in.
    pub fn plan_package_association(
        &self,
        domain_name: &str,
        request: &AssociationRequest,
        associated_by_request: &BTreeSet<String>,
    ) -> Result<Package, AssociationRefusal> {
        // The condition is reported, not worded: which sentence an unresolvable
        // `DomainName` is refused with belongs to the OPERATION that answers, so this
        // decision hands the name back and the handler words it.
        let domain = self
            .domains
            .get(domain_name)
            .ok_or_else(|| AssociationRefusal::DomainUnresolved(domain_name.to_string()))?;
        let package = self
            .packages
            .get(&request.package_id)
            .ok_or_else(|| AssociationRefusal::Validation(package_not_found()))?;

        // A package can be put on a domain only once its content is in place: until
        // then there is no version of it to associate, which is the conflict between
        // the call and the status the package is in.
        if package.status != PACKAGE_STATUS_AVAILABLE {
            return Err(AssociationRefusal::Conflict(package_not_available(
                &request.package_id,
                &package.status,
            )));
        }

        // The service runs one change to a domain's package associations at a time: a
        // call made while another associate or dissociate on the SAME domain has not
        // landed yet is refused rather than overtaking it. Once every change has
        // landed, a package already on the domain is re-associated, which is what
        // moves the domain onto a newer version of its content.
        if domain
            .package_associations
            .values()
            .any(PackageAssociation::in_flight)
        {
            return Err(AssociationRefusal::Conflict(concurrent_package_operation()));
        }

        for prerequisite in &request.prerequisite_package_ids {
            if prerequisite == &request.package_id {
                return Err(AssociationRefusal::Validation(prerequisite_is_self(
                    &request.package_id,
                )));
            }
            if !self.packages.contains_key(prerequisite) {
                return Err(AssociationRefusal::Validation(package_not_found()));
            }
            // A prerequisite "must be linked to the domain before or simultaneously
            // with the package association": one the domain already holds satisfies
            // the first, one the same call associates the second, and anything else
            // is a request the domain cannot serve as asked.
            let satisfied = domain.package_associations.contains_key(prerequisite)
                || associated_by_request.contains(prerequisite);
            if !satisfied {
                return Err(AssociationRefusal::Validation(prerequisite_not_associated(
                    &request.package_id,
                    prerequisite,
                    domain_name,
                )));
            }
        }

        Ok(package.clone())
    }

    /// Make the association `request` asks for on `domain_name` and hand back the
    /// stored record as `DomainPackageDetails`.
    ///
    /// `package` is the record [`Self::plan_package_association`] resolved, so this
    /// writes only a link the decision above admitted. `None` only where the domain
    /// is gone, which the plan already refused.
    ///
    /// The association REPLACES any the domain already had with the same package: a
    /// domain holds one per package, which is also what makes a second call the way a
    /// newer version of the package's content reaches the domain.
    pub fn put_package_association(
        &mut self,
        domain_name: &str,
        package: &Package,
        request: &AssociationRequest,
        now: SystemTime,
    ) -> Option<DomainPackageDetails> {
        let domain = self.domains.get_mut(domain_name)?;
        let association = PackageAssociation {
            package_id: package.package_id.clone(),
            // The package is not on the domain the instant the call returns: it is
            // being put there, and the clock owes the association the status that
            // leaves it in once the span has passed.
            status: DOMAIN_PACKAGE_STATUS_ASSOCIATING.to_string(),
            owed: Some(OwedStatus {
                status: DOMAIN_PACKAGE_STATUS_ACTIVE.to_string(),
                message: None,
                due: now + PACKAGE_ASSOCIATION_CHANGE,
            }),
            // The version this association puts on the domain is the one the package
            // has available now, so a package whose content was updated reaches the
            // domain through THIS call rather than on its own.
            package_version: package.version.to_string(),
            // The call's own list, which REPLACES whatever an earlier association of
            // the same package recorded: the prerequisites belong to the association
            // this call makes.
            prerequisite_package_ids: request.prerequisite_package_ids.clone(),
            association_configuration: request.association_configuration.clone(),
            last_updated: now,
            // This call puts the package ON the domain, so nothing about it is owed a
            // removal: a dissociation is what asks for one.
            removal_due: None,
            // The package is being put in place, which is not an error state; whatever
            // an earlier association of the same package failed at is not this one's.
            error_details: None,
        };
        // Answered from the STORED record, through the projection every read path
        // reports an association with: the slot the write went into IS the record, so
        // there is no second lookup and no second way this can fail.
        let stored = match domain.package_associations.entry(package.package_id.clone()) {
            Entry::Occupied(mut slot) => {
                slot.insert(association);
                slot.into_mut()
            }
            Entry::Vacant(slot) => slot.insert(association),
        };
        Some(stored.to_details(domain_name, package))
    }

    /// Decide whether the association between `domain_name` and `package_id` can be
    /// removed and, where it can, hand back the stored [`Package`] it links to.
    ///
    /// Reads ONLY — the mirror of [`Self::plan_package_association`], so a call that
    /// removes several links can decide on all of them before writing any, and a
    /// refused request leaves the domain's associations exactly as they were.
    ///
    /// Call AFTER [`Self::settle_packages`] and the domain's own settling, so an
    /// association whose move has landed is judged in the status the clock already
    /// left it in, and one whose removal has already fallen due is no longer there to
    /// be removed again.
    pub fn plan_package_dissociation(
        &self,
        domain_name: &str,
        package_id: &str,
    ) -> Result<Package, AssociationRefusal> {
        // As on the association side, the condition is reported and the handler words
        // it: these two operations do not refuse an unresolvable `DomainName` with the
        // same sentence.
        let domain = self
            .domains
            .get(domain_name)
            .ok_or_else(|| AssociationRefusal::DomainUnresolved(domain_name.to_string()))?;
        let package = self
            .packages
            .get(package_id)
            .ok_or_else(|| AssociationRefusal::Validation(package_not_found()))?;

        // The service runs one change to a domain's package associations at a time, so
        // a call made while another associate or dissociate on the SAME domain has not
        // landed yet is refused rather than overtaking it — the identical rule the
        // associate decision applies, because it is one rule about the domain.
        //
        // It is judged BEFORE the link itself, in the same place the associate decision
        // judges it: the condition is about the DOMAIN, so which of its associations
        // the call happens to name cannot change the answer — a domain with a change
        // still landing refuses every package call alike, and only once nothing is in
        // flight does the set it holds decide the call.
        if domain
            .package_associations
            .values()
            .any(PackageAssociation::in_flight)
        {
            return Err(AssociationRefusal::Conflict(concurrent_package_operation()));
        }

        // A removal needs a link to remove: a package the domain is not holding is
        // nothing this call can take off it.
        if !domain.package_associations.contains_key(package_id) {
            return Err(AssociationRefusal::Validation(package_not_associated()));
        }

        Ok(package.clone())
    }

    /// Take the package off `domain_name` and hand back the stored record as
    /// `DomainPackageDetails`.
    ///
    /// `package` is the record [`Self::plan_package_dissociation`] resolved, so this
    /// writes only a removal the decision above admitted. `None` only where the
    /// domain or the association is gone, which that plan already refused.
    ///
    /// The package is still on the domain the instant the call returns: it is being
    /// taken off, so the association rests in [`DOMAIN_PACKAGE_STATUS_DISSOCIATING`]
    /// and the clock owes it the OUTCOME of that removal
    /// ([`PACKAGE_ASSOCIATION_CHANGE`]) — which is decided when it falls due, from the
    /// indexes the domain holds then ([`Domain::settle_package_associations`]), not
    /// here: an index created while the removal is in flight still depends on the
    /// package by the time the removal would happen.
    pub fn remove_package_association(
        &mut self,
        domain_name: &str,
        package: &Package,
        now: SystemTime,
    ) -> Option<DomainPackageDetails> {
        let domain = self.domains.get_mut(domain_name)?;
        let association = domain.package_associations.get_mut(&package.package_id)?;
        association.status = DOMAIN_PACKAGE_STATUS_DISSOCIATING.to_string();
        // Whatever move was outstanding is replaced by the removal this call asked for:
        // the association is on its way off the domain, not on its way to any other
        // status, and it is not in an error state while that is in flight.
        association.owed = None;
        association.removal_due = Some(now + PACKAGE_ASSOCIATION_CHANGE);
        association.error_details = None;
        association.last_updated = now;
        // Answered from the STORED record, through the projection every read path
        // reports an association with.
        Some(association.to_details(domain_name, package))
    }

    /// The published offering catalog, in the ORDER the service publishes it
    /// ([`offering_catalog_rank`]).
    ///
    /// The store is keyed by offering id so a purchase and the id filter can resolve one
    /// entry, and that key orders the catalog by a digest rather than by anything a
    /// caller reads — so the listing is published along the catalog's own order instead.
    /// The order is a function of the records alone, so two reads report the same
    /// offerings in the same order and a page resumes where the previous one stopped.
    pub fn published_reserved_instance_offerings(&self) -> Vec<&ReservedInstanceOffering> {
        let mut catalog: Vec<&ReservedInstanceOffering> =
            self.reserved_instance_offerings.values().collect();
        catalog.sort_by_key(|offering| offering_catalog_rank(offering));
        catalog
    }

    /// The published offering whose `ReservedInstanceOfferingId` is `offering_id`.
    ///
    /// The catalog is keyed by that id, so this is the one lookup both the
    /// offerings listing and a purchase resolve an offering by.
    pub fn reserved_instance_offering(
        &self,
        offering_id: &str,
    ) -> Option<&ReservedInstanceOffering> {
        self.reserved_instance_offerings.get(offering_id)
    }

    /// Mint the `BillingSubscriptionId` a purchase is billed under: "the unique
    /// identifier of the billing subscription".
    ///
    /// A counter, so no two purchases share one and the id is stable for the
    /// reservation it was minted for.
    pub fn mint_billing_subscription_id(&mut self) -> i64 {
        self.billing_subscription_seq = self.billing_subscription_seq.saturating_add(1);
        self.billing_subscription_seq
    }

    /// The number of OpenSearch instances this account currently has reserved, which
    /// is what [`RESERVED_INSTANCE_QUOTA`] bounds.
    pub fn reserved_instance_count(&self) -> i64 {
        self.reserved_instances
            .values()
            .map(|reservation| i64::from(reservation.instance_count))
            .sum()
    }

    /// Store `record` and hand back the stored copy, so the purchase answers with
    /// the record this service persisted rather than a copy beside it.
    pub fn insert_reserved_instance(
        &mut self,
        record: ReservedInstanceRecord,
    ) -> &ReservedInstanceRecord {
        let id = record.reserved_instance_id.clone();
        self.reserved_instances.insert(id.clone(), record);
        &self.reserved_instances[&id]
    }

    /// Materialise every state change the clock owes the stored reservations, for a
    /// caller about to report on them.
    pub fn settle_reserved_instances(&mut self, now: SystemTime) {
        for reservation in self.reserved_instances.values_mut() {
            reservation.settle(now);
        }
    }
}

/// `EngineType` for an `EngineVersion`, derived from its prefix: `OpenSearch_X.Y`
/// runs the OpenSearch engine, `Elasticsearch_X.Y` the legacy Elasticsearch one.
///
/// `None` only for a version matching neither prefix, which `VersionString`'s
/// pattern `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`
/// does not allow.
pub fn engine_type_of(engine_version: &str) -> Option<&'static str> {
    if engine_version.starts_with("OpenSearch_") {
        Some(ENGINE_TYPE_OPENSEARCH)
    } else if engine_version.starts_with("Elasticsearch_") {
        Some(ENGINE_TYPE_ELASTICSEARCH)
    } else {
        None
    }
}
