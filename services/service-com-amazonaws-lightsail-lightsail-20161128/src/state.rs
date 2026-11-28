// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lightsail-lightsail-20161128 service.
//!
//! The slice implemented so far is Lightsail's primary resource, the **instance**
//! (a virtual private server), the **block storage disk** (additional storage an
//! instance can attach), the **disk snapshot** (a point-in-time copy of a disk
//! or of an instance's system volume), the **instance snapshot** (a point-in-time
//! copy of a whole instance — its system volume, its blueprint and bundle, and
//! the block storage disks attached to it) together with the **export snapshot
//! record** (what an export of one of those snapshots to Amazon EC2 leaves
//! behind), the **domain** (a DNS zone the account holds recordsets in), the
//! **container service** (a compute resource containers are
//! deployed to), the **bucket** (an object-storage resource of the Lightsail
//! object storage service) together with the **access keys** it grants
//! programmatic access through — whose OBJECTS live in the one object store this
//! emulator holds, Amazon S3's own, which this module provisions a bucket in
//! ([`claim_bucket_object_store`]) rather than keeping a second object map here —
//! the **managed relational database** (a MySQL or PostgreSQL database the service
//! runs for the account) together with the **database snapshot** (a point-in-time
//! copy of one) and the **event log** each database carries of what the service
//! has done to it, the **load balancer** (a regional resource that spreads
//! HTTP traffic across the instances attached to it), the **static IP** (a public
//! IPv4 address the account reserves and attaches to an instance), the **SSH key
//! pair** (the public half of a key pair the account holds, which an instance's
//! `sshKeyName` names), the **CDN
//! distribution** (a global resource that caches and serves the content of an
//! origin resource) together with the **certificate** (an SSL/TLS certificate
//! the account requests for a domain, which is attached to a distribution so it
//! accepts HTTPS traffic for that certificate's domains), the peering of a
//! Region's **Lightsail VPC** with the account's default VPC there, the
//! temporary **container image registry log in** credentials
//! `CreateContainerServiceRegistryLogin` hands out for the account's container
//! image registry, the **contact method** (the email address or mobile phone
//! number the account's notifications are sent to, one per protocol per
//! Region), the append-only **operation log** every mutating
//! call writes, and the service's static **Region catalog** — the Regions and
//! Availability Zones `GetRegions` lists and every other operation validates a
//! location against. Everything mutable in this module is reached through
//! [`with_state`], so no handler can observe a store whose clock-owed transitions
//! have not been applied yet.
//!
//! Two kinds of value live on a stored instance:
//!
//! * what the caller asked for — name, availability zone, blueprint, bundle,
//!   key pair, tags, add-ons, IP address type, and the inbound firewall rules a
//!   later `OpenInstancePublicPorts`, `CloseInstancePublicPorts` or
//!   `PutInstancePublicPorts` call makes of the platform's defaults — kept
//!   verbatim; and
//! * what the service assigns — ARN, support code, creation time, addresses,
//!   hardware, firewall rules, user name and lifecycle state — each computed
//!   from the request plus this store's own allocators, never from a canned body.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

// A bucket's OBJECT data plane is not a map of this module's own: a Lightsail bucket
// is an S3-compatible object store, and the service that serves the S3 object
// protocol in this emulator is Amazon S3, so a bucket is provisioned in that one
// store (see `claim_bucket_object_store`) and this is the outcome it reports.
pub use ::service_com_amazonaws_s3_amazons3::state::BucketCreation;

use crate::ops::types::{
    AccessKey, AccessKeyLastUsed, AccessRules, AccountLevelBpaSync, AddOn, AddOnRequest, Alarm,
    AttachedDisk, AutoSnapshotDetails,
    AvailabilityZone, Blueprint, Bucket, BucketAccessLogConfig, BucketBundle, BucketCorsConfig,
    BucketCorsRule, BucketState, Bundle, CacheBehavior, CacheBehaviorPerPath, CacheSettings,
    Certificate, CertificateSummary, Container,
    ContainerImage,
    ContainerService, ContainerServiceDeployment, ContainerServiceDeploymentRequest,
    ContainerServiceECRImagePullerRole, ContainerServiceEndpoint,
    ContactMethod,
    ContainerServiceHealthCheckConfig, ContainerServicePower, ContainerServiceStateDetail,
    DestinationInfo,
    DistributionBundle, DistributionCustomErrorResponse,
    Domain, DomainEntry, DomainValidationRecord, ExportSnapshotRecord,
    ExportSnapshotRecordSourceInfo,
    HostKeyAttributes, Instance,
    InstanceHardware, InstanceHealthStatus, InstanceHealthStatusDetail, InstanceHealthSummary,
    InstanceMetadataOptions, InstanceNetworking,
    InstancePortInfo, InstanceState, KeyPair, LightsailDistribution,
    LightsailDistributionSubdomain, LoadBalancer,
    LoadBalancerTlsCertificate, LoadBalancerTlsCertificateDomainValidationOption,
    LoadBalancerTlsCertificateDomainValidationRecord, LoadBalancerTlsCertificateRenewalSummary,
    LoadBalancerTlsCertificateSummary,
    MonitoredResourceInfo, MonthlyTransfer, Operation, Origin, PartnerInfo, PrivateRegistryAccess,
    Region,
    RelationalDatabaseBundle, RelationalDatabaseParameter,
    ResourceLocation, ResourceReceivingAccess,
    StaticIp, Subdomain, SubdomainAttachableResource, Tag, WafConfig, WafRule, REGION_NAME_VALUES,
};

// ---------------------------------------------------------------------------
// Service identity
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Operation log
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller, so each growth site carries a cap
// and nothing caller-sized reaches the store unchecked.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Catalog paging
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Bundles
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Buckets
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Blueprints
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Relational database blueprints
// ---------------------------------------------------------------------------

// This section is a PUBLISHED catalog, not per-account state: the data every
// account sees the same, which the reporting operation over it answers from. Its
// source is named below, and the modelled view type is referred to by its full
// path so this section needs nothing from the import list above.

// ---------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Firewall
// ---------------------------------------------------------------------------

// The shapes and the value catalog THIS slice reads, imported beside it rather
// than in the module's one shared `use` at the top: that list is where every
// concurrent change to this service adds a name, so a name added there collides
// with every other change in the same window, while these lines are the
// firewall's own. (The same reasoning the router's `tests/all.rs` records for
// where a suite is declared.)
use crate::ops::types::{InstancePortState, PortInfo, NETWORK_PROTOCOL_VALUES};

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// GUI (Amazon DCV) sessions
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Block storage disks
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Instance snapshots
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// CloudFormation stack records
//
// Everything `CreateCloudFormationStack` and `GetCloudFormationStackRecords`
// need that is not a field of the store itself lives in this one section — the
// resource's identifiers, bounds, page size and stored record — rather than
// beside each of its neighbours' own constants. The record's clock-owed
// `Started` -> `Succeeded` settle is applied by
// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`], bounded by
// [`MAX_CLOUD_FORMATION_STACK_RECORDS`], like every other resource's.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Static IPs
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSH key pairs
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Managed relational databases
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Instance HTTPS setup
//
// The append-only history of the `SetupInstanceHttps` calls made against one
// instance: what `GetSetupHistory` reports back. It sits beside the operation
// log, which every one of those calls also writes an entry to.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Container services
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Instance metrics
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Load balancers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Load balancer TLS security policies
// ---------------------------------------------------------------------------

use crate::ops::types::LoadBalancerTlsPolicy;

// ---------------------------------------------------------------------------
// SSL/TLS certificates and CDN distributions
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Released attachments
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-lightsail-lightsail-20161128. A handler
/// reaches it through [`with_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLightsailLightsail20161128State {
    /// Every instance, keyed by its name — the identifier every instance
    /// operation takes. A `BTreeMap` so `GetInstances` walks a stable, total
    /// order and a page token can name the next instance.
    pub instances: BTreeMap<String, StoredInstance>,
    /// Every domain recordset, keyed by its domain name — the identifier every
    /// domain operation takes. A `BTreeMap` so `GetDomains` walks a stable,
    /// total order.
    pub domains: BTreeMap<String, StoredDomain>,
    /// Every block storage disk, keyed by its disk name — the identifier every
    /// disk operation takes. A `BTreeMap` so `GetDisks` walks a stable, total
    /// order and a page token can name the next disk.
    ///
    /// The key is the name ALONE, not the `(region, name)` pair a bucket and a
    /// load balancer are held under, because every modelled disk operation
    /// addresses a disk by `diskName` and nothing else. The disk is still a
    /// LOCATED record — its [`StoredDisk::location`] carries the region of the
    /// `availabilityZone` its create named — and the one listing of it is scoped
    /// to a region ("all block storage disks in your AWS account and region"), so
    /// `GetDisks` selects that region's records on the walk rather than the key
    /// doing it.
    pub disks: BTreeMap<String, StoredDisk>,
    /// Every disk snapshot, keyed by its snapshot name — the identifier every
    /// disk snapshot operation takes. A `BTreeMap` so `GetDiskSnapshots` walks a
    /// stable, total order and a page token can name the next snapshot.
    pub disk_snapshots: BTreeMap<String, StoredDiskSnapshot>,
    /// Every instance snapshot, keyed by its snapshot name — the identifier every
    /// instance snapshot operation takes. A `BTreeMap` so `GetInstanceSnapshots`
    /// walks a stable, total order and a page token can name the next snapshot.
    pub instance_snapshots: BTreeMap<String, StoredInstanceSnapshot>,
    /// What the `AutoSnapshot` add-on has taken of each resource it is enabled
    /// on, keyed by that resource's `ResourceType` and its name — the pair that
    /// identifies it, because an automatic snapshot has no name of its own and
    /// one name can be both an instance's and a disk's. A `BTreeMap` so the
    /// walk in
    /// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] is in a
    /// stable, total order.
    ///
    /// Bounded by [`MAX_INSTANCES`] plus [`MAX_DISKS`] (an entry belongs to one
    /// instance or disk this account holds) and, within one entry, by
    /// [`MAX_AUTO_SNAPSHOTS_PER_RESOURCE`].
    pub auto_snapshots: BTreeMap<(&'static str, String), StoredAutoSnapshots>,
    /// Every export snapshot record, keyed by the name this store minted for it
    /// — the only identifier an export snapshot record has, because the caller
    /// names none. A `BTreeMap` so `GetExportSnapshotRecords` walks a stable,
    /// total order and a page token can name the next record.
    pub export_snapshot_records: BTreeMap<String, StoredExportSnapshotRecord>,
    /// Every CloudFormation stack record, keyed by the name this store minted
    /// for it — the only identifier a CloudFormation stack record has, because
    /// the caller names none. A `BTreeMap` so
    /// `GetCloudFormationStackRecords` walks a stable, total order and a page
    /// token can name the next record.
    pub cloud_formation_stack_records: BTreeMap<String, StoredCloudFormationStackRecord>,
    /// The append-only operation log, newest last: the source of truth for the
    /// `operations` every mutating instance call answers with.
    pub operations: Vec<StoredOperation>,
    /// The append-only `SetupInstanceHttps` history, newest last: the source of
    /// truth for what `GetSetupHistory` reports for an instance. A `Vec` for the
    /// same reason the operation log is one — it is a log, several entries of it
    /// name the same instance, and `GetSetupHistory` reports the most recent
    /// ones first.
    pub instance_https_setups: Vec<StoredInstanceHttpsSetup>,
    /// Every container service, keyed by the region it was created in and its
    /// name — the pair that identifies it, because a container service name is
    /// unique within one region of the account and `GetContainerServices`
    /// reports "all of your container services in the Amazon Web Services Region
    /// where the request is made". A `BTreeMap` so a listing walks a stable,
    /// total order.
    pub container_services: BTreeMap<(String, String), StoredContainerService>,
    /// Every bucket, keyed by the region it was created in and its name — the
    /// pair that identifies it, because a bucket name is unique within one
    /// region of the account and `GetBuckets` reports "all of your buckets in the
    /// Amazon Web Services Region where the request is made". A `BTreeMap` so
    /// `GetBuckets` walks a stable, total order and a page token can name the
    /// next bucket.
    pub buckets: BTreeMap<(String, String), StoredBucket>,
    /// Every managed relational database, keyed by its name — the identifier
    /// every database operation takes. A `BTreeMap` so `GetRelationalDatabases`
    /// walks a stable, total order and a page token can name the next database.
    pub relational_databases: BTreeMap<String, StoredRelationalDatabase>,
    /// The open DATA plane of each managed relational database, keyed by the
    /// same name the record is: the listener its engine is served on, and the
    /// engine itself.
    ///
    /// Beside the records rather than on them, because a stored record is
    /// CLONED by the reads that report it and a listener is not a value that can
    /// be copied: the plane is the live resource, held here for as long as the
    /// database is, and dropped with it — which is what closes the port
    /// ([`relational_database_plane`]).
    pub relational_database_planes: BTreeMap<String, RelationalDatabasePlane>,
    /// Every managed relational database snapshot, keyed by its snapshot name —
    /// the identifier every database snapshot operation takes. A `BTreeMap` so
    /// `GetRelationalDatabaseSnapshots` walks a stable, total order and a page
    /// token can name the next snapshot.
    pub relational_database_snapshots: BTreeMap<String, StoredRelationalDatabaseSnapshot>,
    /// Every static IP, keyed by its static IP name — the identifier every static
    /// IP operation takes, and the same key an instance is held under, because a
    /// static IP is attached to an instance by name. A `BTreeMap` so
    /// `GetStaticIps` walks a stable, total order and a page token can name the
    /// next static IP.
    pub static_ips: BTreeMap<String, StoredStaticIp>,
    /// Every SSH key pair, keyed by its key pair name — the identifier every key
    /// pair operation takes. A `BTreeMap` so `GetKeyPairs` walks a stable, total
    /// order and a page token can name the next key pair.
    ///
    /// Keyed by name alone, like the instance whose `sshKeyName` names one:
    /// both are served by name whichever Region the request is made from. That
    /// is this store's existing shape for a resource an instance refers to, and
    /// it is a DEVIATION worth knowing about — against AWS a key pair belongs to
    /// one Region, so a pair held in another Region is not addressable from this
    /// endpoint. The key to revisit is here, together with `GetKeyPairs`'
    /// Region-scoped listing, once the calls that create a key pair are
    /// implemented.
    pub key_pairs: BTreeMap<String, StoredKeyPair>,
    /// The key material this service holds for the account's Lightsail DEFAULT
    /// key pair — the pair `DownloadDefaultKeyPair` mints when the account
    /// holds none and hands back on every later call, which is why that one
    /// pair's private half is held at all (see
    /// [`HeldDefaultKeyPairMaterial`]).
    ///
    /// Not a second copy of the key pair: the RESOURCE is the record in
    /// [`Self::key_pairs`] under [`DEFAULT_SSH_KEY_NAME`], this is only the
    /// material of it, reported through [`Self::default_key_pair_material`] and
    /// dropped by [`Self::remove_key_pair`] when that record goes.
    pub default_key_pair_material: Option<HeldDefaultKeyPairMaterial>,
    /// Every alarm, keyed by its name — the identifier every alarm operation
    /// takes, and the one `PutAlarm` creates or updates a record under. A
    /// `BTreeMap` so `GetAlarms` walks a stable, total order and a page token can
    /// name the next alarm.
    pub alarms: BTreeMap<String, StoredAlarm>,
    /// Every contact method, keyed by the Region it was created in and the
    /// `ContactProtocol` it sends through — the pair that identifies it, because
    /// "you can add one email address and one mobile phone number contact method
    /// in each Amazon Web Services Region" (`CreateContactMethod`), and every
    /// contact-method operation is scoped to the Region the request is made in.
    ///
    /// Unbounded by any `MAX_` of its own, and it needs none: both halves of the
    /// key come from a modelled enum — the Region from [`request_region`], which
    /// only ever answers a `RegionName` value, and the protocol from
    /// `ContactProtocol` — so the map holds at most one entry per Region and
    /// protocol however many calls a caller makes. A `BTreeMap` so
    /// `GetContactMethods` walks a stable, total order.
    pub contact_methods: BTreeMap<(String, String), StoredContactMethod>,
    /// Every load balancer, keyed by the region it was created in and its name.
    ///
    /// A load balancer is a REGIONAL resource — the modelled
    /// `CreateLoadBalancer` documentation caps them "per AWS Region in your
    /// account" — so the region is part of its identity, exactly as it is for a
    /// container service and a bucket. A `BTreeMap` so a listing walks a stable,
    /// total order and a page token can name the next load balancer.
    pub load_balancers: BTreeMap<(String, String), StoredLoadBalancer>,
    /// The live sets of container image registry log in credentials, keyed by
    /// the region the creating request was made in, newest last.
    ///
    /// The registry is the ACCOUNT's — `CreateContainerServiceRegistryLogin`
    /// creates credentials for "the container image registry of your Lightsail
    /// account" — and the address each set was handed carries the region that
    /// request was signed for, so that region is what a later log in presents
    /// its credentials at. The clock forgets a set 12 hours after it was created
    /// (see [`Self::advance_clock`]), so every set held here is still valid.
    pub registry_logins: BTreeMap<String, Vec<StoredRegistryLogin>>,
    /// The container image registry of the account, keyed by the region whose
    /// announced address a push reached it at.
    ///
    /// One registry per region, for the same reason the log in credentials are
    /// held per region: the address `CreateContainerServiceRegistryLogin` hands
    /// out carries the region the creating request was made in, so that is the
    /// registry a push and the `RegisterContainerImage` after it address. A
    /// region nothing has ever pushed to holds no entry. Each registry is
    /// bounded by [`MAX_REGISTRY_BYTES_PER_REGION`], and the number of them by
    /// the modelled `RegionName` enum: an entry is created only for a region a
    /// push authenticated in, and a set of log in credentials is held under the
    /// region [`request_region`] answers, which is always one of that enum's
    /// values.
    pub container_image_registries: BTreeMap<String, StoredImageRegistry>,
    /// The peering of each Region's Lightsail VPC with the account's default VPC
    /// there, keyed by the Region's name. A Region with no entry is one whose
    /// Lightsail VPC nothing has ever asked to peer.
    ///
    /// Unbounded by any `MAX_` of its own, and it needs none: the key is what
    /// [`request_region`] answers, which is always a value of the modelled
    /// `RegionName` enum, so the map holds at most one entry per Region that
    /// enum declares however many calls a caller makes. A `BTreeMap` so the walk
    /// in [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] is in a
    /// stable, total order.
    pub vpc_peerings: BTreeMap<String, StoredVpcPeering>,
    /// Every CDN distribution, keyed by its name — the identifier every
    /// distribution operation takes.
    ///
    /// NOT keyed by region, unlike a bucket or a load balancer: a distribution
    /// is a global resource and "all distributions are located in the
    /// `us-east-1` Region" (`LightsailDistribution$location`), so one name
    /// addresses one distribution of the account whichever Region a request is
    /// made from. A `BTreeMap` so `GetDistributions` walks a stable, total order
    /// and a page token can name the next distribution.
    pub distributions: BTreeMap<String, StoredDistribution>,
    /// Every SSL/TLS certificate, keyed by its certificate name — the identifier
    /// every certificate operation takes. NOT keyed by region, for the same
    /// reason a distribution is not: a Lightsail certificate is addressed from
    /// the one Region its API is served in (see [`DISTRIBUTION_REGION_NAME`]). A
    /// `BTreeMap` so `GetCertificates` walks a stable, total order and a page
    /// token can name the next certificate.
    pub certificates: BTreeMap<String, StoredCertificate>,
    /// Every subdomain, keyed by its subdomain name — the identifier every
    /// subdomain operation takes. NOT keyed by region, for the same reason a
    /// distribution is not: a subdomain is published under one global domain
    /// and addressed from the one Region its API is served in (see
    /// [`SUBDOMAIN_REGION_NAME`]). A `BTreeMap` so `GetSubdomains` walks a
    /// stable, total order and a page token can name the next subdomain.
    ///
    /// Bounded by [`MAX_SUBDOMAINS`] records a caller created, each of which
    /// can hold one IMPLICIT parent beside it.
    pub subdomains: BTreeMap<String, StoredSubdomain>,
    /// The reservation this account holds of each free subdomain, keyed by the
    /// TOP-LEVEL name it is held under, with the instant it expires at.
    ///
    /// Beside the subdomain records rather than among them: a reservation holds
    /// a name the account does NOT hold a subdomain under, which is what makes
    /// it a reservation. Bounded by [`MAX_SUBDOMAIN_RESERVATIONS`]: the ONE call
    /// that reads and writes reservations (`CheckSubdomainAvailability`)
    /// releases every expired entry and refuses a sixth live one before it
    /// writes, so every reservation held here is still active and the map never
    /// grows past that quota. A `BTreeMap` so it is walked in a stable, total
    /// order.
    pub subdomain_reservations: BTreeMap<String, i64>,
    /// The profile of the account this emulator serves — the Lightsail partner
    /// program membership it holds — which `GetProfile` reports.
    ///
    /// ONE record rather than a map: a profile belongs to the ACCOUNT, and this
    /// emulator serves one account (`engine::emulator_account_id`), so there is
    /// one profile however many Regions a caller reaches it from.
    pub account_profile: StoredAccountProfile,
    /// Offset of the next address pair to hand out, so two instances never share
    /// a public or a private address within one process.
    next_address: u32,
    /// Mixed into every minted id, so two ids minted in the same nanosecond
    /// differ.
    next_id: u64,
}

impl ComAmazonawsLightsailLightsail20161128State {
    /// The instance `name` names, for a reporting operation.
    pub fn instance(&self, name: &str) -> Option<&StoredInstance> {
        self.instances.get(name)
    }

    /// The instance `name` names, for an operation the spec says mutates it.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames an instance.
    pub fn instance_mut(&mut self, name: &str) -> Option<&mut StoredInstance> {
        self.instances.get_mut(name)
    }

    /// Store a newly created instance.
    pub fn insert_instance(&mut self, instance: StoredInstance) {
        // A create that enabled the `AutoSnapshot` add-on started that add-on's
        // daily schedule at the instant of the create, so the schedule is
        // anchored there rather than at whenever somebody next calls.
        let name = instance.name.clone();
        let created_at_ms = instance.created_at_ms;
        let hour = enabled_auto_snapshot_hour(&instance.add_ons);
        self.discard_kept_auto_snapshots(&name);
        self.instances.insert(name.clone(), instance);
        self.reconcile_auto_snapshots(created_at_ms, RESOURCE_TYPE_INSTANCE, &name, hour.as_deref());
    }

    /// Create the GUI (Amazon DCV) session access of the virtual computer
    /// `name`, answering what was stored — or `None` when no instance of that
    /// name is stored, in which case nothing was written and no id was spent.
    ///
    /// The presence check comes BEFORE the mint on purpose: a call that resolves
    /// no instance must not consume an identifier, and the mint and the write
    /// live together here so no caller can be answered with a session the store
    /// does not hold.
    ///
    /// A session started on a virtual computer that is still coming up is up
    /// once that computer is: the session is a session ON the computer, so the
    /// instant it finishes coming up follows the one the clock already owes the
    /// computer its running state (see
    /// [`StoredGuiSession::starting_once_ready`]).
    pub fn create_gui_session(&mut self, name: &str, now_ms: i64) -> Option<StoredGuiSession> {
        let instance = self.instances.get(name)?;
        // Only a transition INTO `running` makes the computer ready; one into
        // any other state leaves the session's start where this call is.
        let ready_at_ms = instance
            .due
            .filter(|due| due.state == RUNNING)
            .map_or(now_ms, |due| due.at_ms);
        let token = self.mint_uuid();
        let session = StoredGuiSession::starting_once_ready(token, now_ms, ready_at_ms);
        self.instances.get_mut(name)?.gui_session = Some(session.clone());
        Some(session)
    }

    /// The domain recordset `name` names, for a reporting operation.
    pub fn domain(&self, name: &str) -> Option<&StoredDomain> {
        self.domains.get(name)
    }

    /// Store a newly created domain recordset.
    pub fn insert_domain(&mut self, domain: StoredDomain) {
        self.domains.insert(domain.name.clone(), domain);
    }

    /// Remove the domain recordset `name` names, answering it. `None` when
    /// nothing matched.
    ///
    /// A zone's recordset entries live on the record, so removing it removes
    /// "the specified domain recordset and all of its domain records" — what the
    /// modelled `DeleteDomain` documentation says the call deletes — in one go,
    /// with no second map to keep in step.
    pub fn remove_domain(&mut self, name: &str) -> Option<StoredDomain> {
        self.domains.remove(name)
    }

    /// The domain recordset `name` names, for an operation the spec says mutates
    /// it — adding, changing or removing one of the zone's entries.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a zone.
    pub fn domain_mut(&mut self, name: &str) -> Option<&mut StoredDomain> {
        self.domains.get_mut(name)
    }

    /// The block storage disk `name` names, for a reporting operation.
    pub fn disk(&self, name: &str) -> Option<&StoredDisk> {
        self.disks.get(name)
    }

    /// Store a newly created block storage disk.
    pub fn insert_disk(&mut self, disk: StoredDisk) {
        // The same anchoring an instance's schedule gets
        // ([`Self::insert_instance`]): the add-on a create enabled has been
        // running since that create.
        let name = disk.name.clone();
        let created_at_ms = disk.created_at_ms;
        let hour = enabled_auto_snapshot_hour(&disk.add_ons);
        self.discard_kept_auto_snapshots(&name);
        self.disks.insert(name.clone(), disk);
        self.reconcile_auto_snapshots(created_at_ms, RESOURCE_TYPE_DISK, &name, hour.as_deref());
    }

    /// The block storage disk `name` names, for an operation the spec says
    /// mutates it — attaching it to an instance, or releasing the attachment it
    /// holds.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a disk.
    pub fn disk_mut(&mut self, name: &str) -> Option<&mut StoredDisk> {
        self.disks.get_mut(name)
    }

    /// Remove the block storage disk `name` names, answering the record that was
    /// removed. `None` when nothing matched.
    ///
    /// A disk is removable only while it is attached to nothing (the modelled
    /// `DeleteDisk` documentation: "The disk must be in the available state (not
    /// attached to a Lightsail instance)"), which the caller is what enforces,
    /// so there is no attachment left for this removal to release — unlike
    /// [`Self::remove_instance`], whose resource other records do hold
    /// attachments to. A snapshot taken of the disk is a resource of its own and
    /// keeps the source name and ARN it recorded, so it outlives the disk.
    ///
    /// `delete_add_ons` says the delete was asked to delete the disk's add-ons
    /// with it, which is what decides the fate of the copies the disk's add-on
    /// took — see [`Self::settle_auto_snapshots_of_removed`].
    pub fn remove_disk(&mut self, name: &str, delete_add_ons: bool) -> Option<StoredDisk> {
        let disk = self.disks.remove(name)?;
        self.settle_auto_snapshots_of_removed(
            RESOURCE_TYPE_DISK,
            name,
            &disk.location,
            delete_add_ons,
        );
        Some(disk)
    }

    /// The instance `name` names as the modelled `Instance`, with every block
    /// storage disk attached to it reported in its hardware — and, when
    /// `include_health_status` says the reading request asked for it, the health
    /// of the instance's checks.
    ///
    /// The one way a reporting operation renders an instance: an attachment
    /// lives on the DISK record, so assembling the view needs the whole store,
    /// not just the instance.
    pub fn instance_view(&self, name: &str, include_health_status: bool) -> Option<Instance> {
        Some(self.instance_view_of(self.instances.get(name)?, include_health_status))
    }

    /// The same view for an instance record a caller already holds — the listing
    /// walk, which has the record in hand and renders it exactly as `GetInstance`
    /// does rather than keeping a second rendering of an instance.
    pub fn instance_view_of(
        &self,
        instance: &StoredInstance,
        include_health_status: bool,
    ) -> Instance {
        instance.view(
            self.attached_disk_views(&instance.name),
            self.next_auto_snapshot_time_of_day(RESOURCE_TYPE_INSTANCE, &instance.name),
            include_health_status,
        )
    }

    /// The disk record `disk` as the modelled `Disk`, with the daily time its
    /// next automatic copy is due at.
    ///
    /// The one way a reporting operation renders a disk: that time lives on the
    /// schedule the disk's add-on runs on, so rendering a disk needs the store
    /// and not just the disk record.
    pub fn disk_view(&self, disk: &StoredDisk) -> crate::ops::types::Disk {
        disk.view(self.next_auto_snapshot_time_of_day(RESOURCE_TYPE_DISK, &disk.name))
    }

    /// Every block storage disk attached to the instance `instance_name` names,
    /// in the store's own stable order (by disk name).
    pub fn disks_attached_to(&self, instance_name: &str) -> Vec<&StoredDisk> {
        self.disks
            .values()
            .filter(|disk| disk.attached_to.as_deref() == Some(instance_name))
            .collect()
    }

    /// The same disks as [`Self::disks_attached_to`], as the modelled `Disk`.
    pub fn attached_disk_views(&self, instance_name: &str) -> Vec<crate::ops::types::Disk> {
        self.disks_attached_to(instance_name)
            .into_iter()
            .map(|disk| self.disk_view(disk))
            .collect()
    }

    /// The daily time the next automatic snapshot of the resource `name` of kind
    /// `resource_type` is due at, in the `HH:00` UTC form the modelled
    /// `AddOn$nextSnapshotTimeOfDay` reports ("the next daily time an automatic
    /// snapshot will be created"), and `None` when the store owes that resource
    /// no further copy.
    ///
    /// READ from the schedule the resource's `AutoSnapshot` add-on runs on
    /// ([`StoredAutoSnapshots::due_at_ms`]), which is the instant the clock takes
    /// the next copy at — so the time reported is the hour that copy lands in,
    /// and it follows the schedule as the clock re-arms it after each copy. A
    /// resource whose add-on is disabled is owed nothing and reports nothing.
    fn next_auto_snapshot_time_of_day(
        &self,
        resource_type: &'static str,
        name: &str,
    ) -> Option<String> {
        self.auto_snapshots
            .get(&(resource_type, name.to_string()))?
            .due_at_ms
            .map(utc_hour_of_day)
    }

    /// The same disks as [`Self::disks_attached_to`], captured whole for an
    /// automatic snapshot of the instance: a copy of that snapshot describes the
    /// disks it captured as the resources they are, so the records are kept
    /// rather than reduced to what `AutoSnapshotDetails$fromAttachedDisks`
    /// reports of them.
    fn attached_disk_captures(&self, instance_name: &str) -> Vec<StoredDisk> {
        self.disks_attached_to(instance_name)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The automatic snapshots this store holds of the resource `name` names,
    /// together with the `ResourceType` that resource reports — what
    /// `GetAutoSnapshots` answers for it. `None` when that name addresses no
    /// automatic snapshots of this account at all.
    ///
    /// The name is resolved as every operation that addresses a resource's
    /// automatic snapshots resolves one ([`Self::auto_snapshot_source`]), so a
    /// delete that kept a resource's copies kept them reportable under the name
    /// they were taken under. A resource whose add-on has never been enabled
    /// holds no copy, which is an EMPTY list rather than a refusal: the resource
    /// exists, and the answer is that nothing has been copied of it.
    pub fn auto_snapshots_of(&self, name: &str) -> Option<(&'static str, Vec<AutoSnapshotDetails>)> {
        let (resource_type, _) = self.auto_snapshot_source(name)?;
        let reported = self
            .auto_snapshots
            .get(&(resource_type, name.to_string()))
            .map(StoredAutoSnapshots::view)
            .unwrap_or_default();
        Some((resource_type, reported))
    }

    /// The automatic snapshot of the resource `name` of kind `resource_type`
    /// that `date` addresses — the latest RESTORABLE copy of it when `date` is
    /// absent, which is what a request asking for "the latest available
    /// automatic snapshot" addresses. `None` when the resource holds no copy the
    /// request addresses.
    ///
    /// The add-on takes one copy a day ([`StoredAutoSnapshots::take`]), so a
    /// date addresses exactly one of a resource's copies; the latest restorable
    /// one is the newest copy the service has finished taking, the copies being
    /// held oldest first. A copy that is still being taken is held, and
    /// addressable by its date — what a caller may do with one is the caller's
    /// own rule, so the lookup answers it rather than hiding it.
    ///
    /// The ONE lookup every operation that addresses an automatic snapshot of a
    /// resource resolves it through, so the same request shape finds the same
    /// copy at whichever of those doors it arrives.
    pub fn auto_snapshot(
        &self,
        resource_type: &'static str,
        name: &str,
        date: Option<&str>,
    ) -> Option<&StoredAutoSnapshot> {
        let taken = self
            .auto_snapshots
            .get(&(resource_type, name.to_string()))
            .map(|schedule| schedule.taken.as_slice())
            .unwrap_or_default();
        match date {
            Some(date) => taken.iter().find(|copy| copy.date == date),
            None => taken
                .iter()
                .rev()
                .find(|copy| copy.status == AUTO_SNAPSHOT_STATUS_SUCCESS),
        }
    }

    /// Remove the automatic snapshot of the resource `name` names that was taken
    /// on `date`. `None` when that name addresses no automatic snapshots of this
    /// account at all.
    ///
    /// A copy is addressed by the resource it was taken of plus its DATE — it
    /// has no name and no ARN of its own — and the add-on takes one copy a day,
    /// so the date identifies exactly one of a resource's copies. The name is
    /// resolved as every other operation that addresses an automatic snapshot
    /// resolves it ([`Self::auto_snapshot_source`]), so a copy a delete kept is
    /// deletable under the name it was taken under.
    ///
    /// Removing a copy takes nothing else with it: the other copies stay, and
    /// the schedule keeps the instant its next copy falls due, so a deletion
    /// neither stops the add-on nor makes it take a replacement. A resource left
    /// holding no copy and owed none — its add-on is disabled, or the resource
    /// itself is gone and this was the last copy a delete kept of it — holds no
    /// schedule either, exactly as it did before the add-on was ever enabled.
    pub fn delete_auto_snapshot(
        &mut self,
        name: &str,
        date: &str,
    ) -> Option<AutoSnapshotDeletion> {
        let (resource_type, location) = self.auto_snapshot_source(name)?;
        let key = (resource_type, name.to_string());
        let found = self
            .auto_snapshots
            .get_mut(&key)
            .and_then(|schedule| {
                let at = schedule
                    .taken
                    .iter()
                    .position(|taken| taken.date == date)?;
                schedule.taken.remove(at);
                Some(schedule.taken.is_empty() && schedule.due_at_ms.is_none())
            });
        match found {
            Some(emptied) => {
                if emptied {
                    self.auto_snapshots.remove(&key);
                }
                Some(AutoSnapshotDeletion::Deleted {
                    resource_type,
                    location,
                })
            }
            None => Some(AutoSnapshotDeletion::NoCopyForDate { resource_type }),
        }
    }

    /// Bring the automatic snapshot schedule of the resource `name` of kind
    /// `resource_type` into line with its add-ons at `now_ms`, where `hour` is
    /// the daily hour its enabled `AutoSnapshot` add-on carries and `None` says
    /// it carries no such add-on.
    ///
    /// The one place a schedule is started or stopped, so every operation that
    /// writes a resource's add-ons — the creates, through
    /// [`Self::insert_instance`] and [`Self::insert_disk`], and the add-on
    /// changes, through [`Self::with_add_on_record`] — starts exactly the same
    /// schedule. Enabling the add-on makes the service owe the resource a copy:
    /// the first one falls due a short interval later
    /// ([`AUTO_SNAPSHOT_FIRST_MILLIS`]), and from then on the copies follow the
    /// add-on's own daily hour. An ENABLED add-on therefore always has its next
    /// copy outstanding, which is also what
    /// [`Self::next_auto_snapshot_time_of_day`] reports. Disabling it stops the
    /// schedule and keeps the copies already taken, which are the account's
    /// until something deletes them.
    fn reconcile_auto_snapshots(
        &mut self,
        now_ms: i64,
        resource_type: &'static str,
        name: &str,
        hour: Option<&str>,
    ) {
        let key = (resource_type, name.to_string());
        let Some(hour) = hour else {
            if let Some(schedule) = self.auto_snapshots.get_mut(&key) {
                schedule.due_at_ms = None;
                if schedule.taken.is_empty() {
                    self.auto_snapshots.remove(&key);
                }
            }
            return;
        };
        let schedule = self.auto_snapshots.entry(key).or_default();
        // An add-on already taking copies keeps the schedule it is on, so
        // modifying it does not restart the wait.
        if schedule.due_at_ms.is_some() {
            return;
        }
        let first_due_ms = now_ms.saturating_add(AUTO_SNAPSHOT_FIRST_MILLIS);
        schedule.due_at_ms = Some(
            if schedule
                .taken
                .last()
                .is_none_or(|taken| taken.date != utc_date(first_due_ms))
            {
                first_due_ms
            } else {
                // A resource that already holds the copy for the day the first
                // one would fall on is not copied twice because the add-on was
                // switched off and on again: the copy it is owed is the next one
                // the add-on's own daily hour comes round for.
                next_auto_snapshot_due_ms(first_due_ms, hour)
            },
        );
    }

    /// The daily hour the enabled `AutoSnapshot` add-on of the resource `name`
    /// of kind `resource_type` carries, and `None` when this account holds no
    /// such resource or that resource has the add-on disabled.
    fn auto_snapshot_hour(&self, resource_type: &'static str, name: &str) -> Option<String> {
        let add_ons = if resource_type == RESOURCE_TYPE_INSTANCE {
            &self.instances.get(name)?.add_ons
        } else {
            &self.disks.get(name)?.add_ons
        };
        enabled_auto_snapshot_hour(add_ons)
    }

    /// Settle the automatic snapshots of the resource `name` of kind
    /// `resource_type`, whose record a delete has just removed from `location`,
    /// where `delete_add_ons` says that delete was asked to delete the
    /// resource's add-ons with it.
    ///
    /// The add-on itself lives on the resource's record, so it goes with that
    /// record either way; what the request decides is the fate of what the
    /// add-on PRODUCED. A delete asked for the add-ons takes the copies with
    /// them. A delete that was not asked KEEPS them — nothing asked for them to
    /// be deleted — and stops the schedule: no further copy is owed of a
    /// resource this account no longer holds, and the copies that are left stay
    /// addressed by the name they were taken under
    /// ([`Self::auto_snapshot_source`]) until something deletes them
    /// ([`Self::delete_auto_snapshot`]), or a new resource takes that name over
    /// ([`Self::discard_kept_auto_snapshots`]).
    ///
    /// A resource whose add-on took no copy leaves nothing behind either way:
    /// an empty schedule that is owed nothing is not held, exactly as it is not
    /// held before the add-on is ever enabled.
    fn settle_auto_snapshots_of_removed(
        &mut self,
        resource_type: &'static str,
        name: &str,
        location: &StoredLocation,
        delete_add_ons: bool,
    ) {
        let key = (resource_type, name.to_string());
        if delete_add_ons {
            self.auto_snapshots.remove(&key);
            return;
        }
        let Some(schedule) = self.auto_snapshots.get_mut(&key) else {
            return;
        };
        schedule.due_at_ms = None;
        if schedule.taken.is_empty() {
            self.auto_snapshots.remove(&key);
            return;
        }
        schedule.kept_source = Some(KeptAutoSnapshotSource {
            removed_at_ms: now_ms(),
            location: location.clone(),
        });
        self.drop_oldest_kept_auto_snapshots();
    }

    /// Drop the kept copies of the resources removed longest ago until this
    /// store holds no more than [`MAX_KEPT_AUTO_SNAPSHOT_SOURCES`] resources'
    /// worth of them.
    fn drop_oldest_kept_auto_snapshots(&mut self) {
        let mut kept: Vec<(i64, (&'static str, String))> = self
            .auto_snapshots
            .iter()
            .filter_map(|(key, schedule)| {
                Some((schedule.kept_source.as_ref()?.removed_at_ms, key.clone()))
            })
            .collect();
        if kept.len() <= MAX_KEPT_AUTO_SNAPSHOT_SOURCES {
            return;
        }
        kept.sort();
        let excess = kept.len() - MAX_KEPT_AUTO_SNAPSHOT_SOURCES;
        for (_, key) in kept.into_iter().take(excess) {
            self.auto_snapshots.remove(&key);
        }
    }

    /// Discard every automatic snapshot kept under the name `name`, called as a
    /// resource is stored under that name.
    ///
    /// A copy is addressed by the name of the resource it was taken of, and a
    /// resource created under that name takes the name over: the copies kept
    /// under it were taken of a resource this account no longer holds, so they
    /// are not this one's, and a resource starts out with the copies its own
    /// add-on has taken — none. Both kinds the add-on applies to are discarded,
    /// whichever kind the new resource is: a name is unique per resource type,
    /// so copies kept of an instance would otherwise sit behind a disk created
    /// under the same name and come back when that disk went. A schedule the
    /// account still holds the resource of is left exactly as it is, which is
    /// what keeps a resource's own copies across the writes that re-store its
    /// record.
    fn discard_kept_auto_snapshots(&mut self, name: &str) {
        for resource_type in [RESOURCE_TYPE_INSTANCE, RESOURCE_TYPE_DISK] {
            let key = (resource_type, name.to_string());
            if self
                .auto_snapshots
                .get(&key)
                .is_some_and(|schedule| schedule.kept_source.is_some())
            {
                self.auto_snapshots.remove(&key);
            }
        }
    }

    /// The kind of resource the name `name` addresses automatic snapshots of,
    /// and where that resource is: an instance or a block storage disk this
    /// account holds, or — once a delete has kept its copies — the resource
    /// those copies were taken of.
    ///
    /// ONE name addresses one resource, and the resources an automatic snapshot
    /// is taken of are the ones the `AutoSnapshot` add-on describes ("an Amazon
    /// Lightsail instance or disk"), so this asks the same two maps the add-on
    /// resolution does ([`Self::with_add_on_record`]), in the same order, and
    /// then the copies kept under that name. The ONE place an operation that
    /// addresses a resource's automatic snapshots resolves the name, so the same
    /// name finds the same copies at whichever of those doors it arrives.
    fn auto_snapshot_source(&self, name: &str) -> Option<(&'static str, StoredLocation)> {
        if let Some(instance) = self.instances.get(name) {
            return Some((RESOURCE_TYPE_INSTANCE, instance.location.clone()));
        }
        if let Some(disk) = self.disks.get(name) {
            return Some((RESOURCE_TYPE_DISK, disk.location.clone()));
        }
        [RESOURCE_TYPE_INSTANCE, RESOURCE_TYPE_DISK]
            .into_iter()
            .find_map(|resource_type| {
                let kept = self
                    .auto_snapshots
                    .get(&(resource_type, name.to_string()))?
                    .kept_source
                    .as_ref()?;
                Some((resource_type, kept.location.clone()))
            })
    }

    /// The disk snapshot `name` names, for a reporting operation.
    pub fn disk_snapshot(&self, name: &str) -> Option<&StoredDiskSnapshot> {
        self.disk_snapshots.get(name)
    }

    /// The static IP `name` names, for a reporting operation.
    pub fn static_ip(&self, name: &str) -> Option<&StoredStaticIp> {
        self.static_ips.get(name)
    }

    /// The static IP attached to the instance `instance_name`, while one is.
    ///
    /// The attachment lives on the STATIC IP record — the address is what is
    /// attached, exactly as a disk names the instance it is attached to — so the
    /// one way to find it is to ask the static IPs, and this is where that walk
    /// lives rather than in each handler that needs it. The walk is bounded by
    /// [`MAX_STATIC_IPS`].
    pub fn attached_static_ip(&self, instance_name: &str) -> Option<&StoredStaticIp> {
        self.static_ips
            .values()
            .find(|static_ip| static_ip.attached_to.as_deref() == Some(instance_name))
    }

    /// Store a newly allocated static IP.
    pub fn insert_static_ip(&mut self, static_ip: StoredStaticIp) {
        self.static_ips.insert(static_ip.name.clone(), static_ip);
    }

    /// Remove the static IP `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Giving up the reservation gives up the attachment with it, for the reason
    /// spelled out on [`Self::remove_instance`]: an attachment names "the
    /// resources to which" a resource "is attached" (the modelled
    /// `StaticIp$attachedTo`), and an address this account no longer holds is
    /// not one of them — so the removal and the release are one step and no read
    /// can observe an instance holding an address the store does not have.
    ///
    /// The instance that held it keeps public addressing, drawn afresh from this
    /// store's one address allocator — see
    /// [`Self::redraw_public_address_without_static_ip`], which is where that
    /// fallback lives for every call that releases an attachment.
    pub fn remove_static_ip(&mut self, name: &str) -> Option<StoredStaticIp> {
        let static_ip = self.static_ips.remove(name)?;
        if let Some(instance_name) = static_ip.attached_to.clone() {
            self.redraw_public_address_without_static_ip(&instance_name);
        }
        Some(static_ip)
    }

    /// Release the attachment the static IP `name` holds, answering the instance
    /// it was attached to. `None` when the account holds no static IP of that
    /// name, or that static IP is attached to nothing — in which case nothing
    /// was changed.
    ///
    /// Only the ATTACHMENT is released: the address stays reserved for the
    /// account, so the record keeps every other member it has and a later read
    /// still reports the reservation — releasing the reservation is
    /// [`Self::remove_static_ip`]'s business. The instance it came off falls
    /// back to an ordinary public address, through the same
    /// [`Self::redraw_public_address_without_static_ip`] a release puts it on,
    /// so the two calls leave an instance in the same state.
    pub fn detach_static_ip(&mut self, name: &str) -> Option<String> {
        let instance_name = self.static_ips.get_mut(name)?.attached_to.take()?;
        self.redraw_public_address_without_static_ip(&instance_name);
        Some(instance_name)
    }

    /// Attach the static IP `name` to the instance `instance_name`, answering the
    /// reserved address the instance is reachable at from now on. `None` when the
    /// account holds no static IP of that name or no such instance — in which
    /// case nothing was changed.
    ///
    /// Both halves of the attachment are this ONE step, exactly as releasing it
    /// is [`Self::detach_static_ip`]'s: the attachment itself lives on the static
    /// IP record (the modelled `StaticIp$attachedTo` names the resource "to which
    /// the static IP is attached"), and the instance it names is reachable at the
    /// reserved address, because an attached static IP is the only thing that
    /// pins an instance's public IPv4 address — so `Instance$isStaticIp`, "a
    /// Boolean value indicating whether this instance has a static IP assigned to
    /// it", is true for as long as the attachment stands. Nothing is drawn from
    /// the address allocator here: the address the instance takes on is the one
    /// the reservation already holds, which is what makes it static.
    ///
    /// Attaching the static IP an instance already holds writes exactly what is
    /// already stored, so the call is idempotent; whether an attachment may be
    /// made at all — the static IP is free, the instance holds none, the instance
    /// has public IPv4 addressing — is the calling operation's question, decided
    /// before this is reached.
    pub fn attach_static_ip(&mut self, name: &str, instance_name: &str) -> Option<String> {
        let ip_address = self.static_ips.get(name)?.ip_address.clone();
        // The instance is written FIRST, so a name this store holds no instance
        // under leaves the reservation exactly as it was rather than pointing it
        // at an instance that is not there.
        let instance = self.instance_mut(instance_name)?;
        instance.is_static_ip = true;
        instance.public_ip_address = Some(ip_address.clone());
        self.static_ips.get_mut(name)?.attached_to = Some(instance_name.to_string());
        Some(ip_address)
    }

    /// Put the instance `instance_name` back on the public addressing it holds
    /// while no static IP is attached to it, drawn afresh from this store's one
    /// address allocator.
    ///
    /// An attached static IP is the ONLY thing that pins an instance's public
    /// IPv4 address ("To use the same IP address after stopping and starting an
    /// instance, create a static IP address and attach it to the instance" — the
    /// modelled `StartInstance` documentation), so an instance with none holds an
    /// ordinary address the platform assigned it, and `Instance$isStaticIp` — "a
    /// Boolean value indicating whether this instance has a static IP assigned to
    /// it" — is false again. The private address and the IPv6 addresses are left
    /// where they are: a static IP reserves one public IPv4 address (the modelled
    /// `StaticIp$ipAddress`), so it pinned neither.
    fn redraw_public_address_without_static_ip(&mut self, instance_name: &str) {
        // Which addresses the instance has at all is its own `ipAddressType`,
        // exactly as it is on a start; an instance this store does not hold has
        // none to redraw.
        let Some(ip_address_type) = self
            .instance(instance_name)
            .map(|instance| instance.ip_address_type.clone())
        else {
            return;
        };
        // ONE draw from the one allocator every address in this store comes out
        // of, so the address the instance falls back to is one no resource in
        // this process has held. The private address and the IPv6 addresses it
        // also yields are dropped — the instance keeps its own — which spends
        // the offset rather than wasting it.
        let (_private, public, _ipv6) = self.fresh_addresses(&ip_address_type);
        if let Some(instance) = self.instance_mut(instance_name) {
            instance.is_static_ip = false;
            instance.public_ip_address = public;
        }
    }

    /// Store a newly created disk snapshot.
    pub fn insert_disk_snapshot(&mut self, snapshot: StoredDiskSnapshot) {
        self.disk_snapshots.insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the disk snapshot `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Nothing else in the store points AT a snapshot: a disk created from one
    /// copies what it needs out of the snapshot record as it is created, and an
    /// export record captures its source the same way — which is what lets the
    /// service say that "regardless of which prior snapshots have been deleted,
    /// all active snapshots will have access to all the information needed to
    /// restore the disk". So removing the record is the whole deletion, and the
    /// clock-owed transition it was carrying goes with it.
    pub fn remove_disk_snapshot(&mut self, name: &str) -> Option<StoredDiskSnapshot> {
        self.disk_snapshots.remove(name)
    }

    /// The instance snapshot `name` names, for a reporting operation.
    pub fn instance_snapshot(&self, name: &str) -> Option<&StoredInstanceSnapshot> {
        self.instance_snapshots.get(name)
    }

    /// Whether this account already holds a snapshot called `name`, of EITHER
    /// kind of snapshot the service's snapshot operations address.
    ///
    /// A snapshot name addresses ONE snapshot: every operation that takes a
    /// snapshot name takes it unqualified — `ExportSnapshot`'s
    /// `sourceSnapshotName`, `CopySnapshot`'s `sourceSnapshotName`, the
    /// `resourceName` the tag plane resolves ([`Self::with_tagged_record`]) —
    /// and resolves it by asking the disk snapshots and then the instance
    /// snapshots, so a name both maps held would leave the second record
    /// unreachable through every one of them. This is the one place the
    /// question "is this snapshot name taken?" is asked, so each operation that
    /// MINTS a snapshot name refuses exactly the names that resolution already
    /// answers for.
    pub fn holds_snapshot_named(&self, name: &str) -> bool {
        self.disk_snapshots.contains_key(name) || self.instance_snapshots.contains_key(name)
    }

    /// Store a newly created instance snapshot.
    pub fn insert_instance_snapshot(&mut self, snapshot: StoredInstanceSnapshot) {
        self.instance_snapshots
            .insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the instance snapshot `name` names, answering it. `None` when
    /// nothing matched.
    ///
    /// Nothing else in the store points AT an instance snapshot: the snapshot
    /// itself holds the point-in-time copy of its source — the blueprint, the
    /// bundle, the system volume size and the block storage disks the instance
    /// had when it was taken — and an instance created from it, or an export of
    /// it, copies what it needs out of that record as it is created. So removing
    /// the record is the whole deletion: the clock-owed transition it was still
    /// carrying goes with it, and the instance it was taken of, together with
    /// every other snapshot of that instance, is untouched.
    pub fn remove_instance_snapshot(&mut self, name: &str) -> Option<StoredInstanceSnapshot> {
        self.instance_snapshots.remove(name)
    }

    /// The export snapshot record `name` names, for a reporting operation.
    pub fn export_snapshot_record(&self, name: &str) -> Option<&StoredExportSnapshotRecord> {
        self.export_snapshot_records.get(name)
    }

    /// Store a newly created export snapshot record.
    pub fn insert_export_snapshot_record(&mut self, record: StoredExportSnapshotRecord) {
        self.export_snapshot_records
            .insert(record.name.clone(), record);
    }

    /// Store a newly created CloudFormation stack record.
    pub fn insert_cloud_formation_stack_record(
        &mut self,
        record: StoredCloudFormationStackRecord,
    ) {
        self.cloud_formation_stack_records
            .insert(record.name.clone(), record);
    }

    /// The CloudFormation stack record built from the export snapshot record
    /// `export_snapshot_record_name` whose stack has not finished yet, if the
    /// account holds one.
    ///
    /// The walk is bounded by [`MAX_CLOUD_FORMATION_STACK_RECORDS`].
    pub fn cloud_formation_stack_record_in_progress_for(
        &self,
        export_snapshot_record_name: &str,
    ) -> Option<&StoredCloudFormationStackRecord> {
        self.cloud_formation_stack_records.values().find(|record| {
            record.state == RECORD_STATE_STARTED
                && record.source_export_snapshot_record_name() == Some(export_snapshot_record_name)
        })
    }

    /// Remove the instance `name` names, answering the record and every
    /// reference its removal released. `None` when nothing matched.
    ///
    /// Removing a resource also releases every live reference another stored
    /// record holds to it — an attachment, an access grant or an alarm's
    /// monitored resource: each of those names a resource this account holds
    /// (the modelled `Disk$attachedTo` and `StaticIp$attachedTo` name "the
    /// resources to which" one resource "is attached";
    /// `Bucket$resourcesReceivingAccess` describes "Lightsail instances that
    /// have access to the bucket"; an alarm's `monitoredResourceName` resolves
    /// through [`Self::monitored_resource`]), and a resource this account no
    /// longer holds is not one of them — so the removal and the release are
    /// one step, and no read can observe a reference to a record that is gone.
    /// `delete_add_ons` says the delete was asked to delete the instance's
    /// add-ons with it, which is what decides the fate of the copies the
    /// instance's add-on took — see
    /// [`Self::settle_auto_snapshots_of_removed`].
    pub fn remove_instance(
        &mut self,
        name: &str,
        delete_add_ons: bool,
    ) -> Option<RemovedInstance> {
        let instance = self.instances.remove(name)?;
        self.settle_auto_snapshots_of_removed(
            RESOURCE_TYPE_INSTANCE,
            name,
            &instance.location,
            delete_add_ons,
        );
        let released_attachments = self.release_attachments_to_instance(name);
        Some(RemovedInstance {
            instance,
            released_attachments,
        })
    }

    /// Release every reference the store holds to the instance `name`,
    /// answering one entry per release so the caller can log it.
    ///
    /// Five kinds of record can hold one. Four of them keep the reference in a
    /// member they can also hold EMPTY, so the release is a rewrite of that
    /// member: a block storage disk (`attachedTo` plus the device `path` it is
    /// mounted at), a static IP (`attachedTo`), a load balancer (the instance
    /// registered with it), and a bucket (the instance it reports in
    /// `resourcesReceivingAccess`, which "describe Lightsail instances that have
    /// access to the bucket"). Each returns to the state it has while it holds
    /// nothing — for a disk that is `available`, the modelled `DiskState` value
    /// for a disk that "exists and is attached to nothing" next to `in-use`; for
    /// a bucket, one fewer resource receiving access.
    ///
    /// The fifth is an ALARM, whose reference is required rather than optional,
    /// so it is removed with the resource instead — see
    /// [`Self::release_alarms_monitoring`].
    ///
    /// A DISTRIBUTION's origin is not among them: a distribution cannot be left
    /// holding none, so the reference is not released here but stands in the
    /// delete's way instead — see
    /// [`Self::in_use_as_distribution_origin`].
    ///
    /// Every walk is bounded: the disks by [`MAX_DISKS`], the static IPs by
    /// [`MAX_STATIC_IPS`], the load balancers by
    /// [`MAX_LOAD_BALANCERS_PER_REGION`] per region, the buckets by
    /// [`MAX_BUCKETS`] and the alarms by [`MAX_ALARMS`].
    fn release_attachments_to_instance(&mut self, name: &str) -> Vec<ReleasedAttachment> {
        let mut released = Vec::new();
        for disk in self.disks.values_mut() {
            if disk.attached_to.as_deref() != Some(name) {
                continue;
            }
            disk.attached_to = None;
            disk.path = None;
            disk.state = DISK_STATE_AVAILABLE;
            // A detachment the clock still owed this disk has nothing left to
            // do: the attachment it was going to release is already released,
            // and leaving the `due` in place would only re-apply the same
            // settle. A `due` of any OTHER state is left alone — it is not this
            // attachment's.
            if disk.due.is_some_and(|due| due.state == DISK_STATE_AVAILABLE) {
                disk.due = None;
            }
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_DISK,
                resource_type: RESOURCE_TYPE_DISK,
                resource_name: disk.name.clone(),
                location: disk.location.clone(),
            });
        }
        for static_ip in self.static_ips.values_mut() {
            if static_ip.attached_to.as_deref() != Some(name) {
                continue;
            }
            // The address stays ALLOCATED to the account — nothing released the
            // reservation, only the attachment — so the record keeps every
            // other member it has. The instance is not put back on platform
            // addressing here (as `detach_static_ip` does through
            // [`Self::redraw_public_address_without_static_ip`]): this walk runs
            // while the instance itself is being removed, so there is no record
            // left to hold an address.
            static_ip.attached_to = None;
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_STATIC_IP,
                resource_type: RESOURCE_TYPE_STATIC_IP,
                resource_name: static_ip.name.clone(),
                location: static_ip.location.clone(),
            });
        }
        for balancer in self.load_balancers.values_mut() {
            if !balancer
                .attached_instances
                .iter()
                .any(|registration| registration.name == name)
            {
                continue;
            }
            balancer
                .attached_instances
                .retain(|registration| registration.name != name);
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_INSTANCES_FROM_LOAD_BALANCER,
                resource_type: RESOURCE_TYPE_LOAD_BALANCER,
                resource_name: balancer.name.clone(),
                location: balancer.location.clone(),
            });
        }
        for bucket in self.buckets.values_mut() {
            // The grant lives on the BUCKET's record and is keyed on the
            // instance's name alone, so taking it back is the same write
            // `SetResourceAccessForBucket` makes with `deny` — which is why the
            // release is logged under that modelled `OperationType`, against the
            // bucket whose record changed.
            if !bucket.set_resource_access(name, RESOURCE_TYPE_INSTANCE, false) {
                continue;
            }
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_SET_RESOURCE_ACCESS_FOR_BUCKET,
                resource_type: RESOURCE_TYPE_BUCKET,
                resource_name: bucket.name.clone(),
                location: bucket.location.clone(),
            });
        }
        released.extend(self.release_alarms_monitoring(RESOURCE_TYPE_INSTANCE, name));
        released
    }

    /// Remove every alarm the store holds that monitors the resource `name` of
    /// kind `resource_type`, answering one entry per removal so the caller can
    /// log it.
    ///
    /// An alarm NAMES the resource it watches: `monitoredResourceName` is a
    /// required member of the put that creates one, the record keeps what that
    /// name resolved to ([`Self::monitored_resource`], which resolves only a
    /// resource this account holds) and every read reports it as
    /// `Alarm$monitoredResourceInfo`, "an object that lists information about
    /// the resource monitored by the alarm". That reference cannot be left
    /// empty the way a disk's `attachedTo` can, so an alarm whose monitored
    /// resource is removed goes with it rather than being rewritten — which is
    /// also the only outcome the put itself admits, since it refuses to create
    /// an alarm on a name that resolves to nothing. Each removal is logged as
    /// that alarm's own `DeleteAlarm`, against the alarm whose record went, so
    /// the per-metric alarm budget [`Self::alarms_on_metric`] counts and the
    /// `GetAlarms` listing hold only alarms watching resources the account
    /// still has.
    ///
    /// The walk is bounded by [`MAX_ALARMS`].
    fn release_alarms_monitoring(
        &mut self,
        resource_type: &'static str,
        name: &str,
    ) -> Vec<ReleasedAttachment> {
        let monitoring: Vec<String> = self
            .alarms
            .values()
            .filter(|alarm| {
                alarm.monitored_resource.resource_type == resource_type
                    && alarm.monitored_resource.name == name
            })
            .map(|alarm| alarm.name.clone())
            .collect();
        let mut released = Vec::with_capacity(monitoring.len());
        for alarm_name in monitoring {
            // The names were just read out of this map under the same guard, so
            // every one of them is still there.
            if let Some(alarm) = self.alarms.remove(&alarm_name) {
                released.push(ReleasedAttachment {
                    operation_type: OPERATION_TYPE_DELETE_ALARM,
                    resource_type: RESOURCE_TYPE_ALARM,
                    resource_name: alarm.name,
                    location: alarm.location,
                });
            }
        }
        released
    }

    /// The container service `name` names in `region`, for a reporting
    /// operation.
    pub fn container_service(&self, region: &str, name: &str) -> Option<&StoredContainerService> {
        self.container_services
            .get(&(region.to_string(), name.to_string()))
    }

    /// The container service `name` names in `region`, for an operation that
    /// mutates it.
    pub fn container_service_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut StoredContainerService> {
        self.container_services
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Every container service in `region`, in the store's stable order.
    pub fn container_services_in(&self, region: &str) -> Vec<&StoredContainerService> {
        self.container_services
            .iter()
            .filter(|((service_region, _), _)| service_region == region)
            .map(|(_, service)| service)
            .collect()
    }

    /// Store a newly created container service.
    pub fn insert_container_service(&mut self, service: StoredContainerService) {
        self.container_services.insert(
            (service.location.region_name.clone(), service.name.clone()),
            service,
        );
    }

    /// Remove the container service `name` names in `region`, answering the
    /// record that was stored under it, or `None` when this account holds no
    /// container service of that name there.
    ///
    /// Everything a container service carries lives ON this record — its power
    /// and scale, its tags, the groups of public domain names it served, the
    /// access its image puller role held, its deployment history and the
    /// container images registered to it — so removing the record is what
    /// removes them all, and no later read can report a deployment or a
    /// registered image of a service this account no longer holds. The name is
    /// free again for a service that inherits nothing from this one, exactly as
    /// it is for every other removal here. A CERTIFICATE a group of public
    /// domain names was keyed by is NOT removed with it: a certificate is a
    /// resource of its own, so it is released by this removal rather than
    /// deleted by it. The operation log is append-only, so the entries this
    /// service's earlier calls wrote stay readable.
    pub fn remove_container_service(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<StoredContainerService> {
        self.container_services
            .remove(&(region.to_string(), name.to_string()))
    }

    /// The bucket `name` names in `region`, for a reporting operation.
    pub fn bucket(&self, region: &str, name: &str) -> Option<&StoredBucket> {
        self.buckets.get(&(region.to_string(), name.to_string()))
    }

    /// The CDN distribution `name` names, for a reporting operation.
    pub fn distribution(&self, name: &str) -> Option<&StoredDistribution> {
        self.distributions.get(name)
    }

    /// The CDN distribution whose ARN is `arn`, for a call that addresses a
    /// resource BY ARN rather than by name — the way a subdomain names the
    /// resource it is to be associated with
    /// (`UpdateSubdomainAssociationRequest$targetResourceArn`).
    ///
    /// The walk is over the account's distributions, bounded by
    /// [`MAX_DISTRIBUTIONS`], and an ARN is unique to one record because this
    /// store mints it from an id of its own.
    pub fn distribution_with_arn(&self, arn: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| distribution.arn == arn)
    }

    /// The most recent cache reset of ANY distribution this account holds, or
    /// `None` when no distribution's cache has ever been reset.
    ///
    /// What `GetDistributionLatestCacheReset` reports when the request names no
    /// distribution: its `distributionName` documentation says that "when
    /// omitted, the response includes the latest cache reset timestamp of all
    /// your distributions", and the result carries ONE timestamp and ONE status
    /// — so the answer is the latest of them. Ties go to the distribution that
    /// sorts last in the store's own total order, which is stable across reads.
    pub fn latest_cache_reset(&self) -> Option<StoredCacheReset> {
        self.distributions
            .values()
            .filter_map(|distribution| distribution.latest_cache_reset)
            .max_by_key(|reset| reset.created_at_ms)
    }

    /// Store a newly created CDN distribution.
    pub fn insert_distribution(&mut self, distribution: StoredDistribution) {
        self.distributions
            .insert(distribution.name.clone(), distribution);
    }

    /// Remove the CDN distribution `name` names, answering the record that was
    /// stored under it, or `None` when this account holds no distribution of
    /// that name.
    ///
    /// Everything a distribution carries lives ON this record — its origin, its
    /// cache behaviours, its tags, the certificate it names and the last reset
    /// of its cached content — so removing the record is what removes them all.
    /// A certificate itself is NOT removed with it: a certificate is a resource
    /// of its own, and the number of resources it is attached to is counted from
    /// the records that name it ([`Self::certificate_in_use_resource_count`]),
    /// so it is released by this removal rather than deleted by it. The
    /// operation log is append-only, so the entries a deleted distribution's
    /// earlier calls wrote stay readable.
    pub fn remove_distribution(&mut self, name: &str) -> Option<StoredDistribution> {
        self.distributions.remove(name)
    }

    /// What the origin name `name` addresses in `region`, or `None` when this
    /// account holds no resource of that name there.
    ///
    /// An origin "can be a Lightsail instance, bucket, container service, or
    /// load balancer" (`InputOrigin`), and those are looked up in exactly that
    /// order — a name can only be taken by one of them, since each kind refuses
    /// a create whose name is already its own. An instance is held by name
    /// alone, so its own `location` is what decides whether it is in `region`.
    pub fn resolve_origin(&self, region: &str, name: &str) -> Option<ResolvedOrigin> {
        if self
            .instance(name)
            .is_some_and(|instance| instance.location.region_name == region)
        {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_INSTANCE,
                public_dns: None,
            });
        }
        if self.bucket(region, name).is_some() {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_BUCKET,
                public_dns: None,
            });
        }
        if self.container_service(region, name).is_some() {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_CONTAINER_SERVICE,
                public_dns: None,
            });
        }
        if let Some(balancer) = self.load_balancer(region, name) {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_LOAD_BALANCER,
                public_dns: Some(balancer.dns_name.clone()),
            });
        }
        None
    }

    /// How many of this account's distributions pull their content from the
    /// resource `name` names in `region`, which this store resolved to
    /// `resource_type`.
    ///
    /// Counted from the distribution records that HOLD the origin rather than
    /// from a counter kept on the origin resource, so the number cannot drift
    /// from the distributions that actually name it — the same way
    /// [`Self::certificate_in_use_resource_count`] counts the attachments of a
    /// certificate. A distribution's origin is held as the name, the Region and
    /// the resource type the create resolved ([`StoredDistribution::origin`]),
    /// so a resource is the origin of a distribution exactly when all three
    /// match. The walk is bounded by [`MAX_DISTRIBUTIONS`].
    pub fn distributions_with_origin(&self, region: &str, name: &str, resource_type: &str) -> usize {
        self.distributions
            .values()
            .filter(|distribution| {
                distribution.origin.name.as_deref() == Some(name)
                    && distribution.origin.region_name.as_deref() == Some(region)
                    && distribution.origin.resource_type.as_deref() == Some(resource_type)
            })
            .count()
    }

    /// Why a delete of the resource `name` names in `region`, which this store
    /// holds as `resource_type`, cannot be served while distributions of this
    /// account still pull their content from it — or `None` when none does, so
    /// nothing stands in the delete's way.
    ///
    /// A distribution pulls its content from ONE origin resource of this
    /// account, and both write paths refuse an origin that addresses nothing
    /// ([`Self::resolve_origin`]), so a resource a distribution names is in use
    /// by it and a delete that would leave that distribution pulling from a
    /// resource the account no longer holds is refused. The condition is the one
    /// the modelled `DeleteBucketRequest$forceDelete` documents ("You must force
    /// delete the bucket if it has one of the following conditions: The bucket
    /// is the origin of a distribution"), counted from the distribution records
    /// that name the resource ([`Self::distributions_with_origin`]) — so it is
    /// the same condition whichever of the four kinds of origin resource the
    /// delete names, and a delete that offers a force (`DeleteBucket` is the
    /// only one that does) is what overrides it.
    pub fn in_use_as_distribution_origin(
        &self,
        region: &str,
        name: &str,
        resource_type: &str,
    ) -> Option<String> {
        let condition = origin_of_distributions_condition(self.distributions_with_origin(
            region,
            name,
            resource_type,
        ))?;
        Some(format!(
            "{resource_type} is in use: {name} ({condition}); delete the distribution, or \
             point it at another origin, before deleting the {resource_type}"
        ))
    }

    /// The managed relational database `name` names, for a reporting operation.
    pub fn relational_database(&self, name: &str) -> Option<&StoredRelationalDatabase> {
        self.relational_databases.get(name)
    }

    /// Store a newly created managed relational database, together with the
    /// data plane opened for it: the listener the plane holds is what keeps its
    /// published endpoint answering, so it is admitted in the same step the
    /// record is and no read can see a database whose endpoint nothing serves.
    pub fn insert_relational_database(
        &mut self,
        database: StoredRelationalDatabase,
        plane: RelationalDatabasePlane,
    ) {
        self.relational_database_planes
            .insert(database.name.clone(), plane);
        self.relational_databases
            .insert(database.name.clone(), database);
    }

    /// The open data plane of the managed relational database `name`, for a
    /// caller that reads what its engine holds.
    pub fn relational_database_plane(&self, name: &str) -> Option<&RelationalDatabasePlane> {
        self.relational_database_planes.get(name)
    }

    /// The managed relational database `name` names, for an operation the spec
    /// says mutates it — setting a runtime parameter of its parameter group.
    ///
    /// `name` is this record's store key, so it may not be reassigned through
    /// this handle; no operation in this slice renames a database.
    pub fn relational_database_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut StoredRelationalDatabase> {
        self.relational_databases.get_mut(name)
    }

    /// Remove the managed relational database `name` names, answering the
    /// record it removed together with every reference the removal released.
    /// `None` when nothing matched.
    ///
    /// No attachment names a database — a database attaches to no instance,
    /// disk or load balancer — so the one kind of record that can hold a live
    /// reference to it is an ALARM, which a database can be the monitored
    /// resource of: see [`Self::release_alarms_monitoring`] for why that
    /// reference is removed with the database rather than rewritten. A SNAPSHOT
    /// of it is deliberately left where it is: a snapshot is a point-in-time
    /// copy that carries its own copy of everything it reports about its source
    /// ([`StoredRelationalDatabaseSnapshot`]), which is what lets the final
    /// snapshot a delete takes outlive the database it was taken of.
    pub fn remove_relational_database(
        &mut self,
        name: &str,
    ) -> Option<RemovedRelationalDatabase> {
        let database = self.relational_databases.remove(name)?;
        // The plane goes with the record: dropping it closes the port the
        // database published and severs every session on it, which is what
        // deleting a database does to the clients connected to it.
        self.relational_database_planes.remove(name);
        let released_references =
            self.release_alarms_monitoring(RESOURCE_TYPE_RELATIONAL_DATABASE, name);
        Some(RemovedRelationalDatabase {
            database,
            released_references,
        })
    }

    /// The managed relational database snapshot `name` names, for a reporting
    /// operation.
    pub fn relational_database_snapshot(
        &self,
        name: &str,
    ) -> Option<&StoredRelationalDatabaseSnapshot> {
        self.relational_database_snapshots.get(name)
    }

    /// The message a request that would take this store past
    /// [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`] is refused with, or `None` when
    /// there is room for one more snapshot.
    ///
    /// The ONE place that bound is checked, so every operation that creates a
    /// managed relational database snapshot — `CreateRelationalDatabaseSnapshot`,
    /// `StopRelationalDatabase`'s pre-stop snapshot and
    /// `DeleteRelationalDatabase`'s final one — refuses at the same count with
    /// the same wording. Each lifts it into its own modelled
    /// `InvalidInputException`.
    pub fn relational_database_snapshot_capacity_refusal(&self) -> Option<String> {
        (self.relational_database_snapshots.len() >= MAX_RELATIONAL_DATABASE_SNAPSHOTS).then(|| {
            format!(
                "creating a relational database snapshot would take this account past \
                 the {MAX_RELATIONAL_DATABASE_SNAPSHOTS} this emulator holds"
            )
        })
    }

    /// Store a newly created managed relational database snapshot.
    pub fn insert_relational_database_snapshot(
        &mut self,
        snapshot: StoredRelationalDatabaseSnapshot,
    ) {
        self.relational_database_snapshots
            .insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the managed relational database snapshot `name` names, answering
    /// it. `None` when nothing matched.
    ///
    /// Nothing else in the store points AT a database snapshot: a database
    /// restored from one copies everything it needs out of the snapshot record
    /// as it is created ([`Self::snapshot_relational_database`] captures the
    /// source the same way), so each snapshot is an independent point-in-time
    /// copy and removing its record is the whole deletion — the clock-owed
    /// transition it was still carrying goes with it, and the database it was
    /// taken of, and every other snapshot of that database, are untouched.
    pub fn remove_relational_database_snapshot(
        &mut self,
        name: &str,
    ) -> Option<StoredRelationalDatabaseSnapshot> {
        self.relational_database_snapshots.remove(name)
    }

    /// Take a point-in-time copy of `source` under the snapshot name `name`,
    /// store it and answer the record that was stored.
    ///
    /// The ONE place a database snapshot is built, whichever call asks for it —
    /// `CreateRelationalDatabaseSnapshot` on its own, the pre-stop snapshot a
    /// `StopRelationalDatabase` takes before it shuts the engine down, or the
    /// final snapshot a `DeleteRelationalDatabase` takes before it removes the
    /// database — so all of them report the same copy of the same source.
    ///
    /// Everything the snapshot says about its SOURCE — and the master database
    /// name and credentials a later `CreateRelationalDatabaseFromSnapshot`
    /// restores it with — is read out of `source` as it stands now and never
    /// followed afterwards, which is what lets a snapshot outlive the database it
    /// was taken of. The copy is not restorable yet: the
    /// clock owes it [`RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE`], named by
    /// the `due` the answered record carries, so the caller can log the operation
    /// that settle completes.
    ///
    /// PRECONDITION: `name` is a snapshot name this account does not already hold
    /// and the store is below [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`] — a caller
    /// that skipped either check would replace a record a restore may be using,
    /// or grow the store past its bound.
    pub fn snapshot_relational_database(
        &mut self,
        source: &StoredRelationalDatabase,
        name: String,
        tags: Vec<Tag>,
        now_ms: i64,
    ) -> StoredRelationalDatabaseSnapshot {
        let id = self.mint_uuid();
        let snapshot = StoredRelationalDatabaseSnapshot {
            name,
            arn: relational_database_snapshot_arn(&source.location.region_name, &id),
            support_code: support_code(&id),
            created_at_ms: now_ms,
            location: source.location.clone(),
            tags,
            engine: source.blueprint.engine.to_string(),
            engine_version: source.blueprint.engine_version.to_string(),
            // The snapshotted disk is the database bundle's own storage — the
            // size `GetRelationalDatabase` reports as its hardware disk size.
            size_in_gb: source.bundle.disk_size_in_gb,
            // A snapshot is a copy of the DATABASE, so the database name and the
            // master credentials it was created with are captured here too: a
            // later `CreateRelationalDatabaseFromSnapshot` restores a database
            // reached with them, and it must still do so after the source has
            // been deleted. No member of `RelationalDatabaseSnapshot` reports
            // them, exactly as no member of `RelationalDatabase` reports a
            // password.
            master_database_name: source.master_database_name.clone(),
            master_username: source.master_username.clone(),
            master_user_password: Some(source.master_user_password.password.clone()),
            state: RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING,
            due: Some(DueNamedState::after(
                now_ms,
                RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE,
            )),
            from_relational_database_name: source.name.clone(),
            from_relational_database_arn: source.arn.clone(),
            from_relational_database_bundle_id: source.bundle.bundle_id.to_string(),
            from_relational_database_blueprint_id: source.blueprint.blueprint_id(),
        };
        self.insert_relational_database_snapshot(snapshot.clone());
        // Taking a snapshot is something the service does TO the source
        // database, so the source's own event log carries it — and a second
        // entry when the copy can be restored from (see [`Self::advance_clock`]).
        if let Some(database) = self
            .relational_databases
            .get_mut(&snapshot.from_relational_database_name)
        {
            database.record_event(
                now_ms,
                format!(
                    "{RELATIONAL_DATABASE_EVENT_BACKUP_STARTED}: {}",
                    snapshot.name
                ),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
            );
        }
        snapshot
    }

    /// The alarm `name` names, for a reporting operation.
    pub fn alarm(&self, name: &str) -> Option<&StoredAlarm> {
        self.alarms.get(name)
    }

    /// The alarm `name` names, for an operation the spec says mutates it —
    /// putting it into the state a test asks for.
    ///
    /// The name is this record's store key, so it may not be reassigned through
    /// this handle; no operation in this slice renames an alarm.
    pub fn alarm_mut(&mut self, name: &str) -> Option<&mut StoredAlarm> {
        self.alarms.get_mut(name)
    }

    /// The contact method `protocol` addresses in `region`, for an operation the
    /// spec says mutates it — sending it a fresh verification request.
    ///
    /// The `(region, protocol)` pair is this record's store key, so neither may
    /// be reassigned through this handle; a contact method never changes protocol
    /// or Region.
    pub fn contact_method_mut(
        &mut self,
        region: &str,
        protocol: &str,
    ) -> Option<&mut StoredContactMethod> {
        self.contact_methods
            .get_mut(&(region.to_string(), protocol.to_string()))
    }

    /// Store the alarm `PutAlarm` just created or re-configured.
    ///
    /// An alarm name addresses one alarm, so putting a name that is already taken
    /// REPLACES that record: "specify the name of an existing alarm to update, and
    /// overwrite the previous configuration of the alarm".
    pub fn insert_alarm(&mut self, alarm: StoredAlarm) {
        self.alarms.insert(alarm.name.clone(), alarm);
    }

    /// Remove the alarm `name` names, answering it. `None` when nothing matched.
    pub fn remove_alarm(&mut self, name: &str) -> Option<StoredAlarm> {
        self.alarms.remove(name)
    }

    /// The SSH key pair `name` names, for a reporting operation.
    pub fn key_pair(&self, name: &str) -> Option<&StoredKeyPair> {
        self.key_pairs.get(name)
    }

    /// Store a newly created or imported SSH key pair.
    pub fn insert_key_pair(&mut self, key_pair: StoredKeyPair) {
        self.key_pairs.insert(key_pair.name.clone(), key_pair);
    }

    /// Remove the SSH key pair `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Removing the record is what removes the public key from the service,
    /// which is the whole of what the modelled `DeleteKeyPair` deletes: an
    /// instance launched with that pair keeps the `sshKeyName` its own record
    /// carries, because the pair is named there rather than referenced.
    pub fn remove_key_pair(&mut self, name: &str) -> Option<StoredKeyPair> {
        let removed = self.key_pairs.remove(name)?;
        // The held material belongs to the record that is going, so it goes
        // with it: the account holds no default key pair after this, and the
        // next download mints a fresh one rather than handing out the halves of
        // a pair the service no longer holds.
        if removed.is_default() {
            self.default_key_pair_material = None;
        }
        Some(removed)
    }

    /// How many alarms other than `excluding` already watch `metric` on the
    /// resource `monitored_resource_name` names.
    ///
    /// `excluding` is the name `PutAlarm` is writing, because re-putting an
    /// existing alarm overwrites that one record rather than adding another.
    pub fn alarms_on_metric(
        &self,
        monitored_resource_name: &str,
        metric_name: &str,
        excluding: &str,
    ) -> usize {
        self.alarms
            .values()
            .filter(|alarm| {
                alarm.name != excluding
                    && alarm.monitored_resource.name == monitored_resource_name
                    && alarm.metric.name == metric_name
            })
            .count()
    }

    /// The contact method of `protocol` in `region`, for a reporting operation.
    pub fn contact_method(&self, region: &str, protocol: &str) -> Option<&StoredContactMethod> {
        self.contact_methods
            .get(&(region.to_string(), protocol.to_string()))
    }

    /// Every contact method of `region`, in the store's own total order.
    pub fn contact_methods_in(&self, region: &str) -> Vec<&StoredContactMethod> {
        self.contact_methods
            .iter()
            .filter(|((method_region, _), _)| method_region == region)
            .map(|(_, method)| method)
            .collect()
    }

    /// Store the contact method `CreateContactMethod` just created in `region`.
    ///
    /// One Region holds one contact method per protocol, and the model declares
    /// no operation that updates one, so creating under a protocol that already
    /// has a contact method REPLACES that record rather than adding a second.
    pub fn insert_contact_method(&mut self, region: &str, method: StoredContactMethod) {
        self.contact_methods
            .insert((region.to_string(), method.protocol.clone()), method);
    }

    /// Remove the contact method of `protocol` in `region`, answering it. `None`
    /// when nothing matched.
    pub fn remove_contact_method(
        &mut self,
        region: &str,
        protocol: &str,
    ) -> Option<StoredContactMethod> {
        self.contact_methods
            .remove(&(region.to_string(), protocol.to_string()))
    }

    /// The resource an alarm would monitor under `name` for a request made in
    /// `region`, or `None` when this account holds no alarmable resource of that
    /// name there.
    ///
    /// ONE name addresses one resource — the reasoning is spelled out on
    /// [`Self::with_tagged_record`]: `GetActiveNames` reports the account's
    /// resources as a single flat list of names with no resource type
    /// qualifying them, and each kind's own create refuses a name already
    /// taken — so at most one of the three below holds a given name and the
    /// order they are searched in is not a tie-break.
    ///
    /// Which resource types those are is [`MonitoredResource`]; each is searched
    /// the way that resource's OWN operations address it, the way
    /// [`Self::with_tagged_record`] searches them, so an alarm can watch exactly
    /// what those operations serve and no more. An instance and a relational
    /// database are held by name alone — `GetInstance` and
    /// `GetRelationalDatabase` serve them whichever Region the request is made
    /// from — and a load balancer is held under its Region together with its
    /// name, because a load balancer is a REGIONAL resource whose own
    /// documentation caps them "per AWS Region in your account", so the one a
    /// name addresses is the one in `region`.
    pub fn monitored_resource(&self, region: &str, name: &str) -> Option<MonitoredResource> {
        if let Some(instance) = self.instances.get(name) {
            return Some(MonitoredResource {
                name: instance.name.clone(),
                arn: instance.arn.clone(),
                resource_type: RESOURCE_TYPE_INSTANCE,
                location: StoredLocation::regional(&instance.location.region_name),
            });
        }
        if let Some(database) = self.relational_databases.get(name) {
            return Some(MonitoredResource {
                name: database.name.clone(),
                arn: database.arn.clone(),
                resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
                location: StoredLocation::regional(&database.location.region_name),
            });
        }
        let balancer = self.load_balancer(region, name)?;
        Some(MonitoredResource {
            name: balancer.name.clone(),
            arn: balancer.arn.clone(),
            resource_type: RESOURCE_TYPE_LOAD_BALANCER,
            location: StoredLocation::regional(&balancer.location.region_name),
        })
    }

    /// The bucket `name` names in `region`, for an operation the spec says
    /// mutates it — minting or withdrawing one of its access keys.
    ///
    /// `name` and the bucket's region are this record's store key, so neither may
    /// be reassigned through this handle; no operation in this slice renames or
    /// moves a bucket.
    pub fn bucket_mut(&mut self, region: &str, name: &str) -> Option<&mut StoredBucket> {
        self.buckets
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Store a newly created bucket.
    pub fn insert_bucket(&mut self, bucket: StoredBucket) {
        self.buckets.insert(
            (bucket.location.region_name.clone(), bucket.name.clone()),
            bucket,
        );
    }

    /// Remove the bucket `name` names in `region`, answering it together with
    /// every reference the removal released. `None` when nothing matched.
    ///
    /// Only the control-plane RECORD: the bucket's objects live in this emulator's
    /// one object store, which [`release_bucket_object_store`] is what releases
    /// (it takes that store's lock, so it is called with no guard of this store
    /// held).
    ///
    /// One kind of record can hold a live reference to a bucket that the removal
    /// itself has to release: ANOTHER bucket's access log configuration, whose
    /// `destination` "can be a Lightsail bucket in the same account, and in the
    /// same Amazon Web Services Region as the source bucket" — see
    /// [`Self::release_access_log_destinations`]. A DISTRIBUTION's origin is not
    /// among them, for the reason spelled out on
    /// [`Self::release_attachments_to_instance`]: a distribution cannot be left
    /// holding none, so that reference stands in the delete's way instead.
    pub fn remove_bucket(&mut self, region: &str, name: &str) -> Option<RemovedBucket> {
        let bucket = self
            .buckets
            .remove(&(region.to_string(), name.to_string()))?;
        // Released AFTER the record is gone, so the walk cannot reach the bucket
        // being removed: a bucket whose access logs were saved in itself holds
        // no reference once its own record is removed, and nothing is logged
        // against a record this account no longer holds.
        let released_references = self.release_access_log_destinations(region, name);
        Some(RemovedBucket {
            bucket,
            released_references,
        })
    }

    /// Release every access log configuration that saved its logs in the bucket
    /// `name` in `region`, answering one entry per release so the caller can log
    /// it.
    ///
    /// A bucket's access logs are saved in another bucket of the same account and
    /// the same Region, and the destination "is required when enabling the access
    /// log for a bucket, and should be omitted when disabling the access log", so
    /// a source bucket whose destination this account no longer holds has nowhere
    /// to write: its configuration returns to the state it has while it names no
    /// destination — the DISABLED form, exactly the write `UpdateBucket` makes
    /// when it is asked to turn the access log off. Only buckets of the SAME
    /// Region are walked, because that is the only Region a destination resolves
    /// in.
    ///
    /// The walk is bounded by [`MAX_BUCKETS`].
    fn release_access_log_destinations(
        &mut self,
        region: &str,
        name: &str,
    ) -> Vec<ReleasedAttachment> {
        let mut released = Vec::new();
        for ((bucket_region, _), bucket) in self.buckets.iter_mut() {
            if bucket_region != region
                || bucket.access_log_config.destination.as_deref() != Some(name)
            {
                continue;
            }
            bucket.access_log_config = BucketAccessLogConfig {
                enabled: false,
                destination: None,
                prefix: None,
            };
            // The configuration lives on the SOURCE bucket's record, so taking
            // the destination back is the same write `UpdateBucket` makes when
            // disabling the access log — which is why the release is logged
            // under that modelled `OperationType`, against the bucket whose
            // record changed.
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_UPDATE_BUCKET,
                resource_type: RESOURCE_TYPE_BUCKET,
                resource_name: bucket.name.clone(),
                location: bucket.location.clone(),
            });
        }
        released
    }

    /// The load balancer `name` names in `region`, for a reporting operation.
    pub fn load_balancer(&self, region: &str, name: &str) -> Option<&StoredLoadBalancer> {
        self.load_balancers
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every load balancer in `region`, in the store's stable order.
    pub fn load_balancers_in(&self, region: &str) -> Vec<&StoredLoadBalancer> {
        self.load_balancers
            .iter()
            .filter(|((balancer_region, _), _)| balancer_region == region)
            .map(|(_, balancer)| balancer)
            .collect()
    }

    /// The load balancer `name` names in `region`, for an operation the spec says
    /// mutates it — registering an instance with it, releasing one, or removing
    /// an SSL/TLS certificate it holds.
    ///
    /// The `(region, name)` pair is this record's store key, so neither may be
    /// reassigned through this handle; no operation in this slice renames a load
    /// balancer or moves one between Regions.
    pub fn load_balancer_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut StoredLoadBalancer> {
        self.load_balancers
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Remove the load balancer `name` names in `region`, answering it. `None`
    /// when nothing matched.
    ///
    /// Everything the load balancer holds goes with the record it lives on: its
    /// SSL/TLS certificates — "Deletes a Lightsail load balancer and all its
    /// associated SSL/TLS certificates" ([the modelled `DeleteLoadBalancer`
    /// documentation]) — its configuration options and the registrations of the
    /// instances that were attached to it, so no read can observe a certificate
    /// or a registration of a load balancer this account no longer holds, and
    /// there is no second map to keep in step. An INSTANCE that was registered
    /// is untouched: a registration lives on the load balancer, and nothing
    /// about the instance itself changes when the load balancer it served goes
    /// away.
    ///
    /// [the modelled `DeleteLoadBalancer` documentation]: crate::ops::delete_load_balancer
    pub fn remove_load_balancer(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<StoredLoadBalancer> {
        self.load_balancers
            .remove(&(region.to_string(), name.to_string()))
    }

    /// How many SSL/TLS certificates of load balancers this account holds,
    /// across every Region — what [`MAX_LOAD_BALANCER_TLS_CERTIFICATES`]
    /// bounds.
    ///
    /// Counted from the load balancers that hold them rather than from a
    /// counter, so the number cannot drift from the records that actually carry
    /// a certificate. The walk is bounded by
    /// [`MAX_LOAD_BALANCERS_PER_REGION`] load balancers per Region, each
    /// holding at most [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn load_balancer_tls_certificate_count(&self) -> usize {
        self.load_balancers
            .values()
            .map(|balancer| balancer.certificates.len())
            .sum()
    }

    /// Store a newly created load balancer.
    pub fn insert_load_balancer(&mut self, balancer: StoredLoadBalancer) {
        self.load_balancers.insert(
            (balancer.location.region_name.clone(), balancer.name.clone()),
            balancer,
        );
    }

    /// The certificate `name` names, for a reporting operation.
    pub fn certificate(&self, name: &str) -> Option<&StoredCertificate> {
        self.certificates.get(name)
    }

    /// The certificate `name` names, when it is one this account may secure
    /// traffic with — the ONE owner of that rule.
    ///
    /// A certificate a resource of this account references has to be a
    /// certificate this account holds, addressed by its exact name, and one the
    /// certificate authority has already issued: "Only certificates with a
    /// status of ISSUED can be attached to a distribution", and "You must create
    /// and validate an SSL/TLS certificate before you can use public domain
    /// names with your container service." Every operation that records a
    /// certificate reference — on a distribution, or as the key of a container
    /// service's public domain names — asks here FIRST, so a request naming a
    /// certificate this account does not hold, or one still being validated, is
    /// refused before anything is written and no record is left naming a
    /// certificate that secures nothing.
    pub fn issued_certificate(
        &self,
        name: &str,
    ) -> Result<&StoredCertificate, CertificateNotUsable> {
        let certificate = self
            .certificate(name)
            .ok_or(CertificateNotUsable::DoesNotExist)?;
        if !certificate.is_issued() {
            return Err(CertificateNotUsable::NotIssued {
                status: certificate.status,
            });
        }
        Ok(certificate)
    }

    /// Why the public domain names `names` carries are not ones a container
    /// service may serve, or `None` when every certificate they are keyed by is
    /// one this account holds and the certificate authority has issued.
    ///
    /// A container service's `publicDomainNames` is keyed by the NAME of the
    /// SSL/TLS certificate that secures the domains listed under it, so the
    /// certificate that key names answers the same exists-and-issued rule a
    /// distribution's certificate does — asked of the store's one owner of it,
    /// [`Self::issued_certificate`]. Whichever operation records those names
    /// asks here BEFORE its first write, so a refused request leaves no
    /// container service reporting a domain under a certificate that secures
    /// nothing. The map is ordered, so the certificate the refusal names is the
    /// same one on every run.
    pub fn unusable_public_domain_certificate(
        &self,
        names: &BTreeMap<String, Vec<String>>,
    ) -> Option<(String, CertificateNotUsable)> {
        names.keys().find_map(|certificate_name| {
            self.issued_certificate(certificate_name)
                .err()
                .map(|refusal| (certificate_name.clone(), refusal))
        })
    }

    /// Store a newly requested certificate.
    pub fn insert_certificate(&mut self, certificate: StoredCertificate) {
        self.certificates
            .insert(certificate.name.clone(), certificate);
    }

    /// Remove the SSL/TLS certificate `name` names, answering the record that
    /// was stored under it, or `None` when this account holds no certificate of
    /// that name.
    ///
    /// Everything a certificate carries lives ON this record — the domains it
    /// covers and their validation, its tags, its serial number and the window
    /// it is valid for — so removing the record is what removes them all, and
    /// no later read, listing or tag change can reach a certificate this
    /// account no longer holds. Nothing else in the store is rewritten: a
    /// distribution and a container service are each a resource of their own,
    /// and whether any resource still references this certificate is what the
    /// caller asks [`Self::certificate_in_use_resource_count`] BEFORE removing
    /// it, so no record is ever left naming a certificate that secures
    /// nothing.
    /// The operation log is append-only, so the entries this certificate's
    /// earlier calls wrote stay readable.
    pub fn remove_certificate(&mut self, name: &str) -> Option<StoredCertificate> {
        self.certificates.remove(name)
    }

    /// How many resources of this account the certificate `name` is attached to,
    /// which is what `Certificate$inUseResourceCount` reports — "the number of
    /// Lightsail resources that the certificate is attached to".
    ///
    /// A resource is attached to a certificate exactly when its own record
    /// references it, and this store records such a reference in the two places
    /// [`Self::issued_certificate`] guards on the way in: a DISTRIBUTION names
    /// it in `certificateName`, and a CONTAINER SERVICE keys a group of its
    /// `publicDomainNames` by it. Both are counted, so every resource whose
    /// record would be left naming a certificate that secures nothing is one
    /// this number reports — which is what the delete's in-use refusal rests on.
    ///
    /// Counted from the records that hold the reference rather than from a
    /// counter, so the number cannot drift from the resources that actually
    /// name the certificate. A certificate of this store is held by its name
    /// alone while a container service is held per Region, and the write side
    /// resolves a certificate by name alone too, so a service of ANY Region
    /// that keys its domains by this certificate is one of the resources it is
    /// attached to. The walk is bounded by [`MAX_DISTRIBUTIONS`] plus
    /// [`MAX_CONTAINER_SERVICES`].
    pub fn certificate_in_use_resource_count(&self, name: &str) -> i32 {
        let distributions = self
            .distributions
            .values()
            .filter(|distribution| distribution.certificate_name.as_deref() == Some(name))
            .count();
        let container_services = self
            .container_services
            .values()
            .filter(|service| service.public_domain_names.contains_key(name))
            .count();
        i32::try_from(distributions.saturating_add(container_services)).unwrap_or(i32::MAX)
    }

    /// The certificate `name` names as the modelled `CertificateSummary`, with
    /// the resources it is attached to counted.
    ///
    /// The one way a reporting operation renders a certificate: the reference
    /// lives on the record of the resource that holds it — a distribution, or a
    /// container service whose public domain names it keys — so assembling the
    /// view needs the whole store, not just the certificate.
    pub fn certificate_view(&self, name: &str, include_details: bool) -> Option<CertificateSummary> {
        let certificate = self.certificates.get(name)?;
        Some(self.certificate_record_view(name, certificate, include_details))
    }

    /// [`Self::certificate_view`] for a caller that already HOLDS the record —
    /// a page walk, which took the record from the store to decide it belongs
    /// on the page and so has nothing left to look up.
    ///
    /// This is where the assembly itself lives, and [`Self::certificate_view`]
    /// is the name lookup in front of it, so the single-record read and the
    /// listing render a certificate through ONE pairing of the record with the
    /// count only the whole store knows: a change to how a reported certificate
    /// is assembled is made here once and reaches both.
    pub fn certificate_record_view(
        &self,
        name: &str,
        certificate: &StoredCertificate,
        include_details: bool,
    ) -> CertificateSummary {
        certificate.view(
            self.certificate_in_use_resource_count(name),
            include_details,
        )
    }

    /// The distribution `name` names, for an operation the spec says mutates it
    /// — attaching a certificate to it, or releasing the one it holds.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a distribution.
    pub fn distribution_mut(&mut self, name: &str) -> Option<&mut StoredDistribution> {
        self.distributions.get_mut(name)
    }

    /// A token of `len` characters drawn from `alphabet`, for the purpose
    /// `label` names.
    ///
    /// Draws from the SAME `next_id` counter [`Self::mint_uuid`] does, on
    /// purpose: one counter means two values minted in this process are never
    /// built from the same input, and the hash is domain-separated by `label`
    /// so two tokens of the same counter value still differ. Every minted token
    /// of this store goes through here, so they cannot drift apart.
    fn mint_token(&mut self, label: &[u8], len: usize, alphabet: &[u8]) -> String {
        // One SHA-256 digest is 32 bytes, so a token asked for longer than that
        // would be silently cut short rather than drawn.
        debug_assert!(len <= 32, "a minted token draws at most 32 characters");
        if alphabet.is_empty() {
            return String::new();
        }
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        hasher.update(label);
        hasher
            .finalize()
            .iter()
            .take(len)
            .map(|byte| char::from(alphabet[usize::from(*byte) % alphabet.len()]))
            .collect()
    }

    /// A token for one container service's default domain: see
    /// [`DOMAIN_TOKEN_LEN`].
    pub fn mint_domain_token(&mut self) -> String {
        self.mint_token(
            b"container-service-domain",
            DOMAIN_TOKEN_LEN,
            DOMAIN_TOKEN_ALPHABET,
        )
    }

    /// A strong master user password for one managed relational database: the
    /// password `UpdateRelationalDatabase$rotateMasterUserPassword` asks the
    /// service to generate, which
    /// `GetRelationalDatabaseMasterUserPassword` is then what answers with.
    pub fn mint_master_user_password(&mut self) -> String {
        self.mint_token(
            b"relational-database-master-user-password",
            MINTED_MASTER_USER_PASSWORD_LEN,
            MINTED_MASTER_USER_PASSWORD_ALPHABET,
        )
    }

    /// The next private/public IPv4 offset, consumed so no two instances in this
    /// process share an address.
    fn fresh_address_offset(&mut self) -> u32 {
        self.next_address = self.next_address.wrapping_add(1);
        self.next_address
    }

    /// The private address, public address and IPv6 addresses an instance with
    /// `ip_address_type` gets, each present only when that address type gives the
    /// instance one.
    pub fn fresh_addresses(
        &mut self,
        ip_address_type: &str,
    ) -> (Option<String>, Option<String>, Vec<String>) {
        let offset = self.fresh_address_offset();
        let has_ipv4 = ip_address_type_has_ipv4(ip_address_type);
        (
            has_ipv4.then(|| ipv4(PRIVATE_IPV4_BASE, offset)),
            has_ipv4.then(|| ipv4(PUBLIC_IPV4_BASE, offset)),
            if ip_address_type_has_ipv6(ip_address_type) {
                vec![ipv6(offset)]
            } else {
                Vec::new()
            },
        )
    }

    /// The next public IPv4 address to reserve for a static IP, consumed so no
    /// two static IPs — and no static IP and instance — in this process share an
    /// address.
    ///
    /// Drawn from the SAME counter and the SAME public range
    /// [`Self::fresh_addresses`] gives an instance its public address from: one
    /// counter means one address is handed out once, whichever resource asked for
    /// it.
    pub fn fresh_static_ip_address(&mut self) -> String {
        let offset = self.fresh_address_offset();
        ipv4(PUBLIC_IPV4_BASE, offset)
    }

    /// An identifier shaped like the UUID Lightsail answers with for an instance
    /// or an operation.
    pub fn mint_uuid(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        let hex = hex::encode(hasher.finalize());
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The peering of the Lightsail VPC in `region`, for a reporting operation.
    pub fn vpc_peering(&self, region: &str) -> Option<&StoredVpcPeering> {
        self.vpc_peerings.get(region)
    }

    /// The peering of the Lightsail VPC in `region`, for an operation the spec
    /// says mutates it — the teardown `UnpeerVpc` starts.
    ///
    /// `region` is this record's store key, so it must not be reassigned through
    /// this handle: a peering belongs to the Region whose VPC it peers.
    pub fn vpc_peering_mut(&mut self, region: &str) -> Option<&mut StoredVpcPeering> {
        self.vpc_peerings.get_mut(region)
    }

    /// Store the peering `PeerVpc` just started for the Lightsail VPC in
    /// `region`, replacing the spent record of a peering that has since been
    /// torn down.
    pub fn insert_vpc_peering(&mut self, region: &str, peering: StoredVpcPeering) {
        self.vpc_peerings.insert(region.to_string(), peering);
    }

    /// Whether the Lightsail VPC in `region` is peered with the account's
    /// default VPC — what `IsVpcPeered` answers.
    ///
    /// A Region nothing has peered reports `false`, which is also what a Region
    /// whose peering has been torn down reports.
    pub fn is_vpc_peered(&self, region: &str) -> bool {
        self.vpc_peering(region)
            .is_some_and(StoredVpcPeering::is_peered)
    }

    /// The log entry `id` names, for a reporting operation.
    ///
    /// The log is append-only and bounded, so this is a linear scan of at most
    /// [`MAX_OPERATION_HISTORY`] entries.
    pub fn operation(&self, id: &str) -> Option<&StoredOperation> {
        self.operations.iter().find(|entry| entry.id == id)
    }

    /// One page of the log entries recorded against `resource_name`, oldest
    /// first, together with the id of the first entry of the page after it —
    /// present only while entries remain.
    ///
    /// The log is append-only, so an entry outlives the resource it was
    /// recorded against: a `DeleteInstance` entry is still reported under the
    /// name of the instance it deleted. Reporting is therefore by NAME over the
    /// log, never conditional on the resource still existing, and a name the log
    /// holds nothing for reports an empty page.
    ///
    /// A page token NAMES the entry a page starts at (an operation id) rather
    /// than carrying an index, so an entry appended between two pages cannot
    /// make the walk skip or repeat another one. `None` is the answer when the
    /// token names no entry of this resource's slice of the log — there is no
    /// page to report, which the caller turns into a refusal.
    ///
    /// The walk is bounded by [`MAX_OPERATION_HISTORY`].
    ///
    /// The page is located over the STORED entries and the modelled view is
    /// built only for the entries the page actually carries, so a resource with
    /// a long slice of the log is not viewed in full to answer ten entries.
    pub fn operations_for_resource_page(
        &self,
        resource_name: &str,
        page_token: Option<&str>,
    ) -> Option<(Vec<Operation>, Option<String>)> {
        let reported: Vec<&StoredOperation> = self
            .operations
            .iter()
            .filter(|entry| entry.resource_name == resource_name)
            .collect();
        let (page, next_page_token) =
            catalog_page(
                &reported,
                OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
                page_token,
                |entry| {
                    // An entry's token is its own `id`, a member it already holds,
                    // so the walk LENDS it rather than cloning it.
                    Some(std::borrow::Cow::Borrowed(entry.id.as_str()))
                },
            )?;
        Some((
            page.into_iter().map(StoredOperation::view).collect(),
            next_page_token,
        ))
    }

    /// One page of the WHOLE log — every entry recorded against every resource
    /// of this account — oldest first, together with the id of the first entry
    /// of the page after it, present only while entries remain.
    ///
    /// This is [`Self::operations_for_resource_page`] without the name filter,
    /// and it pages exactly the same way (the one [`catalog_page`] walk, a token
    /// that NAMES the entry a page starts at, `None` when the token names no
    /// entry of the log) at the size `GetOperations` publishes,
    /// [`OPERATIONS_PAGE_SIZE`].
    ///
    /// The log is append-only, so insertion order IS oldest first and an entry
    /// outlives the resource it was recorded against. The walk is bounded by
    /// [`MAX_OPERATION_HISTORY`], and the modelled view is built only for the
    /// entries the page carries.
    pub fn operations_page(
        &self,
        page_token: Option<&str>,
    ) -> Option<(Vec<Operation>, Option<String>)> {
        let reported: Vec<&StoredOperation> = self.operations.iter().collect();
        let (page, next_page_token) =
            catalog_page(&reported, OPERATIONS_PAGE_SIZE, page_token, |entry| {
                // An entry's token is its own `id`, a member it already holds,
                // so the walk LENDS it rather than cloning it.
                Some(std::borrow::Cow::Borrowed(entry.id.as_str()))
            })?;
        Some((
            page.into_iter().map(StoredOperation::view).collect(),
            next_page_token,
        ))
    }

    /// The live set of registry log in credentials for `region` that `username`
    /// and `password` log in with, if the store holds one.
    ///
    /// Every set the clock has already expired is gone before this runs (see
    /// [`Self::advance_clock`]), so a match here is a set that is still valid.
    pub fn registry_login(
        &self,
        region: &str,
        username: &str,
        password: &str,
    ) -> Option<&StoredRegistryLogin> {
        self.registry_logins
            .get(region)?
            .iter()
            .find(|login| login.username == username && login.accepts(password))
    }

    /// Record a set of registry log in credentials just created in `region`,
    /// dropping the oldest once that region holds
    /// [`MAX_REGISTRY_LOGINS_PER_REGION`].
    pub fn insert_registry_login(&mut self, region: &str, login: StoredRegistryLogin) {
        let logins = self.registry_logins.entry(region.to_string()).or_default();
        logins.push(login);
        if let Some(excess) = logins.len().checked_sub(MAX_REGISTRY_LOGINS_PER_REGION) {
            // A set the store had to forget early stops logging in before the
            // expiry its caller was handed, so the eviction is traceable rather
            // than a silent surprise. The username is the id this store holds;
            // no password or digest is logged.
            for evicted in logins.drain(..excess) {
                ::tracing::warn!(
                    region = %region,
                    username = %evicted.username,
                    created_at_epoch_millis = evicted.created_at_ms,
                    limit = MAX_REGISTRY_LOGINS_PER_REGION,
                    "the oldest container image registry log in credentials were dropped to \
                     keep this region within the store's bound"
                );
            }
        }
    }

    /// The container image registry of the account in `region`, for a reporting
    /// read. `None` where nothing has ever been pushed to it: a region with no
    /// registry answers every read about its content the same way an empty one
    /// does, so no read creates one.
    pub fn image_registry(&self, region: &str) -> Option<&StoredImageRegistry> {
        self.container_image_registries.get(region)
    }

    /// The container image registry of the account in `region`, for a push,
    /// which is what creates that region's registry the first time something is
    /// pushed to it.
    pub fn image_registry_mut(&mut self, region: &str) -> &mut StoredImageRegistry {
        self.container_image_registries
            .entry(region.to_string())
            .or_default()
    }

    /// Whether the registry in `region` holds the image `digest` names — the
    /// question `RegisterContainerImage` asks of the digest a caller presents.
    ///
    /// The digest of an IMAGE is the digest of its manifest, which is what a
    /// push reports and what the caller registers; a blob of the image (a layer
    /// or its configuration) is content of the image rather than the image, so
    /// it is not one.
    pub fn image_registry_holds_image(&self, region: &str, digest: &str) -> bool {
        self.image_registry(region)
            .is_some_and(|registry| registry.has_manifest(digest))
    }

    /// Start one blob upload against `repository` in `region`'s registry, and
    /// answer the session id the registry minted for it.
    ///
    /// The id comes from this store's own minting (see [`Self::mint_uuid`]), so
    /// nothing a client sends names a session.
    pub fn start_blob_upload(&mut self, region: &str, repository: &str, now_ms: i64) -> String {
        let session = self.mint_uuid();
        self.image_registry_mut(region)
            .start_upload(session.clone(), repository, now_ms);
        session
    }

    /// Append `setup` to the `SetupInstanceHttps` history, dropping the oldest
    /// records past [`MAX_INSTANCE_HTTPS_SETUPS`] so the history stays bounded
    /// however many calls a caller makes.
    ///
    /// The bound is over the WHOLE history, as the operation log's own is: the
    /// log is one append-only sequence, so a store driven past the bound drops
    /// its oldest records whichever instance they name.
    pub fn record_instance_https_setup(&mut self, setup: StoredInstanceHttpsSetup) {
        self.instance_https_setups.push(setup);
        if let Some(excess) = self
            .instance_https_setups
            .len()
            .checked_sub(MAX_INSTANCE_HTTPS_SETUPS)
        {
            self.instance_https_setups.drain(..excess);
        }
    }

    /// One page of the HTTPS setup history of the resource `resource_name`, and
    /// the token naming the first record of the page after it — present only
    /// while records remain.
    ///
    /// The most recent record comes first, and a page carries at most
    /// [`SETUP_HISTORY_PAGE_SIZE`] of them. The walk itself is [`catalog_page`],
    /// shared with this service's other paged listings; only the token a record
    /// is named by is this history's own — it is DERIVED from the record's
    /// operation id (see [`StoredInstanceHttpsSetup::page_token`]) rather than a
    /// member it holds, which is what `catalog_page`'s `Cow` answer is for. So a
    /// token naming no record of this resource's history answers `None`, which
    /// the caller turns into a refusal, and an absent token asks for the first
    /// page.
    pub fn setup_history_page(
        &self,
        resource_name: &str,
        page_token: Option<&str>,
    ) -> Option<(Vec<crate::ops::types::SetupHistory>, Option<String>)> {
        let reported = self
            .instance_https_setups
            .iter()
            .rev()
            .filter(|setup| setup.instance_name == resource_name)
            .collect::<Vec<_>>();
        let (page, next_page_token) = catalog_page(
            &reported,
            SETUP_HISTORY_PAGE_SIZE,
            page_token,
            // Owned, because the token is the operation id's hex digits rather
            // than a member the record holds. The walk is over at most
            // [`MAX_INSTANCE_HTTPS_SETUPS`] records, so the comparison is
            // bounded.
            |setup| Some(std::borrow::Cow::Owned(setup.page_token())),
        )?;
        Some((
            page.iter().map(|setup| setup.view()).collect::<Vec<_>>(),
            next_page_token,
        ))
    }

    /// Materialise every transition the clock had already made due at `now_ms`:
    /// an instance, a disk, a disk snapshot, an instance snapshot, an export
    /// snapshot record, a managed
    /// relational database, one of its snapshots, or a load balancer
    /// settles into the state it was owed, a container service walks
    /// into the phase it was owed (activating the deployment that was waiting),
    /// a certificate reaches the validation status it was owed,
    /// a Region's Lightsail VPC peering walks into the phase it was owed
    /// (established, or gone), a CDN distribution settles into the status it was
    /// owed, the `AutoSnapshot` add-on of a resource takes the automatic
    /// snapshot its schedule owed (and one it has finished taking reports
    /// `Success`), an operation whose work is done settles into `Succeeded`, and a set
    /// of container image registry log in credentials past its 12-hour validity
    /// is forgotten.
    ///
    /// This is the one write a reporting operation performs. Each transition was
    /// owed at this instant whether or not anybody called, so applying it here
    /// cannot show a caller anything a later caller would not also see.
    ///
    /// Every walk is bounded by [`MAX_INSTANCES`], [`MAX_DISKS`],
    /// [`MAX_DISK_SNAPSHOTS`], [`MAX_INSTANCE_SNAPSHOTS`],
    /// [`MAX_EXPORT_SNAPSHOT_RECORDS`],
    /// [`MAX_CONTAINER_SERVICES`],
    /// [`MAX_RELATIONAL_DATABASES`],
    /// [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`], [`MAX_LOAD_BALANCERS_PER_REGION`],
    /// [`MAX_CERTIFICATES`], [`MAX_DISTRIBUTIONS`] and
    /// [`MAX_OPERATION_HISTORY`] — and, for the peering walk, by the modelled
    /// `RegionName` enum [`vpc_peerings`](Self::vpc_peerings) is keyed by — and a
    /// record drops out of the work as soon as it has nothing outstanding. The
    /// automatic snapshot walk is bounded by
    /// [`auto_snapshots`](Self::auto_snapshots)' own bounds.
    fn advance_clock(&mut self, now_ms: i64) {
        for instance in self.instances.values_mut() {
            if let Some(due) = instance.due {
                if due.at_ms <= now_ms {
                    instance.state = due.state;
                    instance.due = None;
                }
            }
            // A metadata-options selection the service was still putting in
            // place is in place once its instant has come: the options were
            // applied then whether or not anybody asked.
            if let Some(applied_at_ms) = instance.metadata_options.applied_at_ms {
                if applied_at_ms <= now_ms {
                    instance.metadata_options.applied_at_ms = None;
                }
            }
            // An Amazon DCV session runs ON the instance, so it does not survive
            // the instance leaving `running`: the session was gone the instant
            // the computer stopped serving, whether or not anybody asked, so the
            // access is released here rather than by whoever happens to look. A
            // session whose hour has run out WHILE the computer kept running is
            // left where it is: `CreateGUISessionAccessDetails` reports it as
            // `startExpired`, which is what tells a caller to resume it.
            //
            // A session on a computer that is still COMING UP is left where it
            // is: that computer has not stopped serving, it has not started
            // yet, and the session's own start already waits for it
            // (`StoredGuiSession::starting_once_ready`).
            if (instance.state == STOPPING || instance.state == STOPPED)
                && instance.gui_session.is_some()
            {
                instance.gui_session = None;
            }
        }
        for disk in self.disks.values_mut() {
            let Some(due) = disk.due else { continue };
            if due.at_ms <= now_ms {
                disk.state = due.state;
                disk.due = None;
                // A disk that has settled into `available` is attached to
                // nothing — that is what the modelled `DiskState` value means
                // next to `in-use` — so settling there is also what completes
                // the detachment `DetachDisk` started: the attachment and the
                // device path it was mounted at are released together. A disk
                // that reached `available` from `pending` was attached to
                // nothing already, so the same step is a no-op for it. An
                // operation that ATTACHES a disk therefore has to put it in
                // `in-use` and clear the `due` it carries, or the settle it was
                // owed would release the attachment that operation just made.
                if disk.state == DISK_STATE_AVAILABLE {
                    disk.attached_to = None;
                    disk.path = None;
                }
            }
        }
        for snapshot in self.disk_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for snapshot in self.instance_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for record in self.export_snapshot_records.values_mut() {
            let Some(due) = record.due else { continue };
            if due.at_ms <= now_ms {
                record.state = due.state;
                record.due = None;
            }
        }
        for record in self.cloud_formation_stack_records.values_mut() {
            let Some(due) = record.due else { continue };
            if due.at_ms <= now_ms {
                record.state = due.state;
                record.due = None;
            }
        }
        for service in self.container_services.values_mut() {
            service.advance(now_ms);
        }
        for database in self.relational_databases.values_mut() {
            // A modification left for the database's maintenance window is
            // applied when that window is due, whatever state the database is
            // in — the window comes round on the clock, not on a state change.
            database.apply_due_modification(now_ms);
            let Some(due) = database.due else { continue };
            if due.at_ms <= now_ms {
                // The states whose engine is DOWN and coming back up: one being
                // restarted, and one being started again after a stop.
                let engine_coming_up = database.state == RELATIONAL_DATABASE_STATE_REBOOTING
                    || database.state == RELATIONAL_DATABASE_STATE_STARTING;
                let was_creating = database.state == RELATIONAL_DATABASE_STATE_CREATING;
                let was_rebooting = database.state == RELATIONAL_DATABASE_STATE_REBOOTING;
                database.state = due.state;
                database.due = None;
                // Settling into a new state is a change to the database, so the
                // database's event log carries one entry for it, timestamped at
                // the instant it fell due rather than at the instant someone
                // happened to read it.
                let settled = match database.state {
                    RELATIONAL_DATABASE_STATE_AVAILABLE if was_creating => {
                        Some(RELATIONAL_DATABASE_EVENT_CREATED)
                    }
                    RELATIONAL_DATABASE_STATE_AVAILABLE if was_rebooting => {
                        Some(RELATIONAL_DATABASE_EVENT_RESTARTED)
                    }
                    RELATIONAL_DATABASE_STATE_AVAILABLE => {
                        Some(RELATIONAL_DATABASE_EVENT_STARTED)
                    }
                    RELATIONAL_DATABASE_STATE_STOPPED => {
                        Some(RELATIONAL_DATABASE_EVENT_STOPPED)
                    }
                    // A database the clock has only moved towards its engine
                    // coming back up (`starting`) has nothing to report yet: it
                    // is not serving, and the entry for it is logged when it is.
                    _ => None,
                };
                if let Some(message) = settled {
                    let category = if was_creating {
                        RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION
                    } else {
                        RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY
                    };
                    database.record_event(due.at_ms, message, &[category]);
                }
                // A database that has come back up has an engine that read its
                // whole parameter group as it started, so this is the instant a
                // value applied `pending-reboot` is loaded — the one thing a
                // reboot changes about the record.
                if engine_coming_up && database.state == RELATIONAL_DATABASE_STATE_AVAILABLE {
                    database.load_pending_parameters();
                }
                // A database that has finished shutting down is owed one more
                // transition, the service's own: "If you don't manually start
                // your database instance after it has been stopped for seven
                // consecutive days, Amazon Lightsail automatically starts it for
                // you" (the modelled `StopRelationalDatabase` documentation).
                // It takes the same two steps a caller's `StartRelationalDatabase`
                // takes — `starting`, then serving — and
                // `StartRelationalDatabase` replaces this `due` when a caller
                // starts the database first.
                if database.state == RELATIONAL_DATABASE_STATE_STOPPED {
                    database.due = Some(DueNamedState {
                        at_ms: due
                            .at_ms
                            .saturating_add(RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS),
                        state: RELATIONAL_DATABASE_STATE_STARTING,
                    });
                } else if database.state == RELATIONAL_DATABASE_STATE_STARTING {
                    // The automatic start's second step: a start a CALLER made
                    // owes `available` from the handler, so the only database
                    // that settles INTO `starting` is one the clock started.
                    database.due = Some(DueNamedState::after(
                        due.at_ms,
                        RELATIONAL_DATABASE_STATE_AVAILABLE,
                    ));
                }
            }
        }
        for snapshot in self.relational_database_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
                // The copy can be restored from now, which completes the backup
                // of the database it was taken of: the second of the two entries
                // that backup leaves in THAT database's event log (the first is
                // logged where the snapshot is taken,
                // [`Self::snapshot_relational_database`]). A snapshot whose
                // source has since been deleted has no log to append to.
                if let Some(database) = self
                    .relational_databases
                    .get_mut(&snapshot.from_relational_database_name)
                {
                    database.record_event(
                        due.at_ms,
                        format!(
                            "{RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED}: {}",
                            snapshot.name
                        ),
                        &[RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
                    );
                }
            }
        }
        for balancer in self.load_balancers.values_mut() {
            // A registration whose deregistration has fallen due is gone: the
            // instance was going to come off the load balancer at that instant
            // whether or not anybody asked again, so the clock is what removes
            // it rather than whoever happens to read the load balancer next.
            balancer.attached_instances.retain(|registration| {
                registration.drains_at_ms.is_none_or(|at_ms| at_ms > now_ms)
            });
            // A certificate whose validation has fallen due is issued: the
            // certificate authority issued it at that instant whether or not
            // anybody asked, so the clock is what moves its status rather than
            // whoever happens to read the certificate next.
            for certificate in &mut balancer.certificates {
                let Some(due) = certificate.due.filter(|due| due.at_ms <= now_ms) else {
                    continue;
                };
                certificate.status = due.state;
                certificate.due = None;
            }
            let Some(due) = balancer.due else { continue };
            if due.at_ms <= now_ms {
                balancer.state = due.state;
                balancer.due = None;
            }
        }
        for peering in self.vpc_peerings.values_mut() {
            let Some(due) = peering.due else { continue };
            if due.at_ms <= now_ms {
                peering.phase = due.phase;
                peering.due = None;
            }
        }
        for distribution in self.distributions.values_mut() {
            if let Some(due) = distribution.due.filter(|due| due.at_ms <= now_ms) {
                distribution.status = due.state;
                distribution.due = None;
            }
            // A reset of the distribution's cached content settles on its OWN
            // clock: deleting cached content is not a configuration change, so
            // it neither waits for the distribution's deployment nor delays it.
            let Some(reset) = distribution.latest_cache_reset.as_mut() else {
                continue;
            };
            if let Some(due) = reset.due.filter(|due| due.at_ms <= now_ms) {
                reset.status = due.state;
                reset.due = None;
            }
        }
        for certificate in self.certificates.values_mut() {
            let Some(due) = certificate.due else { continue };
            if due.at_ms <= now_ms {
                certificate.status = due.state;
                certificate.due = None;
            }
        }
        // A subdomain the service was still creating exists, and one it was
        // still associating with a resource is associated with it: each step
        // was owed at the instant it fell due whether or not anybody asked.
        for subdomain in self.subdomains.values_mut() {
            subdomain.advance(now_ms);
        }
        // The `AutoSnapshot` add-on takes a copy of the resource it is enabled
        // on once a day: that copy was owed at the instant the schedule named
        // whether or not anybody called, so the clock is what takes it — and
        // what settles one the service has finished taking into `Success`.
        //
        // The copies due are collected first because taking one READS the rest
        // of the store (the disks attached to an instance), which cannot be done
        // while the schedule it is written to is borrowed.
        let owed: Vec<(&'static str, String)> = self
            .auto_snapshots
            .iter()
            .filter(|(_, schedule)| schedule.due_at_ms.is_some_and(|at_ms| at_ms <= now_ms))
            .map(|(key, _)| key.clone())
            .collect();
        for (resource_type, name) in owed {
            // A resource whose add-on has been disabled, or that this account no
            // longer holds, is owed no copy — and the hour the add-on carries is
            // what says when the next one falls due.
            let Some(hour) = self.auto_snapshot_hour(resource_type, &name) else {
                continue;
            };
            // What the copy captures: for a virtual private server, the block
            // storage disks attached to it at this instant; a disk's own copy is
            // of one volume, which has no disks attached to it.
            let captured = (resource_type == RESOURCE_TYPE_INSTANCE)
                .then(|| self.attached_disk_captures(&name));
            let Some(schedule) = self.auto_snapshots.get_mut(&(resource_type, name)) else {
                continue;
            };
            let Some(due_at_ms) = schedule.due_at_ms else {
                continue;
            };
            schedule.take(due_at_ms, captured);
            schedule.due_at_ms = Some(next_auto_snapshot_due_ms(due_at_ms, &hour));
        }
        for schedule in self.auto_snapshots.values_mut() {
            for taken in &mut schedule.taken {
                let Some(due) = taken.due.filter(|due| due.at_ms <= now_ms) else {
                    continue;
                };
                taken.status = due.state;
                taken.due = None;
            }
        }
        for operation in &mut self.operations {
            let Some(settles_at_ms) = operation.settles_at_ms else {
                continue;
            };
            if settles_at_ms <= now_ms {
                operation.status = OPERATION_STATUS_SUCCEEDED.to_string();
                operation.is_terminal = true;
                operation.status_changed_at_ms = settles_at_ms;
                operation.settles_at_ms = None;
            }
        }
        for setup in &mut self.instance_https_setups {
            let Some(settles_at_ms) = setup.settles_at_ms else {
                continue;
            };
            if settles_at_ms <= now_ms {
                setup.status = SETUP_STATUS_SUCCEEDED.to_string();
                setup.settles_at_ms = None;
            }
        }
        // A set of container image registry log in credentials expires 12 hours
        // after it was created whether or not anybody calls again
        // ([`REGISTRY_LOGIN_LIFETIME_MS`]), so a set the clock has taken past
        // its expiry is forgotten rather than left loggable-in with.
        for logins in self.registry_logins.values_mut() {
            logins.retain(|login| login.expires_at_ms > now_ms);
        }
        self.registry_logins.retain(|_, logins| !logins.is_empty());
    }

    /// The name of every resource this account holds that a request made in
    /// `region` addresses — what `GetActiveNames` reports: "the names of all
    /// active (not deleted) resources" as one flat list, with no resource type
    /// qualifying them.
    ///
    /// A SET, so the names come back in the store's own total order and a name
    /// two kinds of record somehow both hold is reported once: the answer is a
    /// set of NAMES, not one entry per record.
    ///
    /// Which records are the account's resources is the same question
    /// [`Self::with_tagged_record`] answers for one name, so this walks the same
    /// maps the same way: the ones keyed by name alone hold the resources every
    /// operation of theirs serves by name whichever Region the request is made
    /// from, and the ones keyed by the Region together with the name hold the
    /// resources whose identity includes a Region, of which only `region`'s own
    /// are addressable there. Beside them are the kinds whose modelled shape
    /// carries no `tags` member, so no tag operation resolves them — the static
    /// IP, the export snapshot record and the CloudFormation stack record. A
    /// resource a delete removed is held by no map, so its name is gone from
    /// here with it, which is what makes these the ACTIVE names.
    ///
    /// Every walk is bounded by the `MAX_` of the map it walks.
    pub fn active_names(&self, region: &str) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        // The maps keyed by name alone: the key IS the name the resource's own
        // reader serves it under.
        names.extend(self.instances.keys().cloned());
        names.extend(self.disks.keys().cloned());
        names.extend(self.disk_snapshots.keys().cloned());
        names.extend(self.instance_snapshots.keys().cloned());
        names.extend(self.export_snapshot_records.keys().cloned());
        names.extend(self.cloud_formation_stack_records.keys().cloned());
        names.extend(self.domains.keys().cloned());
        names.extend(self.relational_databases.keys().cloned());
        names.extend(self.relational_database_snapshots.keys().cloned());
        names.extend(self.static_ips.keys().cloned());
        names.extend(self.alarms.keys().cloned());
        names.extend(self.key_pairs.keys().cloned());
        names.extend(self.distributions.keys().cloned());
        names.extend(self.certificates.keys().cloned());
        names.extend(self.subdomains.keys().cloned());
        // The maps keyed by the Region together with the name.
        names.extend(names_in_region(&self.buckets, region));
        names.extend(names_in_region(&self.container_services, region));
        names.extend(names_in_region(&self.load_balancers, region));
        // A load balancer's SSL/TLS certificate is a resource of its own, named
        // by the request that asked for it and gone once it is deleted — and the
        // certificates live on the load balancer's record, which is held under
        // the Region they therefore belong to as well.
        names.extend(
            self.load_balancers_in(region)
                .into_iter()
                .flat_map(|balancer| balancer.certificates.iter())
                .map(|certificate| certificate.name.clone()),
        );
        // A contact method is held under its Region and its PROTOCOL, so its
        // name is the one on the record — the name this store minted, which
        // `GetContactMethods` reports.
        names.extend(
            self.contact_methods_in(region)
                .into_iter()
                .map(|method| method.name.clone()),
        );
        names
    }

    /// Run `body` against the tag-carrying record `name` names in `region` —
    /// whichever kind of resource that turns out to be — together with the
    /// `ResourceType` that kind reports. `None` when this account holds no
    /// tag-carrying resource of that name there.
    ///
    /// ONE name addresses one resource: `GetActiveNames` answers "the names of
    /// all active (not deleted) resources" of the account as a single flat list,
    /// with no resource type qualifying them, so at most one of the maps below
    /// holds a given name. Each map is searched the way that resource's OWN
    /// operations address it, so a tag change reaches exactly what they reach
    /// and no more: the maps keyed by `(region, name)` hold the resources whose
    /// identity includes a Region — a bucket, a container service, a load
    /// balancer, whose listings report "all of your … in the Amazon Web Services
    /// Region where the request is made" — so one of those is resolved in
    /// `region` alone; the maps keyed by name alone hold the resources every
    /// operation of theirs takes by name alone (`GetInstance`, `GetDisk`,
    /// `GetRelationalDatabase` and the rest serve them whichever Region the
    /// request is made from), so a tag change does the same rather than refusing
    /// a resource their own readers serve. A contact method is the one resource
    /// whose store key is neither: it is held under its Region and its PROTOCOL,
    /// so the map is resolved by scanning that Region's few entries for the name
    /// the store minted. A load balancer's SSL/TLS certificate is held on the
    /// record of the load balancer that holds it, which is a record of its
    /// Region, so it is scanned for there the same way.
    ///
    /// The one place a tag change resolves a resource, so every operation that
    /// edits tags admits exactly the same set of resources — the ones whose
    /// modelled shape carries a `tags` member.
    pub fn with_tagged_record<R>(
        &mut self,
        region: &str,
        name: &str,
        body: impl FnOnce(&mut dyn TaggedRecord, &'static str) -> R,
    ) -> Option<R> {
        if let Some(record) = self.instances.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_INSTANCE));
        }
        if let Some(record) = self.disks.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISK));
        }
        if let Some(record) = self.disk_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISK_SNAPSHOT));
        }
        if let Some(record) = self.instance_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_INSTANCE_SNAPSHOT));
        }
        if let Some(record) = self.domains.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DOMAIN));
        }
        if let Some(record) = self.relational_databases.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_RELATIONAL_DATABASE));
        }
        if let Some(record) = self.relational_database_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT));
        }
        if let Some(record) = self.alarms.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_ALARM));
        }
        // A key pair is held under its name alone, the name every key pair
        // operation addresses it by, so a tag change resolves it that way too.
        if let Some(record) = self.key_pairs.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_KEY_PAIR));
        }
        if let Some(record) = self.distributions.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISTRIBUTION));
        }
        // A certificate is held under its name alone, the name
        // `GetCertificates` serves it by, so a tag change resolves it that way
        // too — exactly like the distribution it is attached to.
        if let Some(record) = self.certificates.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_CERTIFICATE));
        }
        // A subdomain is held under its name alone, the name every subdomain
        // operation addresses it by, so a tag change resolves it that way too —
        // and the modelled `Subdomain$tags` names this door itself ("returned
        // only by the `GetSubdomains` action when you specify a subdomain",
        // populated by a create or by `TagResource`).
        if let Some(record) = self.subdomains.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_SUBDOMAIN));
        }
        // Only the Region-keyed maps need the pair, so it is built here rather
        // than above: a name the maps keyed by name alone hold never allocates
        // it.
        let regional = (region.to_string(), name.to_string());
        if let Some(record) = self.buckets.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_BUCKET));
        }
        if let Some(record) = self.container_services.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_CONTAINER_SERVICE));
        }
        if let Some(record) = self.load_balancers.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_LOAD_BALANCER));
        }
        // An SSL/TLS certificate of a load balancer is a tag-carrying resource
        // of its own: the modelled `LoadBalancerTlsCertificate` carries a `tags`
        // member, the call that asks for one stores the tags its request named,
        // and `GetActiveNames` reports the certificate's name among the
        // account's. Its record lives ON the load balancer that holds it, which
        // is held under the Region, so it is resolved in `region` too — by its
        // own name among that Region's load balancers' certificates, which is
        // how every certificate operation addresses one. The walk is bounded by
        // [`MAX_LOAD_BALANCERS_PER_REGION`] records of at most
        // [`MAX_CERTIFICATES_PER_LOAD_BALANCER`] certificates each.
        //
        // A certificate name is scoped to the load balancer that holds it —
        // "the same name may name a certificate of another load balancer, and
        // the two are different records"
        // ([`StoredLoadBalancer::certificate`]) — so where two load balancers
        // of one Region each hold a certificate of some name, that name
        // addresses the certificate of the first of them in the store's own
        // order: the one record the Region's flat set of active names reports
        // that name for.
        if let Some(record) = self
            .load_balancers
            .iter_mut()
            .filter(|((balancer_region, _), _)| balancer_region == region)
            .flat_map(|(_, balancer)| balancer.certificates.iter_mut())
            .find(|certificate| certificate.name == name)
        {
            return Some(body(record, RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE));
        }
        // A contact method is held under its Region and its protocol, so the
        // name it is tagged by — the one this store minted for it, which
        // `GetContactMethods` reports — is found by walking that Region's
        // entries. One Region holds at most one contact method per protocol, so
        // the walk is over as many entries as the modelled `ContactProtocol`
        // enum has members.
        if let Some(record) = self
            .contact_methods
            .iter_mut()
            .filter(|((method_region, _), _)| method_region == region)
            .map(|(_, method)| method)
            .find(|method| method.name == name)
        {
            return Some(body(record, RESOURCE_TYPE_CONTACT_METHOD));
        }
        None
    }

    /// Run `body` against the add-on-carrying record `name` names — whichever
    /// kind of resource that turns out to be — together with the `ResourceType`
    /// that kind reports. `None` when this account holds no add-on-carrying
    /// resource of that name.
    ///
    /// ONE name addresses one resource (the reasoning is spelled out on
    /// [`Self::with_tagged_record`]), and the resources whose modelled shape
    /// carries an `addOns` member are the virtual private server and the block
    /// storage disk, each of which every operation of theirs addresses by name
    /// alone. The one place an add-on change resolves a resource, so every
    /// operation that edits add-ons admits exactly the same set of them.
    ///
    /// The automatic snapshot schedule of the record is reconciled with
    /// whatever add-ons it carries once `body` has run
    /// ([`Self::reconcile_auto_snapshots`]) — so an add-on change starts or
    /// stops the schedule, and a call that refused the request and wrote
    /// nothing leaves the schedule exactly as it was.
    pub fn with_add_on_record<R>(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut dyn AddOnRecord, &'static str) -> R,
    ) -> Option<R> {
        let now_ms = now_ms();
        let (outcome, resource_type, hour) = if let Some(record) = self.instances.get_mut(name) {
            let outcome = body(record, RESOURCE_TYPE_INSTANCE);
            let hour = enabled_auto_snapshot_hour(&record.add_ons);
            (outcome, RESOURCE_TYPE_INSTANCE, hour)
        } else {
            // The other resource type add-ons apply to; a name that is neither
            // names no resource at all.
            let record = self.disks.get_mut(name)?;
            let outcome = body(record, RESOURCE_TYPE_DISK);
            let hour = enabled_auto_snapshot_hour(&record.add_ons);
            (outcome, RESOURCE_TYPE_DISK, hour)
        };
        self.reconcile_auto_snapshots(now_ms, resource_type, name, hour.as_deref());
        Some(outcome)
    }
}

/// The names of the records of a `(region, name)`-keyed map that belong to
/// `region`.
///
/// What a resource whose identity includes a Region is named by there: the
/// second half of its store key. Used by
/// [`ComAmazonawsLightsailLightsail20161128State::active_names`], which walks
/// three such maps and so asks the question three times.
fn names_in_region<'a, R>(
    records: &'a BTreeMap<(String, String), R>,
    region: &'a str,
) -> impl Iterator<Item = String> + 'a {
    records
        .keys()
        .filter(move |(record_region, _)| record_region == region)
        .map(|(_, name)| name.clone())
}

// ---------------------------------------------------------------------------
// Resource add-ons
//
// An add-on is held on the stored record of the resource it is enabled for,
// beside the rest of what that resource is. The trait below is what lets one
// add-on operation reach them all without knowing which kind of resource it was
// handed: see
// [`ComAmazonawsLightsailLightsail20161128State::with_add_on_record`].
// ---------------------------------------------------------------------------

/// What a stored record must expose for an add-on operation to work on it: where
/// it is, and the add-ons it has enabled.
pub trait AddOnRecord {
    /// Where the resource is, for the operation-log entry an add-on change
    /// writes.
    fn location(&self) -> &StoredLocation;
    /// The add-ons the resource has enabled now.
    fn add_ons(&self) -> &[AddOn];
    /// Replace the add-ons the resource has enabled.
    fn set_add_ons(&mut self, add_ons: Vec<AddOn>);
}

/// Implement [`AddOnRecord`] for each stored record that holds `location` and
/// `add_ons` — which every add-on-carrying Lightsail resource's record does, so
/// the implementation is the same for all of them.
macro_rules! add_on_records {
    ($($record:ident),+ $(,)?) => {
        $(impl AddOnRecord for $record {
            fn location(&self) -> &StoredLocation {
                &self.location
            }

            fn add_ons(&self) -> &[AddOn] {
                &self.add_ons
            }

            fn set_add_ons(&mut self, add_ons: Vec<AddOn>) {
                self.add_ons = add_ons;
            }
        })+
    };
}

add_on_records!(StoredInstance, StoredDisk);

/// What an add-on operation needs from the CALLER's request — the member path a
/// refusal names, and the sentence an unresolvable name is refused with — in a
/// module of its own. Re-exported here, so every caller reaches it as
/// `crate::state::…` exactly like the rest of this module.
pub mod add_ons;
pub use add_ons::{
    add_on_resource_not_found, AddOnMember, ADD_ONS_LIST_MEMBER, ADD_ON_REQUEST_MEMBER,
};

/// The add-ons of `record` with the one of type `add_on_type` removed.
///
/// A stored add-on is named by its type (the modelled `AddOnType` value the
/// request that enabled it carried), and a resource holds at most one add-on of
/// a type, so this removes the one the request named and leaves every other
/// add-on alone. A type the resource has not enabled removes nothing — the
/// result depends only on which type the request named.
pub fn add_ons_without(record: &dyn AddOnRecord, add_on_type: &str) -> Vec<AddOn> {
    record
        .add_ons()
        .iter()
        .filter(|add_on| add_on.name.as_deref() != Some(add_on_type))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Resource tags
//
// Every Lightsail resource whose modelled shape carries a `tags` member holds
// those tags on its own stored record, beside the rest of what the resource is.
// The trait below is what lets one tag operation reach them all without knowing
// which kind of resource it was handed: see
// [`ComAmazonawsLightsailLightsail20161128State::with_tagged_record`].
// ---------------------------------------------------------------------------

/// What a stored record must expose for a tag operation to work on it: where it
/// is, how else it can be named, and the tags it carries.
pub trait TaggedRecord {
    /// The resource's ARN, which a tag operation may name it by as well as by
    /// its name.
    fn arn(&self) -> &str;
    /// Where the resource is, for the operation-log entry a tag change writes.
    fn location(&self) -> &StoredLocation;
    /// The tags the resource carries now.
    fn tags(&self) -> &[Tag];
    /// Replace the tags the resource carries.
    fn set_tags(&mut self, tags: Vec<Tag>);
}

/// Implement [`TaggedRecord`] for each stored record that holds `arn`,
/// `location` and `tags` — which every tag-carrying Lightsail resource's record
/// does, so the implementation is the same for all of them.
macro_rules! tagged_records {
    ($($record:ident),+ $(,)?) => {
        $(impl TaggedRecord for $record {
            fn arn(&self) -> &str {
                &self.arn
            }

            fn location(&self) -> &StoredLocation {
                &self.location
            }

            fn tags(&self) -> &[Tag] {
                &self.tags
            }

            fn set_tags(&mut self, tags: Vec<Tag>) {
                self.tags = tags;
            }
        })+
    };
}

tagged_records!(
    StoredInstance,
    StoredDisk,
    StoredDiskSnapshot,
    StoredInstanceSnapshot,
    StoredDomain,
    StoredRelationalDatabase,
    StoredRelationalDatabaseSnapshot,
    StoredAlarm,
    StoredKeyPair,
    StoredContactMethod,
    StoredDistribution,
    StoredCertificate,
    StoredBucket,
    StoredContainerService,
    StoredLoadBalancer,
    StoredLoadBalancerCertificate,
);

/// The tags of `record` with every tag whose key is in `keys` removed.
///
/// Removal is BY KEY, whatever value the tag carries, and a key the resource
/// does not carry removes nothing — so the result depends only on which keys
/// the request named, never on how many times it named one or on whether the
/// resource ever carried it.
pub fn tags_without(record: &dyn TaggedRecord, keys: &BTreeSet<String>) -> Vec<Tag> {
    record
        .tags()
        .iter()
        .filter(|tag| tag.key.as_deref().is_none_or(|key| !keys.contains(key)))
        .cloned()
        .collect()
}

/// The tags of `record` with `added` merged into them.
///
/// Merging is BY KEY, the other half of [`tags_without`]'s rule: "tag keys must
/// be unique per resource" (the modelled `TagResource` documentation), so a key
/// the resource already carries keeps its place in the set and takes the new
/// value, and a key it does not carry is appended in the order the request named
/// it. A key the request names twice is therefore held once, carrying the last
/// value that request gave it.
///
/// Answers the set rather than applying it, so a caller can refuse a request that
/// would take the resource past [`MAX_TAGS`] before anything is written.
pub fn tags_merged_with(record: &dyn TaggedRecord, added: &[Tag]) -> Vec<Tag> {
    tags_merged_into(record.tags().to_vec(), added)
}

/// The tags a resource CREATED with `requested` carries, or a refusal naming the
/// bound that list broke.
///
/// A create starts the resource from no tags at all, so the set it ends up
/// carrying is the one [`tags_merged_with`] would answer for an untagged
/// resource: merging is BY KEY, because "tag keys must be unique per resource"
/// (the modelled `TagResource` documentation) is a rule about the RESOURCE and so
/// holds however the resource came to carry its tags. A key the request names
/// twice is therefore held once, carrying the last value that request gave it,
/// leaving a created resource in exactly the states a tag change can also reach —
/// and never one where a later `TagResource` for that key could not reach every
/// entry holding it.
///
/// The LIST is bounded first, at [`MAX_TAGS`] and in the wording every operation
/// of this crate that accepts `tags` answers with, so a caller-sized list is
/// refused before a single key is compared; the bound is deliberately on the
/// list rather than on the collapsed set, exactly as the tag-change path bounds
/// the list it is given.
pub fn tags_created_from(requested: Option<&[Tag]>) -> Result<Vec<Tag>, String> {
    let requested = requested.unwrap_or_default();
    if requested.len() > MAX_TAGS {
        return Err(format!(
            "tags carries {} entries, more than the {MAX_TAGS} this service accepts",
            requested.len()
        ));
    }
    Ok(tags_merged_into(Vec::new(), requested))
}

/// `added` merged into the tags `held`, the one place this crate applies the
/// per-resource key-uniqueness rule — reached from [`tags_merged_with`] for a
/// resource that already exists and from [`tags_created_from`] for one being
/// created, so both answer the same set for the same list.
fn tags_merged_into(mut held: Vec<Tag>, added: &[Tag]) -> Vec<Tag> {
    for tag in added {
        match held.iter_mut().find(|entry| entry.key == tag.key) {
            Some(entry) => entry.value = tag.value.clone(),
            None => held.push(tag.clone()),
        }
    }
    held
}

// ---------------------------------------------------------------------------
// Input conversion and bounds
// ---------------------------------------------------------------------------

/// The add-ons `requests` enable, or a refusal naming the member that is wrong.
///
/// `addOnType` targets the modelled `AddOnType` enum, so only its two members are
/// accepted; each add-on is stored enabled, carrying the parameters its own
/// request structure supplies. An `AutoSnapshot` request that names no
/// `snapshotTimeOfDay` gets the hour Lightsail would pick for it, drawn here from
/// the resource's own name so the answer is the same on every later read, and one
/// that DOES name a time is held to the hourly increment its documentation
/// requires (`is_hourly_time_of_day`) before it is stored.
///
/// `member` is where the caller's request carries them, so every refusal below
/// names a member of the request that was actually sent.
pub fn add_ons_from_requests(
    member: AddOnMember,
    resource_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if requests.len() > MAX_ADD_ONS {
        return Err(format!(
            "{} carries {} entries, more than the {MAX_ADD_ONS} this service accepts",
            member.carrier,
            requests.len()
        ));
    }
    requests
        .iter()
        .map(|request| match request.add_on_type.as_str() {
            "AutoSnapshot" => {
                // The daily hour the caller asked for, if any. The modelled
                // pattern admits every wall-clock minute, so the half of the
                // constraint only the documentation states — the time is an
                // hourly increment — is applied here, before the add-on is
                // built: an hour this service would never run a snapshot at is
                // refused rather than stored and reported back as the schedule.
                let named = request
                    .auto_snapshot_add_on_request
                    .as_ref()
                    .and_then(|auto| auto.snapshot_time_of_day.clone());
                if let Some(time_of_day) = &named {
                    if !is_hourly_time_of_day(time_of_day) {
                        return Err(format!(
                            "{}.autoSnapshotAddOnRequest.snapshotTimeOfDay '{time_of_day}' \
                             must be in HH:00 format, and in an hourly increment",
                            member.entry
                        ));
                    }
                }
                Ok(AddOn {
                    name: Some(ADD_ON_TYPE_AUTO_SNAPSHOT.to_string()),
                    status: Some(ADD_ON_STATUS_ENABLED.to_string()),
                    snapshot_time_of_day: Some(
                        named.unwrap_or_else(|| default_snapshot_time_of_day(resource_name)),
                    ),
                    ..Default::default()
                })
            }
            "StopInstanceOnIdle" => Ok(AddOn {
                name: Some("StopInstanceOnIdle".to_string()),
                status: Some(ADD_ON_STATUS_ENABLED.to_string()),
                threshold: request
                    .stop_instance_on_idle_request
                    .as_ref()
                    .and_then(|idle| idle.threshold.clone()),
                duration: request
                    .stop_instance_on_idle_request
                    .as_ref()
                    .and_then(|idle| idle.duration.clone()),
                ..Default::default()
            }),
            other => Err(format!(
                "{}.addOnType must be one of [{}], got '{other}'",
                member.entry,
                crate::ops::types::ADD_ON_TYPE_VALUES.join(", ")
            )),
        })
        .collect()
}

/// The one modelled `AddOnType` member a block storage disk can carry: the
/// automatic snapshot add-on.
///
/// The other member stops a virtual computer after an idle period
/// (`StopInstanceOnIdleRequest` — "the amount of idle time in minutes after
/// which your virtual computer will automatically stop"), which a block storage
/// disk does not have.
pub const ADD_ON_TYPE_AUTO_SNAPSHOT: &str = "AutoSnapshot";

/// The `AddOn$status` value an add-on a resource has enabled reports ("The
/// status of the add-on").
///
/// A disabled add-on is not reported at all — `DisableAddOn` removes it from the
/// resource's `addOns` — so this is the status every stored add-on carries, and
/// what tells the automatic snapshot schedule that the add-on is running.
pub const ADD_ON_STATUS_ENABLED: &str = "Enabled";

/// The daily hour the enabled `AutoSnapshot` add-on among `add_ons` takes a copy
/// at, and `None` when none of them is that add-on.
///
/// An add-on is named for its type and carries the status it is in, so those two
/// members are what say whether the service owes the resource a daily copy at
/// all — and `snapshotTimeOfDay` is when, "in `HH:00` format, and in Coordinated
/// Universal Time (UTC)".
fn enabled_auto_snapshot_hour(add_ons: &[AddOn]) -> Option<String> {
    add_ons
        .iter()
        .find(|add_on| {
            add_on.name.as_deref() == Some(ADD_ON_TYPE_AUTO_SNAPSHOT)
                && add_on.status.as_deref() == Some(ADD_ON_STATUS_ENABLED)
        })
        .and_then(|add_on| add_on.snapshot_time_of_day.clone())
}

/// `add_ons` as a read of the resource carrying them reports them: the enabled
/// `AutoSnapshot` add-on also says when its NEXT daily copy is due
/// (`next_auto_snapshot_time_of_day`, `None` when the store owes the resource no
/// further copy); every other add-on is reported exactly as it is stored.
///
/// The modelled `AddOn$nextSnapshotTimeOfDay` is a fact about the SCHEDULE the
/// resource's add-on runs on — the instant the next copy falls due, which the
/// clock re-arms after each one — and not about the request that enabled the
/// add-on, so it is read from that schedule at every read
/// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`])
/// rather than stored a second time on the record, where it could disagree with
/// the copy the clock actually takes next.
fn add_ons_view(add_ons: &[AddOn], next_auto_snapshot_time_of_day: Option<String>) -> Vec<AddOn> {
    add_ons
        .iter()
        .map(|add_on| AddOn {
            next_snapshot_time_of_day: (add_on.name.as_deref()
                == Some(ADD_ON_TYPE_AUTO_SNAPSHOT))
                .then(|| next_auto_snapshot_time_of_day.clone())
                .flatten(),
            ..add_on.clone()
        })
        .collect()
}

/// How many milliseconds one hour of Unix time counts — the unit the automatic
/// snapshot hour is reported in, since its times are hourly increments.
const MILLIS_PER_HOUR: i64 = 60 * 60 * 1_000;

/// How many milliseconds one day of Unix time counts. Unix time has no leap
/// second, so every UTC day in it is exactly this long — which is what lets the
/// daily automatic snapshot hour below be found by arithmetic rather than by a
/// calendar.
const MILLIS_PER_DAY: i64 = 24 * MILLIS_PER_HOUR;

/// The UTC hour the instant `epoch_millis` falls in, in the `HH:00` form the
/// modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` admits and
/// the automatic snapshot members report ("in `HH:00` format, and in Coordinated
/// Universal Time (UTC)").
///
/// Unix time has no leap second, so the hour of a UTC day an instant falls in is
/// arithmetic on that instant rather than a calendar lookup — the same reasoning
/// [`MILLIS_PER_DAY`] rests on. The division floors, so an instant anywhere
/// inside an hour reports that hour, which is what the member's "between the
/// time shown and up to 45 minutes after" describes.
fn utc_hour_of_day(epoch_millis: i64) -> String {
    format!(
        "{:02}:00",
        epoch_millis.div_euclid(MILLIS_PER_HOUR).rem_euclid(24)
    )
}

/// The instant the daily automatic snapshot hour `hour` next comes round after
/// `after_ms`.
///
/// `hour` is the add-on's `snapshotTimeOfDay`, which that member's
/// documentation states is "in `HH:00` format, and in Coordinated Universal Time
/// (UTC)" — so the next copy is due that far into a UTC day, on the first such
/// instant still ahead of `after_ms`. An hour that is not that form (nothing
/// this store writes) is read as the start of the day.
fn next_auto_snapshot_due_ms(after_ms: i64, hour: &str) -> i64 {
    // The one `hh24:mi` parser this module has, so the hour an automatic
    // snapshot is scheduled at and the window members of a relational database
    // are read the same way.
    let into_day_ms = minute_of_day(hour).unwrap_or(0).saturating_mul(60_000);
    let day_start_ms = after_ms.div_euclid(MILLIS_PER_DAY).saturating_mul(MILLIS_PER_DAY);
    let at_ms = day_start_ms.saturating_add(into_day_ms);
    if at_ms > after_ms {
        at_ms
    } else {
        at_ms.saturating_add(MILLIS_PER_DAY)
    }
}

/// The add-ons `requests` enable on a DISK, or a refusal naming the member that
/// is wrong.
///
/// The same conversion as [`add_ons_from_requests`], narrowed to the one add-on
/// a disk can carry ([`ADD_ON_TYPE_AUTO_SNAPSHOT`]): asking a disk for the
/// idle-stop add-on is refused rather than stored as an add-on that can never
/// act. Every operation that creates a disk goes through here, so `CreateDisk`
/// and `CreateDiskFromSnapshot` accept exactly the same add-ons.
pub fn disk_add_ons_from_requests(
    member: AddOnMember,
    disk_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if let Some(request) = requests
        .iter()
        .find(|request| request.add_on_type != ADD_ON_TYPE_AUTO_SNAPSHOT)
    {
        return Err(format!(
            "{}.addOnType '{}' cannot be enabled on a disk: a disk accepts only \
             {ADD_ON_TYPE_AUTO_SNAPSHOT}",
            member.entry, request.add_on_type
        ));
    }
    add_ons_from_requests(member, disk_name, requests)
}

/// Whether `value` is a date in the `YYYY-MM-DD` form an automatic snapshot is
/// addressed by.
///
/// The form is the modelled `AutoSnapshotDate` pattern
/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}$`; the members that carry a restore date
/// (`CreateDiskFromSnapshot$restoreDate` and its siblings) target a plain
/// `string`, whose documentation says "Must be specified in YYYY-MM-DD format",
/// so the constraint is enforced here rather than by the modelled-constraint
/// walk.
pub fn is_auto_snapshot_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|index| bytes[*index].is_ascii_digit())
}

/// The source members of the operations that can address an AUTOMATIC snapshot
/// — the one rule those three requests are judged by, and the sentences it
/// refuses a combination with — in a module of its own. Re-exported here, so
/// every caller reaches it as `crate::state::…` exactly like the rest of this
/// module.
pub mod snapshot_source;
pub use snapshot_source::{
    no_such_auto_snapshot, requested_snapshot_source, RequestedSnapshotSource,
    SnapshotSourceMembers, COPY_SNAPSHOT_SOURCE_MEMBERS, DISK_RESTORE_SOURCE_MEMBERS,
    INSTANCE_RESTORE_SOURCE_MEMBERS,
};

/// Whether `value` is a daily automatic-snapshot hour an `AutoSnapshot` add-on
/// can be scheduled at.
///
/// `AutoSnapshotAddOnRequest$snapshotTimeOfDay` and the `AddOn` members that
/// report it target the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$`, which admits any wall-clock minute of
/// the day; their documentation narrows it to a value that "Must be in `HH:00`
/// format, and in an hourly increment". That narrowing is prose the
/// modelled-constraint walk cannot express, so — exactly as for
/// [`is_auto_snapshot_date`] — it is enforced here: an hourly increment is a
/// minute of the day that is a whole number of hours into it, which is also the
/// only form this store ever writes. The time itself is read by the one
/// `hh24:mi` parser this module has ([`minute_of_day`], which admits only two
/// ASCII digits per component and bounds both), so the answer does not depend on
/// the modelled pattern having been walked first.
fn is_hourly_time_of_day(value: &str) -> bool {
    minute_of_day(value).is_some_and(|minute| minute % 60 == 0)
}

/// The daily automatic-snapshot hour for `resource_name`, in the `HH:00` form the
/// modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` requires.
///
/// Derived from the resource's own name rather than drawn per call, so every read
/// of the resource reports the same hour.
fn default_snapshot_time_of_day(resource_name: &str) -> String {
    let digest = Sha256::digest(resource_name.as_bytes());
    format!("{:02}:00", u32::from(digest[0]) % 24)
}

// ---------------------------------------------------------------------------
// Domain names
// ---------------------------------------------------------------------------

/// The most characters a domain name may carry.
///
/// RFC 1035 §2.3.4 caps an encoded domain name at 255 octets, which is 253
/// characters in the dotted presentation form a request uses.
pub const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// The most characters one label of a domain name may carry, from the same
/// RFC 1035 §2.3.4 limit on a single label.
pub const MAX_DOMAIN_LABEL_CHARS: usize = 63;

/// Refuse `domain_name` unless it is a DNS domain name this service can hold a
/// recordset for, answering the member and the constraint it broke.
///
/// The modelled `DomainName` shape carries no pattern and no length, and the
/// modelled `CreateDomain` documentation says the name is "the specified domain
/// (example.com)" — so the rule is DNS's own: at least two dot-separated
/// labels (a name with no parent zone addresses no domain), each a non-empty
/// run of at most [`MAX_DOMAIN_LABEL_CHARS`] letters, digits and hyphens that
/// neither begins nor ends with a hyphen (RFC 1035 §2.3.1 preferred syntax,
/// widened to leading digits by RFC 1123 §2.1), and at most
/// [`MAX_DOMAIN_NAME_CHARS`] characters overall.
pub fn validate_domain_name(domain_name: &str) -> Result<(), String> {
    if domain_name.chars().count() > MAX_DOMAIN_NAME_CHARS {
        return Err(format!(
            "domainName carries {} characters, more than the {MAX_DOMAIN_NAME_CHARS} a \
             domain name may have",
            domain_name.chars().count()
        ));
    }
    let labels: Vec<&str> = domain_name.split('.').collect();
    if labels.len() < 2 {
        return Err(format!(
            "domainName '{domain_name}' is not a domain name: it must carry at least two \
             dot-separated labels (example.com)"
        ));
    }
    for label in labels {
        if label.is_empty() {
            return Err(format!(
                "domainName '{domain_name}' is not a domain name: every dot-separated label \
                 must be non-empty"
            ));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_CHARS {
            return Err(format!(
                "domainName label '{label}' carries {} characters, more than the \
                 {MAX_DOMAIN_LABEL_CHARS} a label may have",
                label.chars().count()
            ));
        }
        if !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(format!(
                "domainName label '{label}' is not a domain label: a label carries only \
                 ASCII letters, digits and hyphens"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "domainName label '{label}' is not a domain label: a label neither begins \
                 nor ends with a hyphen"
            ));
        }
    }
    Ok(())
}

/// The domain entry types a zone may hold, from the modelled `DomainEntry$type`
/// documentation: "address for IPv4 (A), address for IPv6 (AAAA), canonical name
/// (CNAME), mail exchanger (MX), name server (NS), start of authority (SOA),
/// service locator (SRV), or text (TXT)", listed there as the types that "can be
/// used".
///
/// The modelled `DomainEntryType` shape is a plain string, so this list — not a
/// generated enum — is what an entry's `type` is checked against.
pub const DOMAIN_ENTRY_TYPES: &[&str] = &["A", "AAAA", "CNAME", "MX", "NS", "SOA", "SRV", "TXT"];

/// The types `DOMAIN_ENTRY_TYPES` admits, for a refusal message.
pub fn domain_entry_types() -> String {
    DOMAIN_ENTRY_TYPES.join(", ")
}

/// Refuse `entry_name` unless it is a DNS name the zone `domain_name` can hold a
/// recordset for, answering the member and the constraint it broke.
///
/// A recordset entry lives INSIDE one zone, so its name is either the zone's own
/// name (the apex) or a name under it; `member` is the request member the name
/// came from, so the refusal names what the caller sent.
///
/// The syntax rule is DNS's own, and WIDER than [`validate_domain_name`]'s,
/// because a zone holds entries a zone name may not be: a label may be the `*`
/// wildcard (leftmost label only, RFC 4592 §2.1.1) and a label may carry an
/// underscore, which is what an `_tcp` service locator (RFC 2782) or a `_`-prefixed
/// validation record is made of. Names are compared case-insensitively (RFC 4343).
pub fn validate_domain_entry_name(
    member: &str,
    entry_name: &str,
    domain_name: &str,
) -> Result<(), String> {
    if entry_name.chars().count() > MAX_DOMAIN_NAME_CHARS {
        return Err(format!(
            "{member} carries {} characters, more than the {MAX_DOMAIN_NAME_CHARS} a \
             domain name may have",
            entry_name.chars().count()
        ));
    }
    let labels: Vec<&str> = entry_name.split('.').collect();
    if labels.len() < 2 {
        return Err(format!(
            "{member} '{entry_name}' is not a domain name: it must carry at least two \
             dot-separated labels (www.example.com)"
        ));
    }
    for (index, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(format!(
                "{member} '{entry_name}' is not a domain name: every dot-separated label \
                 must be non-empty"
            ));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_CHARS {
            return Err(format!(
                "{member} label '{label}' carries {} characters, more than the \
                 {MAX_DOMAIN_LABEL_CHARS} a label may have",
                label.chars().count()
            ));
        }
        if *label == "*" {
            if index == 0 {
                continue;
            }
            return Err(format!(
                "{member} '{entry_name}' is not a domain name: the '*' wildcard is only \
                 the leftmost label of a name"
            ));
        }
        if !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
        {
            return Err(format!(
                "{member} label '{label}' is not a domain label: a label carries only \
                 ASCII letters, digits, hyphens and underscores"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "{member} label '{label}' is not a domain label: a label neither begins \
                 nor ends with a hyphen"
            ));
        }
    }
    let name = entry_name.to_ascii_lowercase();
    let zone = domain_name.to_ascii_lowercase();
    if name != zone && !name.ends_with(&format!(".{zone}")) {
        return Err(format!(
            "{member} '{entry_name}' is not inside the domain '{domain_name}': an entry \
             names either the domain itself or a name under it"
        ));
    }
    Ok(())
}

/// Whether `entry` is the recordset entry `name`/`entry_type`/`target` describes:
/// the three values that identify one entry of a zone. Names and types are
/// compared case-insensitively (RFC 4343, and the type is a DNS record type).
pub fn is_same_domain_entry(
    entry: &DomainEntry,
    name: &str,
    entry_type: &str,
    target: &str,
) -> bool {
    let same = |stored: &Option<String>, requested: &str| {
        stored
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(requested))
    };
    same(&entry.name, name) && same(&entry.r#type, entry_type) && entry.target.as_deref() == Some(target)
}

// ---------------------------------------------------------------------------
// Clocks and ids
// ---------------------------------------------------------------------------

/// Now, in milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Epoch milliseconds as the modelled `IsoDate` timestamp.
pub fn timestamp(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
}

/// The UTC calendar date of `epoch_millis`, in the `YYYY-MM-DD` form an
/// automatic snapshot is dated by: `AutoSnapshotDetails$date` is "the date of
/// the automatic snapshot in `YYYY-MM-DD` format".
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)`
/// renders the instant as RFC 3339 in UTC (`2026-10-24T01:05:05Z`), whose first
/// field is that date, so no leap-year arithmetic is re-derived here.
pub fn utc_date(epoch_millis: i64) -> String {
    timestamp(epoch_millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| {
            rendered
                .split_once('T')
                .map(|(date, _)| date.to_string())
        })
        .unwrap_or_default()
}

/// The monthly billing cycle `epoch_millis` falls in, as the UTC calendar month
/// it belongs to (`YYYY-MM`): an account is billed per calendar month, so two
/// instants of the same month are the same cycle and the month after it is the
/// next one.
///
/// The calendar conversion is [`utc_date`]'s, so no month arithmetic is
/// re-derived here.
pub fn billing_cycle(epoch_millis: i64) -> String {
    let date = utc_date(epoch_millis);
    match date.rsplit_once('-') {
        Some((cycle, _day)) => cycle.to_string(),
        None => date,
    }
}

/// Whether the one bundle change a monthly billing cycle allows is available at
/// `now_ms` to a resource whose bundle was last changed at `last_changed_at_ms`
/// (`None` on one that has never been moved).
///
/// The allowance belongs to the CYCLE the change was made in: a further request
/// in that same cycle finds it spent, and it is available again once that cycle
/// has passed, with nothing to materialise for the resource in between. A
/// bucket's and a distribution's bundle update document the one rule, so both
/// ask here instead of latching a flag of their own.
pub fn bundle_change_available(last_changed_at_ms: Option<i64>, now_ms: i64) -> bool {
    last_changed_at_ms.is_none_or(|changed| billing_cycle(changed) != billing_cycle(now_ms))
}

/// Order two modelled timestamps. A `DateTime` carries seconds plus a
/// sub-second part, so both are compared — two instants one nanosecond apart
/// are not the same instant.
pub fn timestamp_key(value: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
}

/// Nanoseconds in one second, the divisor a `DateTime`'s sub-second part is held
/// against.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// One modelled timestamp as the seconds since the epoch a refusal reports it
/// by: the whole seconds, the sub-second part beside them, and the instant's own
/// sign.
///
/// The whole seconds alone are NOT the instant — every instant inside one second
/// shares them, and the seconds component of a pre-epoch instant is the second
/// BELOW it — so a message built from them alone states two equal numbers for a
/// pair that differs. The fraction is rendered exactly, from the nanoseconds
/// rather than through a float, and trailing zeros are dropped so a whole second
/// still reads as one number.
fn epoch_seconds_display(value: ::aws_smithy_types::DateTime) -> String {
    let (secs, subsec_nanos) = timestamp_key(value);
    if subsec_nanos == 0 {
        return secs.to_string();
    }
    // `secs` is the second at or below the instant and `subsec_nanos` the part
    // above that second, so a NEGATIVE instant's magnitude is one second less
    // its fraction: -0.5 is seconds -1 plus 500000000 nanoseconds.
    let (sign, whole, nanos) = if secs < 0 {
        (
            "-",
            secs.unsigned_abs().saturating_sub(1),
            NANOS_PER_SECOND - u64::from(subsec_nanos),
        )
    } else {
        ("", secs.unsigned_abs(), u64::from(subsec_nanos))
    };
    let fraction = format!("{nanos:09}");
    format!("{sign}{whole}.{}", fraction.trim_end_matches('0'))
}

/// The refusal a time window whose end does not follow its start earns, and
/// `None` while the pair is a window — the one sentence this service answers
/// that pair with wherever it takes one.
///
/// Both bounds are named as the INSTANTS they are, sub-second part and sign
/// included, so a window inverted inside one second — or one lying before the
/// epoch — is reported as what it is rather than as two equal whole seconds.
pub fn inverted_window_refusal(
    start: ::aws_smithy_types::DateTime,
    end: ::aws_smithy_types::DateTime,
) -> Option<String> {
    (timestamp_key(start) >= timestamp_key(end)).then(|| {
        format!(
            "startTime must be earlier than endTime, got startTime {} and endTime {} \
             (seconds since the epoch)",
            epoch_seconds_display(start),
            epoch_seconds_display(end)
        )
    })
}

/// A seed drawn once per process, so ids minted by two emulator processes from
/// the same counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos())
    })
}

/// The ARN of the instance `id` in `region`.
pub fn instance_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INSTANCE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the block storage disk `id` in `region`.
pub fn disk_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DISK_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the disk snapshot `id` in `region`.
pub fn disk_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DISK_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the instance snapshot `id` in `region`.
pub fn instance_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INSTANCE_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the managed relational database snapshot `id` in `region`.
pub fn relational_database_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RELATIONAL_DATABASE_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the SSH key pair `id` in `region`.
pub fn key_pair_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{KEY_PAIR_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the alarm `id` in `region`.
pub fn alarm_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ALARM_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the contact method `id` in `region`.
pub fn contact_method_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONTACT_METHOD_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the domain recordset `id`. A DNS zone is not regional, so its ARN
/// carries [`DOMAIN_ARN_REGION`] where an instance ARN carries a region.
pub fn domain_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DOMAIN_ARN_REGION,
        &format!("{DOMAIN_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the static IP `id` in `region`.
pub fn static_ip_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{STATIC_IP_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The support code an instance reports: the `{account}/{resource id}` form the
/// modelled `supportCode` member describes, so support can look the resource up
/// from the value alone.
pub fn support_code(id: &str) -> String {
    format!("{}/{id}", ::engine::emulator_account_id())
}

/// How Lightsail words the refusal of a create whose name the account already
/// holds, naming the name the request asked for: "Some names are already in use:
/// <name>".
///
/// Plural because the sentence is the one Lightsail answers whether the create
/// names one resource or several (`CreateInstances` takes a list of names), so
/// the wording does not change with the count — only the names it reports do.
/// The owner of that wording for the key-pair create, and where another create
/// of this crate adopts it rather than keeping a second copy.
pub fn names_already_in_use_message(name: &str) -> String {
    format!("Some names are already in use: {name}")
}

/// How a refusal words the condition "the resource is the origin of a
/// distribution", given how many of this account's distributions pull their
/// content from it, and `None` when none does.
///
/// The ONE owner of that wording: every delete that can remove a resource a
/// distribution pulls from reports the same condition, so it cannot drift
/// between the kinds of origin resource — the bucket delete, which offers a
/// force that overrides it, names it beside the other conditions that member
/// documents, and the deletes that offer none name it on its own (see
/// [`ComAmazonawsLightsailLightsail20161128State::in_use_as_distribution_origin`]).
pub fn origin_of_distributions_condition(count: usize) -> Option<String> {
    (count > 0).then(|| {
        format!(
            "it is the origin of {count} distribution{}",
            if count == 1 { "" } else { "s" }
        )
    })
}

/// How a refusal words the condition "the disk's own state does not allow this
/// call", given the operation's verb (`attach`, `detach`, `delete`) and the
/// disk's current `DiskState` value.
///
/// The ONE owner of that wording: every disk call a disk's state blocks reports
/// the condition the same way, keyed on the STATE alone — the only variable part
/// besides the verb — so attach, detach and delete cannot word one condition
/// three different ways.
pub fn disk_blocked_by_state_message(verb: &str, state: &str) -> String {
    format!("You can't {verb} this disk right now. The state of this disk is: {state}.")
}

/// How a refusal words the condition "the disk is still attached to an
/// instance", naming the instance that holds it — the only variable part, read
/// from the disk's own [`StoredDisk::attached_to`].
///
/// The ONE owner of that wording, beside [`disk_blocked_by_state_message`]: this
/// sentence is what names the instance a caller has to detach from, so a call
/// blocked by the ATTACHMENT reports the holder rather than a state word.
pub fn disk_still_attached_message(instance_name: &str) -> String {
    format!(
        "You can't perform this operation because the disk is still attached to a Lightsail \
         instance: {instance_name}."
    )
}

/// The region a request is scoped to: the one its SigV4 credential scope names,
/// or [`DEFAULT_REGION`] when the request carries no scope (an unsigned call) or
/// names a region Lightsail does not serve.
///
/// `ResourceLocation$regionName` targets the modelled `RegionName` enum, so the
/// answer is always one of that enum's values — a scope naming anything else is
/// not a region this service has resources in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    signed_region(parts)
        .filter(|region| REGION_NAME_VALUES.contains(&region.as_str()))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region this request's SigV4 credential scope names, verbatim, and `None`
/// when it carries no scope at all.
///
/// The one place this crate reads the Region off the request: what the scope says
/// and what the service does with it are two different questions, and the callers
/// below answer them differently.
fn signed_region(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region)
}

/// The refusal for a domain or distribution call this request cannot be served
/// by, and `None` when it can.
///
/// Those APIs are served by the N. Virginia endpoint ALONE (see
/// [`DOMAIN_REGION_ONLY_MESSAGE`]), so what decides is the Region the request's
/// SigV4 credential scope names — whatever it names, not only the Regions this
/// service holds resources in, since a scope naming a Region Lightsail does not
/// serve is no more us-east-1 than any other. A request carrying no scope at all
/// is served in [`DEFAULT_REGION`], which IS the domain Region, so it is served
/// rather than refused.
///
/// The same documented restriction covers the distribution (and distribution
/// certificate) operations: `CreateDistribution` and `GetDistributions` call
/// THIS helper, and so must any other one of them when it is implemented,
/// rather than re-deriving the rule — so the restriction stays in one place.
pub fn domain_region_refusal(parts: &::http::request::Parts) -> Option<String> {
    let signed = signed_region(parts)?;
    (signed != DOMAIN_REGION_NAME).then(|| DOMAIN_REGION_ONLY_MESSAGE.to_string())
}

/// The kinds of record the restriction [`domain_region_refusal`] carries covers:
/// the DNS zone, the CDN distribution, the SSL/TLS certificate a distribution is
/// served with, and the subdomain a distribution is reachable at — the records
/// whose own operations all ask that helper, each held under its name alone
/// because it is addressed from the one Region its API is served in (see
/// [`DISTRIBUTION_REGION_NAME`]).
///
/// A load balancer's SSL/TLS certificate is NOT one of them: it is a record of
/// the Region its load balancer lives in, and the load balancer API is served in
/// every Region.
pub const DOMAIN_REGION_ONLY_RESOURCE_TYPES: [&str; 4] = [
    RESOURCE_TYPE_DOMAIN,
    RESOURCE_TYPE_DISTRIBUTION,
    RESOURCE_TYPE_CERTIFICATE,
    RESOURCE_TYPE_SUBDOMAIN,
];

/// The refusal for a call onto a record of `resource_type` this request cannot
/// be served by, and `None` when the kind is not one the restriction covers or
/// the request is made in the Region those APIs are served from.
///
/// For a door that admits every kind of record and so learns WHICH kind a name
/// addresses only once it has resolved it — a tag change, which reaches every
/// record whose modelled shape carries a `tags` member
/// ([`ComAmazonawsLightsailLightsail20161128State::with_tagged_record`]). It
/// asks [`domain_region_refusal`], the one owner of the rule, so such a door
/// admits exactly the Regions the record's own operations do rather than
/// re-deriving the restriction.
pub fn domain_region_refusal_for_resource_type(
    parts: &::http::request::Parts,
    resource_type: &str,
) -> Option<String> {
    if !DOMAIN_REGION_ONLY_RESOURCE_TYPES.contains(&resource_type) {
        return None;
    }
    domain_region_refusal(parts)
}

/// The endpoint this emulator was reached on for this request: its `Host`
/// header, or the `:authority` an HTTP/2 request carries on the URI instead of a
/// `Host` header.
///
/// This is what an operation announcing an address a caller must be able to
/// reach has to build it from — never a literal AWS hostname, which would send
/// the next client out of the emulator (and out of a sandboxed run) entirely.
/// `None` when the request names no host at all, and then there is no endpoint
/// to announce.
///
/// Each source is tested on its own, in that order, and the FIRST one carrying a value
/// wins: a `Host` header that names nothing (an empty value) leaves the authority the URI
/// carries still to be read, rather than standing in for it.
pub fn request_endpoint(parts: &::http::request::Parts) -> Option<String> {
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|host| host.to_str().ok()),
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|host| !host.is_empty())
    .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Reaching the store
// ---------------------------------------------------------------------------

/// Run `body` against this service's store, with every clock-owed transition
/// already applied.
///
/// EVERY handler goes through here, so no handler can observe an instance or a
/// disk that should already have settled. `None` means the engine is not the shared
/// in-memory one — answer it with a modelled server error, never with a fresh
/// engine.
pub fn with_state<Eng: Send + Sync + 'static, R>(
    engine: &Eng,
    body: impl FnOnce(&mut ComAmazonawsLightsailLightsail20161128State) -> R,
) -> Option<R> {
    let state = service_state(engine)?;
    Some(with_held_state(&state, body))
}

/// Run `body` against the store a caller ALREADY holds the handle to, with
/// every clock-owed transition already applied.
///
/// The same read [`with_state`] gives a handler, for a holder that was handed
/// the handle rather than the engine: a data plane's session, which has no
/// request context of its own and still must not observe a database that should
/// already have settled.
pub fn with_held_state<R>(
    state: &::engine::ServiceState<ComAmazonawsLightsailLightsail20161128State>,
    body: impl FnOnce(&mut ComAmazonawsLightsailLightsail20161128State) -> R,
) -> R {
    let mut guard = state.lock();
    guard.advance_clock(now_ms());
    body(&mut guard)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLightsailLightsail20161128State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLightsailLightsail20161128State>)
}

// ---------------------------------------------------------------------------
// The parts of this module
// ---------------------------------------------------------------------------
//
// One file per concern — the service's own conventions, each published catalog,
// and each resource family's stored records — all re-exported here, so every
// caller still reaches them as `crate::state::…` exactly as before.

/// How this service names itself and its resources.
mod identity;
pub use identity::*;

/// The states a stored resource moves through, and what the clock owes it.
mod lifecycle;
pub use lifecycle::*;

/// What this store will hold, and how much of it one page reports.
mod bounds;
pub use bounds::*;

/// How a paginated read serves one page.
mod paging;
pub use paging::*;

/// The published instance and bucket bundle catalogs.
mod bundles;
pub use bundles::*;

/// Stored buckets and the access keys they grant access through.
mod buckets;
pub use buckets::*;

/// The published blueprint catalog.
mod blueprints;
pub use blueprints::*;

/// The published managed-database blueprint, bundle and parameter catalogs.
mod relational_database_catalog;
pub use relational_database_catalog::*;

/// The Region catalog, the locations resources live in, and their addresses.
mod regions;
pub use regions::*;

/// An instance's inbound firewall.
mod firewall;
pub use firewall::*;

/// The stored instance, this service's primary resource.
mod instances;
pub use instances::*;

/// The GUI (Amazon DCV) session of a virtual computer.
mod gui_sessions;
pub use gui_sessions::*;

/// The temporary access the service grants to one instance, and the host key its
/// browser-based access path has witnessed.
mod instance_access;
pub use instance_access::*;

/// Block storage disks and their snapshots.
mod disks;
pub use disks::*;

/// Instance snapshots, automatic snapshots and their exports.
mod snapshots;
pub use snapshots::*;

/// The CloudFormation stack records an export is turned into.
mod cloud_formation_stack_records;
pub use cloud_formation_stack_records::*;

/// The stored domain recordset.
mod domains;
pub use domains::*;

/// The static IPs the account reserves.
mod static_ips;
pub use static_ips::*;

/// The SSH key pairs the account holds.
mod key_pairs;
pub use key_pairs::*;

/// The managed relational databases and their snapshots.
mod relational_databases;
pub use relational_databases::*;

/// The DATA plane of a managed relational database: the endpoint its engine is
/// served at.
mod relational_database_plane;
pub use relational_database_plane::*;

/// The append-only operation log every mutating call writes.
mod operation_log;
pub use operation_log::*;

/// The history of the HTTPS setup calls made against one instance.
mod instance_https_setup;
pub use instance_https_setup::*;

/// Container services, their power catalog and the image registry.
mod container_services;
pub use container_services::*;

/// The metrics, alarms and contact methods this service watches and tells.
mod monitoring;
pub use monitoring::*;

/// The series a metric read answers with, and what one sample of one is.
mod metric_series;
pub use metric_series::*;

/// Load balancers, their registrations and their attached certificates.
mod load_balancers;
pub use load_balancers::*;

/// The published load balancer TLS security policy catalog.
mod load_balancer_tls_policies;
pub use load_balancer_tls_policies::*;

/// The SSL/TLS certificates the account requests.
mod certificates;
pub use certificates::*;

/// The content delivery network distributions and their bundles.
mod distributions;
pub use distributions::*;

/// The subdomains the account holds, and the resources they route traffic to.
mod subdomains;
pub use subdomains::*;

/// Released attachments, VPC peering and the account profile.
mod account;
pub use account::*;

#[cfg(test)]
mod tests;
