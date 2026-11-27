// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanager-awslicensemanager service.
//!
//! The self-managed **license configuration** is this service's primary resource.
//! Every configuration is stored exactly once, keyed by its ARN, so each operation
//! of the lifecycle — create, get, list, update, delete — reads and writes the one
//! record, and a later read reports what an earlier call persisted.
//!
//! A **license** — the granted, entitlement-carrying resource `CreateLicense` mints
//! and `GetLicense` reports — is stored the same way, once, keyed by its ARN, beside
//! the `ClientToken` the request that minted it carried, so a repeat of that request
//! reports the same license instead of minting a second one.
//!
//! A license's **entitlement consumption** is held as one record per grant a
//! checkout answered, keyed by the consumption token that grant was issued under,
//! so the usage of an entitlement is the sum over the grants that still hold units:
//! a report of the usage recomputes it from stored state instead of reading a
//! counter, and releasing a grant drops exactly what it consumed. A released or
//! expired grant is KEPT, holding nothing, so the token it was issued under can
//! never be checked out, checked in or extended again — and a store at its cap
//! reclaims the record released longest ago, which no caller can tell apart,
//! because a released token is refused whether its record is still held or not.
//!
//! A **grant** — the shared use of one license's entitlements, which `CreateGrant`
//! mints and `GetGrant` reports — is stored the same way, keyed by its ARN, with the
//! rules and records that belong to it alone in [`grants`].
//!
//! Tags live in their own ARN-keyed map rather than on the record: `TagResource`,
//! `UntagResource` and `ListTagsForResource` are modelled against a generic
//! `ResourceArn`, so the map is the single place the tags of any taggable resource
//! this service grows later will live.
//!
//! Authorization and throttling are deliberately OUT OF SCOPE for this slice: the
//! emulator verifies every request signature centrally and models no per-principal
//! ownership of a configuration, so the `AccessDeniedException`,
//! `AuthorizationException` and `RateLimitExceededException` variants these
//! operations declare are never constructed here. They are modelled, not enforced.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    ConsumedLicenseSummary, ConsumptionConfiguration, DatetimeRange, Entitlement, EntitlementData,
    Issuer, IssuerDetails, License, LicenseConfiguration, ManagedResourceSummary, Metadata,
    ProductInformation, Tag, ENTITLEMENT_UNIT_VALUES, RENEW_TYPE_VALUES,
};

/// The grant — the shared use of one license's entitlements — and the records and
/// rules that belong to it alone.
mod grants;
pub use grants::*;

/// The ARN service namespace for this service: the model's `arnNamespace` and sigv4
/// name, `license-manager`
/// (`smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "license-manager";

/// The resource portion a license-configuration ARN carries before its id, so a
/// generated ARN reads
/// `arn:aws:license-manager:<region>:<account>:license-configuration:lic-<32 hex>`.
pub const CONFIGURATION_ARN_RESOURCE_PREFIX: &str = "license-configuration:";

/// The prefix of a license-configuration id.
pub const CONFIGURATION_ID_PREFIX: &str = "lic-";

/// Number of hex characters after [`CONFIGURATION_ID_PREFIX`] in a configuration id.
pub const CONFIGURATION_ID_HEX_LEN: usize = 32;

/// The resource portion a license ARN carries before its id, so a generated ARN
/// reads `arn:aws:license-manager:<home region>:<account>:license:l-<32 hex>`.
pub const LICENSE_ARN_RESOURCE_PREFIX: &str = "license:";

/// The prefix of a license id.
pub const LICENSE_ID_PREFIX: &str = "l-";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The status a license configuration carries until an `UpdateLicenseConfiguration`
/// moves it. `CreateLicenseConfiguration` returns a configuration that is usable at
/// once, so it is created in this terminal status and owes the clock no transition.
///
/// One of the modelled `LicenseConfigurationStatus` values
/// (`smithy_query(query="shape",shape="LicenseConfigurationStatus")`).
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The status a license carries from the moment `CreateLicense` mints it: AWS hands
/// back a license that can be checked out at once, so it is created in this terminal
/// status and owes the clock no transition. One of the modelled `LicenseStatus`
/// values (`smithy_query(query="shape",shape="LicenseStatus")`); the non-terminal
/// `PENDING_AVAILABLE` belongs to a license a grant workflow is still activating,
/// which nothing in this slice produces.
pub const LICENSE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `CheckoutType` of a checkout the license's provisional configuration bounds
/// in time. One of the modelled `CheckoutType` values
/// (`smithy_query(query="shape",shape="CheckoutType")`).
pub const CHECKOUT_TYPE_PROVISIONAL: &str = "PROVISIONAL";

/// The `CheckoutType` of a checkout no time to live bounds; it lasts as long as the
/// license it was taken on is valid. The other modelled `CheckoutType` value.
pub const CHECKOUT_TYPE_PERPETUAL: &str = "PERPETUAL";

/// The entitlement unit that carries a resource rather than a count: the modelled
/// `EntitlementUnit`/`EntitlementDataUnit` value `Entitlement.Value` is documented
/// against ("Entitlement resource. Use only if the unit is None").
pub const ENTITLEMENT_UNIT_NONE: &str = "None";

/// The fewest characters a `SignedToken` carries: the modelled minimum length of
/// the token a checkout answers with
/// (`smithy_query(query="shape",shape="SignedToken")`).
pub const SIGNED_TOKEN_MIN_LEN: usize = 4096;

/// The domain tag mixed into the key a grant's signed token is MAC'd under, and
/// into the filler that token carries, so neither can be confused with any other
/// digest this service derives.
const SIGNED_TOKEN_DOMAIN: &str = "license-manager:signed-token:v1";

/// The version `CreateLicense` gives a license it mints. License versions are
/// numbered from 1 and only `CreateLicenseVersion` moves one on
/// (`smithy_query(query="operation",operation="CreateLicenseVersion")`).
pub const FIRST_LICENSE_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// delete or untag, and the model publishes NO length for any of the lists or
// strings involved — so the centralized constraint walk cannot refuse an oversized
// one and each growth site carries its bound here. Each cap is this emulator's
// resource guard, set well above any realistic use; this service publishes no
// quota the model carries.
// ---------------------------------------------------------------------------

/// The most license configurations this emulator will hold. A create past it is
/// refused with the modelled `ResourceLimitExceededException` (HTTP 400), enforced
/// in [`ComAmazonawsLicensemanagerAwslicensemanagerState::insert_configuration`].
pub const MAX_LICENSE_CONFIGURATIONS: usize = 500;

/// The most licenses this emulator will hold, enforced in
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::insert_license`]. A create
/// past it stores nothing; `CreateLicense` models no resource-limit error, so its
/// handler renders the refusal as the modelled `ServerInternalException`.
pub const MAX_LICENSES: usize = 500;

/// The most consumption records this emulator will hold — one per outstanding
/// checkout of a license — enforced in
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::insert_consumption`]. A
/// record past it is not stored.
pub const MAX_LICENSE_CONSUMPTIONS: usize = 500;

/// The most tags one resource may carry. Emulator resource guard, matching the
/// 50-tags-per-resource ceiling AWS applies to tagged resources generally; enforced
/// in [`ComAmazonawsLicensemanagerAwslicensemanagerState::merge_tags`].
pub const MAX_TAGS: usize = 50;

/// The most entries this emulator stores for any other caller-sized list on a
/// configuration or a license — `LicenseRules`, `ProductInformationList`, a product
/// information's filter list, that filter's own value list, a license's
/// `Entitlements` and its `LicenseMetadata`.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// The longest caller-supplied text this emulator stores in a configuration, a
/// license or a tag.
pub const MAX_TEXT_LEN: usize = 2048;

/// The last segment of every issuer key fingerprint this service reports: fixed
/// text, not a per-issuer value, so the whole fingerprint is spelled out by the
/// account the license belongs to and the issuer name its create named.
const ISSUER_FINGERPRINT_SUFFIX: &str = "issuer-fingerprint";

/// The issuer key fingerprint a license of this issuer carries.
///
/// `KeyFingerprint` ("Issuer key fingerprint") is on the READ shape only:
/// `IssuerDetails`, which `License.Issuer` targets, carries `Name`, `SignKey` and
/// `KeyFingerprint`, while `Issuer` — the shape a create carries — has only `Name`
/// and `SignKey`
/// (`smithy_query(query="shape",shape="Issuer,IssuerDetails")`), and the model
/// publishes no pattern for it. No caller can supply a fingerprint, so the service
/// derives it, and it derives it as the colon-delimited text License Manager reports
/// for a license issuer: `aws:`, the account the license belongs to, the issuer
/// NAME exactly as the create stated it, and [`ISSUER_FINGERPRINT_SUFFIX`].
///
/// Only the account and the name vary, so the value is CONSTRUCTIBLE by a client
/// that knows the issuer it created a license with — which is what makes it usable
/// as the second half of the (`ProductSKU`, `KeyFingerprint`) key `CheckoutLicense`
/// resolves a license by — and it is stable per issuer name: every read of every
/// license of one issuer reports the same text. No key material is held or signed
/// here: the fingerprint IDENTIFIES the issuer, it does not certify it.
pub fn issuer_key_fingerprint(name: &str) -> String {
    format!(
        "aws:{}:{name}:{ISSUER_FINGERPRINT_SUFFIX}",
        ::engine::emulator_account_id()
    )
}

/// The `IssuerDetails` a read of a license reports for the `Issuer` its create
/// carried: the name and signing key exactly as given, plus the fingerprint this
/// service derives for that issuer.
///
/// Every path that stores a license issuer goes through here, so one issuer
/// fingerprints the same way whichever operation minted the license.
pub fn issuer_details(issuer: Issuer) -> IssuerDetails {
    IssuerDetails {
        key_fingerprint: Some(issuer_key_fingerprint(&issuer.name)),
        name: Some(issuer.name),
        sign_key: issuer.sign_key,
    }
}

/// One stored license configuration: every member `GetLicenseConfiguration`
/// reports, except the tags, which are keyed by ARN in
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::tags`].
#[derive(Debug, Clone, Default)]
pub struct StoredLicenseConfiguration {
    /// `lic-<32 hex>`: the ARN's last segment and this configuration's id.
    pub id: String,
    /// This record's store key.
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub license_counting_type: String,
    pub license_count: Option<i64>,
    pub license_count_hard_limit: Option<bool>,
    pub license_rules: Vec<String>,
    pub product_information_list: Vec<ProductInformation>,
    pub disassociate_when_not_found: Option<bool>,
    pub license_expiry: Option<i64>,
    /// One of the modelled `LicenseConfigurationStatus` values.
    pub status: String,
    pub owner_account_id: String,
    /// Licenses assigned to resources. Nothing in this slice associates a resource
    /// with a configuration (`UpdateLicenseSpecificationsForResource` and the
    /// checkout operations are still scaffolded), so it stays at the value a
    /// configuration with no association has.
    pub consumed_licenses: i64,
    pub consumed_license_summary_list: Vec<ConsumedLicenseSummary>,
    pub managed_resource_summary_list: Vec<ManagedResourceSummary>,
}

impl StoredLicenseConfiguration {
    /// Whether hard license enforcement is used — what the documented
    /// `enforceLicenseCount` list filter scopes on.
    pub fn enforces_license_count(&self) -> bool {
        self.license_count_hard_limit.unwrap_or(false)
    }

    /// Whether the available licenses have been exceeded — what the documented
    /// `usagelimitExceeded` list filter scopes on. A configuration that sets no
    /// `LicenseCount` has no limit to exceed.
    pub fn usage_limit_exceeded(&self) -> bool {
        self.license_count
            .is_some_and(|count| self.consumed_licenses > count)
    }

    /// This record as the modelled `LicenseConfiguration` structure
    /// `ListLicenseConfigurations` reports.
    ///
    /// The shape carries no `Tags` member — only `GetLicenseConfiguration` and
    /// `ListTagsForResource` report tags — so none are rendered here.
    /// `AutomatedDiscoveryInformation` is left absent: automated discovery is a
    /// service setting nothing in this slice turns on, so there is no last-run time
    /// to report.
    pub fn wire(&self) -> LicenseConfiguration {
        LicenseConfiguration {
            license_configuration_id: Some(self.id.clone()),
            license_configuration_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            license_counting_type: Some(self.license_counting_type.clone()),
            license_rules: Some(self.license_rules.clone()),
            license_count: self.license_count,
            license_count_hard_limit: self.license_count_hard_limit,
            disassociate_when_not_found: self.disassociate_when_not_found,
            consumed_licenses: Some(self.consumed_licenses),
            status: Some(self.status.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            consumed_license_summary_list: Some(self.consumed_license_summary_list.clone()),
            managed_resource_summary_list: Some(self.managed_resource_summary_list.clone()),
            product_information_list: Some(self.product_information_list.clone()),
            automated_discovery_information: None,
            license_expiry: self.license_expiry,
        }
    }
}

/// One stored license: every member `GetLicense` reports, plus the identity and the
/// `ClientToken` of the request that minted it.
///
/// `Tags` are NOT held here — a license is tagged through the same ARN-keyed map
/// every other taggable resource of this service uses.
#[derive(Debug, Clone, Default)]
pub struct StoredLicense {
    /// `l-<32 hex>`: the ARN's last segment and this license's id.
    pub id: String,
    /// This record's store key.
    pub arn: String,
    pub license_name: String,
    pub product_name: String,
    pub product_sku: String,
    /// The issuer, as the `IssuerDetails` a read reports: the `Issuer` the create
    /// carried plus the `KeyFingerprint` [`issuer_details`] derived for it, which is
    /// on the read shape only and so can come from nowhere else.
    pub issuer: IssuerDetails,
    pub home_region: String,
    /// One of the modelled `LicenseStatus` values.
    pub status: String,
    pub validity: DatetimeRange,
    pub beneficiary: String,
    pub entitlements: Vec<Entitlement>,
    pub consumption_configuration: ConsumptionConfiguration,
    /// Absent when the create carried none, so a read reports what the caller set
    /// and nothing more.
    pub license_metadata: Option<Vec<Metadata>>,
    /// The ISO8601-UTC instant this record was minted at.
    pub create_time: String,
    /// The license version this record is, as a decimal counted from
    /// [`FIRST_LICENSE_VERSION`].
    pub version: String,
    /// The `ClientToken` of the request that minted this license, so a repeat of
    /// that request is answered from this record instead of minting another.
    pub client_token: String,
}

impl StoredLicense {
    /// This record as the modelled `License` structure `GetLicense` reports.
    pub fn wire(&self) -> License {
        License {
            license_arn: Some(self.arn.clone()),
            license_name: Some(self.license_name.clone()),
            product_name: Some(self.product_name.clone()),
            product_sku: Some(self.product_sku.clone()),
            issuer: Some(self.issuer.clone()),
            home_region: Some(self.home_region.clone()),
            status: Some(self.status.clone()),
            validity: Some(self.validity.clone()),
            beneficiary: Some(self.beneficiary.clone()),
            entitlements: Some(self.entitlements.clone()),
            consumption_configuration: Some(self.consumption_configuration.clone()),
            license_metadata: self.license_metadata.clone(),
            create_time: Some(self.create_time.clone()),
            version: Some(self.version.clone()),
        }
    }
}

/// One checkout's consumption of a license's entitlements, keyed by the
/// `LicenseConsumptionToken` that checkout answered.
///
/// Consumption is persisted HERE and nowhere else, so a license's entitlement usage
/// is the sum over the grants that still hold units rather than a counter some
/// operation moved: a record is added when a checkout grants entitlements and stops
/// holding them when the grant is released or its expiration passes, and every read
/// of the usage recomputes the same total from them.
#[derive(Debug, Clone, Default)]
pub struct StoredLicenseConsumption {
    /// The consumption token the checkout minted; this record's store key.
    pub token: String,
    /// The ARN of the license whose entitlements this record consumes.
    pub license_arn: String,
    /// The `ClientToken` of the checkout that was answered with this grant, so a
    /// repeat of that checkout is answered from this record instead of granting the
    /// same entitlements twice.
    pub client_token: String,
    /// The `CheckoutType` the checkout asked for; one of
    /// [`CHECKOUT_TYPE_PROVISIONAL`] and [`CHECKOUT_TYPE_PERPETUAL`].
    pub checkout_type: String,
    /// The node the checkout named, absent when it named none.
    pub node_id: Option<String>,
    /// The beneficiary the checkout named, absent when it named none.
    pub beneficiary: Option<String>,
    /// The entitlements this grant was allowed, as the checkout that minted it
    /// reports them.
    pub entitlements_allowed: Vec<EntitlementData>,
    /// How many units of each entitlement — keyed by the `Entitlement.Name` the
    /// license declares — this checkout holds, in that entitlement's own unit.
    pub consumed_units: BTreeMap<String, i64>,
    /// The ISO8601-UTC instant this grant was issued at.
    pub issued_at: String,
    /// The ISO8601-UTC instant this grant expires at, absent when nothing bounds
    /// it.
    pub expiration: Option<String>,
    /// The instant a check-in released this grant, absent while it is still
    /// outstanding. A released grant is kept, so its token can never be reused.
    pub checked_in_at: Option<String>,
}

impl StoredLicenseConsumption {
    /// Whether this grant is still outstanding: no check-in has released it.
    pub fn is_outstanding(&self) -> bool {
        self.checked_in_at.is_none()
    }

    /// Whether this grant's expiration has passed.
    pub fn has_expired(&self) -> bool {
        self.expiration
            .as_deref()
            .and_then(parse_instant)
            .is_some_and(|expiration| expiration <= now_secs())
    }

    /// Whether this grant still holds the units it consumed: a grant holds them
    /// until a check-in releases it or its expiration passes, whichever comes
    /// first.
    ///
    /// An expiration the clock has already made due releases the units whether or
    /// not anybody calls, so it is answered HERE, from the stored instant, rather
    /// than written into the store by whichever operation happens to look next.
    pub fn holds_units(&self) -> bool {
        self.is_outstanding() && !self.has_expired()
    }

    /// The `SignedToken` a checkout of this grant answers with: a compact JWS,
    /// `<header>.<claims>.<mac>` with each segment base64url, whose claims are THIS
    /// record and the license it is a grant on — so the token is derived from stored
    /// state and from nothing else, and the repeat of a checkout answers the token
    /// its first call answered.
    ///
    /// This emulator holds no issuer key material — [`issuer_key_fingerprint`]
    /// IDENTIFIES an issuer, it does not certify one — so the third segment is an
    /// HMAC-SHA256 over the first two under a key derived from that fingerprint: it
    /// binds the token to one grant of one issuer's licenses in this store, and
    /// certifies nothing.
    ///
    /// The member is modelled at [`SIGNED_TOKEN_MIN_LEN`] characters or more, which
    /// the claims of a single grant do not reach, so a filler claim expanded from
    /// this grant's own token carries it up to that length.
    pub fn signed_token(&self, license: &StoredLicense) -> String {
        let header = base64url(
            ::serde_json::json!({"alg": "HS256", "typ": "JWT"})
                .to_string()
                .as_bytes(),
        );
        let mut filler_len = 0_usize;
        loop {
            let claims = self.token_claims(license, &token_filler(&self.token, filler_len));
            let token = jws(&header, &claims.to_string(), &signed_token_key(license));
            if token.len() >= SIGNED_TOKEN_MIN_LEN {
                return token;
            }
            // Every filler character lengthens the token by at least one, so
            // raising the filler by the shortfall converges in a couple of rounds.
            filler_len += SIGNED_TOKEN_MIN_LEN - token.len();
        }
    }

    /// The claims this grant's signed token carries: the grant, and the license it
    /// is a grant on, plus `filler` under a `pad` claim — the token stays one whose
    /// MAC covers every claim in it, and the padding is what carries its length up to
    /// the modelled minimum.
    fn token_claims(&self, license: &StoredLicense, filler: &str) -> ::serde_json::Value {
        ::serde_json::json!({
            "iss": license.issuer.name,
            "sub": license.arn,
            "jti": self.token,
            "iat": parse_instant(&self.issued_at).map(|secs| secs as i64),
            "exp": self.expiration.as_deref().and_then(parse_instant).map(|secs| secs as i64),
            "ProductSKU": license.product_sku,
            "KeyFingerprint": license.issuer.key_fingerprint,
            "CheckoutType": self.checkout_type,
            "NodeId": self.node_id,
            "Beneficiary": self.beneficiary,
            "LicenseName": license.license_name,
            "ProductName": license.product_name,
            "HomeRegion": license.home_region,
            "Validity": {"Begin": license.validity.begin, "End": license.validity.end},
            "EntitlementsAllowed": self
                .entitlements_allowed
                .iter()
                .map(|entitlement| ::serde_json::json!({
                    "Name": entitlement.name,
                    "Value": entitlement.value,
                    "Unit": entitlement.unit,
                }))
                .collect::<Vec<_>>(),
            "pad": filler,
        })
    }
}

/// One compact JWS over `claims`: the base64url of `header` and of `claims`, joined
/// by `.`, then the base64url of the HMAC-SHA256 of that text under `key`.
fn jws(header: &str, claims: &str, key: &[u8]) -> String {
    let signing_input = format!("{header}.{}", base64url(claims.as_bytes()));
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        // HMAC accepts a key of any length, so this cannot fail for a digest.
        .expect("HMAC-SHA256 takes a key of any length");
    mac.update(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        base64url(&mac.finalize().into_bytes())
    )
}

/// One base64url segment of a compact JWS: unpadded, as that encoding requires.
fn base64url(bytes: &[u8]) -> String {
    ::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The key a grant on `license` has its signed token MAC'd under: the digest of
/// this service's token domain and the license issuer's fingerprint. That
/// fingerprint is a public identifier a client can spell out for itself
/// ([`issuer_key_fingerprint`]), so no key material is held or protected here — the
/// MAC makes a token self-consistent against this store, and nothing in this service
/// verifies a token or grants anything on one: it does not certify it.
fn signed_token_key(license: &StoredLicense) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SIGNED_TOKEN_DOMAIN.as_bytes());
    digest.update(
        license
            .issuer
            .key_fingerprint
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    digest.finalize().into()
}

/// `len` characters of filler for a signed token's filler claim, expanded from
/// `seed` block by block so one grant's token is stable and two grants' tokens
/// differ.
fn token_filler(seed: &str, len: usize) -> String {
    let mut filler = String::with_capacity(len);
    let mut block = 0_u64;
    while filler.len() < len {
        let mut digest = Sha256::new();
        digest.update(SIGNED_TOKEN_DOMAIN.as_bytes());
        digest.update(seed.as_bytes());
        digest.update(block.to_be_bytes());
        filler.push_str(&::hex::encode(digest.finalize()));
        block = block.wrapping_add(1);
    }
    filler.truncate(len);
    filler
}

/// The instant a checkout of `license` of type `checkout_type`, issued at
/// `issued_at`, expires at — or `None` when nothing bounds it.
///
/// A `PROVISIONAL` checkout is bounded by the license's own
/// `ConsumptionConfiguration.ProvisionalConfiguration.MaxTimeToLiveInMinutes`
/// ("Maximum time for the provisional configuration, in minutes",
/// `smithy_query(query="shape",shape="ProvisionalConfiguration")`), so it expires
/// that many minutes after it is issued. No checkout outlives the license it is a
/// checkout of, so a validity end that comes first wins; a `PERPETUAL` checkout —
/// one no time to live bounds — expires exactly when the license's validity ends,
/// and never when the license names no end.
pub fn checkout_expiration(
    license: &StoredLicense,
    checkout_type: &str,
    issued_at: &str,
) -> Option<String> {
    let time_to_live = (checkout_type == CHECKOUT_TYPE_PROVISIONAL)
        .then_some(license.consumption_configuration.provisional_configuration.as_ref())
        .flatten()
        .and_then(|provisional| {
            instant_plus_minutes(issued_at, i64::from(provisional.max_time_to_live_in_minutes))
        });
    match (time_to_live, license.validity.end.clone()) {
        (Some(bound), Some(validity_end)) => Some(earlier(bound, validity_end)),
        (Some(bound), None) => Some(bound),
        (None, validity_end) => validity_end,
    }
}

/// The earlier of two ISO8601 instants, decided on the PARSED instants so a zone
/// offset is honoured rather than compared as text. An instant this emulator cannot
/// parse leaves `first` in place, because the member's modelled pattern is what
/// admits it.
fn earlier(first: String, second: String) -> String {
    match (parse_instant(&first), parse_instant(&second)) {
        (Some(first_secs), Some(second_secs)) if second_secs < first_secs => second,
        _ => first,
    }
}

/// `instant` moved on by `minutes`, as an ISO8601-UTC instant, or `None` when
/// `instant` is not one this emulator can parse.
fn instant_plus_minutes(instant: &str, minutes: i64) -> Option<String> {
    let seconds = parse_instant(instant)? as i64 + minutes.saturating_mul(60);
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// What a checkout of a license's entitlements was granted: the entitlements it is
/// allowed, and the units of each of them it consumes.
#[derive(Debug, Clone, Default)]
pub struct CheckoutGrant {
    /// The entitlements the checkout is allowed, in the order it asked for them.
    pub entitlements_allowed: Vec<EntitlementData>,
    /// The units of each allowed entitlement the checkout consumes, keyed by the
    /// `Entitlement.Name` the license declares.
    pub consumed_units: BTreeMap<String, i64>,
}

/// Why a checkout of a license's entitlements is not allowed, as the modelled error
/// the refusal is answered on.
#[derive(Debug, Clone)]
pub enum CheckoutRefusal {
    /// The license declares no entitlement the request named, or an entitlement it
    /// named is already consumed up to the maximum count it caps — the two cases
    /// the modelled `NoEntitlementsAllowedException` carries.
    NoEntitlementsAllowed(String),
    /// A value the request carried for an entitlement is not a count of that
    /// entitlement's unit — the modelled `InvalidParameterValueException`.
    InvalidParameterValue(String),
}

/// The message the modelled `ResourceNotFoundException` carries for a license
/// consumption token that names no checkout this service can act on, as AWS words it
/// for that error class. The member it names is the request's own
/// `LicenseConsumptionToken`, and the value is not echoed back.
pub const INVALID_CONSUMPTION_TOKEN: &str = "Invalid value of consumptionToken.";

/// The message the modelled `NoEntitlementsAllowedException` carries, as AWS words
/// it for that error class: a license that declares none of the entitlements a
/// checkout asked for, and one whose entitlement is already consumed up to the
/// maximum count it caps, are the same refusal in the same words.
const NO_ENTITLEMENTS_ALLOWED: &str = "No Entitlements Allowed.";

/// How many units of `entitlement` the request `data` takes of it.
///
/// `Value` is the count for every unit but `None`, which the model documents as
/// carrying the entitlement RESOURCE rather than a count, so a grant on one of those
/// is one grant; a request that carries no `Value` takes one unit. A count the
/// entitlement's unit cannot hold — text that is not a number, or a negative one —
/// is refused rather than rounded or ignored.
fn requested_units(
    entitlement: &Entitlement,
    data: &EntitlementData,
) -> Result<i64, CheckoutRefusal> {
    if entitlement.unit == ENTITLEMENT_UNIT_NONE {
        return Ok(1);
    }
    match data.value.as_deref() {
        None => Ok(1),
        Some(value) => value
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|units| *units >= 0)
            .ok_or_else(|| {
                CheckoutRefusal::InvalidParameterValue(format!(
                    "Value of the entitlement '{}' must be a count of {}, got '{value}'",
                    data.name, entitlement.unit
                ))
            }),
    }
}

/// The `Value` a checkout reports for an entitlement it was allowed: the units it
/// was granted for a counted entitlement, and the entitlement's own resource for one
/// whose unit is `None`.
fn granted_value(entitlement: &Entitlement, units: i64) -> Option<String> {
    if entitlement.unit == ENTITLEMENT_UNIT_NONE {
        entitlement.value.clone()
    } else {
        Some(units.to_string())
    }
}

/// What a check-in of a consumption token did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckInOutcome {
    /// The grant was still holding its units, and this check-in released them.
    Released,
    /// The token names a grant that holds nothing any more: an earlier check-in or
    /// its expiration released it.
    AlreadyReleased,
    /// No checkout has ever been answered with this token.
    UnknownToken,
}

/// Why the members a checkout carries are unacceptable, or `None` when every one of
/// them fits the store.
///
/// The model publishes no length for any string on a checkout and no size for its
/// entitlement list, so those bounds live here with the store that holds them, like
/// every other caller-sized value of this service.
pub fn checkout_input_rejection(
    product_sku: &str,
    key_fingerprint: &str,
    node_id: Option<&str>,
    beneficiary: Option<&str>,
    entitlements: &[EntitlementData],
) -> Option<String> {
    text_rejection("ProductSKU", Some(product_sku))
        .or_else(|| text_rejection("KeyFingerprint", Some(key_fingerprint)))
        .or_else(|| text_rejection("NodeId", node_id))
        .or_else(|| text_rejection("Beneficiary", beneficiary))
        .or_else(|| {
            list_rejection(
                "Entitlements",
                entitlements.len(),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            entitlements.iter().find_map(|data| {
                text_rejection("an Entitlements entry's Name", Some(&data.name)).or_else(|| {
                    text_rejection("an Entitlements entry's Value", data.value.as_deref())
                })
            })
        })
}

/// Per-service state for com-amazonaws-licensemanager-awslicensemanager. A handler
/// reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// Every license configuration, keyed by its ARN. A `BTreeMap` so
    /// `ListLicenseConfigurations` walks one stable, total order on every call and a
    /// page token can name the ARN the next page starts at — no token table, and no
    /// page that skips or repeats a configuration created between two calls.
    pub license_configurations: BTreeMap<String, StoredLicenseConfiguration>,
    /// Every license, keyed by its ARN, for the same reason: one record per license,
    /// walked in one stable total order.
    pub licenses: BTreeMap<String, StoredLicense>,
    /// The ARN each accepted `CreateLicense` `ClientToken` minted, so the repeat of
    /// a request is idempotent. Written only by
    /// [`Self::insert_license`] and dropped with its license by
    /// [`Self::remove_license`], so it holds at most one entry per stored license
    /// and is bounded by [`MAX_LICENSES`].
    pub license_client_tokens: BTreeMap<String, String>,
    /// Every consumption of a license's entitlements, keyed by the consumption token
    /// it was granted under, whether it still holds those units or a check-in or its
    /// expiration has released them. A `BTreeMap` so the usage read walks them in one
    /// stable order, and so a token names at most one grant for ever.
    pub license_consumptions: BTreeMap<String, StoredLicenseConsumption>,
    /// Every grant, keyed by its ARN, for the same reason: one record per grant,
    /// walked in one stable total order.
    pub grants: BTreeMap<String, StoredGrant>,
    /// The ARN each accepted `CreateGrant` `ClientToken` minted, so the repeat of a
    /// request is idempotent. Written only by [`Self::insert_grant`] and dropped
    /// with its grant by [`Self::remove_grant`], so it holds at most one entry per
    /// stored grant and is bounded by [`MAX_GRANTS`].
    pub grant_client_tokens: BTreeMap<String, String>,
    /// Tags, keyed by resource ARN then tag key. Shared by `TagResource`,
    /// `UntagResource` and `ListTagsForResource`, and seeded from the `Tags` passed
    /// to `CreateLicenseConfiguration`, `CreateLicense` or `CreateGrant`.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// Mixed into each generated id so two creates in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// The configuration `arn` names, for a reporting operation.
    pub fn configuration(&self, arn: &str) -> Option<&StoredLicenseConfiguration> {
        self.license_configurations.get(arn)
    }

    /// The configuration `arn` names, for an operation the spec says mutates it.
    ///
    /// `id` and `arn` are this record's identity — the store is keyed on the ARN —
    /// so no handler reassigns them through this handle.
    pub fn configuration_mut(&mut self, arn: &str) -> Option<&mut StoredLicenseConfiguration> {
        self.license_configurations.get_mut(arn)
    }

    /// Store a newly created configuration, refusing it — and changing nothing —
    /// when the store already holds [`MAX_LICENSE_CONFIGURATIONS`] or already holds
    /// a configuration under this ARN.
    ///
    /// The bound is enforced HERE, at the growth site, so no handler can grow the
    /// store past it by forgetting a pre-check, and an ARN collision can never
    /// silently replace a stored configuration. `false` means nothing was stored;
    /// the caller renders that as its own modelled resource-limit error.
    #[must_use]
    pub fn insert_configuration(&mut self, configuration: StoredLicenseConfiguration) -> bool {
        if self.license_configurations.len() >= MAX_LICENSE_CONFIGURATIONS
            || self
                .license_configurations
                .contains_key(&configuration.arn)
        {
            return false;
        }
        self.license_configurations
            .insert(configuration.arn.clone(), configuration);
        true
    }

    /// Remove the configuration `arn` names, with its tags. `None` when nothing
    /// matched.
    pub fn remove_configuration(&mut self, arn: &str) -> Option<StoredLicenseConfiguration> {
        let configuration = self.license_configurations.remove(arn)?;
        self.tags.remove(arn);
        Some(configuration)
    }

    /// The license `arn` names, for a reporting operation.
    pub fn license(&self, arn: &str) -> Option<&StoredLicense> {
        self.licenses.get(arn)
    }

    /// The license the request carrying `client_token` minted, or `None` when no
    /// accepted `CreateLicense` has carried it. This is what makes the repeat of a
    /// create idempotent: the second call reports this record and mints nothing.
    pub fn license_for_client_token(&self, client_token: &str) -> Option<&StoredLicense> {
        let arn = self.license_client_tokens.get(client_token)?;
        self.licenses.get(arn)
    }

    /// Store a newly minted license, with the `ClientToken` it was minted under,
    /// refusing it — and changing nothing — when the store already holds
    /// [`MAX_LICENSES`] or already holds a license under this ARN.
    ///
    /// The bound is enforced HERE, at the growth site, so no handler can grow the
    /// store past it by forgetting a pre-check, and an ARN collision can never
    /// silently replace a stored license. `false` means nothing was stored.
    #[must_use]
    pub fn insert_license(&mut self, license: StoredLicense) -> bool {
        if self.licenses.len() >= MAX_LICENSES || self.licenses.contains_key(&license.arn) {
            return false;
        }
        // The token map is written only here, so it can never name an ARN the store
        // does not hold. `ClientToken` is `^\S+$`, so the empty string is not one.
        if !license.client_token.is_empty() {
            self.license_client_tokens
                .insert(license.client_token.clone(), license.arn.clone());
        }
        self.licenses.insert(license.arn.clone(), license);
        true
    }

    /// Remove the license `arn` names, with its tags and the client token it was
    /// minted under. `None` when nothing matched.
    pub fn remove_license(&mut self, arn: &str) -> Option<StoredLicense> {
        let license = self.licenses.remove(arn)?;
        self.tags.remove(arn);
        self.license_client_tokens.retain(|_, minted| minted != arn);
        // Nothing consumes the entitlements of a license the store no longer holds,
        // so its consumption records go with it and can never be summed into another
        // license's usage.
        self.license_consumptions
            .retain(|_, consumption| consumption.license_arn != arn);
        Some(license)
    }

    /// Store one checkout's consumption of a license's entitlements, refusing it —
    /// and changing nothing — when this token already names a grant, or when the
    /// store is at [`MAX_LICENSE_CONSUMPTIONS`] and every grant it holds still holds
    /// its units.
    ///
    /// The bound is enforced HERE, at the growth site, like every other store bound
    /// in this file, so no caller can grow the map past it by forgetting a
    /// pre-check, and a repeated token can never silently replace a stored grant.
    /// It bounds the grants this emulator HOLDS rather than the history of them: at
    /// the cap, the grant released longest ago is reclaimed to make room, which no
    /// caller can tell apart, because a token whose grant was released is refused
    /// whether the record is still there or not.
    /// `false` means nothing was stored.
    #[must_use]
    pub fn insert_consumption(&mut self, consumption: StoredLicenseConsumption) -> bool {
        if self
            .license_consumptions
            .contains_key(&consumption.token)
            || consumption.token.is_empty()
        {
            return false;
        }
        if self.license_consumptions.len() >= MAX_LICENSE_CONSUMPTIONS
            && !self.reclaim_released_consumption()
        {
            return false;
        }
        self.license_consumptions
            .insert(consumption.token.clone(), consumption);
        true
    }

    /// Drop the grant that stopped holding its units longest ago, and say whether
    /// there was one to drop. A grant released by a check-in carries the instant
    /// that released it; one released by its expiration carries none, so it is
    /// reclaimed first, and the issue order breaks any tie.
    fn reclaim_released_consumption(&mut self) -> bool {
        let reclaimable = self
            .license_consumptions
            .values()
            .filter(|consumption| !consumption.holds_units())
            .min_by(|first, second| {
                first
                    .checked_in_at
                    .cmp(&second.checked_in_at)
                    .then_with(|| first.issued_at.cmp(&second.issued_at))
            })
            .map(|consumption| consumption.token.clone());
        match reclaimable {
            Some(token) => self.license_consumptions.remove(&token).is_some(),
            None => false,
        }
    }

    /// The license a checkout of `product_sku` presenting `key_fingerprint`
    /// resolves to, or `None` when the store holds no license of that product from
    /// that issuer.
    ///
    /// A checkout names no ARN: it names the product and the issuer key fingerprint,
    /// which together are the key [`issuer_key_fingerprint`] exists to make
    /// resolvable. The walk is over the ARN-keyed store, so the resolution is in one
    /// stable total order on every call.
    pub fn license_for_checkout(
        &self,
        product_sku: &str,
        key_fingerprint: &str,
    ) -> Option<&StoredLicense> {
        self.licenses.values().find(|license| {
            license.product_sku == product_sku
                && license.issuer.key_fingerprint.as_deref() == Some(key_fingerprint)
        })
    }

    /// Whether the store holds any license of the product `product_sku`, whatever
    /// issuer issued it. What a checkout whose (`ProductSKU`, `KeyFingerprint`) pair
    /// resolved nothing asks to say WHICH half of the pair named nothing.
    pub fn has_license_for_product_sku(&self, product_sku: &str) -> bool {
        self.licenses
            .values()
            .any(|license| license.product_sku == product_sku)
    }

    /// The grant `token` was issued under, whether it still holds its units or an
    /// earlier check-in or its expiration has released them.
    pub fn consumption(&self, token: &str) -> Option<&StoredLicenseConsumption> {
        self.license_consumptions.get(token)
    }

    /// The still-held grant on the license `license_arn` that the checkout carrying
    /// `client_token` was answered with, or `None` when no accepted checkout of that
    /// license carried it. This is what makes the repeat of a checkout idempotent:
    /// the second call reports this grant and consumes nothing more.
    ///
    /// The token is scoped to the license it was used against, like the one
    /// [`Self::insert_license`] keys a create by, so reusing a token on another
    /// license is answered with that license's own grant rather than this one's.
    pub fn held_consumption_for_client_token(
        &self,
        license_arn: &str,
        client_token: &str,
    ) -> Option<&StoredLicenseConsumption> {
        self.license_consumptions.values().find(|consumption| {
            consumption.license_arn == license_arn
                && consumption.client_token == client_token
                && consumption.holds_units()
        })
    }

    /// Move the expiration of the grant `token` names on to `expiration`, and say
    /// whether a grant was there to move. A grant that holds nothing any more is not
    /// moved: the caller resolves it first, so this only writes the new bound.
    pub fn extend_consumption(&mut self, token: &str, expiration: Option<String>) -> bool {
        match self.license_consumptions.get_mut(token) {
            Some(consumption) if consumption.holds_units() => {
                consumption.expiration = expiration;
                true
            }
            _ => false,
        }
    }

    /// Release the grant `token` names, at the instant `at`, and say what that did.
    ///
    /// The record is KEPT, holding nothing from now on, so the token names the grant
    /// it was issued for: a grant that holds nothing any more — released by an
    /// earlier check-in or by its expiration — releases nothing a second time, which
    /// is the same question [`Self::extend_consumption`] asks of a token, and no
    /// later checkout can be answered with it.
    pub fn check_in_consumption(&mut self, token: &str, at: String) -> CheckInOutcome {
        match self.license_consumptions.get_mut(token) {
            None => CheckInOutcome::UnknownToken,
            Some(consumption) if !consumption.holds_units() => CheckInOutcome::AlreadyReleased,
            Some(consumption) => {
                consumption.checked_in_at = Some(at);
                CheckInOutcome::Released
            }
        }
    }

    /// What a checkout of `requested` against `license` is granted, or why it is
    /// refused.
    ///
    /// A requested entitlement is matched against the ones the license DECLARES, by
    /// name and unit together: a name the license does not declare, and a name it
    /// declares in another unit, are both entitlements this license does not have.
    /// The units a request takes of an entitlement are the count its `Value` names —
    /// one unit when it names none, and one grant on the resource when the
    /// entitlement's unit is `None`, where the model documents `Value` as the
    /// resource rather than a count. A grant is allowed only while the units it
    /// would add leave that entitlement's total at or below the maximum count the
    /// entitlement caps, unless the entitlement allows overage, and an entitlement
    /// that caps no maximum count caps nothing
    /// (`smithy_query(query="shape",shape="Entitlement")`).
    pub fn allow_checkout(
        &self,
        license: &StoredLicense,
        requested: &[EntitlementData],
    ) -> Result<CheckoutGrant, CheckoutRefusal> {
        if requested.is_empty() {
            return Err(CheckoutRefusal::NoEntitlementsAllowed(
                NO_ENTITLEMENTS_ALLOWED.to_string(),
            ));
        }
        let mut grant = CheckoutGrant::default();
        for data in requested {
            let entitlement = license
                .entitlements
                .iter()
                .find(|entitlement| entitlement.name == data.name && entitlement.unit == data.unit)
                .ok_or_else(|| {
                    CheckoutRefusal::NoEntitlementsAllowed(NO_ENTITLEMENTS_ALLOWED.to_string())
                })?;
            let units = requested_units(entitlement, data)?;
            let running = grant
                .consumed_units
                .entry(entitlement.name.clone())
                .or_default();
            *running = running.saturating_add(units);
            let total = self
                .consumed_entitlement_units(&license.arn, &entitlement.name)
                .saturating_add(*running);
            if let Some(max_count) = entitlement.max_count {
                if total > max_count && !entitlement.overage.unwrap_or(false) {
                    return Err(CheckoutRefusal::NoEntitlementsAllowed(
                        NO_ENTITLEMENTS_ALLOWED.to_string(),
                    ));
                }
            }
            grant.entitlements_allowed.push(EntitlementData {
                name: entitlement.name.clone(),
                value: granted_value(entitlement, units),
                unit: entitlement.unit.clone(),
            });
        }
        Ok(grant)
    }

    /// A `<32 hex>` consumption token no grant in the store is keyed by.
    pub fn fresh_consumption_token(&mut self) -> String {
        let consumptions = &self.license_consumptions;
        fresh_id(
            &mut self.id_counter,
            // No prefix: the model types `LicenseConsumptionToken` as a plain String
            // and publishes no format for it
            // (`smithy_query(query="operation",operation="CheckoutLicense")`), so a
            // token is minted as the same opaque `<32 hex>` id every other record of
            // this service is keyed by, under no prefix that would claim a format AWS
            // does not document.
            "",
            consumptions.len(),
            |candidate| consumptions.contains_key(candidate),
        )
    }

    /// How many units of the entitlement `entitlement_name` of the license
    /// `license_arn` are consumed: the sum of that entitlement's units over every
    /// grant the store holds against that license that still holds its units. Zero
    /// when none does, which is what a license nothing has checked out consumes.
    ///
    /// A read of the usage computes this; it never writes, so two reads of an
    /// unchanged store report the same total.
    pub fn consumed_entitlement_units(&self, license_arn: &str, entitlement_name: &str) -> i64 {
        self.license_consumptions
            .values()
            .filter(|consumption| {
                consumption.license_arn == license_arn && consumption.holds_units()
            })
            .filter_map(|consumption| consumption.consumed_units.get(entitlement_name))
            .fold(0_i64, |total, units| total.saturating_add(*units))
    }

    /// Whether `arn` names a resource this service stores, and can therefore tag: a
    /// license configuration, a license or a grant. Each is ARN-keyed and capped, so
    /// the number of KEYS in the tag map is bounded by
    /// [`MAX_LICENSE_CONFIGURATIONS`] + [`MAX_LICENSES`] + [`MAX_GRANTS`].
    ///
    /// `TagResource`, `UntagResource` and `ListTagsForResource` are modelled against
    /// a generic `ResourceArn`, so each of them asks THIS question rather than
    /// naming one resource kind.
    pub fn is_taggable(&self, arn: &str) -> bool {
        self.license_configurations.contains_key(arn)
            || self.licenses.contains_key(arn)
            || self.grants.contains_key(arn)
    }

    /// The tags stored for `arn`, as the modelled `TagList`. Empty when none were
    /// ever set.
    pub fn tags_of(&self, arn: &str) -> Vec<Tag> {
        self.tags
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The number of distinct tag keys `arn` would carry after merging `tags`.
    fn tag_count_after_merge(&self, arn: &str, tags: &[Tag]) -> usize {
        let mut keys: std::collections::BTreeSet<&str> = self
            .tags
            .get(arn)
            .map(|stored| stored.keys().map(String::as_str).collect())
            .unwrap_or_default();
        for tag in tags {
            if let Some(key) = tag.key.as_deref() {
                keys.insert(key);
            }
        }
        keys.len()
    }

    /// Merge `tags` into the tags of `arn`, replacing the value of a key already
    /// present. A tag whose `Key` is absent carries nothing to key on and is
    /// skipped; an absent `Value` stores the empty string, which is what the
    /// `Value` member admits.
    ///
    /// Every bound on this growth site is enforced HERE, so no handler can grow the
    /// map past one by forgetting a pre-check, and `Err` always means nothing was
    /// written (the caller renders it as its own modelled refusal):
    ///
    /// * `arn` must name a resource the store holds, so the number of KEYS in the
    ///   outer map can never exceed the number of stored configurations and
    ///   licenses, and is therefore bounded by [`MAX_LICENSE_CONFIGURATIONS`] +
    ///   [`MAX_LICENSES`] + [`MAX_GRANTS`]. A tag map entry is also dropped with its
    ///   resource by [`Self::remove_configuration`], [`Self::remove_license`] or
    ///   [`Self::remove_grant`], so the map cannot outlive what it describes.
    /// * that ARN is length-bounded like every other stored string, so no single
    ///   key can be arbitrarily large.
    /// * the merge may not leave `arn` carrying more than [`MAX_TAGS`] distinct
    ///   keys.
    pub fn merge_tags(&mut self, arn: &str, tags: &[Tag]) -> Result<(), String> {
        if let Some(why) = text_rejection("ResourceArn", Some(arn)) {
            return Err(why);
        }
        if !self.is_taggable(arn) {
            return Err(invalid_resource_arn_message(arn));
        }
        let after = self.tag_count_after_merge(arn, tags);
        if after > MAX_TAGS {
            return Err(format!(
                "this resource would carry {after} tags, more than the {MAX_TAGS} this service accepts"
            ));
        }
        let stored = self.tags.entry(arn.to_string()).or_default();
        for tag in tags {
            if let Some(key) = tag.key.as_deref() {
                stored.insert(key.to_string(), tag.value.clone().unwrap_or_default());
            }
        }
        Ok(())
    }

    /// Drop `keys` from the tags of `arn`. A key the resource does not carry is not
    /// an error: untagging is idempotent.
    pub fn remove_tag_keys(&mut self, arn: &str, keys: &[String]) {
        if let Some(stored) = self.tags.get_mut(arn) {
            for key in keys {
                stored.remove(key);
            }
            if stored.is_empty() {
                self.tags.remove(arn);
            }
        }
    }

    /// A `lic-<32 hex>` id no stored configuration is using.
    pub fn fresh_configuration_id(&mut self) -> String {
        let configurations = &self.license_configurations;
        fresh_id(
            &mut self.id_counter,
            CONFIGURATION_ID_PREFIX,
            configurations.len(),
            |candidate| configurations.keys().any(|arn| arn.ends_with(candidate)),
        )
    }

    /// An `l-<32 hex>` id no stored license is using.
    pub fn fresh_license_id(&mut self) -> String {
        let licenses = &self.licenses;
        fresh_id(
            &mut self.id_counter,
            LICENSE_ID_PREFIX,
            licenses.len(),
            |candidate| licenses.keys().any(|arn| arn.ends_with(candidate)),
        )
    }
}

/// One `<prefix><32 hex>` id that `taken` does not already claim.
///
/// Shared by every id this service mints, so each kind differs only in its prefix
/// and in the records it must not collide with. `stored` is how many records that
/// kind already holds, which bounds the fallback walk below.
fn fresh_id(
    counter: &mut u64,
    prefix: &str,
    stored: usize,
    taken: impl Fn(&str) -> bool,
) -> String {
    for _ in 0..1024 {
        *counter = counter.wrapping_add(1);
        let candidate = id_from_seed(prefix, now_nanos() ^ counter.rotate_left(17), *counter);
        if !taken(&candidate) {
            return candidate;
        }
    }
    // Bounded fallback, so a pathological clock can never spin here: the
    // zero-padded hex counters are also `<prefix>[0-9a-f]{32}`, and there are always
    // more of them than there are stored records.
    (0u64..=stored as u64)
        .map(|n| format!("{prefix}{n:032x}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| format!("{prefix}{stored:032x}"))
}

/// The members a license-minting request persists, borrowed for validation.
///
/// `CreateLicense` and `CreateLicenseVersion` carry the same license body, so the
/// rule that decides whether that body is storable lives once, here. Pass `None` for
/// a member the request does not carry.
#[derive(Debug)]
pub struct LicenseInput<'a> {
    pub license_name: &'a str,
    pub product_name: &'a str,
    pub product_sku: Option<&'a str>,
    pub issuer: &'a Issuer,
    pub home_region: &'a str,
    pub validity: &'a DatetimeRange,
    pub beneficiary: Option<&'a str>,
    pub entitlements: &'a [Entitlement],
    pub consumption_configuration: &'a ConsumptionConfiguration,
    pub license_metadata: Option<&'a [Metadata]>,
    pub tags: Option<&'a [Tag]>,
}

/// Why the license body `input` carries is unacceptable, or `None` when every member
/// of it fits the store.
pub fn license_input_rejection(input: &LicenseInput<'_>) -> Option<String> {
    text_rejection("LicenseName", Some(input.license_name))
        .or_else(|| text_rejection("ProductName", Some(input.product_name)))
        .or_else(|| text_rejection("ProductSKU", input.product_sku))
        .or_else(|| text_rejection("Issuer.Name", Some(&input.issuer.name)))
        .or_else(|| text_rejection("Issuer.SignKey", input.issuer.sign_key.as_deref()))
        .or_else(|| text_rejection("Beneficiary", input.beneficiary))
        .or_else(|| region_rejection("HomeRegion", input.home_region))
        .or_else(|| validity_rejection(input.validity))
        .or_else(|| {
            list_rejection(
                "Entitlements",
                input.entitlements.len(),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| input.entitlements.iter().find_map(entitlement_rejection))
        .or_else(|| consumption_configuration_rejection(input.consumption_configuration))
        .or_else(|| {
            list_rejection(
                "LicenseMetadata",
                input.license_metadata.map_or(0, <[Metadata]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            input
                .license_metadata
                .unwrap_or_default()
                .iter()
                .find_map(|entry| {
                    text_rejection("a LicenseMetadata entry's Name", entry.name.as_deref()).or_else(
                        || text_rejection("a LicenseMetadata entry's Value", entry.value.as_deref()),
                    )
                })
        })
        .or_else(|| input.tags.and_then(tag_list_rejection))
}

/// Why a caller-supplied Region is unacceptable, or `None` when it is a Region
/// token.
///
/// The value lands inside a generated ARN, so anything that is not an AWS Region
/// token is refused rather than stored: an ARN can never carry arbitrary caller
/// text. The value itself is not echoed back, because the model bounds it at no
/// length.
fn region_rejection(member: &str, region: &str) -> Option<String> {
    (!is_region_token(region)).then(|| {
        format!(
            "{member} must be an AWS Region name: up to 64 alphanumeric-or-hyphen characters"
        )
    })
}

/// Why one `Entitlement` entry is unacceptable, or `None` when it fits the store.
fn entitlement_rejection(entitlement: &Entitlement) -> Option<String> {
    text_rejection("an Entitlements entry's Name", Some(&entitlement.name))
        .or_else(|| {
            text_rejection(
                "an Entitlements entry's Value",
                entitlement.value.as_deref(),
            )
        })
        .or_else(|| {
            // The catalog is the modelled `EntitlementUnit` enum; this is the
            // handler-side guard, so nothing outside it can reach the store whatever
            // the centralized constraint walk does.
            (!ENTITLEMENT_UNIT_VALUES.contains(&entitlement.unit.as_str())).then(|| {
                format!(
                    "an Entitlements entry's Unit must be one of [{}], got '{}'",
                    ENTITLEMENT_UNIT_VALUES.join(", "),
                    entitlement.unit
                )
            })
        })
        // `MaxCount` is the "maximum entitlement count", so it has no meaning below
        // zero; the model gives it the unconstrained `Long`.
        .or_else(|| negative_rejection("maxCount", entitlement.max_count))
}

/// Why a `ConsumptionConfiguration` is unacceptable, or `None` when it fits.
fn consumption_configuration_rejection(
    configuration: &ConsumptionConfiguration,
) -> Option<String> {
    configuration
        .renew_type
        .as_deref()
        .filter(|renew_type| !RENEW_TYPE_VALUES.contains(renew_type))
        .map(|renew_type| {
            format!(
                "ConsumptionConfiguration.RenewType must be one of [{}], got '{renew_type}'",
                RENEW_TYPE_VALUES.join(", ")
            )
        })
        .or_else(|| {
            configuration
                .provisional_configuration
                .as_ref()
                .and_then(|provisional| {
                    minutes_rejection(
                        "maxTimeToLiveInMinutes",
                        provisional.max_time_to_live_in_minutes,
                    )
                })
        })
        .or_else(|| {
            configuration
                .borrow_configuration
                .as_ref()
                .and_then(|borrow| {
                    minutes_rejection(
                        "maxTimeToLiveInMinutes",
                        borrow.max_time_to_live_in_minutes,
                    )
                })
        })
}

/// Why a time-to-live in minutes is unacceptable, or `None` when it is not
/// negative: a maximum time to live is a duration, so it has no meaning below zero.
fn minutes_rejection(member: &str, minutes: i32) -> Option<String> {
    (minutes < 0).then(|| negative_value_message(member))
}

/// Why a `Validity` range is unacceptable, or `None` when it fits the store.
///
/// A range runs forwards: `End`, when the request carries one, may not precede
/// `Begin`. The ordering is decided on the PARSED instants, so a zone offset is
/// honoured rather than compared as text; a bound the ISO8601 parser does not accept
/// is left alone, because the member's modelled pattern is what admits it.
fn validity_rejection(validity: &DatetimeRange) -> Option<String> {
    text_rejection("Validity.Begin", Some(&validity.begin))
        .or_else(|| text_rejection("Validity.End", validity.end.as_deref()))
        .or_else(|| {
            let begin = parse_instant(&validity.begin)?;
            let end = parse_instant(validity.end.as_deref()?)?;
            (end < begin).then(|| {
                "Validity.End must not be earlier than Validity.Begin".to_string()
            })
        })
}

/// `text` as seconds since the Unix epoch, or `None` when it is not an ISO8601
/// instant this emulator can order.
fn parse_instant(text: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// Why a caller-supplied string is unacceptable, or `None` when it fits the store.
///
/// The model publishes no length for any string on these requests, so this is where
/// the bound lives, measured in UTF-8 bytes — the units the store actually holds.
/// Every operation that stores one applies this one rule.
pub fn text_rejection(member: &str, value: Option<&str>) -> Option<String> {
    let value = value?;
    (value.len() > MAX_TEXT_LEN).then(|| {
        format!(
            "{member} is {} bytes long, more than the {MAX_TEXT_LEN} this service accepts",
            value.len()
        )
    })
}

/// Why a caller-sized list is unacceptable, or `None` when it fits the store.
fn list_rejection(member: &str, len: usize, cap: usize) -> Option<String> {
    (len > cap).then(|| format!("{member} carries {len} entries, more than the {cap} this service accepts"))
}

/// Why a count or an instant is unacceptable, or `None` when it is not negative.
///
/// `LicenseCount` is "the number of licenses managed by the license configuration"
/// and `LicenseExpiry` is an instant, so neither has a meaning below zero. The model
/// gives both the unconstrained `BoxLong`, so this is where the rule lives.
fn negative_rejection(member: &str, value: Option<i64>) -> Option<String> {
    value
        .filter(|value| *value < 0)
        .map(|_| negative_value_message(member))
}

/// How this service states that a member whose value cannot be below zero carried
/// one: fixed text around the member's own wire name, quoted.
///
/// The sentence is the same whichever member it is about, and it echoes NO value:
/// the refusal names the field and the rule, and the submitted number is the
/// caller's own input, which this error class does not carry back.
fn negative_value_message(member: &str) -> String {
    format!("Field '{member}' cannot be lesser than 0.")
}

/// How this service states a value a parameter cannot carry: fixed text around the
/// value the request sent and the member's own wire name.
///
/// Shared by the pre-handler refusal of a value a modelled constraint refuses
/// ([`crate::service::modelled_refusal`]) and by the handlers' own catalog guards,
/// so one unacceptable value is stated the same way wherever it is caught.
pub fn invalid_parameter_value_message(member: &str, value: &str) -> String {
    format!("Invalid parameter value {value} for {member}")
}

/// How this service refuses a filter name it does not scope on: fixed text after
/// the name the request asked for.
///
/// Shared by every list operation that takes `Filters`, so one unsupported filter is
/// refused the same way whichever of them was called.
pub fn unsupported_filter_message(name: &str) -> String {
    format!("{name} is not a supported filter.")
}

/// How this service refuses a parameter whose value it will not accept and will not
/// echo: fixed text around the member's own wire name.
///
/// What it is for: a value that is neither absent nor outside a published range, but
/// one this service alone can judge — a pagination token it did not issue, say.
/// Echoing it back would state nothing a caller can act on.
pub fn invalid_parameter_message(member: &str) -> String {
    format!("Invalid value for {member} parameter.")
}

/// How this service refuses a license count that is enforced but not positive.
///
/// Fully fixed text: the refusal states the rule and names neither the member nor
/// the value.
pub const ENFORCED_LICENSE_COUNT_MESSAGE: &str =
    "License count should be non-null and greater than zero, if license count has \
     been enforced.";

/// The restriction names each counting dimension admits in `LicenseRules`.
///
/// The catalog is the `LicenseRules` member's own documentation, which lists the
/// rules "available by dimension"
/// (`smithy_query(query="doc",shape="CreateLicenseConfigurationRequest",member="LicenseRules")`):
/// one row per modelled `LicenseCountingType` value, naming the restrictions that
/// dimension counts by. A restriction outside its dimension's row is not a rule this
/// dimension has, so the configuration could never be enforced by it.
const LICENSE_RULES_BY_DIMENSION: &[(&str, &[&str])] = &[
    (
        "Core",
        &[
            "allowedTenancy",
            "licenseAffinityToHost",
            "maximumCores",
            "minimumCores",
        ],
    ),
    (
        "Instance",
        &["allowedTenancy", "maximumVcpus", "minimumVcpus"],
    ),
    (
        "Socket",
        &[
            "allowedTenancy",
            "licenseAffinityToHost",
            "maximumSockets",
            "minimumSockets",
        ],
    ),
    (
        "vCPU",
        &[
            "allowedTenancy",
            "honorVcpuOptimization",
            "maximumVcpus",
            "minimumVcpus",
        ],
    ),
];

/// The restriction name one `LicenseRules` entry names.
///
/// The documented syntax is `#name=value`, so the name is what stands between the
/// leading `#` and the `=`; an entry that spells neither is taken whole, so it is
/// reported as the restriction the caller asked for rather than silently accepted.
fn restriction_name(rule: &str) -> &str {
    let named = rule.strip_prefix('#').unwrap_or(rule);
    named.split_once('=').map_or(named, |(name, _)| name)
}

/// Why `rules` are unacceptable for a configuration counting by `counting_type`, or
/// `None` when every one of them is a restriction that dimension admits.
///
/// The refusal names the restriction the request asked for and the dimension that
/// does not admit it, and nothing else.
fn license_rules_rejection(counting_type: &str, rules: &[String]) -> Option<String> {
    let admitted = LICENSE_RULES_BY_DIMENSION
        .iter()
        .find(|(dimension, _)| *dimension == counting_type)
        .map(|(_, admitted)| *admitted)?;
    rules.iter().find_map(|rule| {
        let restriction = restriction_name(rule);
        (!admitted.contains(&restriction)).then(|| {
            format!("Restriction {restriction} not allowed for license type {counting_type}.")
        })
    })
}

/// The resource type a `ProductInformation` entry describes that only ONE counting
/// dimension can be licensed by, and that dimension.
///
/// A relational database is licensed per virtual CPU, so product information about
/// one belongs to a configuration counting that way and to no other.
const RDS_RESOURCE_TYPE: &str = "RDS";

/// See [`RDS_RESOURCE_TYPE`]. One of the modelled `LicenseCountingType` values
/// (`smithy_query(query="shape",shape="LicenseCountingType")`).
const VCPU_COUNTING_TYPE: &str = "vCPU";

/// How this service refuses product information about a resource type the
/// configuration's own counting dimension cannot be licensed by.
///
/// Fully fixed text, as the service words it.
pub const RDS_PRODUCT_INFORMATION_MESSAGE: &str =
    "RDS product information is only supported vCPU license type.";

/// Why `product_information_list` is unacceptable for a configuration counting by
/// `counting_type`, or `None` when every entry describes a resource type that
/// dimension can be licensed by.
fn product_information_dimension_rejection(
    counting_type: &str,
    product_information_list: &[ProductInformation],
) -> Option<String> {
    (counting_type != VCPU_COUNTING_TYPE
        && product_information_list
            .iter()
            .any(|entry| entry.resource_type == RDS_RESOURCE_TYPE))
    .then(|| RDS_PRODUCT_INFORMATION_MESSAGE.to_string())
}

/// Why what the request asks a configuration to carry does not fit the dimension it
/// counts by, or `None` when all of it does.
///
/// The dimension decides two things about the rest of the configuration — which
/// restrictions it can be enforced by, and which resource types it can carry product
/// information about — so both live here, applied to the dimension the configuration
/// HAS: the one the create names, and the stored one on an update, which cannot
/// change it.
pub fn configuration_dimension_rejection(
    counting_type: &str,
    license_rules: Option<&[String]>,
    product_information_list: Option<&[ProductInformation]>,
) -> Option<String> {
    license_rules_rejection(counting_type, license_rules.unwrap_or_default()).or_else(|| {
        product_information_dimension_rejection(
            counting_type,
            product_information_list.unwrap_or_default(),
        )
    })
}

/// Why a configuration's license-count enforcement is unacceptable, or `None` when
/// the two members agree.
///
/// `LicenseCountHardLimit` selects hard enforcement, and there is nothing to enforce
/// without a positive `LicenseCount`: a configuration that enforces a limit it does
/// not have could never block a launch, so the combination is refused rather than
/// stored. The model makes `LicenseCount` optional and correlates the two members
/// nowhere, so the rule is service-side and lives here, applied to the values the
/// configuration would END UP with — the request's own on a create, and the
/// request's merged over the stored record's on an update, which is what the record
/// would carry once the update landed.
pub fn enforced_license_count_rejection(
    license_count_hard_limit: Option<bool>,
    license_count: Option<i64>,
) -> Option<String> {
    (license_count_hard_limit.unwrap_or(false)
        && license_count.is_none_or(|count| count <= 0))
    .then(|| ENFORCED_LICENSE_COUNT_MESSAGE.to_string())
}

/// How this service refuses a license-configuration ARN it resolves to no stored
/// configuration: fixed text plus the ARN exactly as the request supplied it.
///
/// Shared by every operation that reaches a configuration through its ARN, so one
/// unresolvable ARN is refused the same way whichever of them was called.
pub fn invalid_configuration_arn_message(arn: &str) -> String {
    format!("Invalid license configuration ARN {arn}.")
}

/// How this service refuses a resource ARN it resolves to no stored resource:
/// fixed text plus the ARN exactly as the request supplied it.
///
/// The tagging operations are modelled against a generic `ResourceArn` rather than
/// against one resource kind, so their refusal names the resource generically too.
pub fn invalid_resource_arn_message(arn: &str) -> String {
    format!("Invalid resource ARN {arn}.")
}

/// How `UpdateLicenseConfiguration` refuses a configuration it cannot resolve for
/// this owner and status.
///
/// Fully fixed text that deliberately names NO ARN: the update path answers
/// ownership and status together, and says nothing about which configurations the
/// caller's account holds.
pub const CONFIGURATION_OWNERSHIP_MESSAGE: &str =
    "Please check license configurations for ownership and status.";

/// The smallest and largest page a list operation of this service will build, and
/// the page size applied when the request omits `MaxResults`.
///
/// Several `MaxResults` members of this model carry no `@range`, so the accepted
/// range is a service-side rule and lives here, as one bound for every list
/// operation rather than one per handler. The numbers are the model's own: the
/// `MaxSize100` shape the paged operations that DO declare a range target bounds a
/// page at 1 to 100
/// (`smithy_query(query="operation",operation="ListLicenseVersions")`).
pub const MIN_PAGE_SIZE: i32 = 1;

/// See [`MIN_PAGE_SIZE`].
pub const MAX_PAGE_SIZE: i32 = 100;

/// The page size `max_results` asks for, or why it is outside the range this service
/// accepts.
///
/// The refusal states the range and the member it is about, and echoes no value.
/// `member` is the member's wire name, so an operation that pages on a differently
/// named member states that name.
pub fn requested_page_size(member: &str, max_results: Option<i32>) -> Result<usize, String> {
    let Some(requested) = max_results else {
        return Ok(MAX_PAGE_SIZE as usize);
    };
    if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&requested) {
        return Err(format!(
            "Invalid parameter value. Valid range is from {MIN_PAGE_SIZE} to \
             {MAX_PAGE_SIZE} for {member}"
        ));
    }
    Ok(requested as usize)
}

/// The fewest and most entries a caller-supplied selection list — a list of ARNs or
/// ids a request scopes a read to — may carry.
///
/// No such member of this model carries a `@length`, so the bound is service-side
/// and lives here; a request that carries the member at all has to carry a usable
/// selection, so the range starts at one.
pub const MIN_SELECTION_ENTRIES: usize = 1;

/// See [`MIN_SELECTION_ENTRIES`].
pub const MAX_SELECTION_ENTRIES: usize = 100;

/// Why a caller-supplied selection list of `len` entries is unacceptable, or `None`
/// when its size is inside the accepted range.
///
/// `member` is the member's wire name, which the refusal states along with the
/// range; no entry of the list is echoed.
pub fn selection_list_size_rejection(member: &str, len: usize) -> Option<String> {
    (!(MIN_SELECTION_ENTRIES..=MAX_SELECTION_ENTRIES).contains(&len)).then(|| {
        format!(
            "Invalid list size for parameter {member}. Valid range is range \
             {MIN_SELECTION_ENTRIES} to {MAX_SELECTION_ENTRIES}."
        )
    })
}

/// Why the members `CreateLicenseConfiguration` and `UpdateLicenseConfiguration`
/// store are unacceptable, or `None` when every one of them fits the store.
///
/// Both operations accept the same members and persist them into the same record, so
/// the rule lives once here. Pass `None` for a member the request omitted.
pub fn configuration_input_rejection(
    name: Option<&str>,
    description: Option<&str>,
    license_count: Option<i64>,
    license_expiry: Option<i64>,
    license_rules: Option<&[String]>,
    product_information_list: Option<&[ProductInformation]>,
    tags: Option<&[Tag]>,
) -> Option<String> {
    text_rejection("Name", name)
        .or_else(|| text_rejection("Description", description))
        .or_else(|| negative_rejection("licenseCount", license_count))
        .or_else(|| negative_rejection("licenseExpiry", license_expiry))
        .or_else(|| {
            list_rejection(
                "LicenseRules",
                license_rules.map_or(0, <[String]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            license_rules
                .unwrap_or_default()
                .iter()
                .find_map(|rule| text_rejection("a LicenseRules entry", Some(rule)))
        })
        .or_else(|| {
            list_rejection(
                "ProductInformationList",
                product_information_list.map_or(0, <[ProductInformation]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            product_information_list
                .unwrap_or_default()
                .iter()
                .find_map(product_information_rejection)
        })
        .or_else(|| tags.and_then(tag_list_rejection))
}

/// The resource types a `ProductInformation` entry can describe.
///
/// The catalog is the `ResourceType` member's own documentation, which states "The
/// possible values are SSM_MANAGED | RDS"
/// (`smithy_query(query="doc",shape="ProductInformation",member="ResourceType")`).
/// The member targets a plain string rather than a modelled enum, so the catalog is
/// enforced here.
const PRODUCT_RESOURCE_TYPES: &[&str] = &["SSM_MANAGED", "RDS"];

/// How this service refuses a `ProductInformation` entry describing a resource type
/// it does not know.
///
/// Fully fixed text: the refusal names neither the value nor the entry it came from.
pub const INVALID_PRODUCT_RESOURCE_TYPE_MESSAGE: &str =
    "Invalid or non-existent resource type in Product Information.";

/// One product-information resource type: the filters it supports, each with the
/// logical operators it is applied with.
///
/// The catalogs are the `ProductInformationFilterList` member's own documentation,
/// which lists "the following filters and logical operators" supported "when the
/// resource type is SSM_MANAGED" and "when the resource type is RDS", one row per
/// filter
/// (`smithy_query(query="doc",shape="ProductInformation",member="ProductInformationFilterList")`).
/// A filter the entry's resource type does not support cannot be applied to it, and
/// neither can an operator that filter is not applied with, so a configuration could
/// never discover anything by either.
struct ProductInformationSurface {
    /// The `ResourceType` value this row is about.
    resource_type: &'static str,
    /// The supported filters, in the order the model lists them, each with its
    /// logical operators.
    filters: &'static [(&'static str, &'static [&'static str])],
}

/// One row per resource type a `ProductInformation` entry can describe. See
/// [`ProductInformationSurface`].
const PRODUCT_INFORMATION_SURFACES: &[ProductInformationSurface] = &[
    ProductInformationSurface {
        resource_type: "SSM_MANAGED",
        filters: &[
            ("Application Name", &[EQUALS]),
            ("Application Publisher", &[EQUALS]),
            ("Application Version", &[EQUALS]),
            ("Platform Name", &[EQUALS]),
            ("Platform Type", &[EQUALS]),
            (TAG_KEY_FILTER, &[NOT_EQUALS]),
            ("AccountId", &[NOT_EQUALS]),
            ("License Included", &[EQUALS, NOT_EQUALS]),
        ],
    },
    ProductInformationSurface {
        resource_type: RDS_RESOURCE_TYPE,
        filters: &[("Engine Edition", &[EQUALS]), ("License Pack", &[EQUALS])],
    },
];

/// The surface a refusal of a filter's logical operator names.
///
/// It is fixed text of that refusal rather than the entry's own resource type: the
/// service states this one surface whichever resource type the product information
/// describes, and the filter-name refusal above is the one that names the resource
/// type.
const FILTER_COMPARATOR_SURFACE: &str = "SSM Managed Instance";

/// The two logical operators the documented filters are applied with.
const EQUALS: &str = "EQUALS";

/// See [`EQUALS`].
const NOT_EQUALS: &str = "NOT_EQUALS";

/// The one supported filter name the caller completes rather than spells exactly:
/// "the key for your tag must be appended to `Tag:`", so the name a request carries
/// is `Tag:` followed by that key.
const TAG_KEY_FILTER: &str = "Tag:key";

/// The part of [`TAG_KEY_FILTER`] a request's own filter name begins with.
const TAG_KEY_PREFIX: &str = "Tag:";

/// Why one product-information filter is unacceptable for an entry of
/// `resource_type`, or `None` when that resource type supports the filter's name and
/// is filtered with the comparator the request asked for.
///
/// Each refusal states what the request asked for and what the surface does support,
/// so a caller is told what to send instead: the supported filter NAMES for a name
/// the surface has no filter under, and the operators THAT filter is applied with
/// for one it does.
fn product_information_filter_rejection(
    resource_type: &str,
    name: &str,
    comparator: &str,
) -> Option<String> {
    let surface = PRODUCT_INFORMATION_SURFACES
        .iter()
        .find(|row| row.resource_type == resource_type)?;
    let supported = surface.filters.iter().find(|(filter, _)| {
        *filter == name || (*filter == TAG_KEY_FILTER && name.starts_with(TAG_KEY_PREFIX))
    });
    let Some((_, comparators)) = supported else {
        let names: Vec<&str> = surface.filters.iter().map(|(filter, _)| *filter).collect();
        return Some(format!(
            "Unsupported filter name - {name} - provided in product information for \
             {resource_type}. Supported values are [{}].",
            names.join(", ")
        ));
    };
    (!comparators.contains(&comparator)).then(|| {
        let quoted: Vec<String> = comparators
            .iter()
            .map(|comparator| format!("\"{comparator}\""))
            .collect();
        format!(
            "Unsupported filter comparator {comparator} provided for \
             {FILTER_COMPARATOR_SURFACE}. Supported comparators are [{}].",
            quoted.join(", ")
        )
    })
}

/// Why one `ProductInformation` entry is unacceptable, or `None` when it fits.
fn product_information_rejection(product_information: &ProductInformation) -> Option<String> {
    text_rejection(
        "a ProductInformationList entry's ResourceType",
        Some(&product_information.resource_type),
    )
    .or_else(|| {
        (!PRODUCT_RESOURCE_TYPES.contains(&product_information.resource_type.as_str()))
            .then(|| INVALID_PRODUCT_RESOURCE_TYPE_MESSAGE.to_string())
    })
    .or_else(|| {
        list_rejection(
            "a ProductInformationList entry's ProductInformationFilterList",
            product_information.product_information_filter_list.len(),
            MAX_COLLECTION_ENTRIES,
        )
    })
    .or_else(|| {
        product_information
            .product_information_filter_list
            .iter()
            .find_map(|filter| {
                text_rejection(
                    "a ProductInformationFilter's ProductInformationFilterName",
                    Some(&filter.product_information_filter_name),
                )
                .or_else(|| {
                    text_rejection(
                        "a ProductInformationFilter's ProductInformationFilterComparator",
                        Some(&filter.product_information_filter_comparator),
                    )
                })
                .or_else(|| {
                    product_information_filter_rejection(
                        &product_information.resource_type,
                        &filter.product_information_filter_name,
                        &filter.product_information_filter_comparator,
                    )
                })
                .or_else(|| {
                    list_rejection(
                        "a ProductInformationFilter's ProductInformationFilterValue",
                        filter
                            .product_information_filter_value
                            .as_ref()
                            .map_or(0, Vec::len),
                        MAX_COLLECTION_ENTRIES,
                    )
                })
                .or_else(|| {
                    filter
                        .product_information_filter_value
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find_map(|value| {
                            text_rejection(
                                "a ProductInformationFilterValue entry",
                                Some(value),
                            )
                        })
                })
            })
    })
}

/// Why a `TagList` is unacceptable, or `None` when it fits the store.
///
/// Shared by `CreateLicenseConfiguration` and `TagResource`: both store the same
/// list against the same ARN-keyed map.
pub fn tag_list_rejection(tags: &[Tag]) -> Option<String> {
    list_rejection("Tags", tags.len(), MAX_TAGS).or_else(|| {
        tags.iter().find_map(|tag| {
            text_rejection("a tag Key", tag.key.as_deref())
                .or_else(|| text_rejection("a tag Value", tag.value.as_deref()))
        })
    })
}

/// Record that a request changed stored state, as this service's attribution trail.
///
/// One line per accepted mutation, naming the operation, the resource and the
/// caller's own SigV4 access key id. The access key id is an identifier, not a
/// secret, and nothing else off the request is logged — never a credential, a tag
/// value or any other request body member.
pub fn record_mutation(operation: &str, arn: &str, parts: &::http::request::Parts) {
    let akid = ::engine::extract_akid(&parts.headers);
    ::tracing::info!(
        target: "license_manager.audit",
        operation,
        resource = arn,
        access_key_id = %akid,
        "license-manager state mutated"
    );
}

/// The ARN of the license configuration `id` in `region`.
pub fn configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONFIGURATION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// Whether `arn` is spelled like a license-configuration ARN this service mints:
/// one whose resource part is [`CONFIGURATION_ARN_RESOURCE_PREFIX`] followed by a
/// [`CONFIGURATION_ID_PREFIX`] id of [`CONFIGURATION_ID_HEX_LEN`] hex characters.
///
/// It says nothing about whether the store HOLDS one — that is
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::configuration`]'s answer.
/// What it is for: text this service hands a caller and reads back, where only a
/// complete ARN can have come from here.
pub fn is_configuration_arn(arn: &str) -> bool {
    let resource = format!("{CONFIGURATION_ARN_RESOURCE_PREFIX}{CONFIGURATION_ID_PREFIX}");
    arn.starts_with("arn:")
        && arn.split_once(&resource).is_some_and(|(_, id)| {
            id.len() == CONFIGURATION_ID_HEX_LEN && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// The ARN of the license `id` in `region`, which for a license is its home Region:
/// a license is a resource OF the Region the create named as its home.
pub fn license_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LICENSE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The fractional-second digits an instant this service reports carries when it
/// carries any: three, a millisecond. The modelled pattern of `ISO8601DateTime`
/// admits any number of them, so the precision is the service's to choose, and
/// License Manager reports an instant to the millisecond — never to the microseconds
/// or nanoseconds a host clock can also offer, and never to one or two digits.
const RENDERED_SUBSECOND_DIGITS: usize = 3;

/// Nanoseconds per millisecond: what the sub-second part of a clock reading is
/// truncated by before it is rendered.
const NANOS_PER_MILLI: u32 = 1_000_000;

/// Now, as the ISO8601-UTC instant `License.CreateTime` and every other instant this
/// service reports carries.
pub fn now_iso8601() -> String {
    iso8601(SystemTime::now())
}

/// `at` as the ISO8601-UTC instant this service reports: `YYYY-MM-DDThh:mm:ss.sssZ`,
/// or `YYYY-MM-DDThh:mm:ssZ` for an instant that falls on a whole second — the two
/// forms License Manager reports an instant in, both admitted by the member's
/// modelled pattern.
///
/// `Format::DateTime` renders RFC 3339 in UTC, but at whatever sub-second precision
/// the instant handed to it holds — for a host clock, microseconds or finer — and
/// with trailing zeros dropped, which would leave one or two digits. So the whole
/// second is rendered through it and the millisecond field is written here. The
/// millisecond is TRUNCATED, not rounded, so the instant reported is at or before the
/// one the clock read and never names a moment that has not happened yet.
pub fn iso8601(at: SystemTime) -> String {
    let instant = ::aws_smithy_types::DateTime::from(at);
    let millis = instant.subsec_nanos() / NANOS_PER_MILLI;
    let whole_second = ::aws_smithy_types::DateTime::from_secs(instant.secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // That format renders every instant this type can hold; the epoch is the
        // bounded fallback, so no handler has to carry a formatting failure.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    match whole_second.strip_suffix('Z') {
        // An instant on a whole second carries no fractional field at all.
        Some(_) if millis == 0 => whole_second,
        Some(civil) => format!(
            "{civil}.{millis:0width$}Z",
            width = RENDERED_SUBSECOND_DIGITS
        ),
        // Unreachable for a UTC rendering, which that format always suffixes with
        // `Z`; an instant with no `Z` to replace is reported as rendered.
        None => whole_second,
    }
}

/// Whether `region` is an AWS Region token: the only text this service lets into a
/// generated ARN's Region segment.
pub fn is_region_token(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not an AWS region token is refused here and the default is used
/// instead: a generated ARN can never carry arbitrary caller text.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in seconds since the Unix epoch, for ordering a stored instant against the
/// clock: what decides whether a grant's expiration has already passed.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since_epoch| since_epoch.as_secs_f64())
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `<prefix><32 hex>` id from `seed`.
fn id_from_seed(prefix: &str, seed: u64, salt: u64) -> String {
    let mut state = seed;
    let high = splitmix64(&mut state) ^ salt.rotate_right(29);
    let low = splitmix64(&mut state);
    format!("{prefix}{high:016x}{low:016x}")
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerAwslicensemanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerAwslicensemanagerState>)
}
