// SPDX-License-Identifier: CC0-1.0

//! The prefix lists (the ones the account creates and the ones AWS publishes for a Region),
//! the dedicated-host store, and the endpoint services a Region offers.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The `ResourceType` a `CreateManagedPrefixList` tag specification may name — "The tags to
/// apply to the prefix list during creation"
/// (`CreateManagedPrefixListRequest$TagSpecifications`) — from the modelled catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES` carries `prefix-list`). It is also the type
/// segment of a prefix list's ARN.
pub const RESOURCE_TYPE_PREFIX_LIST: &str = "prefix-list";

/// The `AddressFamily` of an IPv4 managed prefix list: "The IP address type. Valid Values:
/// IPv4 | IPv6" (`CreateManagedPrefixListRequest$AddressFamily`).
///
/// Spelled with the capitals that member documents, which is NOT how IPAM's own modelled
/// `AddressFamily` enum spells its members ([`ADDRESS_FAMILY_IPV4`] is `ipv4`): the prefix
/// list member is a plain string with a documented value list, so the documented spelling is
/// the one a caller sends and the one a describe reports.
pub const PREFIX_LIST_ADDRESS_FAMILY_IPV4: &str = "IPv4";

/// The `AddressFamily` of an IPv6 managed prefix list, from that same value list.
pub const PREFIX_LIST_ADDRESS_FAMILY_IPV6: &str = "IPv6";

/// Every `AddressFamily` a managed prefix list may have, in the order that member documents
/// them. A value outside this list is refused rather than stored.
pub const PREFIX_LIST_ADDRESS_FAMILIES: [&str; 2] = [
    PREFIX_LIST_ADDRESS_FAMILY_IPV4,
    PREFIX_LIST_ADDRESS_FAMILY_IPV6,
];

/// A prefix list whose creation has been asked for and has not settled yet: the
/// `PrefixListState` enum's `create-in-progress` member
/// ([`crate::ops::types::PREFIX_LIST_STATE_VALUES`]), which is the state
/// `CreateManagedPrefixList` answers with.
pub const PREFIX_LIST_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// A prefix list whose creation has settled: that enum's `create-complete` member, which the
/// clock owes a [`PREFIX_LIST_STATE_CREATE_IN_PROGRESS`] list one [`TRANSITION_DELAY`] after
/// the call that created it — the same contract as a `create-in-progress` IPAM scope's move.
pub const PREFIX_LIST_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A prefix list whose deletion has been asked for and has not been carried out yet: that
/// enum's `delete-in-progress` member, which is what `DeleteManagedPrefixList` answers with.
pub const PREFIX_LIST_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// The `Version` a newly created prefix list carries: "The version of the prefix list"
/// (`ManagedPrefixList$Version`) counts the revisions of its entry set, and a create is the
/// first of them.
pub const FIRST_PREFIX_LIST_VERSION: i64 = 1;

/// How long a prefix list's name may be: "A name for the prefix list. Constraints: Up to 255
/// characters in length" (`CreateManagedPrefixListRequest$PrefixListName`).
pub const MAX_PREFIX_LIST_NAME_CHARS: usize = 255;

/// The name prefix a caller's own prefix list may not take: "The name cannot start with
/// com.amazonaws" (`CreateManagedPrefixListRequest$PrefixListName`) — that is the namespace
/// the Amazon Web Services SERVICE prefix lists of a Region are named in
/// ([`service_prefix_lists_in_region`]).
pub const RESERVED_PREFIX_LIST_NAME_PREFIX: &str = "com.amazonaws";

/// How long one entry's description may be: "A description for the entry. Constraints: Up to
/// 255 characters in length" (`AddPrefixListEntry$Description`).
pub const MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS: usize = 255;

/// The smallest `MaxEntries` a prefix list may be created with: "The maximum number of
/// entries for the prefix list" (`CreateManagedPrefixListRequest$MaxEntries`) — a list that
/// may hold no entry at all is not a prefix list, so a create below this is refused.
pub const MIN_PREFIX_LIST_MAX_ENTRIES: i32 = 1;

/// The largest number of managed prefix lists this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_CARRIER_GATEWAYS`], not an AWS quota: each
/// list is a record in this process's memory, so the number a caller can create has to be
/// finite. [`ComAmazonawsEc2Amazonec2State::has_managed_prefix_list_capacity`] is the
/// predicate a writer asks first; a create past it refuses with
/// [`PREFIX_LIST_LIMIT_EXCEEDED`].
pub const MAX_MANAGED_PREFIX_LISTS: usize = 1_000;

/// The error code a create refuses with once the store is full, in the form EC2 names every
/// quota refusal with (`VpcLimitExceeded`, `CarrierGatewayLimitExceeded`):
/// `<Resource>LimitExceeded`. The ceiling it reports is this store's
/// ([`MAX_MANAGED_PREFIX_LISTS`]), not a published AWS quota.
pub const PREFIX_LIST_LIMIT_EXCEEDED: &str = "PrefixListLimitExceeded";

/// EC2's error code for a request that named a prefix list id no prefix list of the account
/// or of the Region has, spelled the way this service spells its other id-not-found codes
/// ([`INVALID_VPC_ID_NOT_FOUND`], `InvalidSubnetID.NotFound`).
pub const INVALID_PREFIX_LIST_ID_NOT_FOUND: &str = "InvalidPrefixListID.NotFound";

/// Refuse a request that named a prefix list id nothing the caller can see has: HTTP 400
/// [`INVALID_PREFIX_LIST_ID_NOT_FOUND`].
///
/// Built by [`id_not_found`], this crate's one writer of an `Invalid<Kind>.NotFound`, so the
/// wording of the prefix list surface's not-found answer cannot drift from the rest of the
/// service's.
pub fn invalid_prefix_list_id_not_found<E>(prefix_list_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_PREFIX_LIST_ID_NOT_FOUND,
        "prefix list ID",
        prefix_list_id,
    )
}

/// Refuse a create that would take the store past [`MAX_MANAGED_PREFIX_LISTS`]: HTTP 400
/// [`PREFIX_LIST_LIMIT_EXCEEDED`].
pub fn prefix_list_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(PREFIX_LIST_LIMIT_EXCEEDED, 400, message)
}

/// Does the Region `region` PUBLISH a prefix list carrying the id `prefix_list_id`?
///
/// The one question an operation that writes a prefix list asks about an id its own store holds
/// no list for: "Describes your managed prefix lists and any Amazon Web Services-managed prefix
/// lists" (`DescribeManagedPrefixLists`), so such a list is visible to the caller but is not the
/// account's to change. Answered off [`managed_prefix_lists_in_region`], the one derivation of
/// the set a Region publishes.
pub fn region_publishes_prefix_list(region: &str, prefix_list_id: &str) -> bool {
    managed_prefix_lists_in_region(region)
        .iter()
        .any(|published| published.prefix_list_id == prefix_list_id)
}

/// Refuse a change to a prefix list the REGION publishes: HTTP 400 `UnsupportedOperation`,
/// naming the list and the change it cannot take (`modified`, `deleted`).
///
/// "The ID of the owner of the prefix list" (`ManagedPrefixList$OwnerId`) is Amazon Web Services
/// itself for such a list, so this is a refusal about the RESOURCE rather than about the id —
/// which is why it is not the not-found an unresolvable id earns. The ONE wording of it, so the
/// operations that refuse it cannot word the same refusal differently.
pub fn published_prefix_list_unsupported<E>(prefix_list_id: &str, change: &str) -> Scaffolded<E> {
    unsupported_operation(format!(
        "The prefix list '{prefix_list_id}' is an Amazon Web Services-managed prefix list, \
         which cannot be {change}"
    ))
}

/// What one managed prefix list is created with, beyond everything the service itself derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_managed_prefix_list`] mints the id, builds the
/// ARN, fills in the owning account, and sets the first version and the state the list starts
/// in).
#[derive(Debug, Clone, Default)]
pub struct ManagedPrefixListSpec {
    /// "A name for the prefix list" (`CreateManagedPrefixListRequest$PrefixListName`).
    pub prefix_list_name: String,
    /// One of [`PREFIX_LIST_ADDRESS_FAMILIES`].
    pub address_family: String,
    /// "The maximum number of entries for the prefix list"
    /// (`CreateManagedPrefixListRequest$MaxEntries`).
    pub max_entries: i32,
    /// "One or more entries for the prefix list"
    /// (`CreateManagedPrefixListRequest$Entries`), already read into the shape
    /// `GetManagedPrefixListEntries` reports them as.
    pub entries: Vec<crate::ops::types::PrefixListEntry>,
    /// "The tags to apply to the prefix list during creation"
    /// (`CreateManagedPrefixListRequest$TagSpecifications`).
    pub tags: Vec<Tag>,
    /// The Region the request was made in, which the list's ARN names.
    pub region: String,
    /// The `ClientToken` the request carried, if any: the token map keeps it for as long as
    /// the list lives, so the bytes it holds are charged with the list's.
    pub client_token: Option<String>,
}

/// What the clock owes one stored managed prefix list next.
///
/// A word of the modelled `PrefixListState` catalog for a move that leaves the list stored,
/// and [`ManagedPrefixListMove::Removed`] for the one that does not: the catalog's
/// `delete-complete` member is the deletion HAVING BEEN CARRIED OUT, and a list whose deletion
/// has been carried out is not one `DescribeManagedPrefixLists` reports at all — the same
/// reading as [`PrefixListReferenceMove::Removed`] has of a route that is no longer in its
/// table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedPrefixListMove {
    /// The state of the catalog the list settles into:
    /// [`PREFIX_LIST_STATE_CREATE_COMPLETE`] after a create, `modify-complete` after a
    /// revision.
    Settled(&'static str),
    /// The list is reported no longer: the record goes out of the store, its caller-supplied
    /// bytes go back to the budget and the client token that named it is forgotten.
    Removed,
}

/// The move the clock owes one managed prefix list: a [`ClockTransition`] to the next
/// [`ManagedPrefixListMove`].
pub type ManagedPrefixListTransition = ClockTransition<ManagedPrefixListMove>;

/// One stored managed prefix list: the modelled shape every operation that reports one
/// answers with, plus the facts that shape carries no member for.
#[derive(Debug, Clone)]
pub struct ManagedPrefixListRecord {
    /// The list as `CreateManagedPrefixList` answered it and `DescribeManagedPrefixLists`
    /// reports it.
    pub view: crate::ops::types::ManagedPrefixList,
    /// The entries of the list's CURRENT version, in the order the request named them. The
    /// modelled [`crate::ops::types::ManagedPrefixList`] carries no member for them —
    /// `GetManagedPrefixListEntries` is what reports them, through
    /// [`ComAmazonawsEc2Amazonec2State::visible_prefix_list_entries`] — so the entries a
    /// create accepted are kept here rather than dropped.
    pub entries: Vec<crate::ops::types::PrefixListEntry>,
    /// The move the clock owes the list — the `create-in-progress` -> `create-complete`
    /// settle, the one a revision starts, or the REMOVAL a delete starts — until
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it. `None` on a
    /// list that has nothing owed.
    pub pending: Option<ManagedPrefixListTransition>,
}

/// The caller-chosen bytes one stored managed prefix list holds for the process's lifetime:
/// its name, its entries' CIDRs and descriptions, its tags and the client token the token map
/// keeps. The id, the ARN, the owning account, the state and the version are derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_managed_prefix_list`], so they are bounded already
/// and not counted here.
///
/// The ONE definition of that charge, the way [`carrier_gateway_payload_bytes`] is for a
/// carrier gateway: the create asks it before the write, off the values it has in hand, and
/// that insert charges exactly it.
pub fn managed_prefix_list_payload_bytes(
    prefix_list_name: &str,
    entries: &[crate::ops::types::PrefixListEntry],
    tags: &[Tag],
    client_token: Option<&str>,
) -> usize {
    let entry_bytes: usize = entries
        .iter()
        .map(|entry| {
            entry.cidr.as_ref().map_or(0, String::len)
                + entry.description.as_ref().map_or(0, String::len)
        })
        .sum();
    tag_list_bytes(tags)
        .saturating_add(prefix_list_name.len())
        .saturating_add(entry_bytes)
        .saturating_add(client_token.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another managed prefix list fits inside the store's ceiling
    /// ([`MAX_MANAGED_PREFIX_LISTS`]).
    ///
    /// The same contract as [`Self::has_carrier_gateway_capacity`]: a create asks this first
    /// and refuses with [`prefix_list_limit_exceeded`] when it answers `false`.
    pub fn has_managed_prefix_list_capacity(&self) -> bool {
        self.managed_prefix_lists.len() < MAX_MANAGED_PREFIX_LISTS
    }

    /// Insert a managed prefix list, and answer the `PrefixListId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller —
    /// the id, the ARN, the owning account, the first version and the state the list starts
    /// in — so two writers cannot report the same list differently. The caller-chosen bytes
    /// the record retains ([`managed_prefix_list_payload_bytes`]) are charged against the
    /// store's byte budget here, so a writer cannot store them uncharged, and the
    /// `create-complete` move the clock owes the list is recorded with it rather than slept
    /// for.
    pub fn insert_managed_prefix_list(&mut self, spec: ManagedPrefixListSpec) -> String {
        let prefix_list_id = self.next_id(PREFIX_LIST_ID_PREFIX);
        self.add_stored_bytes(managed_prefix_list_payload_bytes(
            &spec.prefix_list_name,
            &spec.entries,
            &spec.tags,
            spec.client_token.as_deref(),
        ));
        let record = ManagedPrefixListRecord {
            view: crate::ops::types::ManagedPrefixList {
                prefix_list_id: Some(prefix_list_id.clone()),
                address_family: Some(spec.address_family),
                state: Some(PREFIX_LIST_STATE_CREATE_IN_PROGRESS.to_string()),
                state_message: None,
                prefix_list_arn: Some(::engine::emulator_arn(
                    "ec2",
                    &spec.region,
                    &format!("{RESOURCE_TYPE_PREFIX_LIST}/{prefix_list_id}"),
                )),
                prefix_list_name: Some(spec.prefix_list_name),
                max_entries: Some(spec.max_entries),
                version: Some(FIRST_PREFIX_LIST_VERSION),
                tags: Some(spec.tags),
                owner_id: Some(::engine::emulator_account_id().to_string()),
                ipam_prefix_list_resolver_target_id: None,
                ipam_prefix_list_resolver_sync_enabled: None,
            },
            entries: spec.entries,
            pending: Some(ManagedPrefixListTransition::starting_now(
                ManagedPrefixListMove::Settled(PREFIX_LIST_STATE_CREATE_COMPLETE),
            )),
        };
        if let Some(token) = spec.client_token {
            self.managed_prefix_list_client_tokens
                .insert(token, prefix_list_id.clone());
        }
        self.managed_prefix_lists
            .insert(prefix_list_id.clone(), record);
        prefix_list_id
    }

    /// Start the delete of the managed prefix list `prefix_list_id`, and answer it as it now
    /// stands — or `None` when the store holds no list with that id.
    ///
    /// The ONE place a prefix list's delete is started, on the same contract as
    /// [`Self::begin_transit_gateway_prefix_list_reference_delete`]: the list goes into
    /// [`PREFIX_LIST_STATE_DELETE_IN_PROGRESS`] NOW, with its REMOVAL owed to the clock one
    /// [`TRANSITION_DELAY`] later ([`ManagedPrefixListMove::Removed`], applied by
    /// [`Self::materialize_due_transitions`], which is what takes the record out). The span is
    /// never zero, so a caller reading straight after the delete still sees the list in its
    /// `delete-in-progress` state rather than gone.
    ///
    /// A list whose removal is ALREADY owed is answered as it stands and its due instant is
    /// left where it was: the list is on its way out either way, so a second delete does not
    /// push its removal further off.
    pub fn begin_managed_prefix_list_delete(
        &mut self,
        prefix_list_id: &str,
    ) -> Option<crate::ops::types::ManagedPrefixList> {
        let record = self.managed_prefix_lists.get_mut(prefix_list_id)?;
        if record
            .pending
            .as_ref()
            .is_none_or(|pending| pending.to != ManagedPrefixListMove::Removed)
        {
            record.view.state = Some(PREFIX_LIST_STATE_DELETE_IN_PROGRESS.to_string());
            record.pending = Some(ManagedPrefixListTransition::starting_now(
                ManagedPrefixListMove::Removed,
            ));
        }
        Some(record.view.clone())
    }

    /// Take the managed prefix list `prefix_list_id` out of the store, and answer the record
    /// that was removed — or `None` when the store held no list with that id.
    ///
    /// The ONE place a prefix list stops being stored, the counterpart of
    /// [`Self::insert_managed_prefix_list`]: it gives the caller-supplied bytes the record
    /// retained back to the store's budget — counted by the same one definition of that charge
    /// ([`managed_prefix_list_payload_bytes`]) the insert charged — and forgets the
    /// `ClientToken` that named the list, so a create replaying that token mints a new list
    /// rather than naming one nothing holds any more.
    pub fn remove_managed_prefix_list(
        &mut self,
        prefix_list_id: &str,
    ) -> Option<ManagedPrefixListRecord> {
        let record = self.managed_prefix_lists.remove(prefix_list_id)?;
        let client_token = self
            .managed_prefix_list_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == prefix_list_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = client_token.as_deref() {
            self.managed_prefix_list_client_tokens.remove(token);
        }
        self.release_stored_bytes(managed_prefix_list_payload_bytes(
            record.view.prefix_list_name.as_deref().unwrap_or_default(),
            &record.entries,
            record.view.tags.as_deref().unwrap_or_default(),
            client_token.as_deref(),
        ));
        Some(record)
    }

    /// The entries of the prefix list `prefix_list_id` as a caller signed for `region` can
    /// see them — or `None` when no prefix list that caller can see carries that id.
    ///
    /// The two sources are the ones a report of a prefix list has: the lists the ACCOUNT
    /// created ([`Self::managed_prefix_lists`]) and the ones the REGION publishes
    /// ([`managed_prefix_lists_in_region`]) — the same pair `DescribeManagedPrefixLists`
    /// reports, so one id names one list whichever operation is asked, and the entries of a
    /// published list are read off that one derivation rather than spelled a second time
    /// here.
    ///
    /// A published list has never been revised, so the entries it has are the ones it was
    /// published with: they are reported as [`FIRST_PREFIX_LIST_VERSION`], the version
    /// `GetManagedPrefixListEntries` version-scopes its answer by
    /// (`GetManagedPrefixListEntriesRequest$TargetVersion`) — which is a property of the
    /// ENTRY SET rather than a member of the list, and is why
    /// [`ServicePrefixList::to_managed_prefix_list`] reports no `Version` on the list
    /// itself: nothing revises a list Amazon Web Services publishes, so the list has no
    /// revision count to report. A stored list's entries belong to the version its record
    /// carries.
    pub fn visible_prefix_list_entries(
        &self,
        region: &str,
        prefix_list_id: &str,
    ) -> Option<PrefixListEntriesView> {
        if let Some(record) = self.managed_prefix_lists.get(prefix_list_id) {
            return Some(PrefixListEntriesView {
                version: record.view.version.unwrap_or(FIRST_PREFIX_LIST_VERSION),
                entries: record.entries.clone(),
            });
        }
        managed_prefix_lists_in_region(region)
            .into_iter()
            .find(|published| published.prefix_list_id == prefix_list_id)
            .map(|published| PrefixListEntriesView {
                version: FIRST_PREFIX_LIST_VERSION,
                entries: published.to_prefix_list_entries(),
            })
    }
}

/// One prefix list's entries and the version they belong to, as
/// [`ComAmazonawsEc2Amazonec2State::visible_prefix_list_entries`] answers them.
///
/// The version travels WITH the entries because a report of them is version-scoped: "the
/// version of the prefix list for which to return the entries. The default is the current
/// version" (`GetManagedPrefixListEntriesRequest$TargetVersion`), so the reader has to know
/// which version the entries it was handed are of.
#[derive(Debug, Clone)]
pub struct PrefixListEntriesView {
    /// The version of the prefix list these entries are the entry set of
    /// (`ManagedPrefixList$Version`).
    pub version: i64,
    /// The entries themselves, in the order the source holds them.
    pub entries: Vec<crate::ops::types::PrefixListEntry>,
}

/// One Amazon Web Services SERVICE prefix list of one Region.
///
/// A prefix list of a service is a fact of a Region, not a resource a caller creates: no EC2
/// operation adds or withdraws one, so [`service_prefix_lists_in_region`] derives them from
/// the Region the same way [`availability_zones_in_region`] derives its zones, and the
/// operations that report them write nothing. They live here, beside the data model, because
/// BOTH reports of a prefix list read them — `DescribePrefixLists` reports the services
/// ("available Amazon Web Services services in a prefix list format") and
/// `DescribeManagedPrefixLists` reports "your managed prefix lists and any Amazon Web
/// Services-managed prefix lists" — and the two must agree about one list's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrefixList {
    /// `pl-0123abcd` — the id [`service_prefix_list_id`] derives for this list.
    pub prefix_list_id: String,
    /// `com.amazonaws.us-east-1.s3` — the service's name in this Region.
    pub prefix_list_name: String,
    /// The IP address ranges of the service, from [`service_prefix_list_cidrs`].
    pub cidrs: Vec<String>,
}

impl ServicePrefixList {
    /// This prefix list as the modelled `PrefixList` shape `DescribePrefixLists` reports,
    /// whose three members are exactly what that operation documents reporting: the name, the
    /// id and the ranges.
    pub fn to_prefix_list(&self) -> crate::ops::types::PrefixList {
        crate::ops::types::PrefixList {
            prefix_list_id: Some(self.prefix_list_id.clone()),
            prefix_list_name: Some(self.prefix_list_name.clone()),
            cidrs: Some(self.cidrs.clone()),
        }
    }

    /// The entry set of this prefix list, as the modelled `PrefixListEntry` shape
    /// `GetManagedPrefixListEntries` reports one.
    ///
    /// One entry per range the list carries, and nothing else: "The CIDR block" and "The
    /// description" are the shape's two members (`PrefixListEntry`), and a description is
    /// something an ACCOUNT writes on an entry of its own list
    /// (`AddPrefixListEntry$Description`) — a published list's entries carry none, so each
    /// reports its range alone. Derived from the same [`Self::cidrs`] `to_prefix_list`
    /// reports, so the ranges of one published list cannot differ between the operation that
    /// reports the list and the one that reports its entries.
    pub fn to_prefix_list_entries(&self) -> Vec<crate::ops::types::PrefixListEntry> {
        self.cidrs
            .iter()
            .map(|cidr| crate::ops::types::PrefixListEntry {
                cidr: Some(cidr.clone()),
                description: None,
            })
            .collect()
    }

    /// This prefix list as the modelled `ManagedPrefixList` shape
    /// `DescribeManagedPrefixLists` reports, for `region`.
    ///
    /// The members that are facts of the list itself are read off it: its id, its name and the
    /// ranges' address family. Its owner is Amazon Web Services rather than the account
    /// ([`AWS_MANAGED_PREFIX_LIST_OWNER`]), so its ARN carries the service's account segment
    /// too ([`aws_owned_arn`]) and not the account the request is signed for; it carries no
    /// tag, because an account tags only its own resources, and it is settled: a list a
    /// Region publishes has no creation still in progress, so its state is
    /// [`PREFIX_LIST_STATE_CREATE_COMPLETE`].
    ///
    /// `MaxEntries` and `Version` are ABSENT, because both are facts of a list an ACCOUNT
    /// created: the maximum is "the maximum number of entries for the prefix list" the
    /// creating request chose (`CreateManagedPrefixListRequest$MaxEntries`), and the version
    /// counts the revisions `ModifyManagedPrefixList` makes. Nothing adds an entry to a list
    /// Amazon Web Services publishes and nothing revises one, so neither member has a value
    /// to report rather than one derived from the entries the list happens to carry today.
    pub fn to_managed_prefix_list(&self, region: &str) -> crate::ops::types::ManagedPrefixList {
        crate::ops::types::ManagedPrefixList {
            prefix_list_id: Some(self.prefix_list_id.clone()),
            address_family: Some(
                if self.cidrs.iter().any(|cidr| cidr.contains(':')) {
                    PREFIX_LIST_ADDRESS_FAMILY_IPV6
                } else {
                    PREFIX_LIST_ADDRESS_FAMILY_IPV4
                }
                .to_string(),
            ),
            state: Some(PREFIX_LIST_STATE_CREATE_COMPLETE.to_string()),
            state_message: None,
            // "The Amazon Resource Name (ARN) for the prefix list"
            // (`ManagedPrefixList$PrefixListArn`) names the account the list belongs to, and
            // this list belongs to Amazon Web Services rather than to the account the
            // request is signed for — so its account segment is
            // [`AWS_OWNED_RESOURCE_ACCOUNT`], the ARN spelling of the `OwnerId` below. The
            // Region and the id are the list's own.
            prefix_list_arn: Some(aws_owned_arn(
                "ec2",
                region,
                &format!("{RESOURCE_TYPE_PREFIX_LIST}/{}", self.prefix_list_id),
            )),
            prefix_list_name: Some(self.prefix_list_name.clone()),
            max_entries: None,
            version: None,
            tags: Some(Vec::new()),
            owner_id: Some(AWS_MANAGED_PREFIX_LIST_OWNER.to_string()),
            ipam_prefix_list_resolver_target_id: None,
            ipam_prefix_list_resolver_sync_enabled: None,
        }
    }
}

/// The `OwnerId` of a prefix list Amazon Web Services publishes rather than the account
/// creating: "The ID of the owner of the prefix list" (`ManagedPrefixList$OwnerId`) is the
/// account id for a list the caller created, and the service itself owns the lists it
/// publishes, which it names `AWS` — the value the `owner-id` filter of
/// `DescribeManagedPrefixLists` selects those lists by.
pub const AWS_MANAGED_PREFIX_LIST_OWNER: &str = "AWS";

/// The prefix lists Amazon Web Services publishes in every Region: each list's service
/// label, whether its ranges are IPv6, and whether it is one of the lists the LEGACY report
/// `DescribePrefixLists` carries.
///
/// These are the "Amazon Web Services-managed prefix lists" `DescribeManagedPrefixLists`
/// reports beside the account's own — the set the Amazon VPC User Guide's "AWS-managed
/// prefix lists" page publishes: the two gateway services and Amazon S3 Express One Zone,
/// Amazon CloudFront's origin-facing ranges, AWS Ground Station's downlink ranges, the Amazon
/// Route 53 health checkers, Amazon VPC Lattice and Amazon EC2 Instance Connect — with the
/// IPv6 list each service that publishes one has, named with the `ipv6` label the guide gives
/// it.
///
/// The third flag is what separates the two reports of a prefix list: `DescribePrefixLists`
/// "describes available Amazon Web Services services in a prefix list format" (its own doc),
/// which is the Amazon S3 and Amazon DynamoDB service family — the services whose ranges a
/// ROUTE TABLE names, each with its IPv6 list — while `DescribeManagedPrefixLists` reports
/// every list of this catalog. A list AWS publishes for any other service is therefore
/// reported by the second and not by the first. The flag says which lists that legacy report
/// carries and nothing about the endpoint TYPES a service is reachable through, which is
/// [`AWS_ENDPOINT_SERVICES`]'s own column.
///
/// Each label is the service component of the list's name, spelled the way an endpoint
/// service name spells it: `com.amazonaws.<region>.<label>`.
pub(super) const AWS_MANAGED_PREFIX_LIST_SERVICES: [(&str, bool, bool); 15] = [
    ("cloudfront.origin-facing", false, false),
    ("dynamodb", false, true),
    ("ec2-instance-connect", false, false),
    ("groundstation", false, false),
    ("ipv6.cloudfront.origin-facing", true, false),
    ("ipv6.dynamodb", true, true),
    ("ipv6.groundstation", true, false),
    ("ipv6.route53-healthchecks", true, false),
    ("ipv6.s3", true, true),
    ("ipv6.s3express", true, true),
    ("ipv6.vpc-lattice", true, false),
    ("route53-healthchecks", false, false),
    ("s3", false, true),
    ("s3express", false, true),
    ("vpc-lattice", false, false),
];

/// The first address of the range every derived prefix list CIDR is carved out of:
/// `198.18.0.0/15`, which IANA reserves for benchmarking (RFC 2544) and no real service
/// answers on. See [`service_prefix_list_cidrs`].
pub(super) const BENCHMARK_RANGE_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// The prefix length of that reserved range.
pub(super) const BENCHMARK_RANGE_PREFIX_LENGTH: u32 = 15;

/// The prefix length of each block carved out of it: one block is one prefix list's reported
/// range.
pub(super) const PREFIX_LIST_BLOCK_PREFIX_LENGTH: u32 = 21;

/// Every prefix list Amazon Web Services publishes in Region `region`, in `PrefixListName`
/// order — the order a `NextToken` walk of `DescribeManagedPrefixLists` resumes in, and
/// unique by construction, one name per entry of [`AWS_MANAGED_PREFIX_LIST_SERVICES`].
///
/// This is the set `DescribeManagedPrefixLists` reports, and the one a security group rule or
/// a route may reference; [`service_prefix_lists_in_region`] is the gateway-service subset of
/// it that `DescribePrefixLists` reports. Both read the same entries, so one list's id, name
/// and ranges are the same whichever report reaches it.
pub fn managed_prefix_lists_in_region(region: &str) -> Vec<ServicePrefixList> {
    let mut lists: Vec<ServicePrefixList> = AWS_MANAGED_PREFIX_LIST_SERVICES
        .iter()
        .enumerate()
        .map(|(index, (service, ipv6, _))| {
            let prefix_list_name = format!("com.amazonaws.{region}.{service}");
            ServicePrefixList {
                prefix_list_id: service_prefix_list_id(&prefix_list_name),
                cidrs: service_prefix_list_cidrs(u32::try_from(index).unwrap_or(0), *ipv6),
                prefix_list_name,
            }
        })
        .collect();
    lists.sort_by(|left, right| left.prefix_list_name.cmp(&right.prefix_list_name));
    // `PrefixListIds` selects by id, so two lists of one Region sharing one would make a
    // request ambiguous. The ids are derived from distinct names, which the catalog's own
    // entries keep distinct; the assertion pins that invariant if the catalog grows.
    debug_assert!(
        lists
            .windows(2)
            .all(|pair| pair[0].prefix_list_id != pair[1].prefix_list_id),
        "two prefix lists of one Region must not share an id"
    );
    lists
}

/// The Amazon Web Services SERVICE prefix lists Region `region` publishes — the Amazon S3
/// and Amazon DynamoDB service family of [`AWS_MANAGED_PREFIX_LIST_SERVICES`], which is what
/// `DescribePrefixLists` reports — in `PrefixListName` order.
pub fn service_prefix_lists_in_region(region: &str) -> Vec<ServicePrefixList> {
    let legacy: Vec<&str> = AWS_MANAGED_PREFIX_LIST_SERVICES
        .iter()
        .filter(|(_, _, legacy)| *legacy)
        .map(|(service, _, _)| *service)
        .collect();
    managed_prefix_lists_in_region(region)
        .into_iter()
        .filter(|list| {
            legacy.iter().any(|service| {
                list.prefix_list_name == format!("com.amazonaws.{region}.{service}")
            })
        })
        .collect()
}

/// The id of the service prefix list named `prefix_list_name`.
///
/// AWS mints an opaque id of the form `pl-` followed by hexadecimal digits for every prefix
/// list and keeps it for the life of the list, so the id here is DERIVED from the list's name
/// — a pure function of it through [`stable_derived_id_hash`] — which gives the same two
/// properties without storing anything: every read of one Region's prefix list reports the
/// same id, and two lists with different names get different ids. It carries 8 digits where
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints 17, so a derived id can never collide with
/// the id of a managed prefix list a caller created.
pub(super) fn service_prefix_list_id(prefix_list_name: &str) -> String {
    format!(
        "{PREFIX_LIST_ID_PREFIX}-{:08x}",
        stable_derived_id_hash(prefix_list_name) & 0xffff_ffff
    )
}

/// The IP address ranges this emulator reports for prefix list number `service_index` of
/// [`AWS_MANAGED_PREFIX_LIST_SERVICES`], as an IPv6 range when `ipv6` says the list carries
/// one.
///
/// AWS publishes a service's real ranges per Region in its `ip-ranges.json` feed, which
/// neither this service's model nor a request carries and this emulator holds no copy of. So a
/// range reported here is DERIVED, and it is derived from the one thing a prefix list is scoped
/// to within a Region — the service: one `/21` block of `198.18.0.0/15`, the range IANA
/// reserves for benchmarking (RFC 2544), or one `/48` block of `2001:db8::/32`, the range
/// reserved for documentation (RFC 3849) — so a range this emulator reports can never be read
/// as an address a real Amazon Web Services service answers on. Indexing the block by the
/// service alone is what the two properties this derivation claims need, and no more: every
/// read of one Region's list reports the same range, and two lists of one Region never share a
/// block. Two Regions DO report the same ranges for one service — the real ranges are the
/// feed's, which this emulator does not hold, so nothing here distinguishes Regions by address.
pub(super) fn service_prefix_list_cidrs(service_index: u32, ipv6: bool) -> Vec<String> {
    if ipv6 {
        return vec![format!("2001:db8:{service_index:x}::/48")];
    }
    // How many blocks of [`PREFIX_LIST_BLOCK_PREFIX_LENGTH`] the reserved range holds. A
    // catalog larger than that would have nothing left to stay inside, so the block number
    // wraps — a derivation that keeps answering an address in the reserved range rather than a
    // request-path panic — and the assertion catches the overgrown catalog in development,
    // where two services would otherwise come to share a block.
    let blocks = 1u32 << (PREFIX_LIST_BLOCK_PREFIX_LENGTH - BENCHMARK_RANGE_PREFIX_LENGTH);
    debug_assert!(
        u32::try_from(AWS_MANAGED_PREFIX_LIST_SERVICES.len()).unwrap_or(u32::MAX) <= blocks,
        "the prefix list catalog must fit the blocks the reserved range holds"
    );
    let block = service_index % blocks;
    let address = BENCHMARK_RANGE_BASE + block * (1u32 << (32 - PREFIX_LIST_BLOCK_PREFIX_LENGTH));
    vec![format!(
        "{}/{PREFIX_LIST_BLOCK_PREFIX_LENGTH}",
        ::std::net::Ipv4Addr::from(address)
    )]
}

/// A deterministic digest of `value`, for deriving the id of a resource this emulator does not
/// store — a published prefix list, an endpoint service of a Region — from its name.
///
/// The standard library's hasher, seeded by its own default rather than randomly, so a digest
/// is a pure function of its input: every read of one name answers the same value, which is
/// what makes the derived id stable across calls. `DefaultHasher`'s output is not guaranteed
/// stable ACROSS Rust versions, so a derived id is stable within one build of the
/// emulator, not across toolchains — which is all an opaque id a caller reads back from the
/// same running emulator needs. Nothing here is a secret and nothing depends on a digest being
/// unforgeable.
pub(super) fn stable_derived_id_hash(value: &str) -> u64 {
    use ::std::hash::{Hash, Hasher};

    let mut hasher = ::std::hash::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Transit gateway prefix list references
// ---------------------------------------------------------------------------

// The id prefix a prefix list carries lives beside the other resource-id prefixes as
// `PREFIX_LIST_ID_PREFIX`; a reference's form check holds a `PrefixListResourceId` to the
// EC2 resource-id form every other id of this module is held to
// ([`prefixed_id_is_well_formed`]: `pl-` and 8 or 17 lowercase hexadecimal digits, the two
// forms AWS's own prefix list ids take), since the model publishes no `@pattern` for that
// shape (`smithy_query(query="shape",shape="PrefixListResourceId")` is a plain `String`).

/// `InvalidPrefixListId.Malformed` (HTTP 400): the request named something that is not a
/// prefix list id at all, so there is nothing to reference.
///
/// Spelled like [`INVALID_ROUTE_TABLE_ID_MALFORMED`], the sibling code the same requests
/// answer for the route table they name, so one family of refusals reads one way.
pub const INVALID_PREFIX_LIST_ID_MALFORMED: &str = "InvalidPrefixListId.Malformed";

/// A prefix list reference the service is still putting into the route table: the
/// `TransitGatewayPrefixListReferenceState` enum's `pending` member
/// ([`crate::ops::types::TRANSIT_GATEWAY_PREFIX_LIST_REFERENCE_STATE_VALUES`]).
pub const PREFIX_LIST_REFERENCE_STATE_PENDING: &str = "pending";

/// A prefix list reference that is in force as a route of its route table: that enum's
/// `available` member.
pub const PREFIX_LIST_REFERENCE_STATE_AVAILABLE: &str = "available";

/// A prefix list reference the service is taking back out of its route table: that enum's
/// `deleting` member, which is what `DeleteTransitGatewayPrefixListReference` answers with
/// until the removal falls due.
pub const PREFIX_LIST_REFERENCE_STATE_DELETING: &str = "deleting";

/// What the clock owes one stored prefix list reference next.
///
/// The `deleting` member of `TransitGatewayPrefixListReferenceState` is deliberately not a
/// move but the state a reference SITS in while its removal is owed: once the removal has
/// been carried out the pair is not a route of that route table at all, and the state catalog
/// has no word for "not a route" — the same reading as [`PropagationMove::Removed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixListReferenceMove {
    /// `pending` -> `available`: the route is in the table.
    Available,
    /// `deleting` -> reported no longer: the route is out of the table.
    Removed,
}

/// The move the clock owes one prefix list reference: a [`ClockTransition`] to the next
/// [`PrefixListReferenceMove`].
pub type PrefixListReferenceTransition = ClockTransition<PrefixListReferenceMove>;

/// Refuse a prefix list the named transit gateway route table holds no reference (route) for:
/// HTTP 400 [`INVALID_ROUTE_NOT_FOUND`].
///
/// A prefix list reference IS a route — "Deletes a reference (route) to a prefix list in a
/// specified transit gateway route table"
/// (`DeleteTransitGatewayPrefixListReference`) — so this is EC2's code for a route it cannot
/// resolve in a route table that DOES exist, which is a different answer from the route
/// table's own `.NotFound` and lets a client tell "no such route table" from "no such route
/// in it". The counterpart of [`local_gateway_route_not_found`] on the transit gateway side.
pub fn transit_gateway_prefix_list_reference_not_found<E>(
    route_table_id: &str,
    prefix_list_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_NOT_FOUND,
        400,
        format!(
            "The route to the prefix list '{prefix_list_id}' does not exist in the transit \
             gateway route table '{route_table_id}'"
        ),
    )
}

/// One prefix list reference of one transit gateway route table: the modelled
/// [`TransitGatewayPrefixListReference`] every operation that reports one answers with, plus
/// the move the clock owes it.
///
/// The view is kept as the modelled shape rather than a parallel record, for the reason the
/// attachment and announcement stores are: the create and the read answer the one stored
/// item, so they cannot describe one reference differently.
#[derive(Debug, Clone)]
pub struct TransitGatewayPrefixListReferenceRecord {
    /// The reference as the model shapes it, carrying the state reached so far.
    pub view: TransitGatewayPrefixListReference,
    /// The move the clock owes this reference, while one is owed.
    pub pending: Option<PrefixListReferenceTransition>,
}

/// What one stored prefix list reference costs against the crate's byte budget: the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every record costs plus the two caller-supplied ids it
/// holds.
///
/// The ONE definition of that cost, like [`capacity_block_payload_bytes`] is for a Capacity
/// Block: a writer charges exactly it through
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_prefix_list_reference`], and a
/// handler asking `would_exceed_stored_bytes` BEFORE the write asks about the same figure,
/// so the pre-check and the charge cannot drift apart.
pub fn transit_gateway_prefix_list_reference_payload_bytes(
    route_table_id: &str,
    prefix_list_id: &str,
) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(route_table_id.len())
        .saturating_add(prefix_list_id.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another prefix list reference fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_prefix_list_reference_capacity(&self) -> bool {
        self.transit_gateway_prefix_list_references.len() < MAX_STORED_RECORDS_PER_STORE
    }


    /// Store the reference to `prefix_list_id` in `route_table_id` and answer it as it now
    /// stands — or `None` when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING
    /// was stored.
    ///
    /// The ONE place a prefix list reference is built, so every operation that reports one
    /// reports the same member set: the owning account and the `pending` state come from
    /// here rather than from a request, and the clock owes the reference
    /// [`PREFIX_LIST_REFERENCE_STATE_AVAILABLE`] [`TRANSITION_DELAY`] later — the span is
    /// never zero, so a caller acting straight after the create still sees it pending.
    ///
    /// The caller-chosen bytes the record holds are charged here on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, so no path can store one
    /// uncharged.
    pub fn insert_transit_gateway_prefix_list_reference(
        &mut self,
        route_table_id: &str,
        prefix_list_id: &str,
        blackhole: bool,
        attachment: Option<TransitGatewayPrefixListAttachment>,
    ) -> Option<TransitGatewayPrefixListReference> {
        if !self.has_transit_gateway_prefix_list_reference_capacity() {
            return None;
        }
        let view = TransitGatewayPrefixListReference {
            transit_gateway_route_table_id: Some(route_table_id.to_string()),
            prefix_list_id: Some(prefix_list_id.to_string()),
            // "The ID of the prefix list owner" (`$PrefixListOwnerId`): the account this
            // emulator serves, which is the account every resource it stores belongs to.
            prefix_list_owner_id: Some(::engine::emulator_account_id().to_string()),
            state: Some(PREFIX_LIST_REFERENCE_STATE_PENDING.to_string()),
            blackhole: Some(blackhole),
            transit_gateway_attachment: attachment,
        };
        self.add_stored_bytes(transit_gateway_prefix_list_reference_payload_bytes(
            route_table_id,
            prefix_list_id,
        ));
        self.transit_gateway_prefix_list_references.insert(
            (route_table_id.to_string(), prefix_list_id.to_string()),
            TransitGatewayPrefixListReferenceRecord {
                view: view.clone(),
                pending: Some(PrefixListReferenceTransition::starting_now(
                    PrefixListReferenceMove::Available,
                )),
            },
        );
        Some(view)
    }

    /// Start the delete of the reference to `prefix_list_id` in `route_table_id`, and answer it
    /// as it now stands — or `None` when the store holds no reference for that pair.
    ///
    /// The ONE place a prefix list reference's delete is started, on the same contract as
    /// [`TransitGatewayPropagationRecord::begin_move`]: the reference goes into
    /// [`PREFIX_LIST_REFERENCE_STATE_DELETING`] NOW, with its REMOVAL owed to the clock one
    /// [`TRANSITION_DELAY`] later ([`PrefixListReferenceMove::Removed`], applied by
    /// [`Self::materialize_due_transitions`], which also gives the record's bytes back to the
    /// store's budget). The span is never zero, so a caller reading straight after the delete
    /// still sees the route in its `deleting` state rather than gone.
    ///
    /// A reference whose removal is ALREADY owed is answered as it stands and its due instant
    /// is left where it was: the route is on its way out either way, so a second delete does
    /// not push its removal further off.
    pub fn begin_transit_gateway_prefix_list_reference_delete(
        &mut self,
        route_table_id: &str,
        prefix_list_id: &str,
    ) -> Option<TransitGatewayPrefixListReference> {
        let record = self
            .transit_gateway_prefix_list_references
            .get_mut(&(route_table_id.to_string(), prefix_list_id.to_string()))?;
        if record
            .pending
            .as_ref()
            .is_none_or(|pending| pending.to != PrefixListReferenceMove::Removed)
        {
            record.view.state = Some(PREFIX_LIST_REFERENCE_STATE_DELETING.to_string());
            record.pending = Some(PrefixListReferenceTransition::starting_now(
                PrefixListReferenceMove::Removed,
            ));
        }
        Some(record.view.clone())
    }

    /// The prefix list references of one transit gateway route table, in prefix-list-id
    /// order — the stable order `GetTransitGatewayPrefixListReferences` reports and pages
    /// over.
    ///
    /// The store is keyed by (route table, prefix list), so this is a RANGE over the keys of
    /// one route table rather than a scan of every reference stored, exactly as
    /// [`Self::transit_gateway_route_table_propagations`] is.
    ///
    /// A read, so it writes nothing: the caller applies the clock-owed moves
    /// ([`Self::materialize_due_transitions`]) before asking.
    pub fn transit_gateway_prefix_list_references_of<'a>(
        &'a self,
        route_table_id: &'a str,
    ) -> impl Iterator<Item = &'a TransitGatewayPrefixListReferenceRecord> {
        self.transit_gateway_prefix_list_references
            .range((route_table_id.to_string(), String::new())..)
            .take_while(move |((table, _), _)| table == route_table_id)
            .map(|(_, record)| record)
    }
}

// ---------------------------------------------------------------------------
// Dedicated Hosts
// ---------------------------------------------------------------------------

/// A Dedicated Host that is allocated and can accept launches: `AllocationState`'s
/// `available` (`smithy_query(query="shape",shape="AllocationState")`).
pub const HOST_STATE_AVAILABLE: &str = "available";

/// A Dedicated Host the account has given back: `AllocationState`'s `released` — "On-Demand
/// billing is stopped and the host goes into released state" (`ReleaseHosts`).
pub const HOST_STATE_RELEASED: &str = "released";

/// A Dedicated Host given back after its hardware failed permanently:
/// `AllocationState`'s `released-permanent-failure`. Released as much as
/// [`HOST_STATE_RELEASED`] is, which is what [`host_is_released`] reads the two for.
pub const HOST_STATE_RELEASED_PERMANENT_FAILURE: &str = "released-permanent-failure";

/// Whether `host` has been released, so that its id "can no longer be specified in another
/// request" (`ReleaseHosts`).
///
/// A released host is one in either released member of the modelled `AllocationState` enum,
/// so a host released after a permanent hardware failure is as released as one a caller
/// gave back — the one definition every operation that has to tell the two apart from a held
/// host reads.
pub fn host_is_released(host: &Host) -> bool {
    matches!(
        host.state.as_deref(),
        Some(HOST_STATE_RELEASED) | Some(HOST_STATE_RELEASED_PERMANENT_FAILURE)
    )
}

/// The auto-placement a Dedicated Host gets when the request names none: "Default:
/// `off`" (`AllocateHostsRequest$AutoPlacement`). A value of the modelled
/// `AutoPlacement` enum ([`crate::ops::types::AUTO_PLACEMENT_VALUES`]).
pub const DEFAULT_AUTO_PLACEMENT: &str = "off";

/// The host recovery a Dedicated Host gets when the request names none: "Host recovery is
/// disabled by default […] Default: `off`" (`AllocateHostsRequest$HostRecovery`). A value
/// of the modelled `HostRecovery` enum.
pub const DEFAULT_HOST_RECOVERY: &str = "off";

/// The AMD SEV-SNP setting a Dedicated Host gets when the request's `CpuOptions` names
/// none: "If you don't specify a value, AMD SEV-SNP is `disabled`"
/// (`HostCpuOptionsRequest$AmdSevSnp`). A value of the modelled `AmdSevSnp` enum
/// ([`crate::ops::types::AMD_SEV_SNP_VALUES`]).
pub const AMD_SEV_SNP_DISABLED: &str = "disabled";

/// A Dedicated Host that supports every instance type of ONE instance family:
/// "`on`[,] the Dedicated Host supports multiple instance types in the instance family"
/// (`Host$AllowsMultipleInstanceTypes`), which is the host an `InstanceFamily` request
/// allocates. A value of the modelled `AllowsMultipleInstanceTypes` enum
/// ([`crate::ops::types::ALLOWS_MULTIPLE_INSTANCE_TYPES_VALUES`]).
pub const ALLOWS_MULTIPLE_INSTANCE_TYPES_ON: &str = "on";

/// A Dedicated Host that supports ONE instance type only — "`off`[,] the Dedicated Host
/// supports a single instance type only" (`Host$AllowsMultipleInstanceTypes`), which is
/// the host an `InstanceType` request allocates.
pub const ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF: &str = "off";

/// The one resource type `AllocateHosts`'s `TagSpecifications` can name: "The tags to
/// apply to the Dedicated Host during creation"
/// (`AllocateHostsRequest$TagSpecifications`), which is the `dedicated-host` member of the
/// modelled `ResourceType` enum ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub const DEDICATED_HOST_TAGGABLE_RESOURCE: &str = "dedicated-host";

/// How many Dedicated Hosts this emulator stores before `AllocateHosts` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored host, so the store needs a ceiling that is not the process's memory.
/// It is well above any published per-Region Dedicated Host quota a test would exercise.
/// A count ceiling is no bound on the caller-chosen bytes each record holds, so those are
/// charged against [`MAX_STORED_BYTES`] too ([`host_payload_bytes`]).
pub const MAX_STORED_DEDICATED_HOSTS: usize = 4096;

/// `HostLimitExceeded` (HTTP 400): the allocation would take the account past the number
/// of Dedicated Hosts it may hold.
pub const HOST_LIMIT_EXCEEDED: &str = "HostLimitExceeded";

/// `InvalidHostID.NotFound` (HTTP 400): the request named a host id the account holds no
/// such host for.
pub const INVALID_HOST_ID_NOT_FOUND: &str = "InvalidHostID.NotFound";

/// `InvalidHostID.Malformed` (HTTP 400): the request named something that is not a host id
/// at all, so there is nothing to look up.
pub const INVALID_HOST_ID_MALFORMED: &str = "InvalidHostID.Malformed";

/// `InvalidHostReservationOfferingId.Malformed` (HTTP 400): the request named something that
/// is not a Dedicated Host reservation OFFERING id at all, so there is nothing to look up —
/// the `Invalid<Kind>.Malformed` half of the pair, refused before the published catalog is
/// consulted.
pub const INVALID_HOST_RESERVATION_OFFERING_ID_MALFORMED: &str =
    "InvalidHostReservationOfferingId.Malformed";

/// Refuse an `OfferingId` that is not a host reservation offering id at all: HTTP 400
/// [`INVALID_HOST_RESERVATION_OFFERING_ID_MALFORMED`], in the sentence EC2 words for it —
/// fixed text with the value the REQUEST sent single-quoted in it, bounded by
/// [`echoed_resource_id`] the way every other echoed id is.
pub fn invalid_host_reservation_offering_id_malformed<E>(offering_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_HOST_RESERVATION_OFFERING_ID_MALFORMED,
        400,
        format!(
            "The specified host reservation Offering ID '{}' is not valid.",
            echoed_resource_id(offering_id)
        ),
    )
}

/// `InvalidHostConfiguration` (HTTP 400): the request asks for a Dedicated Host configuration
/// EC2 cannot provide, rather than for a parameter it could not read.
///
/// The code the recorded refusal of a host configured for hardware EC2 does not offer carries.
/// The two say different things to a client: this one means the configuration asked for is
/// impossible, while `InvalidParameterValue` invites a retry with another value for the
/// parameter — so a configuration refusal carries this code and the retry and reporting logic
/// branching on it takes the right arm.
pub const INVALID_HOST_CONFIGURATION: &str = "InvalidHostConfiguration";

/// Refuse a Dedicated Host configured for an instance type EC2 does not offer: HTTP 400
/// [`INVALID_HOST_CONFIGURATION`], `The instance type '<instanceType>' is not supported.`
///
/// The sentence the recorded refusal carries: fixed text with the `InstanceType` the REQUEST
/// sent single-quoted in it, bounded by [`echoed_resource_id`] like every other echoed value.
/// The ONE writer of this refusal, so the operations that configure a host for an instance
/// type — the allocate and the modify — cannot word it two ways.
pub fn unsupported_host_instance_type<E>(instance_type: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_HOST_CONFIGURATION,
        400,
        format!(
            "The instance type '{}' is not supported.",
            echoed_resource_id(instance_type)
        ),
    )
}

/// Refuse a Dedicated Host configured for an instance family no instance type EC2 offers
/// belongs to: HTTP 400 [`INVALID_HOST_CONFIGURATION`], the same sentence
/// [`unsupported_host_instance_type`] carries with the member's own resource named — the
/// `InstanceFamily` member is the other way one request configures one host, and an
/// unsupported value of it makes the same configuration impossible.
pub fn unsupported_host_instance_family<E>(instance_family: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_HOST_CONFIGURATION,
        400,
        format!(
            "The instance family '{}' is not supported.",
            echoed_resource_id(instance_family)
        ),
    )
}

/// The prefix EC2 names its Apple-hardware instance families with: `mac1`, `mac2`,
/// `mac2-m2pro`. The families themselves are read off the modelled `InstanceType` enum
/// rather than listed here — see [`is_mac_instance_family`].
pub const MAC_INSTANCE_FAMILY_PREFIX: &str = "mac";

/// The instance family part of an instance type: `mac2-m2pro` of `mac2-m2pro.metal`, `m5`
/// of `m5.large` — "The instance family […] For example, `m5`"
/// (`HostProperties$InstanceFamily`). A value carrying no size is its own family.
pub fn instance_family_of(instance_type: &str) -> &str {
    instance_type
        .split_once('.')
        .map_or(instance_type, |(family, _size)| family)
}

/// Is `allowed` the `InstanceType` catalog — the value set the model declares for every
/// member that names the type an instance is placed as?
///
/// The boundary's constraint walk reports a refused enum value as the value set it failed
/// rather than as the shape that declared it, so the catalog IS how that type is recognised
/// ([`is_caller_role_catalog`] records the rule in full). Keyed to the TYPE's catalog and not
/// to the operation carrying it, so a launch, a Spot Instance request, a Spot Fleet pool and a
/// launch template's own data are each refused an unpublished type in the same words.
///
/// Compared as a SET rather than member by member in order: this catalog is the largest in the
/// model, and [`offered_instance_types`] reports it sorted while the walk reports it in the
/// model's own declaration order — two orders of one value set.
pub fn is_instance_type_catalog(allowed: &[String]) -> bool {
    let catalog: ::std::collections::BTreeSet<&str> =
        offered_instance_types().iter().copied().collect();
    allowed.len() == catalog.len()
        && allowed
            .iter()
            .all(|held| catalog.contains(held.as_str()))
}

/// How EC2 states an instance type the catalog does not hold: HTTP 400
/// `InvalidParameterValue` with the submitted value — "Invalid value '`<value>`' for
/// InstanceType." EC2 does not state a modelled refusal as the generic validation clause; it
/// names the member that refused the value, and the only part built from the request is the
/// value echoed back.
pub fn instance_type_refusal_message(value: &str) -> String {
    format!("Invalid value '{}' for InstanceType.", echoed_resource_id(value))
}

/// Whether the model declares `instance_type` at all: it is a value of the `InstanceType`
/// enum, the catalog [`offered_instance_types`] reports.
pub fn instance_type_is_modelled(instance_type: &str) -> bool {
    offered_instance_types().contains(&instance_type)
}

/// The instance families whose network adapters encrypt in-transit traffic even though
/// their generation is older than the one from which every family does.
///
/// Source: "Encryption in transit" in the Amazon EC2 User Guide, which names them one by
/// one beside the generations that all encrypt — C5a, C5ad, C5n, D3, D3en, G4 (the NVIDIA
/// `g4dn` and the AMD `g4ad`), I3en, I4i, Im4gn, Is4gen, M5dn, M5n, M5zn, P3dn, P4d, R5dn,
/// R5n, X2idn, X2iedn and X2iezn. The high-memory U family that page names as `U-*` is on
/// the same list and is answered by [`HIGH_MEMORY_ROOT`] instead, because its names carry a
/// memory size where every entry here carries a generation.
///
/// FAMILIES, not sizes: every size of a family carries the same network adapter, so an
/// entry is compared against [`instance_family_of`] rather than against a whole instance
/// type.
pub const ENCRYPTING_INSTANCE_FAMILIES: [&str; 21] = [
    "c5a", "c5ad", "c5n", "d3", "d3en", "g4ad", "g4dn", "i3en", "i4i", "im4gn", "is4gen",
    "m5dn", "m5n", "m5zn", "p3dn", "p4d", "r5dn", "r5n", "x2idn", "x2iedn", "x2iezn",
];

/// The family root of the high-memory instance types — the `U-*` of the same published
/// list, whose names state a memory size (`u-6tb1`, `u7i-12tb`) rather than a generation,
/// so the whole family is named by its root instead of entry by entry.
pub const HIGH_MEMORY_ROOT: &str = "u";

/// The first instance generation every family of which encrypts in-transit traffic: the
/// 6th, from which every family is built on the 4th version of the AWS Nitro System
/// ("Encryption in transit", Amazon EC2 User Guide — traffic between instances is
/// encrypted for instances built on that System, plus the earlier types
/// [`ENCRYPTING_INSTANCE_FAMILIES`] names).
pub const FIRST_ENCRYPTING_INSTANCE_GENERATION: u32 = 6;

/// Whether `instance_type` automatically encrypts in-transit traffic between instances —
/// the modelled `NetworkInfo$EncryptionInTransitSupported`, "indicates whether the
/// instance type automatically encrypts in-transit traffic between instances".
///
/// A fact of the instance TYPE, not of a stored instance or of a request: the figure the
/// published catalog holds for the type ([`published_instance_type_facts`]) where it holds
/// one, and otherwise derived from the parts [`InstanceTypeName`] parses out of the name,
/// like every other fact about an instance type this module answers — a type that runs on the
/// Nitro System ([`InstanceTypeName::nitro`]) from [`FIRST_ENCRYPTING_INSTANCE_GENERATION`]
/// on, the generation from which it is the 4th version of that System, or one of the earlier
/// families [`ENCRYPTING_INSTANCE_FAMILIES`] names, or the high-memory family
/// [`HIGH_MEMORY_ROOT`] names.
///
/// The ONE site of this fact: [`instance_type_info`] reports it as the modelled member, so
/// `DescribeInstanceTypes` and `GetVpcResourcesBlockingEncryptionEnforcement` cannot
/// answer differently for one instance type. [`ENCRYPTING_INSTANCE_FAMILIES`] is where a
/// family AWS's list names below that generation belongs, with a test row of its own,
/// rather than being inferred from the name.
///
/// A value that is not an instance type at all (no `<family>.<size>`) names nothing that
/// encrypts.
pub fn instance_type_encrypts_in_transit(instance_type: &str) -> bool {
    if let Some(published) = published_instance_type_facts(instance_type)
        .and_then(|facts| facts.encryption_in_transit_supported)
    {
        return published;
    }
    InstanceTypeName::parse(instance_type)
        .is_some_and(|name| name.encrypts_in_transit())
}

/// Whether `instance_family` is the family of at least one instance type the model
/// declares, so that a host allocated for it could ever run an instance.
pub fn instance_family_is_modelled(instance_family: &str) -> bool {
    offered_instance_types()
        .iter()
        .any(|instance_type| instance_family_of(instance_type) == instance_family)
}

/// Whether `instance_family` runs macOS, and so whether a Dedicated Host configured for it
/// is an EC2 Mac Dedicated Host.
///
/// EC2 names every Apple-hardware family with the [`MAC_INSTANCE_FAMILY_PREFIX`] and no
/// other family with it, and the families themselves come from the modelled `InstanceType`
/// enum — so this is the model's own answer rather than a list maintained here.
pub fn is_mac_instance_family(instance_family: &str) -> bool {
    instance_family.starts_with(MAC_INSTANCE_FAMILY_PREFIX)
        && instance_family_is_modelled(instance_family)
}

/// The macOS release lines each EC2 Mac instance family's hardware supports, newest first,
/// as `(instance family, releases)`.
///
/// Source: "Amazon EC2 Mac instances" in the Amazon EC2 User Guide, which pairs each Mac
/// instance type with the macOS releases its hardware runs — `mac1` is the x86 Mac mini
/// and reaches back to macOS Mojave (10.14), `mac2` the M1 Mac mini, which starts at Big
/// Sur (11), `mac2-m1ultra` the M1 Ultra Mac Studio, which starts at Monterey (12), and
/// the M2 families start at Ventura (13). Every Mac family of the modelled `InstanceType`
/// enum ([`is_mac_instance_family`]) appears exactly once.
///
/// Release LINES, not patch versions: which patch is current is Apple's to decide and
/// changes without any EC2 API call, and this emulator makes no outbound connection to
/// find out — so a line is the part of "the latest macOS versions that the EC2 Mac
/// Dedicated Host can launch without being upgraded" (`MacHost$MacOSLatestSupportedVersions`)
/// that is a stable fact of the host's hardware.
pub const MAC_OS_SUPPORTED_VERSIONS: [(&str, &[&str]); 5] = [
    ("mac1", &["15", "14", "13", "12", "11", "10.15", "10.14"]),
    ("mac2", &["15", "14", "13", "12", "11"]),
    ("mac2-m1ultra", &["15", "14", "13", "12"]),
    ("mac2-m2", &["15", "14", "13"]),
    ("mac2-m2pro", &["15", "14", "13"]),
];

/// The macOS release lines `instance_family`'s hardware supports, newest first, or `None`
/// for a family [`MAC_OS_SUPPORTED_VERSIONS`] publishes none for.
pub fn mac_os_supported_versions(instance_family: &str) -> Option<&'static [&'static str]> {
    MAC_OS_SUPPORTED_VERSIONS
        .iter()
        .find(|(family, _versions)| *family == instance_family)
        .map(|(_family, versions)| *versions)
}

/// The instance family a stored host is configured for: the family it was allocated for,
/// or the family of the one instance type it supports.
///
/// A host allocated for an `InstanceFamily` carries that family and no instance type, and
/// one allocated for an `InstanceType` carries both — "If the host supports multiple
/// instance types, no `instanceType` is returned" (`HostProperties$InstanceType`) — so the
/// family is the member every host has.
pub fn host_instance_family(host: &Host) -> Option<&str> {
    let properties = host.host_properties.as_ref()?;
    properties
        .instance_family
        .as_deref()
        .or_else(|| properties.instance_type.as_deref().map(instance_family_of))
}

/// `host` as the modelled [`MacHost`] shape, or `None` when it is not an EC2 Mac Dedicated
/// Host at all.
///
/// Both members are derived from the stored host rather than stored beside it: the id is
/// the host's own, and the macOS releases it can launch are a fact of its family's
/// hardware ([`mac_os_supported_versions`]), so nothing can report one host two ways.
pub fn mac_host_view(host: &Host) -> Option<MacHost> {
    let instance_family = host_instance_family(host)?;
    if !is_mac_instance_family(instance_family) {
        return None;
    }
    Some(MacHost {
        host_id: host.host_id.clone(),
        mac_os_latest_supported_versions: mac_os_supported_versions(instance_family).map(
            |versions| {
                versions
                    .iter()
                    .map(|version| (*version).to_string())
                    .collect()
            },
        ),
    })
}

/// The caller-chosen bytes one stored Dedicated Host holds: its tags and the `ClientToken`
/// the token map keeps for it. Everything else on the record — the id, the zone, the
/// family, the states, the account — is derived by this module or bounded by the model, so
/// it is fixed-size.
///
/// The ONE definition of that charge: `AllocateHosts` asks it before the write and charges
/// exactly it, and a removal path must give back the same number.
pub fn host_payload_bytes(host: &Host) -> usize {
    tag_list_bytes(host.tags.as_deref().unwrap_or_default())
        .saturating_add(host.client_token.as_deref().map_or(0, str::len))
}

/// Whether `id` is a well-formed Dedicated Host id.
///
/// A host id is `h-` followed by 8 or 17 lowercase hexadecimal digits — the short and the
/// long id form, as the model's own `^h-([0-9a-f]{8}|[0-9a-f]{17})$` style id patterns
/// read. EC2 answers `InvalidHostID.Malformed` for anything else, BEFORE it looks the id
/// up.
pub fn host_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("h-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a request that named a host id the account holds no such host for: HTTP 400
/// `InvalidHostID.NotFound`.
pub fn invalid_host_id_not_found<E>(host_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_HOST_ID_NOT_FOUND,
        400,
        format!("The host ID '{host_id}' does not exist"),
    )
}

/// Refuse a host id this service cannot resolve: a malformed one is answered as malformed
/// and a well-formed one as not found — the way the service orders the two, and the same
/// ordering [`require_known_volume`] keeps.
pub fn unresolvable_host_id<E>(host_id: &str) -> Scaffolded<E> {
    if host_id_is_well_formed(host_id) {
        return invalid_host_id_not_found(host_id);
    }
    Scaffolded::service_error(INVALID_HOST_ID_MALFORMED, 400, invalid_id_message(host_id))
}

/// The message EC2 answers Dedicated Host ids it cannot resolve with, built from the ids the
/// request named.
///
/// To EC2 a host id it never issued is NOT VALID rather than merely absent, whatever its
/// shape, and the recorded contract for this service's host operations answers a well-formed
/// id no host has in exactly this form — so the wording lives HERE, in one place, rather
/// than once per operation that has to report an id it could not resolve. Whether an
/// operation reports it as a refusal of the whole request or as one item of a batch is that
/// operation's own contract.
pub fn unresolvable_host_ids_message<'a>(
    host_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    unresolvable_ids_message(DEDICATED_HOST_IDS_IN_REFUSALS, host_ids)
}

/// How a refusal of this service names the Dedicated Host ids it could not resolve, inside
/// [`unresolvable_ids_message`]'s sentence: the resource as the recorded refusals of the host
/// operations spell it.
const DEDICATED_HOST_IDS_IN_REFUSALS: &str = "Dedicated host IDs";

/// The sentence EC2 refuses a SET of ids of `kind` it cannot resolve with: the fixed
/// `The specified <kind> [..] are not valid.`, the ids quoted inside it as a bracketed list.
///
/// The ONE template behind every refusal of this service that names a whole set of ids rather
/// than one — the Dedicated Host operations ([`unresolvable_host_ids_message`]) and the
/// Dedicated Host reservation describe — because the recorded refusals of both carry that one
/// sentence with only the resource's name differing. `kind` is spelled the way the recorded
/// refusal of THAT resource spells it, so a caller reads the resource's own name back.
pub fn unresolvable_ids_message<'a>(kind: &str, ids: impl IntoIterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = ids.into_iter().map(|id| format!("'{id}'")).collect();
    format!(
        "The specified {kind} [{ids}] are not valid.",
        ids = quoted.join(", ")
    )
}

/// Refuse an allocation that would take the store past [`MAX_STORED_DEDICATED_HOSTS`] or
/// its byte budget: HTTP 400 `HostLimitExceeded`, EC2's own Dedicated-Hosts quota code.
pub fn host_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(HOST_LIMIT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Dedicated Hosts fit inside the store's ceiling
    /// ([`MAX_STORED_DEDICATED_HOSTS`]).
    ///
    /// An allocation must ask this BEFORE inserting anything and refuse with
    /// [`host_limit_exceeded`] when it answers `false`: one call allocates `Quantity`
    /// hosts, so the whole batch is gated rather than each host as it is written.
    pub fn has_dedicated_host_capacity(&self, count: usize) -> bool {
        self.dedicated_hosts.len().saturating_add(count) <= MAX_STORED_DEDICATED_HOSTS
    }

    /// Store one newly allocated Dedicated Host, charging the caller-chosen bytes it
    /// holds, and answer its id — or `None` when the store is full.
    ///
    /// `None` means the store is at [`MAX_STORED_DEDICATED_HOSTS`] and NOTHING was stored:
    /// the ceiling is enforced HERE as well as by [`Self::has_dedicated_host_capacity`],
    /// the way [`Self::insert_vpc_endpoint_connection`] enforces its own, so a writer that
    /// forgets the batch gate cannot grow the store without bound either. The record's own
    /// [`STORED_RECORD_OVERHEAD_BYTES`] are charged beside its caller-chosen bytes, so a
    /// host with no
    /// tags and no client token is not free against the byte budget.
    pub fn insert_dedicated_host(&mut self, host: Host) -> Option<String> {
        let host_id = host.host_id.clone().unwrap_or_default();
        // A write that REPLACES a host already stored under this id adds no record, so only
        // a new key has to fit inside the ceiling.
        if !self.dedicated_hosts.contains_key(&host_id) && !self.has_dedicated_host_capacity(1) {
            return None;
        }
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES.saturating_add(host_payload_bytes(&host)),
        );
        self.dedicated_hosts.insert(host_id.clone(), host);
        Some(host_id)
    }
}

// =============================================================================
// Route servers
// =============================================================================

/// The state a route server AWS has just created is in: `RouteServerState`'s `pending`
/// (`smithy_query(query="shape",shape="RouteServerState")`).
pub const ROUTE_SERVER_STATE_PENDING: &str = "pending";

/// The state a created route server reaches: `RouteServerState`'s `available`.
pub const ROUTE_SERVER_STATE_AVAILABLE: &str = "available";

/// The state a route server whose configuration a modification is being applied to sits
/// in: `RouteServerState`'s `modifying`, the enum's one non-terminal state besides
/// `pending` and `deleting`.
pub const ROUTE_SERVER_STATE_MODIFYING: &str = "modifying";

/// The state a route server whose delete has been asked for is in while it is being taken
/// down: `RouteServerState`'s `deleting`
/// (`smithy_query(query="shape",shape="RouteServerState")`).
pub const ROUTE_SERVER_STATE_DELETING: &str = "deleting";

/// The state a route server a delete took down ends in: `RouteServerState`'s `deleted`, the
/// enum's terminal delete state.
///
/// A deleted route server is still DESCRIBED in that state rather than dropped from the
/// store, the way a deleted route server endpoint and peer are
/// ([`ROUTE_SERVER_ENDPOINT_STATE_DELETED`]), which is what lets a caller waiting for the
/// delete to finish observe it.
pub const ROUTE_SERVER_STATE_DELETED: &str = "deleted";

/// Route persistence being turned on: `RouteServerPersistRoutesState`'s `enabling`
/// (`smithy_query(query="shape",shape="RouteServerPersistRoutesState")`).
pub const PERSIST_ROUTES_STATE_ENABLING: &str = "enabling";

/// Route persistence on: `RouteServerPersistRoutesState`'s `enabled`.
pub const PERSIST_ROUTES_STATE_ENABLED: &str = "enabled";

/// Route persistence being turned off: `RouteServerPersistRoutesState`'s `disabling`.
pub const PERSIST_ROUTES_STATE_DISABLING: &str = "disabling";

/// Route persistence off: `RouteServerPersistRoutesState`'s `disabled`.
pub const PERSIST_ROUTES_STATE_DISABLED: &str = "disabled";

/// Persisted routes being withdrawn: `RouteServerPersistRoutesState`'s `resetting`, the
/// state a route server whose persisted routes are being cleared is in — "reset will
/// withdraw all routes and reset route server to an empty FIB and RIB"
/// (`ModifyRouteServerRequest$PersistRoutes`).
pub const PERSIST_ROUTES_STATE_RESETTING: &str = "resetting";

/// `RouteServerPersistRoutesAction`'s `enable` — "Indicates whether routes should be
/// persisted after all BGP sessions are terminated"
/// (`CreateRouteServerRequest$PersistRoutes`).
pub const PERSIST_ROUTES_ENABLE: &str = "enable";

/// `RouteServerPersistRoutesAction`'s `disable`: "Routes will not be persisted in FIB and
/// RIB after all BGP sessions are terminated" (`ModifyRouteServerRequest$PersistRoutes`).
pub const PERSIST_ROUTES_DISABLE: &str = "disable";

/// `RouteServerPersistRoutesAction`'s `reset`: withdraws the routes a route server has
/// persisted, leaving whether it persists them at all as it is
/// (`ModifyRouteServerRequest$PersistRoutes`).
pub const PERSIST_ROUTES_RESET: &str = "reset";

/// The ASN range the Amazon side of the BGP session accepts: "Valid values are from 1 to
/// 4294967295" (`CreateRouteServerRequest$AmazonSideAsn`). The model declares no `@range`
/// on the member, so the handler is the only place the published bounds can exist.
pub const AMAZON_SIDE_ASN_RANGE: (i64, i64) = (1, 4_294_967_295);

/// The range `PersistRoutesDuration` accepts, in minutes: "Value must be in the range of
/// 1-5" (`CreateRouteServerRequest$PersistRoutesDuration`). The model declares no `@range`
/// on the member either.
pub const PERSIST_ROUTES_DURATION_RANGE: (i64, i64) = (1, 5);

/// How the service words a `PersistRoutesDuration` it will not accept, whatever the value
/// at fault: the duration is a fixed sentence about the parameter rather than a template
/// quoting what was sent.
pub const PERSIST_ROUTES_DURATION_INVALID_MESSAGE: &str =
    "Persist routes time is invalid for this operation.";

/// Refuse a `PersistRoutesDuration` the operation cannot act on — one outside
/// [`PERSIST_ROUTES_DURATION_RANGE`], or one the request's persistence choice gives nothing
/// to wait out: HTTP 400 `InvalidParameterValue` with
/// [`PERSIST_ROUTES_DURATION_INVALID_MESSAGE`].
pub fn persist_routes_duration_invalid<E>() -> Scaffolded<E> {
    invalid_parameter_value(PERSIST_ROUTES_DURATION_INVALID_MESSAGE)
}

/// The one resource type a route server create's `TagSpecifications` can name: "The tags to
/// apply to the route server during creation"
/// (`CreateRouteServerRequest$TagSpecifications`), which is the `route-server` member of
/// `ResourceType`.
pub const ROUTE_SERVER_TAGGABLE_RESOURCE: &str = "route-server";

/// How many route servers this emulator stores before a create is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored route server unless a caller deletes it, so the store needs a ceiling
/// that is not the process's memory. It is well above any published per-VPC route server
/// quota a test would exercise.
pub const MAX_STORED_ROUTE_SERVERS: usize = 1_024;

/// `InvalidRouteServerId.NotFound` (HTTP 400): the request named a route server id this
/// account holds no route server for. EC2 spells a per-resource lookup failure
/// `Invalid<Resource>Id.NotFound` (`InvalidLaunchTemplateId.NotFound`,
/// `InvalidVpcID.NotFound`).
pub const INVALID_ROUTE_SERVER_ID_NOT_FOUND: &str = "InvalidRouteServerId.NotFound";

/// `InvalidRouteServerId.Malformed` (HTTP 400): the request named something that is not a
/// route server id at all, so there is nothing to look up — the same pairing
/// [`INVALID_LAUNCH_TEMPLATE_ID_MALFORMED`] has.
pub const INVALID_ROUTE_SERVER_ID_MALFORMED: &str = "InvalidRouteServerId.Malformed";

/// `RouteServerLimitExceeded` (HTTP 400): the create would take the account past the number
/// of route servers it may hold, EC2's `<Resource>LimitExceeded` quota spelling
/// ([`LAUNCH_TEMPLATE_LIMIT_EXCEEDED`], [`VOLUME_LIMIT_EXCEEDED`]).
pub const ROUTE_SERVER_LIMIT_EXCEEDED: &str = "RouteServerLimitExceeded";

/// The resource-id prefix a route server id carries, as the VPC Route Server
/// documentation's own ids read (`rs-0123456789abcdef0`).
pub const ROUTE_SERVER_ID_PREFIX: &str = "rs";

/// One stored route server: the modelled [`RouteServer`] every operation that reports one
/// answers with, plus the instants the clock owes its two non-terminal states.
///
/// The view holds no `State` and no `PersistRoutesState`: both are facts of the record's
/// age, so [`RouteServerRecord::view_at`] derives them rather than letting two writers
/// store two different words for one route server.
#[derive(Debug, Clone)]
pub struct RouteServerRecord {
    /// The route server as the model shapes it, with `State` and `PersistRoutesState` left
    /// for [`RouteServerRecord::view_at`] to fill.
    pub view: RouteServer,
    /// The instant from which this route server's `pending` -> `available` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and
    /// never reported, so a wall-clock adjustment cannot move a route server back to
    /// `pending`.
    pub available_at: Instant,
    /// Whether route persistence was asked for: "Indicates whether routes should be
    /// persisted after all BGP sessions are terminated"
    /// (`CreateRouteServerRequest$PersistRoutes`).
    pub persist_routes_enabled: bool,
    /// The instant from which the persistence state is settled — `enabling` -> `enabled`
    /// for a route server that asked for persistence. A route server that did not is
    /// `disabled` from the start, so its instant is the create's own.
    pub persist_routes_settled_at: Instant,
    /// The transient `RouteServerPersistRoutesState` the clock has yet to settle
    /// ([`PERSIST_ROUTES_STATE_ENABLING`], [`PERSIST_ROUTES_STATE_DISABLING`] or
    /// [`PERSIST_ROUTES_STATE_RESETTING`]), reported until
    /// [`Self::persist_routes_settled_at`]; `None` for a route server whose persistence
    /// nobody has asked to move.
    pub persist_routes_transition: Option<&'static str>,
    /// The instant from which this route server's `modifying` -> `available` move is due:
    /// a configuration change is applied over a span, like every other move this store
    /// owes. Monotonic for the reason [`Self::available_at`] is, and the CREATE's own
    /// instant for a route server no modification has been asked for, which is already
    /// behind every read.
    pub modify_settled_at: Instant,
    /// The instant from which this route server's `deleting` -> `deleted` move is due, for a
    /// route server whose delete has been asked for; `None` while none has.
    ///
    /// A delete is a clock-owed move like every other: it is `deleting` until this instant
    /// and `deleted` from it, which [`Self::state_at`] derives, so the record reports the
    /// state the clock has reached without anybody writing it. Monotonic for the reason
    /// [`Self::available_at`] is.
    pub deleted_at: Option<Instant>,
}

impl RouteServerRecord {
    /// This route server as of `now`: the stored view with the two states the clock has
    /// reached. A read observes the transitions the clock already owed without writing
    /// anything.
    pub fn view_at(&self, now: Instant) -> RouteServer {
        RouteServer {
            state: Some(self.state_at(now).to_string()),
            persist_routes_state: Some(self.persist_routes_state_at(now).to_string()),
            ..self.view.clone()
        }
    }

    /// The route server's `RouteServerState` at `now`: `pending` until its create has
    /// settled, `modifying` while a configuration change is being applied, and `available`
    /// once the clock has settled both.
    ///
    /// A route server whose delete has been asked for is in the delete states instead,
    /// whatever the moves its creation or a modification still owed: `deleting` until the
    /// terminal state falls due and `deleted` from then on, the terminal state it stays in.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if let Some(deleted_at) = self.deleted_at {
            return if now < deleted_at {
                ROUTE_SERVER_STATE_DELETING
            } else {
                ROUTE_SERVER_STATE_DELETED
            };
        }
        if now < self.available_at {
            ROUTE_SERVER_STATE_PENDING
        } else if now < self.modify_settled_at {
            ROUTE_SERVER_STATE_MODIFYING
        } else {
            ROUTE_SERVER_STATE_AVAILABLE
        }
    }

    /// The route server's `RouteServerPersistRoutesState` at `now`: the transient state a
    /// persistence move is still within, else the settled state its configuration names.
    ///
    /// A route server whose persistence nobody has asked to move carries no transient
    /// state, so it reports `enabled` or `disabled` from the instant it was written.
    pub fn persist_routes_state_at(&self, now: Instant) -> &'static str {
        if now < self.persist_routes_settled_at {
            if let Some(transition) = self.persist_routes_transition {
                return transition;
            }
        }
        if self.persist_routes_enabled {
            PERSIST_ROUTES_STATE_ENABLED
        } else {
            PERSIST_ROUTES_STATE_DISABLED
        }
    }
}

/// The caller-chosen bytes one stored route server holds: its tags and the `ClientToken`
/// the token map keeps for it. Everything else on the record — the id, the ASN, the states,
/// the SNS topic ARN built from the id — is derived by this module or bounded by the model,
/// so it is fixed-size.
///
/// The ONE definition of that charge: a create asks it before the write and charges exactly
/// it, and a removal path must give back the same number.
pub fn route_server_payload_bytes(tags: &[Tag], client_token: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed route server id: [`ROUTE_SERVER_ID_PREFIX`] followed by 8
/// or 17 lowercase hexadecimal digits — the short and the long EC2 id form. The id shape
/// itself is defined in exactly one place, [`prefixed_id_is_well_formed`], which this names
/// the prefix for.
pub fn route_server_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(ROUTE_SERVER_ID_PREFIX, id)
}

/// Refuse a route server id the request named that this service cannot resolve: a malformed
/// one before an unknown one, the way the service orders the two.
pub fn require_known_route_server<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    route_server_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !route_server_id_is_well_formed(route_server_id) {
        return Err(invalid_route_server_id_malformed(route_server_id));
    }
    if !store.route_servers.contains_key(route_server_id) {
        return Err(invalid_route_server_id_not_found(route_server_id));
    }
    Ok(())
}

/// Resolve a route server an operation is about to ACT on: the id has to be well formed and
/// name a route server of the account — the two refusals [`require_known_route_server`]
/// answers, in the same order — AND that route server must not be one a delete has already
/// taken down. The record is resolved ONCE here, so the state the refusal names is the state
/// the lookup read.
///
/// A deleted route server is still reported, in the terminal `deleted` state, so it still
/// RESOLVES — but there is nothing left to associate, modify, endpoint or delete, and the
/// state the caller would have to change is the route server's own, which is what EC2's
/// `IncorrectState` names in this surface's own wording
/// ([`route_server_incorrect_state`]). The ONE place that guard lives, so the
/// operations acting on a route server cannot disagree about which ones admit a call.
pub fn require_live_route_server<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    route_server_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !route_server_id_is_well_formed(route_server_id) {
        return Err(invalid_route_server_id_malformed(route_server_id));
    }
    let Some(record) = store.route_servers.get(route_server_id) else {
        return Err(invalid_route_server_id_not_found(route_server_id));
    };
    let state = record.state_at(Instant::now());
    if matches!(
        state,
        ROUTE_SERVER_STATE_DELETING | ROUTE_SERVER_STATE_DELETED
    ) {
        return Err(route_server_incorrect_state(route_server_id));
    }
    Ok(())
}

/// Refuse a request that named something that is not a route server id: HTTP 400
/// `InvalidRouteServerId.Malformed`.
pub fn invalid_route_server_id_malformed<E>(route_server_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_SERVER_ID_MALFORMED,
        400,
        invalid_id_message(route_server_id),
    )
}

/// Refuse a request that named a route server id no route server of the account has: HTTP
/// 400 `InvalidRouteServerId.NotFound`.
///
/// The resource is named in the message the way EC2 names this resource TYPE —
/// [`ROUTE_SERVER_TAGGABLE_RESOURCE`], the `route-server` member of the model's
/// `ResourceType` — which is also how the two route server resources under it are named in
/// [`invalid_route_server_endpoint_id_not_found`] and
/// [`invalid_route_server_peer_id_not_found`], so one wording rule covers the family.
pub fn invalid_route_server_id_not_found<E>(route_server_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_SERVER_ID_NOT_FOUND,
        400,
        format!(
            "The {ROUTE_SERVER_TAGGABLE_RESOURCE} ID '{route_server_id}' does not exist"
        ),
    )
}

/// Refuse a create that would take the store past [`MAX_STORED_ROUTE_SERVERS`]: HTTP 400
/// `RouteServerLimitExceeded`.
pub fn route_server_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(ROUTE_SERVER_LIMIT_EXCEEDED, 400, message)
}

/// The ARN of the SNS topic a route server publishes its BGP status changes to: "Enabling
/// SNS notifications persists BGP status changes to an SNS topic provisioned by Amazon Web
/// Services" (`RouteServer$SnsNotificationsEnabled`) — so the topic is the service's, named
/// for the route server it belongs to, and it exists only for a route server whose
/// notifications are enabled.
pub fn route_server_sns_topic_arn(region: &str, route_server_id: &str) -> String {
    ::engine::emulator_arn("sns", region, &format!("RouteServer-{route_server_id}"))
}

// =============================================================================
// VPC endpoint connections
// =============================================================================

/// The id prefix a stored VPC endpoint connection is minted with.
///
/// The model publishes no pattern for `VpcEndpointConnection$VpcEndpointConnectionId` (it
/// is a plain `String` — `smithy_query(query="shape",shape="VpcEndpointConnection")`), so
/// the id takes EC2's own `<prefix>-<hex>` form through [`ComAmazonawsEc2Amazonec2State::next_id`],
/// with a prefix naming the resource. It is minted by the store and never taken from a
/// request.
pub const VPC_ENDPOINT_CONNECTION_ID_PREFIX: &str = "vpce-conn";

/// The id prefix a stored VPC endpoint is minted with: `vpce-`, as
/// `DescribeVpcEndpoints`'s own evidence of an unknown endpoint id (`vpce-…`) spells it.
pub const VPC_ENDPOINT_ID_PREFIX: &str = "vpce";

/// The id prefix a stored VPC endpoint SERVICE configuration is minted with.
///
/// `vpce-svc-` rather than `vpce-`: a service id and an endpoint id are different
/// resources, and an endpoint id is therefore one whose prefix is `vpce-` and which is
/// NOT one of the longer `vpce-svc-`/`vpce-conn-` forms — see
/// [`vpc_endpoint_id_is_well_formed`].
pub const VPC_ENDPOINT_SERVICE_ID_PREFIX: &str = "vpce-svc";

/// Every state a VPC endpoint can be in, as EC2 REPORTS it.
///
/// The model enumerates them on the filter that selects on this very state:
/// "`vpc-endpoint-state` - The state of the endpoint (`pendingAcceptance` | `pending` |
/// `available` | `deleting` | `deleted` | `rejected` | `failed`)"
/// (`DescribeVpcEndpointsRequest$Filters`, `DescribeVpcEndpointConnectionsRequest$Filters`),
/// and a filter's values are the values the attribute it selects on carries — so this is
/// the spelling `VpcEndpoint$State` and `VpcEndpointConnection$VpcEndpointState` are
/// reported in. Both members target the `State` enum, whose MEMBER NAMES are capitalized
/// (`crate::ops::types::STATE_VALUES`) and also serve states of other resources (`Expired`,
/// `Partial`); the member names are not what this attribute reports.
pub const VPC_ENDPOINT_STATE_VALUES: [&str; 7] = [
    "pendingAcceptance",
    "pending",
    "available",
    "deleting",
    "deleted",
    "rejected",
    "failed",
];

/// An endpoint waiting for the service owner to accept it, which is the state an endpoint to
/// a service with `AcceptanceRequired` enters — as [`VPC_ENDPOINT_STATE_VALUES`] spells it.
pub const VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE: &str = "pendingAcceptance";

/// "The type of endpoint … Default: Gateway" (`CreateVpcEndpointRequest$VpcEndpointType`):
/// the `VpcEndpointType` enum's `Gateway` member
/// ([`crate::ops::types::VPC_ENDPOINT_TYPE_VALUES`]), and the type an endpoint is created
/// as when the request names none.
pub const VPC_ENDPOINT_TYPE_GATEWAY: &str = "Gateway";

/// An endpoint served by endpoint network interfaces in the VPC's subnets: that enum's
/// `Interface` member.
pub const VPC_ENDPOINT_TYPE_INTERFACE: &str = "Interface";

/// An endpoint served by a Gateway Load Balancer: that enum's `GatewayLoadBalancer`
/// member.
pub const VPC_ENDPOINT_TYPE_GATEWAY_LOAD_BALANCER: &str = "GatewayLoadBalancer";

/// The `ResourceType` enum's `vpc-endpoint` member: the one resource type a
/// `CreateVpcEndpoint` `TagSpecification` may name, since that is the resource it creates.
pub const RESOURCE_TYPE_VPC_ENDPOINT: &str = "vpc-endpoint";

/// DNS records for IPv4 addresses only: the `DnsRecordIpType` enum's `ipv4` member
/// ([`crate::ops::types::DNS_RECORD_IP_TYPE_VALUES`]), which is what an endpoint whose
/// address type is [`IP_ADDRESS_TYPE_IPV4`] has records for.
pub const DNS_RECORD_IP_TYPE_IPV4: &str = "ipv4";

/// The prefix of every endpoint service name Amazon Web Services itself offers, as the
/// `ServiceName` doc's own examples spell it (`com.amazonaws.us-east-1.s3`).
pub const AWS_ENDPOINT_SERVICE_PREFIX: &str = "com.amazonaws.";

/// How many characters a VPC endpoint policy may carry: the published Amazon VPC quota for
/// "VPC endpoint policy size", which is what bounds the document a create reads and stores.
pub const MAX_VPC_ENDPOINT_POLICY_CHARS: usize = 20_480;

/// Read the `PolicyDocument` a request attaches to a VPC endpoint, or refuse it.
///
/// "(Interface and gateway endpoints) A policy to attach to the endpoint that controls access
/// to the service. The policy must be in valid JSON format"
/// (`CreateVpcEndpointRequest$PolicyDocument`). The ONE reading of that member, so the create
/// and the modify cannot hold one caller's document to different terms: it is held to
/// [`MAX_VPC_ENDPOINT_POLICY_CHARS`] BEFORE it is parsed, so the work of reading it is bounded
/// by the published quota rather than by what a caller chose to send, and the parse itself is
/// the JSON parser this crate already depends on rather than a hand-rolled shape check, so
/// "valid JSON format" means what JSON means.
///
/// `None` is a request that named no document. What that means is the CALLER's: the default
/// full-access policy for a create, and "leave the attached one alone" for a modify.
pub fn requested_vpc_endpoint_policy<E>(
    policy_document: Option<&str>,
) -> Result<Option<String>, Scaffolded<E>> {
    let Some(document) = policy_document.filter(|document| !document.trim().is_empty()) else {
        return Ok(None);
    };
    if document.chars().count() > MAX_VPC_ENDPOINT_POLICY_CHARS {
        return Err(invalid_parameter_value(format!(
            "Invalid value for policyDocument: a VPC endpoint policy may carry at most {max} \
             characters",
            max = MAX_VPC_ENDPOINT_POLICY_CHARS,
        )));
    }
    if ::serde_json::from_str::<::serde_json::Value>(document).is_err() {
        return Err(invalid_parameter_value(
            "Invalid value for policyDocument: the policy must be in valid JSON format",
        ));
    }
    Ok(Some(document.to_string()))
}

/// The further label a partner or Marketplace endpoint service name carries between that
/// prefix and its Region (`com.amazonaws.vpce.<region>.vpce-svc-…`).
pub const PARTNER_ENDPOINT_SERVICE_LABEL: &str = "vpce.";

/// The second prefix an endpoint service name of Amazon Web Services takes:
/// `aws.api.<region>.<service>`, the form the services that publish their private-link
/// name under the `aws.api` label are called by (the Amazon EMR session endpoint service,
/// `aws.api.<region>.emr-service-cell01`, is one). A name under either prefix names a
/// service of the Region it carries, so both are read by
/// [`parse_endpoint_service_name`] and both appear in the Region's catalog.
pub const AWS_API_ENDPOINT_SERVICE_PREFIX: &str = "aws.api.";

/// The third prefix an endpoint service name of Amazon Web Services takes:
/// `api.aws.<region>.<service>`, the form the services published under the `api.aws` label
/// are called by (the Amazon Keyspaces streams endpoint service,
/// `api.aws.<region>.cassandra-streams`, is one). Read by
/// [`parse_endpoint_service_name`] like the other two, so a name of this family resolves to
/// the service of the Region its name carries.
pub const API_AWS_ENDPOINT_SERVICE_PREFIX: &str = "api.aws.";

/// The DNS suffix of the endpoint-specific names an interface endpoint answers on, under
/// which the Region's own label sits (`<endpoint>.<service>.<region>.vpce.amazonaws.com`).
pub const ENDPOINT_DNS_SUFFIX: &str = "vpce.amazonaws.com";

/// The DNS suffix of the default public name of a service of a Region, which private DNS
/// resolves inside the VPC — "the default public DNS name for the service for the Region
/// (for example, `kinesis.us-east-1.amazonaws.com`)"
/// (`CreateVpcEndpointRequest$PrivateDnsEnabled`).
pub const SERVICE_DNS_SUFFIX: &str = "amazonaws.com";

/// `InvalidServiceName` (HTTP 400): the request named something that is not the name of an
/// endpoint service.
pub const INVALID_SERVICE_NAME: &str = "InvalidServiceName";

/// `VpcEndpointLimitExceeded` (HTTP 400): the account already holds as many VPC endpoints
/// as this emulator stores.
pub const VPC_ENDPOINT_LIMIT_EXCEEDED: &str = "VpcEndpointLimitExceeded";

/// Refuse a request that named something no endpoint service of the Region is called, the
/// way EC2 does: HTTP 400 [`INVALID_SERVICE_NAME`], naming the value at fault.
pub fn invalid_service_name<E>(service_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SERVICE_NAME,
        400,
        format!("The Vpc Endpoint Service '{service_name}' does not exist"),
    )
}

/// Refuse a create that would take the account past the number of VPC endpoints this
/// emulator stores: HTTP 400 [`VPC_ENDPOINT_LIMIT_EXCEEDED`], the quota code EC2 answers
/// for one endpoint too many — the same contract as [`vpc_limit_exceeded`].
pub fn vpc_endpoint_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_ENDPOINT_LIMIT_EXCEEDED,
        400,
        format!(
            "This emulator stores at most {max} VPC endpoints",
            max = MAX_STORED_RECORDS_PER_STORE,
        ),
    )
}

/// `DuplicateSubnetsInSameZone` (HTTP 400): the request named two subnets of one
/// Availability Zone for an endpoint that has at most one network interface per zone.
pub const DUPLICATE_SUBNETS_IN_SAME_ZONE: &str = "DuplicateSubnetsInSameZone";

/// Refuse a request naming a subnet in a zone the endpoint is already served in: HTTP 400
/// [`DUPLICATE_SUBNETS_IN_SAME_ZONE`], naming the subnet the REQUEST asked for — the one whose
/// zone is already taken — and stating the rule it breaks, the way EC2 words this refusal.
pub fn duplicate_subnets_in_same_zone<E>(subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        DUPLICATE_SUBNETS_IN_SAME_ZONE,
        400,
        format!(
            "Found another VPC endpoint subnet in the availability zone of {subnet_id}. VPC \
             endpoint subnets should be in different availability zones supported by the VPC \
             endpoint service."
        ),
    )
}

/// `InvalidParameter` (HTTP 400): a member's value cannot be honoured given the state of the
/// resource it names — as opposed to [`INVALID_PARAMETER_VALUE`], which is about the value
/// itself.
pub const INVALID_PARAMETER: &str = "InvalidParameter";

/// Refuse a request asking to associate a private hosted zone with a VPC whose DNS
/// attributes do not admit one: HTTP 400 [`INVALID_PARAMETER`], in the wording a recorded
/// refusal of this class pins — the sentence names both attributes that have to be on and the
/// VPC they are off for.
///
/// "To use a private hosted zone, you must set the following VPC attributes to true:
/// enableDnsHostnames and enableDnsSupport"
/// (`CreateVpcEndpointRequest$PrivateDnsEnabled`), which [`VpcAttributes`] holds.
pub fn private_dns_unsupported_vpc<E>(vpc_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER,
        400,
        format!(
            "Enabling private DNS requires both enableDnsSupport and enableDnsHostnames VPC \
             attributes set to true for {vpc_id}"
        ),
    )
}

/// What an endpoint service name says about the service it names: the Region hosting it and
/// the service's own label within that Region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointServiceName {
    /// "The Region where the service is hosted" (`VpcEndpoint$ServiceRegion`), as the name
    /// itself carries it.
    pub region: String,
    /// The service's label under that Region — `logs` in `com.amazonaws.us-west-2.logs`.
    /// This is the label the endpoint-specific and private DNS names are built on.
    pub service: String,
}

/// Read an endpoint service name, or `None` when the value is not one at all.
///
/// A service name is `com.amazonaws.<region>.<service>`,
/// `com.amazonaws.vpce.<region>.<service>` for one a partner offers, or
/// `aws.api.<region>.<service>` / `api.aws.<region>.<service>` for one published under the
/// `aws.api` or `api.aws` label — the forms the
/// `ServiceName` doc's own examples take ([`AWS_ENDPOINT_SERVICE_PREFIX`],
/// [`PARTNER_ENDPOINT_SERVICE_LABEL`]) plus the ones those two families take
/// ([`AWS_API_ENDPOINT_SERVICE_PREFIX`], [`API_AWS_ENDPOINT_SERVICE_PREFIX`]). The Region
/// label has to be a Region name
/// ([`region_name_is_well_formed`]) and the service label a non-empty run of the characters
/// a DNS label of a service name is made of, since both end up in the DNS names the
/// endpoint answers on. The ONE reading of a service name, so every operation that resolves
/// one agrees about what it names — and so a name of a published family is never refused as
/// a value that is no service name.
pub fn parse_endpoint_service_name(service_name: &str) -> Option<EndpointServiceName> {
    let within_region = match service_name.strip_prefix(AWS_ENDPOINT_SERVICE_PREFIX) {
        Some(within_amazon) => within_amazon
            .strip_prefix(PARTNER_ENDPOINT_SERVICE_LABEL)
            .unwrap_or(within_amazon),
        None => service_name
            .strip_prefix(AWS_API_ENDPOINT_SERVICE_PREFIX)
            .or_else(|| service_name.strip_prefix(API_AWS_ENDPOINT_SERVICE_PREFIX))?,
    };
    let (region, service) = within_region.split_once('.')?;
    if !region_name_is_well_formed(region) {
        return None;
    }
    if service.is_empty()
        || !service.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '.'
        })
    {
        return None;
    }
    Some(EndpointServiceName {
        region: region.to_string(),
        service: service.to_string(),
    })
}

// =============================================================================
// The endpoint services available in a Region
// =============================================================================
//
// Nothing in the EC2 API creates, renames or withdraws an endpoint service Amazon Web
// Services offers: it offers the services a Region HAS, exactly as a Region HAS its
// Availability Zones ([`availability_zones_in_region`]) and its published prefix lists
// ([`service_prefix_lists_in_region`]). So the catalog below is DERIVED from the Region a
// request asks about rather than kept in [`ComAmazonawsEc2Amazonec2State`], and the describe
// that reports it writes nothing at all: two calls asking about one Region report the same
// services, and a read can never change what a later caller sees.
//
// The services the ACCOUNT offers are a different thing and ARE stored
// ([`ComAmazonawsEc2Amazonec2State::vpc_endpoint_services`]). Both are projected into the
// same [`EndpointServiceRecord`], so one reading of "an endpoint service a caller can create
// an endpoint to" serves whichever of the two it came from.

/// The owner reported for an endpoint service Amazon Web Services offers itself, rather than
/// the account id a service the account created reports: the ALIAS the `owner` filter selects
/// such a service by ("`owner` - The ID or alias of the Amazon Web Services account that owns
/// the service" — `DescribeVpcEndpointServicesRequest$Filters`).
pub const AWS_ENDPOINT_SERVICE_OWNER_ALIAS: &str = "amazon";

/// A private DNS name whose ownership is settled: the `DnsNameState` enum's `verified` member
/// ([`crate::ops::types::DNS_NAME_STATE_VALUES`]). "Consumers of the endpoint service cannot
/// use the private name when the state is not `verified`"
/// (`ServiceDetail$PrivateDnsNameVerificationState`), and the default public name of a service
/// of a Region is the service's own — there is no third party whose ownership of it could
/// still be pending — so a service out of the catalog below reports exactly this state.
pub const DNS_NAME_STATE_VERIFIED: &str = "verified";

/// The one member of the `PayerResponsibility` enum
/// ([`crate::ops::types::PAYER_RESPONSIBILITY_VALUES`]): the service's owner pays.
pub const PAYER_RESPONSIBILITY_SERVICE_OWNER: &str = "ServiceOwner";

/// One service of the catalog: the label it sits under in a Region, and the endpoint types it
/// is reachable through.
pub(super) struct AwsEndpointService {
    /// The service's label under the Region, as an endpoint service name carries it — `logs`
    /// in `com.amazonaws.us-west-2.logs`, and the label [`parse_endpoint_service_name`] reads
    /// back out of that name.
    pub(super) label: &'static str,
    /// The `ServiceType` enum members this service is reachable through
    /// ([`crate::ops::types::SERVICE_TYPE_VALUES`]). A LIST, because `ServiceDetail$ServiceType`
    /// is one: Amazon S3 is reachable both through a gateway endpoint and through an interface
    /// endpoint under the one service name.
    pub(super) types: &'static [&'static str],
}

/// A service reachable only through an interface endpoint, which is what an endpoint service
/// of Amazon Web Services is unless it is one of the two gateway services.
pub(super) const INTERFACE_ONLY: &[&str] = &[VPC_ENDPOINT_TYPE_INTERFACE];

/// A service reachable only through a gateway endpoint.
pub(super) const GATEWAY_ONLY: &[&str] = &[VPC_ENDPOINT_TYPE_GATEWAY];

/// A service reachable through either, reported in the order the `ServiceType` enum declares
/// its members.
pub(super) const GATEWAY_AND_INTERFACE: &[&str] = &[VPC_ENDPOINT_TYPE_GATEWAY, VPC_ENDPOINT_TYPE_INTERFACE];

/// The endpoint services Amazon Web Services offers in every Region.
///
/// The source is the Amazon Web Services PrivateLink User Guide's own list of the services
/// that integrate with PrivateLink, taking each service's endpoint service label and the kind
/// of endpoint it is reached through; the two gateway services are the ones the guide's
/// "Gateway endpoints" page names (Amazon S3 and Amazon DynamoDB). Sorted by label, which
/// [`endpoint_services_in_region`] relies on for the order it reports in.
///
/// Every Region offers every entry: which Regions a given service has reached is a fact of
/// that service's own rollout, which neither this model nor a request carries and this
/// emulator holds no copy of, so nothing here distinguishes Regions by catalog.
pub(super) const AWS_ENDPOINT_SERVICES: &[AwsEndpointService] = &[
    AwsEndpointService { label: "access-analyzer", types: INTERFACE_ONLY },
    AwsEndpointService { label: "account", types: INTERFACE_ONLY },
    AwsEndpointService { label: "acm-pca", types: INTERFACE_ONLY },
    AwsEndpointService { label: "airflow.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "airflow.env", types: INTERFACE_ONLY },
    AwsEndpointService { label: "airflow.ops", types: INTERFACE_ONLY },
    AwsEndpointService { label: "amplify", types: INTERFACE_ONLY },
    AwsEndpointService { label: "aps", types: INTERFACE_ONLY },
    AwsEndpointService { label: "aps-workspaces", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appconfig", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appconfigdata", types: INTERFACE_ONLY },
    AwsEndpointService { label: "application-autoscaling", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appmesh-envoy-management", types: INTERFACE_ONLY },
    AwsEndpointService { label: "apprunner", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appstream.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appstream.streaming", types: INTERFACE_ONLY },
    AwsEndpointService { label: "appsync-api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "athena", types: INTERFACE_ONLY },
    AwsEndpointService { label: "auditmanager", types: INTERFACE_ONLY },
    AwsEndpointService { label: "autoscaling", types: INTERFACE_ONLY },
    AwsEndpointService { label: "autoscaling-plans", types: INTERFACE_ONLY },
    AwsEndpointService { label: "backup", types: INTERFACE_ONLY },
    AwsEndpointService { label: "batch", types: INTERFACE_ONLY },
    AwsEndpointService { label: "bedrock", types: INTERFACE_ONLY },
    AwsEndpointService { label: "bedrock-agent", types: INTERFACE_ONLY },
    AwsEndpointService { label: "bedrock-agent-runtime", types: INTERFACE_ONLY },
    AwsEndpointService { label: "bedrock-runtime", types: INTERFACE_ONLY },
    AwsEndpointService { label: "braket", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cassandra", types: INTERFACE_ONLY },
    AwsEndpointService { label: "clouddirectory", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cloudformation", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cloudhsmv2", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cloudtrail", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codeartifact.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codeartifact.repositories", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codebuild", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codecommit", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codedeploy", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codedeploy-commands-secure", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codeguru-profiler", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codeguru-reviewer", types: INTERFACE_ONLY },
    AwsEndpointService { label: "codepipeline", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cognito-identity", types: INTERFACE_ONLY },
    AwsEndpointService { label: "cognito-idp", types: INTERFACE_ONLY },
    AwsEndpointService { label: "comprehend", types: INTERFACE_ONLY },
    AwsEndpointService { label: "comprehendmedical", types: INTERFACE_ONLY },
    AwsEndpointService { label: "config", types: INTERFACE_ONLY },
    AwsEndpointService { label: "databrew", types: INTERFACE_ONLY },
    AwsEndpointService { label: "dataexchange", types: INTERFACE_ONLY },
    AwsEndpointService { label: "datasync", types: INTERFACE_ONLY },
    AwsEndpointService { label: "datazone", types: INTERFACE_ONLY },
    AwsEndpointService { label: "dms", types: INTERFACE_ONLY },
    AwsEndpointService { label: "drs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ds", types: INTERFACE_ONLY },
    AwsEndpointService { label: "dynamodb", types: GATEWAY_ONLY },
    AwsEndpointService { label: "ebs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ec2", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ec2messages", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ecr.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ecr.dkr", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ecs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ecs-agent", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ecs-telemetry", types: INTERFACE_ONLY },
    AwsEndpointService { label: "eks", types: INTERFACE_ONLY },
    AwsEndpointService { label: "eks-auth", types: INTERFACE_ONLY },
    AwsEndpointService { label: "elasticache", types: INTERFACE_ONLY },
    AwsEndpointService { label: "elasticbeanstalk", types: INTERFACE_ONLY },
    AwsEndpointService { label: "elasticfilesystem", types: INTERFACE_ONLY },
    AwsEndpointService { label: "elasticloadbalancing", types: INTERFACE_ONLY },
    AwsEndpointService { label: "elasticmapreduce", types: INTERFACE_ONLY },
    AwsEndpointService { label: "email-smtp", types: INTERFACE_ONLY },
    AwsEndpointService { label: "emr-containers", types: INTERFACE_ONLY },
    AwsEndpointService { label: "emr-serverless", types: INTERFACE_ONLY },
    AwsEndpointService { label: "events", types: INTERFACE_ONLY },
    AwsEndpointService { label: "execute-api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "finspace", types: INTERFACE_ONLY },
    AwsEndpointService { label: "fsx", types: INTERFACE_ONLY },
    AwsEndpointService { label: "git-codecommit", types: INTERFACE_ONLY },
    AwsEndpointService { label: "glue", types: INTERFACE_ONLY },
    AwsEndpointService { label: "grafana", types: INTERFACE_ONLY },
    AwsEndpointService { label: "greengrass", types: INTERFACE_ONLY },
    AwsEndpointService { label: "groundstation", types: INTERFACE_ONLY },
    AwsEndpointService { label: "guardduty-data", types: INTERFACE_ONLY },
    AwsEndpointService { label: "healthlake", types: INTERFACE_ONLY },
    AwsEndpointService { label: "identitystore", types: INTERFACE_ONLY },
    AwsEndpointService { label: "inspector2", types: INTERFACE_ONLY },
    AwsEndpointService { label: "iot.data", types: INTERFACE_ONLY },
    AwsEndpointService { label: "iotsitewise.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "kendra", types: INTERFACE_ONLY },
    AwsEndpointService { label: "kinesis-firehose", types: INTERFACE_ONLY },
    AwsEndpointService { label: "kinesis-streams", types: INTERFACE_ONLY },
    AwsEndpointService { label: "kms", types: INTERFACE_ONLY },
    AwsEndpointService { label: "lakeformation", types: INTERFACE_ONLY },
    AwsEndpointService { label: "lambda", types: INTERFACE_ONLY },
    AwsEndpointService { label: "license-manager", types: INTERFACE_ONLY },
    AwsEndpointService { label: "logs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "macie2", types: INTERFACE_ONLY },
    AwsEndpointService { label: "memory-db", types: INTERFACE_ONLY },
    AwsEndpointService { label: "monitoring", types: INTERFACE_ONLY },
    AwsEndpointService { label: "personalize", types: INTERFACE_ONLY },
    AwsEndpointService { label: "pinpoint", types: INTERFACE_ONLY },
    AwsEndpointService { label: "polly", types: INTERFACE_ONLY },
    AwsEndpointService { label: "profile", types: INTERFACE_ONLY },
    AwsEndpointService { label: "qldb.session", types: INTERFACE_ONLY },
    AwsEndpointService { label: "quicksight", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ram", types: INTERFACE_ONLY },
    AwsEndpointService { label: "rbin", types: INTERFACE_ONLY },
    AwsEndpointService { label: "rds", types: INTERFACE_ONLY },
    AwsEndpointService { label: "rds-data", types: INTERFACE_ONLY },
    AwsEndpointService { label: "redshift", types: INTERFACE_ONLY },
    AwsEndpointService { label: "redshift-data", types: INTERFACE_ONLY },
    AwsEndpointService { label: "rekognition", types: INTERFACE_ONLY },
    AwsEndpointService { label: "resource-groups", types: INTERFACE_ONLY },
    AwsEndpointService { label: "route53resolver", types: INTERFACE_ONLY },
    AwsEndpointService { label: "s3", types: GATEWAY_AND_INTERFACE },
    AwsEndpointService { label: "s3express", types: INTERFACE_ONLY },
    AwsEndpointService { label: "sagemaker.api", types: INTERFACE_ONLY },
    AwsEndpointService { label: "sagemaker.runtime", types: INTERFACE_ONLY },
    AwsEndpointService { label: "scheduler", types: INTERFACE_ONLY },
    AwsEndpointService { label: "secretsmanager", types: INTERFACE_ONLY },
    AwsEndpointService { label: "securityhub", types: INTERFACE_ONLY },
    AwsEndpointService { label: "serverlessrepo", types: INTERFACE_ONLY },
    AwsEndpointService { label: "servicecatalog", types: INTERFACE_ONLY },
    AwsEndpointService { label: "servicediscovery", types: INTERFACE_ONLY },
    AwsEndpointService { label: "snow-device-management", types: INTERFACE_ONLY },
    AwsEndpointService { label: "sns", types: INTERFACE_ONLY },
    AwsEndpointService { label: "sqs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ssm", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ssm-contacts", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ssm-incidents", types: INTERFACE_ONLY },
    AwsEndpointService { label: "ssmmessages", types: INTERFACE_ONLY },
    AwsEndpointService { label: "states", types: INTERFACE_ONLY },
    AwsEndpointService { label: "storagegateway", types: INTERFACE_ONLY },
    AwsEndpointService { label: "sts", types: INTERFACE_ONLY },
    AwsEndpointService { label: "swf", types: INTERFACE_ONLY },
    AwsEndpointService { label: "synthetics", types: INTERFACE_ONLY },
    // The DEDICATED regional service a CloudWatch Logs syslog ingestion endpoint is created
    // to, which the PrivateLink guide publishes beside the plain `logs` API service: the two
    // are different services, and only an endpoint to this one carries syslog data into a log
    // group.
    AwsEndpointService { label: "syslog-logs", types: INTERFACE_ONLY },
    AwsEndpointService { label: "textract", types: INTERFACE_ONLY },
    AwsEndpointService { label: "timestream", types: INTERFACE_ONLY },
    AwsEndpointService { label: "transcribe", types: INTERFACE_ONLY },
    AwsEndpointService { label: "transfer", types: INTERFACE_ONLY },
    AwsEndpointService { label: "translate", types: INTERFACE_ONLY },
    AwsEndpointService { label: "waf-regional", types: INTERFACE_ONLY },
    AwsEndpointService { label: "wisdom", types: INTERFACE_ONLY },
    AwsEndpointService { label: "workspaces", types: INTERFACE_ONLY },
    AwsEndpointService { label: "xray", types: INTERFACE_ONLY },
];

/// The endpoint services Amazon Web Services offers in every Region under the `aws.api`
/// label ([`AWS_API_ENDPOINT_SERVICE_PREFIX`]) rather than under `com.amazonaws`.
///
/// Amazon EMR's session endpoint service is one: a client that runs Spark Connect sessions
/// on a cluster reaches them through an interface endpoint to
/// `aws.api.<region>.emr-service-cell01`, the service cell EMR publishes sessions under.
/// Sorted by label, the same way [`AWS_ENDPOINT_SERVICES`] is.
///
/// The family has further members, and each is added here under the label Amazon Web
/// Services publishes it as; a label nothing states is not guessed at, since the name IS
/// the service's identity and an invented one would name a service no Region offers. This
/// catalogue is what decides whether a name of the family names a service at all
/// ([`endpoint_service_name_is_offered`]), so a member it does not list is refused by the
/// create exactly as `DescribeVpcEndpointServices` refuses it — the two cannot disagree
/// about which services exist.
pub(super) const AWS_API_ENDPOINT_SERVICES: &[AwsEndpointService] =
    &[AwsEndpointService { label: "emr-service-cell01", types: INTERFACE_ONLY }];

/// The endpoint services Amazon Web Services offers in every Region under the `api.aws`
/// label ([`API_AWS_ENDPOINT_SERVICE_PREFIX`]).
///
/// Amazon Keyspaces publishes the change-data-capture streams of its Apache Cassandra-
/// compatible tables as one: a client reaches them through an interface endpoint to
/// `api.aws.<region>.cassandra-streams`, beside the table API's own
/// `com.amazonaws.<region>.cassandra`. Read and reported exactly like the other two
/// families, so a name of this one is never refused as a value that is no service name.
/// Sorted by label, the way [`AWS_ENDPOINT_SERVICES`] is.
pub(super) const API_AWS_ENDPOINT_SERVICES: &[AwsEndpointService] =
    &[AwsEndpointService { label: "cassandra-streams", types: INTERFACE_ONLY }];

/// One endpoint service a caller can create a VPC endpoint to, whichever of the two sources it
/// came from: a service of the Region's catalog, or one of the account's own stored service
/// configurations.
///
/// The members are the ones `ServiceDetail` reports, held as the types they are reported as
/// rather than as the modelled optionals, so the projection
/// ([`EndpointServiceRecord::to_service_detail`]) is the only place that decides which members
/// a reported service carries and both sources are reported the same way.
#[derive(Debug, Clone)]
pub struct EndpointServiceRecord {
    /// "The name of the service" (`ServiceDetail$ServiceName`):
    /// `com.amazonaws.<region>.<label>` for a service of the catalog, and the name the
    /// configuration carries for one of the account's own.
    pub service_name: String,
    /// "The ID of the endpoint service" (`ServiceDetail$ServiceId`).
    pub service_id: String,
    /// "The type of service" (`ServiceDetail$ServiceType`), as `ServiceType` enum members.
    pub service_types: Vec<String>,
    /// "The Region where the service is hosted" (`ServiceDetail$ServiceRegion`), which is the
    /// Region the service's own NAME carries.
    pub service_region: String,
    /// "The Amazon Web Services account ID of the service owner" (`ServiceDetail$Owner`), or
    /// [`AWS_ENDPOINT_SERVICE_OWNER_ALIAS`] for a service Amazon Web Services owns.
    pub owner: String,
    /// "The Availability Zones in which the service is available"
    /// (`ServiceDetail$AvailabilityZones`).
    pub availability_zones: Vec<String>,
    /// "The IDs of the Availability Zones in which the service is available"
    /// (`ServiceDetail$AvailabilityZoneIds`) — the same zones, by id.
    pub availability_zone_ids: Vec<String>,
    /// "The DNS names for the service" (`ServiceDetail$BaseEndpointDnsNames`).
    pub base_endpoint_dns_names: Vec<String>,
    /// "The private DNS name for the service" (`ServiceDetail$PrivateDnsName`), or `None` for
    /// a service that has none.
    pub private_dns_name: Option<String>,
    /// "Indicates whether the service supports endpoint policies"
    /// (`ServiceDetail$VpcEndpointPolicySupported`).
    pub vpc_endpoint_policy_supported: bool,
    /// "Indicates whether VPC endpoint connection requests to the service must be accepted by
    /// the service owner" (`ServiceDetail$AcceptanceRequired`).
    pub acceptance_required: bool,
    /// "Indicates whether the service manages its VPC endpoints"
    /// (`ServiceDetail$ManagesVpcEndpoints`).
    pub manages_vpc_endpoints: bool,
    /// "The payer responsibility" (`ServiceDetail$PayerResponsibility`), as a
    /// `PayerResponsibility` enum member.
    pub payer_responsibility: Option<String>,
    /// "The verification state of the VPC endpoint service"
    /// (`ServiceDetail$PrivateDnsNameVerificationState`), as a `DnsNameState` enum member;
    /// `None` for a service carrying no private DNS name to verify.
    pub private_dns_name_verification_state: Option<String>,
    /// "The supported IP address types" (`ServiceDetail$SupportedIpAddressTypes`).
    pub supported_ip_address_types: Vec<String>,
    /// "The tags assigned to the service" (`ServiceDetail$Tags`).
    pub tags: Vec<Tag>,
}

impl EndpointServiceRecord {
    /// This service as `DescribeVpcEndpointServices` reports it.
    pub fn to_service_detail(&self) -> ServiceDetail {
        ServiceDetail {
            service_name: Some(self.service_name.clone()),
            service_id: Some(self.service_id.clone()),
            service_type: Some(
                self.service_types
                    .iter()
                    .map(|service_type| ServiceTypeDetail {
                        service_type: Some(service_type.clone()),
                    })
                    .collect(),
            ),
            service_region: Some(self.service_region.clone()),
            availability_zone_ids: Some(self.availability_zone_ids.clone()),
            availability_zones: Some(self.availability_zones.clone()),
            owner: Some(self.owner.clone()),
            base_endpoint_dns_names: Some(self.base_endpoint_dns_names.clone()),
            private_dns_name: self.private_dns_name.clone(),
            // "The private DNS names assigned to the VPC endpoint service"
            // (`ServiceDetail$PrivateDnsNames`): the same name the member above carries, so a
            // service with none reports neither rather than an empty list beside an absent
            // name.
            private_dns_names: self.private_dns_name.as_ref().map(|private_dns_name| {
                vec![PrivateDnsDetails {
                    private_dns_name: Some(private_dns_name.clone()),
                }]
            }),
            vpc_endpoint_policy_supported: Some(self.vpc_endpoint_policy_supported),
            acceptance_required: Some(self.acceptance_required),
            manages_vpc_endpoints: Some(self.manages_vpc_endpoints),
            payer_responsibility: self.payer_responsibility.clone(),
            tags: Some(self.tags.clone()),
            private_dns_name_verification_state: self
                .private_dns_name_verification_state
                .clone(),
            supported_ip_address_types: Some(self.supported_ip_address_types.clone()),
        }
    }
}

/// The endpoint services Amazon Web Services offers in Region `region`, in `ServiceName`
/// order — the order a `NextToken` walk of `DescribeVpcEndpointServices` resumes in, and
/// unique by construction, one name per entry of [`AWS_ENDPOINT_SERVICES`], of
/// [`AWS_API_ENDPOINT_SERVICES`] and of [`API_AWS_ENDPOINT_SERVICES`].
///
/// Every reported value is derived from the Region and the entry: the zones come from
/// [`availability_zones_in_region`], so the service is available in exactly the zones the
/// Region has, and the DNS names are built the way `CreateVpcEndpoint` builds the names of an
/// endpoint TO this service — the endpoint-specific names it mints extend
/// [`ENDPOINT_DNS_SUFFIX`] under this service's base name, and the name its private hosted
/// zone answers on is this service's private DNS name — so the catalog and a created endpoint
/// cannot disagree about what the service is called.
pub fn endpoint_services_in_region(region: &str) -> Vec<EndpointServiceRecord> {
    let zones = availability_zones_in_region(region);
    let record_of = |prefix: &str, service: &AwsEndpointService| -> EndpointServiceRecord {
        let service_name = format!("{prefix}{region}.{}", service.label);
        // Only a name under `com.amazonaws` says what the service's default public DNS
        // name is (`<label>.<region>.amazonaws.com`). Nothing in this model or in a request
        // says which public name a service of another published family answers on, and a
        // private hosted zone resolves the service's REAL public name or nothing at all, so
        // such a service reports none here rather than one invented for it — an
        // acknowledged omission about this catalogue's evidence, not a property of the
        // family.
        let publishes_public_name = prefix == AWS_ENDPOINT_SERVICE_PREFIX;
        let interface = service.types.contains(&VPC_ENDPOINT_TYPE_INTERFACE);
        let gateway = service.types.contains(&VPC_ENDPOINT_TYPE_GATEWAY);
        // "The DNS names for the service": the base name an endpoint's own names sit
        // under. An interface endpoint answers on
        // `<endpoint>.<service>.<region>.vpce.amazonaws.com`, so the service's base name
        // under that suffix is what a consumer resolves; a gateway endpoint mints no names
        // of its own and is reached through the service's default public name.
        let mut base_endpoint_dns_names = Vec::new();
        if interface {
            base_endpoint_dns_names.push(format!(
                "{label}.{region}.{ENDPOINT_DNS_SUFFIX}",
                label = service.label,
            ));
        }
        if gateway {
            base_endpoint_dns_names.push(format!(
                "{label}.{region}.{SERVICE_DNS_SUFFIX}",
                label = service.label,
            ));
        }
        EndpointServiceRecord {
            service_id: endpoint_service_id(&service_name),
            service_name,
            service_types: service
                .types
                .iter()
                .map(|service_type| (*service_type).to_string())
                .collect(),
            service_region: region.to_string(),
            owner: AWS_ENDPOINT_SERVICE_OWNER_ALIAS.to_string(),
            availability_zones: zones
                .iter()
                .map(|zone| zone.zone_name.clone())
                .collect(),
            availability_zone_ids: zones.iter().map(|zone| zone.zone_id.clone()).collect(),
            base_endpoint_dns_names,
            // "The default public DNS name for the service for the Region (for example,
            // kinesis.us-east-1.amazonaws.com)" (`CreateVpcEndpointRequest$PrivateDnsEnabled`),
            // which is the name a private hosted zone resolves inside the VPC — so only an
            // interface endpoint has one, a private hosted zone being an interface
            // endpoint's option.
            private_dns_name: (interface && publishes_public_name)
                .then(|| format!("{label}.{region}.{SERVICE_DNS_SUFFIX}", label = service.label)),
            // "(Interface and gateway endpoints) A policy to attach to the endpoint that
            // controls access to the service" (`CreateVpcEndpointRequest$PolicyDocument`):
            // both kinds of endpoint to a service of this catalog take one.
            vpc_endpoint_policy_supported: true,
            // Nothing has to accept a connection to a service Amazon Web Services offers,
            // and it does not manage the endpoints consumers create to it: the endpoint a
            // caller creates is the caller's.
            acceptance_required: false,
            manages_vpc_endpoints: false,
            payer_responsibility: Some(PAYER_RESPONSIBILITY_SERVICE_OWNER.to_string()),
            private_dns_name_verification_state: (interface && publishes_public_name)
                .then(|| DNS_NAME_STATE_VERIFIED.to_string()),
            // "The supported IP address types" (`ServiceDetail$SupportedIpAddressTypes`) is
            // about what the SERVICE answers on. An interface endpoint service answers on
            // both IPv4 and IPv6, the interface endpoint's own addresses being what an
            // endpoint of either type is created with; a gateway service is reached through
            // its route-table entry, which is an IPv4 prefix list
            // ([`service_prefix_lists_in_region`] publishes the IPv6 list separately, under
            // its own name).
            supported_ip_address_types: if interface {
                vec![
                    IP_ADDRESS_TYPE_IPV4.to_string(),
                    IP_ADDRESS_TYPE_IPV6.to_string(),
                ]
            } else {
                vec![IP_ADDRESS_TYPE_IPV4.to_string()]
            },
            // Nobody can tag a service Amazon Web Services offers, there being no resource
            // of the account to tag.
            tags: Vec::new(),
        }
    };
    let mut records: Vec<EndpointServiceRecord> = AWS_ENDPOINT_SERVICES
        .iter()
        .map(|service| record_of(AWS_ENDPOINT_SERVICE_PREFIX, service))
        .chain(
            AWS_API_ENDPOINT_SERVICES
                .iter()
                .map(|service| record_of(AWS_API_ENDPOINT_SERVICE_PREFIX, service)),
        )
        .chain(
            API_AWS_ENDPOINT_SERVICES
                .iter()
                .map(|service| record_of(API_AWS_ENDPOINT_SERVICE_PREFIX, service)),
        )
        .collect();
    // The report is in `ServiceName` order whichever prefix a service's name sits under,
    // since that order is what a page cursor resumes in.
    records.sort_by(|left, right| left.service_name.cmp(&right.service_name));
    // `ServiceNames` selects by name and the page cursor names the service the next page
    // starts at, so two services of one Region sharing a name would make both ambiguous. The
    // catalog's labels are distinct, which keeps the names distinct; the assertion pins that
    // invariant if the catalog grows.
    debug_assert!(
        records
            .windows(2)
            .all(|pair| pair[0].service_name < pair[1].service_name),
        "the endpoint services of one Region must be distinct and sorted by name"
    );
    records
}

/// The id of the endpoint service named `service_name`.
///
/// Derived from the name rather than stored, exactly as [`service_prefix_list_id`] derives a
/// published prefix list's id: every read of one Region's service reports the same id, and two
/// services with different names get different ids, without a store holding either. It carries
/// 8 hexadecimal digits where [`ComAmazonawsEc2Amazonec2State::next_id`] mints 17, so a
/// derived id can never collide with the id of a service configuration the account created.
pub fn endpoint_service_id(service_name: &str) -> String {
    format!(
        "{VPC_ENDPOINT_SERVICE_ID_PREFIX}-{:08x}",
        stable_derived_id_hash(service_name) & 0xffff_ffff
    )
}

/// A service configuration that is in service: the `ServiceState` enum's `Available` member
/// ([`crate::ops::types::SERVICE_STATE_VALUES`]). A service is offered as soon as it has been
/// configured — the load balancers behind it already exist, which is the prerequisite
/// `CreateVpcEndpointServiceConfiguration` documents ("before you create an endpoint service, you
/// must create one of the following for your service") — so a created configuration is reported
/// in this terminal state and the clock owes it no move.
pub const SERVICE_STATE_AVAILABLE: &str = "Available";

/// A private DNS name whose ownership has NOT been proven yet: the `DnsNameState` enum's
/// `pendingVerification` member ([`crate::ops::types::DNS_NAME_STATE_VALUES`]). "If you set the
/// private DNS name, you must prove that you own the private DNS domain name"
/// (`CreateVpcEndpointServiceConfiguration`), so a name the owner has just assigned starts here
/// rather than at [`DNS_NAME_STATE_VERIFIED`], which is what a service of the Region's own
/// catalog reports.
pub const DNS_NAME_STATE_PENDING_VERIFICATION: &str = "pendingVerification";

/// The `ResourceType` enum's `vpc-endpoint-service` member
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]): the one type a
/// `CreateVpcEndpointServiceConfiguration` `TagSpecification` may name, since an endpoint service
/// configuration is the resource it creates.
pub const RESOURCE_TYPE_VPC_ENDPOINT_SERVICE: &str = "vpc-endpoint-service";

/// "The endpoint service verification type, for example TXT"
/// (`PrivateDnsNameConfiguration$Type`): the kind of DNS record the owner of a private DNS name
/// publishes the verification value in.
pub const PRIVATE_DNS_VERIFICATION_RECORD_TYPE: &str = "TXT";

/// `VpcEndpointServiceLimitExceeded` (HTTP 400): the account already offers as many endpoint
/// services as this emulator stores.
pub const VPC_ENDPOINT_SERVICE_LIMIT_EXCEEDED: &str = "VpcEndpointServiceLimitExceeded";

/// Refuse a create that would take the account past the number of endpoint service configurations
/// this emulator stores: HTTP 400 [`VPC_ENDPOINT_SERVICE_LIMIT_EXCEEDED`] — the same
/// quota-code-and-emulator-ceiling contract [`vpc_endpoint_limit_exceeded`] has for one endpoint
/// too many.
pub fn vpc_endpoint_service_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_ENDPOINT_SERVICE_LIMIT_EXCEEDED,
        400,
        format!(
            "This emulator stores at most {max} VPC endpoint service configurations",
            max = MAX_STORED_RECORDS_PER_STORE,
        ),
    )
}

/// Whether an endpoint service called `service_name` is one a caller of this account can
/// create a VPC endpoint TO.
///
/// ONE catalogue decides which service names exist, so the create and
/// `DescribeVpcEndpointServices` cannot disagree: a name that describe refuses as naming no
/// service ([`invalid_service_name`]) is not a name an endpoint can be created to either.
/// The two sources are the same two that describe reports — the services Amazon Web Services
/// publishes in the Region the NAME carries ([`endpoint_services_in_region`]), and the
/// account's own stored service configurations
/// ([`ComAmazonawsEc2Amazonec2State::vpc_endpoint_services`], read through
/// [`account_endpoint_service_record`] so one reading of "a service a caller can reach" serves
/// both).
///
/// The PARTNER family (`com.amazonaws.vpce.<region>.<service-id>`,
/// [`PARTNER_ENDPOINT_SERVICE_LABEL`]) is the exception, and it is one about this emulator's
/// knowledge rather than about the rule: such a name belongs to a service ANOTHER account or
/// an Amazon Web Services Marketplace partner offers, which this service holds no catalogue of
/// at all — only the account's own configurations and the Region's published services are
/// enumerable here. So a name of that family is read for FORM: its service label has to be an
/// endpoint service id ([`VPC_ENDPOINT_SERVICE_ID_PREFIX`]), which is what a service of another
/// account is named by ([`account_endpoint_service_name`]), and anything else names no service.
pub fn endpoint_service_name_is_offered(
    store: &ComAmazonawsEc2Amazonec2State,
    service_name: &str,
) -> bool {
    let Some(parsed) = parse_endpoint_service_name(service_name) else {
        return false;
    };
    // The account's own services first: the cheapest source, and the one a name of either
    // family may sit in.
    if store
        .vpc_endpoint_services
        .values()
        .filter_map(account_endpoint_service_record)
        .any(|service| service.service_name == service_name)
    {
        return true;
    }
    if service_name.starts_with(AWS_ENDPOINT_SERVICE_PREFIX)
        && service_name[AWS_ENDPOINT_SERVICE_PREFIX.len()..]
            .starts_with(PARTNER_ENDPOINT_SERVICE_LABEL)
    {
        return prefixed_id_is_well_formed(VPC_ENDPOINT_SERVICE_ID_PREFIX, &parsed.service);
    }
    endpoint_services_in_region(&parsed.region)
        .iter()
        .any(|service| service.service_name == service_name)
}

/// The endpoint TYPES the endpoint service called `service_name` is reachable through — the
/// `ServiceType` enum members `ServiceDetail$ServiceType` reports for it — or `None` for a
/// name this service holds no catalogue entry for.
///
/// The two sources are the two [`endpoint_service_name_is_offered`] resolves a name against,
/// read in the same order, so the types a create of an endpoint is held to are exactly the
/// ones `DescribeVpcEndpointServices` reports for that service. `None` is the PARTNER family
/// (`com.amazonaws.vpce.<region>.<service-id>`), whose service belongs to another account and
/// whose types are therefore not knowable here: such a name is read for form only, the way
/// that function reads it, rather than held to a type list this emulator does not have.
pub fn endpoint_service_types(
    store: &ComAmazonawsEc2Amazonec2State,
    service_name: &str,
) -> Option<Vec<String>> {
    let parsed = parse_endpoint_service_name(service_name)?;
    if let Some(service) = store
        .vpc_endpoint_services
        .values()
        .filter_map(account_endpoint_service_record)
        .find(|service| service.service_name == service_name)
    {
        return Some(service.service_types);
    }
    endpoint_services_in_region(&parsed.region)
        .into_iter()
        .find(|service| service.service_name == service_name)
        .map(|service| service.service_types)
}

/// The DEFAULT PUBLIC DNS NAME of the endpoint service called `service_name` — the name a
/// private hosted zone resolves inside the VPC — or `None` when the service publishes none,
/// and for a name this service holds no catalogue entry for.
///
/// "(Interface endpoint) Indicates whether to associate a private hosted zone with the
/// specified VPC. The private hosted zone contains a record set for the default public DNS
/// name for the service for the Region" (`CreateVpcEndpointRequest$PrivateDnsEnabled`): that
/// record set is the whole content of the zone, so a service with no such name has no private
/// hosted zone to associate and an endpoint to it resolves nothing privately.
///
/// Read from the same two sources, in the same order, as [`endpoint_service_types`], so the
/// name a create of an endpoint is held to is the one `DescribeVpcEndpointServices` reports
/// for that service ([`EndpointServiceRecord::private_dns_name`]).
pub fn endpoint_service_private_dns_name(
    store: &ComAmazonawsEc2Amazonec2State,
    service_name: &str,
) -> Option<String> {
    let parsed = parse_endpoint_service_name(service_name)?;
    if let Some(service) = store
        .vpc_endpoint_services
        .values()
        .filter_map(account_endpoint_service_record)
        .find(|service| service.service_name == service_name)
    {
        return service.private_dns_name;
    }
    endpoint_services_in_region(&parsed.region)
        .into_iter()
        .find(|service| service.service_name == service_name)
        .and_then(|service| service.private_dns_name)
}

/// Refuse an endpoint of a type the service it names is not reachable through: HTTP 400
/// [`INVALID_PARAMETER`], naming the type the request asked for and the types the service is
/// available through.
///
/// "The type of endpoint" (`CreateVpcEndpointRequest$VpcEndpointType`) has to be one of the
/// service's own ("the type of service", `ServiceDetail$ServiceType`) — an endpoint of any
/// other type would reach nothing — so the pair is what the message names.
pub fn endpoint_type_mismatch<E>(endpoint_type: &str, service_types: &[String]) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER,
        400,
        format!(
            "Endpoint type ({endpoint_type}) does not match available service types ([{types}]).",
            types = service_types.join(", "),
        ),
    )
}

/// The name of an endpoint service the ACCOUNT offers, for the service `service_id` hosted in
/// `region`.
///
/// The partner form of a service name — `com.amazonaws.vpce.<region>.<service-id>`
/// ([`AWS_ENDPOINT_SERVICE_PREFIX`], [`PARTNER_ENDPOINT_SERVICE_LABEL`]) — whose service label is
/// the service's own id, which is how a service nobody at Amazon Web Services named gets a name
/// no other service can collide with. Derived from the minted id rather than stored twice, and
/// read back by [`parse_endpoint_service_name`], so the Region this name says the service is
/// hosted in is the Region it was created in.
pub fn account_endpoint_service_name(region: &str, service_id: &str) -> String {
    format!("{AWS_ENDPOINT_SERVICE_PREFIX}{PARTNER_ENDPOINT_SERVICE_LABEL}{region}.{service_id}")
}

/// "The DNS names for the service" (`ServiceConfiguration$BaseEndpointDnsNames`) of one the
/// account offers: the service's own label under its Region beneath [`ENDPOINT_DNS_SUFFIX`].
///
/// Built exactly as [`endpoint_services_in_region`] builds the base name of a service of the
/// Region's catalog, so the name an interface endpoint's own names extend is spelled one way
/// whichever of the two sources the service came from.
pub fn account_endpoint_service_base_dns_name(region: &str, service_id: &str) -> String {
    format!("{service_id}.{region}.{ENDPOINT_DNS_SUFFIX}")
}

/// The verification record the owner of `private_dns_name` has to publish for the service
/// `service_id` before consumers may use that name.
///
/// "If you set the private DNS name, you must prove that you own the private DNS domain name"
/// (`CreateVpcEndpointServiceConfiguration`), so the name starts
/// [`DNS_NAME_STATE_PENDING_VERIFICATION`] and the configuration carries what the proof is: "the
/// name of the record subdomain the service provider needs to create"
/// (`PrivateDnsNameConfiguration$Name`) and "the value the service provider adds to the private
/// DNS name domain record before verification" (`$Value`), in the record type
/// [`PRIVATE_DNS_VERIFICATION_RECORD_TYPE`]. Both are DERIVED from the service and the name
/// through [`stable_derived_id_hash`], the way a derived resource id is: every read of one
/// service's configuration reports the same record, and two services asked to verify one name are
/// given different ones. Nothing here is a secret — the record only has to be unguessable to
/// nobody, since this emulator verifies ownership of nothing.
pub fn private_dns_name_verification(
    service_id: &str,
    private_dns_name: &str,
) -> PrivateDnsNameConfiguration {
    let subdomain = stable_derived_id_hash(&format!("name/{service_id}/{private_dns_name}"));
    let value = stable_derived_id_hash(&format!("value/{service_id}/{private_dns_name}"));
    PrivateDnsNameConfiguration {
        state: Some(DNS_NAME_STATE_PENDING_VERIFICATION.to_string()),
        r#type: Some(PRIVATE_DNS_VERIFICATION_RECORD_TYPE.to_string()),
        value: Some(format!("vpce:{value:016x}")),
        name: Some(format!("_{subdomain:016x}")),
    }
}

/// Is `arn` the ARN of an Elastic Load Balancing load balancer —
/// `arn:<partition>:elasticloadbalancing:<region>:<account>:loadbalancer/<rest>`?
///
/// The shape of the members that name the load balancers an endpoint service is served by
/// (`CreateVpcEndpointServiceConfigurationRequest$NetworkLoadBalancerArns` and
/// `$GatewayLoadBalancerArns`). A load balancer is a resource of ANOTHER service, which this
/// store holds none of, so a value is held to the form of a load balancer ARN and no further: the
/// account is not held to this emulator's own, for the reason [`iam_role_arn_is_well_formed`]
/// gives, and WHICH kind of load balancer one is stays the business of the member it was given
/// in, since the two members are exactly how the request says which kind it means.
pub fn load_balancer_arn_is_well_formed(arn: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let labelled = |value: &str| -> bool {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    *prefix == "arn"
        && labelled(partition)
        && *service == ELASTIC_LOAD_BALANCING_ARN_SERVICE
        && labelled(region)
        && !account.is_empty()
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && resource
            .strip_prefix(LOAD_BALANCER_ARN_RESOURCE_PREFIX)
            .is_some_and(|name| !name.is_empty())
}

/// The ARN service namespace of Elastic Load Balancing, which a load balancer's ARN sits in.
pub(super) const ELASTIC_LOAD_BALANCING_ARN_SERVICE: &str = "elasticloadbalancing";

/// The resource-type prefix of a load balancer's ARN resource field — the type every load
/// balancer ARN opens with, whatever kind of load balancer it names.
pub(super) const LOAD_BALANCER_ARN_RESOURCE_PREFIX: &str = "loadbalancer/";

/// The longest a DNS name may be, in bytes: the 253 characters a fully qualified domain name
/// holds (RFC 1035's 255-octet wire form, less the length and root bytes).
pub const MAX_DOMAIN_NAME_BYTES: usize = 253;

/// The longest one label of a DNS name may be, in bytes (RFC 1035).
pub const MAX_DNS_LABEL_BYTES: usize = 63;

/// Is `name` a domain name a caller could assign to a resource of this service and prove
/// ownership of?
///
/// The shape of a member that carries a name consumers RESOLVE, as
/// `CreateVpcEndpointServiceConfigurationRequest$PrivateDnsName` does ("the private DNS name to
/// assign to the VPC endpoint service"): dot-separated labels of letters, digits and inner
/// hyphens, each within [`MAX_DNS_LABEL_BYTES`] and the whole within [`MAX_DOMAIN_NAME_BYTES`],
/// and more than one label, nobody owning a top-level domain. The ONE reading of a domain name
/// in this crate, so every operation that takes one agrees on its shape.
pub fn domain_name_is_well_formed(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_DOMAIN_NAME_BYTES {
        return false;
    }
    let well_formed = |label: &str| -> bool {
        !label.is_empty()
            && label.len() <= MAX_DNS_LABEL_BYTES
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    };
    let mut labels = 0usize;
    for label in name.split('.') {
        if !well_formed(label) {
            return false;
        }
        labels += 1;
    }
    labels > 1
}

/// One of the account's own stored service configurations as an endpoint service a caller can
/// create an endpoint to — `None` when the stored name is not an endpoint service name at all,
/// since the Region the service is hosted in is the one its NAME carries.
///
/// What differs from a service of the catalog is what the OWNER decided, so each member is
/// read off the stored configuration: the account owns it, and
/// [`EndpointServiceRecord::to_service_detail`] then reports both sources identically.
pub fn account_endpoint_service_record(
    configuration: &ServiceConfiguration,
) -> Option<EndpointServiceRecord> {
    let service_name = configuration.service_name.clone()?;
    let parsed = parse_endpoint_service_name(&service_name)?;
    // A configuration carrying no id names no service a consumer could select: every one
    // this store holds was given its id by [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_service`].
    let service_id = configuration.service_id.clone()?;
    Some(EndpointServiceRecord {
        service_name,
        service_id,
        service_types: configuration
            .service_type
            .iter()
            .flatten()
            .filter_map(|detail| detail.service_type.clone())
            .collect(),
        service_region: parsed.region,
        owner: ::engine::emulator_account_id().to_string(),
        availability_zones: configuration.availability_zones.clone().unwrap_or_default(),
        availability_zone_ids: configuration
            .availability_zone_ids
            .clone()
            .unwrap_or_default(),
        base_endpoint_dns_names: configuration
            .base_endpoint_dns_names
            .clone()
            .unwrap_or_default(),
        private_dns_name: configuration.private_dns_name.clone(),
        // "You can create an endpoint policy for interface endpoints to Amazon Web Services
        // services" (the PrivateLink User Guide's endpoint policies): a service another account
        // offers is not one of them, so an endpoint to it carries no policy.
        vpc_endpoint_policy_supported: false,
        acceptance_required: configuration.acceptance_required.unwrap_or(false),
        manages_vpc_endpoints: configuration.manages_vpc_endpoints.unwrap_or(false),
        payer_responsibility: configuration.payer_responsibility.clone(),
        private_dns_name_verification_state: configuration
            .private_dns_name_configuration
            .as_ref()
            .and_then(|private_dns| private_dns.state.clone()),
        supported_ip_address_types: configuration
            .supported_ip_address_types
            .clone()
            .unwrap_or_default(),
        tags: configuration.tags.clone().unwrap_or_default(),
    })
}

/// An endpoint that is in service, as [`VPC_ENDPOINT_STATE_VALUES`] spells it.
pub const VPC_ENDPOINT_STATE_AVAILABLE: &str = "available";

/// An endpoint that has been created, or accepted, and is not in service yet: the
/// non-terminal state the clock owes [`VPC_ENDPOINT_STATE_AVAILABLE`], as
/// [`VPC_ENDPOINT_STATE_VALUES`] spells it.
pub const VPC_ENDPOINT_STATE_PENDING: &str = "pending";

/// What one stored VPC endpoint connection is created with, beside the connection id the
/// store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for a subnet: an
/// operation that creates an endpoint to one of the account's services fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_connection`] turns it into the
/// modelled [`VpcEndpointConnection`] the describe reports, so the id, the stored key and
/// the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct VpcEndpointConnectionSpec {
    /// The service the endpoint is connected to (`VpcEndpointConnection$ServiceId`).
    pub service_id: String,
    /// The endpoint itself, which is also the key the connection is stored under.
    pub vpc_endpoint_id: String,
    /// The account that owns the endpoint (`VpcEndpointConnection$VpcEndpointOwner`).
    pub vpc_endpoint_owner: String,
    /// The endpoint's state, as EC2 reports it — see [`VPC_ENDPOINT_STATE_VALUES`].
    pub vpc_endpoint_state: String,
    /// When the endpoint was created.
    pub creation_timestamp: SystemTime,
    /// "The Region of the endpoint" (`VpcEndpointConnection$VpcEndpointRegion`).
    pub vpc_endpoint_region: String,
    /// "The IP address type for the endpoint" (`VpcEndpointConnection$IpAddressType`), or
    /// `None` for an endpoint whose type carries none.
    pub ip_address_type: Option<String>,
    /// The DNS entries for the endpoint.
    pub dns_entries: Vec<DnsEntry>,
    /// The Network Load Balancers behind the service.
    pub network_load_balancer_arns: Vec<String>,
    /// The Gateway Load Balancers behind the service.
    pub gateway_load_balancer_arns: Vec<String>,
    /// The payer responsibility settings for the endpoint.
    pub payer_responsibilities: Vec<PayerResponsibilityEntry>,
    /// The tags on the endpoint.
    pub tags: Vec<Tag>,
}

/// How many VPC endpoint connections this emulator stores before a writer is refused.
///
/// CHOSEN, like [`MAX_STORED_DEDICATED_HOSTS`] and for the same reason: nothing in this
/// emulator reclaims a stored connection, and each one is keyed by a REQUEST-SUPPLIED
/// `VpcEndpointId`, so without a count ceiling repeated writes with fresh endpoint ids
/// would grow memory without bound. It is well above any published per-VPC VPC endpoint
/// quota a test would exercise. A count ceiling is no bound on the bytes each record
/// holds, so those are charged against [`MAX_STORED_BYTES`] too
/// ([`vpc_endpoint_connection_payload_bytes`]).
pub const MAX_STORED_VPC_ENDPOINT_CONNECTIONS: usize = 4096;
