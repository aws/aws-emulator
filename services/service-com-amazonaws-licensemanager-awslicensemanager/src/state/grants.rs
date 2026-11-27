// SPDX-License-Identifier: CC0-1.0

//! The grant slice of this service's state: the resource `CreateGrant` mints and
//! `GetGrant` reports.
//!
//! A **grant** shares the entitlements of one stored license with one principal. It
//! is stored exactly once, keyed by its ARN, beside the `ClientToken` the request
//! that minted it carried, so a repeat of that request reports the same grant
//! instead of minting a second one — the same shape the license and the license
//! configuration are held in.
//!
//! A grant is created for a license this store HOLDS: a grant is the shared use of
//! that license's entitlements, so there is nothing to share when no license is
//! there, and the stored grant can never name a license a read cannot resolve.
//!
//! The status a create answers with is not a transition the clock owes: a grant
//! waits for the grantee's own `AcceptGrant` or `RejectGrant` call, so it stays in
//! the status it was created in until a caller moves it. Only the `Options` a create
//! may carry decide that status, as the model documents them.

use crate::ops::types::{
    Grant, Options, Tag, ACTIVATION_OVERRIDE_BEHAVIOR_VALUES, ALLOWED_OPERATION_VALUES,
    GRANT_TYPE_VALUES,
};

use super::{
    fresh_id, region_rejection, tag_list_rejection, text_rejection,
    ComAmazonawsLicensemanagerAwslicensemanagerState, ARN_SERVICE,
};

/// The resource portion a grant ARN carries before its id, so a generated ARN reads
/// `arn:aws:license-manager:<home region>:<account>:grant:g-<32 hex>`.
pub const GRANT_ARN_RESOURCE_PREFIX: &str = "grant:";

/// The prefix of a grant id.
pub const GRANT_ID_PREFIX: &str = "g-";

/// How many grants this emulator holds at once, matching the bound on the other
/// resources this store keeps: every map in it is finite, so no sequence of accepted
/// requests can grow one without end.
pub const MAX_GRANTS: usize = 500;

/// The version a create mints: a grant's first version, before any
/// `CreateGrantVersion` adds another. Spelled as the decimal version the license
/// slice also mints, so a caller reads one version grammar from this service.
pub const FIRST_GRANT_VERSION: &str = "1";

/// The status a grant carries from the moment `CreateGrant` mints it: the grantee
/// has not accepted it yet, so it is not usable until an `AcceptGrant` call moves
/// it. One of the modelled `GrantStatus` values
/// (`smithy_query(query="shape",shape="GrantStatus")`).
pub const GRANT_STATUS_PENDING_ACCEPT: &str = "PENDING_ACCEPT";

/// The status of a grant that is usable at once: the only value `Options.DesiredStatus`
/// admits, which the model documents as creating and activating the grant in one
/// request.
pub const GRANT_STATUS_ACTIVE: &str = "ACTIVE";

/// How this service refuses an activation the grant's principal does not admit.
///
/// `Options.DesiredStatus` is documented as specifiable "only when the grant
/// principal is an organization or an organizational unit (OU)"
/// (`smithy_query(query="doc",shape="Options",member="DesiredStatus")`), so a grant
/// to a single account cannot ask to be created already active.
pub const ORGANIZATION_PRINCIPAL_REQUIRED_MESSAGE: &str =
    "Options.DesiredStatus can be specified only when the grant principal is an \
     organization or an organizational unit.";

/// The ARN namespace an organization and an organizational unit principal are named
/// in: the service portion of `arn:aws:organizations::<account>:organization/o-…`.
const ORGANIZATIONS_ARN_SERVICE: &str = "organizations";

/// The resource portions of the two principal forms that are an organization or an
/// organizational unit, as AWS Organizations names them.
const ORGANIZATION_RESOURCE_PREFIXES: &[&str] = &["organization/", "ou/"];

/// One grant, as this service stores it.
///
/// Every member a `CreateGrant` request carries is held here, plus the identity the
/// service mints for it, so `GetGrant` answers from the record rather than from the
/// request that made it.
#[derive(Debug, Clone)]
pub struct StoredGrant {
    /// The minted `g-<32 hex>` id, the last segment of [`Self::arn`].
    pub id: String,
    /// The grant's ARN: the key this record is stored under.
    pub arn: String,
    pub grant_name: String,
    /// The resource this grant was created from. A grant created on a license has
    /// that license as its parent; a version or a distributed grant names another.
    pub parent_arn: String,
    pub license_arn: String,
    /// The one principal the grant shares the license with: the single entry of the
    /// request's `Principals`, which the model bounds at exactly one.
    pub grantee_principal_arn: String,
    pub home_region: String,
    /// One of the modelled `GrantStatus` values.
    pub status: String,
    pub status_reason: Option<String>,
    pub version: String,
    /// The operations the grant allows, as the request's `AllowedOperations` named
    /// them.
    pub granted_operations: Vec<String>,
    pub options: Option<Options>,
    /// One of the modelled `GrantType` values, when the request named one.
    pub grant_type: Option<String>,
    /// The `ClientToken` of the request that minted this grant, so a repeat of it
    /// mints nothing.
    pub client_token: String,
}

impl StoredGrant {
    /// This grant as the modelled read shape, member for member.
    pub fn wire(&self) -> Grant {
        Grant {
            grant_arn: self.arn.clone(),
            grant_name: self.grant_name.clone(),
            parent_arn: self.parent_arn.clone(),
            license_arn: self.license_arn.clone(),
            grantee_principal_arn: self.grantee_principal_arn.clone(),
            home_region: self.home_region.clone(),
            grant_status: self.status.clone(),
            status_reason: self.status_reason.clone(),
            version: self.version.clone(),
            granted_operations: self.granted_operations.clone(),
            options: self.options.clone(),
            grant_type: self.grant_type.clone(),
        }
    }
}

/// The ARN of the grant `id` in `region`, which for a grant is its home Region: a
/// grant is a resource OF the Region the create named as its home, like the license
/// it shares.
pub fn grant_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{GRANT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// Whether `arn` names an organization or an organizational unit: an ARN in the AWS
/// Organizations namespace whose resource is an organization or an OU.
///
/// What it is for: `Options.DesiredStatus` is admitted for those principals only, so
/// this is the question that decides it — an account principal, named in any other
/// namespace, is not one.
pub fn is_organization_principal(arn: &str) -> bool {
    // An ARN is `arn:<partition>:<service>:<region>:<account>:<resource…>`, and the
    // resource part itself may hold colons, so the first five fields are split off
    // and the rest is the resource.
    let mut fields = arn.splitn(6, ':');
    let is_arn = fields.next() == Some("arn");
    let _partition = fields.next();
    let service = fields.next();
    let _region = fields.next();
    let _account = fields.next();
    let resource = fields.next().unwrap_or_default();
    is_arn
        && service == Some(ORGANIZATIONS_ARN_SERVICE)
        && ORGANIZATION_RESOURCE_PREFIXES
            .iter()
            .any(|prefix| resource.starts_with(prefix))
}

/// The members a grant-minting request persists, borrowed for validation.
///
/// `CreateGrant` and `CreateGrantVersion` carry the same grant body, so the rule
/// that decides whether that body is storable lives once, here. Pass `None` for a
/// member the request does not carry.
#[derive(Debug)]
pub struct GrantInput<'a> {
    pub grant_name: Option<&'a str>,
    pub home_region: Option<&'a str>,
    pub principals: &'a [String],
    pub allowed_operations: &'a [String],
    pub grant_type: Option<&'a str>,
    pub options: Option<&'a Options>,
    pub tags: Option<&'a [Tag]>,
}

/// Why the grant body `input` carries is unacceptable, or `None` when every member
/// of it fits the store.
///
/// The modelled constraints — the length of `Principals` and `AllowedOperations`,
/// the ARN patterns — are refused before any handler runs. What is left is this
/// service's own: the length of the strings it stores, the Region shape of a value
/// that lands in a generated ARN, and the enum catalogs, guarded here so nothing
/// outside one can reach the store whatever the centralized walk does.
pub fn grant_input_rejection(input: &GrantInput<'_>) -> Option<String> {
    text_rejection("GrantName", input.grant_name)
        .or_else(|| {
            input
                .home_region
                .and_then(|region| region_rejection("HomeRegion", region))
        })
        .or_else(|| {
            input
                .principals
                .iter()
                .find_map(|principal| text_rejection("a Principals entry", Some(principal)))
        })
        .or_else(|| {
            input
                .allowed_operations
                .iter()
                .find(|operation| !ALLOWED_OPERATION_VALUES.contains(&operation.as_str()))
                .map(|operation| {
                    format!(
                        "an AllowedOperations entry must be one of [{}], got '{operation}'",
                        ALLOWED_OPERATION_VALUES.join(", ")
                    )
                })
        })
        .or_else(|| {
            input
                .grant_type
                .filter(|grant_type| !GRANT_TYPE_VALUES.contains(grant_type))
                .map(|grant_type| {
                    format!(
                        "GrantType must be one of [{}], got '{grant_type}'",
                        GRANT_TYPE_VALUES.join(", ")
                    )
                })
        })
        .or_else(|| input.options.and_then(options_rejection))
        .or_else(|| input.tags.and_then(tag_list_rejection))
}

/// Why the `Options` a grant request carries are unacceptable, or `None` when they
/// fit.
///
/// `DesiredStatus` is "the status of the grant after License Manager creates it. The
/// only supported value is `ACTIVE`"
/// (`smithy_query(query="doc",shape="Options",member="DesiredStatus")`), so it is
/// the one value of the `GrantStatus` catalog this member admits.
fn options_rejection(options: &Options) -> Option<String> {
    options
        .activation_override_behavior
        .as_deref()
        .filter(|behavior| !ACTIVATION_OVERRIDE_BEHAVIOR_VALUES.contains(behavior))
        .map(|behavior| {
            format!(
                "Options.ActivationOverrideBehavior must be one of [{}], got '{behavior}'",
                ACTIVATION_OVERRIDE_BEHAVIOR_VALUES.join(", ")
            )
        })
        .or_else(|| {
            options
                .desired_status
                .as_deref()
                .filter(|status| *status != GRANT_STATUS_ACTIVE)
                .map(|status| {
                    format!(
                        "Options.DesiredStatus must be '{GRANT_STATUS_ACTIVE}', got '{status}'"
                    )
                })
        })
}

/// The status a create answers for a grant whose request carried `options`.
///
/// A grant waits for its grantee: it is created `PENDING_ACCEPT` and stays there
/// until an accept or a reject moves it. `Options.DesiredStatus` is the one way a
/// create asks for another: specifying it "create[s] and activate[s] the grant in
/// one request".
pub fn created_grant_status(options: Option<&Options>) -> &'static str {
    let activating = options
        .and_then(|options| options.desired_status.as_deref())
        .is_some_and(|status| status == GRANT_STATUS_ACTIVE);
    if activating {
        GRANT_STATUS_ACTIVE
    } else {
        GRANT_STATUS_PENDING_ACCEPT
    }
}

impl ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// The grant `arn` names, for a reporting operation.
    pub fn grant(&self, arn: &str) -> Option<&StoredGrant> {
        self.grants.get(arn)
    }

    /// The grant the request carrying `client_token` minted, or `None` when no
    /// accepted `CreateGrant` has carried it. This is what makes the repeat of a
    /// create idempotent: the second call reports this record and mints nothing.
    pub fn grant_for_client_token(&self, client_token: &str) -> Option<&StoredGrant> {
        let arn = self.grant_client_tokens.get(client_token)?;
        self.grants.get(arn)
    }

    /// Store a newly minted grant, with the `ClientToken` it was minted under,
    /// refusing it — and changing nothing — when the store already holds
    /// [`MAX_GRANTS`] or already holds a grant under this ARN.
    ///
    /// The bound is enforced HERE, at the growth site, like every other store bound
    /// in this module, so no handler can grow the store past it by forgetting a
    /// pre-check, and an ARN collision can never silently replace a stored grant.
    /// `false` means nothing was stored.
    #[must_use]
    pub fn insert_grant(&mut self, grant: StoredGrant) -> bool {
        if self.grants.len() >= MAX_GRANTS || self.grants.contains_key(&grant.arn) {
            return false;
        }
        // The token map is written only here, so it can never name an ARN the store
        // does not hold. `ClientToken` is `^\S+$`, so the empty string is not one.
        if !grant.client_token.is_empty() {
            self.grant_client_tokens
                .insert(grant.client_token.clone(), grant.arn.clone());
        }
        self.grants.insert(grant.arn.clone(), grant);
        true
    }

    /// Remove the grant `arn` names, with its tags and the client token it was
    /// minted under. `None` when nothing matched.
    pub fn remove_grant(&mut self, arn: &str) -> Option<StoredGrant> {
        let grant = self.grants.remove(arn)?;
        self.tags.remove(arn);
        self.grant_client_tokens.retain(|_, minted| minted != arn);
        Some(grant)
    }

    /// A `g-<32 hex>` id no stored grant is using.
    pub fn fresh_grant_id(&mut self) -> String {
        let grants = &self.grants;
        fresh_id(
            &mut self.id_counter,
            GRANT_ID_PREFIX,
            grants.len(),
            |candidate| grants.keys().any(|arn| arn.ends_with(candidate)),
        )
    }
}
