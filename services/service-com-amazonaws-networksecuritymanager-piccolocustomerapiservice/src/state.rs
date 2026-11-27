// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networksecuritymanager-piccolocustomerapiservice service.
//!
//! The Policy lifecycle (`CreatePolicy`, `GetPolicy`, `ListPolicies`, `UpdatePolicy`,
//! `DeletePolicy`) is backed by [`ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState::policies`],
//! the single source of truth those five handlers read and write. The record, the
//! id/token minting, the `policyIdentifier` resolution and the `nextToken` codec live
//! here rather than in any one `src/ops/*.rs`, so all five operations agree on one copy
//! of each.
//!
//! One entity per module: the Rule record and its lookups live in [`rules`], the
//! Deployment record in [`deployments`], the Template record in [`templates`] and the
//! Scope record in [`scopes`], re-exported here so `crate::state::<name>` keeps naming
//! every one of them. The references those records hold between one another — the graph a
//! delete consults before removing anything — live in [`references`], a policy's
//! published version and the snapshots taken of it in [`policy_snapshots`], and a rule's
//! in [`rule_snapshots`]. The coverage those records imply — which protected resource each
//! entity covers, and through which deployment — is derived in [`coverage`].

mod coverage;
mod deployments;
mod policy_snapshots;
mod references;
mod rule_snapshots;
mod rules;
mod scopes;
mod templates;

pub use coverage::*;
pub use deployments::*;
pub use policy_snapshots::*;
pub use references::*;
pub use rule_snapshots::*;
pub use rules::*;
pub use scopes::*;
pub use templates::*;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AssociatedTemplateOrRule, PolicyConfiguration, PolicySummary, ResourceNotFoundException,
    TemplateOrRuleReference, ValidationExceptionField,
};

/// This service's ARN namespace (`awsService.arnNamespace` / `aws.auth#sigv4` name in
/// the model), i.e. the service segment of every ARN it mints.
pub const ARN_NAMESPACE: &str = "network-security-manager";

/// The region an ARN carries when the request has no SigV4 credential scope to read one
/// from (an unsigned call). Matches the region the emulator's own shared config names.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EntityStatus` for a published policy — the model: "DRAFT (unpublished, editable) or
/// ACTIVE (published, in use)".
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// `EntityStatus` for an unpublished policy. See [`STATUS_ACTIVE`].
pub const STATUS_DRAFT: &str = "DRAFT";

/// `PolicyFirewallType` value whose policies carry no template/rule associations.
pub const FIREWALL_TYPE_SHIELD_ADVANCED: &str = "SHIELD_ADVANCED";

/// `PolicyFirewallType` value whose policies carry the AWS WAF settings block (see
/// [`waf_config_violation`]).
pub const FIREWALL_TYPE_WAF: &str = "WAF";

/// The `policyConfiguration` member of a policy create/update, as the model names it:
/// the root of the dotted path a refusal of one of its nested members reports.
pub const POLICY_CONFIGURATION_MEMBER: &str = "policyConfiguration";

/// `ServiceResourceType` naming a policy, for the `resourceType` member the modelled
/// `ConflictException` / `ResourceNotFoundException` shapes carry.
pub const RESOURCE_TYPE_POLICY: &str = "Policy";

/// The modelled `ResourceNotFoundException` this service answers when an identifier names
/// no resource of the entity being resolved: the `ServiceResourceType` of that entity,
/// then the id the identifier names ([`entity_id_in`]).
///
/// `resource_type` is one of the modelled `ServiceResourceType` values — `Rule`,
/// `Template`, `Policy`, `Deployment`, `Scope` — which is also the word the sentence opens
/// with, so the message names the kind of thing that is absent and the one variable part
/// is the id taken from the request. One home for the refusal, so every operation that
/// resolves such an identifier words its miss alike and fills `resourceId` /
/// `resourceType` from the same reading of the identifier.
pub fn resource_not_found(resource_type: &str, identifier: &str) -> ResourceNotFoundException {
    let resource_id = entity_id_in(identifier);
    ResourceNotFoundException {
        message: format!("{resource_type} not found: {resource_id}"),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The entity id an identifier names, whether or not a resource still answers to it: the
/// resource segment of the ARN the model documents an identifier to be ("This is the
/// <entity>'s Amazon Resource Name (ARN)"), with the version suffix a version-qualified
/// identifier carries dropped ([`is_entity_version`]), or the value itself when it carries
/// neither.
///
/// What a miss reports, so a refusal names the resource rather than the whole ARN the
/// caller sent.
pub fn entity_id_in(identifier: &str) -> &str {
    let unversioned = match identifier.rsplit_once(':') {
        Some((head, suffix)) if is_entity_version(suffix) => head,
        _ => identifier,
    };
    unversioned.split('/').next_back().unwrap_or(unversioned)
}

/// The `message` this service states when a write's `updateToken` is not the one the
/// addressed resource currently carries — the modelled `ConflictException`'s
/// "the resource was modified concurrently" case.
///
/// Fixed text for the error class: it names neither the resource nor either token,
/// because the `ConflictException` shape carries the resource in its own optional
/// `resourceId` / `resourceType` members, and a token is a value one run produced. One
/// home for the sentence, so every entity's update refuses a stale token alike.
pub const UPDATE_TOKEN_CONFLICT_MESSAGE: &str =
    "The provided updateToken is not the current updateToken for the resource";

/// The largest page `ListPolicies` can answer with, and so the page size used when the
/// caller sends no `maxResults`: `PolicySummaryList` is modelled `@length(max: 100)`, so
/// a larger page could not be serialized at all.
pub const MAX_POLICY_PAGE: usize = 100;

/// Version tag of the opaque `nextToken` rendering, so a token minted by an older
/// emulator is rejected as invalid rather than silently misread.
const NEXT_TOKEN_VERSION: &str = "1";

/// The most policies this emulator stores, after which `CreatePolicy` answers with the
/// modelled `ServiceQuotaExceededException` (HTTP 402, "The request would exceed a
/// service quota") instead of growing [`Self::policies`] for as long as a caller keeps
/// asking. The number is a CHOSEN emulator capacity, not a quota this service documents:
/// what matters is that the store has a ceiling and that reaching it is refused through a
/// modelled error rather than by exhausting the host.
pub const MAX_POLICIES: usize = 100;

/// The most `@idempotencyToken` records this emulator remembers. Each successful
/// create/update with a `clientToken` adds one, and the token is chosen by the caller, so
/// the map needs a ceiling of its own: past it the OLDEST record is evicted, which at
/// worst makes a very old replay act as a fresh request — the same thing that happens
/// once a real service's idempotency window expires.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 1024;

/// `@length` of this service's ARN members: `AssociatedTemplateOrRule`'s
/// `templateArn`/`ruleArn` are `length 20..1010`.
const MIN_ARN_LENGTH: usize = 20;
/// See [`MIN_ARN_LENGTH`].
const MAX_ARN_LENGTH: usize = 1010;

/// Whether `value` is an ARN as this service's model defines one: inside the `@length` its
/// ARN members carry, and matching the pattern they carry,
/// `^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:(.+)$` — that is,
/// the literal `arn`, a non-empty partition and service segment, a region and account
/// segment that may be empty, and then a non-empty resource remainder, with every segment
/// in the grammar [`is_arn_segment`] checks.
pub fn is_modelled_arn(value: &str) -> bool {
    if !(MIN_ARN_LENGTH..=MAX_ARN_LENGTH).contains(&value.chars().count()) {
        return false;
    }
    // Five colons split the six fields; the sixth keeps any further colon, which is what
    // the pattern's trailing `(.+)` allows inside a resource.
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    is_arn_segment(partition)
        && is_arn_segment(service)
        && (region.is_empty() || is_arn_segment(region))
        && (account.is_empty() || is_arn_segment(account))
        && !resource.is_empty()
}

/// One persisted policy: every member `GetPolicy` answers with, plus the bookkeeping the
/// lifecycle needs (`tags`, `created_at`).
#[derive(Debug, Clone)]
pub struct Policy {
    pub policy_id: String,
    pub policy_arn: String,
    pub policy_name: String,
    pub policy_description: Option<String>,
    /// `EntityStatus` — [`STATUS_ACTIVE`] or [`STATUS_DRAFT`], recomputed from
    /// `isPublished` on every create and update.
    pub status: String,
    pub priority: i32,
    /// `PolicyFirewallType`. Fixed at create: no modelled operation changes it.
    pub firewall_type: String,
    pub policy_configuration: Option<PolicyConfiguration>,
    pub associated_template_and_rule_list: Vec<AssociatedTemplateOrRule>,
    /// Rendered as the modelled `EntityVersion` (`^[1-9][0-9]*$`) by
    /// [`Policy::version_string`]; starts at 1 and is bumped by every update.
    pub version: u32,
    /// `UpdateToken` — a UUID rotated on every mutation, so a caller holding a stale one
    /// is refused with `ConflictException`.
    pub update_token: String,
    pub is_snapshot: bool,
    pub has_published_version: bool,
    /// The policy's PUBLISHED version: the body as it stood when a create or update last
    /// published one, which a draft update leaves alone, and which
    /// [`PolicySnapshot`] copies. `None` until the policy publishes a version, so it is
    /// `Some` exactly when [`Policy::has_published_version`] is true — a create or update
    /// writes the two together.
    pub published: Option<PolicyVersion>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Policy {
    /// The modelled `EntityVersion` rendering of [`Policy::version`].
    pub fn version_string(&self) -> String {
        self.version.to_string()
    }

    /// `updatedAt` as the modelled `DateTimestamp`.
    pub fn updated_at_timestamp(&self) -> ::aws_smithy_types::DateTime {
        to_date_time(self.updated_at)
    }

    /// This policy as the `PolicySummary` `ListPolicies` returns.
    pub fn summary(&self) -> PolicySummary {
        PolicySummary {
            policy_id: self.policy_id.clone(),
            policy_arn: self.policy_arn.clone(),
            policy_name: Some(self.policy_name.clone()),
            status: Some(self.status.clone()),
            version: Some(self.version_string()),
            has_published_version: Some(self.has_published_version),
            firewall_type: Some(self.firewall_type.clone()),
            priority: Some(self.priority),
            updated_at: Some(self.updated_at_timestamp()),
        }
    }
}

/// The `EntityStatus` a policy carries for an `isPublished` value: the model's
/// "When true, the resource is saved in published (ACTIVE) state. When false, it is
/// saved as a draft (DRAFT)."
pub fn status_for(is_published: bool) -> &'static str {
    if is_published { STATUS_ACTIVE } else { STATUS_DRAFT }
}

/// A `SystemTime` as the modelled `DateTimestamp`.
///
/// Computed from the UNIX epoch rather than through a `From` impl so a pre-epoch clock
/// degrades to the epoch instead of panicking on the request path.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A fresh `UpdateToken`: 16 bytes from [`random_bytes`] rendered as a version-4 UUID,
/// which is exactly the modelled `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`.
pub fn mint_update_token() -> String {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&random_bytes()[..16]);
    // The version and variant nibbles a version-4 UUID carries.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// 32 fresh bytes for one minted id or token.
///
/// The seed is what the standard library offers without reaching for another crate:
/// `RandomState`, which std seeds randomly once per process and varies per instance, plus
/// the clock and a per-process mint counter so two values minted in the same instant still
/// differ. SHA-256 spreads the seed so none of its structure shows in the value. A
/// `policyId` and an `updateToken` are identifiers, not secrets — this is a uniqueness
/// source, and nothing here is used as a credential.
fn random_bytes() -> [u8; 32] {
    use ::sha2::Digest as _;
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// How many values this process has minted, so the seed never repeats within it.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let mut digest = ::sha2::Sha256::new();
    for _ in 0..2 {
        digest.update(RandomState::new().build_hasher().finish().to_le_bytes());
    }
    digest.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    digest.update(MINTED.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    digest.finalize().into()
}

/// The region of ARNs minted for this request: the SigV4 credential scope's region when
/// it is a legal ARN segment, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_segment(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` can stand in an ARN segment, i.e. matches `[a-z0-9]+([.-][a-z0-9]+)*`
/// — the per-segment grammar `PolicyArn`'s own pattern declares. Guards the one segment
/// taken from the request (the credential-scope region) so a hostile scope cannot mint an
/// ARN outside the modelled pattern.
fn is_arn_segment(value: &str) -> bool {
    value.split(['.', '-']).all(|group| {
        !group.is_empty()
            && group
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// The `templateArn`/`ruleArn` association a create/update request's
/// `TemplateOrRuleReference` names.
///
/// The model: "The identifier of the template. This is the template's Amazon Resource
/// Name (ARN)" — so the reference carries the ARN and the association echoes it under the
/// matching output member.
pub fn association_of(reference: &TemplateOrRuleReference) -> AssociatedTemplateOrRule {
    match reference {
        TemplateOrRuleReference::templateIdentifier(identifier) => {
            AssociatedTemplateOrRule::templateArn(identifier.clone())
        }
        TemplateOrRuleReference::ruleIdentifier(identifier) => {
            AssociatedTemplateOrRule::ruleArn(identifier.clone())
        }
    }
}

/// The opaque `ListPolicies` `nextToken` resuming AFTER `policy_id`.
///
/// Hex-encoding the key keeps every byte inside `NextToken`'s modelled
/// `^[0-9A-Za-z:/+=_-]+$` pattern whatever the key contains, and keeps the token opaque
/// to a caller that should only hand it back.
pub fn encode_next_token(policy_id: &str) -> String {
    format!("{NEXT_TOKEN_VERSION}:{}", ::hex::encode(policy_id))
}

/// The exclusive start key a `nextToken` names, or `None` when it was not minted by
/// [`encode_next_token`] — including a token whose payload decodes to nothing, which
/// names no policy to resume after.
pub fn decode_next_token(token: &str) -> Option<String> {
    let encoded = token.strip_prefix(NEXT_TOKEN_VERSION)?.strip_prefix(':')?;
    let decoded = String::from_utf8(::hex::decode(encoded).ok()?).ok()?;
    (!decoded.is_empty()).then_some(decoded)
}

/// Refuse an association list the model's documented rules forbid, returning the message
/// naming what is wrong — `None` when the list is allowed.
///
/// The model states three rules its traits cannot express. From `CreatePolicy`'s
/// `associatedTemplateAndRuleList`: "For AWS WAF policies, specify 1 to 100 templates or
/// rules, of which at most 2 can be templates. For AWS Shield Advanced policies, this
/// list must be empty." And from `TemplateOrRuleReference`: "The identifier of the
/// template. This is the template's Amazon Resource Name (ARN)."
///
/// The documented WAF *minimum* of one association is deliberately not enforced: a WAF
/// policy with no associations is the shape this emulator's own lifecycle creates first,
/// and no modelled trait or error distinguishes it.
pub fn association_list_violation(
    firewall_type: &str,
    references: &[TemplateOrRuleReference],
) -> Option<String> {
    if firewall_type == FIREWALL_TYPE_SHIELD_ADVANCED && !references.is_empty() {
        return Some(format!(
            "associatedTemplateAndRuleList must be empty for a {FIREWALL_TYPE_SHIELD_ADVANCED} \
             policy, but {} entries were given",
            references.len()
        ));
    }
    let templates = references
        .iter()
        .filter(|reference| {
            matches!(reference, TemplateOrRuleReference::templateIdentifier(_))
        })
        .count();
    if templates > MAX_ASSOCIATED_TEMPLATES {
        return Some(format!(
            "associatedTemplateAndRuleList accepts at most {MAX_ASSOCIATED_TEMPLATES} \
             templates, but {templates} were given"
        ));
    }
    references.iter().find_map(|reference| {
        let (member, identifier) = match reference {
            TemplateOrRuleReference::templateIdentifier(identifier) => {
                ("templateIdentifier", identifier)
            }
            TemplateOrRuleReference::ruleIdentifier(identifier) => ("ruleIdentifier", identifier),
        };
        // The reference member itself carries no pattern, so the boundary validator
        // accepts any string — but the association it becomes is returned under
        // `AssociatedTemplateOrRule`, whose members ARE `@length(20..1010)` and carry the
        // ARN pattern. An identifier outside that is one this service could not answer
        // with, hence a `ValidationException` rather than a record nothing can read.
        (!is_modelled_arn(identifier)).then(|| {
            format!(
                "associatedTemplateAndRuleList {member} must be an Amazon Resource Name of \
                 {MIN_ARN_LENGTH} to {MAX_ARN_LENGTH} characters, but {identifier:?} is not one"
            )
        })
    })
}

/// The `ValidationExceptionReason` a refused member carries.
pub const VALIDATION_REASON_FIELD: &str = "FIELD_VALIDATION_FAILED";

/// The `wafConfig` member of `PolicyConfiguration`, as the model names it: "AWS WAF-specific
/// policy settings. This is populated only for AWS WAF policies."
const WAF_CONFIG_MEMBER: &str = "wafConfig";

/// `ExistingCustomerWebACLResolution`'s admissible values in the order the MODEL declares
/// them (`smithy_query(query="shape", shape="ExistingCustomerWebACLResolution")`) — the
/// order [`waf_config_violation`] lists them in, because the list it states is the modelled
/// enum rather than anything one request carries.
const EXISTING_CUSTOMER_WEB_ACL_RESOLUTION_DECLARED: &[&str] =
    &["RETROFIT", "OVERRIDE_ASSOCIATION", "NO_REMEDIATION"];

/// `WAFConflictResolutionOptions`' admissible values, declared as
/// [`EXISTING_CUSTOMER_WEB_ACL_RESOLUTION_DECLARED`] is
/// (`smithy_query(query="shape", shape="WAFConflictResolutionOptions")`).
const WAF_CONFLICT_RESOLUTION_DECLARED: &[&str] = &["MERGE_WHERE_APPLICABLE"];

/// Refuse a policy configuration the service does not accept, returning the `message` and
/// the `fieldList` the refusal states — `None` when the configuration is one it accepts.
///
/// Two rules the model states in prose but no constraint trait expresses, checked in the
/// order the recordings show them reported:
///
/// * the members `WafConfig` declares `@required` ([`waf_config_violation`]);
/// * the resource cleanup the service requires ([`resources_clean_up_violation`]).
///
/// One home for the pair, so `CreatePolicy` and `UpdatePolicy` refuse alike.
pub fn policy_configuration_violation(
    firewall_type: &str,
    configuration: &PolicyConfiguration,
) -> Option<(String, Vec<ValidationExceptionField>)> {
    waf_config_violation(firewall_type, configuration)
        .or_else(|| resources_clean_up_violation(configuration))
}

/// Refuse a policy configuration that leaves the resources the service creates behind,
/// returning the `message` and the `fieldList` the refusal states.
///
/// The model: `resourcesCleanUp` — "Specifies whether AWS Network Security Manager
/// automatically removes the resources it created when they are no longer needed." The
/// service only takes a policy it can clean up after, so a configuration that turns that
/// off is refused whatever else it carries: unlike the WAF rule above, the refusal names no
/// condition because the recorded one states none — it holds for every firewall type and
/// whether or not remediation is enabled, the modelled `@default` of `false` included.
///
/// The `fieldList` entry carries the member's dotted path and the sentence the service
/// states for the violation; nothing is invented for it, because the one thing recorded
/// about this refusal is that sentence.
fn resources_clean_up_violation(
    configuration: &PolicyConfiguration,
) -> Option<(String, Vec<ValidationExceptionField>)> {
    if configuration.resources_clean_up {
        return None;
    }
    let message = format!("{RESOURCES_CLEAN_UP_MEMBER} must be true");
    Some((
        message.clone(),
        vec![ValidationExceptionField {
            name: format!("{POLICY_CONFIGURATION_MEMBER}.{RESOURCES_CLEAN_UP_MEMBER}"),
            message,
        }],
    ))
}

/// The `resourcesCleanUp` member of `PolicyConfiguration`, as the model names it.
const RESOURCES_CLEAN_UP_MEMBER: &str = "resourcesCleanUp";

/// Refuse a policy configuration that does not carry a member its firewall type requires,
/// returning the `message` and the `fieldList` the refusal states — `None` when nothing is
/// missing.
///
/// `WafConfig`'s two members are `@required`, and the model says of each: "Required for AWS
/// WAF policies"; of the block that holds them it says "This is populated only for AWS WAF
/// policies". So a WAF policy has to supply both, whether or not it sends the block at all,
/// and a policy of any firewall type that DOES send the block has to complete it.
///
/// The SERVICE, not the protocol boundary, is what reports an omitted one: the refusal is
/// this operation's modelled `ValidationException`, whose `reason`
/// (`FIELD_VALIDATION_FAILED`) and `fieldList` only a handler can fill, so the shape is read
/// with both members optional (`crate::ops::types::waf_config_types`) and checked here. The
/// dotted `name` is the path to the member from the operation's input, built from the
/// members the model names — the same path whether the block was sent or not; the firewall
/// type is the one the request is for; and the valid-value list is the member's target enum
/// as the model declares it.
///
/// One member is named, the first the model declares of those missing, so a request missing
/// several states what a request missing one states.
fn waf_config_violation(
    firewall_type: &str,
    configuration: &PolicyConfiguration,
) -> Option<(String, Vec<ValidationExceptionField>)> {
    let waf_config = configuration.waf_config.as_ref();
    if waf_config.is_none() && firewall_type != FIREWALL_TYPE_WAF {
        return None;
    }
    // Each of `WafConfig`'s `@required` members, in the model's member order: the member
    // name, what the request supplied for it, and the values its target enum admits — for
    // the refusal's wording only. A value PRESENT but outside that enum is refused before
    // this runs, by the `EnumTrait` the member schema carries
    // (`crate::ops::types::waf_config_types`), so what is left to check here is presence.
    let members: [(&str, Option<&String>, &[&str]); 2] = [
        (
            "existingCustomerWebACLResolution",
            waf_config.and_then(|config| config.existing_customer_web_acl_resolution.as_ref()),
            EXISTING_CUSTOMER_WEB_ACL_RESOLUTION_DECLARED,
        ),
        (
            "conflictResolution",
            waf_config.and_then(|config| config.conflict_resolution.as_ref()),
            WAF_CONFLICT_RESOLUTION_DECLARED,
        ),
    ];
    let (member, _, values) = members
        .iter()
        .find(|(_, supplied, _)| supplied.is_none())?;
    Some((
        format!("{member} is required for {firewall_type} policies"),
        vec![ValidationExceptionField {
            name: format!("{POLICY_CONFIGURATION_MEMBER}.{WAF_CONFIG_MEMBER}.{member}"),
            message: format!(
                "{member} must be specified when firewallType is {firewall_type}. \
                 Valid values: {}",
                values.join(", ")
            ),
        }],
    ))
}

/// Refuse a create's reference member whose identifier is not an ARN, returning the
/// offending LIST member and the message naming what is wrong — `None` when every
/// identifier is usable.
///
/// Each item is `(list member, identifier member, the identifier submitted)`. A reference
/// member (`PolicyReference`'s `policyIdentifier`, `ScopeReference`'s `scopeIdentifier`,
/// `RuleReference`'s `ruleIdentifier`, …) carries no pattern of its own, so the boundary
/// validator accepts any string — but the association it becomes is returned under a
/// member that IS `@length(20..1010)` with the ARN `@pattern`, so an identifier outside
/// that is one no read of the resource could serialize. Whether the referenced resource
/// EXISTS is deliberately not checked: no create of this service models a
/// `ResourceNotFoundException`, so a reference is stored as the caller gave it.
///
/// One home for the check, so every entity that holds references refuses alike.
pub fn reference_arn_violation<'a>(
    references: impl IntoIterator<Item = (&'static str, &'static str, &'a str)>,
) -> Option<(&'static str, String)> {
    references
        .into_iter()
        .find_map(|(list, member, identifier)| {
            (!is_modelled_arn(identifier)).then(|| {
                (
                    list,
                    format!(
                        "{list} {member} must be an Amazon Resource Name of {MIN_ARN_LENGTH} to \
                         {MAX_ARN_LENGTH} characters, but {identifier:?} is not one"
                    ),
                )
            })
        })
}

/// Refuse an identifier the model documents to be the named resource's ARN when the value
/// carried is not an ARN, returning the `message` the refusal states — `None` when it is
/// one.
///
/// The member it names is documented "The identifier of the rule. This is the rule's
/// Amazon Resource Name (ARN)", so the admissible FORM of the value is an ARN
/// ([`is_modelled_arn`]): a bare entity id, a resource name or half an ARN is a field of
/// the request that failed validation, not an identifier that happens to resolve to
/// nothing. The refusal is therefore the modelled `ValidationException`
/// ([`VALIDATION_REASON_FIELD`]) and it is owed BEFORE the store is consulted, so no
/// lookup — hit or miss — decides it.
///
/// The sentence is fixed text for this error class, naming the offending input member and
/// nothing else. One home for it, so every operation resolving such an identifier refuses
/// alike.
pub fn identifier_arn_violation(member: &str, identifier: &str) -> Option<String> {
    (!is_modelled_arn(identifier)).then(|| format!("{member} must be the resource's ARN."))
}

/// The most templates (as opposed to rules) one policy may associate, per the model's
/// "of which at most 2 can be templates".
const MAX_ASSOCIATED_TEMPLATES: usize = 2;

/// A fresh entity id: lowercase hex of 16 fresh bytes followed by `seq`'s next value,
/// which satisfies the modelled `^[a-z0-9]{1,64}$` the `PolicyId` and `RuleId` shapes
/// declare (at most 32 + 16 characters) and cannot collide with an id this store already
/// handed out.
///
/// One function behind every entity's id, so each per-entity `mint_*_id` differs only in
/// which counter it advances.
fn mint_entity_id(seq: &mut u64) -> String {
    *seq = seq.saturating_add(1);
    format!("{}{:x}", ::hex::encode(&random_bytes()[..16]), *seq)
}

/// What one `@idempotencyToken` already did: the resource the original call left behind,
/// and the `EntityVersion` it left it at, so a replay can tell whether the record it
/// would answer with is still that call's result.
#[derive(Debug, Clone)]
pub struct IdempotentCall {
    /// The `policyId`/`ruleId` the original call created or updated. Which store it names
    /// follows from the operation the key is namespaced by (see [`idempotency_key`]).
    pub resource_id: String,
    /// The entity's version as the original call left it.
    pub version: u32,
    /// Insertion order within [`ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState::idempotency`],
    /// so the oldest record is the one evicted at [`MAX_IDEMPOTENCY_RECORDS`].
    seq: u64,
}

/// Per-service state for com-amazonaws-networksecuritymanager-piccolocustomerapiservice. A handler reaches it through [`service_state`].
///
/// Every map here is bounded: `policies` (and with it `policy_name_index`, one entry per
/// policy) by [`MAX_POLICIES`], `rules` by [`MAX_RULES`], `deployments` by
/// [`MAX_DEPLOYMENTS`], `templates` by [`MAX_TEMPLATES`], `scopes` by [`MAX_SCOPES`], and
/// `idempotency` by [`MAX_IDEMPOTENCY_RECORDS`], so no sequence of requests can grow this
/// process without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState {
    /// Every persisted policy, keyed by its `policyId` and so iterated in a stable
    /// order — which is what makes `ListPolicies`' `nextToken` resume exactly where the
    /// previous page stopped. At most [`MAX_POLICIES`] entries.
    pub policies: BTreeMap<String, Policy>,
    /// `policyName` -> `policyId`, so a create can detect a name already in use. One
    /// entry per stored policy: an entry is added with the policy and removed with it.
    pub policy_name_index: BTreeMap<String, String>,
    /// `<operation>:<clientToken>` -> what that token already did, so replaying a request
    /// with the same `@idempotencyToken` answers with the first call's result instead of
    /// acting twice. At most [`MAX_IDEMPOTENCY_RECORDS`] entries; write it through
    /// [`Self::remember_idempotent_call`], which enforces that bound.
    pub idempotency: BTreeMap<String, IdempotentCall>,
    /// `seq` -> idempotency key, oldest first: the eviction order of `idempotency`.
    idempotency_order: BTreeMap<u64, String>,
    /// Monotonic counter feeding [`Self::remember_idempotent_call`].
    next_idempotency_seq: u64,
    /// Monotonic counter feeding [`Self::mint_policy_id`].
    pub next_policy_seq: u64,
    /// Every snapshot taken of a policy's published version, keyed by that policy's
    /// `policyId` and the version the snapshot froze — so one published version has one
    /// snapshot, and the snapshots of one policy are iterated in version order. At most
    /// [`MAX_POLICY_SNAPSHOTS`] entries per policy; a policy's snapshots are removed with
    /// the policy.
    pub policy_snapshots: BTreeMap<(String, u32), PolicySnapshot>,
    /// Every persisted rule, keyed by its `ruleId` and so iterated in a stable order —
    /// which is what makes `ListRules`' `nextToken` resume exactly where the previous
    /// page stopped. At most [`MAX_RULES`] entries.
    ///
    /// Rules are NOT indexed by name: unlike a policy, two rules may carry the same
    /// `ruleName` and remain distinct resources.
    pub rules: BTreeMap<String, Rule>,
    /// Every snapshot taken of a rule's published version, keyed by that rule's `ruleId`
    /// and the version the snapshot froze — so one published version has one snapshot, and
    /// the snapshots of one rule are iterated in version order. At most
    /// [`MAX_RULE_SNAPSHOTS`] entries per rule; a rule's snapshots are removed with the
    /// rule.
    pub rule_snapshots: BTreeMap<(String, u32), RuleSnapshot>,
    /// Monotonic counter feeding [`Self::mint_rule_id`].
    pub next_rule_seq: u64,
    /// Every persisted deployment, keyed by its `deploymentId` and so iterated in a
    /// stable order — which is what makes `ListDeployments`' `nextToken` resume exactly
    /// where the previous page stopped. At most [`MAX_DEPLOYMENTS`] entries.
    ///
    /// Deployments are NOT indexed by name: no modelled trait and no documented rule
    /// makes `deploymentName` unique, so two deployments may carry one name and remain
    /// distinct resources, exactly as two rules may.
    pub deployments: BTreeMap<String, Deployment>,
    /// Monotonic counter feeding [`Self::mint_deployment_id`].
    pub next_deployment_seq: u64,
    /// Every persisted template, keyed by its `templateId` and so iterated in a stable
    /// order — which is what makes `ListTemplates`' `nextToken` resume exactly where the
    /// previous page stopped. At most [`MAX_TEMPLATES`] entries.
    ///
    /// Templates are NOT indexed by name, for the same reason rules are not: no modelled
    /// trait and no documented rule makes `templateName` unique.
    pub templates: BTreeMap<String, Template>,
    /// Monotonic counter feeding [`Self::mint_template_id`].
    pub next_template_seq: u64,
    /// Every persisted scope, keyed by its `scopeId` and so iterated in a stable order —
    /// which is what makes `ListScopes`' `nextToken` resume exactly where the previous
    /// page stopped. At most [`MAX_SCOPES`] entries.
    ///
    /// Scopes are NOT indexed by name, for the same reason rules are not: no modelled
    /// trait and no documented rule makes `scopeName` unique.
    pub scopes: BTreeMap<String, Scope>,
    /// Monotonic counter feeding [`Self::mint_scope_id`].
    pub next_scope_seq: u64,
}

impl ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState {
    /// A fresh `PolicyId`, minted by [`mint_entity_id`].
    pub fn mint_policy_id(&mut self) -> String {
        mint_entity_id(&mut self.next_policy_seq)
    }

    /// The `policyId` a `policyIdentifier` names — the id itself or the policy's ARN —
    /// or `None` when no stored policy answers to it.
    pub fn resolve_policy_id(&self, identifier: &str) -> Option<String> {
        if self.policies.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.policies
            .values()
            .find(|policy| policy.policy_arn == identifier)
            .map(|policy| policy.policy_id.clone())
    }

    /// The policy a `policyIdentifier` names.
    pub fn policy_by_identifier(&self, identifier: &str) -> Option<&Policy> {
        let id = self.resolve_policy_id(identifier)?;
        self.policies.get(&id)
    }

    /// What the `@idempotencyToken` behind `key` already did, if this store still
    /// remembers it.
    pub fn idempotent_call(&self, key: &str) -> Option<&IdempotentCall> {
        self.idempotency.get(key)
    }

    /// Remember that `key` produced `resource_id` at `version`, evicting the oldest record
    /// once the store holds [`MAX_IDEMPOTENCY_RECORDS`] of them — the bound that keeps a
    /// caller sending fresh tokens from growing this map without limit.
    pub fn remember_idempotent_call(&mut self, key: String, resource_id: String, version: u32) {
        self.next_idempotency_seq = self.next_idempotency_seq.saturating_add(1);
        let seq = self.next_idempotency_seq;
        let replaced = self.idempotency.insert(
            key.clone(),
            IdempotentCall {
                resource_id,
                version,
                seq,
            },
        );
        if let Some(replaced) = replaced {
            self.idempotency_order.remove(&replaced.seq);
        }
        self.idempotency_order.insert(seq, key);
        while self.idempotency.len() > MAX_IDEMPOTENCY_RECORDS {
            let Some((_, oldest)) = self.idempotency_order.pop_first() else {
                break;
            };
            self.idempotency.remove(&oldest);
        }
    }

    /// Drop every idempotency record pointing at `resource_id`, so deleting a resource
    /// releases what creating it allocated instead of leaving rows for an id no later
    /// call can read.
    pub fn forget_idempotent_calls_for(&mut self, resource_id: &str) {
        let stale: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, call)| call.resource_id == resource_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            if let Some(call) = self.idempotency.remove(&key) {
                self.idempotency_order.remove(&call.seq);
            }
        }
    }
}

/// The idempotency-map key for `operation`'s `clientToken`. Namespacing by operation keeps
/// one token reused across two operations from resolving to the other's result.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState>)
}
