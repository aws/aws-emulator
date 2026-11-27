// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice service.
//!
//! The deployment-parameter lifecycle is backed here: every value
//! `PutDeploymentParameter`, `TagResource`, `UntagResource` and
//! `ListTagsForResource` report is read out of one of these records, never built
//! from the request that asked for it.
//!
//! A record also carries the EXPIRY the last write named
//! (`expirationDate`), and that date is enforced here: every operation that
//! resolves a deployment parameter first materialises the deletion the clock has
//! made due ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::purge_expired`]),
//! so no caller ever sees a parameter past its expiration date.
//!
//! What a record does NOT carry is the parameter's own text: `secretString` is "the
//! text to encrypt and store in the secret", and the secret is AWS Secrets Manager's
//! — so the value lives there ([`deployment_secret`]) and a record keeps only the
//! identity of the secret holding it. The deletion the expiry makes due therefore has
//! two halves as well: the record is dropped here, and the secret it named is queued
//! for the deleting call Secrets Manager owns
//! ([`deployment_secret::release_expired_secrets`]), which every operation makes as
//! soon as it is no longer holding this store's guard.
//!
//! For the
//! operations that resolve a RESOURCE by the `resourceArn` the CALLER supplied,
//! that whole sequence — judge the ARN as an input, take the store, materialise
//! what the clock has already made due, then resolve the record or answer the
//! service's not-found failure — is this module's [`with_tagged_resource`], so the
//! rule is stated
//! and applied in one place rather than re-derived per handler; the operations
//! whose success WRITES the record it resolves take [`write_tagged_resource`],
//! which is that same sequence plus the one thing a write owes the record it
//! changed: the instant it was written
//! ([`DeploymentParameterRecord::mark_written`]). Two resources of this service
//! carry tags — a deployment parameter and the deployment configuration of a
//! product's delivery option ([`deployment_configuration`]) — and that sequence
//! resolves either, as [`TaggedResource`] sets out.
//!
//! The AGREEMENTS a deployment parameter is written against are held here too
//! ([`ProvisionedAgreement`]). This API models no operation that creates a
//! product or an agreement — in AWS an agreement appears when a buyer accepts a
//! seller's offer, entirely out of band of this service — so an agreement is a
//! PRECONDITION of a deployment-parameter write and never something the write
//! establishes: `PutDeploymentParameter` RESOLVES its target against this store
//! ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::holds_target`])
//! and a target the store does not hold is the modelled
//! `ResourceNotFoundException` ([`target_not_found`]).
//!
//! Because the out-of-band creation has to come from somewhere, a fresh store is
//! already holding the fixture targets [`DEFAULT_AGREEMENTS`] names, in every
//! catalog this service serves — so the operation's documented create path is
//! reachable by an ordinary signed request with no in-process help — and
//! [`seed_agreements`] records further targets for a fixture that wants its own.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::SystemTime;

use regex::Regex;

use crate::ops::types::{
    ConflictException, InternalServerException, ResourceNotFoundException,
    ServiceQuotaExceededException, ValidationException,
};

/// The AWS Secrets Manager secret a deployment parameter IS: the one place its
/// `secretString` is held, and the only place any caller can read it.
pub mod deployment_secret;

/// The other resource this service holds: the deployment configuration of a
/// product's delivery option, and the jobs that publish one.
pub mod deployment_configuration;

/// This service's ARN namespace and sigv4 signing name
/// (`aws.api#service$arnNamespace` / `aws.auth#sigv4$name` are both
/// `aws-marketplace`).
pub const ARN_NAMESPACE: &str = "aws-marketplace";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; `engine::emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The catalogs this service serves.
///
/// MODELLED, not chosen: `PutDeploymentParameterRequest$catalog` documents "The catalog
/// related to the request. Fixed value: `AWSMarketplace`"
/// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`).
/// So the `Catalog` shape's `length 1..64` / `^[a-zA-Z_-]+$` is the SYNTAX a catalog name
/// takes, and this list is the set of catalogs that EXIST: a syntactically valid value
/// outside it names no catalog at all, and the request that carries it is refused as
/// invalid input rather than looked up.
///
/// A list rather than a single constant because the member's constraint is an
/// allowed-value set that AWS can extend; everything below reads the set, so adding an
/// entry here is the whole change.
pub const SUPPORTED_CATALOGS: &[&str] = &["AWSMarketplace"];

/// Whether `candidate` names a catalog this service serves ([`SUPPORTED_CATALOGS`]).
///
/// Compared exactly: `Catalog` carries no `@pattern` casing freedom and AWS's own
/// catalog names are case-sensitive, so a value differing only in case names no catalog.
pub fn is_supported_catalog(candidate: &str) -> bool {
    SUPPORTED_CATALOGS.contains(&candidate)
}

/// The modelled `ValidationException` (HTTP 400) for a `catalog` that names no catalog
/// this service serves.
///
/// The status and the code are what the model fixes; the sentence names the member and
/// lists the values that ARE supported, read off [`SUPPORTED_CATALOGS`] — so it cannot
/// drift from what the service accepts. `fieldName` names the member the way the
/// validation report's own paths spell it (lowerCamel, as the request carries it).
pub fn unsupported_catalog() -> ValidationException {
    let supported = SUPPORTED_CATALOGS
        .iter()
        .map(|catalog| format!("\"{catalog}\""))
        .collect::<Vec<_>>()
        .join(", ");
    ValidationException {
        message: format!(
            "Catalog: The provided Catalog is invalid. Provide a valid Catalog. \
             Supported values are [{supported}]."
        ),
        field_name: "catalog".to_string(),
    }
}

/// The resource portion prefix of a deployment-parameter ARN.
///
/// The ARN's resource segment is the minted `dp-…` id ALONE. It deliberately
/// does not embed `catalog` or `productId`: those are `ResourceId`/`Catalog`
/// values, whose patterns admit `_`, while the modelled `ResourceArn` pattern is
/// `^[a-zA-Z0-9:*/-]+$` — so an ARN carrying them could not satisfy its own
/// shape. The id is unique on its own, which is all the tagging operations need
/// to resolve by.
pub const ARN_RESOURCE_PREFIX: &str = "deployment-parameter/";

/// The input member every tagging operation names its target resource with.
///
/// `ListTagsForResource`, `TagResource` and `UntagResource` all bind
/// `/tags/{resourceArn}` with `resourceArn` as a required `@httpLabel`
/// (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`),
/// and it is the one input whose value this service reads as its own resource ARN.
/// Spelled the way the request carries it (lowerCamel), which is how the modelled
/// `ValidationException$fieldName` names a member.
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// Longest resource ARN this service addresses — the modelled `ResourceArn`
/// `length 1..2048` (`smithy_query(query="shape",shape="ResourceArn")`).
pub const RESOURCE_ARN_MAX_LEN: usize = 2048;

/// The characters a resource ARN of this service may hold: the expression the
/// modelled `ResourceArn` `@pattern` states, WITHOUT the `^` / `$` the model spells
/// it with — for the reason [`TAG_CHARACTER_CLASS`] gives. The model's own form is
/// `^[a-zA-Z0-9:*/-]+$`.
///
/// This is also the expression that ENFORCES the class (the module's
/// `matches_resource_arn_character_class` matches it), so the set of characters a
/// resource ARN may carry is stated here ONCE: what the refusal quotes and what the
/// check admits cannot come to differ.
pub const RESOURCE_ARN_CHARACTER_CLASS: &str = "[a-zA-Z0-9:*/-]+";

/// The number of tags one deployment parameter holds.
///
/// AWS's per-resource tag limit: a taggable resource carries at most this many
/// tags, whichever member a tag arrives through — so, exactly like the key and
/// value length bounds below, it holds for a member targeting the map the model
/// leaves unconstrained too. The model restates it for its constrained copy of the
/// tag map (`PutDeploymentParameterRequest$tags` and
/// `PutDeploymentParameterResponse$tags` are a `TagsMap` with `length max 50`,
/// `smithy_query(query="shape",shape="TagsMap")`).
///
/// It is NOT inferred from what that response member can serialize, and no limit
/// on what this service STORES is: the same stored tag set is also reported through
/// the map the model leaves unconstrained (`ListTagsForResourceResponse$tags`
/// targets `Tags`, `smithy_query(query="shape",shape="Tags,TagsMap")`), so the two
/// members the model gives one resource's tags state different things about what a
/// tag may hold. What a member's own target map constrains is the tag set a REQUEST
/// may carry through that member ([`TagConstraints`]); a reported set is the set
/// the record holds.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Longest tag key this service stores, in CHARACTERS.
///
/// AWS's tagging limit for every taggable resource: a tag key holds at least one
/// and at most this many characters. The modelled `TagKey` `length 1..128`
/// restates it for the constrained copy of the tag map
/// (`smithy_query(query="shape",shape="TagKey")`), so the bound is the same
/// whichever member a tag arrives through.
pub const TAG_KEY_MAX_LEN: usize = 128;

/// Longest tag value this service stores, in CHARACTERS.
///
/// AWS's tagging limit for every taggable resource, restated by the modelled
/// `TagValue` `length 1..256` (`smithy_query(query="shape",shape="TagValue")`).
/// Only the MAXIMUM is a limit on a value: the key is what must carry text, so a
/// tag whose value is empty is an ordinary tag.
pub const TAG_VALUE_MAX_LEN: usize = 256;

/// The characters the model's CONSTRAINED tag map admits: the expression the
/// modelled `TagKey` / `TagValue` `@pattern` states, WITHOUT the `^` / `$` the
/// model spells it with.
///
/// Smithy's `@pattern` is an unanchored find, so those two characters are how the
/// model makes the match total; they are not part of the character class a client
/// may send, and a message that quotes them reports a pattern the caller never
/// sent. The model's own form is `^[a-zA-Z0-9/_+=.:@-]+$`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
///
/// It is NOT a rule about tags in general: it applies exactly where a member
/// targets the shape that carries it ([`TagConstraints::TagsMap`]), and a member
/// targeting a map the model leaves unconstrained admits any UTF-8 text inside the
/// length bounds above.
///
/// As with [`RESOURCE_ARN_CHARACTER_CLASS`], this is also the expression that
/// ENFORCES the class (the module's `matches_tag_character_class` matches it), so the
/// class is
/// stated here once and the refusal quotes exactly what the check admits.
pub const TAG_CHARACTER_CLASS: &str = "[a-zA-Z0-9/_+=.:@-]+";

/// What the map a tagging member TARGETS constrains about the keys and values that
/// member carries — which is what decides the rules a merge through it enforces.
///
/// Two members of this service carry tags and they target different shapes, so the
/// same tag set is legal through one and refused through the other. A caller's
/// request is judged by the shape ITS member names and never by the strictest shape
/// the service happens to model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagConstraints {
    /// The member targets a tag map the model leaves unconstrained
    /// (`map<String,String>`: no `@length` and no `@pattern` on either side). Only
    /// the tagging limits AWS applies to every taggable resource hold — a non-empty
    /// key, [`TAG_KEY_MAX_LEN`] / [`TAG_VALUE_MAX_LEN`] characters and
    /// [`MAX_TAGS_PER_RESOURCE`] entries — so any UTF-8 text inside them is stored,
    /// spaces, letters outside ASCII and an empty value included.
    Unconstrained,
    /// The member targets the modelled `TagsMap`, whose key and value shapes carry a
    /// `@length` and the `@pattern` [`TAG_CHARACTER_CLASS`] states. Those hold on top
    /// of the limits above. The centralized constraint walk reports a map's VALUES
    /// (through the element-constraints wrapper it captures) but not its KEYS, so the
    /// key side of that pattern is judged here and the value side is already refused
    /// before a handler runs.
    TagsMap,
}

/// The tag-key namespace AWS owns.
///
/// Keys beginning with this prefix are generated by AWS itself; a caller may not
/// create, change or remove one, on any resource of any service. It is NOT a
/// character-class rule — the modelled `TagKey` `@pattern`
/// ([`TAG_CHARACTER_CLASS`]) admits `:`, so a key in this namespace satisfies its
/// own shape and the modelled constraint walk cannot refuse it; the reservation is
/// a namespace judgement on top of that, made by [`is_reserved_tag_key`].
///
/// Matched exactly, as the prefix is written: tag keys are case-sensitive, so a key
/// differing in case is a key of the caller's own and not one of AWS's.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `candidate` names a tag key inside the namespace AWS reserves for its
/// own generated tags ([`RESERVED_TAG_KEY_PREFIX`]).
pub fn is_reserved_tag_key(candidate: &str) -> bool {
    candidate.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The modelled `ValidationException` (HTTP 400) for a request that would create,
/// change or remove the reserved-namespace tag key `key`, blamed on the input
/// member `field_name` that carried it.
pub fn reserved_tag_key(key: &str, field_name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "tag key '{key}' is in the reserved '{RESERVED_TAG_KEY_PREFIX}' namespace; \
             a tag key beginning with '{RESERVED_TAG_KEY_PREFIX}' is managed by AWS and \
             cannot be created, changed or removed by a request"
        ),
        field_name: field_name.to_string(),
    }
}

/// Refuse a tagging request that names any reserved-namespace key, before
/// anything is read or written.
///
/// Every operation of this service that names tag keys — the `tags` a
/// deployment-parameter write or a `TagResource` carries, the `tagKeys` an
/// `UntagResource` carries — runs this over its keys: the reservation is a
/// judgement on the INPUT, so a request naming one is refused whether or not the
/// resource it addresses exists and whether or not the tag set would have been
/// applied, and nothing is stored or removed.
///
/// `field_name` is the member that carried the keys, spelled as the request spells
/// it (lowerCamel), which is how the modelled `ValidationException$fieldName`
/// names a member. The keys are judged in sorted order, so the key a refusal
/// quotes does not depend on hash iteration order.
pub fn validate_tag_keys_unreserved<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    field_name: &str,
) -> Result<(), ValidationException> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    reserved.sort_unstable();
    match reserved.first() {
        Some(key) => Err(reserved_tag_key(key, field_name)),
        None => Ok(()),
    }
}

/// The number of deployment parameters this emulator holds at once.
///
/// `PutDeploymentParameter` models `ServiceQuotaExceededException` (HTTP 402)
/// for passing an account maximum. AWS publishes no figure, so the number
/// itself is CHOSEN: its purpose is that the store cannot grow without bound,
/// and that the ceiling is reached through the modelled error rather than by
/// exhausting memory.
pub const MAX_DEPLOYMENT_PARAMETERS: usize = 500;

/// The `ServiceCode` this service reports in its quota error: its own signing
/// name, which is also its ARN namespace.
pub const SERVICE_CODE: &str = "aws-marketplace";

/// How many `@idempotencyToken` entries the replay ledger keeps.
///
/// The ledger grows by one per `PutDeploymentParameter` that carries a fresh
/// `clientToken` — including on the update-in-place path, which mints nothing and
/// so is not covered by [`MAX_DEPLOYMENT_PARAMETERS`] — so it needs a bound of its
/// own: it is capped here with FIFO eviction of the oldest token
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::remember_client_token`]).
/// A token evicted past the cap simply stops replaying and the request is served as
/// an ordinary create-or-update, which is the same answer the caller would get from
/// a token the service never saw.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// How many write targets the target ledger
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::agreements`])
/// holds at once, counted as `(catalog, agreementId, productId)` triples.
///
/// The ledger grows by one per target recorded out of band — the fixture set
/// [`DEFAULT_AGREEMENTS`] a fresh store boots with, plus whatever
/// [`seed_agreements`] hands it — and those identifiers are caller-supplied text,
/// so it needs a ceiling of its own exactly as the `clientToken` ledger does.
///
/// The ceiling is a REFUSAL, not an eviction: at the ceiling
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::provision_target`])
/// a further target records nothing and every target already held stays held. A
/// target is the precondition a write resolves, so dropping one would take a
/// STORED deployment parameter's own target out from under it — the next update of
/// that record would be refused as unresolved — and would let enough seeding
/// displace the set a fresh store boots with.
///
/// The figure is the deployment-parameter ceiling itself, not a second number: one
/// stored parameter names one target, so a ledger this size holds the target of
/// every parameter the store can hold at once.
pub const MAX_PROVISIONED_TARGETS: usize = MAX_DEPLOYMENT_PARAMETERS;

/// The agreements a fresh store already holds, as `(agreementId, productIds)`,
/// recorded in every catalog this service serves ([`SUPPORTED_CATALOGS`]).
///
/// CHOSEN, not modelled — which is why they are fixtures and not AWS values. An
/// agreement exists in AWS because a buyer accepted a seller's offer and a product
/// exists because a seller published it, and this service's model declares no
/// operation that does either, so there is no request that can create the target a
/// `PutDeploymentParameter` resolves. Holding this set from boot is what makes the
/// operation's documented create path reachable over the wire while the write
/// still RESOLVES its target and refuses one the store does not hold.
///
/// Every identifier here satisfies the modelled `ResourceId`
/// (`^[A-Za-z0-9_/-]+$`, `length 1..64`), and the set deliberately covers both
/// shapes a target check has to tell apart: a product covered by two agreements
/// (`prod-default-2`) and an agreement covering one product alone.
pub const DEFAULT_AGREEMENTS: &[(&str, &[&str])] = &[
    ("agmt-default-1", &["prod-default-1", "prod-default-2"]),
    ("agmt-default-2", &["prod-default-2"]),
];

/// A modelled timestamp as whole nanoseconds since the Unix epoch, so a stored
/// wire instant and an instant of the system clock compare at the full precision
/// the wire carries.
fn timestamp_epoch_nanos(at: ::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// An instant of the system clock as whole nanoseconds since the Unix epoch — the
/// comparable form of [`timestamp_epoch_nanos`]. An instant BEFORE the epoch is
/// negative, which is why the error arm is kept rather than saturated to zero: a
/// clock set before 1970 must not read as "the epoch".
fn system_time_epoch_nanos(at: SystemTime) -> i128 {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i128::try_from(since.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// How many expired parameters' secrets the deletion queue
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::expired_secrets`])
/// holds at once.
///
/// An EMULATOR ceiling, not an AWS quota. One purge queues at most one entry per
/// stored parameter, and the call whose purge filled the queue drains it
/// ([`deployment_secret::release_expired_secrets`]), so the queue holds at most the
/// secrets of a store's worth of parameters — which is the figure below, not a
/// second number. At the ceiling a further entry is not queued: the record is gone
/// either way, and a queue that grew without bound would be a leak of its own.
pub const MAX_EXPIRED_SECRETS: usize = MAX_DEPLOYMENT_PARAMETERS;

/// One deployment parameter, as written by `PutDeploymentParameter`.
///
/// The VALUE the parameter carries is not here: `secretString` is "the text to
/// encrypt and store in the secret", and the secret is Secrets Manager's
/// ([`deployment_secret`]), so what this record keeps is the identity of that
/// secret and never a copy of what is in it.
///
/// `Debug` is hand-written so this record prints the same way the rest of the
/// crate's state does, field by field.
#[derive(Clone)]
pub struct DeploymentParameterRecord {
    /// The ARN minted for this record; the key both tagging operations resolve by.
    pub resource_arn: String,
    /// The minted `dp-…` id, matching `^dp-[a-zA-Z0-9]+$` within `len 1..32`.
    pub deployment_parameter_id: String,
    /// The catalog the record was written under.
    pub catalog: String,
    /// The product the record was written for.
    pub product_id: String,
    /// The agreement the record is targeted at.
    pub agreement_id: String,
    /// The parameter name: the identifier deployment parameters are keyed on for
    /// a given buyer and product.
    pub name: String,
    /// The name of the AWS Secrets Manager secret this parameter's text is held
    /// in ([`deployment_secret::deployment_parameter_secret_name`]).
    pub secret_name: String,
    /// The ARN Secrets Manager allocated for that secret, as that service
    /// answered it — the id every later write and the expiry's delete address it
    /// by. A stored reference, not a claim: what the secret HOLDS is read from
    /// Secrets Manager, which owns it.
    pub secret_arn: String,
    /// When the parameter expires and is scheduled for deletion, when the last
    /// write named a date. ENFORCED: at that instant the record is dropped
    /// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::purge_expired`]),
    /// so no operation resolves it afterwards.
    pub expiration_date: Option<::aws_smithy_types::DateTime>,
    /// Tags, keyed so a later `TagResource` with the same key overwrites.
    pub tags: BTreeMap<String, String>,
    /// When the record was first written.
    pub created_at: SystemTime,
    /// When the record was last written.
    pub updated_at: SystemTime,
}

impl std::fmt::Debug for DeploymentParameterRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentParameterRecord")
            .field("resource_arn", &self.resource_arn)
            .field("deployment_parameter_id", &self.deployment_parameter_id)
            .field("catalog", &self.catalog)
            .field("product_id", &self.product_id)
            .field("agreement_id", &self.agreement_id)
            .field("name", &self.name)
            .field("secret_name", &self.secret_name)
            .field("secret_arn", &self.secret_arn)
            .field("expiration_date", &self.expiration_date)
            .field("tags", &self.tags)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl DeploymentParameterRecord {
    /// Whether the expiry this record carries has already come due at `now`.
    ///
    /// `PutDeploymentParameterRequest$expirationDate` is "the date when deployment
    /// parameters expire and are scheduled for deletion"
    /// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="expirationDate")`),
    /// so the stored date is a deadline, not a label: at it the parameter is expired
    /// and no operation resolves it any more
    /// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::purge_expired`]).
    /// A record the last write named no date for never expires.
    ///
    /// The comparison is at the full precision the wire carries (whole nanoseconds
    /// since the epoch), and it is inclusive: the deadline itself is the instant the
    /// expiry is due, so a record whose date is exactly `now` is expired.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.expiration_date
            .is_some_and(|at| timestamp_epoch_nanos(at) <= system_time_epoch_nanos(now))
    }

    /// The key this record is stored under: the identity [`ParameterKey`]
    /// defines, read off the record itself.
    ///
    /// The one place a stored record's identity is spelled out, so a caller that
    /// has to decide whether a record IS the one a request names compares key
    /// values rather than restating which members make up the identity.
    pub fn key(&self) -> ParameterKey {
        (
            self.catalog.clone(),
            self.product_id.clone(),
            self.agreement_id.clone(),
            self.name.clone(),
        )
    }

    /// Stamp `now` as the instant this record's stored content was last written.
    ///
    /// [`Self::updated_at`] is the record's "last written" instant, so EVERY
    /// operation that writes the stored record refreshes it from the clock sample
    /// its own call was served at — the content a write leaves and the instant the
    /// record reports for it are one write, so no stored record can report a
    /// "last written" instant older than its own content. Only a call that wrote
    /// does: a reporting operation, and a write the service refused, leave the
    /// field where the last write left it. Stated here once so the write paths
    /// cannot come to disagree, and applied for the tagging writes by
    /// [`write_tagged_resource`].
    pub fn mark_written(&mut self, now: SystemTime) {
        self.updated_at = now;
    }

    /// The tag map as the modelled `TagsMap` / `Tags` wire shape.
    pub fn tags_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// One AWS Marketplace agreement this emulator holds, and the products it
/// covers — the target `PutDeploymentParameter` writes against.
///
/// `PutDeploymentParameter` "creates or updates a deployment parameter and is
/// targeted by `catalog` and `agreementId`" for the product the request
/// addresses, so every write resolves one of these and is refused when it
/// resolves none. Nothing in THIS model creates an agreement or a product — and
/// no other API is emulated here that could — so a record gets here out of band
/// of any request: from the fixture set [`DEFAULT_AGREEMENTS`] a fresh store
/// boots with, or from [`seed_agreements`].
#[derive(Debug, Clone, Default)]
pub struct ProvisionedAgreement {
    /// The catalog the agreement lives in; a request naming another catalog does
    /// not reach it.
    pub catalog: String,
    /// The agreement's own identifier, as `PutDeploymentParameter$agreementId`
    /// names it.
    pub agreement_id: String,
    /// The products this agreement covers, as
    /// `PutDeploymentParameter$productId` names them.
    pub product_ids: BTreeSet<String>,
}

impl ProvisionedAgreement {
    /// An agreement in `catalog` with no products yet.
    pub fn new(catalog: impl Into<String>, agreement_id: impl Into<String>) -> Self {
        Self {
            catalog: catalog.into(),
            agreement_id: agreement_id.into(),
            product_ids: BTreeSet::new(),
        }
    }

    /// The same agreement, also covering `product_id`.
    #[must_use]
    pub fn with_product(mut self, product_id: impl Into<String>) -> Self {
        self.product_ids.insert(product_id.into());
        self
    }

    /// The key this agreement is stored under: an agreement id is unique inside
    /// its catalog.
    pub fn key(&self) -> AgreementKey {
        (self.catalog.clone(), self.agreement_id.clone())
    }

    /// Whether this agreement covers `product_id`.
    pub fn covers(&self, product_id: &str) -> bool {
        self.product_ids.iter().any(|held| held == product_id)
    }
}

/// How a provisioned agreement is keyed: `(catalog, agreementId)`.
pub type AgreementKey = (String, String);

/// The identity a deployment parameter is keyed on: the catalog, the product,
/// the agreement and the parameter name.
///
/// From the model's own doc for `DeploymentParameterInput$name`: it "is the
/// identifier on which deployment parameters are keyed for a given buyer and
/// product. If this name matches an existing deployment parameter, this request
/// will update the existing resource."
pub type ParameterKey = (String, String, String, String);

/// Per-service state for com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice. A handler reaches it through [`service_state`].
///
/// `Default` is hand-written rather than derived: a fresh store already holds the
/// out-of-band targets [`DEFAULT_AGREEMENTS`] names, which is what a
/// `PutDeploymentParameter` resolves against.
#[derive(Debug)]
pub struct ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// Every stored deployment parameter, keyed by its minted `resourceArn`.
    pub parameters: BTreeMap<String, DeploymentParameterRecord>,
    /// The identity index that makes a repeat `PutDeploymentParameter` update
    /// the existing record in place: [`ParameterKey`] -> `resourceArn`.
    pub by_name: BTreeMap<ParameterKey, String>,
    /// `clientToken` -> `resourceArn`, for `@idempotencyToken` replay. Bounded by
    /// [`MAX_CLIENT_TOKENS`]; write it only through [`Self::remember_client_token`],
    /// which keeps [`Self::client_token_order`] in step.
    pub client_tokens: BTreeMap<String, String>,
    /// The agreements this account holds, keyed by [`AgreementKey`], each with the
    /// products it covers. Created out of band of this API in AWS, and here by the
    /// fixture set [`DEFAULT_AGREEMENTS`] a fresh store boots with or by
    /// [`seed_agreements`] — never by a request, which RESOLVES a target against
    /// this map ([`Self::holds_target`]). Bounded by [`MAX_PROVISIONED_TARGETS`];
    /// write it only through [`Self::provision_target`], which holds that ceiling.
    pub agreements: BTreeMap<AgreementKey, ProvisionedAgreement>,
    /// The `client_tokens` keys in insertion order, so the ledger evicts the oldest
    /// token first once it is at [`MAX_CLIENT_TOKENS`].
    pub client_token_order: VecDeque<String>,
    /// The secrets of the parameters the clock has taken, waiting to be deleted in
    /// Secrets Manager: [`Self::purge_expired`] runs under this guard and cannot
    /// await, so it queues here and the call it is serving drains the queue
    /// ([`deployment_secret::release_expired_secrets`]). Bounded by
    /// [`MAX_EXPIRED_SECRETS`]; write it only through [`Self::queue_expired_secret`].
    pub expired_secrets: VecDeque<String>,
    /// Every deployment configuration job, keyed by its minted `jobId`. Bounded by
    /// [`deployment_configuration::MAX_DEPLOYMENT_CONFIGURATION_JOBS`]; write it only
    /// through [`Self::record_configuration_job`], which holds that ceiling.
    pub configuration_jobs:
        BTreeMap<String, deployment_configuration::DeploymentConfigurationJobRecord>,
    /// The ACTIVE deployment configuration of each resource a job has published one
    /// for, keyed by its `configurationArn` — the key the tagging operations address
    /// it by. Written only by a job the clock has settled as succeeded
    /// ([`Self::settle_due_jobs`]).
    pub configurations: BTreeMap<String, deployment_configuration::DeploymentConfigurationRecord>,
    /// The identity index of those configurations:
    /// [`deployment_configuration::ConfigurationKey`] -> `configurationArn`, so a
    /// later job for the same `(catalog, productId, deliveryOptionType)` publishes
    /// over the configuration that resource already holds rather than beside it.
    pub by_configuration: BTreeMap<deployment_configuration::ConfigurationKey, String>,
    /// Source of the unique job identifiers.
    pub next_job_id: u64,
    /// Source of the unique `dp-…` identifiers.
    pub next_id: u64,
}

impl Default for ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// An empty parameter store already holding the out-of-band targets
    /// [`DEFAULT_AGREEMENTS`] names, in every catalog this service serves.
    ///
    /// Recorded through [`Self::provision_target`], so the fixtures land in the
    /// one bounded ledger every other target goes through.
    fn default() -> Self {
        let mut state = Self {
            parameters: BTreeMap::new(),
            by_name: BTreeMap::new(),
            client_tokens: BTreeMap::new(),
            agreements: BTreeMap::new(),
            client_token_order: VecDeque::new(),
            expired_secrets: VecDeque::new(),
            configuration_jobs: BTreeMap::new(),
            configurations: BTreeMap::new(),
            by_configuration: BTreeMap::new(),
            next_job_id: 0,
            next_id: 0,
        };
        for catalog in SUPPORTED_CATALOGS {
            for (agreement_id, product_ids) in DEFAULT_AGREEMENTS {
                for product_id in *product_ids {
                    state.provision_target(catalog, product_id, agreement_id);
                }
            }
        }
        state
    }
}

impl ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// Mint the next deployment-parameter id.
    ///
    /// `dp-` plus 16 hex digits is 19 characters of `^dp-[a-zA-Z0-9]+$`, inside
    /// the modelled `DeploymentParameterResourceIdentifier` `length 1..32`.
    pub fn mint_id(&mut self) -> String {
        self.next_id += 1;
        format!("dp-{:016x}", self.next_id)
    }

    /// Whether another deployment parameter fits inside
    /// [`MAX_DEPLOYMENT_PARAMETERS`].
    pub fn has_capacity(&self) -> bool {
        self.parameters.len() < MAX_DEPLOYMENT_PARAMETERS
    }

    /// Record that this account holds `agreement_id` in `catalog`, covering
    /// `product_id` — the target a deployment-parameter write is aimed at.
    ///
    /// An agreement exists in AWS because a buyer accepted a seller's offer and a
    /// product exists because a seller published it, both out of band of this
    /// API, which declares no operation that does either. So this is the
    /// OUT-OF-BAND seam, never a step of serving a request: it is called by
    /// [`Default`] for the fixture set [`DEFAULT_AGREEMENTS`] and by
    /// [`seed_agreements`], and a `PutDeploymentParameter` only ever READS what
    /// they recorded ([`Self::holds_target`]).
    ///
    /// Idempotent, and ADDITIVE on an agreement already stored: one agreement can
    /// cover several products, so recording another product under the same
    /// agreement widens the record rather than replacing it, and a repeat of the
    /// same target changes nothing.
    ///
    /// BOUNDED: at [`MAX_PROVISIONED_TARGETS`] recorded targets a further one is
    /// REFUSED — it records nothing — rather than evicting one already held, so no
    /// stored deployment parameter can lose the target it was written against and
    /// no amount of seeding displaces the fixture set. A caller that needs to know
    /// reads the count back ([`Self::recorded_targets`], [`seed_agreements`]).
    pub fn provision_target(&mut self, catalog: &str, product_id: &str, agreement_id: &str) {
        if self.holds_target(catalog, product_id, agreement_id)
            || self.recorded_targets() >= MAX_PROVISIONED_TARGETS
        {
            return;
        }
        self.agreements
            .entry((catalog.to_string(), agreement_id.to_string()))
            .or_insert_with(|| ProvisionedAgreement::new(catalog, agreement_id))
            .product_ids
            .insert(product_id.to_string());
    }

    /// How many targets this account holds: one per product an agreement covers,
    /// which is what [`MAX_PROVISIONED_TARGETS`] bounds.
    pub fn recorded_targets(&self) -> usize {
        self.agreements
            .values()
            .map(|agreement| agreement.product_ids.len())
            .sum()
    }

    /// Whether this account holds `agreement_id` in `catalog` covering
    /// `product_id`: the target a deployment-parameter write resolves, recorded
    /// out of band of this API ([`DEFAULT_AGREEMENTS`], [`seed_agreements`]).
    pub fn holds_target(&self, catalog: &str, product_id: &str, agreement_id: &str) -> bool {
        self.agreements
            .get(&(catalog.to_string(), agreement_id.to_string()))
            .is_some_and(|agreement| agreement.covers(product_id))
    }

    /// Whether this account holds `agreement_id` in `catalog` at all, whatever
    /// products that agreement covers.
    ///
    /// This is the resource a deployment-parameter write is TARGETED by —
    /// `PutDeploymentParameter` "creates or updates a deployment parameter and is
    /// targeted by `catalog` and `agreementId`"
    /// (`smithy_query(query="doc",shape="PutDeploymentParameter")`) — resolved on
    /// its own, independently of the product a particular request addresses.
    pub fn holds_agreement(&self, catalog: &str, agreement_id: &str) -> bool {
        self.agreements
            .contains_key(&(catalog.to_string(), agreement_id.to_string()))
    }

    /// Whether this account holds `product_id` in `catalog` at all — whether ANY
    /// agreement it holds there covers that product.
    ///
    /// This is the resource the request's URI PATH addresses: `catalog` and
    /// `productId` are the operation's two `@httpLabel` members
    /// (`POST /catalogs/{catalog}/products/{productId}/deployment-parameters`,
    /// `smithy_query(query="operation",operation="PutDeploymentParameter")`),
    /// resolved on their own, independently of which agreement a particular
    /// request's body names.
    pub fn holds_product(&self, catalog: &str, product_id: &str) -> bool {
        self.agreements
            .iter()
            .any(|((held_catalog, _), agreement)| {
                held_catalog == catalog && agreement.covers(product_id)
            })
    }

    /// Which segment of the target `(catalog, productId, agreementId)` this
    /// account does not hold, or `None` when the whole target resolves.
    ///
    /// ONE order, for every request: the resource the request's URI PATH
    /// addresses is resolved FIRST and the member its BODY carries second. The
    /// path addresses the product inside the catalog (`catalog` and `productId`
    /// are the two `@httpLabel` members of
    /// `POST /catalogs/{catalog}/products/{productId}/deployment-parameters`)
    /// while `agreementId` is a body member, so `productId` is the segment
    /// reported whenever no agreement this account holds in the catalog covers
    /// that product — the case a request naming neither a held product nor a held
    /// agreement falls in — and `agreementId` only once the product itself
    /// resolves and the agreement the body names is not one that covers it.
    ///
    /// Nothing else about a request bears on the segment reported: the order is
    /// the same whatever else the body carries (tags, an idempotency token, an
    /// expiry).
    ///
    /// The returned name is the member as [`target_not_found`] states it.
    pub fn unresolved_target(
        &self,
        catalog: &str,
        product_id: &str,
        agreement_id: &str,
    ) -> Option<&'static str> {
        if self.holds_target(catalog, product_id, agreement_id) {
            return None;
        }
        Some(if self.holds_product(catalog, product_id) {
            "AgreementId"
        } else {
            "ProductId"
        })
    }

    /// Materialise the expiry the clock has already made due at `now`: drop every
    /// deployment parameter whose `expirationDate` has come, with the identity index
    /// and the idempotency entries that named it.
    ///
    /// The stored date is the one the last write named, "the date when deployment
    /// parameters expire and are scheduled for deletion" — so the deletion is owed at
    /// that instant whether or not anybody called, which is why even a REPORTING
    /// operation may apply it. Every operation that resolves a deployment parameter
    /// calls this first, so no caller sees a parameter the clock has already taken:
    /// the tagging operations answer the service's not-found failure for it
    /// ([`tagged_resource_not_found`]) and a `PutDeploymentParameter` for the same identity
    /// finds no record to update and mints a fresh resource.
    ///
    /// The index is pruned in step with [`Self::parameters`] — the two are written
    /// together — and so is the `clientToken` ledger, whose entries for a dropped
    /// record could otherwise only ever replay a resource that is gone.
    ///
    /// The parameter's VALUE goes with it: a dropped record's secret is queued for
    /// deletion in Secrets Manager ([`Self::queue_expired_secret`]), which the call
    /// whose purge this is carries out as soon as it is not holding this guard
    /// ([`deployment_secret::release_expired_secrets`]) — the deletion is owed at the
    /// same instant, and the text the parameter held must not outlive the parameter.
    pub fn purge_expired(&mut self, now: SystemTime) {
        let expired: BTreeSet<String> = self
            .parameters
            .iter()
            .filter(|(_, record)| record.has_expired(now))
            .map(|(resource_arn, _)| resource_arn.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        let dropped: Vec<String> = expired
            .iter()
            .filter_map(|resource_arn| self.parameters.get(resource_arn))
            .map(|record| record.secret_arn.clone())
            .collect();
        for secret_arn in dropped {
            self.queue_expired_secret(secret_arn);
        }
        self.parameters
            .retain(|resource_arn, _| !expired.contains(resource_arn));
        self.by_name
            .retain(|_, resource_arn| !expired.contains(resource_arn));
        self.client_tokens
            .retain(|_, resource_arn| !expired.contains(resource_arn));
        // Borrowed as a local so the retain over one field can read the other.
        let live_tokens = &self.client_tokens;
        self.client_token_order
            .retain(|token| live_tokens.contains_key(token));
    }

    /// Queue the secret `secret_arn` names for deletion, unless the queue is already
    /// at [`MAX_EXPIRED_SECRETS`] entries — in which case nothing is queued, for the
    /// reason that constant gives. An empty id names no secret and is not queued.
    pub fn queue_expired_secret(&mut self, secret_arn: String) {
        if secret_arn.is_empty() || self.expired_secrets.len() >= MAX_EXPIRED_SECRETS {
            return;
        }
        self.expired_secrets.push_back(secret_arn);
    }

    /// Take every queued secret deletion, leaving the queue empty: the caller is the
    /// one that carries them out, so an entry is handed over once.
    pub fn take_expired_secrets(&mut self) -> Vec<String> {
        self.expired_secrets.drain(..).collect()
    }

    /// Record that `client_token` wrote the parameter at `resource_arn`, evicting the
    /// oldest token once the ledger is at [`MAX_CLIENT_TOKENS`], so repeated writes
    /// cannot grow it without limit.
    pub fn remember_client_token(&mut self, client_token: String, resource_arn: String) {
        if self
            .client_tokens
            .insert(client_token.clone(), resource_arn)
            .is_none()
        {
            self.client_token_order.push_back(client_token);
        }
        while self.client_token_order.len() > MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

/// The ARN of the deployment parameter `id` in `region`.
pub fn deployment_parameter_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{ARN_RESOURCE_PREFIX}{id}"))
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern (`^[a-zA-Z0-9:*/-]+$`) admits only `[a-zA-Z0-9-]` there, so
/// anything else is refused here and the default is used instead: a generated
/// ARN can never carry text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The matcher that admits a value exactly when the WHOLE of it is inside
/// `character_class`.
///
/// Each class this service enforces is stated by ONE constant, as the expression a
/// refusal quotes — the model's `@pattern` without the `^` / `$` the model makes its
/// match total with ([`TAG_CHARACTER_CLASS`] gives the reason) — and this is what
/// turns that one statement into the check, so the class is never written a second
/// time as code that could come to admit a different set. The total match the dropped
/// anchors stood for is restored around the class itself, which is what deciding every
/// character of the value means.
///
/// The expression compiled here comes from a constant of this module and never from a
/// request, so whether it compiles is decided by this file alone and is the same on
/// every call. Each caller compiles its class once and reuses the matcher.
fn whole_value_matcher(character_class: &str) -> Regex {
    Regex::new(&format!(r"\A(?:{character_class})\z"))
        .expect("a character class this module states compiles")
}

/// Whether `candidate` holds only characters [`TAG_CHARACTER_CLASS`] admits.
///
/// Decided by matching [`TAG_CHARACTER_CLASS`] itself ([`whole_value_matcher`]), so
/// the class enforced here is the class the refusal quotes.
fn matches_tag_character_class(candidate: &str) -> bool {
    static COMPILED: OnceLock<Regex> = OnceLock::new();
    COMPILED
        .get_or_init(|| whole_value_matcher(TAG_CHARACTER_CLASS))
        .is_match(candidate)
}

/// The length of `candidate` the way a tag-length limit counts it: in CHARACTERS,
/// not bytes — which is also how Smithy's `@length` measures a string — so a key or
/// value of letters outside ASCII is measured by what a caller wrote rather than by
/// how many bytes UTF-8 took to carry it.
fn tag_text_len(candidate: &str) -> usize {
    candidate.chars().count()
}

/// Whether `candidate` is a tag key this service stores: 1..[`TAG_KEY_MAX_LEN`]
/// characters.
///
/// Any UTF-8 text inside that bound is one. The character class the model's
/// constrained tag map states is a constraint of THAT map and not of tag keys in
/// general ([`TagConstraints`]), so it is no part of this judgement.
pub fn is_tag_key(candidate: &str) -> bool {
    (1..=TAG_KEY_MAX_LEN).contains(&tag_text_len(candidate))
}

/// Whether `candidate` is a tag value this service stores: at most
/// [`TAG_VALUE_MAX_LEN`] characters.
///
/// The EMPTY value is one — a tag's key is what must carry text — and so is any
/// other UTF-8 text inside the bound, for the reason [`is_tag_key`] gives.
pub fn is_tag_value(candidate: &str) -> bool {
    tag_text_len(candidate) <= TAG_VALUE_MAX_LEN
}

/// The modelled `ValidationException` (HTTP 400) for a tag key outside the length
/// bound a tag key may hold ([`is_tag_key`]), blamed on the input member
/// `field_name` that carried it.
///
/// The bound is stated, never the member's own shape: the limit is AWS's tagging
/// limit for every taggable resource and holds whichever member carried the key.
pub fn tag_key_outside_length(key: &str, field_name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "tag key '{key}' is outside the 1..{TAG_KEY_MAX_LEN} characters a tag key may hold"
        ),
        field_name: field_name.to_string(),
    }
}

/// Judge the tag KEYS a request names, before anything is read or written.
///
/// A tag key earns the same judgement whichever member carries it and whatever the
/// request means to do with it: AWS's tagging limits are limits on what a tag key
/// IS, so a string outside them names no tag key the service can hold — nothing to
/// create, nothing to change and nothing to remove. Both halves therefore hold on
/// every input of this service that names tag keys: the namespace AWS reserves
/// ([`validate_tag_keys_unreserved`]) and the length bound [`is_tag_key`] states.
///
/// The members that carry tag keys here are the `tags` a deployment-parameter write
/// or a `TagResource` carries — judged through [`validate_tags_member`], which runs
/// this over the map's keys — and the `tagKeys` an `UntagResource` carries, which is
/// keys alone and runs this directly.
///
/// `field_name` is the member that carried them, spelled as the request spells it
/// (lowerCamel), which is how the modelled `ValidationException$fieldName` names a
/// member. The keys are judged in sorted order and the reservation ahead of the
/// bound, so the key a refusal quotes depends neither on hash iteration order nor on
/// which key the caller happened to write first.
pub fn validate_tag_keys_member<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    field_name: &str,
) -> Result<(), ValidationException> {
    let mut ordered: Vec<&str> = keys.into_iter().collect();
    ordered.sort_unstable();
    validate_tag_keys_unreserved(ordered.iter().copied(), field_name)?;
    match ordered.into_iter().find(|key| !is_tag_key(key)) {
        Some(key) => Err(tag_key_outside_length(key, field_name)),
        None => Ok(()),
    }
}

/// What a tag set a request carries can be refused by — two failures with different
/// wire identities, kept apart here so no layer of this service can come to answer
/// one with the other's.
///
/// A tag outside a bound on what a tag IS — the namespace AWS reserves, the length a
/// key or a value may hold, the character class the map a member targets states — is
/// a violation of this service's OWN modelled input, so it stays the modelled
/// `ValidationException`, which names the member it is about. The NUMBER of tags one
/// resource may hold is not this service's rule: it is AWS's per-resource tagging
/// limit, enforced by the tagging front end that runs ahead of this service's
/// modelled input validation, and its recorded answer carries the generic
/// `BadRequestException` identity ([`crate::scaffolded::BAD_REQUEST_CODE`]) with one
/// fixed sentence ([`too_many_tags`]). The difference is client-visible on restJson1,
/// where the error SHAPE is resolved from the wire code: a caller's
/// `catch ValidationException` must not fire for a tag set refused for its size, and
/// a caller matching the front end's identity must see it.
#[derive(Debug)]
pub enum TagRefusal {
    /// A violation of this service's own modelled input, blamed on the member that
    /// carried it.
    Invalid(ValidationException),
    /// A tag set past the number of tags one resource holds.
    TooManyTags(crate::scaffolded::BadRequest),
}

impl TagRefusal {
    /// This refusal in the calling operation's error channel.
    ///
    /// `modelled` is that operation's own `ValidationException` variant, so every
    /// operation answers the modelled shape it declares, while the count refusal
    /// keeps the front end's identity — which the model does not declare, so it
    /// cannot travel as a modelled variant at all.
    pub fn into_error<E>(
        self,
        modelled: impl FnOnce(ValidationException) -> E,
    ) -> crate::scaffolded::Scaffolded<E> {
        match self {
            Self::Invalid(why) => crate::scaffolded::Scaffolded::Modeled(modelled(why)),
            Self::TooManyTags(refused) => crate::scaffolded::Scaffolded::BadRequest(refused),
        }
    }
}

/// Lets `?` lift this service's modelled input refusals into [`TagRefusal`], so the
/// judgements that are modelled stay stated as the modelled shape.
impl From<ValidationException> for TagRefusal {
    fn from(why: ValidationException) -> Self {
        Self::Invalid(why)
    }
}

/// The tagging front end's refusal of a tag set past [`MAX_TAGS_PER_RESOURCE`]:
/// HTTP 400 with the code [`crate::scaffolded::BAD_REQUEST_CODE`] and AWS's own fixed
/// sentence for that limit.
///
/// The sentence carries NOTHING from the request — neither the number of tags it sent
/// nor the member that carried them: the one number in it is the per-resource maximum
/// itself, read off [`MAX_TAGS_PER_RESOURCE`] so the text cannot drift from the limit
/// the service keeps. It is the same sentence whichever side of the limit was crossed
/// — a set already past it on its own, or one only past it once merged onto what a
/// record already holds — because what the limit is about is the RESULTANT tag set.
pub fn too_many_tags() -> crate::scaffolded::BadRequest {
    crate::scaffolded::BadRequest::new(format!(
        "The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags."
    ))
}

/// Judge the tag map a request CARRIES, before anything is read or written.
///
/// This is a judgement on the INPUT alone: every entry of `incoming` is held to
/// AWS's own tagging limits — its key to the judgement every tag key of this
/// service earns ([`validate_tag_keys_member`]: no reserved namespace and the
/// [`is_tag_key`] bound) and its value to the bound [`is_tag_value`]
/// states — the map is held to the part of
/// [`MAX_TAGS_PER_RESOURCE`] the request alone decides, and a map arriving through a
/// member that targets the modelled constrained map is additionally held to the
/// character class that map's key shape states ([`TagConstraints`]).
///
/// Because it judges only the input, a request carrying a tag the member does not
/// admit is refused the SAME way whichever path the operation would then take, and
/// whether or not the tags would have been applied at all: an operation runs this
/// over its `tags` ahead of resolving anything, and nothing is stored either way.
/// [`merge_tags`] runs it too, so no merge can store an entry this would refuse.
///
/// `incoming` is walked in key order, so the key a refusal names does not depend on
/// hash iteration order.
pub fn validate_tags_member(
    incoming: &HashMap<String, String>,
    constraints: TagConstraints,
) -> Result<(), TagRefusal> {
    validate_tag_keys_member(incoming.keys().map(String::as_str), "tags")?;
    let ordered: BTreeMap<&String, &String> = incoming.iter().collect();
    for (key, value) in ordered {
        if !is_tag_value(value) {
            return Err(ValidationException {
                message: format!(
                    "the value of tag key '{key}' is longer than the {TAG_VALUE_MAX_LEN} characters a tag value may hold"
                ),
                field_name: "tags".to_string(),
            }
            .into());
        }
        // The key side of the constrained map's own `@pattern`. Judged here because
        // the centralized constraint walk reports a map's values and not its keys;
        // the value side has therefore already been refused before this runs.
        if constraints == TagConstraints::TagsMap && !matches_tag_character_class(key) {
            return Err(ValidationException {
                message: format!(
                    "tag key '{key}' is outside the {TAG_CHARACTER_CLASS} characters this member's tag keys may hold"
                ),
                field_name: "tags".to_string(),
            }
            .into());
        }
    }
    // The part of the per-resource cap the REQUEST alone decides. Applying a tag set
    // can only leave a resource holding at least as many tags as the set itself, so a
    // map carrying more entries than one resource may hold could not be stored on any
    // resource in any state: it is an over-sized input, not a full resource, and so is
    // refused here with everything else the request decides on its own. The cap on the
    // RESULT of merging onto what a record already holds needs the record, so it stays
    // where the record is known ([`merge_tags`]) — and both answer the one refusal
    // [`too_many_tags`] states, which is the front end's and not this service's
    // modelled input error ([`TagRefusal`]).
    //
    // It is judged AFTER the walk above deliberately: a request that breaks a bound of
    // one entry AND carries too many entries is told about the entry, which is the
    // narrower fault and the one a caller can see in what it sent. The precedence is
    // the same whatever the store holds, so it is no part of the rule above.
    if incoming.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRefusal::TooManyTags(too_many_tags()));
    }
    Ok(())
}

/// Merge `incoming` onto `existing`, or say why the result could not be stored.
///
/// A key already present takes the new value. The merge is computed BEFORE
/// anything is stored, so a refused request leaves the record exactly as it was.
///
/// Every entry of `incoming` is judged first ([`validate_tags_member`], which the
/// operations also run on their own input ahead of resolving anything), so no merge
/// can leave on a record an entry that judgement refuses — a reserved-namespace key,
/// a key or value outside its length bound, a map of more entries than one resource
/// may hold, or a key outside the character class the member's own map shape states.
/// What this adds is the one limit that needs the record rather than the request:
/// whether merging onto the tags the record ALREADY holds would pass
/// [`MAX_TAGS_PER_RESOURCE`] — refused as the front end's own count refusal
/// ([`too_many_tags`], [`TagRefusal`]) and not as this service's modelled input
/// error, exactly as the request-only half of that same limit is.
pub fn merge_tags(
    existing: &BTreeMap<String, String>,
    incoming: &HashMap<String, String>,
    constraints: TagConstraints,
) -> Result<BTreeMap<String, String>, TagRefusal> {
    validate_tags_member(incoming, constraints)?;
    let mut merged = existing.clone();
    let ordered: BTreeMap<&String, &String> = incoming.iter().collect();
    for (key, value) in ordered {
        merged.insert(key.clone(), value.clone());
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRefusal::TooManyTags(too_many_tags()));
    }
    Ok(merged)
}

/// Whether `candidate` holds only characters [`RESOURCE_ARN_CHARACTER_CLASS`] admits.
///
/// Decided by matching [`RESOURCE_ARN_CHARACTER_CLASS`] itself
/// ([`whole_value_matcher`]), so the class enforced here is the class the refusal
/// quotes.
fn matches_resource_arn_character_class(candidate: &str) -> bool {
    static COMPILED: OnceLock<Regex> = OnceLock::new();
    COMPILED
        .get_or_init(|| whole_value_matcher(RESOURCE_ARN_CHARACTER_CLASS))
        .is_match(candidate)
}

/// Read `candidate` as a resource ARN of THIS service, or say why it is not one.
///
/// A caller-supplied resource ARN is judged on the INPUT, before any resource is
/// looked up: an ARN of another vendor, or a value that is not a resource ARN at
/// all, names nothing in this service's namespace however the store happens to be
/// populated, so it is a bad REQUEST (HTTP 400) — never the not-found a missing
/// resource gets, which would tell a caller with a permanent input bug to retry or
/// to create.
///
/// THE CODE that refusal carries is [`crate::scaffolded::BAD_REQUEST_CODE`], not the
/// modelled `ValidationException`: reading a value as an ARN is the tagging
/// front-end's own parse, which judges whether the API can address the named resource
/// at all, ahead of this service's modelled input validation — and the recorded answer
/// to a tagging request whose `resourceArn` is not one of this service's carries that
/// generic identity rather than a shape this model declares. A violation of this
/// service's OWN modelled input (an unsupported `catalog`, a tag outside `TagsMap`)
/// stays the modelled `ValidationException`, which names the member in `fieldName`.
/// Neither refusal echoes the ARN the caller sent.
///
/// Note that the tagging operations bind `resourceArn` to a plain `String`
/// (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`),
/// so NO modelled constraint refuses any value here ahead of this check: this is the
/// whole judgement of the member.
///
/// Two conditions, in the order the service applies them:
///
/// 1. the value is inside the SYNTAX this service's own resource ARNs take — the
///    modelled `ResourceArn` `length 1..2048` and [`RESOURCE_ARN_CHARACTER_CLASS`];
/// 2. it parses as a resource ARN whose vendor field is this service's
///    [`ARN_NAMESPACE`] — the one ARN parse, [`::service_common::arn`], refused
///    with the one shared sentence,
///    [`::service_common::validation_message::expected_vendor`], so this service
///    and every other that refuses a foreign ARN word it identically.
///
/// Everything that passes both is a resource ARN this API can address, and whether
/// it names a stored record is then the store's question
/// ([`tagged_resource_not_found`]).
pub fn validate_resource_arn(candidate: &str) -> Result<(), crate::scaffolded::BadRequest> {
    if !(1..=RESOURCE_ARN_MAX_LEN).contains(&candidate.len())
        || !matches_resource_arn_character_class(candidate)
    {
        return Err(crate::scaffolded::BadRequest::new(format!(
            "Invalid input resource arn: {RESOURCE_ARN_MEMBER} must be \
             1..{RESOURCE_ARN_MAX_LEN} characters of {RESOURCE_ARN_CHARACTER_CLASS}"
        )));
    }
    if !::service_common::arn::is_resource_arn_of(candidate, ARN_NAMESPACE) {
        return Err(crate::scaffolded::BadRequest::new(
            ::service_common::validation_message::expected_vendor(ARN_NAMESPACE),
        ));
    }
    Ok(())
}

/// The not-found failure for an ARN that names no stored resource of this service
/// — neither a deployment parameter nor a deployment configuration:
/// HTTP 404 with the error code `NotFoundException`.
///
/// NOT the modelled `ResourceNotFoundException`, whose shape name would reach the
/// wire as the error code: this service's recorded responses answer a resource ARN
/// it holds no record for with the code `NotFoundException`
/// ([`crate::scaffolded::NOT_FOUND_CODE`]) and the constant text
/// [`crate::scaffolded::NOT_FOUND_TEXT`], at the same 404 the modelled shape
/// carries, and a client branches on the code. It names no ARN because the service
/// names none. Generic over the operation's modelled error enum, so every operation
/// that resolves a caller-supplied resource ARN answers the one failure.
pub fn tagged_resource_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found()
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for a write whose target
/// this account does not hold, blamed on `member` — the segment of
/// `(catalog, productId, agreementId)` that did not resolve, as
/// [`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::unresolved_target`]
/// picks it.
///
/// The status and the code are what the model fixes (`ResourceNotFoundException`,
/// `httpError 404`); the sentence names the unresolved member, which is the one
/// thing about it the request determines.
pub fn target_not_found(member: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{member}: Request references a resource which does not exist."),
    }
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a write that
/// would pass [`MAX_DEPLOYMENT_PARAMETERS`].
pub fn parameter_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "this account already holds {held} deployment parameters, the maximum for this quota"
        ),
    }
}

/// The modelled `ConflictException` (HTTP 409) for a `clientToken` already used
/// to write a DIFFERENT deployment parameter.
///
/// `clientToken` is the request's `@idempotencyToken` — "a unique identifier for
/// the new version" — so one token identifies one write; reusing it for another
/// parameter is a conflicting request, not a replay. `resourceId` names the
/// parameter the token is already bound to, which satisfies the modelled
/// `ResourceId` pattern `^[A-Za-z0-9_/-]+$` by construction.
pub fn client_token_conflict(
    client_token: &str,
    bound_parameter_id: &str,
) -> ConflictException {
    ConflictException {
        message: format!(
            "clientToken '{client_token}' was already used to write deployment parameter \
             {bound_parameter_id}; a token identifies exactly one write"
        ),
        resource_id: bound_parameter_id.to_string(),
    }
}

/// The modelled `ConflictException` (HTTP 409) for a write that another write of the
/// same identity kept taking out from under it.
///
/// A deployment-parameter write has two parts — the secret the value goes into, which
/// belongs to Secrets Manager, and this service's own record — so it cannot hold this
/// store's guard across the whole of itself. It therefore decides what to write, writes
/// the secret, and then commits; a commit the store no longer admits is retried against
/// what the store holds now, and this is what is answered when even that is overtaken.
/// `resourceId` names the product the identity belongs to, which satisfies the modelled
/// `ResourceId` pattern by construction (it is the one the request addressed).
pub fn concurrent_write_conflict(product_id: &str, name: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "the deployment parameter {name} is being written concurrently for product \
             {product_id}; retry the request"
        ),
        resource_id: product_id.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState>)
}

/// The modelled `InternalServerException` (HTTP 500) for a request that could not
/// reach this service's store at all ([`service_state`] answering `None`).
///
/// ONE sentence for every operation: the condition is the same one wherever it is
/// met — the engine a handler holds is not the shared store — so the answer carries
/// nothing from the request that happened to meet it.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the deployment-parameter store is unavailable".to_string(),
    }
}

/// Resolve the deployment parameter a caller-supplied `resource_arn` names, and run
/// `resolved` on it — the whole sequence an operation that addresses a parameter BY
/// ARN runs, in one place.
///
/// The sequence, in the order the service applies it:
///
/// 1. judge the ARN as an INPUT ([`validate_resource_arn`]): a value that is not a
///    resource ARN of this service's vendor names nothing this API can address, so
///    it is the front-end's bad request and the store is never consulted for it;
/// 2. the rest of the operation's request-only judgement (`judge_request`), still
///    ahead of the store, so a refusal the request earns on its own content does not
///    depend on what the store holds;
/// 3. take this service's store ([`service_state`]), or answer the modelled server
///    error ([`store_unavailable`]) through the operation's own variant —
///    `store_unavailable_as` is that variant's constructor;
/// 4. materialise the deletion the clock has already made due, from ONE clock sample
///    taken under the lock
///    ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::purge_expired`]),
///    so a parameter past its `expirationDate` is gone before the ARN is resolved;
/// 5. resolve the record, or answer the service's not-found failure
///    ([`tagged_resource_not_found`]).
///
/// `resolved` therefore sees a record that EXISTS, still inside the store lock, and
/// what it answers is what the operation answers. Generic over the operation's
/// modelled error enum and over what the operation reports, so one implementation of
/// the sequence serves a reporting operation and a mutating one alike — and a change
/// to any step is a change to all of them.
///
/// `judge_request` and `resolved` answer in the operation's whole error CHANNEL
/// rather than in its modelled error enum alone, because not every refusal a tagging
/// request earns is a shape this model declares: the tagging front end's own
/// refusals — an unaddressable `resourceArn`, a tag set past the per-resource count
/// ([`too_many_tags`]) — carry their own identity, and funnelling them through the
/// modelled enum would restate them as this service's `ValidationException`.
///
/// This is the REPORTING entry point: it leaves the record's "last written" instant
/// alone, because reporting a record is not writing it. An operation whose success
/// writes the record takes [`write_tagged_resource`], which runs this same sequence
/// and stamps that instant.
pub fn with_tagged_resource<Eng, E, T>(
    engine: &Eng,
    resource_arn: &str,
    store_unavailable_as: impl FnOnce(InternalServerException) -> E,
    judge_request: impl FnOnce() -> Result<(), crate::scaffolded::Scaffolded<E>>,
    resolved: impl FnOnce(&mut TaggedResource<'_>) -> Result<T, crate::scaffolded::Scaffolded<E>>,
) -> Result<T, crate::scaffolded::Scaffolded<E>>
where
    Eng: Send + Sync + 'static,
{
    resolve_tagged_resource(
        engine,
        resource_arn,
        store_unavailable_as,
        judge_request,
        |record, _now| resolved(record),
    )
}

/// [`with_tagged_resource`] for an operation whose success WRITES the record it
/// resolved: `write` sees the resolved record and, when it answers success, the
/// record's "last written" instant is refreshed from the sequence's own clock
/// sample ([`TaggedResource::mark_written`]).
///
/// The stamp is applied HERE rather than in each handler so the two tagging writes
/// cannot drift from one another or from the write `PutDeploymentParameter`
/// performs: a mutating operation that goes through this sequence refreshes the
/// field by going through it. It is applied only on SUCCESS and only from this
/// entry point, so a refused write leaves the record exactly as it found it and a
/// reporting operation (which takes [`with_tagged_resource`]) never moves the
/// field at all.
///
/// The instant is the sample the sequence already took under the lock for the
/// expiry it materialises, so the whole call — the deletion the clock made due and
/// the write this record records — is served at ONE instant.
pub fn write_tagged_resource<Eng, E, T>(
    engine: &Eng,
    resource_arn: &str,
    store_unavailable_as: impl FnOnce(InternalServerException) -> E,
    judge_request: impl FnOnce() -> Result<(), crate::scaffolded::Scaffolded<E>>,
    write: impl FnOnce(&mut TaggedResource<'_>) -> Result<T, crate::scaffolded::Scaffolded<E>>,
) -> Result<T, crate::scaffolded::Scaffolded<E>>
where
    Eng: Send + Sync + 'static,
{
    resolve_tagged_resource(
        engine,
        resource_arn,
        store_unavailable_as,
        judge_request,
        |record, now| {
            let answered = write(record)?;
            record.mark_written(now);
            Ok(answered)
        },
    )
}

/// One resource of this service a caller addresses BY ARN, as the tagging
/// operations see it.
///
/// Two resources of this service carry tags — a deployment parameter and the
/// deployment configuration of a product's delivery option — and `TagResource`,
/// `UntagResource` and `ListTagsForResource` name their target by `resourceArn`
/// alone, so which one an ARN names is the STORE's question and not the
/// operation's: each of those operations states one rule about the tag set, over
/// whichever of the two the ARN resolves to. This is that view — the tags of the
/// resolved record, and the one thing a write owes the record it changed — so the
/// rule is written once rather than once per resource.
pub enum TaggedResource<'a> {
    /// The ARN named a stored deployment parameter.
    DeploymentParameter(&'a mut DeploymentParameterRecord),
    /// The ARN named the active deployment configuration of a resource.
    DeploymentConfiguration(&'a mut deployment_configuration::DeploymentConfigurationRecord),
}

impl TaggedResource<'_> {
    /// The tags the resolved record holds.
    pub fn tags(&self) -> &BTreeMap<String, String> {
        match self {
            Self::DeploymentParameter(record) => &record.tags,
            Self::DeploymentConfiguration(record) => &record.tags,
        }
    }

    /// The tags the resolved record holds, to write.
    pub fn tags_mut(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Self::DeploymentParameter(record) => &mut record.tags,
            Self::DeploymentConfiguration(record) => &mut record.tags,
        }
    }

    /// Those tags as the modelled `TagsMap` / `Tags` wire shape.
    pub fn tags_map(&self) -> HashMap<String, String> {
        match self {
            Self::DeploymentParameter(record) => record.tags_map(),
            Self::DeploymentConfiguration(record) => record.tags_map(),
        }
    }

    /// Stamp `now` as the instant this record's stored content was last written —
    /// the one thing every write through this sequence owes the record it changed,
    /// whichever resource it is.
    fn mark_written(&mut self, now: SystemTime) {
        match self {
            Self::DeploymentParameter(record) => record.mark_written(now),
            Self::DeploymentConfiguration(record) => record.updated_at = now,
        }
    }
}

/// The resolution sequence itself, which [`with_tagged_resource`] and
/// [`write_tagged_resource`] are the reporting and the writing view of: `resolved`
/// additionally sees the instant the sequence sampled under the lock, which is what
/// lets a write stamp the record it changed with the instant its own call was
/// served at.
fn resolve_tagged_resource<Eng, E, T>(
    engine: &Eng,
    resource_arn: &str,
    store_unavailable_as: impl FnOnce(InternalServerException) -> E,
    judge_request: impl FnOnce() -> Result<(), crate::scaffolded::Scaffolded<E>>,
    resolved: impl FnOnce(
        &mut TaggedResource<'_>,
        SystemTime,
    ) -> Result<T, crate::scaffolded::Scaffolded<E>>,
) -> Result<T, crate::scaffolded::Scaffolded<E>>
where
    Eng: Send + Sync + 'static,
{
    validate_resource_arn(resource_arn)?;
    judge_request()?;
    let Some(state) = service_state(engine) else {
        return Err(crate::scaffolded::Scaffolded::Modeled(store_unavailable_as(
            store_unavailable(),
        )));
    };
    // The sample is taken AFTER the lock, not before waiting for it: the instant a
    // call is served at is the instant it reaches the store, so a call that waited
    // behind another one judges the expiry, and stamps the record, with the clock as
    // it stands once it holds the store — never with a reading from before the wait.
    let mut guard = state.lock();
    let now = SystemTime::now();
    guard.purge_expired(now);
    // The other transition the clock owes at this instant: a job whose terminal
    // status is due publishes its configuration, so an ARN a succeeded job gave a
    // configuration resolves here as soon as that status was owed.
    guard.settle_due_jobs(now);
    // Reborrowed once, so the two lookups below borrow DISJOINT fields of the store
    // rather than the guard twice.
    let store = &mut *guard;
    let mut record = if let Some(parameter) = store.parameters.get_mut(resource_arn) {
        TaggedResource::DeploymentParameter(parameter)
    } else if let Some(configuration) = store.configurations.get_mut(resource_arn) {
        TaggedResource::DeploymentConfiguration(configuration)
    } else {
        return Err(tagged_resource_not_found());
    };
    resolved(&mut record, now)
}

/// Record the agreements this account holds ahead of any request.
///
/// This is the out-of-band creation seam: an AWS Marketplace agreement exists
/// when a buyer accepts a seller's offer and a product exists when a seller
/// publishes it, and THIS API models no operation that does either. A fixture
/// that wants a particular target in the store beyond the set a fresh store
/// already holds ([`DEFAULT_AGREEMENTS`]) hands the records to the emulator here;
/// a write against a target the store does not hold is refused with the modelled
/// `ResourceNotFoundException` ([`target_not_found`]) rather than recording one,
/// so what this seam holds IS the precondition of a write.
/// A record for a target already stored changes nothing, so seeding the same
/// agreement twice holds one agreement, not two. Every seeded target goes through
/// the same bounded ledger the fixture set uses
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::provision_target`]),
/// so this seam cannot grow the store past [`MAX_PROVISIONED_TARGETS`] either. A
/// record naming no product covers nothing and is therefore no target: it records
/// nothing.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so
/// there is no store to seed), otherwise the number of agreements the store holds
/// afterwards — which includes the fixture set a fresh store boots with
/// ([`DEFAULT_AGREEMENTS`]), so it is the store's total and not a count of what
/// this call recorded.
pub fn seed_agreements<Eng, I>(engine: &Eng, records: I) -> Option<usize>
where
    Eng: Send + Sync + 'static,
    I: IntoIterator<Item = ProvisionedAgreement>,
{
    let state = service_state(engine)?;
    let mut store = state.lock();
    for record in records {
        for product_id in &record.product_ids {
            store.provision_target(&record.catalog, product_id, &record.agreement_id);
        }
    }
    Some(store.agreements.len())
}
