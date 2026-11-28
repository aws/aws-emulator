// SPDX-License-Identifier: CC0-1.0

//! The store itself, split out of [`crate::state`] unchanged: the struct every
//! operation reaches this service's records through, the ids and tokens it mints, and
//! the settling each record kind is materialised by.

use super::*;

use super::foreign_data_sources::ForeignDataSources;

/// Per-service state for com-amazonaws-opensearch-amazonopensearchservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Every created domain, keyed by its name — unique per account and Region, so
    /// the name IS the key. Iterating a `BTreeMap` is order-stable, which is what
    /// lets `ListDomainNames` report the same order twice.
    pub domains: BTreeMap<String, Domain>,
    /// The data-plane LISTENER opened for each domain, keyed by that domain's name — the
    /// address `DomainStatus$Endpoint` publishes ([`domain_plane`]). Held beside the domain
    /// records rather than on the record itself, because a listener is a live socket and a
    /// [`Domain`] is cloned by every read; dropping the handle closes the port, so a domain
    /// the store no longer holds stops answering. Bounded by [`DOMAIN_QUOTA`], one per
    /// domain.
    pub domain_planes: BTreeMap<String, domain_plane::DomainPlane>,
    /// Every cross-cluster connection, keyed by its `ConnectionId` — unique per
    /// connection, so the id IS the key. Iterating a `BTreeMap` is order-stable,
    /// which is what lets `DescribeOutboundConnections` report the same order twice
    /// and page through the set without repeating or skipping a record.
    pub connections: BTreeMap<String, Connection>,
    /// The place in this store's connection history the LAST connection took, which
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::next_connection_sequence`]
    /// advances: a connection's own identifier is drawn rather than counted, so this is
    /// what keeps the records in the order they were minted and lets the history release
    /// the oldest one first.
    pub connection_seq: u64,
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
    /// Every OpenSearch UI application of this account, keyed by its `Id` — unique
    /// per application, so the id IS the key. `CreateApplication` mints one and
    /// `GetApplication` reports it, from THIS one record. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads of the set report it in the same
    /// order.
    pub applications: BTreeMap<String, Application>,
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
    /// The claim on the ONE cluster-metric sampler of this store: the task that observes
    /// every provisioned domain's `AWS/ES` gauges on a cadence
    /// ([`cluster_metrics::drive_metric_sampling`]). It holds no reading of its own — the
    /// readings are the records above — only whether a sampler runs and which claim it
    /// holds.
    pub metric_sampling: cluster_metrics::MetricSampling,
}

impl Default for ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// The store a fresh engine hands this service: no account resources at all,
    /// and the published Reserved Instance offering catalog already seeded,
    /// because the catalog is service content rather than something a call
    /// creates.
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
            domain_planes: BTreeMap::new(),
            connections: BTreeMap::new(),
            connection_seq: 0,
            reserved_instance_offerings: seed_reserved_instance_offerings(),
            default_application_arn: None,
            reserved_instances: BTreeMap::new(),
            billing_subscription_seq: 0,
            packages: BTreeMap::new(),
            account_insights: BTreeMap::new(),
            package_vending_entitled: false,
            vpc_endpoints: BTreeMap::new(),
            applications: BTreeMap::new(),
            direct_query_data_sources: BTreeMap::new(),
            migrations: BTreeMap::new(),
            metric_sampling: cluster_metrics::MetricSampling::default(),
        }
    }
}

/// The characters an opaque token this service mints is built from.
pub(super) const TOKEN_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The length of an opaque token this service mints, from `ServiceUrl`'s own example
/// `search-imdb-movies-oopcnjfn6ugo.eu-west-1.es.amazonaws.com`.
pub(super) const TOKEN_LEN: u32 = 12;

/// The number of characters an OpenSearch UI application's `Id` carries.
///
/// `Id` is `^[a-z0-9]{3,30}$`, a bound far wider than the ONE width the service mints
/// inside it, so the modelled pattern does not settle this. What does is the identity's
/// own shape: the id is the host label AWS's own endpoint for an application carries
/// (`<id>.<region>.opensearch.amazonaws.com`), which is the form this account's
/// OpenSearch collections and their endpoints take too — 20 lowercase alphanumeric
/// characters. A client, a provider or a test that checks the form of an id against AWS's
/// own takes this width and no other.
pub(super) const APPLICATION_ID_LEN: u32 = 20;

/// The widest token [`drawn_token`] renders: `36^24` is the last power of 36 a `u128`
/// holds, so one draw renders a whole token up to that width.
const MAX_TOKEN_LEN: u32 = 24;

/// How many times [`drawn_id`] draws again when the id it rendered is one this store
/// already holds a record under. A draw is over `36^len`, so a collision is already
/// vanishing; the redraw is what makes the id this store hands out unique rather than
/// only improbable.
const DRAWS: u8 = 8;

/// ONE freshly drawn opaque lowercase-alphanumeric token of `len` characters.
///
/// The entropy is a version-4 UUID's — 122 drawn bits — folded into the `36^len` tokens
/// of that width, which is what makes the token the resource's OWN identity: it is not a
/// function of how many resources this store has minted, so the first resource of a fresh
/// store is not addressed by a constant and no two resources, in one store or across runs,
/// are handed the same token.
pub(super) fn drawn_token(len: u32) -> String {
    debug_assert!(
        (1..=MAX_TOKEN_LEN).contains(&len),
        "a token is drawn at a width one draw renders"
    );
    let len = len.clamp(1, MAX_TOKEN_LEN);
    let mut drawn = ::uuid::Uuid::new_v4().as_u128() % 36u128.pow(len);
    let mut token = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let digit = usize::try_from(drawn % 36).unwrap_or(0);
        token.push(TOKEN_ALPHABET[digit]);
        drawn /= 36;
    }
    String::from_utf8(token).unwrap_or_default()
}

/// The identifier a fresh draw of `len` characters renders, drawn again while this store
/// already holds a record under it.
///
/// `render` builds the id a drawn token is used as — the token alone, or behind the
/// prefix the resource's own pattern asks for — and `held` answers whether this store
/// already holds a record under that id. EVERY draw is asked, the last one included, and
/// the first free one is the answer.
///
/// A store is bounded by its own quotas, so it holds a vanishing fraction of the `36^len`
/// tokens of a width and the first draw is almost always free. All [`DRAWS`] draws landing
/// on a held record is therefore unreachable in practice; were it reached, the last draw is
/// what this hands back — a minted identity is not a failure a caller is answered with —
/// and the insert paths keep the record this store already holds rather than overwriting it,
/// which is why that draw is logged where an operator can see it.
pub(super) fn drawn_id(
    len: u32,
    render: impl Fn(&str) -> String,
    held: impl Fn(&str) -> bool,
) -> String {
    let mut drawn = render(&drawn_token(len));
    for _ in 1..DRAWS {
        if !held(&drawn) {
            return drawn;
        }
        drawn = render(&drawn_token(len));
    }
    if held(&drawn) {
        ::tracing::warn!(
            draws = DRAWS,
            "every drawn identifier landed on a record this store already holds"
        );
    }
    drawn
}

impl ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Mint the opaque suffix a domain's endpoint host carries, so two domains of
    /// the same name in different runs do not share an endpoint.
    ///
    /// DRAWN ([`drawn_token`]), so the suffix is this domain's own rather than a
    /// rendering of how many domains the store has created — and no two hosts of this
    /// store, in one store or across runs, carry one suffix
    /// ([`Self::endpoint_suffix_held`]).
    pub fn mint_endpoint_suffix(&self) -> String {
        drawn_id(TOKEN_LEN, str::to_string, |suffix| {
            self.endpoint_suffix_held(suffix)
        })
    }

    /// Whether a host of this store already carries `suffix` — the public endpoint a
    /// domain was created with, the host its VPC-access endpoints carry, or the
    /// connection endpoint of a service-managed VPC endpoint, which is minted from the
    /// same draw.
    ///
    /// Reads ONLY.
    fn endpoint_suffix_held(&self, suffix: &str) -> bool {
        self.domains.values().any(|domain| {
            domain.endpoint.contains(suffix) || domain.vpc_access_suffix.as_deref() == Some(suffix)
        }) || self
            .vpc_endpoints
            .values()
            .any(|endpoint| endpoint.endpoint.contains(suffix))
    }

    /// Mint the identifier a cross-cluster connection is addressed by, so two
    /// connections never share one.
    ///
    /// `ConnectionId` is `@length(min: 10, max: 256)` with pattern
    /// `^[a-z][a-z0-9\-]+$` — an opaque lowercase token. The token is DRAWN
    /// ([`drawn_token`]) behind a `cc-` prefix for the cross-cluster resource it names,
    /// which is also what satisfies the pattern's leading letter and carries the id past
    /// the 10-character minimum.
    pub fn mint_connection_id(&self) -> String {
        drawn_id(
            TOKEN_LEN,
            |token| format!("cc-{token}"),
            |id| self.connections.contains_key(id),
        )
    }

    /// The place in this store's connection history the next connection takes: the
    /// order the record was minted in, which is what lets the history release the
    /// oldest record first.
    pub fn next_connection_sequence(&mut self) -> u64 {
        self.connection_seq = self.connection_seq.wrapping_add(1);
        self.connection_seq
    }

    /// Mint the identifier a service-managed VPC endpoint is addressed by, so two
    /// endpoints never share one.
    ///
    /// `VpcEndpointId` is `@length(min: 5, max: 256)` with pattern
    /// `^aos-[a-zA-Z0-9]*$`: the `aos-` prefix is the service's own, and the body is
    /// DRAWN ([`drawn_token`]).
    pub fn mint_vpc_endpoint_id(&self) -> String {
        drawn_id(
            TOKEN_LEN,
            |token| format!("aos-{token}"),
            |id| self.vpc_endpoints.contains_key(id),
        )
    }

    /// Mint the identifier an OpenSearch UI application is addressed by, so two
    /// applications never share one.
    ///
    /// `Id` is `^[a-z0-9]{3,30}$` — an opaque lowercase token — so the whole id is the
    /// DRAWN token ([`drawn_token`]), at the width AWS mints inside that bound
    /// ([`APPLICATION_ID_LEN`]). The id is the application's OWN identity: it is drawn
    /// FOR the application rather than rendered from how many applications the store has
    /// minted, so the first application of a fresh store is not addressed by a constant
    /// and the ARN built over it addresses that one application alone.
    pub fn mint_application_id(&self) -> String {
        drawn_id(APPLICATION_ID_LEN, str::to_string, |id| {
            self.applications.contains_key(id)
        })
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
    /// The id comes from [`Self::mint_application_id`], which draws one this store
    /// holds no application under, so the key cannot already be taken; the entry API is
    /// used so no path can overwrite a record this store already holds under one.
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
    /// imports into and then a DRAWN opaque part ([`drawn_token`]). The application's
    /// `Id` carries no hyphen (`^[a-z0-9]{3,30}$`), so the hyphen between the two parts
    /// is what [`migration_application_id`] reads the application back out of.
    pub fn mint_migration_id(&self, application_id: &str) -> String {
        drawn_id(
            TOKEN_LEN,
            |token| format!("{application_id}-{token}"),
            |id| self.migrations.contains_key(id),
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
    /// The id comes from [`Self::mint_migration_id`], which draws one this store holds
    /// no job under, so the key cannot already be taken; the entry API is used so no
    /// path can overwrite a record this store already holds under one.
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
    /// `attachmentId` is an opaque string the model constrains not at all, so it is a
    /// DRAWN token ([`drawn_token`]) behind the `dsa-` prefix that names the resource it
    /// identifies the way `cc-` names a cross-cluster connection.
    pub fn mint_attachment_id(&self) -> String {
        drawn_id(
            TOKEN_LEN,
            |token| format!("dsa-{token}"),
            |id| self.attachment_id_held(id),
        )
    }

    /// Whether an attachment of any application of this store is already addressed by
    /// `attachment_id`. An attachment is held ON the application it links, so every
    /// application is asked.
    ///
    /// Reads ONLY.
    fn attachment_id_held(&self, attachment_id: &str) -> bool {
        self.applications.values().any(|application| {
            application
                .attachments
                .values()
                .any(|attachment| attachment.attachment_id == attachment_id)
        })
    }

    /// Materialise the status move the clock owes every data source attachment of every
    /// stored application, for a caller about to report on any of them.
    ///
    /// The data sources the attachments name are resolved FIRST, because that is what
    /// decides whether a pending attachment that has fallen due completes, waits longer or
    /// fails: an attachment whose data source is gone can never complete, and one whose
    /// data source is still coming up waits for it. Each DISTINCT ARN is resolved once, so
    /// two applications attached to one data source cost one resolution rather than two.
    ///
    /// `foreign` is where every data source another service holds stands, resolved before
    /// this store was locked ([`crate::state::foreign_data_sources`]).
    pub fn settle_attachments(&mut self, now: SystemTime, foreign: &ForeignDataSources) {
        let named: BTreeSet<String> = self
            .applications
            .values()
            .flat_map(|application| application.attachments.keys())
            .cloned()
            .collect();
        let activations: BTreeMap<String, DataSourceActivation> = named
            .into_iter()
            .map(|arn| {
                let activation = self.data_source_activation(&arn, now, foreign);
                (arn, activation)
            })
            .collect();
        for application in self.applications.values_mut() {
            for attachment in application.attachments.values_mut() {
                attachment.settle(now, &activations);
            }
            // A detach whose own span has passed has taken its attachment with it, so the
            // record it was serializing further detaches on is released here — what was
            // owed at `now` whether or not anybody called.
            application
                .attachments
                .retain(|_, attachment| !attachment.detach_landed(now));
        }
    }

    /// Materialise everything the clock owes the records an attachment operation reads:
    /// the applications the attachments belong to, and the attachments' own moves.
    ///
    /// The ONE place the attachment operations settle, so the four of them cannot drift
    /// apart over which records they report at `now`.
    pub fn settle_for_attachments(&mut self, now: SystemTime, foreign: &ForeignDataSources) {
        self.settle_applications(now);
        self.settle_attachments(now, foreign);
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
    /// its domains alone: a domain of this account, a direct query data source of it,
    /// this account's AWS Glue Data Catalog, or an OpenSearch Serverless collection, which
    /// another service holds and which is resolved from `foreign`, the snapshot of that
    /// service's own store
    /// ([`crate::state::foreign_data_sources`]). Anything else names no data source of this
    /// account, which is what an attachment can never complete against.
    ///
    /// The ONE resolution of a data source ARN, so the attach that decides an
    /// attachment's outcome and the settlement that lands it cannot disagree about what
    /// the account holds.
    ///
    /// `now` is the instant the resolution is made at: a data source whose own activation
    /// has already fallen due at `now` is ACTIVE, whether or not its record has been
    /// settled yet, so a report of an attachment does not wait for the data source's own
    /// reader to materialise what the clock already owed it.
    ///
    /// Reads ONLY.
    pub fn data_source_activation(
        &self,
        arn: &str,
        now: SystemTime,
        foreign: &ForeignDataSources,
    ) -> DataSourceActivation {
        let activation = self.held_data_source_activation(arn, foreign);
        match activation {
            DataSourceActivation::ActiveAt(due) if now >= due => DataSourceActivation::Active,
            held => held,
        }
    }

    /// Where the data source `arn` names stands by its own RECORD, before the clock is read
    /// against it — [`Self::data_source_activation`]'s one lookup.
    fn held_data_source_activation(
        &self,
        arn: &str,
        foreign: &ForeignDataSources,
    ) -> DataSourceActivation {
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
        // Everything else is resolved where it is held, which for a kind of resource this
        // store knows nothing about is another service's store.
        foreign.activation(arn)
    }

    /// Whether this account holds a data source under `arn` — what decides whether an
    /// attach can be served at all, and whether a pending attachment that has fallen due
    /// completes or fails.
    pub fn holds_data_source(
        &self,
        arn: &str,
        now: SystemTime,
        foreign: &ForeignDataSources,
    ) -> bool {
        self.data_source_activation(arn, now, foreign) != DataSourceActivation::Absent
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

    /// The insight the entity a request names holds under `insight_id`, if it holds one.
    ///
    /// An insight is addressed by its entity and its id together, and the two halves are
    /// NOT alike: the ENTITY has to resolve — it is resolved exactly as
    /// [`Self::insights_of_entity`] resolves it, so a `DomainName` this account holds no
    /// domain under is the ONE missing resource naming neither half of the address
    /// ([`insight_resource_not_found`]), and an entity whose type is outside
    /// `InsightEntityType` stays the malformed request it is — while the ID half merely
    /// SELECTS: an id the entity holds no insight under is `Ok(None)`, for the detail read
    /// to report on as the informational read it is. The id is looked up among the insights
    /// THAT entity holds, so an insight another entity holds selects nothing here.
    ///
    /// The sibling SUBMISSION addresses an insight the same way and does require both
    /// halves, because feedback is about an existing insight
    /// ([`Self::insight_for_feedback_mut`]).
    ///
    /// Reads ONLY.
    pub fn insight_held_by_entity(
        &self,
        entity: &InsightEntity,
        insight_id: &str,
    ) -> Result<Option<&InsightRecord>, InsightEntityRefusal> {
        let held = self.insights_of_entity(entity).map_err(|refusal| match refusal {
            // Which resource the entity named is not disclosed: an entity this account
            // holds nothing for is simply a missing resource to a caller.
            InsightEntityRefusal::NotFound(_) => {
                InsightEntityRefusal::NotFound(insight_resource_not_found())
            }
            malformed => malformed,
        })?;
        Ok(held
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

    /// Every package VISIBLE to this account, in `PackageID` order: the plugin
    /// packages the SERVICE publishes ([`published_packages`]) and the packages this
    /// account created itself.
    ///
    /// This is the set every package READ reports and resolves an id against —
    /// `DescribePackages` describes "all packages available to Amazon OpenSearch
    /// Service", not only the ones a caller created. The published entries are
    /// computed from the read-only catalog table on each call rather than inserted
    /// into the store, so reporting them writes nothing, and the merged set is ordered
    /// by package id — the order the store itself keys the account's packages in — so
    /// two calls report the same order and a pagination token names a place in it.
    pub fn visible_packages(&self) -> Vec<Package> {
        let mut visible = published_packages();
        visible.extend(self.packages.values().cloned());
        visible.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        visible
    }

    /// The visible package carrying `package_id` — one this account created, else one
    /// the service publishes — and `None` for an id neither set holds.
    pub fn visible_package(&self, package_id: &str) -> Option<Package> {
        self.packages
            .get(package_id)
            .cloned()
            .or_else(|| published_package(package_id))
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
        // Resolved against the packages VISIBLE to this account
        // ([`Self::visible_package`]): a plugin package the service publishes is
        // associated with a domain exactly as one the account created is, so an id the
        // catalog publishes names a package here too. The condition is REPORTED, not
        // worded: the sentence an unresolvable `PackageID` is refused with belongs to
        // the operation that answers.
        let package = self
            .visible_package(&request.package_id)
            .ok_or_else(|| AssociationRefusal::PackageUnresolved(request.package_id.clone()))?;

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
            if self.visible_package(prerequisite).is_none() {
                return Err(AssociationRefusal::PackageUnresolved(prerequisite.clone()));
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

        Ok(package)
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
            package_version: package_version_label(package.version),
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
            .visible_package(package_id)
            .ok_or_else(|| AssociationRefusal::PackageUnresolved(package_id.to_string()))?;

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

        Ok(package)
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

    /// The published offering catalog, in the order the service publishes it: the order of
    /// the `ReservedInstanceOfferingId` each entry carries, which is the key the catalog is
    /// held under.
    ///
    /// A published order has to be a function of the records alone, so two reads report the
    /// same offerings in the same order and a page resumes where the previous one stopped.
    /// The key every entry publishes is its own offering id, and ordering the catalog by it
    /// interleaves instance type, term and payment option — which is what the recorded
    /// listing carries, and what keeps a page of the catalog from being one instance type's
    /// corner of it: the listing is `@paginated` with `MaxResults` capped at 100 over a
    /// catalog of every instance type the service publishes times each term times each
    /// payment option, so nesting type over term over payment option would leave a client
    /// reading page 1 a single instance family and a single term.
    pub fn published_reserved_instance_offerings(&self) -> Vec<&ReservedInstanceOffering> {
        self.reserved_instance_offerings.values().collect()
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
