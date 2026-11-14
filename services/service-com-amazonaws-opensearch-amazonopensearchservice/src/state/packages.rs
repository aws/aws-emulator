// SPDX-License-Identifier: CC0-1.0

//! Packages, split out of [`crate::state`]: a package of this account, its versions, and
//! the association that links one package to one domain.
//!
//! [`Package`] below is the ONE copy every package operation reads and writes, and
//! [`Package::to_details`] the single `PackageDetails` projection they report it through,
//! so no two of them can describe the same package differently. A [`PackageAssociation`]
//! is the one record of a package's link to a domain, which the single and batch
//! associate and dissociate operations all read and write.

use super::*;

/// `PackageStatus`'s `COPYING`: the package's content is being copied out of the
/// Amazon S3 location the create named.
pub const PACKAGE_STATUS_COPYING: &str = "COPYING";

/// `PackageStatus`'s `AVAILABLE`: the package's content is in place and the package
/// can be associated with a domain.
pub const PACKAGE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `PackageStatus`'s `DELETING`: the package is being removed from this account.
///
/// The status a delete leaves the record in, which is the last status any read reports
/// it in: once the clock has taken the record out of the store there is nothing left to
/// report, so `PackageStatus`'s `DELETED` is a status no stored package rests in.
pub const PACKAGE_STATUS_DELETING: &str = "DELETING";

/// `PackageType`'s `ZIP-PLUGIN` value: the only type an engine version may be given
/// for, because `CreatePackageRequest$EngineVersion` "can only be specified for
/// package type ZIP-PLUGIN".
pub const PACKAGE_TYPE_ZIP_PLUGIN: &str = "ZIP-PLUGIN";

/// The `PackageVersion` a package's first available content carries.
///
/// `PackageVersion` is a plain string in the model and the version history is a
/// SEQUENCE, so a package's versions are numbered from one and each update of its
/// content adds the next number.
pub const FIRST_PACKAGE_VERSION: u32 = 1;

/// How many packages this account holds before `CreatePackage` refuses with the
/// modelled `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a
/// caller looping with fresh names from growing the state without end: every record
/// it counts is released by deleting the package. Because the stored set IS what
/// `DescribePackages` answers with, it also bounds the size of that answer.
pub const PACKAGE_QUOTA: usize = 25;

/// How many users one package's scope holds before `UpdatePackageScope` refuses to
/// grant another.
///
/// The emulator's OWN bound, like [`PACKAGE_QUOTA`]: the model puts no length on
/// `PackageUserList`, so without one a caller granting fresh accounts could grow a
/// package's stored scope — and the `AllowListedUserList` every read of that package
/// answers with — without end. Every user it counts is released by withdrawing that
/// user's access.
pub const PACKAGE_SCOPE_QUOTA: usize = 100;

/// How many numbered versions of its content one package holds before
/// `UpdatePackage` refuses to put another in place.
///
/// The emulator's OWN bound, like [`PACKAGE_QUOTA`], and not a published service
/// quota: the service documents no limit on how many versions of its content one
/// package may have had. A package's version sequence only ever grows — an import adds
/// a version and nothing removes one — so without a bound a caller looping updates
/// could grow the stored sequence, and the `PackageVersionHistoryList` every read of
/// that history answers with, without end. A package version is a sub-resource of the
/// package, so the bound is the modelled `LimitExceededException` ("trying to create
/// more than the allowed number of resources or sub-resources"), and the whole
/// sequence is released by deleting the package.
pub const PACKAGE_VERSION_QUOTA: usize = 50;

/// One version of a package's content, as this emulator persists it.
///
/// A package's content is imported, and each import is a new numbered version of it,
/// so the versions a package has had are a SEQUENCE and this is one entry of it. It
/// is the ONE record a version is reported through
/// ([`PackageVersionRecord::to_history`]), the way [`InsightRecord`] is for an
/// insight — so the version a read of the package reports and the entry its history
/// lists for that same version cannot disagree.
#[derive(Debug, Clone)]
pub struct PackageVersionRecord {
    /// `PackageVersionHistory$PackageVersion`: which numbered version of the
    /// package's content this entry is, counting from [`FIRST_PACKAGE_VERSION`].
    pub version: u32,
    /// `PackageVersionHistory$CommitMessage`: "a message associated with the package
    /// version when it was uploaded". Absent where the import that added this version
    /// carried none — which includes the first version of every package, because the
    /// create that imports it declares no `CommitMessage` to give.
    pub commit_message: Option<String>,
    /// `PackageVersionHistory$CreatedAt`: "the date and time when the package was
    /// created", for this version of its content.
    pub created_at: SystemTime,
    /// `PackageVersionHistory$PluginProperties`: "additional information about plugin
    /// properties if the package is a `ZIP-PLUGIN` package". Absent for a package
    /// whose plugin descriptor this emulator never read — every package a CALLER
    /// imports, whose archive is not fetched — and carried by the plugin packages the
    /// service publishes itself, whose properties the catalog states
    /// ([`PublishedPackage`]).
    pub plugin_properties: Option<PluginProperties>,
    /// `PackageVersionHistory$PackageConfiguration`: the configuration stored with
    /// this version of the content, absent where the import carried none.
    pub configuration: Option<PackageConfiguration>,
    /// The CONTENT this version of the package imported, as text — the bytes of the
    /// Amazon S3 object the import named, where this emulator could read them as text.
    ///
    /// A dictionary package's content IS a text file, and what it holds is what an
    /// index's analysis settings read through the package's mount point
    /// ([`crate::state::analysis::package_files`]), so the import keeps it. `None` for a
    /// version whose object the account holds none of, or whose bytes are not text: a
    /// file this process never read contributes no entries, rather than an empty one.
    pub content: Option<String>,
}

impl PackageVersionRecord {
    /// This version as `PackageVersionHistory` — one entry of what
    /// `GetPackageVersionHistory` answers with, so every operation that reports a
    /// package version reports it through the one projection.
    pub fn to_history(&self) -> PackageVersionHistory {
        PackageVersionHistory {
            package_version: Some(self.version.to_string()),
            commit_message: self.commit_message.clone(),
            created_at: Some(datetime(self.created_at)),
            // The plugin's name, class and uncompressed size are the ones recorded for
            // this version: the catalog states them for a package the SERVICE
            // publishes, and a package a caller imported has none, because the
            // descriptor inside its archive is not fetched — so they stay absent there
            // rather than being invented, exactly as
            // `PackageDetails$AvailablePluginProperties` does in [`Package::to_details`].
            plugin_properties: self.plugin_properties.clone(),
            package_configuration: self.configuration.clone(),
        }
    }
}

/// One custom package of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Package {
    /// `PackageDetails$PackageID`: unique per package, so it IS the key the store
    /// holds the record under.
    pub package_id: String,
    /// `PackageDetails$PackageName`: the name the create gave the package, unique
    /// across the account's packages.
    pub package_name: String,
    /// `PackageDetails$PackageType`: one of `PackageType`'s values.
    pub package_type: String,
    /// `PackageDetails$PackageDescription`, absent when the create carried none.
    pub description: Option<String>,
    /// The Amazon S3 location the package's content was imported from. Stored as the
    /// modelled shape rather than a second hand-written mirror of its members; no
    /// read path reports it, because `PackageDetails` declares no member for it.
    pub source: PackageSource,
    /// Where the package is in its lifecycle: one of `PackageStatus`'s values.
    pub status: String,
    /// The status move the clock owes this package, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// When the clock owes this record's REMOVAL from the account's packages, set by
    /// the delete that asked for it and `None` for every package no delete has
    /// addressed — the package mirror of [`PackageAssociation::removal_due`].
    ///
    /// A status move the record survives is [`Self::owed`]; a removal leaves no record
    /// to carry a status at all, so it is held apart and performed by the store that
    /// holds the record ([`ComAmazonawsOpensearchAmazonopensearchserviceState::settle_packages`]).
    pub removal_due: Option<SystemTime>,
    /// Which numbered version of its content the package currently has IN PLACE —
    /// the version a read reports as available and an association puts on a domain,
    /// counting from [`FIRST_PACKAGE_VERSION`]. A version whose content is still
    /// being copied is not in place yet, so this moves to it when that copy lands.
    pub version: u32,
    /// Every version of this package's content, oldest first — the sequence
    /// `GetPackageVersionHistory` reports. A package is minted with its first version
    /// recorded here and each later import of content appends the next, so the newest
    /// entry is the version [`Self::version`] names once that import's copy has
    /// landed. Bounded by [`PACKAGE_VERSION_QUOTA`].
    pub versions: Vec<PackageVersionRecord>,
    /// `PackageDetails$EngineVersion`, absent for a package that is not tied to one.
    pub engine_version: Option<String>,
    /// `PackageDetails$AvailablePackageConfiguration`, absent when the create carried
    /// no configuration.
    pub configuration: Option<PackageConfiguration>,
    /// `PackageDetails$AvailablePluginProperties`: what the plugin inside a
    /// `ZIP-PLUGIN` package is. Carried by the plugin packages the service publishes
    /// itself ([`PublishedPackage`]), whose properties its catalog states, and absent
    /// for a package a CALLER imported, whose archive this emulator does not fetch.
    pub plugin_properties: Option<PluginProperties>,
    /// `PackageDetails$PackageVendingOptions`, absent when the create carried none.
    pub vending_options: Option<PackageVendingOptions>,
    /// `PackageDetails$PackageEncryptionOptions`, absent when the create carried
    /// none.
    pub encryption_options: Option<PackageEncryptionOptions>,
    /// The accounts the package is vended to, as `PackageUser` values. Empty until a
    /// scope update allow-lists one.
    pub allow_listed_users: Vec<String>,
    /// `PackageDetails$PackageOwner`: the account the package belongs to.
    pub owner: String,
    /// `PackageDetails$CreatedAt`.
    pub created_at: SystemTime,
    /// `PackageDetails$LastUpdatedAt`: when the record last changed.
    pub updated_at: SystemTime,
}

impl Package {
    /// Materialise the status move the clock owes this package, if `now` is past its
    /// due instant: a package whose content was being copied becomes one the copy has
    /// left ready to use.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called — which is also why the
    /// record is stamped with the instant the move fell DUE rather than the instant of
    /// the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
        self.updated_at = owed.due;
        // A copy is what puts a version's content in place, so the newest version the
        // history holds becomes the one the package carries once that copy has landed —
        // which is what the move to the available status says has happened.
        if self.status == PACKAGE_STATUS_AVAILABLE {
            if let Some(latest) = self.versions.last() {
                self.version = latest.version;
            }
        }
    }

    /// Start this package's deletion, as the call that asked for it leaves the record.
    ///
    /// The package is not gone the instant that call returns: it is being deleted, so
    /// it rests in [`PACKAGE_STATUS_DELETING`] and the clock owes the record's removal
    /// one [`PROVISIONING`] span later, which
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::settle_packages`] performs
    /// when it falls due. Whatever status move was outstanding is dropped: a package on
    /// its way out is on its way to no other status.
    ///
    /// Idempotent — a package whose removal the clock already owes keeps the instant it
    /// was owed at, so repeating the call cannot push the removal back.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.status = PACKAGE_STATUS_DELETING.to_string();
        self.owed = None;
        self.removal_due = Some(now + PROVISIONING);
        self.updated_at = now;
    }

    /// Whether the removal the clock owed this package has fallen due by `now`, so the
    /// account no longer holds it.
    pub fn removal_landed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Record the NEXT numbered version of this package's content, as the call that
    /// put it in place carried it.
    ///
    /// The ONE writer of the package's version sequence, so the numbering is the same
    /// whichever call adds a version: the first content is
    /// [`FIRST_PACKAGE_VERSION`] and each one after it is one past the version before
    /// it, whatever happened to the versions in between. The version this adds is not
    /// in place until its copy lands ([`Self::settle`]), which is why this does not
    /// move [`Self::version`].
    pub fn add_version(
        &mut self,
        commit_message: Option<String>,
        configuration: Option<PackageConfiguration>,
        created_at: SystemTime,
        content: Option<String>,
    ) {
        let version = self.versions.last().map_or(FIRST_PACKAGE_VERSION, |latest| {
            latest.version.saturating_add(1)
        });
        self.versions.push(PackageVersionRecord {
            version,
            commit_message,
            created_at,
            configuration,
            // A caller's own package carries no plugin descriptor this emulator read,
            // so its versions report none; the ones the catalog publishes are built
            // with their properties in place ([`PublishedPackage::record`]).
            plugin_properties: None,
            content,
        });
    }

    /// The content the version `package_version` names imported, where this emulator read
    /// it.
    ///
    /// Addressed by version because that is what an association pins: a domain reads the
    /// content of the version its association put there, not whatever the package has
    /// imported since.
    pub fn content_at(&self, package_version: &str) -> Option<&str> {
        self.versions
            .iter()
            .find(|version| version.version.to_string() == package_version)?
            .content
            .as_deref()
    }

    /// This record as `PackageDetails` — the shape `CreatePackage` answers with and
    /// the element shape `DescribePackages` reports, so a create's answer and a later
    /// read of the package cannot disagree.
    pub fn to_details(&self) -> PackageDetails {
        PackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(self.package_name.clone()),
            package_type: Some(self.package_type.clone()),
            package_description: self.description.clone(),
            package_status: Some(self.status.clone()),
            created_at: Some(datetime(self.created_at)),
            last_updated_at: Some(datetime(self.updated_at)),
            // "The package version", meaning the version a domain association would
            // get: there is one only once the content is in place.
            available_package_version: (self.status == PACKAGE_STATUS_AVAILABLE)
                .then(|| self.version.to_string()),
            // "Additional information if the package is in an error state" — this
            // package is not, so there is nothing to report.
            error_details: None,
            engine_version: self.engine_version.clone(),
            // The plugin's name, class and uncompressed size are the catalog's own for
            // a package the SERVICE publishes; for one a caller imported they are read
            // out of the descriptor inside the plugin's archive, which this emulator
            // does not fetch, so they stay absent there rather than being invented.
            available_plugin_properties: self.plugin_properties.clone(),
            available_package_configuration: self.configuration.clone(),
            // "Lists the accounts that the package was shared with": reported only
            // once the package has been shared with one.
            allow_listed_user_list: (!self.allow_listed_users.is_empty())
                .then(|| self.allow_listed_users.clone()),
            package_owner: Some(self.owner.clone()),
            package_vending_options: self.vending_options.clone(),
            package_encryption_options: self.encryption_options.clone(),
        }
    }
}

/// How much of an imported object's bytes one package version keeps.
///
/// The emulator's OWN bound on what a create or an import copies into this process: the
/// content is read to be READ BACK by an index's analysis chain, and a dictionary file is
/// a word list, so a package keeps up to this much of the object and the content past it
/// is not imported. Nothing here is a published service quota.
pub const MAX_PACKAGE_CONTENT_BYTES: usize = 1 << 20;

/// The content the Amazon S3 location `source` names, as text, read from the store Amazon
/// S3 itself holds.
///
/// `CreatePackage` and `UpdatePackage` "import" the package's content from an S3 bucket and
/// key, and the content of a TXT-DICTIONARY package is what an index's analysis settings
/// read through the package's mount point — so the import reads the OBJECT, from the
/// service that owns it, rather than inventing content for the package.
///
/// `None` wherever nothing can be read: no S3 store on this engine, no such bucket or key,
/// or bytes that are not text. A package whose content could not be read keeps none, which
/// is what [`Package::content_at`] reports and what a filter reading its file is told.
///
/// Reads ONLY, and reads ANOTHER service's store, so it is called before this service's
/// own store is locked: no call holds two service stores at once.
pub fn imported_content<Eng: Send + Sync + 'static>(
    engine: &Eng,
    source: &PackageSource,
) -> Option<String> {
    let (bucket, key) = (source.s3_bucket_name.as_ref()?, source.s3_key.as_ref()?);
    let objects = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = objects.lock();
    let body = &store.bucket(bucket)?.object(key)?.body;
    let read = body.len().min(MAX_PACKAGE_CONTENT_BYTES);
    let prefix = body.get(..read)?;
    match ::std::str::from_utf8(prefix) {
        Ok(text) => Some(text.to_owned()),
        // A cut at the byte bound can fall inside a character, which is the ONE error that
        // says nothing about the content: everything before it was read, so that is what
        // the package keeps. Bytes that are not text at all leave the package with no
        // content rather than with a prefix of something this emulator cannot read.
        Err(invalid) if invalid.error_len().is_none() && read < body.len() => {
            let valid = prefix.get(..invalid.valid_up_to())?;
            ::std::str::from_utf8(valid).ok().map(str::to_owned)
        }
        Err(_) => None,
    }
}

/// The `PackageID` a create mints.
///
/// `PackageID` is pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`: the `[FG][0-9]+` form
/// addresses a package the service itself publishes, so a package a CALLER creates
/// takes the `pkg-` form, whose body is lowercase hexadecimal. The body is a v4 UUID
/// in its unhyphenated rendering — the same generator as every other id this service
/// mints ([`new_reserved_instance_id`]) in the only rendering this pattern admits —
/// so no two packages share an id.
pub fn new_package_id() -> String {
    format!("pkg-{}", ::uuid::Uuid::new_v4().simple())
}

/// How this service states that a package the account already holds cannot be created
/// again: the request asks for the package to be in a state it is already in. One fixed
/// sentence with no variable part — it names no package, because the condition is about
/// the create and not about which name it carried.
pub const PACKAGE_ALREADY_IN_DESIRED_STATE: &str = "Package is already in desired state";

/// The `BaseException` payload for a create naming a package the account already holds.
///
/// `CreatePackage` models `ResourceAlreadyExistsException` (409) and the generic
/// `BaseException` envelope (400, defaulted) alike
/// (`smithy_query(query="operation",operation="CreatePackage")`), and the refusal
/// recorded for a second create of one name is the generic one.
pub fn package_name_exists() -> BaseException {
    BaseException {
        message: Some(PACKAGE_ALREADY_IN_DESIRED_STATE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Package associations
//
// An association is the LINK between one domain and one package of this
// account: `AssociatePackages` makes one and `ListPackagesForDomain` reports
// it. It is held on the [`Domain`] it links, under the `PackageID` of the
// package it links to, so no map beside the domain can disagree about which
// packages that domain has, and releasing the domain releases its associations.
//
// The association carries only what belongs to the LINK — its lifecycle status,
// the package version it put on the domain, its prerequisites and its
// configuration. The package's own name and type are read from the one stored
// [`Package`] record through [`PackageAssociation::to_details`], so a package
// operation and an association one cannot report the same package differently.
// ---------------------------------------------------------------------------

/// `DomainPackageStatus`'s `ASSOCIATING`: the association has been asked for and
/// the package is being put in place on the domain.
pub const DOMAIN_PACKAGE_STATUS_ASSOCIATING: &str = "ASSOCIATING";

/// `DomainPackageStatus`'s `ACTIVE`: the package is in place on the domain, at the
/// version the association carries.
pub const DOMAIN_PACKAGE_STATUS_ACTIVE: &str = "ACTIVE";

/// `DomainPackageStatus`'s `DISSOCIATING`: the package is being removed from the
/// domain.
pub const DOMAIN_PACKAGE_STATUS_DISSOCIATING: &str = "DISSOCIATING";

/// `DomainPackageStatus`'s `DISSOCIATION_FAILED`: the removal was attempted and did not
/// happen, so the package is still on the domain — the status an association rests in
/// when the domain cannot let go of the package.
pub const DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED: &str = "DISSOCIATION_FAILED";

/// `ErrorDetails$ErrorMessage` for a removal the domain could not perform because one
/// of its indexes still resolves a file by the package's reference path.
///
/// The service's own fixed sentence for that condition: it carries no value of the
/// request and names neither the package nor the index.
pub const PACKAGE_IN_USE: &str = "Operation not allowed. Package is in use.";

/// The message every refusal of a `PackageID` that resolves to no package of this
/// account carries ([`package_not_found`]).
///
/// The service's own fixed sentence for that condition: it is the WHOLE message and
/// has no variable part, so the id the call could not resolve does not appear in it.
pub const PACKAGE_NOT_FOUND: &str = "Package not found.";

/// The message a batch package operation refuses a request that names the same package
/// more than once with ([`duplicate_packages_in_request`]).
///
/// The service's own fixed sentence for that condition: it is the WHOLE message and has
/// no variable part, so neither the member that repeated a package nor the repeated id
/// appears in it.
pub const DUPLICATE_PACKAGES_IN_REQUEST: &str =
    "Operation not allowed. Duplicate packages found in the request.";

/// The message the single-package dissociation refuses a `DomainName` it cannot
/// resolve with ([`domain_not_on_latest_software_version`]).
///
/// That call reaches the domain through the capability gate this feature sits behind
/// rather than through a plain existence check, so a name it cannot resolve is
/// reported as a domain that cannot serve the feature. The service's own fixed
/// sentence for that condition: it is the WHOLE message, trailing space included, and
/// has no variable part — neither the domain nor the package appears in it.
pub const DOMAIN_NOT_ON_LATEST_SOFTWARE_VERSION: &str =
    "The domain is not on the latest software version. Upgrade the domain to start using \
     this feature. ";

/// The message a dissociation of a package the domain is not holding is refused with
/// ([`package_not_associated`]).
///
/// The service's own fixed sentence for that condition: the whole message, with no
/// variable part — neither the package nor the domain it is not on appears in it.
pub const PACKAGE_NOT_ASSOCIATED: &str = "Package is not associated to this domain.";

/// `ErrorDetails$ErrorType` for an association whose last transition failed — "the type
/// of error that occurred".
///
/// A transition the domain cannot perform because the package is still being used by
/// one of its indexes is the condition this API models as `ConflictException` ("the
/// client attempts to remove a resource that is currently in use"), so that is the
/// error class the details name.
pub const DOMAIN_PACKAGE_ERROR_TYPE_IN_USE: &str = "ConflictException";

/// `PackageType`'s `TXT-DICTIONARY` value: the dictionary content a domain's index
/// settings reference BY PATH, which is what gives an association of it a
/// `ReferencePath`.
pub const PACKAGE_TYPE_TXT_DICTIONARY: &str = "TXT-DICTIONARY";

/// The directory a domain's index settings reference an associated dictionary
/// package under, from the form the service documents for that reference
/// (`analyzers/<PackageID>`).
pub(super) const ANALYZERS_DIRECTORY: &str = "analyzers";

/// One link between a domain and a package of this account, as this emulator
/// persists it.
#[derive(Debug, Clone)]
pub struct PackageAssociation {
    /// The `PackageID` of the package this association links to, which is also the
    /// key the domain holds it under.
    pub package_id: String,
    /// Where the association is in its lifecycle: one of `DomainPackageStatus`'s
    /// values.
    pub status: String,
    /// The status move the clock owes this association, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// `DomainPackageDetails$PackageVersion`: the version of the package's content
    /// this association put on the domain, which is the version the package had
    /// available when the association was asked for. A package whose content is
    /// updated does not change what a domain already has: a later association is
    /// what moves the domain onto the newer version.
    pub package_version: String,
    /// `DomainPackageDetails$PrerequisitePackageIDList`: the packages this one
    /// needed on the domain, as the call that made the association named them.
    pub prerequisite_package_ids: Vec<String>,
    /// `DomainPackageDetails$AssociationConfiguration`, absent where the call that
    /// made the association carried none.
    pub association_configuration: Option<PackageAssociationConfiguration>,
    /// `DomainPackageDetails$LastUpdated`: "timestamp of the most recent update to
    /// the package association status".
    pub last_updated: SystemTime,
    /// When the clock takes this association OFF the domain, for one a dissociation
    /// call put into its removal status; `None` for one no call has asked to remove.
    ///
    /// A removal needs its own due instant rather than an [`OwedStatus`] because
    /// `DomainPackageStatus` has no value for a package that is no longer on the
    /// domain: the record rests in [`DOMAIN_PACKAGE_STATUS_DISSOCIATING`] until the
    /// span has passed and the removal then either takes it off the domain or fails,
    /// leaving it there ([`Domain::settle_package_associations`]).
    pub removal_due: Option<SystemTime>,
    /// `DomainPackageDetails$ErrorDetails`: "additional information if the package is
    /// in an error state. Null otherwise." — so it is populated exactly while this
    /// association's last transition left it in one, and cleared by the next
    /// transition that lands.
    pub error_details: Option<ErrorDetails>,
}

impl PackageAssociation {
    /// Materialise the status move the clock owes this association, if `now` is past
    /// its due instant: a package that was being put in place on the domain becomes
    /// one that is in place there.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called — which is why the
    /// record is stamped with the instant the move fell DUE rather than the instant
    /// of the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
        self.last_updated = owed.due;
        // A move that LANDED is not an error state: whatever an earlier transition of
        // this association failed at is not this one's.
        self.error_details = None;
    }

    /// Whether this association still has a status move outstanding, meaning the
    /// change asked for by the call that last touched it has not landed yet.
    ///
    /// Read AFTER [`Self::settle`], so a move that has merely fallen due does not
    /// read as one still in flight.
    pub fn in_flight(&self) -> bool {
        self.owed.is_some() || self.removal_due.is_some()
    }

    /// Whether the package's content is in place on the domain for an index's analysis
    /// settings to read a file from.
    ///
    /// The content is there from the moment being put in place has landed until the
    /// record leaves the domain: an association still `ASSOCIATING` has not put it
    /// there yet, and one whose removal failed still has it there, which is what keeps
    /// a dependent index working.
    pub fn in_place(&self) -> bool {
        matches!(
            self.status.as_str(),
            DOMAIN_PACKAGE_STATUS_ACTIVE
                | DOMAIN_PACKAGE_STATUS_DISSOCIATING
                | DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED
        )
    }

    /// This record as `DomainPackageDetails` — the shape every operation that
    /// reports an association answers with, so an association's own answer and a
    /// later read of it cannot disagree.
    ///
    /// `package` is the stored record of the package this association links to: its
    /// name and type belong to the PACKAGE, so they are read from there rather than
    /// copied onto the association.
    pub fn to_details(&self, domain_name: &str, package: &Package) -> DomainPackageDetails {
        DomainPackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(package.package_name.clone()),
            package_type: Some(package.package_type.clone()),
            last_updated: Some(datetime(self.last_updated)),
            domain_name: Some(domain_name.to_string()),
            domain_package_status: Some(self.status.clone()),
            package_version: Some(self.package_version.clone()),
            // "A list of package IDs that must be associated with the domain before
            // or with the package can be associated": reported only where the
            // association has any.
            prerequisite_package_id_list: (!self.prerequisite_package_ids.is_empty())
                .then(|| self.prerequisite_package_ids.clone()),
            reference_path: package_reference_path(package),
            // "Additional information if the package is in an error state. Null
            // otherwise." — the stored details of the transition that left this
            // association in one, and absent for every association that is not.
            error_details: self.error_details.clone(),
            association_configuration: self.association_configuration.clone(),
        }
    }
}

/// `DomainPackageDetails$ReferencePath`: "the relative path of the package on the
/// OpenSearch Service cluster nodes. This is `synonym_path` when the package is for
/// synonym files."
///
/// Dictionary content is the kind a domain's index settings name by path, and the
/// documented form of that reference is the analyzers directory followed by the
/// package's id — so the path is DERIVED from the stored package rather than
/// invented per call. Content of any other type is loaded onto the nodes rather
/// than referenced by an index setting, so it has no such path.
pub fn package_reference_path(package: &Package) -> Option<String> {
    (package.package_type == PACKAGE_TYPE_TXT_DICTIONARY)
        .then(|| format!("{ANALYZERS_DIRECTORY}/{}", package.package_id))
}

/// The refusal every package operation answers a `PackageID` that resolves to no
/// package of this account with.
///
/// The service treats such an id as a bad input VALUE rather than as a resource that
/// is missing: a package that was deleted leaves no record behind, so there is no
/// state for the call to conflict with and nothing about the request will ever make
/// it succeed — which is the client-input refusal (HTTP 400) and not the
/// conflict-shaped missing-resource one (HTTP 409). Its wording is the service's
/// fixed sentence for that condition ([`PACKAGE_NOT_FOUND`]), which carries no value
/// of the request: the rejected id is NOT interpolated into it.
pub fn package_not_found() -> ValidationException {
    ValidationException {
        message: Some(PACKAGE_NOT_FOUND.to_string()),
    }
}

/// The `ConflictException` payload for associating a package whose content is not in
/// place: a package can be put on a domain only once its copy has left it
/// [`PACKAGE_STATUS_AVAILABLE`], so a call made before that conflicts with the
/// status the package is in, and names it.
pub fn package_not_available(package_id: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Package {package_id} is {status}, not {PACKAGE_STATUS_AVAILABLE}."
        )),
    }
}

/// The `ValidationException` payload for deleting a package a domain still holds.
///
/// `DeletePackageRequest` states the constraint the delete is judged by — the package
/// "can't be associated with any OpenSearch Service domain" — so a package still in
/// place somewhere is not one this call can remove, and the refusal is the service's
/// fixed sentence for a package that cannot be let go while it is in use
/// ([`PACKAGE_IN_USE`]), which carries no value of the request.
pub fn package_in_use() -> ValidationException {
    ValidationException {
        message: Some(PACKAGE_IN_USE.to_string()),
    }
}

/// The refusal payload for a dissociation of a package the domain is not holding: the
/// link the call asks to remove does not exist, so there is nothing for it to act on.
///
/// A request naming a relationship that cannot be dissolved is refused as a bad
/// REQUEST rather than as a missing resource: both the package and the domain it
/// names exist, and it is the link between them the service has nothing to act on.
/// Its wording is the service's fixed sentence for that condition
/// ([`PACKAGE_NOT_ASSOCIATED`]), which carries no value of the request.
pub fn package_not_associated() -> ValidationException {
    ValidationException {
        message: Some(PACKAGE_NOT_ASSOCIATED.to_string()),
    }
}

/// The `InternalException` payload for a store that no longer holds what the decision
/// this call already made had resolved — a state the one lock this service takes makes
/// unreachable.
///
/// "Request processing failed because of an unknown error, exception, or internal
/// failure": such a write is not something the CALLER can fix, so it is answered on
/// the server channel rather than reported as a bad request.
pub fn internal_failure(what: &str) -> InternalException {
    InternalException {
        message: Some(format!(
            "Request processing failed: {what} changed while the request was being served."
        )),
    }
}

/// The `ConflictException` payload for an associate or dissociate call made while
/// another change to one of the domain's package associations has not landed yet.
///
/// The service runs these changes on a domain ONE at a time, so the refusal is about
/// the domain rather than about either package, and its wording is the service's own
/// constant for that condition — a fixed sentence every caller that hits it gets,
/// carrying no value of the request.
pub fn concurrent_package_operation() -> ConflictException {
    ConflictException {
        message: Some(
            "Operation not allowed. A concurrent operation (associate/dissociate) is in \
             progress for this domain."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for a prerequisite that the domain does not
/// have and the call does not associate either.
///
/// A prerequisite "must be linked to the domain before or simultaneously with the
/// package association", so the refusal names both packages and the domain the link
/// is missing on.
pub fn prerequisite_not_associated(
    package_id: &str,
    prerequisite: &str,
    domain_name: &str,
) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "PrerequisitePackageIDList of package {package_id} names {prerequisite}, which is \
             not associated with domain {domain_name} and not associated by this request."
        )),
    }
}

/// The `ValidationException` payload for a package-association call whose
/// `DomainName` names no domain this account holds.
///
/// Linking a package resolves the domain as part of READING the request: a name that
/// resolves to no domain is a value the caller has to fix, so the refusal is the
/// client-input one rather than the missing-resource one, and it names the domain
/// that could not be resolved. Compare [`domain_not_found`], which is what a call
/// that ADDRESSES a domain (describing it, changing it, deleting it) answers.
pub fn domain_does_not_exist(domain_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("The domain {domain_name} does not exist.")),
    }
}

/// The `ValidationException` payload the single-package dissociation answers a
/// `DomainName` that names no domain of this account with.
///
/// Unlinking one package resolves the domain behind the capability gate the feature
/// sits behind, so a name that resolves to nothing is reported as a domain that cannot
/// serve the call ([`DOMAIN_NOT_ON_LATEST_SOFTWARE_VERSION`]) rather than as a value
/// that names no domain: the wording carries no part of the request. Compare
/// [`domain_does_not_exist`], which is what the association calls answer the same
/// condition with.
pub fn domain_not_on_latest_software_version() -> ValidationException {
    ValidationException {
        message: Some(DOMAIN_NOT_ON_LATEST_SOFTWARE_VERSION.to_string()),
    }
}

/// The `ValidationException` payload for a batch that names the same package more than
/// once: a domain holds ONE association per package, so a request that names it twice
/// asks for two states of one association and is refused as a WHOLE.
///
/// Its wording is the service's fixed sentence for that condition
/// ([`DUPLICATE_PACKAGES_IN_REQUEST`]), which carries no value of the request: a caller
/// is told its batch repeats a package, not which id it repeated.
pub fn duplicate_packages_in_request() -> ValidationException {
    ValidationException {
        message: Some(DUPLICATE_PACKAGES_IN_REQUEST.to_string()),
    }
}

/// The `ValidationException` payload for a package that names ITSELF among the
/// packages that must be associated before it: no association can precede its own.
pub fn prerequisite_is_self(package_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "PrerequisitePackageIDList of package {package_id} names that same package."
        )),
    }
}

/// What a request asks of ONE association: which package to link to the domain,
/// the packages that must be linked before it, and the configuration to store with
/// the link.
///
/// Both operations that make an association — the one that links a single package
/// and the one that links several at once — describe each link they make with this,
/// so the two cannot ask for different things from the same input.
#[derive(Debug, Clone)]
pub struct AssociationRequest {
    /// The `PackageID` of the package to link, as the request named it.
    pub package_id: String,
    /// The request's `PrerequisitePackageIDList`, empty where it carried none.
    pub prerequisite_package_ids: Vec<String>,
    /// The request's `AssociationConfiguration`, absent where it carried none.
    pub association_configuration: Option<PackageAssociationConfiguration>,
}

/// Why a package cannot be linked to a domain, or unlinked from one, as the
/// modelled exception payload the refusal is reported through.
///
/// Each operation declares its OWN error enum, so this names the payload and the
/// handler lifts it into its own channel: the DECISION lives in one place even
/// though the error channels do not.
#[derive(Debug)]
pub enum AssociationRefusal {
    /// The call's `DomainName` resolves to no domain of this account, carried as the
    /// name that could not be resolved.
    ///
    /// Reported on its OWN variant because the WORDING of this refusal belongs to the
    /// operation that emits it, not to the decision: the shared decision reports the
    /// condition, and each handler answers it with the sentence the service words that
    /// operation's refusal with. The class (client input, HTTP 400) is the same for
    /// all of them.
    DomainUnresolved(String),
    /// The package or the association is in a status that cannot serve the call.
    Conflict(ConflictException),
    /// The request is not one the domain can serve as asked — a value it carries
    /// (the domain, the package, one of its prerequisites) resolves to nothing, the
    /// link it names is not there, or the request contradicts itself.
    Validation(ValidationException),
}
