// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-organizations-awsorganizationsv20161128 service.
//!
//! This holds the organization hierarchy the control plane persists: the singleton
//! organization, its single root, the organizational units (OUs) hung under that root
//! or under another OU, the parent -> child edges that hierarchy is read through, and
//! the accounts that are members of the organization. Every implemented handler in
//! `src/ops/` computes its response from these records; nothing is synthesised per
//! request.
//!
//! The records keep their members NON-optional, unlike the modelled shapes in
//! [`crate::ops::types`] where every output member is `Option`: a stored OU always has
//! an id, an ARN, a name and a parent, so the record says so and
//! [`OrganizationalUnitRecord::to_modelled`] is the single place a record becomes a
//! response shape.

use std::collections::BTreeMap;

use crate::ops::types::{Organization, OrganizationalUnit, PolicyTypeSummary, Root, Tag};

/// Per-service state for com-amazonaws-organizations-awsorganizationsv20161128. A handler reaches it through [`service_state`].
///
/// `organization` is the whole "is this account in an organization" flag: `None` means
/// not in use, and every operation other than `CreateOrganization` answers
/// `AWSOrganizationsNotInUseException` for it. A fresh store holds `None` however this
/// control plane was mounted — see `crate::service::EmulatedService::new`: the account is
/// in an organization only once a `CreateOrganization` request has put it in one.
#[derive(Debug, Default)]
pub struct ComAmazonawsOrganizationsAwsorganizationsv20161128State {
    /// The singleton organization `CreateOrganization` persists, or `None` while this
    /// account is in no organization.
    pub organization: Option<OrganizationRecord>,
    /// The organization's roots. `CreateOrganization` creates exactly one; Organizations
    /// has no operation that creates a second, so this never grows past one entry.
    pub roots: Vec<RootRecord>,
    /// Every stored OU, keyed by OU id.
    pub organizational_units: BTreeMap<String, OrganizationalUnitRecord>,
    /// The hierarchy edges, keyed by parent id (a root id or an OU id), each in the
    /// order the children were added. This is what a listing of a parent's children
    /// pages over and what the not-empty check on a delete reads; a child's own record
    /// carries the reverse edge. The two are only ever written together, through
    /// [`ComAmazonawsOrganizationsAwsorganizationsv20161128State::insert_organizational_unit`] /
    /// [`ComAmazonawsOrganizationsAwsorganizationsv20161128State::remove_organizational_unit`].
    pub children: BTreeMap<String, Vec<ChildRef>>,
    /// Every account that is a member of the organization, keyed by its 12-digit id.
    /// Seeded with the management account when the organization is created.
    pub accounts: BTreeMap<String, AccountRecord>,
    /// Drives [`ComAmazonawsOrganizationsAwsorganizationsv20161128State::mint_suffix`];
    /// see there for how an id suffix is derived from it.
    pub id_seed: u64,
}

/// One stored organization.
#[derive(Debug, Clone)]
pub struct OrganizationRecord {
    pub id: String,
    pub arn: String,
    pub feature_set: String,
    pub master_account_id: String,
    pub master_account_arn: String,
    pub master_account_email: String,
    pub available_policy_types: Vec<PolicyTypeRecord>,
}

impl OrganizationRecord {
    /// This record as the modelled `Organization` an operation returns.
    pub fn to_modelled(&self) -> Organization {
        Organization {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            feature_set: Some(self.feature_set.clone()),
            master_account_arn: Some(self.master_account_arn.clone()),
            master_account_id: Some(self.master_account_id.clone()),
            master_account_email: Some(self.master_account_email.clone()),
            available_policy_types: Some(
                self.available_policy_types
                    .iter()
                    .map(PolicyTypeRecord::to_modelled)
                    .collect(),
            ),
        }
    }
}

/// One stored root: the top-level parent node of the organization's hierarchy.
#[derive(Debug, Clone)]
pub struct RootRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub policy_types: Vec<PolicyTypeRecord>,
}

impl RootRecord {
    /// This record as the modelled `Root` a listing returns.
    pub fn to_modelled(&self) -> Root {
        Root {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            policy_types: Some(
                self.policy_types
                    .iter()
                    .map(PolicyTypeRecord::to_modelled)
                    .collect(),
            ),
        }
    }

    /// The root id's suffix — the id without its `r-` prefix — which is the middle
    /// segment of every OU id in this root (`ou-<root suffix>-<ou suffix>`, from
    /// `OrganizationalUnitId`'s modelled pattern `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`).
    pub fn suffix(&self) -> &str {
        self.id.strip_prefix(ROOT_ID_PREFIX).unwrap_or(&self.id)
    }
}

/// One policy type and its status in the associated root, as `PolicyTypeSummary`
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyTypeRecord {
    pub policy_type: String,
    pub status: String,
}

impl PolicyTypeRecord {
    /// This record as the modelled `PolicyTypeSummary`.
    pub fn to_modelled(&self) -> PolicyTypeSummary {
        PolicyTypeSummary {
            r#type: Some(self.policy_type.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// One stored organizational unit.
#[derive(Debug, Clone)]
pub struct OrganizationalUnitRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// The root id or OU id this OU hangs under.
    pub parent_id: String,
    /// The materialised path of this OU in the organization, as `Path`'s modelled
    /// pattern spells it: the parent's path with this OU's id appended, trailing `/`
    /// included.
    pub path: String,
    pub tags: Vec<Tag>,
}

impl OrganizationalUnitRecord {
    /// This record as the modelled `OrganizationalUnit` an operation returns.
    pub fn to_modelled(&self) -> OrganizationalUnit {
        OrganizationalUnit {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            path: Some(self.path.clone()),
        }
    }
}

/// One hierarchy edge: a child of some parent, and which kind of child it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRef {
    pub id: String,
    /// One of [`CHILD_TYPE_ORGANIZATIONAL_UNIT`] / [`CHILD_TYPE_ACCOUNT`], the two
    /// values the model's `ChildType` enum accepts.
    pub child_type: String,
}

/// One stored member account of the organization.
#[derive(Debug, Clone)]
pub struct AccountRecord {
    pub id: String,
    pub arn: String,
    pub email: String,
    pub name: String,
    pub status: String,
    pub joined_method: String,
    pub joined_timestamp: ::aws_smithy_types::DateTime,
}

/// The `ChildType` value for an OU (`ChildType` accepts `ACCOUNT` and
/// `ORGANIZATIONAL_UNIT`).
pub const CHILD_TYPE_ORGANIZATIONAL_UNIT: &str = "ORGANIZATIONAL_UNIT";

/// The `ChildType` value for a member account; see [`CHILD_TYPE_ORGANIZATIONAL_UNIT`].
pub const CHILD_TYPE_ACCOUNT: &str = "ACCOUNT";

/// The feature set `CreateOrganization` uses when the request omits `FeatureSet`: the
/// operation's own documentation — "By default (or if you set the FeatureSet parameter
/// to ALL), the new organization is created with all features enabled".
pub const DEFAULT_FEATURE_SET: &str = "ALL";

/// The `OrganizationFeatureSet` value for a consolidated-billing-only organization,
/// for which the same documentation says "no policy types are enabled by default".
pub const FEATURE_SET_CONSOLIDATED_BILLING: &str = "CONSOLIDATED_BILLING";

/// The policy type `CreateOrganization` enables in the root of an all-features
/// organization: "service control policies automatically enabled in the root"
/// (CreateOrganization's documentation).
pub const POLICY_TYPE_SERVICE_CONTROL_POLICY: &str = "SERVICE_CONTROL_POLICY";

/// The `PolicyTypeStatus` of a policy type that is enabled in a root.
pub const POLICY_TYPE_STATUS_ENABLED: &str = "ENABLED";

/// The friendly name Organizations gives the root it creates with an organization.
pub const ROOT_NAME: &str = "Root";

/// The domain of the management account's email address. The address is derived from
/// the account id rather than invented per request, so it is stable across reads and
/// satisfies `Email`'s modelled pattern `^[^\s@]+@[^\s@]+\.[^\s@]+$`. `example.com` is
/// the RFC 2606 reserved domain, and it is a crate-local constant because the engine
/// exposes no emulator-wide email or domain setting to source it from (`engine`'s
/// public surface is `state`, `listeners`, `extract_akid`, `emulator_account_id` and
/// `emulator_arn`).
const MANAGEMENT_EMAIL_DOMAIN: &str = "example.com";

/// The management account's friendly name.
const MANAGEMENT_ACCOUNT_NAME: &str = "Management account";

/// The `AccountStatus` of a member account that is in the organization.
pub const ACCOUNT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `AccountJoinedMethod` of the management account: it is the account the
/// organization was created from, so it joined by creation rather than by invitation.
pub const ACCOUNT_JOINED_METHOD_CREATED: &str = "CREATED";

/// The ARN service namespace for every ARN this service mints (OPERATIONS.md, "ARN
/// namespace"). Organizations is a global service, so its ARNs carry no region — which
/// is what every modelled ARN pattern here spells (`^arn:aws:organizations::\d{12}:…`).
pub const ARN_SERVICE: &str = "organizations";

/// The region segment of an Organizations ARN: empty, because the service is global.
pub const ARN_REGION: &str = "";

/// The `o-` prefix of an organization id (`OrganizationId`'s pattern
/// `^o-[a-z0-9]{10,32}$`).
pub const ORGANIZATION_ID_PREFIX: &str = "o-";

/// The `r-` prefix of a root id (`RootId`'s pattern `^r-[0-9a-z]{4,32}$`).
pub const ROOT_ID_PREFIX: &str = "r-";

/// The `ou-` prefix of an OU id (`OrganizationalUnitId`'s pattern
/// `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`).
pub const ORGANIZATIONAL_UNIT_ID_PREFIX: &str = "ou-";

/// The suffix length of a minted organization id. `OrganizationId`'s pattern requires
/// 10 to 32 characters after `o-`; this is the low end of that range.
const ORGANIZATION_SUFFIX_LEN: u32 = 10;

/// The suffix length of a minted root id, within `RootId`'s modelled 4..32.
const ROOT_SUFFIX_LEN: u32 = 8;

/// The length of the trailing segment of a minted OU id. `OrganizationalUnitId`'s
/// pattern requires 8 to 32 characters in that segment; this is the low end.
const ORGANIZATIONAL_UNIT_SUFFIX_LEN: u32 = 8;

// ---------------------------------------------------------------------------
// Store bounds
//
// An OU record is created by a caller and reclaimed only by an explicit delete, so
// every growth site carries a cap and nothing caller-sized reaches the store
// unchecked. The caps below are Organizations' own published quotas.
// ---------------------------------------------------------------------------

/// Organizations' published quota "Maximum number of OUs in an organization: 1,000"
/// (Quotas for Organizations, "Organizations quotas for organizational units").
pub const MAX_ORGANIZATIONAL_UNITS: usize = 1_000;

/// Organizations' published quota "OUs can be nested five levels deep beneath the
/// root", i.e. an OU's depth below the root is at most 5.
pub const MAX_ORGANIZATIONAL_UNIT_DEPTH: usize = 5;

/// Organizations' published quota of 50 tags per resource.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The largest page a paginated list operation of this service returns, and the page
/// size one that omits `MaxResults` gets: `MaxResults` is modelled `@range(min: 1, max:
/// 20)` on `ListRootsRequest$MaxResults` and
/// `ListOrganizationalUnitsForParentRequest$MaxResults`.
pub const MAX_PAGE_SIZE: i32 = 20;

/// The version tag the opaque `NextToken` carries, so a token minted by a different
/// scheme is rejected rather than misread.
const NEXT_TOKEN_VERSION: &str = "v1:";

/// The `NextToken` a page hands back for `id`: opaque to the caller, and carrying only
/// the id the NEXT page starts at.
pub fn encode_next_token(id: &str) -> String {
    hex::encode(format!("{NEXT_TOKEN_VERSION}{id}"))
}

/// The id a `NextToken` points at, or `None` when the token was not minted by
/// [`encode_next_token`].
pub fn decode_next_token(token: &str) -> Option<String> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    text.strip_prefix(NEXT_TOKEN_VERSION).map(str::to_owned)
}

/// One page of an ordered id sequence, as a `MaxResults`/`NextToken` paginated
/// collection reports it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The ids on this page, in the sequence's own order.
    pub ids: Vec<String>,
    /// The opaque cursor the NEXT page is fetched with; absent on the last page.
    pub next_token: Option<String>,
}

/// Cut the page a caller asked for out of `ordered`, a stable sequence of ids.
///
/// `next_token` is the cursor a previous page handed back, and `None` starts at the
/// first id. `max_results` defaults to [`MAX_PAGE_SIZE`].
///
/// The clamp into `1..=`[`MAX_PAGE_SIZE`] below is NOT this function's input
/// validation and must not be relied on as such: `MaxResults` carries the model's
/// `@range(min: 1, max: 20)`, so the framework's centralized request walk has already
/// refused anything outside it with a 400 before a handler runs, which makes the clamp
/// unreachable. It is here only so a value can never index past the sequence.
///
/// `None` is an unresolvable cursor: a token this service did not mint, or one naming
/// an id the sequence no longer holds. The caller must be told rather than served an
/// empty page, so the operation answers `InvalidInputException` with the reason
/// [`INVALID_NEXT_TOKEN_REASON`].
pub fn page_of(
    ordered: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let cursor = decode_next_token(token)?;
            ordered.iter().position(|id| *id == cursor)?
        }
    };
    let size = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let size = usize::try_from(size).unwrap_or(1);
    let end = start.saturating_add(size).min(ordered.len());
    Some(Page {
        ids: ordered[start..end].to_vec(),
        next_token: ordered.get(end).map(|id| encode_next_token(id)),
    })
}

/// The characters an Organizations id suffix is built from: lowercase alphanumerics,
/// which is what every id pattern in this model allows (`[a-z0-9]` / `[0-9a-z]`).
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Scrambles consecutive seeds across the suffix space. Prime, and neither 2 nor 3, so
/// it is coprime with `36^n = 2^2n * 3^2n`, which makes
/// `seed -> seed * MULTIPLIER + OFFSET (mod 36^n)` a BIJECTION: distinct seeds can
/// never mint the same suffix.
const ID_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term, so the first suffix minted is not a run of leading
/// `a`s.
const ID_OFFSET: u128 = 734_291;

/// How many times a mint retries when the id it derived is already taken, which the
/// bijection above makes unreachable in practice but which is cheap to guard. Each
/// retry widens the suffix by one character, so the highest length a retry reaches is
/// `ORGANIZATIONAL_UNIT_SUFFIX_LEN + MINT_ATTEMPTS - 1`, inside the modelled 8..32.
const MINT_ATTEMPTS: u32 = 8;

/// The longest suffix [`ComAmazonawsOrganizationsAwsorganizationsv20161128State::mint_suffix`]
/// can render: `36^24 < u128::MAX < 36^25`, so a longer one would overflow the space the
/// bijection is taken modulo.
const MAX_SUFFIX_LEN: u32 = 24;

impl ComAmazonawsOrganizationsAwsorganizationsv20161128State {
    /// Persist an organization with `feature_set`, its single root, and the emulated
    /// account as the management account.
    ///
    /// The ONE writer of the organization slot, reached from the `CreateOrganization`
    /// handler alone: membership is state that request writes, so mounting this control
    /// plane establishes no organization and no other operation builds a record this one
    /// would not.
    ///
    /// The management account is the calling account — "The account whose user is calling
    /// the CreateOrganization operation automatically becomes the management account of
    /// the new organization" — and it is seeded as a member account under the root, which
    /// is what makes it a real child edge of the hierarchy rather than an id in the
    /// organization record only.
    ///
    /// A root's enabled policy types follow from the feature set: all features enables
    /// service control policies in the root, consolidated billing enables none ("no
    /// policy types are enabled by default and you can't use organization policies").
    ///
    /// Whether an organization MAY be created is the caller's decision — an account
    /// belongs to at most one, which is `AlreadyInOrganizationException` — so this writes
    /// over whatever the slot holds.
    pub fn create_organization(&mut self, feature_set: &str) {
        let policy_types = if feature_set == FEATURE_SET_CONSOLIDATED_BILLING {
            Vec::new()
        } else {
            vec![PolicyTypeRecord {
                policy_type: POLICY_TYPE_SERVICE_CONTROL_POLICY.to_string(),
                status: POLICY_TYPE_STATUS_ENABLED.to_string(),
            }]
        };

        let organization_id = self.mint_organization_id();
        let root_id = self.mint_root_id();
        let master_account_id = ::engine::emulator_account_id().to_string();
        let master_account_email = format!("{master_account_id}@{MANAGEMENT_EMAIL_DOMAIN}");
        let master_account_arn = ::engine::emulator_arn(
            ARN_SERVICE,
            ARN_REGION,
            &format!("account/{organization_id}/{master_account_id}"),
        );

        self.organization = Some(OrganizationRecord {
            arn: ::engine::emulator_arn(
                ARN_SERVICE,
                ARN_REGION,
                &format!("organization/{organization_id}"),
            ),
            master_account_arn: master_account_arn.clone(),
            id: organization_id.clone(),
            feature_set: feature_set.to_string(),
            master_account_id: master_account_id.clone(),
            master_account_email: master_account_email.clone(),
            // Deprecated on the wire ("To determine the policies that are enabled and
            // available for use in your organization, use the ListRoots operation
            // instead"), so it reports exactly what the root reports.
            available_policy_types: policy_types.clone(),
        });
        self.roots = vec![RootRecord {
            arn: ::engine::emulator_arn(
                ARN_SERVICE,
                ARN_REGION,
                &format!("root/{organization_id}/{root_id}"),
            ),
            id: root_id.clone(),
            name: ROOT_NAME.to_string(),
            policy_types,
        }];
        self.children.insert(
            root_id,
            vec![ChildRef {
                id: master_account_id.clone(),
                child_type: CHILD_TYPE_ACCOUNT.to_string(),
            }],
        );
        self.accounts.insert(
            master_account_id.clone(),
            AccountRecord {
                arn: master_account_arn,
                id: master_account_id,
                email: master_account_email,
                name: MANAGEMENT_ACCOUNT_NAME.to_string(),
                status: ACCOUNT_STATUS_ACTIVE.to_string(),
                joined_method: ACCOUNT_JOINED_METHOD_CREATED.to_string(),
                joined_timestamp: ::aws_smithy_types::DateTime::from(
                    ::std::time::SystemTime::now(),
                ),
            },
        );
    }

    /// Mint a fresh `len`-character id suffix of lowercase alphanumerics.
    ///
    /// The suffix is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so within one `len` it is unpredictable-looking yet never
    /// repeats (the bijection is modulo `36^len`, so the no-repeat argument is
    /// per-length; the three id namespaces this service mints into — `o-`, `r-`, `ou-`
    /// — are disjoint, and `mint_organizational_unit_id` additionally checks the store).
    ///
    /// `len` is clamped to [`MAX_SUFFIX_LEN`], past which `36^len` overflows a `u128`.
    pub fn mint_suffix(&mut self, len: u32) -> String {
        let len = len.min(MAX_SUFFIX_LEN);
        self.id_seed = self.id_seed.wrapping_add(1);
        let space = 36u128.pow(len);
        let mut scrambled = (u128::from(self.id_seed) * ID_MULTIPLIER + ID_OFFSET) % space;
        let mut suffix = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let digit = usize::try_from(scrambled % 36).unwrap_or(0);
            suffix.push(ID_ALPHABET[digit]);
            scrambled /= 36;
        }
        String::from_utf8(suffix).unwrap_or_default()
    }

    /// Mint an organization id: `o-` plus [`ORGANIZATION_SUFFIX_LEN`] characters.
    pub fn mint_organization_id(&mut self) -> String {
        let suffix = self.mint_suffix(ORGANIZATION_SUFFIX_LEN);
        format!("{ORGANIZATION_ID_PREFIX}{suffix}")
    }

    /// Mint a root id: `r-` plus [`ROOT_SUFFIX_LEN`] characters.
    pub fn mint_root_id(&mut self) -> String {
        let suffix = self.mint_suffix(ROOT_SUFFIX_LEN);
        format!("{ROOT_ID_PREFIX}{suffix}")
    }

    /// Mint an OU id in `root`: `ou-<root suffix>-<fresh suffix>`, the shape
    /// `OrganizationalUnitId`'s pattern requires.
    ///
    /// `None` when every attempt landed on an id that is already stored, which the
    /// bijection on [`ID_MULTIPLIER`] makes unreachable in practice. The caller must
    /// then refuse the request rather than mint an unchecked id: inserting over an
    /// existing OU would replace that record while adding a second child edge, leaving
    /// an orphaned edge under its old parent.
    pub fn mint_organizational_unit_id(&mut self, root_suffix: &str) -> Option<String> {
        for attempt in 0..MINT_ATTEMPTS {
            // Each retry draws from a LARGER space than the one that collided, staying
            // inside `OrganizationalUnitId`'s modelled 8..32 characters.
            let len = ORGANIZATIONAL_UNIT_SUFFIX_LEN.saturating_add(attempt);
            let suffix = self.mint_suffix(len);
            let id = format!("{ORGANIZATIONAL_UNIT_ID_PREFIX}{root_suffix}-{suffix}");
            if !self.organizational_units.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// The organization's root, or `None` while no organization exists.
    pub fn root(&self) -> Option<&RootRecord> {
        self.roots.first()
    }

    /// Whether `parent_id` names a node that can hold children: this organization's
    /// root, or one of its OUs.
    pub fn parent_exists(&self, parent_id: &str) -> bool {
        self.roots.iter().any(|root| root.id == parent_id)
            || self.organizational_units.contains_key(parent_id)
    }

    /// The ids of `parent_id`'s child OUs, in the order they were created — the stable
    /// sequence [`page_of`] cuts a page out of.
    pub fn child_organizational_unit_ids(&self, parent_id: &str) -> Vec<String> {
        self.children
            .get(parent_id)
            .map(|children| {
                children
                    .iter()
                    .filter(|child| child.child_type == CHILD_TYPE_ORGANIZATIONAL_UNIT)
                    .map(|child| child.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `parent_id` has any child at all — an OU or an account. This is the
    /// not-empty check a delete runs.
    pub fn has_children(&self, parent_id: &str) -> bool {
        self.children
            .get(parent_id)
            .is_some_and(|children| !children.is_empty())
    }

    /// How many OU levels below the root `parent_id` sits: `0` for the root itself,
    /// `1` for an OU directly under it, and so on. `None` when `parent_id` names no
    /// node of this organization.
    pub fn depth_of(&self, parent_id: &str) -> Option<usize> {
        if self.roots.iter().any(|root| root.id == parent_id) {
            return Some(0);
        }
        let mut depth = 0usize;
        let mut cursor = self.organizational_units.get(parent_id)?;
        loop {
            depth += 1;
            // The chain is bounded by the OU count, which `MAX_ORGANIZATIONAL_UNITS`
            // caps, so this terminates even if a record ever named a parent that is
            // neither the root nor a stored OU.
            if depth > MAX_ORGANIZATIONAL_UNITS {
                return None;
            }
            match self.organizational_units.get(&cursor.parent_id) {
                Some(parent) => cursor = parent,
                None => return Some(depth),
            }
        }
    }

    /// The `Path` a new OU with id `ou_id` under `parent_id` gets: the parent's path
    /// with the OU's id appended. `None` when `parent_id` names no node.
    pub fn path_under(&self, parent_id: &str, ou_id: &str) -> Option<String> {
        let organization = self.organization.as_ref()?;
        if self.roots.iter().any(|root| root.id == parent_id) {
            return Some(format!("{}/{parent_id}/{ou_id}/", organization.id));
        }
        let parent = self.organizational_units.get(parent_id)?;
        Some(format!("{}{ou_id}/", parent.path))
    }

    /// Store `record` and add the matching edge under its parent, so the two can never
    /// drift apart.
    pub fn insert_organizational_unit(&mut self, record: OrganizationalUnitRecord) {
        self.children
            .entry(record.parent_id.clone())
            .or_default()
            .push(ChildRef {
                id: record.id.clone(),
                child_type: CHILD_TYPE_ORGANIZATIONAL_UNIT.to_string(),
            });
        self.organizational_units.insert(record.id.clone(), record);
    }

    /// Remove the OU `id` and its edge under its parent, returning the record that was
    /// stored. The counterpart of [`Self::insert_organizational_unit`].
    pub fn remove_organizational_unit(&mut self, id: &str) -> Option<OrganizationalUnitRecord> {
        let record = self.organizational_units.remove(id)?;
        if let Some(siblings) = self.children.get_mut(&record.parent_id) {
            siblings.retain(|child| child.id != record.id);
            if siblings.is_empty() {
                self.children.remove(&record.parent_id);
            }
        }
        self.children.remove(&record.id);
        Some(record)
    }
}

/// The `AWSOrganizationsNotInUseException` message every operation uses for a request
/// made while this account is in no organization.
///
/// The sentence the error itself states, word for word
/// (`smithy_query(query="doc",shape="AWSOrganizationsNotInUseException")`): entirely
/// fixed text, carrying nothing of the request.
pub const NOT_IN_USE_MESSAGE: &str = "Your account isn't a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization.";

/// The `AlreadyInOrganizationException` message `CreateOrganization` uses for a second
/// create.
pub const ALREADY_IN_ORGANIZATION_MESSAGE: &str =
    "This account is already a member of an organization. An account can belong to only one organization at a time.";

/// The `ParentNotFoundException` message for a `ParentId` that names no root or OU.
///
/// The sentence `ParentNotFoundException` itself states in this service's model, word for
/// word: a `ParentId` names a root or an OU — that is all its modelled pattern
/// `^(r-[0-9a-z]{4,32})|(ou-[0-9a-z]{4,32}-[a-z0-9]{8,32})$` accepts — so the two node
/// kinds the message names are the two kinds the member can name.
pub const PARENT_NOT_FOUND_MESSAGE: &str =
    "We can't find a root or OU with the ParentId that you specified.";

/// The `OrganizationalUnitNotFoundException` message for an id that names no OU.
pub const ORGANIZATIONAL_UNIT_NOT_FOUND_MESSAGE: &str =
    "We can't find an OU with the OrganizationalUnitId that you specified.";

/// The `OrganizationalUnitNotEmptyException` message for a delete of an OU that still
/// holds children.
pub const ORGANIZATIONAL_UNIT_NOT_EMPTY_MESSAGE: &str =
    "The specified OU is not empty. Move all accounts to another root or to other OUs, remove all child OUs, and try the operation again.";

/// The `DuplicateOrganizationalUnitException` message for a second OU with the same
/// name under one parent.
pub const DUPLICATE_ORGANIZATIONAL_UNIT_MESSAGE: &str =
    "An OU with the same name already exists.";

/// The `InvalidInputExceptionReason` for a `NextToken` this service did not mint, or
/// one naming an id the sequence no longer holds.
pub const INVALID_NEXT_TOKEN_REASON: &str = "INVALID_NEXT_TOKEN";

/// The `InvalidInputException` message that goes with [`INVALID_NEXT_TOKEN_REASON`].
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "You provided a value for the NextToken parameter that is not valid.";

/// The `InvalidInputExceptionReason` for a `Tags` list carrying one key twice.
pub const DUPLICATE_TAG_KEY_REASON: &str = "DUPLICATE_TAG_KEY";

/// The `ConstraintViolationExceptionReason` for a create that would push the
/// organization past [`MAX_ORGANIZATIONAL_UNITS`].
pub const OU_NUMBER_LIMIT_EXCEEDED_REASON: &str = "OU_NUMBER_LIMIT_EXCEEDED";

/// The `ConstraintViolationExceptionReason` for a create nested deeper than
/// [`MAX_ORGANIZATIONAL_UNIT_DEPTH`].
pub const OU_DEPTH_LIMIT_EXCEEDED_REASON: &str = "OU_DEPTH_LIMIT_EXCEEDED";

/// The `ConstraintViolationExceptionReason` for more than [`MAX_TAGS_PER_RESOURCE`]
/// tags on one resource.
pub const MAX_TAG_LIMIT_EXCEEDED_REASON: &str = "MAX_TAG_LIMIT_EXCEEDED";

/// Refuse a `Tags` list that is too long or that carries a key twice, with the message
/// and the `InvalidInputExceptionReason`/`ConstraintViolationExceptionReason` the
/// caller has to fix the request from.
///
/// `Err` is `(reason, message)`; the reason tells the handler which modelled error to
/// answer with, because the two failures are classified differently by the service:
/// a duplicate key is an invalid input, too many tags is a quota violation.
pub fn bound_tags(tags: &[Tag]) -> Result<(), (&'static str, String)> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err((
            MAX_TAG_LIMIT_EXCEEDED_REASON,
            format!(
                "Tags may carry at most {MAX_TAGS_PER_RESOURCE} entries, got {}",
                tags.len()
            ),
        ));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(tags.len());
    for tag in tags {
        if seen.contains(&tag.key.as_str()) {
            return Err((
                DUPLICATE_TAG_KEY_REASON,
                format!(
                    "Tag keys must be unique among the tags attached to the same entity: {}",
                    tag.key
                ),
            ));
        }
        seen.push(tag.key.as_str());
    }
    Ok(())
}

/// The `ConcurrentModificationException` message for a create whose id could not be
/// minted because every candidate was already stored.
pub const CONCURRENT_MODIFICATION_MESSAGE: &str =
    "The target of the operation is currently being modified by a different request. Try again later.";

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the com-amazonaws-organizations-awsorganizationsv20161128 emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOrganizationsAwsorganizationsv20161128State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOrganizationsAwsorganizationsv20161128State>)
}
