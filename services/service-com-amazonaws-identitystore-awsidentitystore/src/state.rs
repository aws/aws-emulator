// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-identitystore-awsidentitystore service.
//!
//! Besides the state struct itself this module holds the helpers the identity-store
//! handlers share — identifier resolution (every id member also accepts the
//! resource's ARN), ARN and id minting, the records the resources persist as, the
//! record-to-modelled-shape conversions, the list cursor codec and the modelled
//! failures — so the operation files agree on one implementation of each instead of
//! carrying a copy apiece that can drift. How a request the boundary validator refused
//! is WORDED lives in `crate::service::validation`, beside the seam that answers it.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{
    Address, ConflictException, Email, ExternalId, GroupMembership, InternalServerException,
    MemberId, Name, PhoneNumber, Photo, ResourceNotFoundException, Role,
    ServiceQuotaExceededException, ValidationException,
};

/// Which identity stores this account HAS, asked of the service that owns IAM Identity
/// Center instances. A submodule because it reaches ANOTHER service's state, which is a
/// concern of its own.
mod identity_center;
pub use identity_center::*;

/// The identity store this emulator ADVERTISES through IAM Identity Center: the store of
/// the instance `sso-admin` seeds.
///
/// This API models no operation that creates an identity store — in AWS one comes with an
/// IAM Identity Center instance — so WHICH stores exist is Identity Center's answer, not
/// this service's ([`addressed_store_key`]). This constant is not a gate: it is the one
/// store that has a record before anything is written, because another service points
/// clients at it.
///
/// The VALUE is the identity store of the instance `sso-admin` seeds, which its
/// `InstanceMetadata$IdentityStoreId` reports to a client
/// (`service_com_amazonaws_ssoadmin_swbexternalservice::state::SEED_IDENTITY_STORE_ID`,
/// whose doc points back here). The two services hold separate state, so the id that
/// joins them has to stay equal: a client reads it out of `ListInstances` there and then
/// addresses this service with it. The form is `d-` + 10 hexadecimal digits, which both
/// that member (`^[a-zA-Z0-9-]*$`, 1..64) and this service's own `IdentityStoreId` pattern
/// accept, and which keeps the store's ARN inside its modelled 62..93 length.
pub const SEED_IDENTITY_STORE_ID: &str = "d-9067f3a1c2";

/// The `UserStatus` of a user created through this API: the modelled enum is
/// `ENABLED | DISABLED` (`smithy_query(query="shape",shape="UserStatus")`) and
/// `CreateUser` models no `UserStatus` member, so a user this API creates is usable —
/// `ENABLED` — from the moment it exists. An `UpdateUser` naming the `userStatus`
/// attribute moves it from there.
pub const USER_STATUS_ENABLED: &str = "ENABLED";

/// The number of users one identity store will hold.
///
/// `CreateUser` models `ServiceQuotaExceededException` (HTTP 402) for "the request
/// would cause the number of users or groups in the identity store to exceed the
/// maximum allowed", which is exactly this bound. AWS's own figure is far larger than
/// anything an in-memory emulator should hold, so the number is CHOSEN; capacity is
/// released by `DeleteUser` (see [`IdentityStoreData::remove_user`]).
pub const MAX_USERS_PER_STORE: usize = 500;

/// The number of groups one identity store will hold. Chosen for the same reason, and
/// answered through the same modelled error, as [`MAX_USERS_PER_STORE`]; capacity is
/// released by `DeleteGroup` once that operation is implemented.
pub const MAX_GROUPS_PER_STORE: usize = 500;

/// The number of group memberships one identity store will hold. CHOSEN for the same
/// reason as [`MAX_USERS_PER_STORE`]; capacity is released by `DeleteGroupMembership`.
pub const MAX_MEMBERSHIPS_PER_STORE: usize = 2_000;

/// The number of members one group will hold.
///
/// Besides bounding memory this bounds WORK under the single service-wide state mutex:
/// a group's membership ids live in an insertion-ordered `Vec` that
/// [`IdentityStoreData::membership_of`] and [`IdentityStoreData::remove_membership`]
/// scan linearly, so without a cap one caller's group size would set every other
/// caller's latency. CHOSEN, and answered through the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_MEMBERSHIPS_PER_GROUP: usize = 200;

/// The number of identity stores this emulator will hold a record of.
///
/// Every operation is addressed by a CALLER-CHOSEN `IdentityStoreId` and this API creates
/// no identity store, so the set of stores is not something the emulator decides — but
/// the RECORDS it keeps are resident state, and the modelled pattern admits on the order
/// of 10^12 distinct ids, so the number of records is capped here rather than following
/// the number of distinct ids a caller sends. CHOSEN, like the per-store caps, and far
/// above any real deployment: AWS provisions one identity store per IAM Identity Center
/// instance. A create that would record one more past this bound is refused with the
/// `ServiceQuotaExceededException` those operations model (HTTP 402); capacity is
/// released when a store's record is dropped.
pub const MAX_IDENTITY_STORES: usize = 64;

/// The page size a list operation uses when the caller sends no `MaxResults`: the
/// model's own declared default, shared by every `MaxResults` member of this service
/// (`@default: 100`, range 1..100 — `smithy_query(query="operation",operation="ListUsers")`,
/// `smithy_query(query="operation",operation="ListGroups")`,
/// `smithy_query(query="operation",operation="ListGroupMemberships")`).
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The largest page a list operation will answer with: the upper bound of the modelled
/// `MaxResults` range (1..100).
pub const MAX_MAX_RESULTS: i32 = 100;

/// The only attribute path `ListGroups`'s (deprecated) `Filters` member searches.
///
/// `Filter$AttributePath` is "the attribute path that is used to specify which
/// attribute name to search … `DisplayName` is a valid attribute path for the
/// `ListGroups` API" (`smithy_query(query="doc",shape="Filter",member="AttributePath")`),
/// so a filter naming any other attribute is a syntax error in the request.
pub const GROUP_FILTER_ATTRIBUTE_PATH: &str = "DisplayName";

/// The only attribute path `ListUsers`'s (deprecated) `Filters` member searches.
///
/// `Filter$AttributePath` is "the attribute path that is used to specify which
/// attribute name to search … `UserName` is a valid attribute path for the `ListUsers`
/// API" (`smithy_query(query="doc",shape="Filter",member="AttributePath")`), so a
/// filter naming any other attribute is a syntax error in the request.
pub const USER_FILTER_ATTRIBUTE_PATH: &str = "UserName";

/// The `arnNamespace` / `endpointPrefix` of this service, from the model's
/// `aws.api#service` trait (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "identitystore";

/// Which identity-store resource an id or ARN names. The modelled `ResourceArn`
/// pattern admits exactly these three resource kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    User,
    Group,
    Membership,
}

impl ResourceKind {
    /// The resource segment this kind carries in an ARN.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::Membership => "membership",
        }
    }

    /// The `ResourceType` enum value naming this kind in a
    /// `ResourceNotFoundException` (`RESOURCE_TYPE_VALUES`).
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Group => "GROUP",
            Self::Membership => "GROUP_MEMBERSHIP",
        }
    }

    /// The message a `ResourceNotFoundException` for this resource class carries.
    ///
    /// A constant per class, with nothing of the request in it: the service states
    /// WHICH CLASS of resource it could not find, so the message is the same for every
    /// id of that class and discloses no id that was probed. All three are recorded:
    /// `Group not found`, `User not found`, and for a membership the sentence that names
    /// the identity store ("pool") and the request member,
    /// [`MEMBERSHIP_NOT_FOUND_MESSAGE`].
    ///
    /// An operation that RESOLVES an identifier rather than reading a resource by one
    /// (`GetGroupId`, `GetUserId`, `GetGroupMembershipId`) states its own constant for
    /// the same class — the `ResourceType` token followed by ` not found.` — through
    /// [`not_found_by_alternate_identifier`], and the member end of a membership states
    /// its own through [`member_not_found`]; neither widens this one.
    pub fn not_found_message(self) -> &'static str {
        match self {
            Self::User => "User not found",
            Self::Group => "Group not found",
            Self::Membership => MEMBERSHIP_NOT_FOUND_MESSAGE,
        }
    }
}

/// The ARN of an identity-store resource: `arn:aws:identitystore:::<kind>/<id>`.
///
/// Deliberately NOT `engine::emulator_arn`: the modelled `ResourceArn` pattern
/// (`^arn:aws[a-z-]*:identitystore:::(user|group|membership)/…$`, from
/// `smithy_query(query="shape",shape="ResourceArn")`) leaves BOTH the region and the
/// account field empty, while `emulator_arn` always writes the emulator account id
/// into the fifth field. An identity STORE's ARN does carry the account, so that one
/// is built with the shared helper — see [`identity_store_arn`].
pub fn resource_arn(kind: ResourceKind, id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:::{}/{id}", kind.as_str())
}

/// The ARN of an identity store: `arn:aws:identitystore::<account>:identitystore/<id>`,
/// the ARN form the modelled `IdentityStoreId` pattern admits. The account is the
/// emulator's canonical one and the region field is empty, so this is
/// `engine::emulator_arn` with an empty region.
pub fn identity_store_arn(identity_store_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        "",
        &format!("identitystore/{identity_store_id}"),
    )
}

/// The store key an `IdentityStoreId` member names, or `None` when the identifier can
/// name no store this emulator serves.
///
/// The modelled pattern admits both the bare id (`d-0123456789`) and the store's ARN
/// (`arn:aws:identitystore::<account>:identitystore/d-0123456789`), and both name the
/// same store, so the ARN form is reduced to the id it ends with. Any well-formed id is a
/// key: the store a caller names is the store it has, and nothing in the model ties
/// existence to a particular value.
///
/// An ARN also carries the OWNING ACCOUNT, and the modelled pattern admits any 12-digit
/// one: an ARN naming another account names that account's identity store, which is not
/// one this emulator serves however the id ends, so there is no key to look up and the
/// caller is refused with the modelled not-found.
pub fn identity_store_key(identifier: &str) -> Option<String> {
    if identifier.starts_with("arn:") && !names_this_account(identifier) {
        return None;
    }
    Some(bare_id(identifier))
}

/// Whether an ARN's account field (the fifth `:`-separated field) is the emulator's own
/// account.
fn names_this_account(arn: &str) -> bool {
    arn.split(':')
        .nth(4)
        .is_some_and(|account| account == ::engine::emulator_account_id())
}

/// The resource key a `ResourceId` member (`UserId`, `GroupId`, `MembershipId`,
/// `MemberId.UserId`) names.
///
/// The modelled pattern admits both the bare id and the resource's ARN, and both name
/// the same resource, so the ARN form is reduced to the id it ends with. The id is a
/// hexadecimal UUID whose pattern accepts either letter case (`[A-Fa-f0-9]`), so it is
/// folded to lower case: `A1B2…` and `a1b2…` are the same resource.
pub fn resource_key(identifier: &str) -> String {
    bare_id(identifier)
}

/// The last `/`-separated segment of an ARN, or the whole value when it is not one,
/// folded to lower case.
fn bare_id(identifier: &str) -> String {
    let bare = if identifier.starts_with("arn:") {
        identifier.rsplit('/').next().unwrap_or(identifier)
    } else {
        identifier
    };
    bare.to_ascii_lowercase()
}

/// One stored user: every member `CreateUser` was given, plus the id, ARN and revision
/// the service minted for it, so a later `DescribeUser` reports exactly what was
/// stored instead of echoing a request or synthesising a profile.
#[derive(Debug, Clone)]
pub struct User {
    pub user_id: String,
    pub user_arn: String,
    pub revision: u64,
    pub user_name: Option<String>,
    pub name: Option<Name>,
    pub display_name: Option<String>,
    pub nick_name: Option<String>,
    pub profile_url: Option<String>,
    pub emails: Vec<Email>,
    pub addresses: Vec<Address>,
    pub phone_numbers: Vec<PhoneNumber>,
    pub user_type: Option<String>,
    pub title: Option<String>,
    pub preferred_language: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    /// The user's `UserStatus`. Every stored user has one — a created user is
    /// [`USER_STATUS_ENABLED`] — so a describe reports it rather than omitting a
    /// modelled member a caller gates provisioning on; an `UpdateUser` naming the
    /// `userStatus` attribute changes it.
    pub user_status: String,
    pub photos: Vec<Photo>,
    pub website: Option<String>,
    pub birthdate: Option<String>,
    pub roles: Vec<Role>,
    /// External ids are set by an identity provider's provisioning, which no modelled
    /// operation of this slice performs, so this stays empty until one does.
    pub external_ids: Vec<ExternalId>,
    pub extensions: HashMap<String, ::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The principal that created this user and the one that LAST WROTE it, as
    /// [`caller_principal`] derives them from the CALLER's own credentials.
    ///
    /// `updated_by` reports the last write, which a create is: a record nobody has
    /// changed since it was created reports the creating principal there, exactly as its
    /// `updated_at` reports its `created_at`. An update moves it to the updating
    /// principal.
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl User {
    /// The stored revision as the modelled `ResourceRevision` (pattern `^[0-9]+$`).
    pub fn revision(&self) -> String {
        self.revision.to_string()
    }

    /// Record that this user was modified at `at`: the next revision, and the new
    /// `UpdatedAt`.
    ///
    /// `User$Revision` is "the current revision of the user in the identity store. This
    /// value changes each time the user is modified", so every successful update moves
    /// it on.
    pub fn record_update(&mut self, at: SystemTime) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at = at;
    }

    /// The value of the user attribute `attribute_path` names, for a `ListUsers`
    /// filter. `None` when the user does not carry that attribute.
    ///
    /// Matched case-insensitively for the same reason as [`Group::attribute`]: SCIM
    /// attribute names are case-insensitive (RFC 7643 §2.1).
    pub fn attribute(&self, attribute_path: &str) -> Option<&str> {
        if attribute_path.eq_ignore_ascii_case(USER_FILTER_ATTRIBUTE_PATH) {
            return self.user_name.as_deref();
        }
        None
    }

    /// This record as the modelled `User` a list reports.
    ///
    /// Every optional member is reported only when the record holds one, so a user
    /// stored without a profile member comes back without it rather than with an
    /// invented or empty value. `requested_extensions` is why `Extensions` travels in:
    /// the member is "a map of explicitly requested attribute extensions", so only the
    /// names the request asked for are reported.
    pub fn to_user(
        &self,
        identity_store_id: &str,
        requested_extensions: Option<&[String]>,
    ) -> crate::ops::types::User {
        crate::ops::types::User {
            identity_store_id: identity_store_id.to_string(),
            user_id: self.user_id.clone(),
            user_arn: self.user_arn.clone(),
            revision: self.revision(),
            user_name: self.user_name.clone(),
            external_ids: modeled_list(&self.external_ids),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            nick_name: self.nick_name.clone(),
            profile_url: self.profile_url.clone(),
            emails: modeled_list(&self.emails),
            addresses: modeled_list(&self.addresses),
            phone_numbers: modeled_list(&self.phone_numbers),
            user_type: self.user_type.clone(),
            title: self.title.clone(),
            preferred_language: self.preferred_language.clone(),
            locale: self.locale.clone(),
            timezone: self.timezone.clone(),
            user_status: Some(self.user_status.clone()),
            photos: modeled_list(&self.photos),
            website: self.website.clone(),
            birthdate: self.birthdate.clone(),
            roles: modeled_list(&self.roles),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            created_by: self.created_by.clone(),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            updated_by: self.updated_by.clone(),
            extensions: self.requested_extensions(requested_extensions),
        }
    }

    /// The extensions a `DescribeUser` naming `requested` must report: the stored
    /// entries under exactly those names.
    ///
    /// `DescribeUserRequest$Extensions` is "a collection of extension names indicating
    /// what extensions the service should retrieve alongside other user attributes",
    /// so a request that names none asks for none.
    pub fn requested_extensions(
        &self,
        requested: Option<&[String]>,
    ) -> Option<HashMap<String, ::aws_smithy_types::Document>> {
        let names = requested?;
        let selected: HashMap<String, ::aws_smithy_types::Document> = names
            .iter()
            .filter_map(|name| {
                self.extensions
                    .get(name)
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect();
        (!selected.is_empty()).then_some(selected)
    }
}

/// One stored group: what `CreateGroup` was given plus the id, ARN and revision the
/// service minted for it.
#[derive(Debug, Clone)]
pub struct Group {
    pub group_id: String,
    pub group_arn: String,
    pub revision: u64,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// Empty for the same reason as [`User::external_ids`].
    pub external_ids: Vec<ExternalId>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The creating / last-writing principal, exactly as on [`User`].
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl Group {
    /// The stored revision as the modelled `ResourceRevision` (pattern `^[0-9]+$`).
    pub fn revision(&self) -> String {
        self.revision.to_string()
    }

    /// Record that this group was modified at `at`: the next revision, and the new
    /// `UpdatedAt` — the same bump [`User::record_update`] makes, so both resources
    /// move their revision through one implementation.
    ///
    /// `Group$Revision` is "the current revision of the group in the identity store.
    /// This value changes each time the group is modified", so every successful update
    /// moves it on.
    pub fn record_update(&mut self, at: SystemTime) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at = at;
    }

    /// This record as the modelled `Group` a list reports.
    ///
    /// Every optional member is reported only when the record holds one, so a group
    /// stored without a `Description` (or a `DisplayName`, or an external id) comes
    /// back without that member rather than with an invented or empty value.
    pub fn to_group(&self, identity_store_id: &str) -> crate::ops::types::Group {
        crate::ops::types::Group {
            identity_store_id: identity_store_id.to_string(),
            group_id: self.group_id.clone(),
            group_arn: self.group_arn.clone(),
            revision: self.revision(),
            display_name: self.display_name.clone(),
            external_ids: modeled_list(&self.external_ids),
            description: self.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            created_by: self.created_by.clone(),
            updated_by: self.updated_by.clone(),
        }
    }

    /// The value of the group attribute `attribute_path` names, for a `ListGroups`
    /// filter. `None` when the group does not carry that attribute.
    ///
    /// The path is matched case-insensitively: SCIM attribute names are
    /// case-insensitive (RFC 7643 §2.1), which is also how `GetGroupId` reads its
    /// `UniqueAttribute` path.
    pub fn attribute(&self, attribute_path: &str) -> Option<&str> {
        if attribute_path.eq_ignore_ascii_case(GROUP_FILTER_ATTRIBUTE_PATH) {
            return self.display_name.as_deref();
        }
        None
    }
}

/// One stored group membership.
///
/// The member is held as the user id alone because the modelled `MemberId` union
/// declares exactly one variant, `UserId`
/// (`smithy_query(query="shape",shape="MemberId")`), so that id IS the whole member
/// identity; [`GroupMembershipRecord::modeled_member_id`] puts it back in the union.
#[derive(Debug, Clone)]
pub struct GroupMembershipRecord {
    pub membership_id: String,
    pub membership_arn: String,
    pub group_id: String,
    pub member_user_id: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The creating / last-writing principal, exactly as on [`User`]. A membership has no
    /// modelled update operation, so its `updated_by` stays the principal that created
    /// it, as its `updated_at` stays its `created_at`.
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl GroupMembershipRecord {
    /// The stored member as the modelled union.
    pub fn modeled_member_id(&self) -> MemberId {
        MemberId::UserId(self.member_user_id.clone())
    }

    /// This record as the modelled `GroupMembership` a list or describe reports.
    pub fn to_group_membership(&self, identity_store_id: &str) -> GroupMembership {
        GroupMembership {
            identity_store_id: identity_store_id.to_string(),
            membership_id: self.membership_id.clone(),
            membership_arn: self.membership_arn.clone(),
            group_id: Some(self.group_id.clone()),
            member_id: Some(self.modeled_member_id()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            created_by: self.created_by.clone(),
            updated_by: self.updated_by.clone(),
        }
    }
}

/// One identity store: its identity plus the users, groups and memberships it holds.
#[derive(Debug, Default)]
pub struct IdentityStoreData {
    pub identity_store_id: String,
    pub identity_store_arn: String,
    /// Users by `UserId`.
    pub users: HashMap<String, User>,
    /// `UserName` -> `UserId`. Backs `CreateUser`'s uniqueness claim and, in a later
    /// slice, `GetUserId` by `AlternateIdentifier`.
    pub user_name_index: HashMap<String, String>,
    /// Groups by `GroupId`.
    pub groups: HashMap<String, Group>,
    /// `DisplayName` -> `GroupId`, backing `CreateGroup`'s uniqueness claim.
    pub group_display_name_index: HashMap<String, String>,
    /// Memberships by `MembershipId`.
    pub memberships: HashMap<String, GroupMembershipRecord>,
    /// `GroupId` -> its membership ids in creation order, so a list's order — and
    /// therefore its pagination — is deterministic.
    pub memberships_by_group: HashMap<String, Vec<String>>,
    /// `UserId` -> the membership ids naming that user, ready for
    /// `ListGroupMembershipsForMember` / `IsMemberInGroups`.
    pub memberships_by_member: HashMap<String, Vec<String>>,
    /// Bumped per minted id, so two ids minted in the same clock tick still differ.
    id_counter: u64,
}

impl IdentityStoreData {
    /// An empty store with `identity_store_id`'s identity.
    fn new(identity_store_id: String) -> Self {
        Self {
            identity_store_arn: identity_store_arn(&identity_store_id),
            identity_store_id,
            ..Self::default()
        }
    }

    /// A fresh `[0-9a-f]{8}-[0-9a-f]{4}-…` identifier no resource in this store holds.
    ///
    /// The three resource kinds share one id space because they share one ARN space:
    /// the modelled `ResourceId` admits any of them, so an id handed out for a user is
    /// never also handed out for a group or a membership.
    pub fn mint_resource_id(&mut self) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let candidate = uuid_from_seed(self.id_counter, now_nanos());
            if !self.holds_id(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: these are
        // well-formed UUIDs too, and there are always more of them than this store
        // holds resources.
        (0u64..=self.resource_count() as u64)
            .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
            .find(|candidate| !self.holds_id(candidate))
            .unwrap_or_else(|| format!("00000000-0000-4000-8000-{:012x}", u64::MAX))
    }

    /// Whether any resource of this store already carries `id`.
    fn holds_id(&self, id: &str) -> bool {
        self.users.contains_key(id)
            || self.groups.contains_key(id)
            || self.memberships.contains_key(id)
    }

    /// How many resources this store holds, across the three kinds.
    fn resource_count(&self) -> usize {
        self.users.len() + self.groups.len() + self.memberships.len()
    }

    /// Whether another user fits inside [`MAX_USERS_PER_STORE`].
    pub fn has_user_capacity(&self) -> bool {
        self.users.len() < MAX_USERS_PER_STORE
    }

    /// Whether another group fits inside [`MAX_GROUPS_PER_STORE`].
    pub fn has_group_capacity(&self) -> bool {
        self.groups.len() < MAX_GROUPS_PER_STORE
    }

    /// Whether another membership of `group_id` fits inside both
    /// [`MAX_MEMBERSHIPS_PER_STORE`] and [`MAX_MEMBERSHIPS_PER_GROUP`].
    pub fn has_membership_capacity(&self, group_id: &str) -> bool {
        self.memberships.len() < MAX_MEMBERSHIPS_PER_STORE
            && self.members_of(group_id) < MAX_MEMBERSHIPS_PER_GROUP
    }

    /// How many members `group_id` has.
    pub fn members_of(&self, group_id: &str) -> usize {
        self.memberships_by_group
            .get(group_id)
            .map_or(0, Vec::len)
    }

    /// The group whose `DisplayName` is `display_name`, or `None` when this store
    /// holds no such group.
    ///
    /// Resolved through [`Self::group_display_name_index`] — the same index
    /// `CreateGroup` enforces its uniqueness claim with — so a name resolves to the
    /// one group that currently holds it and never to a stale one.
    pub fn group_by_display_name(&self, display_name: &str) -> Option<&Group> {
        let group_id = self.group_display_name_index.get(display_name)?;
        self.groups.get(group_id)
    }

    /// The group carrying the external id `issuer`/`id`, or `None` when this store
    /// holds no such group.
    ///
    /// An external id pair identifies at most one resource, so the scan's answer does
    /// not depend on the order the groups are walked in. No modelled operation of this
    /// slice writes a group's external ids (they come from an identity provider's
    /// provisioning — see [`Group::external_ids`]), so today this resolves nothing;
    /// it is the lookup, not a hard-coded refusal, so it starts answering the moment
    /// a provisioning path stores one.
    pub fn group_by_external_id(&self, issuer: &str, id: &str) -> Option<&Group> {
        self.groups.values().find(|group| {
            group
                .external_ids
                .iter()
                .any(|external| external.issuer == issuer && external.id == id)
        })
    }

    /// The membership of `group_id` by `member_user_id`, or `None` when that user is
    /// not a member of that group.
    pub fn membership_of(&self, group_id: &str, member_user_id: &str) -> Option<&GroupMembershipRecord> {
        self.memberships_by_group
            .get(group_id)?
            .iter()
            .filter_map(|id| self.memberships.get(id))
            .find(|membership| membership.member_user_id == member_user_id)
    }

    /// Store `membership` and index it by group and by member, in creation order.
    pub fn insert_membership(&mut self, membership: GroupMembershipRecord) {
        self.memberships_by_group
            .entry(membership.group_id.clone())
            .or_default()
            .push(membership.membership_id.clone());
        self.memberships_by_member
            .entry(membership.member_user_id.clone())
            .or_default()
            .push(membership.membership_id.clone());
        self.memberships
            .insert(membership.membership_id.clone(), membership);
    }

    /// Remove the membership `membership_id` names, with both of its indexes.
    /// `None` when the store holds no such membership.
    pub fn remove_membership(&mut self, membership_id: &str) -> Option<GroupMembershipRecord> {
        let membership = self.memberships.remove(membership_id)?;
        if let Some(ids) = self.memberships_by_group.get_mut(&membership.group_id) {
            ids.retain(|id| id != membership_id);
            if ids.is_empty() {
                self.memberships_by_group.remove(&membership.group_id);
            }
        }
        if let Some(ids) = self.memberships_by_member.get_mut(&membership.member_user_id) {
            ids.retain(|id| id != membership_id);
            if ids.is_empty() {
                self.memberships_by_member.remove(&membership.member_user_id);
            }
        }
        Some(membership)
    }

    /// Remove the group `group_id` names, releasing its `DisplayName` uniqueness
    /// claim and every membership it holds. `None` when the store holds no such
    /// group.
    ///
    /// The memberships go with the group because a `GroupMembership` is an edge
    /// between a group and a member: once the group is gone there is nothing for the
    /// edge to point at, so a membership naming it is a state no sequence of creates
    /// could produce (`CreateGroupMembership` requires both ends to exist) and the
    /// group index would keep the deleted id alive in `ListGroupMemberships`.
    pub fn remove_group(&mut self, group_id: &str) -> Option<Group> {
        let group = self.groups.remove(group_id)?;
        // The index IS the store's uniqueness claim on the name, so dropping the
        // holder frees the name for a later create. Guarded on the holder's id in case
        // the entry has since been re-pointed at another group.
        let claimed_name = group.display_name.clone().filter(|display_name| {
            self.group_display_name_index
                .get(display_name)
                .is_some_and(|holder| holder == group_id)
        });
        if let Some(display_name) = claimed_name {
            self.group_display_name_index.remove(&display_name);
        }
        // Taking the group's membership list out first means `remove_membership` finds
        // no `memberships_by_group` entry to prune — harmless re-entry, the same way
        // `remove_user` relies on for `memberships_by_member` — and it still clears
        // each membership from `memberships` and from the member index.
        let membership_ids = self
            .memberships_by_group
            .remove(group_id)
            .unwrap_or_default();
        for membership_id in &membership_ids {
            self.remove_membership(membership_id);
        }
        Some(group)
    }

    /// Remove the user `user_id` names, with the `UserName` claim it held and every
    /// membership that names it as a member. `None` when this store holds no such
    /// user, so the caller can report it as not found rather than answer a success
    /// that removed nothing.
    ///
    /// The memberships go with the user because `CreateGroupMembership` requires BOTH
    /// ends to exist in the identity store, so a membership naming a user the store no
    /// longer holds is a state no sequence of creates could produce — and the member
    /// indexes would keep the deleted id alive in `ListGroupMemberships`.
    pub fn remove_user(&mut self, user_id: &str) -> Option<User> {
        let user = self.users.remove(user_id)?;
        // The index IS the store's uniqueness claim on the name, so dropping the
        // holder frees the name for a later create. Guarded on the holder's id in case
        // the entry has since been re-pointed at another user.
        let claimed_name = user
            .user_name
            .clone()
            .filter(|user_name| {
                self.user_name_index
                    .get(user_name)
                    .is_some_and(|holder| holder == user_id)
            });
        if let Some(user_name) = claimed_name {
            self.user_name_index.remove(&user_name);
        }
        let membership_ids = self
            .memberships_by_member
            .remove(user_id)
            .unwrap_or_default();
        for membership_id in &membership_ids {
            self.remove_membership(membership_id);
        }
        Some(user)
    }

    /// This store's users in a deterministic order: when each was created, ties broken
    /// by `UserId`.
    ///
    /// The order is DERIVED from the records rather than held as a second index, so it
    /// cannot drift from `users`, and it is total — two users minted in the same clock
    /// tick still have different ids — which is what makes a paginated walk of the list
    /// stable across calls.
    pub fn users_in_order(&self) -> Vec<&User> {
        let mut users: Vec<&User> = self.users.values().collect();
        users.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.user_id.cmp(&right.user_id))
        });
        users
    }

    /// This store's groups in a stable listing order.
    ///
    /// Creation time first, then `GroupId` for two groups created inside one clock
    /// tick, so a listing — and therefore its pagination — is deterministic and a
    /// group keeps its place in the sequence across calls.
    pub fn groups_in_order(&self) -> Vec<&Group> {
        let mut groups: Vec<&Group> = self.groups.values().collect();
        groups.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.group_id.cmp(&right.group_id))
        });
        groups
    }

    /// The memberships of `group_id` in creation order.
    pub fn memberships_in_order(&self, group_id: &str) -> Vec<&GroupMembershipRecord> {
        self.memberships_by_group
            .get(group_id)
            .map(|ids| ids.iter().filter_map(|id| self.memberships.get(id)).collect())
            .unwrap_or_default()
    }

    /// The `GroupId` of every group this store records `member_id` as a member of, each
    /// keyed the way this store keys a resource id ([`resource_key`]), so `member_id` is
    /// accepted in either of the forms that name the same user.
    ///
    /// The member side of a membership as a SET of groups, for a reader asking which groups
    /// a user belongs to rather than for the membership records themselves
    /// (`ListGroupMembershipsForMember` is that reader's paged form, and
    /// `IsMemberInGroups` the one-group question): the two membership indexes this walks
    /// are this store's own, so a reader outside it does not depend on their layout.
    pub fn groups_of_member(&self, member_id: &str) -> BTreeSet<String> {
        self.memberships_by_member
            .get(&resource_key(member_id))
            .into_iter()
            .flatten()
            .filter_map(|membership_id| self.memberships.get(membership_id))
            .map(|membership| resource_key(&membership.group_id))
            .collect()
    }
}

/// Per-service state for com-amazonaws-identitystore-awsidentitystore. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsIdentitystoreAwsidentitystoreState {
    /// The identity stores this emulator holds a RECORD of, keyed by the store key
    /// [`identity_store_key`] derives from an `IdentityStoreId`.
    ///
    /// An entry is a store something has written to, not the set of stores that exist:
    /// WHICH identity stores exist is IAM Identity Center's answer ([`addressed_store_key`]),
    /// and an entry here appears the first time a resource is CREATED in one
    /// ([`ComAmazonawsIdentitystoreAwsidentitystoreState::register_store_mut`]).
    /// A store with no entry holds nothing, which is what a read of it reports.
    ///
    /// The map is BOUNDED on both axes: an entry is added only by
    /// [`ComAmazonawsIdentitystoreAwsidentitystoreState::register_store_mut`] (a create,
    /// never a read, an update or a delete) and never past [`MAX_IDENTITY_STORES`], while
    /// what each entry can hold is bounded by [`MAX_USERS_PER_STORE`],
    /// [`MAX_GROUPS_PER_STORE`] and [`MAX_MEMBERSHIPS_PER_STORE`] — so total resident
    /// state is the product of two finite numbers.
    pub stores: HashMap<String, IdentityStoreData>,
}

impl Default for ComAmazonawsIdentitystoreAwsidentitystoreState {
    /// Pre-registers the identity store `sso-admin`'s `ListInstances` points a client at
    /// ([`SEED_IDENTITY_STORE_ID`]), so that store is one this emulator has a record of
    /// from the first call of a process rather than from its first write.
    ///
    /// The map is keyed by `IdentityStoreId`, so everything a store holds is already
    /// scoped to it: a second store — seeded here or named by a caller — is one more
    /// entry and nothing else changes.
    fn default() -> Self {
        let mut stores = HashMap::new();
        stores.insert(
            SEED_IDENTITY_STORE_ID.to_string(),
            IdentityStoreData::new(SEED_IDENTITY_STORE_ID.to_string()),
        );
        Self { stores }
    }
}

/// The identity store one request resolved to, as a read sees it.
///
/// `Held` is the record the state keeps. `Unwritten` is the EMPTY store of an
/// `IdentityStoreId` this emulator holds no record of yet: it is built per call and
/// dropped with the request, so resolving a store on a read registers nothing and
/// cannot change what a later caller sees. Both forms carry the store's own identity,
/// so a handler reports the store it was addressed to either way.
#[derive(Debug)]
pub enum ResolvedStore<'a> {
    /// The store this state holds a record of.
    Held(&'a IdentityStoreData),
    /// A store nothing has written to yet, which therefore holds no resource. BOXED
    /// because a store record is far larger than a reference, and this variant is the
    /// rare one: the common path answers from the record the state already holds.
    Unwritten(Box<IdentityStoreData>),
}

impl ::std::ops::Deref for ResolvedStore<'_> {
    type Target = IdentityStoreData;

    fn deref(&self) -> &IdentityStoreData {
        match self {
            Self::Held(store) => store,
            Self::Unwritten(store) => store,
        }
    }
}

impl ComAmazonawsIdentitystoreAwsidentitystoreState {
    /// The store `store_key` names, for a REPORTING operation.
    ///
    /// `store_key` is the key [`addressed_store_key`] resolved the request's
    /// `IdentityStoreId` to, so whether the store EXISTS has already been decided, once,
    /// outside this struct — a read cannot gate differently from a write.
    ///
    /// A store with no record yet resolves to the empty [`ResolvedStore::Unwritten`], so
    /// a read of it reports the nothing it holds — a listing with no entries, the
    /// not-found of the RESOURCE a describe names — and writes nothing.
    pub fn resolve_store(&self, store_key: &str) -> ResolvedStore<'_> {
        match self.stores.get(store_key) {
            Some(store) => ResolvedStore::Held(store),
            None => ResolvedStore::Unwritten(Box::new(IdentityStoreData::new(
                store_key.to_string(),
            ))),
        }
    }

    /// [`Self::resolve_store`] for a write that CHANGES WHAT IS ALREADY THERE: the
    /// store's record, or `None` when this emulator holds none for it.
    ///
    /// An update or a delete only ever acts on a resource that exists, and a store with
    /// no record holds none, so such a write needs no record and REGISTERS none: it
    /// answers for the resource it named (its not-found, or the empty success an
    /// `@idempotent` delete owes for a resource that is already gone). That keeps a
    /// request which stores nothing from leaving a permanent entry behind, so the size of
    /// [`Self::stores`] follows the stores resources were actually CREATED in.
    pub fn resolve_recorded_store_mut(
        &mut self,
        store_key: &str,
    ) -> Option<&mut IdentityStoreData> {
        self.stores.get_mut(store_key)
    }

    /// [`Self::resolve_store`] for a write that STORES A NEW RESOURCE: the store's
    /// record, registered on first use.
    ///
    /// A create has to persist into the store it names, so the record is materialised
    /// here rather than left to a later call — registering it is how the first
    /// `CreateUser`/`CreateGroup` addressed to a store this account has becomes readable.
    /// This is the ONLY path that adds an entry, which is what bounds the map: an entry
    /// appears only where a resource was created, and never past
    /// [`MAX_IDENTITY_STORES`] — reached through the `ServiceQuotaExceededException`
    /// every operation that calls this models.
    ///
    /// `store_key` has already been resolved by [`addressed_store_key`], so a store this
    /// account does not have never reaches this: a create cannot conjure one.
    pub fn register_store_mut(
        &mut self,
        store_key: &str,
    ) -> Result<&mut IdentityStoreData, ServiceQuotaExceededException> {
        if !self.stores.contains_key(store_key) && self.stores.len() >= MAX_IDENTITY_STORES {
            return Err(quota_exceeded(format!(
                "this emulator already records {} identity stores",
                self.stores.len()
            )));
        }
        Ok(self
            .stores
            .entry(store_key.to_string())
            .or_insert_with(|| IdentityStoreData::new(store_key.to_string())))
    }

    /// The RECORD this emulator holds for the store `identifier` names, or `None` when it
    /// holds none (which is not the same as the store not existing — see
    /// [`Self::resolve_store`]).
    pub fn store(&self, identifier: &str) -> Option<&IdentityStoreData> {
        self.stores.get(&identity_store_key(identifier)?)
    }

    /// The store keys this emulator holds a RECORD for.
    ///
    /// What a reader of the identity stores themselves falls back to while IAM Identity
    /// Center knows nothing about the account's enablement
    /// ([`account_identity_store_keys`]): in that window the stores the emulator can
    /// report are the ones it HOLDS — the store another service advertises
    /// ([`SEED_IDENTITY_STORE_ID`]) plus every store a create has registered — rather than
    /// the 10^12 ids the modelled pattern admits.
    ///
    /// A READ: it reports the keys the map holds at this instant and registers none.
    pub fn recorded_store_keys(&self) -> Vec<String> {
        self.stores.keys().cloned().collect()
    }
}

/// `store_keys` as the stable order a paged read of identity stores walks them in, with
/// each store named once.
///
/// The order is the key's own, ascending: an identity store carries no creation time in
/// this emulator's state (this API creates none — see [`SEED_IDENTITY_STORE_ID`]), and the
/// keys come out of a hash map on either side of
/// [`ComAmazonawsIdentitystoreAwsidentitystoreState::recorded_store_keys`] /
/// [`account_identity_store_keys`], so the identifier is what makes the sequence total and
/// therefore a paginated walk of it repeatable. Two Identity Center instances cannot share
/// an identity store, but the sequence is de-duplicated regardless so one store is one
/// entry of one page.
pub fn stores_in_listing_order(mut store_keys: Vec<String>) -> Vec<String> {
    store_keys.sort();
    store_keys.dedup();
    store_keys
}

/// The modelled form of a stored list: `None` when it is empty.
///
/// Every one of the user's lists is `length 1..` in the model, so a user carrying none
/// of them reports an absent member rather than an empty list the model would refuse.
pub fn modeled_list<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// The store key the request's `IdentityStoreId` names, or the modelled
/// `ResourceNotFoundException` when it names no identity store this account HAS.
///
/// Resolution happens ONCE, here, before a handler touches this service's state, so a
/// read cannot gate differently from a write and a create cannot register a store that
/// does not exist. Two things make an identifier name no store:
///
/// * its FORM puts the store outside this account — the ARN of a store another account
///   owns ([`identity_store_key`]);
/// * IAM Identity Center ANSWERS that the account holds no such identity store
///   ([`identity_store_is_absent`]). An identity store is not caller-creatable through
///   this API: it is the identity store OF an Identity Center instance, so the stores the
///   account has are the ones that service reports — and while it has decided nothing, the
///   store a client names is the one its own environment gave it and is served.
///
/// The key is the bare id both the id and ARN forms of one store reduce to, so everything
/// downstream addresses one store by one key.
///
/// LOCK ORDER: this takes the Identity Center state's lock and releases it before
/// returning, so a handler calls it BEFORE locking this service's own state and the two
/// locks are never held together.
pub fn addressed_store_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
) -> Result<String, ResourceNotFoundException> {
    let key = identity_store_key(identifier).ok_or_else(|| identity_store_not_found(identifier))?;
    if identity_store_is_absent(engine, &key) {
        return Err(identity_store_not_found(identifier));
    }
    Ok(key)
}

/// The fixed part of the message [`identity_store_not_found`] carries, so a handler and a
/// test name one text.
pub const IDENTITY_STORE_NOT_FOUND_MESSAGE_PREFIX: &str =
    "IdentityStore not present for IdentityStoreId: ";

/// The message a `ResourceNotFoundException` for an identity store carries: the recorded
/// fixed text followed by the `IdentityStoreId` the request sent, verbatim.
///
/// Unlike the per-class constants a RESOURCE not-found states
/// ([`ResourceKind::not_found_message`]), this class's recorded message names the value
/// the caller addressed — which is the caller's own input, not a store this emulator
/// holds — so it is taken from the request rather than from state.
pub fn identity_store_not_found_message(identifier: &str) -> String {
    format!("{IDENTITY_STORE_NOT_FOUND_MESSAGE_PREFIX}{identifier}")
}

/// The `ResourceNotFoundException` for an identifier that names no identity store this
/// account has (see [`addressed_store_key`]). `IDENTITY_STORE` is the `ResourceType`
/// value for this kind (`RESOURCE_TYPE_VALUES`).
///
/// `ResourceId` is left unset: the modelled member is a `ResourceId` (pattern
/// `^(arn:…)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-…$`, from
/// `smithy_query(query="shape",shape="ResourceId")`), which an `IdentityStoreId` is not —
/// and a not-found refusal of this service names the resource CLASS rather than echoing
/// the identifier that was probed.
pub fn identity_store_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some("IDENTITY_STORE".to_string()),
        resource_id: None,
        reason: None,
        message: Some(identity_store_not_found_message(identifier)),
        request_id: Some(new_request_id()),
    }
}

/// The `ResourceNotFoundException` for a resource this store does not hold, named by the
/// operation's own resource id.
///
/// The MESSAGE is the fixed one its resource class carries
/// ([`ResourceKind::not_found_message`]) rather than a sentence built from the
/// lookup: the service's message is a constant per class, so it neither varies per
/// request nor tells the caller which id was probed.
///
/// `ResourceId` is left unset for the same reason the message carries no id: a recorded
/// not-found of this class reports the `ResourceType` and the message and nothing else,
/// so the probed identifier is not echoed back in a typed member — the way
/// [`not_found_by_alternate_identifier`] already answers for a resource named by another
/// identifier.
pub fn not_found(kind: ResourceKind) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(kind.resource_type().to_string()),
        resource_id: None,
        reason: None,
        message: Some(kind.not_found_message().to_string()),
        request_id: Some(new_request_id()),
    }
}

/// The `ResourceNotFoundException` for a `MemberId.UserId` that names a user this
/// store does not hold.
///
/// A resource the request names through a member OTHER than the operation's own
/// resource id gets its own constant, naming that member: a `CreateGroupMembership`
/// whose `MemberId` matches nobody answers `User with provided member ID not found`,
/// not the bare `User not found` a `DescribeUser` answers, so a caller can tell which
/// end of the membership was missing without parsing an id out of the text.
pub fn member_not_found(member_user_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(ResourceKind::User.resource_type().to_string()),
        resource_id: Some(member_user_id.to_string()),
        reason: None,
        message: Some("User with provided member ID not found".to_string()),
        request_id: Some(new_request_id()),
    }
}

/// The `ResourceNotFoundException` for a group membership the named identity store does
/// not hold.
///
/// The membership class's own fixed message, which [`not_found`] already supplies
/// through [`ResourceKind::not_found_message`]; this stays as the named entry point its
/// handlers call.
pub fn membership_not_found() -> ResourceNotFoundException {
    not_found(ResourceKind::Membership)
}

/// AWS's fixed `ResourceNotFoundException` message for a membership id the identity
/// store does not hold.
///
/// Recorded behaviour, repeated across every recorded rejection of that class: the
/// sentence names the identity store ("pool") and the request member rather than the id
/// that was probed.
pub const MEMBERSHIP_NOT_FOUND_MESSAGE: &str =
    "Group membership not found for given pool and membershipId.";

/// The `ResourceNotFoundException` for a resource an ALTERNATE identifier names and
/// this store does not hold.
///
/// An operation that RESOLVES an identifier (`GetGroupId`, `GetUserId`,
/// `GetGroupMembershipId`) states its own constant for the resource class: the modelled
/// `ResourceType` token followed by ` not found.` — `GROUP not found.` and `USER not
/// found.` are the recorded ones — rather than the sentence a read BY resource id
/// answers ([`ResourceKind::not_found_message`]). Nothing of the request reaches it: not
/// the identifier that failed to resolve and not its value (a `DisplayName` is
/// `@sensitive`), so the message is the same for every identifier of that class.
///
/// `ResourceId` is left unset rather than carrying what the caller sent: the modelled
/// member is a `ResourceId` (pattern
/// `^(arn:…)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-…$`, from
/// `smithy_query(query="shape",shape="ResourceId")`), which an external id, a unique
/// attribute value or a (group, member) pair is not.
pub fn not_found_by_alternate_identifier(kind: ResourceKind) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(kind.resource_type().to_string()),
        resource_id: None,
        reason: None,
        message: Some(format!("{} not found.", kind.resource_type())),
        request_id: Some(new_request_id()),
    }
}

/// Which uniqueness claim a create would violate.
///
/// One variant per claim this service makes: a `UserName` and a group `DisplayName`
/// are unique within an identity store, and a (group, member) pair has at most one
/// membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniquenessClaim {
    /// One group per `DisplayName` in an identity store.
    GroupDisplayName,
    /// One user per `UserName` in an identity store.
    UserName,
    /// One membership per (group, member) pair.
    GroupMembership,
}

impl UniquenessClaim {
    /// The message a `ConflictException` for this claim carries.
    ///
    /// A constant naming the claim that was violated — not a sentence built from the
    /// request or from the record that already holds the value: the caller branches on
    /// WHICH claim collided, and the ids of a resource it did not ask about are not
    /// part of that answer.
    pub fn message(self) -> &'static str {
        match self {
            Self::GroupDisplayName => "Duplicate GroupDisplayName",
            Self::UserName => "Duplicate UserName",
            Self::GroupMembership => "Member and Group relationship already exists",
        }
    }
}

/// The `ConflictException` for a request that would violate a uniqueness claim —
/// a second user with one `UserName`, a second group with one `DisplayName`, or a
/// membership that already exists.
///
/// `reason` is the modelled `ConflictExceptionReason` value for exactly that case
/// (`CONFLICT_EXCEPTION_REASON_VALUES`).
pub fn uniqueness_conflict(claim: UniquenessClaim) -> ConflictException {
    ConflictException {
        message: Some(claim.message().to_string()),
        request_id: Some(new_request_id()),
        reason: Some("UNIQUENESS_CONSTRAINT_VIOLATION".to_string()),
    }
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would
/// pass one of this emulator's capacity bounds: `subject` says which bound and what is
/// already held.
pub fn quota_exceeded(subject: String) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "the request would exceed the maximum this emulator holds: {subject}"
        )),
        request_id: Some(new_request_id()),
    }
}

/// Whether a caller's `Revision` precondition names the revision a stored resource is
/// currently at.
///
/// `ResourceRevision` is a decimal string (len 1..64, pattern `^[0-9]+$`,
/// `smithy_query(query="shape",shape="ResourceRevision")`), so the member names the
/// NUMBER it spells and not one spelling of it: `1` and `01` name one revision, and a
/// value no revision of this store can reach never matches. One implementation for
/// every operation that takes the precondition — the updates and the deletes, of users
/// and of groups — so one value held against one resource gets one answer whichever
/// operation is given it.
pub fn revision_precondition_holds(expected: &str, current: u64) -> bool {
    expected.parse::<u64>().ok() == Some(current)
}

/// The modelled `ConflictException` for an update whose `Revision` precondition does
/// not hold.
///
/// `Revision` is documented as "the expected current revision … the update is applied
/// only if it matches the current revision … If the value doesn't match, the operation
/// fails with a `ConflictException`" (`UpdateUser$Revision`,
/// `smithy_query(query="doc",shape="UpdateUserRequest",member="Revision")`), and
/// `CONCURRENT_MODIFICATION` is the modelled `ConflictExceptionReason` for that case.
pub fn revision_conflict(expected: &str, stored: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "the request expected revision {expected} but the resource is at revision {stored}"
        )),
        request_id: Some(new_request_id()),
        reason: Some("CONCURRENT_MODIFICATION".to_string()),
    }
}

/// The modelled `ValidationException` for a request this service cannot accept, carrying
/// `message` as its reason in words.
///
/// The one constructor every handler calls, so a refusal a HANDLER raises carries the
/// same exception members as one the boundary validator raises: `reason` stays absent
/// (every modelled `ValidationExceptionReason` value is about a customer-managed KMS key,
/// `smithy_query(query="shape",shape="ValidationExceptionReason")`, and the member is "not
/// included" for anything else) and `RequestId` is this call's
/// ([`new_request_id`]).
pub fn validation(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
        request_id: Some(new_request_id()),
        reason: None,
    }
}

/// The names this API reserves: no user and no group may be named one of them.
///
/// Stated by the model on each of the two members that NAME an identity —
/// `CreateUserRequest$UserName` and `CreateGroupRequest$DisplayName`
/// (`smithy_query(query="doc",shape="CreateGroupRequest",member="DisplayName")`): both say
/// `Administrator` and `AWSAdministrators` are reserved names and can't be used for users
/// or groups. One catalog rather than one per handler, so a create and an update refuse
/// the same set.
pub const RESERVED_IDENTITY_NAMES: [&str; 2] = ["Administrator", "AWSAdministrators"];

/// Whether `name` is one of the [`RESERVED_IDENTITY_NAMES`].
///
/// Compared without regard to letter case: what is reserved is the NAME, and these are
/// SCIM identity names, which are not case-exact — a store that admitted another spelling
/// of one would hold an identity named a reserved name.
pub fn is_reserved_identity_name(name: &str) -> bool {
    RESERVED_IDENTITY_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

/// The message a request naming a user or a group a reserved name is refused with.
///
/// The rule as the model words it, built from the catalog above so the sentence cannot
/// name a set the service does not refuse. It states the RULE and echoes nothing a caller
/// sent: the two members it guards are `@sensitive`.
pub fn reserved_identity_name_message() -> String {
    format!(
        "{} are reserved names and can't be used for users or groups",
        RESERVED_IDENTITY_NAMES.join(" and ")
    )
}

/// The modelled `InternalServerException` a handler answers when it cannot reach this
/// service's store at all.
///
/// One constructor rather than the same literal per operation file: the store is reached
/// identically by every handler, so there is one sentence for failing to reach it, and
/// `RequestId` is stamped here like on every other exception this service returns.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the identity store is unavailable".to_string()),
        request_id: Some(new_request_id()),
        ..Default::default()
    }
}

/// The counter behind [`new_request_id`]: process-wide, so two requests served
/// concurrently never seed the same value.
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh `RequestId` for an exception body.
///
/// Every exception this service models carries a `RequestId` member documented as the
/// globally unique id the identity store generates FOR EACH REQUEST and returns inside
/// the exception when that request fails
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so one is minted per
/// failure rather than left absent. The modelled shape is a 36-character UUID (pattern
/// `^[A-Fa-f0-9]{8}-…-[A-Fa-f0-9]{12}$`), which is what [`uuid_from_seed`] produces.
///
/// The VALUE identifies one call and nothing else: it is not derived from the request, it
/// is never stored, and it is not reported twice. It needs no service state — the seed is
/// this process-wide counter plus the clock — so an exception raised before the store is
/// reached carries one too.
pub fn new_request_id() -> String {
    uuid_from_seed(
        REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        now_nanos(),
    )
}

/// The principal that made this call, as the `CreatedBy` / `UpdatedBy` of what it
/// writes.
///
/// `User$CreatedBy` and `Group$CreatedBy` are "the IDENTIFIER of the user or system that
/// created the user/group" (`smithy_query(query="operation",operation="DescribeUser")`,
/// `smithy_query(query="operation",operation="DescribeGroup")`), and what IDENTIFIES a
/// principal on AWS is its unique id — the value `sts:GetCallerIdentity` answers as
/// `UserId` and a policy reads as `${aws:userid}`: `AIDA…` for an IAM user, and
/// `AROA…:<session name>` for a role session. An ARN ADDRESSES a principal; the id names
/// it, and it is what an audit member carries.
///
/// The id is ASKED OF the identity service rather than spelled here: the session the
/// request's access key id names, through STS's own resolution of a request's caller
/// ([`sts_state::resolve_request_caller`]), falling back to the identity a LONG-TERM key
/// speaks for — the IAM user that holds it ([`sts_state::long_term_identity`]), or the
/// principal of the key this emulator publishes when no record holds it. So a client that
/// reads its own identity and then reads who created a resource it just created sees one
/// principal, two callers that are two IAM identities are recorded as two principals, and
/// nothing about the value comes from the credential's key TEXT, which no identity
/// service resolves to a principal.
///
/// An engine with no STS mounted can hold no session at all, and a request whose
/// credential has expired resolves to no session either; both answer the long-term
/// identity, which is the resolution STS itself reports for a key that names no session.
/// An EXPIRED credential never reaches a handler in the first place — the emulator
/// verifies a request's credential before any operation runs — so the expired arm is the
/// same "this key names no session here" case rather than a principal this service
/// decides to substitute.
pub fn caller_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let long_term = long_term_caller_identity(engine, &access_key_id);
    let Some(sessions) = sts_state::service_state(engine) else {
        return long_term.user_id;
    };
    let resolved = sts_state::resolve_request_caller(
        &mut sessions.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term.clone(),
    )
    .map_or(long_term, |caller| caller.identity);
    resolved.user_id
}

/// The identity a LONG-TERM access key speaks for, as STS resolves it against the
/// account's IAM records ([`sts_state::long_term_identity`]): the user that holds the key,
/// and the principal of the key this emulator publishes for a key no record holds.
///
/// STS's resolution is CALLED rather than reproduced, so the principal this service
/// records for a credential and the one `sts:GetCallerIdentity` reports for it are one
/// value.
fn long_term_caller_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
) -> sts_state::Identity {
    let iam = sts_state::iam_state(engine);
    let guard = iam.as_ref().map(|iam| iam.lock());
    sts_state::long_term_identity(guard.as_deref(), access_key_id)
}

// ---------------------------------------------------------------------------
// `AttributeOperation` support, shared by `UpdateUser` and `UpdateGroup`.
//
// Both operations take `Operations: [{AttributePath, AttributeValue}]` and apply them
// to one stored resource, so the refusal AWS builds from a path and the `AttributeValue`
// text reader live here once; which paths each resource ACCEPTS, and how it names its
// own attributes, is that operation's own table.
// ---------------------------------------------------------------------------

/// The message AWS answers an `AttributePath` an update cannot change with: an
/// identifier, a service-generated value, or an attribute the resource does not have.
///
/// Built from the requested path, which is the rule AWS states for it — so this is the
/// ONE place the text lives, for both update operations.
pub fn unsupported_attribute_path_message(path: &str) -> String {
    format!("Updates for AttributePath: {path} is not supported")
}

/// How the VALUE an `AttributeOperation` carries is read and constraint-checked: a
/// submodule because the modelled constraints of the member being written are a concern of
/// their own, shared by both update operations.
mod attribute_text;
pub use attribute_text::*;

/// The cursor for a page that starts at the resource `resource_id` names.
///
/// Hex of the id rather than the id itself, so a caller cannot read structure into the
/// token and has nothing to build one from: a `NextToken` is opaque and only ever
/// travels back unchanged. Hex also satisfies the modelled `NextToken` pattern
/// `^[-a-zA-Z0-9+=/:_]*$`.
pub fn encode_cursor(resource_id: &str) -> String {
    ::hex::encode(resource_id.as_bytes())
}

/// The resource id a cursor minted by [`encode_cursor`] carries, or `None` when the
/// token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The paged WALK that codec serves, shared by every list operation of this service: a
/// submodule because bounding a page, resuming one at a cursor and minting the next are
/// one concern of their own, and the same one for every listing.
mod pagination;
pub use pagination::{page_of, Page};

/// Now, in nanoseconds since the Unix epoch — only an id-minting seed.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// One RFC 9562 version-4-shaped UUID from `counter` and `nanos`.
///
/// The bytes come from SHA-256 of the two seeds (the `sha2` crate), so consecutive
/// counters produce unrelated ids, and the version and variant bits are then set so
/// the value is a well-formed random UUID — which is the form the modelled
/// `ResourceId` pattern demands.
fn uuid_from_seed(counter: u64, nanos: u128) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// The wording of a refused request lives in `crate::service::validation`, beside the
// `override_refusal` seam that answers one, rather than here: it renders the
// `ValidationException` message from the violations the boundary validator found, which
// is not state at all.

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIdentitystoreAwsidentitystoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIdentitystoreAwsidentitystoreState>)
}
