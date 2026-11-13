// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafv2-awswaf-20190729 service.
//!
//! WAFv2 namespaces every resource by `Scope` (`CLOUDFRONT` | `REGIONAL`), so a
//! CLOUDFRONT web ACL named `x` and a REGIONAL one named `x` are two different
//! resources. Every map below is therefore keyed by `(Scope, …)`.
//!
//! An entry stores the MODELLED shape (`WebACL` / `IPSet`) the create call built,
//! not a flattened copy of its members, so a `Get` hands back exactly what was
//! stored — including the rule tree — and no member can be lost in translation
//! when the model grows one. The API bookkeeping that is not part of the shape
//! (its scope, its optimistic-lock token and its tags) sits beside it.

use std::collections::HashMap;
use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    validate, ElementConstraintsTrait, LengthTrait, PatternTrait, Violation, ViolationKind,
};

use crate::ops::types::{
    BlockAction, ChallengeAction, ChallengeConfig, CountAction, FieldToMatch, IPSet, LabelSummary,
    LoggingConfiguration, ManagedProductDescriptor, ManagedRuleSet, MobileSdkRelease,
    MonetizationFilter, OnSourceDDoSProtectionConfig, RegexPatternSet, Rule, RuleAction,
    RuleGroup,
    RuleSummary, SettlementRecord, Statement, Tag, TextTransformation, TimeWindow,
    WAFAssociatedItemException,
    WAFDuplicateItemException, WAFExpiredManagedRuleGroupVersionException,
    WAFFeatureNotIncludedInPricingPlanException, WAFInvalidOperationException,
    WAFInvalidParameterException, WAFInvalidPermissionPolicyException, WAFLimitsExceededException,
    WAFNonexistentItemException, WAFOptimisticLockException, WAFTagOperationException,
    WAFUnsupportedAggregateKeyTypeException, WebACL, BLOCKCHAIN_CHAIN_VALUES,
    CURRENCY_MODE_VALUES, CURRENCY_VALUES, PLATFORM_VALUES, SETTLEMENT_STATUS_VALUES,
};
/// The API-key value itself — the encrypted token a key IS — and the CAPTCHA
/// application integration URL a client loads the script from.
mod api_keys;
/// What one rule list costs to run, in web ACL capacity units.
mod capacity;
/// The managed rule groups this service publishes.
mod catalog;
/// The `Description` an entity carries, and the form it has to take.
mod description;
/// The notation and the order an IP set holds its `Addresses` in.
mod ip_set_addresses;
/// The caller-supplied regular expressions a save carries, and the refusal one that
/// is not something that matches draws.
mod patterns;
/// What a saved rule list has to satisfy across the whole list.
mod rule_set;
/// The `Scope` every resource is namespaced by.
mod scope;
pub use api_keys::*;
pub use capacity::*;
pub use catalog::*;
pub use description::*;
pub use ip_set_addresses::*;
pub use patterns::*;
pub use rule_set::*;
pub use scope::*;

// ---------------------------------------------------------------------------
// The exception messages: ONE constant sentence per exception type.
// ---------------------------------------------------------------------------
//
// Every WAFv2 exception below models its detail as a lone `Message` (the limits
// error adds a `SourceType`), and the model documents ONE sentence for the SHAPE
// itself — `smithy_query(query="shape",shape="WAFNonexistentItemException")` is
// "WAF couldn’t perform the operation because your resource doesn't exist …", not
// a template with a slot for the resource that was named. The message is therefore
// a property of the exception TYPE, not of the call: WAF answers the same sentence
// for every request that trips that exception, and the body carries no name, id,
// ARN or scope taken from the request. A handler that needs to record WHICH
// resource tripped it writes that to the audit log ([`audit`]), never to the
// response.
//
// `WAFInvalidParameterException` is the one exception whose body does describe the
// call: the model gives it `Field`, `Parameter` and `Reason` members for exactly
// that ("The invalid parameter that resulted in the exception"), so its message
// stays request-derived. `WAFInternalErrorException` already carries a constant,
// request-independent sentence naming the internal fault it reports.
//
// The sentence is a property of the exception type, but NOT of the exception type
// alone: where WAF distinguishes CONDITIONS inside one exception type, each
// condition has its own sentence, and a condition about a named thing names it.
// Both appear below — the managed-rules form of the nonexistent-item refusal
// ([`managed_rules_not_found_exception`]) names the vendor and group that failed to
// resolve, and the monetization form of the invalid-operation refusal
// ([`monetization_not_available_exception`]) states what is unavailable and what to
// do instead — so a constructor here answers ONE condition, and the generic
// constructor beside it answers the rest.

/// The sentence `WAFDuplicateItemException` carries, whatever the request named.
///
/// The wire wording WAF answers with, as the recorded wafv2 responses show it; the
/// model documents the same single sentence for the shape (see the note above).
pub const DUPLICATE_ITEM_MESSAGE: &str = "AWS WAF couldn’t perform the operation \
     because some resource in your request is a duplicate of an existing one.";

/// The sentence `WAFOptimisticLockException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`] for where the wording comes from.
pub const OPTIMISTIC_LOCK_MESSAGE: &str = "AWS WAF couldn’t save your changes \
     because someone changed the resource after you started to edit it. Reapply \
     your changes.";

/// The sentence `WAFNonexistentItemException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`].
pub const NONEXISTENT_ITEM_MESSAGE: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// The sentence `WAFAssociatedItemException` carries, whatever the request named.
///
/// The wire wording WAF answers with, as the recorded wafv2 responses show it: the
/// shape's own documented sentence
/// (`smithy_query(query="shape",shape="WAFAssociatedItemException")`, its wrapping
/// whitespace flattened) prefixed with the service name, the way every other
/// exception of this family words it (see [`DUPLICATE_ITEM_MESSAGE`]).
pub const ASSOCIATED_ITEM_MESSAGE: &str = "AWS WAF couldn’t perform the operation \
     because your resource is being used by another resource or it’s associated \
     with another resource.";

/// The sentence `WAFLimitsExceededException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFLimitsExceededException")`),
/// link text flattened, which names no particular quota or request.
pub const LIMITS_EXCEEDED_MESSAGE: &str = "WAF couldn’t perform the operation \
     because you exceeded your resource limit. For example, the maximum number of \
     WebACL objects that you can create for an Amazon Web Services account. For \
     more information, see WAF quotas in the WAF Developer Guide.";

/// `WAFDuplicateItemException` as WAF returns it: nothing but
/// [`DUPLICATE_ITEM_MESSAGE`].
pub fn duplicate_item_exception() -> WAFDuplicateItemException {
    WAFDuplicateItemException {
        message: Some(DUPLICATE_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFOptimisticLockException` as WAF returns it: nothing but
/// [`OPTIMISTIC_LOCK_MESSAGE`].
pub fn optimistic_lock_exception() -> WAFOptimisticLockException {
    WAFOptimisticLockException {
        message: Some(OPTIMISTIC_LOCK_MESSAGE.to_string()),
    }
}

/// `WAFNonexistentItemException` as WAF returns it: nothing but
/// [`NONEXISTENT_ITEM_MESSAGE`].
pub fn nonexistent_item_exception() -> WAFNonexistentItemException {
    WAFNonexistentItemException {
        message: Some(NONEXISTENT_ITEM_MESSAGE.to_string()),
    }
}

/// The sentence `WAFNonexistentItemException` carries when what failed to resolve
/// is a MANAGED rule group — a `(VendorName, Name)` pair, not a resource of this
/// account — as the recorded WAF responses word it: the two names the request
/// carried, and fixed text around them.
///
/// A managed rule group is identified by that pair and by nothing else ("you use
/// this, along with the vendor name, to identify the rule group", `Name`'s own
/// doc), so the pair is the only thing a caller can correct and the refusal states
/// which pair resolved to nothing — unlike a resource of this account, which the
/// caller addressed by an id it already holds.
pub fn managed_rules_not_found_exception(
    vendor_name: &str,
    name: &str,
) -> WAFNonexistentItemException {
    WAFNonexistentItemException {
        message: Some(format!(
            "ManagedRules with name {name} by vendor {vendor_name} does not exist. \
             Please check your inputs."
        )),
    }
}

/// `WAFAssociatedItemException` as WAF returns it: nothing but
/// [`ASSOCIATED_ITEM_MESSAGE`].
pub fn associated_item_exception() -> WAFAssociatedItemException {
    WAFAssociatedItemException {
        message: Some(ASSOCIATED_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFLimitsExceededException` as WAF returns it: [`LIMITS_EXCEEDED_MESSAGE`] and
/// no `SourceType`, which the model leaves optional and which this emulator has no
/// source value to report.
pub fn limits_exceeded_exception() -> WAFLimitsExceededException {
    WAFLimitsExceededException {
        message: Some(LIMITS_EXCEEDED_MESSAGE.to_string()),
        source_type: None,
    }
}

/// The sentence `WAFFeatureNotIncludedInPricingPlanException` carries: the shape's
/// own documented sentence
/// (`smithy_query(query="shape",shape="WAFFeatureNotIncludedInPricingPlanException")`).
/// See [`DUPLICATE_ITEM_MESSAGE`] for why the wording is a property of the
/// exception type rather than of the call.
pub const FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE: &str = "The operation failed \
     because the specified WAF feature isn’t supported by the CloudFront pricing \
     plan associated with the web ACL.";

/// `WAFFeatureNotIncludedInPricingPlanException` as WAF returns it:
/// [`FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE`] and no `DisallowedFeatures`.
///
/// The model leaves `DisallowedFeatures` optional and gives no vocabulary for
/// either of its members (`PricingPlanFeatureName` and `RequiredPricingPlanName`
/// are free strings, with no enum and no documented value list), so this emulator
/// reports no feature/plan names rather than inventing two.
pub fn feature_not_included_in_pricing_plan_exception()
-> WAFFeatureNotIncludedInPricingPlanException {
    WAFFeatureNotIncludedInPricingPlanException {
        message: Some(FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE.to_string()),
        disallowed_features: None,
    }
}

/// The sentence `WAFInvalidOperationException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFInvalidOperationException")`),
/// its wrapping whitespace flattened. See [`DUPLICATE_ITEM_MESSAGE`] for why the
/// wording is a property of the exception type rather than of the call.
pub const INVALID_OPERATION_MESSAGE: &str = "The operation isn't valid.";

/// `WAFInvalidOperationException` as WAF returns it: nothing but
/// [`INVALID_OPERATION_MESSAGE`].
///
/// This is the refusal for a call an operation's own preconditions forbid on the
/// resource it names — the request parsed, the resource exists, and the operation
/// still may not be applied to it.
pub fn invalid_operation_exception() -> WAFInvalidOperationException {
    WAFInvalidOperationException {
        message: Some(INVALID_OPERATION_MESSAGE.to_string()),
    }
}

/// The sentence `WAFInvalidOperationException` carries when the operation asked for
/// is a MONETIZATION one and the scope it names is the regional namespace, as the
/// recorded WAF responses word it: what is not available there, and the scope to
/// use instead.
///
/// Monetization is a CloudFront-delivery feature — a `Monetize` action answers a
/// request with HTTP 402 at the edge and the transactions it settles are recorded
/// per CloudFront-scoped web ACL — so a monetization query under the regional
/// namespace asks for an operation that does not exist there rather than carrying a
/// bad parameter value.
pub const MONETIZATION_NOT_AVAILABLE_MESSAGE: &str = "Monetization is not available \
     for regional resources. Use CloudFront scope to configure monetization.";

/// `WAFInvalidOperationException` as WAF returns it for a monetization query made
/// under the regional namespace: nothing but
/// [`MONETIZATION_NOT_AVAILABLE_MESSAGE`].
pub fn monetization_not_available_exception() -> WAFInvalidOperationException {
    WAFInvalidOperationException {
        message: Some(MONETIZATION_NOT_AVAILABLE_MESSAGE.to_string()),
    }
}

/// The sentence `WAFUnsupportedAggregateKeyTypeException` carries: the shape's own
/// documented sentence
/// (`smithy_query(query="shape",shape="WAFUnsupportedAggregateKeyTypeException")`),
/// its wrapping whitespace flattened. See [`DUPLICATE_ITEM_MESSAGE`] for why the
/// wording is a property of the exception type rather than of the call: the body
/// names neither the rule that was asked about nor the aggregation it uses.
pub const UNSUPPORTED_AGGREGATE_KEY_TYPE_MESSAGE: &str = "The rule that you've named \
     doesn't aggregate solely on the IP address or solely on the forwarded IP \
     address. This call is only available for rate-based rules with an \
     AggregateKeyType setting of IP or FORWARDED_IP.";

/// `WAFUnsupportedAggregateKeyTypeException` as WAF returns it: nothing but
/// [`UNSUPPORTED_AGGREGATE_KEY_TYPE_MESSAGE`].
pub fn unsupported_aggregate_key_type_exception() -> WAFUnsupportedAggregateKeyTypeException {
    WAFUnsupportedAggregateKeyTypeException {
        message: Some(UNSUPPORTED_AGGREGATE_KEY_TYPE_MESSAGE.to_string()),
    }
}

/// `WAFExpiredManagedRuleGroupVersionException` as WAF returns it: the sentence
/// naming the version that has expired, the managed rule group it is a version of
/// and the vendor that published it.
///
/// The one refusal of this service whose sentence is a TEMPLATE rather than a
/// constant: WAF words this one with the three values the call named — "You're
/// trying to use an expired version (`<VersionName>`) of `<RuleGroupName>` by
/// `<VendorName>`." — so its fixed text is reproduced exactly and its variable
/// parts come from the request and the published catalogue. Every other exception
/// type of this slice carries the one sentence its type always carries (see
/// [`DUPLICATE_ITEM_MESSAGE`]).
pub fn expired_managed_rule_group_version_exception(
    vendor_name: &str,
    name: &str,
    version: &str,
) -> WAFExpiredManagedRuleGroupVersionException {
    WAFExpiredManagedRuleGroupVersionException {
        message: Some(format!(
            "You're trying to use an expired version ({version}) of {name} by {vendor_name}."
        )),
    }
}

/// The sentence `WAFInvalidPermissionPolicyException` carries: the wire wording WAF
/// answers with, as the recorded wafv2 responses show it.
///
/// A shape's documentation prose is a description of the error, NOT the sentence the
/// service puts on the wire for it; where the two differ the recorded wording is the
/// one callers see, so this constant carries that and not the first sentence of the
/// shape's `documentation` trait. See [`DUPLICATE_ITEM_MESSAGE`] for why the wording
/// is a property of the exception type rather than of the call: the restrictions that
/// shape's documentation lists are what every request is judged against, not a detail
/// of one call, so the body names neither the policy that was rejected nor which
/// restriction it broke.
pub const INVALID_PERMISSION_POLICY_MESSAGE: &str = "The policy specification doesn't \
     meet the requirements to be attached to an AWS WAF resource.";

/// `WAFInvalidPermissionPolicyException` as WAF returns it: nothing but
/// [`INVALID_PERMISSION_POLICY_MESSAGE`].
///
/// This is the ONE refusal for a permission policy outside the restrictions
/// `PutPermissionPolicy` documents, whichever of them the document breaks (see
/// [`permission_policy_is_valid`]).
pub fn invalid_permission_policy_exception() -> WAFInvalidPermissionPolicyException {
    WAFInvalidPermissionPolicyException {
        message: Some(INVALID_PERMISSION_POLICY_MESSAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tag keys AWS keeps for itself
// ---------------------------------------------------------------------------

/// The tag-key prefix that belongs to AWS: a key beginning with it names a SYSTEM
/// tag, which a caller may neither set nor change.
///
/// `TagKey`'s own `@pattern` admits it (`:` is one of the characters the pattern
/// allows), so a key under this prefix reaches a handler and the handler is what
/// refuses it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The sentence a tagging refusal carries when a request names a key under
/// [`RESERVED_TAG_KEY_PREFIX`]: the wire wording WAF answers with, as the recorded
/// wafv2 responses show it.
///
/// `WAFTagOperationException` is "an error occurred during the tagging operation"
/// — one class covering several tagging failures — so, unlike the one-sentence
/// exceptions above, its wording names WHICH tagging rule the request broke. The
/// sentence carries no key from the request: it states the rule, and the caller
/// already knows which keys it sent.
pub const RESERVED_TAG_KEY_MESSAGE: &str = "You tried to tag your resources with \
     system tag keys. These tags have an aws: prefix and are reserved by AWS. \
     Change your tag keys, and try again.";

/// `true` when `key` is a system tag key — one under
/// [`RESERVED_TAG_KEY_PREFIX`].
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The first key of `tags` that is a system tag key, or `None` when none of them
/// is.
///
/// What every operation that ACCEPTS tags asks before it stores anything, so one
/// reading of the reserved prefix serves the tagging operation and each create
/// that takes a `TagList`.
pub fn reserved_tag_key(tags: &[Tag]) -> Option<&str> {
    tags.iter()
        .map(|tag| tag.key.as_str())
        .find(|key| is_reserved_tag_key(key))
}

/// `WAFTagOperationException` as WAF returns it for a key under the reserved
/// prefix: nothing but [`RESERVED_TAG_KEY_MESSAGE`].
pub fn reserved_tag_key_exception() -> WAFTagOperationException {
    WAFTagOperationException {
        message: Some(RESERVED_TAG_KEY_MESSAGE.to_string()),
    }
}

/// Apply the tags of a tagging request to the tags a resource already carries.
///
/// A tag is a key:value PAIR identified by its key ("you can use a tag key to
/// describe a category of information", `Tag$Key`), so applying one that the
/// resource already carries REPLACES that key's value rather than adding a second
/// pair under the same key, and applying a new key appends it. The keys already
/// there keep their order, so a resource's tag collection reads in the order its
/// keys were first set.
pub fn apply_tags(stored: &mut Vec<Tag>, requested: &[Tag]) {
    for tag in requested {
        match stored.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => stored.push(tag.clone()),
        }
    }
}

/// Take away the tags a resource carries under `keys`, and leave every other tag
/// it carries exactly as it was.
///
/// The counterpart of [`apply_tags`]: a tag is a key:value PAIR identified by its
/// key, so an untagging request names KEYS alone and the pair held under each one
/// goes; a key the resource does not carry removes nothing, which is what makes
/// the removal idempotent. The tags that stay keep their relative order, so the
/// collection still reads in the order its keys were first set.
pub fn remove_tags(stored: &mut Vec<Tag>, keys: &[String]) {
    stored.retain(|held| !keys.contains(&held.key));
}

/// How many tags a resource holding `stored` would carry once `requested` is
/// applied to it: the keys it already has, plus the requested keys it does not.
///
/// What a tagging operation asks before it applies anything, because replacing the
/// value of a key already held adds no tag (see [`apply_tags`]) and so can never
/// take a resource past [`MAX_TAGS_PER_RESOURCE`].
pub fn tags_after_applying(stored: &[Tag], requested: &[Tag]) -> usize {
    let added = requested
        .iter()
        .filter(|tag| !stored.iter().any(|held| held.key == tag.key))
        .map(|tag| tag.key.as_str())
        .collect::<::std::collections::BTreeSet<&str>>()
        .len();
    stored.len() + added
}

/// The most tags one resource carries.
///
/// Source: `TagResource`'s own documentation — "You can specify one or more tags
/// to add to each Amazon Web Services resource, up to 50 tags for a resource."
/// Enforced where a request ADDS tags, so no caller can grow one resource's tag
/// collection without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

// ---------------------------------------------------------------------------
// The mobile SDK release catalogue
// ---------------------------------------------------------------------------

/// The sentence the mobile-SDK operations' `WAFInvalidOperationException` carries
/// when the account may not use the mobile SDK: the wire wording WAF answers with,
/// as the recorded wafv2 responses show it.
///
/// `WAFInvalidOperationException` is the refusal for a call the operation's own
/// preconditions forbid ([`invalid_operation_exception`] carries the sentence the
/// shape documents for the class); this is the sentence WAF words it with for the
/// one precondition the mobile-SDK family has, so a caller can tell an
/// unentitled account from any other invalid operation.
pub const MOBILE_SDK_NOT_AUTHORIZED_MESSAGE: &str = "This account is not authorized \
     to use Mobile-SDK";

/// `WAFInvalidOperationException` as WAF returns it for an account that may not
/// use the mobile SDK: nothing but [`MOBILE_SDK_NOT_AUTHORIZED_MESSAGE`].
pub fn mobile_sdk_not_authorized_exception() -> WAFInvalidOperationException {
    WAFInvalidOperationException {
        message: Some(MOBILE_SDK_NOT_AUTHORIZED_MESSAGE.to_string()),
    }
}

/// `true` when `value` is one of the device platforms the model declares
/// (`smithy_query(query="shape",shape="Platform")` → `IOS`, `ANDROID`).
pub fn is_known_platform(value: &str) -> bool {
    PLATFORM_VALUES.contains(&value)
}

/// The `Reason` a refusal carries for a `Platform` the model does not declare.
///
/// One wording for the whole mobile-SDK family, so its three operations cannot
/// drift apart in what they say about the same member.
pub const PLATFORM_NOT_VALID_REASON: &str = "the platform names the device platform \
     the releases are published for, and is one of ANDROID or IOS";

/// A WebACL as stored: the modelled shape plus the bookkeeping WAFv2 keeps beside
/// it.
#[derive(Debug, Clone)]
pub struct WebAclEntry {
    /// The scope the web ACL was created in.
    pub scope: String,
    /// Exactly what `GetWebACL` answers with.
    pub web_acl: WebACL,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// An IPSet as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct IpSetEntry {
    /// The scope the IP set was created in.
    pub scope: String,
    /// Exactly what `GetIPSet` answers with.
    pub ip_set: IPSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// A RegexPatternSet as stored: the modelled shape plus its scope, lock token and
/// tags.
#[derive(Debug, Clone)]
pub struct RegexPatternSetEntry {
    /// The scope the regex pattern set was created in.
    pub scope: String,
    /// Exactly what `GetRegexPatternSet` answers with.
    pub regex_pattern_set: RegexPatternSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// A RuleGroup as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct RuleGroupEntry {
    /// The scope the rule group was created in.
    pub scope: String,
    /// Exactly what `GetRuleGroup` answers with.
    pub rule_group: RuleGroup,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
    /// The IAM policy `PutPermissionPolicy` attached to this rule group, as the
    /// caller wrote it, or `None` while none is attached.
    ///
    /// A rule group carries at most one: "you can attach only one policy with each
    /// PutPermissionPolicy request" and the policy is addressed by the rule group's
    /// own ARN, so attaching replaces whatever was attached. The document is stored
    /// as the request wrote it, byte for byte — a policy is opaque to WAF beyond the
    /// restrictions [`permission_policy_is_valid`] checks — so
    /// `GetPermissionPolicy` answers the caller's own text.
    pub permission_policy: Option<String>,
}

/// A managed rule set as stored: the modelled shape plus its scope and its
/// optimistic-lock token.
///
/// A `ManagedRuleSet` is "a set of rules that is managed by Amazon Web Services and
/// Amazon Web Services Marketplace sellers to provide versioned managed rule groups
/// for customers of WAF", and the model's note on the shape says the
/// managed-rule-set APIs are "intended for use only by vendors of managed rule
/// sets". Those four operations — `ListManagedRuleSets`, `GetManagedRuleSet`,
/// `PutManagedRuleSetVersions` and `UpdateManagedRuleSetVersionExpiryDate` — list,
/// read and UPDATE a set; none of them creates one, because a vendor's set comes
/// into being when the vendor is onboarded as a WAF managed-rule-set vendor, which
/// no operation in this model performs. So an account holds no managed rule set
/// until one is provisioned through
/// [`ComAmazonawsWafv2Awswaf20190729State::insert_managed_rule_set`], and each of
/// the operations above answers its nonexistent-item failure for an identity the
/// store does not hold.
#[derive(Debug, Clone)]
pub struct ManagedRuleSetEntry {
    /// The scope the managed rule set lives in.
    pub scope: String,
    /// Exactly what `GetManagedRuleSet` answers with.
    pub managed_rule_set: ManagedRuleSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
}

/// An API key as stored: the encrypted key value, the token domains it carries
/// and the bookkeeping WAFv2 reports beside them.
///
/// An API key is not addressed by a name and an id like the entities above — "the
/// generated, encrypted API key" IS its identity within a scope
/// (`DeleteAPIKey$APIKey`, `GetDecryptedAPIKey$APIKey`) — so the entry carries the
/// key value itself and the two members `APIKeySummary` reports beside it, plus
/// the pagination cursor this service orders and resumes a listing by (see
/// [`api_key_cursor`]).
#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    /// The scope the key was created in.
    pub scope: String,
    /// The encrypted key value, as the create call answered it.
    pub api_key: String,
    /// `APIKeySummary$TokenDomains`: "the token domains that are defined in this
    /// API key", in the order they were stored in (see
    /// [`normalized_token_domains`] for the order a mint stores them in).
    pub token_domains: Vec<String>,
    /// `APIKeySummary$CreationTimestamp` ("the date and time that the key was
    /// created") as milliseconds since the Unix epoch, the resolution
    /// [`date_time_millis`] compares a modelled timestamp at.
    pub created_at_millis: i64,
    /// `APIKeySummary$Version`: "internal value used by WAF to manage the key"
    /// (`APIKeyVersion`, `range(min 0)`, default 0); see
    /// [`MINTED_API_KEY_VERSION`].
    pub version: i32,
    /// This entry's position in a listing: [`api_key_cursor`] of the creation
    /// sequence it was minted with.
    pub cursor: String,
}

/// The width a pagination cursor is zero-padded to: the 20 digits `u64::MAX`
/// takes, so every cursor is the same length and comparing two as strings orders
/// them by position.
const CURSOR_WIDTH: usize = 20;

/// The cursor naming position `sequence` in a listing: a decimal position
/// zero-padded to [`CURSOR_WIDTH`].
///
/// A `NextMarker` is opaque to a client — the model gives it no form beyond
/// `length 1..256` and `pattern \S` — so this service mints one from the position
/// a page resumes at, padded so the lexicographic comparison a resume does is the
/// numeric one.
pub fn listing_cursor(sequence: u64) -> String {
    format!("{sequence:0width$}", width = CURSOR_WIDTH)
}

/// The listing cursor of the `sequence`-th API key this service minted.
///
/// `ListAPIKeys`' marker is opaque to a client — the model gives `NextMarker` no
/// form beyond `length 1..256` and `pattern \S`, and an API key is up to 2048
/// characters long, so the key value itself cannot be one. This service mints the
/// cursor from the key's creation sequence instead, through [`listing_cursor`], so
/// a listing enumerates a scope's keys in the order they were created.
pub fn api_key_cursor(sequence: u64) -> String {
    listing_cursor(sequence)
}

/// The `Version` a key WAF minted carries.
///
/// `APIKeySummary$Version` is "internal value used by WAF to manage the key"
/// (`APIKeyVersion`, `range(min 0)`, zero-value default 0), so the number is WAF's
/// own bookkeeping rather than anything a caller supplies: recorded behaviour shows
/// a key WAF minted at 1, and nothing here re-versions a stored key, so every key
/// this service holds reports this.
pub const MINTED_API_KEY_VERSION: i32 = 1;

/// The token domains an API-key record is STORED with, for the domains a request
/// names.
///
/// `CreateAPIKey` "creates an API key that contains a set of token domains": the
/// domains are a SET, so the same domains in a different order are the same
/// definition and a domain named twice is named once. WAF normalizes the set to
/// lexicographic order when it writes the record, so every later read of the key —
/// its decrypted form and its listing summary — reports one order whatever order
/// the mint was given.
pub fn normalized_token_domains(token_domains: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = token_domains.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

/// One request a web ACL evaluated, as WAF's bot visibility recorded it: the URI
/// path the request asked for and the bot that sent it.
///
/// WAF reports `GetTopPathStatisticsByTraffic` by aggregating the requests a web
/// ACL actually evaluated over the window the caller names. This emulator serves
/// the WAFv2 control plane only — no HTTP request is ever evaluated against a
/// stored web ACL here — so nothing appends to
/// [`ComAmazonawsWafv2Awswaf20190729State::bot_requests`] and a web ACL that has
/// evaluated nothing reports zero requests over every window. The aggregation is
/// still the whole answer: a web ACL whose log holds requests reports exactly
/// those, grouped by path and ordered by volume.
#[derive(Debug, Clone)]
pub struct BotRequestRecord {
    /// When WAF evaluated the request, in milliseconds since the Unix epoch: a
    /// `TimeWindow` selects the records that fall inside it.
    pub observed_at_millis: i64,
    /// The URI path the request asked for, which `PathStatistics$Path` groups by.
    pub uri_path: String,
    /// The bot that sent the request, as `BotStatistics$BotName` names it and the
    /// `BotName` filter selects it.
    pub bot_name: String,
    /// The category the bot belongs to, as the `BotCategory` filter selects it.
    pub bot_category: String,
    /// The organization operating the bot, as the `BotOrganization` filter selects
    /// it.
    pub bot_organization: String,
}

// ---------------------------------------------------------------------------
// The monetization ledger: the settlement transactions a scope has recorded.
// ---------------------------------------------------------------------------

/// One settlement transaction, as WAF's monetization recorded it: the modelled
/// `SettlementRecord` a listing reports, plus the few filterable fields the
/// record itself does not carry.
///
/// A record is minted when a rule's `Monetize` action settles a payment for a
/// request a web ACL evaluated — WAF's DATA plane. This emulator serves the WAFv2
/// control plane only, so nothing appends to
/// [`ComAmazonawsWafv2Awswaf20190729State::settlement_records`] and every window
/// reports no transaction, which is the answer WAF gives for an account whose
/// window settled none. The selection is still the whole answer: a ledger that
/// holds transactions reports exactly those that fall in the window and satisfy
/// the request's filters.
#[derive(Debug, Clone)]
pub struct SettlementRecordEntry {
    /// The transaction as a listing reports it.
    pub record: SettlementRecord,
    /// `TerminatingRuleName`: "the name of the WAF rule that triggered the
    /// Monetize action" — a filter field that is not a member of
    /// `SettlementRecord` (`MonetizationFilter$Name`).
    pub terminating_rule_name: Option<String>,
    /// `HttpSourceName`: which Amazon Web Services service fronted the request.
    /// See [`HTTP_SOURCE_NAMES`].
    pub http_source_name: Option<String>,
    /// `HttpSourceId`: "the identifier of the Amazon Web Services resource
    /// associated with the web ACL (for example, CloudFront distribution ID)".
    pub http_source_id: Option<String>,
}

/// `CurrencyMode`'s `REAL` value: payments settled on a production blockchain
/// network (`smithy_query(query="shape",shape="CurrencyMode")`).
pub const CURRENCY_MODE_REAL: &str = "REAL";

/// `CurrencyMode`'s `TEST` value: payments settled on a testnet network. See
/// [`CURRENCY_MODE_REAL`].
pub const CURRENCY_MODE_TEST: &str = "TEST";

/// The `BlockchainChain` values that are testnets: `MonetizationConfig$CurrencyMode`
/// documents `TEST` as settling "on testnet networks (Base Sepolia, Solana Devnet)"
/// and `REAL` as settling "on production blockchain networks (Base, Solana)", so a
/// transaction's network is what puts it in one mode or the other.
pub const TEST_BLOCKCHAIN_CHAINS: &[&str] = &["BASE_SEPOLIA", "SOLANA_DEVNET"];

/// The `CurrencyMode` a settlement on `chain` belongs to. A transaction that names
/// no network is a production one, which is also the mode a query defaults to.
/// See [`TEST_BLOCKCHAIN_CHAINS`].
pub fn currency_mode_of_chain(chain: Option<&str>) -> &'static str {
    match chain {
        Some(chain) if TEST_BLOCKCHAIN_CHAINS.contains(&chain) => CURRENCY_MODE_TEST,
        _ => CURRENCY_MODE_REAL,
    }
}

/// The `HttpSourceName` values a monetization filter accepts, as
/// `MonetizationFilter$Name` lists them (`CF`, `ALB`, `APIGW`, `APPRUNNER`,
/// `COGNITO`, `VERIFIED_ACCESS`) — the member is a free string in the model, and
/// the documented list is the vocabulary, so it is written out here.
pub const HTTP_SOURCE_NAMES: &[&str] = &[
    "CF",
    "ALB",
    "APIGW",
    "APPRUNNER",
    "COGNITO",
    "VERIFIED_ACCESS",
];

/// The `MonetizationFilter$Name` values this service accepts, in the order the
/// member's own documentation lists them: the enum-restricted names, the
/// ARN-validated one, then the free-text ones.
///
/// Every monetization query (settlement records and the revenue statistics over
/// the same ledger) takes `Filters`, so the vocabulary, its validation
/// ([`validate_monetization_filters`]) and its matching
/// ([`monetization_filters_select`]) live here rather than in one operation.
pub const MONETIZATION_FILTER_NAMES: &[&str] = &[
    FILTER_CURRENCY_MODE,
    FILTER_CHAIN_NAME,
    FILTER_SETTLEMENT_STATUS,
    FILTER_HTTP_SOURCE_NAME,
    FILTER_WEB_ACL_ARN,
    FILTER_SOURCE_NAME,
    FILTER_SOURCE_CATEGORY,
    FILTER_INTENT,
    FILTER_ORGANIZATION,
    FILTER_URI_PATH_PREFIX,
    FILTER_REQUEST_ID,
    FILTER_TRANSACTION_ID,
    FILTER_TERMINATING_RULE_NAME,
    FILTER_PAYER_ADDRESS,
    FILTER_HTTP_SOURCE_ID,
];

/// `CurrencyMode`, restricted to [`CURRENCY_MODE_VALUES`].
pub const FILTER_CURRENCY_MODE: &str = "CurrencyMode";
/// `ChainName`, restricted to [`BLOCKCHAIN_CHAIN_VALUES`].
pub const FILTER_CHAIN_NAME: &str = "ChainName";
/// `SettlementStatus`, restricted to [`SETTLEMENT_STATUS_VALUES`].
pub const FILTER_SETTLEMENT_STATUS: &str = "SettlementStatus";
/// `HttpSourceName`, restricted to [`HTTP_SOURCE_NAMES`].
pub const FILTER_HTTP_SOURCE_NAME: &str = "HttpSourceName";
/// `WebACLArn`, which takes a web ACL ARN.
pub const FILTER_WEB_ACL_ARN: &str = "WebACLArn";
/// `SourceName`: "the name of the bot".
pub const FILTER_SOURCE_NAME: &str = "SourceName";
/// `SourceCategory`: "the category classification of the bot".
pub const FILTER_SOURCE_CATEGORY: &str = "SourceCategory";
/// `Intent`: "the declared intent of the bot request".
pub const FILTER_INTENT: &str = "Intent";
/// `Organization`: "the organization operating the bot".
pub const FILTER_ORGANIZATION: &str = "Organization";
/// `UriPathPrefix`: "the URI path of the request that was monetized", matched as
/// the prefix its name says it is.
pub const FILTER_URI_PATH_PREFIX: &str = "UriPathPrefix";
/// `RequestId`: "the WAF request ID associated with the transaction".
pub const FILTER_REQUEST_ID: &str = "RequestId";
/// `TransactionId`: "the blockchain transaction identifier".
pub const FILTER_TRANSACTION_ID: &str = "TransactionId";
/// `TerminatingRuleName`: "the name of the WAF rule that triggered the Monetize
/// action".
pub const FILTER_TERMINATING_RULE_NAME: &str = "TerminatingRuleName";
/// `PayerAddress`: "the blockchain wallet address of the paying client".
pub const FILTER_PAYER_ADDRESS: &str = "PayerAddress";
/// `HttpSourceId`: the Amazon Web Services resource fronting the web ACL.
pub const FILTER_HTTP_SOURCE_ID: &str = "HttpSourceId";

/// The values a filter named `name` accepts, for the names
/// `MonetizationFilter$Name` calls "enum-restricted (invalid values rejected)" —
/// `None` for a name whose values are free text or an ARN.
pub fn monetization_filter_values(name: &str) -> Option<&'static [&'static str]> {
    match name {
        FILTER_CURRENCY_MODE => Some(CURRENCY_MODE_VALUES),
        FILTER_CHAIN_NAME => Some(BLOCKCHAIN_CHAIN_VALUES),
        FILTER_SETTLEMENT_STATUS => Some(SETTLEMENT_STATUS_VALUES),
        FILTER_HTTP_SOURCE_NAME => Some(HTTP_SOURCE_NAMES),
        _ => None,
    }
}

/// Refuse a `Filters` list that names a filter this service does not have, or
/// gives an enum-restricted filter a value outside its vocabulary, or gives the
/// ARN-validated filter something that is not a web ACL ARN.
///
/// The refusal is the modelled `WAFInvalidParameterException`: `Parameter` carries
/// the value that was rejected, so a client reads which filter to correct.
pub fn validate_monetization_filters(
    filters: &[MonetizationFilter],
) -> Result<(), WAFInvalidParameterException> {
    for filter in filters {
        if !MONETIZATION_FILTER_NAMES.contains(&filter.name.as_str()) {
            return Err(invalid_parameter_exception_without_field(
                &filter.name,
                "the filter name is not one this operation filters by",
            ));
        }
        if let Some(allowed) = monetization_filter_values(&filter.name) {
            if let Some(value) = filter
                .values
                .iter()
                .find(|value| !allowed.contains(&value.as_str()))
            {
                return Err(invalid_parameter_exception_without_field(
                    value,
                    &format!("{} takes one of {}", filter.name, allowed.join(", ")),
                ));
            }
        }
        if filter.name == FILTER_WEB_ACL_ARN {
            if let Some(value) = filter
                .values
                .iter()
                .find(|value| arn_resource_kind(value) != Some(WEB_ACL_ARN_KIND))
            {
                return Err(invalid_arn_exception(value));
            }
        }
    }
    Ok(())
}

/// Whether `entry` satisfies every filter in `filters`.
///
/// A filter holds a NAME and a list of values, so a transaction satisfies one when
/// its own value for that field is among the filter's values, and satisfies the
/// list when it satisfies every filter in it. `UriPathPrefix` is the one name
/// matched as a prefix of the field rather than against the whole of it.
///
/// Absent a `CurrencyMode` filter the selection is the production one: "when no
/// CurrencyMode filter is provided, results default to REAL" (every monetization
/// operation's own documentation), so a testnet transaction is reported only to a
/// query that asks for it.
pub fn monetization_filters_select(
    entry: &SettlementRecordEntry,
    filters: &[MonetizationFilter],
) -> bool {
    let mut currency_mode_filtered = false;
    for filter in filters {
        if filter.name == FILTER_CURRENCY_MODE {
            currency_mode_filtered = true;
        }
        let value = monetization_filter_field(entry, &filter.name);
        let matched = match (filter.name.as_str(), value) {
            (_, None) => false,
            (FILTER_URI_PATH_PREFIX, Some(field)) => filter
                .values
                .iter()
                .any(|value| field.starts_with(value.as_str())),
            (_, Some(field)) => filter.values.iter().any(|value| value == field),
        };
        if !matched {
            return false;
        }
    }
    currency_mode_filtered
        || monetization_filter_field(entry, FILTER_CURRENCY_MODE) == Some(CURRENCY_MODE_REAL)
}

/// The value of `entry` a filter named `name` compares against, or `None` when the
/// transaction carries none — a transaction with no value for a field satisfies no
/// filter on it.
fn monetization_filter_field<'a>(entry: &'a SettlementRecordEntry, name: &str) -> Option<&'a str> {
    let record = &entry.record;
    match name {
        FILTER_CURRENCY_MODE => Some(currency_mode_of_chain(record.network.as_deref())),
        FILTER_CHAIN_NAME => record.network.as_deref(),
        FILTER_SETTLEMENT_STATUS => Some(record.status.as_str()),
        FILTER_HTTP_SOURCE_NAME => entry.http_source_name.as_deref(),
        FILTER_WEB_ACL_ARN => record.web_acl_arn.as_deref(),
        FILTER_SOURCE_NAME => record.source_name.as_deref(),
        FILTER_SOURCE_CATEGORY => record.source_category.as_deref(),
        FILTER_INTENT => record.intent.as_deref(),
        FILTER_ORGANIZATION => record.organization.as_deref(),
        FILTER_URI_PATH_PREFIX => record.content_path.as_deref(),
        FILTER_REQUEST_ID => record.request_id.as_deref(),
        FILTER_TRANSACTION_ID => record.transaction_id.as_deref(),
        FILTER_TERMINATING_RULE_NAME => entry.terminating_rule_name.as_deref(),
        FILTER_PAYER_ADDRESS => record.payer_address.as_deref(),
        FILTER_HTTP_SOURCE_ID => entry.http_source_id.as_deref(),
        _ => None,
    }
}

/// The widest span a monetization query may cover: "the maximum supported time
/// window is 90 days", which every monetization operation documents.
pub const MAX_MONETIZATION_WINDOW_DAYS: i64 = 90;

/// [`MAX_MONETIZATION_WINDOW_DAYS`] in milliseconds, the unit a window is compared
/// in.
const MAX_MONETIZATION_WINDOW_MILLIS: i64 = MAX_MONETIZATION_WINDOW_DAYS * 24 * 60 * 60 * 1_000;

/// The instants a monetization query covers, in milliseconds since the Unix epoch,
/// or the modelled refusal for a window wider than the maximum supported one.
///
/// The refusal names the MEMBER in `Parameter` rather than a value: what is at
/// fault is the span between the window's two timestamps, which is not one of the
/// values the request carries, so `TimeWindow` is the most precise thing a client
/// can be pointed at (the `Reason` states the bound it broke).
///
/// A window whose end precedes its start holds no instant, so it selects no
/// transaction: the model names no error for one, and the window is a filter.
pub fn monetization_window(
    window: &TimeWindow,
) -> Result<::std::ops::RangeInclusive<i64>, WAFInvalidParameterException> {
    let start = date_time_millis(&window.start_time);
    let end = date_time_millis(&window.end_time);
    if end.saturating_sub(start) > MAX_MONETIZATION_WINDOW_MILLIS {
        return Err(invalid_parameter_exception_without_field(
            "TimeWindow",
            &format!(
                "the maximum supported time window is {MAX_MONETIZATION_WINDOW_DAYS} days"
            ),
        ));
    }
    Ok(start..=end)
}

/// `SettlementStatus`'s `SETTLED` value: the one status whose payment "was
/// successfully settled on the blockchain and the transfer from the payer's wallet
/// to the publisher's wallet is confirmed" (`SettlementRecord$Status`) — the
/// transactions that moved money, which is what a revenue figure is of. Under
/// every other status "the client is not charged" or "content is not served".
pub const SETTLEMENT_STATUS_SETTLED: &str = "SETTLED";

/// The quantity a monetization amount spells, as the exact decimal it is.
///
/// `MonetizationAmountValue` is a STRING in the model and an amount is a quantity
/// of money, so it is read as a decimal rather than as a float: a sum of amounts
/// has to come to exactly the figure the operands spell, which a binary float
/// cannot do. A value that spells no number counts as nothing, which keeps one
/// unreadable transaction from deciding a whole query's answer.
pub fn monetization_amount(amount: &str) -> ::bigdecimal::BigDecimal {
    amount
        .trim()
        .parse::<::bigdecimal::BigDecimal>()
        .unwrap_or_else(|_| ::bigdecimal::BigDecimal::from(0))
}

/// How many fractional digits a `MonetizationAmountValue` is written to.
///
/// The model carries the shape as a plain string with no pattern, so its textual
/// form is this service's: `Currency` names one denomination, `USDC`
/// (`smithy_query(query="shape",shape="Currency")`), whose smallest unit is one
/// millionth of a token, and every amount is written at that scale — the integer
/// part, `.`, and exactly this many fractional digits, so a zero balance reads
/// `0.000000`. One scale for every amount is what lets a client hand the member
/// straight to a fixed-scale decimal parser.
pub const MONETIZATION_AMOUNT_SCALE: i64 = 6;

/// `amount` written as the `MonetizationAmountValue` this service reports: the
/// decimal it comes to, at [`MONETIZATION_AMOUNT_SCALE`] fractional digits.
///
/// A quantity finer than the currency's smallest unit cannot be written at that
/// scale, so it is rounded to the nearest unit (halves away from zero, the way
/// money is rounded) rather than truncated — THIS EMULATOR'S CHOICE: the model
/// states no rule, and an on-chain USDC amount is a whole number of those units,
/// so nothing finer arises.
pub fn monetization_amount_value(amount: &::bigdecimal::BigDecimal) -> String {
    let scale = usize::try_from(MONETIZATION_AMOUNT_SCALE).unwrap_or_default();
    // At that scale the value is a whole number of the currency's smallest units,
    // so the digits of that whole number, zero-padded to cover the fraction, are
    // the amount's digits: the point goes `scale` digits from the right. Rendering
    // it this way rather than through the decimal's own `Display` is what writes
    // the trailing zeros a quantity shorter than the scale does not carry.
    let units = amount
        .with_scale_round(
            MONETIZATION_AMOUNT_SCALE,
            ::bigdecimal::RoundingMode::HalfUp,
        )
        .as_bigint_and_exponent()
        .0
        .to_string();
    let (sign, digits) = match units.strip_prefix('-') {
        Some(magnitude) => ("-", magnitude),
        None => ("", units.as_str()),
    };
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let point = padded.len() - scale;
    format!("{sign}{}.{}", &padded[..point], &padded[point..])
}

/// `amount` re-read and written back in the one form this service reports a
/// `MonetizationAmountValue` in. See [`monetization_amount_value`].
pub fn monetization_amount_reported(amount: &str) -> String {
    monetization_amount_value(&monetization_amount(amount))
}

/// The sum of `amounts`, as the `MonetizationAmountValue` a revenue figure is
/// reported in. The sum of no amount is zero, the figure a window that settled
/// nothing reports. See [`monetization_amount`] and
/// [`monetization_amount_value`].
pub fn monetization_amount_total<'a>(amounts: impl IntoIterator<Item = &'a str>) -> String {
    monetization_amount_value(
        &amounts
            .into_iter()
            .map(monetization_amount)
            .fold(::bigdecimal::BigDecimal::from(0), |total, amount| {
                total + amount
            }),
    )
}

/// What this service sorts a settlement listing by when the request names no
/// `SortBy`.
///
/// THIS EMULATOR'S CHOICE, not a modelled default: the model documents a default
/// for neither `SortBy` nor `SortOrder`. A settlement listing is a transaction
/// log, so the newest transaction is reported first — `TIMESTAMP`, descending.
pub const DEFAULT_SETTLEMENT_SORT_BY: &str = "TIMESTAMP";

/// `SortOrder`'s descending value
/// (`smithy_query(query="shape",shape="SortOrder")` → `ASC`, `DESC`).
pub const SORT_ORDER_DESC: &str = "DESC";

/// The direction this service sorts a settlement listing in when the request
/// names none — its own choice, like [`DEFAULT_SETTLEMENT_SORT_BY`].
pub const DEFAULT_SETTLEMENT_SORT_ORDER: &str = SORT_ORDER_DESC;

/// What an ARN in [`ComAmazonawsWafv2Awswaf20190729State::arn_index`] resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A web ACL, by `(Scope, Id)`.
    WebAcl {
        /// The scope the web ACL lives in.
        scope: String,
        /// The web ACL's id.
        id: String,
    },
    /// An IP set, by `(Scope, Id)`.
    IpSet {
        /// The scope the IP set lives in.
        scope: String,
        /// The IP set's id.
        id: String,
    },
    /// A regex pattern set, by `(Scope, Id)`.
    ///
    /// `CreateRegexPatternSet` indexes every set it mints under this variant, so a
    /// `regexpatternset` ARN resolves to the pattern set it names and to nothing
    /// else — the same stance the other variants take (see
    /// [`Self::RuleGroup`]).
    RegexPatternSet {
        /// The scope the regex pattern set lives in.
        scope: String,
        /// The regex pattern set's id.
        id: String,
    },
    /// A rule group, by `(Scope, Id)`.
    ///
    /// `CreateRuleGroup` indexes every rule group it mints under this variant, so a
    /// `rulegroup` ARN resolves to the rule group it names and to nothing else: the
    /// index resolves an ARN to its resource KIND as well as to its identity, so a
    /// reference of one kind is never mistaken for a resource of another. See
    /// [`ComAmazonawsWafv2Awswaf20190729State::holds_rule_group`].
    RuleGroup {
        /// The scope the rule group lives in.
        scope: String,
        /// The rule group's id.
        id: String,
    },
}

/// Per-service state for com-amazonaws-wafv2-awswaf-20190729. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafv2Awswaf20190729State {
    /// Web ACLs by `(Scope, Id)`.
    pub web_acls: HashMap<(String, String), WebAclEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub web_acl_names: HashMap<(String, String), String>,
    /// IP sets by `(Scope, Id)`.
    pub ip_sets: HashMap<(String, String), IpSetEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub ip_set_names: HashMap<(String, String), String>,
    /// Regex pattern sets by `(Scope, Id)`.
    pub regex_pattern_sets: HashMap<(String, String), RegexPatternSetEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub regex_pattern_set_names: HashMap<(String, String), String>,
    /// Rule groups by `(Scope, Id)`.
    pub rule_groups: HashMap<(String, String), RuleGroupEntry>,
    /// `(Scope, Name)` to `Id`, for rule groups: duplicate-name detection and
    /// Name+Id lookup.
    pub rule_group_names: HashMap<(String, String), String>,
    /// Managed rule sets by `(Scope, Id)`. See [`ManagedRuleSetEntry`].
    pub managed_rule_sets: HashMap<(String, String), ManagedRuleSetEntry>,
    /// The mobile SDK releases this account holds, by
    /// `(Platform, ReleaseVersion)`.
    ///
    /// An entry stores the MODELLED `MobileSdkRelease` the catalogue publishes, so
    /// `GetMobileSdkRelease` hands back exactly what was published and a listing
    /// summarises the same record. Empty for an account that was never granted the
    /// mobile SDK, which is what every mobile-SDK operation's entitlement gate
    /// reads (see
    /// [`ComAmazonawsWafv2Awswaf20190729State::uses_mobile_sdk`]).
    pub mobile_sdk_releases: HashMap<(String, String), MobileSdkRelease>,
    /// The logging configuration of each web ACL, by
    /// `(LoggingConfiguration$ResourceArn, LogScope)`.
    ///
    /// "You can define one logging destination per web ACL"
    /// (`LoggingConfiguration`'s own doc), so a web ACL carries at most one
    /// configuration per `LogScope` — the member that says who owns the
    /// configuration ("the owner of the logging configuration, which must be set to
    /// CUSTOMER for the configurations that you manage") — and a request that names
    /// no `LogScope` addresses the customer-owned one, the member's documented
    /// default ([`DEFAULT_LOG_SCOPE`]). Keyed by the web ACL's ARN because that is
    /// how every logging operation addresses it, and a logging configuration has no
    /// identity of its own.
    pub logging_configurations: HashMap<(String, String), LoggingConfiguration>,
    /// API keys by `(Scope, APIKey)`: the key value is the key's identity within
    /// its scope. See [`ApiKeyEntry`].
    pub api_keys: HashMap<(String, String), ApiKeyEntry>,
    /// How many API keys this service has minted, over every scope: the creation
    /// sequence [`ApiKeyEntry::cursor`] is rendered from.
    pub api_keys_minted: u64,
    /// Every resource ARN this service minted, so a by-ARN lookup resolves from
    /// state instead of parsing the ARN back apart.
    pub arn_index: HashMap<String, ResourceRef>,
    /// The requests each web ACL has evaluated, by `(Scope, Id)`: the log
    /// `GetTopPathStatisticsByTraffic` aggregates. See [`BotRequestRecord`].
    pub bot_requests: HashMap<(String, String), Vec<BotRequestRecord>>,
    /// The settlement transactions recorded in each scope, in the order they were
    /// settled: the ledger a monetization query reads. A transaction belongs to a
    /// SCOPE rather than to one web ACL — the web ACL it was settled for is a
    /// member of the record and a filter over it — so the ledger is keyed by scope
    /// alone. See [`SettlementRecordEntry`].
    pub settlement_records: HashMap<String, Vec<SettlementRecordEntry>>,
    /// The live web ACL association of every protected resource: the resource's
    /// own ARN → the ARN of the web ACL that is in front of it.
    ///
    /// Keyed by the RESOURCE, because a protected resource has at most one web
    /// ACL in front of it — `GetWebACLForResource` answers "the web ACL that is
    /// associated with the resource", one shape and not a list — while one web ACL
    /// protects any number of resources, which is the direction
    /// `ListResourcesForWebACL` reports. A protected resource is NOT a wafv2
    /// resource: it belongs to another service (see [`PROTECTED_RESOURCE_FORMS`]),
    /// so it is held by ARN and is not in [`Self::arn_index`].
    pub web_acl_associations: HashMap<String, String>,
    /// The requests each web ACL has EVALUATED at the edge, by `(Scope, Id)`: the
    /// population `GetSampledRequests` draws its sample from.
    ///
    /// Written by the data plane
    /// ([`crate::service::edge::decide`]) for every request a web ACL decided whose
    /// deciding dimension has `SampledRequestsEnabled`, and bounded at
    /// [`crate::service::edge::SAMPLE_POPULATION_CAP`] records per web ACL — the
    /// figure `GetSampledRequests` documents its population as ("the first 5,000
    /// requests that your Amazon Web Services resource received"). A web ACL that
    /// has decided nothing holds none, which is what a sample over a quiet window
    /// answers.
    pub sampled_requests: HashMap<(String, String), Vec<crate::service::edge::SampledRequestRecord>>,
    /// How many requests each rate-based aggregation instance was sent, by the second
    /// they arrived in and oldest first.
    ///
    /// Written by the data plane for every request a rate-based statement counted,
    /// and read by it and by `GetRateBasedStatementManagedKeys` to answer which
    /// instances are over their rule's `Limit` across its `EvaluationWindowSec`. See
    /// [`crate::service::edge::RateInstanceKey`] for why the key is the whole
    /// (web ACL, rule, aggregation instance) triple, and
    /// [`crate::service::edge::RateBucket`] for why the counts are kept per second.
    pub rate_based_counts:
        HashMap<crate::service::edge::RateInstanceKey, Vec<crate::service::edge::RateBucket>>,
}

impl ComAmazonawsWafv2Awswaf20190729State {
    /// Store an API key under `scope`, with the next creation cursor, and answer
    /// the entry as it was stored — or `None` when the scope already holds
    /// [`MAX_API_KEYS_PER_SCOPE`] of them, which is the refusal a minting
    /// operation reports as its modelled `WAFLimitsExceededException`.
    ///
    /// A key value is the key's identity within its scope, so storing one the
    /// scope ALREADY holds is idempotent: the stored entry is answered unchanged,
    /// keeping its creation instant and its place in the listing order rather than
    /// being re-sequenced to the end.
    ///
    /// The cursor bookkeeping and the bound live here rather than in a handler so
    /// that every operation that mints a key gets a position in the listing order
    /// without a listing having to invent one, and so no caller can grow this map
    /// without limit.
    pub fn insert_api_key(
        &mut self,
        scope: &str,
        api_key: &str,
        token_domains: Vec<String>,
        created_at_millis: i64,
    ) -> Option<ApiKeyEntry> {
        let key = (scope.to_string(), api_key.to_string());
        if let Some(stored) = self.api_keys.get(&key) {
            return Some(stored.clone());
        }
        if self.api_keys_count_in_scope(scope) >= MAX_API_KEYS_PER_SCOPE {
            return None;
        }
        self.api_keys_minted = self.api_keys_minted.saturating_add(1);
        let entry = ApiKeyEntry {
            scope: scope.to_string(),
            api_key: api_key.to_string(),
            token_domains,
            created_at_millis,
            // Nothing here re-versions a stored key, so a key keeps the version it
            // was minted at; see [`MINTED_API_KEY_VERSION`].
            version: MINTED_API_KEY_VERSION,
            cursor: api_key_cursor(self.api_keys_minted),
        };
        self.api_keys.insert(key, entry.clone());
        Some(entry)
    }

    /// The API key `api_key` stored under `scope`, or `None` when the scope holds
    /// no key with that value.
    ///
    /// A key value is the key's identity within its scope, and a scope is a
    /// namespace, so the key value of one scope names no key of the other.
    pub fn api_key(&self, scope: &str, api_key: &str) -> Option<&ApiKeyEntry> {
        self.api_keys.get(&(scope.to_string(), api_key.to_string()))
    }

    /// The API key `scope` holds for the token-domain set `token_domains`, or
    /// `None` when it holds none for that set.
    ///
    /// An API key "contains a set of token domains", and the key a client copies
    /// into its JavaScript CAPTCHA integration is the key of that set: a scope holds
    /// at most ONE key per domain set, so a mint for a set the scope already holds a
    /// key for answers the stored key instead of a second one. `token_domains` is
    /// compared as the set it is ([`normalized_token_domains`], the order every
    /// record is written in), so the same domains in any order name the same key,
    /// and a scope is a namespace, so a set held in one scope names no key of the
    /// other.
    pub fn api_key_for_token_domains(
        &self,
        scope: &str,
        token_domains: &[String],
    ) -> Option<&ApiKeyEntry> {
        let wanted = normalized_token_domains(token_domains);
        // The earliest-created match, so the answer is the one key of the set
        // whatever order the underlying map happens to enumerate.
        self.api_keys
            .values()
            .filter(|entry| {
                entry.scope == scope && normalized_token_domains(&entry.token_domains) == wanted
            })
            .min_by(|left, right| left.cursor.cmp(&right.cursor))
    }

    /// Remove the API key `api_key` from `scope` and answer the entry as it was
    /// stored, or `None` when the scope holds no key with that value.
    ///
    /// A key value is the key's identity within its scope, and a scope is a
    /// namespace, so a delete in one scope never removes a key of the other. The
    /// creation sequence ([`Self::api_keys_minted`]) is NOT rewound: the cursors
    /// the remaining keys were minted with keep their order, and a key minted
    /// after a delete still sorts after them.
    pub fn remove_api_key(&mut self, scope: &str, api_key: &str) -> Option<ApiKeyEntry> {
        self.api_keys
            .remove(&(scope.to_string(), api_key.to_string()))
    }

    /// How many API keys this scope already holds. See
    /// [`MAX_API_KEYS_PER_SCOPE`].
    pub fn api_keys_count_in_scope(&self, scope: &str) -> usize {
        self.api_keys
            .values()
            .filter(|entry| entry.scope == scope)
            .count()
    }

    /// Every API key stored under `scope`, in the order they were created.
    ///
    /// A scope is a namespace, so a listing of one scope never reports a key of
    /// the other, and the order is the cursor order a marker resumes from (see
    /// [`api_key_cursor`]).
    pub fn api_keys_in_scope(&self, scope: &str) -> Vec<&ApiKeyEntry> {
        let mut entries: Vec<&ApiKeyEntry> = self
            .api_keys
            .values()
            .filter(|entry| entry.scope == scope)
            .collect();
        entries.sort_by(|left, right| left.cursor.cmp(&right.cursor));
        entries
    }

    /// The web ACL stored under `(scope, id)`, but only when it is also the one
    /// called `name`. WAFv2's Get/Update/Delete take Name, Scope AND Id together,
    /// and a triple that does not describe one stored resource names no resource
    /// at all.
    pub fn web_acl(&self, scope: &str, name: &str, id: &str) -> Option<&WebAclEntry> {
        let entry = self.web_acls.get(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// The requests recorded for the web ACL `(scope, id)`, in whatever order they
    /// were appended — the aggregation over them is order-independent. Empty for a
    /// web ACL that has evaluated none, which is every web ACL this emulator holds
    /// (see [`BotRequestRecord`]).
    pub fn bot_requests(&self, scope: &str, id: &str) -> &[BotRequestRecord] {
        self.bot_requests
            .get(&(scope.to_string(), id.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The settlement transactions recorded in `scope`, in the order they were
    /// settled — the order a pagination marker names a position in. Empty for a
    /// scope that settled none, which is every scope this emulator holds (see
    /// [`SettlementRecordEntry`]).
    pub fn settlement_records(&self, scope: &str) -> &[SettlementRecordEntry] {
        self.settlement_records
            .get(scope)
            .map_or(&[], Vec::as_slice)
    }

    /// [`Self::web_acl`], mutably.
    pub fn web_acl_mut(&mut self, scope: &str, name: &str, id: &str) -> Option<&mut WebAclEntry> {
        let entry = self.web_acls.get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// The IP set stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn ip_set(&self, scope: &str, name: &str, id: &str) -> Option<&IpSetEntry> {
        let entry = self.ip_sets.get(&(scope.to_string(), id.to_string()))?;
        (entry.ip_set.name == name).then_some(entry)
    }

    /// [`Self::ip_set`], mutably.
    pub fn ip_set_mut(&mut self, scope: &str, name: &str, id: &str) -> Option<&mut IpSetEntry> {
        let entry = self.ip_sets.get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.ip_set.name == name).then_some(entry)
    }

    /// Drop a web ACL and every index entry that pointed at it.
    ///
    /// A logging configuration belongs to the web ACL its `ResourceArn` names and
    /// has no identity of its own, so it goes with the web ACL: a configuration left
    /// behind would be one a listing of the scope could not report (it resolves the
    /// ARN to a web ACL) while a read of the dead ARN still answered it, and it would
    /// attach itself to the next web ACL minted under the same ARN.
    pub fn remove_web_acl(&mut self, scope: &str, id: &str) -> Option<WebAclEntry> {
        let entry = self.web_acls.remove(&(scope.to_string(), id.to_string()))?;
        self.web_acl_names
            .remove(&(entry.scope.clone(), entry.web_acl.name.clone()));
        self.arn_index.remove(&entry.web_acl.arn);
        self.logging_configurations
            .retain(|(resource_arn, _), _| resource_arn != &entry.web_acl.arn);
        Some(entry)
    }

    /// Drop an IP set and every index entry that pointed at it.
    pub fn remove_ip_set(&mut self, scope: &str, id: &str) -> Option<IpSetEntry> {
        let entry = self.ip_sets.remove(&(scope.to_string(), id.to_string()))?;
        self.ip_set_names
            .remove(&(entry.scope.clone(), entry.ip_set.name.clone()));
        self.arn_index.remove(&entry.ip_set.arn);
        Some(entry)
    }

    /// Drop a rule group and every index entry that pointed at it.
    pub fn remove_rule_group(&mut self, scope: &str, id: &str) -> Option<RuleGroupEntry> {
        let entry = self.rule_groups.remove(&(scope.to_string(), id.to_string()))?;
        self.rule_group_names
            .remove(&(entry.scope.clone(), entry.rule_group.name.clone()));
        self.arn_index.remove(&entry.rule_group.arn);
        Some(entry)
    }

    /// Drop a regex pattern set and every index entry that pointed at it.
    ///
    /// `RegexPatternSet$Name` and `$ARN` are modelled as optional, so an entry
    /// whose stored shape carries neither leaves no index key behind to remove.
    pub fn remove_regex_pattern_set(
        &mut self,
        scope: &str,
        id: &str,
    ) -> Option<RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .remove(&(scope.to_string(), id.to_string()))?;
        if let Some(name) = entry.regex_pattern_set.name.clone() {
            self.regex_pattern_set_names
                .remove(&(entry.scope.clone(), name));
        }
        if let Some(arn) = entry.regex_pattern_set.arn.as_deref() {
            self.arn_index.remove(arn);
        }
        Some(entry)
    }

    /// How many web ACLs this scope already holds. See [`MAX_WEB_ACLS_PER_SCOPE`].
    pub fn web_acls_in_scope(&self, scope: &str) -> usize {
        self.web_acls
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// How many IP sets this scope already holds. See [`MAX_IP_SETS_PER_SCOPE`].
    pub fn ip_sets_in_scope(&self, scope: &str) -> usize {
        self.ip_sets
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// The regex pattern set stored under `(scope, id)` when it is also the one
    /// called `name`. See [`Self::web_acl`].
    pub fn regex_pattern_set(
        &self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .get(&(scope.to_string(), id.to_string()))?;
        (entry.regex_pattern_set.name.as_deref() == Some(name)).then_some(entry)
    }

    /// [`Self::regex_pattern_set`], mutably.
    pub fn regex_pattern_set_mut(
        &mut self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&mut RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.regex_pattern_set.name.as_deref() == Some(name)).then_some(entry)
    }

    /// How many regex pattern sets this scope already holds. See
    /// [`MAX_REGEX_PATTERN_SETS_PER_SCOPE`].
    pub fn regex_pattern_sets_in_scope(&self, scope: &str) -> usize {
        self.regex_pattern_sets
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// The rule group stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn rule_group(&self, scope: &str, name: &str, id: &str) -> Option<&RuleGroupEntry> {
        let entry = self.rule_groups.get(&(scope.to_string(), id.to_string()))?;
        (entry.rule_group.name == name).then_some(entry)
    }

    /// [`Self::rule_group`], mutably.
    pub fn rule_group_mut(
        &mut self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&mut RuleGroupEntry> {
        let entry = self
            .rule_groups
            .get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.rule_group.name == name).then_some(entry)
    }

    /// How many rule groups this scope already holds. See
    /// [`MAX_RULE_GROUPS_PER_SCOPE`].
    pub fn rule_groups_in_scope(&self, scope: &str) -> usize {
        self.rule_groups
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// Every rule group ARN this scope holds, with the group's declared
    /// `Capacity`.
    ///
    /// This is what prices a `RuleGroupReferenceStatement` in
    /// [`web_acl_capacity`]: `RuleGroup$Capacity` is "the web ACL capacity units
    /// (WCUs) required for this rule group", fixed at creation, so a web ACL rule
    /// that runs a rule group consumes exactly that group's declared capacity. It
    /// is read out of state BEFORE a save writes through it, so a handler can hold
    /// it while it borrows the entry it is about to update.
    pub fn rule_group_capacities(&self, scope: &str) -> HashMap<String, i64> {
        self.rule_groups
            .values()
            .filter(|entry| entry.scope == scope)
            .map(|entry| (entry.rule_group.arn.clone(), entry.rule_group.capacity))
            .collect()
    }

    /// Why a reference in `rules` cannot be resolved, if any cannot.
    ///
    /// WAF resolves a rule statement's reference when the rule is SAVED: a
    /// reference statement names another entity ("the Amazon Resource Name (ARN) of
    /// the IPSet that this statement references", `IPSetReferenceStatement$ARN`),
    /// and a save naming an entity that does not exist is refused with the modelled
    /// `WAFNonexistentItemException` ("WAF couldn’t perform the operation because
    /// your resource doesn't exist") rather than stored for a later `Get` to hand
    /// back as though the reference were live. This is the ingress counterpart of
    /// [`Self::web_acl_referencing`]: the same reference keeps an entity from
    /// being deleted and keeps a rule that names a missing one from being stored.
    ///
    /// Each of the model's four reference-bearing statements is resolved against
    /// the entities that KIND of reference names. An `IPSetReferenceStatement`, a
    /// `RuleGroupReferenceStatement` and a `RegexPatternSetReferenceStatement` name
    /// resources of this account, so they resolve
    /// through [`Self::arn_index`], each only to a resource of its own kind in the
    /// SAME scope, because a scope is a namespace. A `ManagedRuleGroupStatement`
    /// names no resource of this account at all — it names a group its VENDOR
    /// publishes, identified by `(VendorName, Name)`, and optionally one VERSION of
    /// that group — so both resolve against [`MANAGED_RULE_GROUP_CATALOGUE`], the
    /// catalogue this service publishes: a pair the catalogue does not list, and a
    /// version no release of that group ever carried, name nothing that exists.
    /// The `Some` it answers is the DIAGNOSTIC for the refusal's
    /// audit record — the response itself carries [`NONEXISTENT_ITEM_MESSAGE`], the
    /// constant sentence that exception type always carries. That is also what keeps
    /// [`web_acl_capacity`] from pricing a statement whose cost it cannot know: a
    /// stored rule list can only carry a reference to a rule group this service
    /// holds, whose declared `Capacity` is exactly that statement's cost, or to a
    /// catalogued managed rule group, whose published capacity is its cost.
    ///
    /// Walked with an explicit worklist, like [`validate_statement_nesting`]: the
    /// tree is supplied by the request.
    pub fn unresolvable_rule_reference(&self, scope: &str, rules: &[Rule]) -> Option<String> {
        let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
        while let Some(statement) = pending.pop() {
            if let Some(reference) = statement.ip_set_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::IpSet {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No IPSet with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::RuleGroup {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No RuleGroup with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.regex_pattern_set_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::RegexPatternSet {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No RegexPatternSet with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(managed) = statement.managed_rule_group_statement.as_ref() {
                match managed_rule_group(&managed.vendor_name, &managed.name) {
                    None => {
                        return Some(format!(
                            "No managed rule group named {:?} from vendor {:?} is available",
                            managed.name, managed.vendor_name
                        ));
                    }
                    // A statement that PINS a version names one more thing the
                    // vendor publishes, so it resolves too: "the version of the
                    // managed rule group to use… You can retrieve the available
                    // versions for the managed rule group by calling
                    // `ListAvailableManagedRuleGroupVersions`"
                    // (`ManagedRuleGroupStatement$Version`), and a name that
                    // listing does not carry names no rule set WAF could run. A
                    // version the vendor published and has since RETIRED is its own
                    // modelled refusal, judged before this one
                    // ([`expired_managed_rule_group_version`]), so what is left here
                    // is a name no version of the group ever carried.
                    Some(group) => {
                        if let Some(version) = managed
                            .version
                            .as_deref()
                            .filter(|version| {
                                group.version_state(version) == ManagedVersionState::Unknown
                            })
                        {
                            return Some(format!(
                                "No version {version:?} of the managed rule group named {:?} \
                                 from vendor {:?} is published",
                                managed.name, managed.vendor_name
                            ));
                        }
                    }
                }
            }
            pending.extend(nested_statements(statement));
        }
        None
    }

    /// `true` when `arn` names a rule group this service holds.
    ///
    /// WAF resolves a published version's `AssociatedRuleGroupArn` against the
    /// vendor's own rule groups, so this is what
    /// `PutManagedRuleSetVersions` consults before it judges anything else about
    /// the request. The ARN must be a well-formed wafv2 ARN whose resource kind is
    /// `rulegroup` AND be indexed as one: an ARN of another kind, or one naming a
    /// rule group that was never created, resolves to nothing.
    ///
    /// A rule group is indexed under [`ResourceRef::RuleGroup`] by
    /// `CreateRuleGroup`, so this answers `true` for a rule group this service
    /// holds and `false` for an ARN of another kind or one naming a rule group that
    /// was never created. That is the same stance
    /// [`Self::unresolvable_rule_reference`] takes for a
    /// `RuleGroupReferenceStatement`: a reference WAF cannot resolve is refused
    /// rather than stored as though it were live.
    pub fn holds_rule_group(&self, arn: &str) -> bool {
        arn_resource_kind(arn) == Some(RULE_GROUP_ARN_KIND)
            && matches!(self.arn_index.get(arn), Some(ResourceRef::RuleGroup { .. }))
    }

    /// The rule group `arn` names, or `None` when it names no rule group this
    /// service holds.
    ///
    /// The by-ARN counterpart of [`Self::rule_group`], resolving through
    /// [`Self::arn_index`] exactly as [`Self::web_acl_by_arn`] does, so an ARN of
    /// another kind and an ARN that was never minted here both name nothing — and so
    /// does a rule group ARN carrying another Region, since the index holds each
    /// resource under the one ARN it was minted with.
    pub fn rule_group_by_arn(&self, arn: &str) -> Option<&RuleGroupEntry> {
        match self.arn_index.get(arn)? {
            ResourceRef::RuleGroup { scope, id } => {
                self.rule_groups.get(&(scope.clone(), id.clone()))
            }
            ResourceRef::WebAcl { .. }
            | ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. } => None,
        }
    }

    /// [`Self::rule_group_by_arn`], for a caller that writes to the entry.
    pub fn rule_group_by_arn_mut(&mut self, arn: &str) -> Option<&mut RuleGroupEntry> {
        let (scope, id) = match self.arn_index.get(arn)? {
            ResourceRef::RuleGroup { scope, id } => (scope.clone(), id.clone()),
            ResourceRef::WebAcl { .. }
            | ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. } => return None,
        };
        self.rule_groups.get_mut(&(scope, id))
    }

    /// The live web ACL whose rule tree references `arn`, if any.
    ///
    /// A resource that a web ACL still references cannot be deleted, so this is
    /// what a delete consults, whatever KIND of resource is being deleted: an ARN
    /// identifies one resource of one kind, so the walk asks only whether any
    /// reference statement names this ARN. The walk itself is
    /// [`rules_reference_arn`], shared with [`Self::rule_group_referencing`] so one
    /// change to it cannot drift between the two kinds of entity that store rules.
    pub fn web_acl_referencing(&self, arn: &str) -> Option<&WebAclEntry> {
        self.web_acls
            .values()
            .find(|entry| rules_reference_arn(entry.web_acl.rules.as_deref(), arn))
    }

    /// The live rule group whose rule tree references `arn`, if any.
    ///
    /// The rule-group counterpart of [`Self::web_acl_referencing`]: `Rule` is "a
    /// single rule, which you can use in a WebACL **or a RuleGroup**", so the very
    /// same reference statements sit in a rule group's rules, and a resource one of
    /// them names is in use by that rule group. The same walk answers both.
    pub fn rule_group_referencing(&self, arn: &str) -> Option<&RuleGroupEntry> {
        self.rule_groups
            .values()
            .find(|entry| rules_reference_arn(entry.rule_group.rules.as_deref(), arn))
    }

    /// How `arn` is in use by a stored rule tree, worded for a refusal's audit
    /// record, or `None` when nothing references it.
    ///
    /// What a delete consults: a resource is "being used by another resource" as
    /// soon as ANY stored rule tree names it, and this model stores rule trees in
    /// two kinds of entity — a web ACL and a rule group — so both are asked. One
    /// place, so every delete that has to refuse a resource in use asks the same
    /// question of the same set of rule trees.
    pub fn rule_tree_referencing(&self, arn: &str) -> Option<String> {
        if let Some(entry) = self.web_acl_referencing(arn) {
            return Some(format!("WebACL {:?}", entry.web_acl.name));
        }
        self.rule_group_referencing(arn)
            .map(|entry| format!("RuleGroup {:?}", entry.rule_group.name))
    }

    /// The web ACL `arn` names, or `None` when it names no web ACL this service
    /// holds.
    ///
    /// The index resolves an ARN to its resource KIND as well as to its identity,
    /// so an ARN of another kind — an IP set, a rule group, a regex pattern set —
    /// names no web ACL, and neither does an ARN that was never minted here. That
    /// is the same stance [`Self::holds_rule_group`] takes.
    pub fn web_acl_by_arn(&self, arn: &str) -> Option<&WebAclEntry> {
        match self.arn_index.get(arn)? {
            ResourceRef::WebAcl { scope, id } => self.web_acls.get(&(scope.clone(), id.clone())),
            ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. }
            | ResourceRef::RuleGroup { .. } => None,
        }
    }

    /// Put the web ACL `web_acl_arn` in front of the resource `resource_arn`, and
    /// answer whether the binding was stored.
    ///
    /// A resource has at most ONE web ACL in front of it, so binding a resource
    /// that is already protected REPLACES its binding rather than adding a second
    /// one, and re-binding the resource to the web ACL it already carries changes
    /// nothing. `false` means the web ACL already protects
    /// [`MAX_PROTECTED_RESOURCES_PER_WEB_ACL`] resources, which is the refusal a
    /// binding operation reports as its modelled `WAFLimitsExceededException`.
    pub fn associate_web_acl(&mut self, resource_arn: &str, web_acl_arn: &str) -> bool {
        let already_bound =
            self.web_acl_associations.get(resource_arn).map(String::as_str) == Some(web_acl_arn);
        if !already_bound
            && self.protected_resources(web_acl_arn).len() >= MAX_PROTECTED_RESOURCES_PER_WEB_ACL
        {
            return false;
        }
        self.web_acl_associations
            .insert(resource_arn.to_string(), web_acl_arn.to_string());
        true
    }

    /// Every resource ARN the web ACL `web_acl_arn` is in front of, in ARN order
    /// so one listing of one web ACL always reports them the same way round.
    pub fn protected_resources(&self, web_acl_arn: &str) -> Vec<&str> {
        let mut arns: Vec<&str> = self
            .web_acl_associations
            .iter()
            .filter(|(_, bound)| bound.as_str() == web_acl_arn)
            .map(|(resource, _)| resource.as_str())
            .collect();
        arns.sort_unstable();
        arns
    }

    /// The web ACL in front of `resource_arn`, or `None` when the resource carries
    /// no association.
    pub fn web_acl_for_resource(&self, resource_arn: &str) -> Option<&WebAclEntry> {
        let web_acl_arn = self.web_acl_associations.get(resource_arn)?;
        self.web_acl_by_arn(web_acl_arn)
    }

    /// Take the web ACL away from `resource_arn`, and answer whether one was there
    /// to take away.
    pub fn disassociate_web_acl(&mut self, resource_arn: &str) -> bool {
        self.web_acl_associations.remove(resource_arn).is_some()
    }

    /// The managed rule set stored under `(scope, id)` when it is also the one
    /// called `name`. See [`Self::web_acl`].
    pub fn managed_rule_set(
        &self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&ManagedRuleSetEntry> {
        let entry = self
            .managed_rule_sets
            .get(&(scope.to_string(), id.to_string()))?;
        (entry.managed_rule_set.name == name).then_some(entry)
    }

    /// [`Self::managed_rule_set`], mutably.
    pub fn managed_rule_set_mut(
        &mut self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&mut ManagedRuleSetEntry> {
        let entry = self
            .managed_rule_sets
            .get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.managed_rule_set.name == name).then_some(entry)
    }

    /// Provision a managed rule set for the vendor `vendor_name` in `scope`, and
    /// answer it as stored.
    ///
    /// The one way a managed rule set comes into being here, because no operation of
    /// this model creates one (see [`ManagedRuleSetEntry`]): a vendor's set, its id
    /// and its first lock token are minted the way every other entity of this
    /// service mints them ([`new_uuid`]), and its ARN by [`resource_arn`] under
    /// [`MANAGED_RULE_SET_ARN_KIND`]. `PublishedVersions` starts empty — a set is
    /// provisioned before its vendor publishes any version, which is what
    /// `PutManagedRuleSetVersions` then does.
    pub fn insert_managed_rule_set(
        &mut self,
        scope: &str,
        vendor_name: &str,
        name: &str,
        request_region: &str,
        description: Option<String>,
    ) -> ManagedRuleSetEntry {
        let id = new_uuid();
        let entry = ManagedRuleSetEntry {
            scope: scope.to_string(),
            managed_rule_set: ManagedRuleSet {
                name: name.to_string(),
                id: id.clone(),
                arn: resource_arn(
                    scope,
                    request_region,
                    MANAGED_RULE_SET_ARN_KIND,
                    name,
                    &id,
                ),
                description: stored_description(description),
                published_versions: Some(HashMap::new()),
                recommended_version: None,
                // `ManagedRuleSet$LabelNamespace` is "the label namespace prefix for
                // the managed rule groups that are offered to customers from this
                // managed rule set", and the member's own doc gives its syntax as
                // `awswaf:managed:<vendor>:<rule group name>:` — the same form
                // `ManagedRuleGroupDescriptor::managed_label_namespace` renders for a
                // catalogued group, built from the vendor this set belongs to and the
                // name its customers see.
                label_namespace: Some(format!("awswaf:managed:{vendor_name}:{name}:")),
            },
            lock_token: new_uuid(),
        };
        self.managed_rule_sets
            .insert((scope.to_string(), id), entry.clone());
        entry
    }

    /// Every managed rule set of `scope`, ordered by name so a listing enumerates
    /// them the same way round on every call.
    pub fn managed_rule_sets_in_scope(&self, scope: &str) -> Vec<&ManagedRuleSetEntry> {
        let mut entries: Vec<&ManagedRuleSetEntry> = self
            .managed_rule_sets
            .values()
            .filter(|entry| entry.scope == scope)
            .collect();
        entries.sort_by(|left, right| {
            left.managed_rule_set
                .name
                .cmp(&right.managed_rule_set.name)
        });
        entries
    }

    /// The logging configuration of the web ACL `resource_arn` under `log_scope`,
    /// or `None` when the web ACL has none there.
    ///
    /// A request that names no `LogScope` addresses the customer-owned
    /// configuration, the member's documented default
    /// ([`DEFAULT_LOG_SCOPE`]).
    pub fn logging_configuration(
        &self,
        resource_arn: &str,
        log_scope: Option<&str>,
    ) -> Option<&LoggingConfiguration> {
        self.logging_configurations
            .get(&logging_configuration_key(resource_arn, log_scope))
    }

    /// Put `configuration` in front of the web ACL its `ResourceArn` names, and
    /// answer what it replaced.
    ///
    /// A web ACL carries one logging configuration per `LogScope`, so putting one
    /// REPLACES whatever was there rather than adding a second.
    pub fn insert_logging_configuration(
        &mut self,
        configuration: LoggingConfiguration,
    ) -> Option<LoggingConfiguration> {
        let key = logging_configuration_key(
            &configuration.resource_arn,
            configuration.log_scope.as_deref(),
        );
        self.logging_configurations.insert(key, configuration)
    }

    /// Take the logging configuration of `resource_arn` under `log_scope` away, and
    /// answer the one that was there, if any.
    pub fn remove_logging_configuration(
        &mut self,
        resource_arn: &str,
        log_scope: Option<&str>,
    ) -> Option<LoggingConfiguration> {
        self.logging_configurations
            .remove(&logging_configuration_key(resource_arn, log_scope))
    }

    /// The tags the resource `arn` carries, or `None` when it names no taggable
    /// resource this service holds.
    ///
    /// "You can tag the Amazon Web Services resources that you manage through WAF:
    /// web ACLs, rule groups, IP sets, and regex pattern sets" (`TagResource`'s own
    /// doc) — the four kinds [`ResourceRef`] resolves — and every tagging operation
    /// addresses one by its ARN alone, so the resolution lives here once instead of
    /// in each of them.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&Vec<Tag>> {
        match self.arn_index.get(arn)? {
            ResourceRef::WebAcl { scope, id } => self
                .web_acls
                .get(&(scope.clone(), id.clone()))
                .map(|entry| &entry.tags),
            ResourceRef::IpSet { scope, id } => self
                .ip_sets
                .get(&(scope.clone(), id.clone()))
                .map(|entry| &entry.tags),
            ResourceRef::RegexPatternSet { scope, id } => self
                .regex_pattern_sets
                .get(&(scope.clone(), id.clone()))
                .map(|entry| &entry.tags),
            ResourceRef::RuleGroup { scope, id } => self
                .rule_groups
                .get(&(scope.clone(), id.clone()))
                .map(|entry| &entry.tags),
        }
    }

    /// [`Self::tags_by_arn`] for a caller that WRITES the collection, handing back
    /// the resource's identity beside it: the one lookup gives a tagging mutation
    /// both the tags it applies to and the `(Scope, Id)` it records in the audit
    /// trail.
    pub fn taggable_resource_mut(
        &mut self,
        arn: &str,
    ) -> Option<(String, String, &mut Vec<Tag>)> {
        match self.arn_index.get(arn)?.clone() {
            ResourceRef::WebAcl { scope, id } => self
                .web_acls
                .get_mut(&(scope.clone(), id.clone()))
                .map(|entry| (scope, id, &mut entry.tags)),
            ResourceRef::IpSet { scope, id } => self
                .ip_sets
                .get_mut(&(scope.clone(), id.clone()))
                .map(|entry| (scope, id, &mut entry.tags)),
            ResourceRef::RegexPatternSet { scope, id } => self
                .regex_pattern_sets
                .get_mut(&(scope.clone(), id.clone()))
                .map(|entry| (scope, id, &mut entry.tags)),
            ResourceRef::RuleGroup { scope, id } => self
                .rule_groups
                .get_mut(&(scope.clone(), id.clone()))
                .map(|entry| (scope, id, &mut entry.tags)),
        }
    }

    /// Provision one release of the mobile SDK for `platform`, and answer it as it
    /// was stored.
    ///
    /// The seam that gives an account ACCESS to the mobile SDK (see
    /// [`Self::uses_mobile_sdk`]): "the mobile SDK is not generally available"
    /// (every mobile-SDK operation's own doc), and no operation of this model
    /// publishes a release — a release catalogue appears for an account because WAF
    /// granted that account the mobile SDK, which is not an API call. So an account
    /// holds no release until one is provisioned here, exactly as it holds no
    /// managed rule set until [`Self::insert_managed_rule_set`] provisions one.
    ///
    /// Provisioning a version the platform already holds REPLACES it: a
    /// `(Platform, ReleaseVersion)` names one release.
    pub fn insert_mobile_sdk_release(
        &mut self,
        platform: &str,
        release_version: &str,
        timestamp_millis: i64,
        release_notes: Option<String>,
        tags: Vec<Tag>,
    ) -> MobileSdkRelease {
        let release = MobileSdkRelease {
            release_version: Some(release_version.to_string()),
            timestamp: Some(::aws_smithy_types::DateTime::from_millis(timestamp_millis)),
            release_notes,
            tags: (!tags.is_empty()).then_some(tags),
        };
        self.mobile_sdk_releases.insert(
            (platform.to_string(), release_version.to_string()),
            release.clone(),
        );
        release
    }

    /// `true` when this account may use the mobile SDK.
    ///
    /// One gate for the whole mobile-SDK family, so listing the catalogue, reading
    /// one release and generating a download URL can never disagree about whether
    /// the account is allowed to see it. The mobile SDK is granted per ACCOUNT
    /// rather than per platform, so a granted account may hold releases for one
    /// platform and none for the other — which is an empty listing, not a refusal.
    pub fn uses_mobile_sdk(&self) -> bool {
        !self.mobile_sdk_releases.is_empty()
    }

    /// Every release this account holds for `platform`, ordered by release version
    /// so a listing enumerates them the same way round on every call and a
    /// pagination marker names the same position.
    pub fn mobile_sdk_releases_for(&self, platform: &str) -> Vec<&MobileSdkRelease> {
        let mut releases: Vec<&MobileSdkRelease> = self
            .mobile_sdk_releases
            .iter()
            .filter(|((held, _), _)| held == platform)
            .map(|(_, release)| release)
            .collect();
        releases.sort_by(|left, right| {
            release_version_of(left).cmp(release_version_of(right))
        });
        releases
    }

    /// The release `release_version` of `platform`, or `None` when this account
    /// holds no such release.
    ///
    /// [`LATEST_RELEASE_VERSION`] resolves to the most recently released version of
    /// the platform — "for the latest available version, specify LATEST"
    /// (`GetMobileSdkReleaseRequest$ReleaseVersion` and the same member of
    /// `GenerateMobileSdkReleaseUrlRequest`) — so both operations that take a
    /// version read it the same way.
    pub fn mobile_sdk_release(
        &self,
        platform: &str,
        release_version: &str,
    ) -> Option<&MobileSdkRelease> {
        if release_version == LATEST_RELEASE_VERSION {
            return self.latest_mobile_sdk_release(platform);
        }
        self.mobile_sdk_releases
            .get(&(platform.to_string(), release_version.to_string()))
    }

    /// The most recently released version of `platform`: the greatest release
    /// instant, and the greater version key when two releases carry the same
    /// instant.
    fn latest_mobile_sdk_release(&self, platform: &str) -> Option<&MobileSdkRelease> {
        self.mobile_sdk_releases_for(platform)
            .into_iter()
            .max_by_key(|release| {
                (
                    release
                        .timestamp
                        .as_ref()
                        .map_or(i64::MIN, date_time_millis),
                    release_version_of(release),
                )
            })
    }
}

/// The version key a stored release carries. `MobileSdkRelease$ReleaseVersion` is
/// optional in the model, and every release
/// [`ComAmazonawsWafv2Awswaf20190729State::insert_mobile_sdk_release`] stores
/// carries one, so the absent case orders before every real key instead of
/// panicking.
fn release_version_of(release: &MobileSdkRelease) -> &str {
    release.release_version.as_deref().unwrap_or_default()
}

/// The `ReleaseVersion` value that names a platform's newest release rather than
/// one particular version.
///
/// Source: `GetMobileSdkReleaseRequest$ReleaseVersion`'s own documentation — "The
/// release version. For the latest available version, specify `LATEST`."
pub const LATEST_RELEASE_VERSION: &str = "LATEST";

/// The `LogScope` value a request that names none addresses.
///
/// Source: `LoggingConfiguration$LogScope`'s own documentation — "The owner of the
/// logging configuration, which must be set to `CUSTOMER` for the configurations
/// that you manage. […] Default: `CUSTOMER`".
pub const DEFAULT_LOG_SCOPE: &str = "CUSTOMER";

/// The `LogType` value a request that names none addresses.
///
/// Source: `LoggingConfiguration$LogType`'s own documentation — "Used to
/// distinguish between various logging options. Currently, there is one option. […]
/// Default: `WAF_LOGS`".
pub const DEFAULT_LOG_TYPE: &str = "WAF_LOGS";

/// The resource-kind segment a managed rule set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const MANAGED_RULE_SET_ARN_KIND: &str = "managedruleset";

/// The key a logging configuration is held under: the web ACL's ARN and the
/// `LogScope` the configuration belongs to, defaulted by [`DEFAULT_LOG_SCOPE`].
fn logging_configuration_key(resource_arn: &str, log_scope: Option<&str>) -> (String, String) {
    (
        resource_arn.to_string(),
        log_scope.unwrap_or(DEFAULT_LOG_SCOPE).to_string(),
    )
}

/// The deepest a rule's statement tree may nest.
///
/// Statements nest: `AndStatement` and `OrStatement` over a list, `NotStatement`
/// over one, and the scope-down statement of a rate-based or managed-rule-group
/// statement. WAF publishes a maximum nesting depth for rule statements in its
/// quota list, and the tree comes straight off the wire, so the bound is enforced
/// ON INGRESS by [`validate_statement_nesting`] — before a tree is stored, before
/// its capacity is computed, and so before any later walk sees it.
pub const MAX_STATEMENT_NESTING_DEPTH: u32 = 10;

/// The most web ACLs this service stores per scope: WAF's published quota is 100
/// web ACLs per Region per scope, and reaching it answers the modelled
/// `WAFLimitsExceededException` the way the real service does — which is also what
/// keeps the store from growing without limit.
pub const MAX_WEB_ACLS_PER_SCOPE: usize = 100;

/// The most IP sets this service stores per scope; see [`MAX_WEB_ACLS_PER_SCOPE`].
/// WAF's published quota is 100 IP sets per Region per scope.
pub const MAX_IP_SETS_PER_SCOPE: usize = 100;

/// The most rule groups this service stores per scope; see
/// [`MAX_WEB_ACLS_PER_SCOPE`]. WAF's published quota is 100 rule groups per Region
/// per scope.
pub const MAX_RULE_GROUPS_PER_SCOPE: usize = 100;

/// The most resources one web ACL is put in front of before a binding is refused
/// with the modelled `WAFLimitsExceededException`.
///
/// Unlike [`MAX_WEB_ACLS_PER_SCOPE`] this is NOT a quota WAF publishes: it is this
/// emulator's own bound on
/// [`ComAmazonawsWafv2Awswaf20190729State::web_acl_associations`], whose keys are
/// resource ARNs the caller supplies, so that no caller can grow that map without
/// limit. It is the same size as the other per-scope bounds, and a web ACL at it
/// can still be re-bound to a resource it already protects.
pub const MAX_PROTECTED_RESOURCES_PER_WEB_ACL: usize = 100;

/// The most regex pattern sets this service stores per scope before
/// `CreateRegexPatternSet` answers its modelled `WAFLimitsExceededException`.
///
/// Unlike [`MAX_WEB_ACLS_PER_SCOPE`], [`MAX_IP_SETS_PER_SCOPE`] and
/// [`MAX_RULE_GROUPS_PER_SCOPE`], which carry WAF's published per-Region figure of
/// 100, no figure is cited here, so this bound is CHOSEN — on the same reasoning
/// the sibling waf-regional service states for its own pattern-set bound: every
/// created set is a stored record, so the number a caller can create has to be
/// bounded, and the bound is well above the handful of pattern sets a rule's
/// `RegexPatternSetReferenceStatement`s name while staying small enough for a test
/// to reach it. It is per scope because a scope is a namespace, like every other
/// bound in this module.
pub const MAX_REGEX_PATTERN_SETS_PER_SCOPE: usize = 10;

/// The most API keys this service stores per scope before a minting operation
/// answers its modelled `WAFLimitsExceededException`
/// ([`ComAmazonawsWafv2Awswaf20190729State::insert_api_key`] is what reports the
/// bound being reached).
///
/// Like [`MAX_REGEX_PATTERN_SETS_PER_SCOPE`], and unlike the three bounds above
/// it, no published figure is cited here, so this bound is CHOSEN, on the same
/// reasoning: every minted key is a stored record, so the number a caller can
/// mint has to be bounded, and the bound is well above the handful of keys a
/// client application's CAPTCHA integration defines while staying small enough
/// for a test to reach it. It is per scope because a scope is a namespace, like
/// every other bound in this module.
pub const MAX_API_KEYS_PER_SCOPE: usize = 10;

/// The most token domains ONE API key carries: "You can use a single key for up to
/// 5 domains" (`CreateAPIKey`'s own documentation,
/// `smithy_query(query="doc",shape="CreateAPIKey")`). A request naming more is
/// refused rather than stored, because a key that carries them could not be used
/// for all of them.
pub const MAX_TOKEN_DOMAINS_PER_API_KEY: usize = 5;

/// The most addresses one IP set holds: WAF's published quota is 10,000 IP
/// addresses or ranges per IP set. It bounds how large a single stored entry can
/// be, beside the serve path's own 16 MiB request-body cap.
pub const MAX_IP_SET_ADDRESSES: usize = 10_000;

/// The most versions one managed rule set offers at a time: this store's own bound,
/// not a documented AWS limit.
///
/// `PutManagedRuleSetVersions` adds to the set's `PublishedVersions`, so without a
/// bound a caller could grow one stored entry without limit by publishing fresh
/// version keys. Neither the model nor the operation's documentation states a
/// ceiling for the map, so the figure here is the store's, chosen to match the
/// per-scope entity bounds this service already holds itself to (see
/// [`MAX_WEB_ACLS_PER_SCOPE`]): a put that would take a set past it is refused
/// rather than stored.
pub const MAX_PUBLISHED_VERSIONS_PER_MANAGED_RULE_SET: usize = 100;

// ---------------------------------------------------------------------------
// The managed rule group catalogue
// ---------------------------------------------------------------------------

/// The `VendorName` every rule group AWS itself publishes carries.
///
/// `ManagedRuleGroupStatement$VendorName` is "the name of the managed rule group
/// vendor. You use this, along with the rule group name, to identify a rule
/// group", and the groups in [`MANAGED_RULE_GROUP_CATALOGUE`] are the ones AWS
/// publishes, so they all carry this one vendor name.
pub const AWS_MANAGED_RULE_GROUP_VENDOR: &str = "AWS";

/// The vendor segment the label namespace of an AWS-published managed rule group
/// carries.
///
/// `DescribeManagedRuleGroupResponse$LabelNamespace` states the syntax
/// "awswaf:managed:<vendor>:<rule group name>:", and the vendor publishes which text
/// stands in each of those two places: for the groups AWS itself publishes the vendor
/// segment is this, lower case, and the group segment is the group's published slug
/// ([`ManagedRuleGroupDescriptor::label_slug`]) rather than its API name. Every entry
/// of [`MANAGED_RULE_GROUP_CATALOGUE`] carries [`AWS_MANAGED_RULE_GROUP_VENDOR`], so
/// one constant covers the catalogue.
pub const AWS_MANAGED_RULE_GROUP_LABEL_VENDOR: &str = "aws";

/// The SNS topic every change to an AWS-published managed rule group is announced on.
///
/// `DescribeManagedRuleGroupResponse$SnsTopicArn` is "the ARN of the Amazon Simple
/// Notification Service SNS topic that's used to provide notification of changes to
/// the managed rule group. You can subscribe to the SNS topic to receive
/// notifications when the managed rule group is modified": AWS Managed Rules announce
/// every such change on ONE topic, which the WAF Developer Guide publishes as
/// `arn:aws:sns:us-east-1:<AWS's own account>:aws-managed-waf-rule-notifications` —
/// one address a subscriber uses whatever region it works in, so the ARN varies with
/// neither the group, the scope nor the caller's region. Minted through the emulator's
/// own ARN builder, like every other ARN this service answers, so the account segment
/// is this emulator's.
pub const AWS_MANAGED_RULE_NOTIFICATION_TOPIC: &str = "aws-managed-waf-rule-notifications";

/// The region [`AWS_MANAGED_RULE_NOTIFICATION_TOPIC`] lives in: the one the guide
/// publishes it in, which is [`DEFAULT_REGION`].
pub const AWS_MANAGED_RULE_NOTIFICATION_REGION: &str = DEFAULT_REGION;

/// The statements nested directly inside `statement`, in no particular order.
///
/// One place names every nesting the model has, so the depth validator and both
/// tree walks below agree on what "nested" means.
///
/// [`nested_statements_mut`] is its mutable twin, arm for arm: a nesting the model
/// grows has to be added to BOTH, or a default would be applied on one walk and
/// not the other.
pub(crate) fn nested_statements(statement: &Statement) -> Vec<&Statement> {
    let mut nested: Vec<&Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_ref() {
        nested.extend(and.statements.iter());
    }
    if let Some(or) = statement.or_statement.as_ref() {
        nested.extend(or.statements.iter());
    }
    if let Some(not) = statement.not_statement.as_ref() {
        nested.push(&not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_ref()
        .and_then(|rate| rate.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_ref()
        .and_then(|managed| managed.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    nested
}

/// [`nested_statements`], mutably: the statements nested directly inside
/// `statement`, in no particular order.
///
/// Used by [`apply_rule_defaults`] to reach every statement of a tree in place.
/// Each arm borrows a DIFFERENT field of `statement`, so the borrows are disjoint.
/// Kept arm for arm in step with [`nested_statements`]: a nesting the model grows
/// belongs in both.
fn nested_statements_mut(statement: &mut Statement) -> Vec<&mut Statement> {
    let mut nested: Vec<&mut Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_mut() {
        nested.extend(and.statements.iter_mut());
    }
    if let Some(or) = statement.or_statement.as_mut() {
        nested.extend(or.statements.iter_mut());
    }
    if let Some(not) = statement.not_statement.as_mut() {
        nested.push(&mut not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_mut()
        .and_then(|rate| rate.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_mut()
        .and_then(|managed| managed.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    nested
}

/// Refuse a rule list whose statements nest deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`]. `Err` carries the reason, for the refusal's
/// message.
///
/// Walked with an explicit worklist rather than by recursion: the tree is supplied
/// by the request, so the check that bounds its depth must not itself consume
/// stack in proportion to that depth.
pub fn validate_statement_nesting(rules: &[Rule]) -> Result<(), String> {
    let mut pending: Vec<(&Statement, u32)> = rules
        .iter()
        .map(|rule| (&rule.statement, 1_u32))
        .collect();
    while let Some((statement, depth)) = pending.pop() {
        if depth > MAX_STATEMENT_NESTING_DEPTH {
            return Err(format!(
                "a rule statement nests {depth} levels deep; WAF allows at most \
                 {MAX_STATEMENT_NESTING_DEPTH}"
            ));
        }
        pending.extend(
            nested_statements(statement)
                .into_iter()
                .map(|nested| (nested, depth + 1)),
        );
    }
    Ok(())
}

/// One kind of statement that runs a whole RULE GROUP: how to spot it, what to
/// call it in a refusal's reason, and the `ParameterExceptionField` value that
/// names it.
struct RuleGroupRunningStatement {
    /// The statement's shape name, for the refusal's reason.
    shape_name: &'static str,
    /// The `ParameterExceptionField` value naming this statement.
    field: &'static str,
    /// `true` when `statement` carries this kind of statement.
    present: fn(&Statement) -> bool,
}

/// Which statements run a whole rule group: the customer-owned
/// `RuleGroupReferenceStatement` and the vendor-published
/// `ManagedRuleGroupStatement`.
///
/// One list, because the model states the SAME placement rule on both and the
/// check below is defined over that rule rather than over either statement.
const RULE_GROUP_RUNNING_STATEMENTS: &[RuleGroupRunningStatement] = &[
    RuleGroupRunningStatement {
        shape_name: "RuleGroupReferenceStatement",
        field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
        present: |statement| statement.rule_group_reference_statement.is_some(),
    },
    RuleGroupRunningStatement {
        shape_name: "ManagedRuleGroupStatement",
        field: FIELD_MANAGED_RULE_SET_STATEMENT,
        present: |statement| statement.managed_rule_group_statement.is_some(),
    },
];

/// Where a statement that runs a whole rule group may appear, if `rules` puts one
/// somewhere it may not. `Some` carries the `ParameterExceptionField` value naming
/// the statement at fault and the reason, for the refusal.
///
/// The model states the placement rule on each such statement itself, in the same
/// words: "You cannot nest a `RuleGroupReferenceStatement`, for example for use
/// inside a `NotStatement` or `OrStatement`. You cannot use a rule group reference
/// statement inside another rule group. You can only reference a rule group as a
/// top-level statement within a rule that you define in a web ACL"
/// (`smithy_query(query="doc",shape="RuleGroupReferenceStatement")`) and "You
/// cannot nest a `ManagedRuleGroupStatement`, for example for use inside a
/// `NotStatement` or `OrStatement`. You cannot use a managed rule group inside
/// another rule group. You can only reference a managed rule group as a top-level
/// statement within a rule that you define in a web ACL"
/// (`smithy_query(query="doc",shape="ManagedRuleGroupStatement")`). So a statement
/// that runs a rule group — whoever owns that group — is well placed ONLY as a
/// rule's own top-level statement, and only in a web ACL: `top_level_allowed` is
/// `true` for a web ACL save and `false` for a rule group save. A misplacement is a
/// parameter WAF does not accept rather than a dangling reference, so it is
/// answered with `WAFInvalidParameterException` ("Your nested statement isn't
/// valid. You might have tried to nest a statement that can’t be nested", from that
/// error's own doc) — and it is judged BEFORE the reference is resolved, because a
/// well-placed reference is what resolution is defined over.
///
/// A managed rule group's own `ScopeDownStatement` is NOT a misplacement: it is the
/// nested statement that managed statement is modelled to carry ("an optional
/// nested statement that narrows the scope of the web requests that are evaluated
/// by the managed rule group"), and what this refuses is a group-running statement
/// reached THROUGH a nesting.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request.
pub fn misplaced_rule_group_statement(
    rules: &[Rule],
    top_level_allowed: bool,
) -> Option<(&'static str, String)> {
    for rule in rules {
        for kind in RULE_GROUP_RUNNING_STATEMENTS {
            if !top_level_allowed && (kind.present)(&rule.statement) {
                return Some((
                    kind.field,
                    format!(
                        "the rule {:?} carries a {}; a rule group can only be \
                         referenced from a rule of a web ACL, not from a rule of \
                         another rule group",
                        rule.name, kind.shape_name
                    ),
                ));
            }
        }
        let mut pending: Vec<&Statement> = nested_statements(&rule.statement);
        while let Some(statement) = pending.pop() {
            for kind in RULE_GROUP_RUNNING_STATEMENTS {
                if (kind.present)(statement) {
                    return Some((
                        kind.field,
                        format!(
                            "the rule {:?} nests a {} inside another statement; a \
                             rule group can only be referenced by a rule's \
                             top-level statement",
                            rule.name, kind.shape_name
                        ),
                    ));
                }
            }
            pending.extend(nested_statements(statement));
        }
    }
    None
}

/// `true` when any rule of `rules` references `arn`, at the top level of its
/// statement or anywhere nested inside it.
///
/// The one walk over a stored rule LIST that every "is this resource in use?"
/// question runs, whichever entity holds the list (see
/// [`ComAmazonawsWafv2Awswaf20190729State::web_acl_referencing`] and
/// [`ComAmazonawsWafv2Awswaf20190729State::rule_group_referencing`]). It descends
/// the whole tree, because a reference statement can sit inside `AndStatement`,
/// `OrStatement`, `NotStatement` or a rate-based/managed-rule-group scope-down
/// statement.
fn rules_reference_arn(rules: Option<&[Rule]>, arn: &str) -> bool {
    rules.unwrap_or_default().iter().any(|rule| {
        statement_references_arn(&rule.statement, arn, MAX_STATEMENT_NESTING_DEPTH)
    })
}

/// `true` when `statement`, or anything nested inside it, is a reference statement
/// naming `arn`.
///
/// The three reference-bearing statements of this model that name a resource of
/// this account — `IPSetReferenceStatement`, `RuleGroupReferenceStatement` and
/// `RegexPatternSetReferenceStatement` — are all checked, because an ARN names one
/// resource of one kind: whichever kind `arn` belongs to, the statement that could
/// carry it is the one for that kind, and no other statement can name it.
///
/// `depth_budget` is the number of levels left to descend, so this cannot recurse
/// further than [`MAX_STATEMENT_NESTING_DEPTH`] even if a tree somehow reached the
/// store without passing [`validate_statement_nesting`].
fn statement_references_arn(statement: &Statement, arn: &str, depth_budget: u32) -> bool {
    if depth_budget == 0 {
        return false;
    }
    let names_arn = statement
        .ip_set_reference_statement
        .as_ref()
        .is_some_and(|reference| reference.arn == arn)
        || statement
            .rule_group_reference_statement
            .as_ref()
            .is_some_and(|reference| reference.arn == arn)
        || statement
            .regex_pattern_set_reference_statement
            .as_ref()
            .is_some_and(|reference| reference.arn == arn);
    if names_arn {
        return true;
    }
    nested_statements(statement)
        .into_iter()
        .any(|nested| statement_references_arn(nested, arn, depth_budget - 1))
}

// ---------------------------------------------------------------------------
// RuleGroup permission policies
//
// `PutPermissionPolicy` attaches an IAM policy to the rule group its `ResourceArn`
// names and `GetPermissionPolicy` reports it back; the attached document itself
// lives on [`RuleGroupEntry::permission_policy`], so one copy serves every reader.
// The restrictions checked below are the ones `PutPermissionPolicy` and
// `WAFInvalidPermissionPolicyException` document, nothing wider: the document is
// only CHECKED here, never rewritten.
// ---------------------------------------------------------------------------

/// The IAM policy language version a permission policy must declare.
///
/// Source: `PutPermissionPolicy`'s own documentation — "The policy must be composed
/// using IAM Policy version 2012-10-17."
pub const PERMISSION_POLICY_VERSION: &str = "2012-10-17";

/// The one `Effect` a permission policy statement may carry.
///
/// Source: `PutPermissionPolicy`'s own documentation — "`Effect` must specify
/// `Allow`."
pub const PERMISSION_POLICY_EFFECT: &str = "Allow";

/// Every `Action` a permission policy has to grant.
///
/// Source: `PutPermissionPolicy`'s own documentation — "`Action` must specify
/// `wafv2:CreateWebACL`, `wafv2:UpdateWebACL`, and
/// `wafv2:PutFirewallManagerRuleGroups`."
pub const PERMISSION_POLICY_REQUIRED_ACTIONS: &[&str] = &[
    "wafv2:CreateWebACL",
    "wafv2:UpdateWebACL",
    "wafv2:PutFirewallManagerRuleGroups",
];

/// The `Action`s a permission policy may grant BESIDES
/// [`PERMISSION_POLICY_REQUIRED_ACTIONS`], and the only ones.
///
/// Source: `PutPermissionPolicy`'s own documentation — "… and may optionally specify
/// `wafv2:GetRuleGroup`. WAF rejects any extra actions or wildcard actions in the
/// policy."
pub const PERMISSION_POLICY_OPTIONAL_ACTIONS: &[&str] = &["wafv2:GetRuleGroup"];

/// Whether `action` is one a permission policy may grant.
///
/// The two lists hold the literal actions the documentation names, so an action
/// outside them is refused — including any form carrying a `*`, which is never one
/// of the literals and so is rejected as the documentation requires, however wide or
/// narrow the wildcard is.
fn permission_policy_action_is_allowed(action: &str) -> bool {
    PERMISSION_POLICY_REQUIRED_ACTIONS.contains(&action)
        || PERMISSION_POLICY_OPTIONAL_ACTIONS.contains(&action)
}

/// Whether `policy` is a permission policy WAF accepts: a JSON object declaring IAM
/// policy version [`PERMISSION_POLICY_VERSION`] whose `Statement` — one statement
/// object, or a non-empty list of them — every entry of satisfies
/// [`permission_policy_statement_actions`], and which between them grant every
/// [`PERMISSION_POLICY_REQUIRED_ACTIONS`] entry.
///
/// Source: the restrictions `PutPermissionPolicy` and
/// `WAFInvalidPermissionPolicyException` document. What gets stored is the caller's
/// own text, byte for byte.
pub fn permission_policy_is_valid(policy: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(document) = document.as_object() else {
        return false;
    };
    if document
        .get("Version")
        .and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_VERSION)
    {
        return false;
    }
    let statements: Vec<&::serde_json::Value> = match document.get("Statement") {
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(::serde_json::Value::Array(list)) => list.iter().collect(),
        // A policy with no `Statement`, or one that is neither a statement nor a
        // list of statements, carries no `Effect`, `Action` or `Principal` at all.
        _ => return false,
    };
    if statements.is_empty() {
        return false;
    }
    let mut granted: Vec<&str> = Vec::new();
    for statement in statements {
        match permission_policy_statement_actions(statement) {
            Some(actions) => granted.extend(actions),
            None => return false,
        }
    }
    // The required actions are required of the POLICY: a document may spread them
    // over several statements, so they are counted once every statement has been
    // found to be within the restrictions.
    PERMISSION_POLICY_REQUIRED_ACTIONS
        .iter()
        .all(|required| granted.contains(required))
}

/// The actions one `Statement` of a permission policy grants, or `None` when the
/// statement is outside the documented restrictions: it must carry an `Effect` of
/// [`PERMISSION_POLICY_EFFECT`], a non-empty `Principal`, an `Action` — one action or
/// a non-empty list of them — naming only actions
/// [`permission_policy_action_is_allowed`] admits, and no `Resource`.
///
/// `NotAction`, `NotPrincipal` and `NotResource` are refused with the member they
/// negate: a statement carrying one names no `Action` or `Principal`, or names a
/// resource the policy "must not include".
fn permission_policy_statement_actions(statement: &::serde_json::Value) -> Option<Vec<&str>> {
    let statement = statement.as_object()?;
    if statement.contains_key("Resource") || statement.contains_key("NotResource") {
        return None;
    }
    if statement.get("Effect").and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_EFFECT)
    {
        return None;
    }
    if statement.contains_key("NotPrincipal") || statement.contains_key("NotAction") {
        return None;
    }
    if !permission_policy_principal_is_present(statement.get("Principal")) {
        return None;
    }
    let actions: Vec<&::serde_json::Value> = match statement.get("Action")? {
        action @ ::serde_json::Value::String(_) => vec![action],
        ::serde_json::Value::Array(list) => list.iter().collect(),
        _ => return None,
    };
    if actions.is_empty() {
        return None;
    }
    actions
        .into_iter()
        .map(|action| {
            let action = action.as_str()?;
            permission_policy_action_is_allowed(action).then_some(action)
        })
        .collect()
}

/// Whether a statement names a `Principal`: a non-blank string, or a map of
/// principal kinds (`AWS`, `Service`, …) whose every entry carries a non-blank value
/// or a non-empty list of them.
fn permission_policy_principal_is_present(principal: Option<&::serde_json::Value>) -> bool {
    match principal {
        Some(::serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(::serde_json::Value::Object(kinds)) => {
            !kinds.is_empty() && kinds.values().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// Whether one entry of a `Principal` map names a principal.
fn permission_policy_principal_value(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(value) => !value.trim().is_empty(),
        ::serde_json::Value::Array(values) => {
            !values.is_empty() && values.iter().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// A modelled timestamp as milliseconds since the Unix epoch, so a `TimeWindow`
/// endpoint and a stored observation ([`BotRequestRecord::observed_at_millis`])
/// compare as plain numbers. Sub-millisecond precision is truncated, which is the
/// resolution a traffic window is reported at.
pub fn date_time_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// Milliseconds since the Unix epoch, now — the instant a mutation records itself
/// at, at the resolution [`date_time_millis`] compares a modelled timestamp at.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// 256 fresh bits, for a value that must be unpredictable per call.
///
/// Three sources hashed together with SHA-256 (the `sha2` crate), the way this
/// repo's other services draw the bits behind a minted id (see
/// `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs`): the
/// OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds,
/// and a process-wide counter, so two calls in the same nanosecond cannot collide.
pub(crate) fn fresh_entropy() -> [u8; 32] {
    use ::sha2::{Digest, Sha256};
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    static SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    hasher.finalize().into()
}

/// A fresh lowercase, hyphenated UUIDv4 — the form every `EntityId` and
/// `LockToken` in this model takes
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`, from
/// `smithy_query(query="shape",shape="LockToken")`).
///
/// The 128 bits come from [`fresh_entropy`] and are hex-encoded (the `hex` crate).
/// The version nibble and the variant bits RFC 4122 §4.4 fixes are then set, so
/// the value is a well-formed version-4 UUID. An id or a lock token names a
/// resource; a credential would come from `ctx.credential_store()`, never here.
pub fn new_uuid() -> String {
    let digest = fresh_entropy();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are
    // `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// Record what a state-changing WAFv2 call did, as the audit trail a mutation
/// leaves behind.
///
/// `outcome` is what the service decided — `"created"`, `"updated"`, `"deleted"`,
/// or the modelled error it answered — so a refused mutation is as visible as an
/// applied one. The record carries only "what was called": the operation and the
/// resource it names. Nothing from the caller's credential is recorded, not even
/// the access key id the request presented, and the request body never enters the
/// record.
pub fn audit(operation: &str, scope: &str, name: &str, id: &str, outcome: &str) {
    ::tracing::info!(
        target: "wafv2::audit",
        operation,
        scope,
        name,
        id,
        outcome,
        "wafv2 state-changing request"
    );
}

/// `true` when `value` has the shape of a wafv2 resource ARN:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`.
///
/// `GetWebACL` separates the two ARN failures the model names: an ARN that is
/// malformed is `WAFInvalidParameterException` ("Your request references an ARN
/// that is malformed", from that error's own doc), while a well-formed ARN naming
/// nothing stored is `WAFNonexistentItemException`.
pub fn is_wafv2_resource_arn(value: &str) -> bool {
    let mut segments = value.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("wafv2"),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return false;
    };
    let resource_parts: Vec<&str> = resource.split('/').collect();
    !partition.is_empty()
        && resource_parts.len() == 4
        && matches!(resource_parts[0], "global" | "regional")
        && resource_parts[1..].iter().all(|part| !part.is_empty())
}

/// One form of protected-resource ARN a web ACL can be put in front of, and the
/// `ResourceType` value that form names.
#[derive(Debug)]
pub struct ProtectedResourceForm {
    /// The ARN's service-namespace segment (`elasticloadbalancing`, `appsync`, …).
    pub service: &'static str,
    /// The ARN's resource part, segment by segment: `Some(literal)` must match
    /// that segment exactly, `None` is any ONE non-empty segment — a name or an
    /// id the owning service minted.
    pub segments: &'static [Option<&'static str>],
    /// The `ResourceType` value this form names.
    pub resource_type: &'static str,
}

impl ProtectedResourceForm {
    /// `true` when `service` and the slash-separated `parts` of an ARN's resource
    /// part are this form.
    fn matches(&self, service: &str, parts: &[&str]) -> bool {
        self.service == service
            && parts.len() == self.segments.len()
            && parts
                .iter()
                .zip(self.segments)
                .all(|(part, segment)| match segment {
                    Some(literal) => part == literal,
                    None => !part.is_empty(),
                })
    }
}

/// Every resource form a web ACL can be put in front of through this API.
///
/// `AssociateWebACLRequest$ResourceArn` gives the ARN format of each one
/// (`smithy_query(query="doc",shape="AssociateWebACLRequest",member="ResourceArn")`:
/// "the ARN must be in one of the following formats"), and `ResourceType`
/// (`smithy_query(query="shape",shape="ResourceType")`) names the same eight kinds,
/// so this table is those two lists joined — the catalogue of what a web ACL can
/// protect, and the only place a protected resource's kind is decided.
///
/// A CloudFront distribution is deliberately absent: it is bound to a web ACL
/// through the distribution's own configuration, not through this API ("use this
/// for all resource types except for Amazon CloudFront distributions", the
/// operation's own doc), and `ResourceType` does not list it either.
pub const PROTECTED_RESOURCE_FORMS: &[ProtectedResourceForm] = &[
    ProtectedResourceForm {
        service: "elasticloadbalancing",
        // `loadbalancer/app/<load-balancer-name>/<load-balancer-id>`: the `app`
        // segment is an APPLICATION Load Balancer, which is the only kind of load
        // balancer the list names.
        segments: &[Some("loadbalancer"), Some("app"), None, None],
        resource_type: "APPLICATION_LOAD_BALANCER",
    },
    ProtectedResourceForm {
        service: "apigateway",
        // `/restapis/<api-id>/stages/<stage-name>`, whose leading slash is dropped
        // before the parts are compared.
        segments: &[Some("restapis"), None, Some("stages"), None],
        resource_type: "API_GATEWAY",
    },
    ProtectedResourceForm {
        service: "appsync",
        // `apis/<GraphQLApiId>`
        segments: &[Some("apis"), None],
        resource_type: "APPSYNC",
    },
    ProtectedResourceForm {
        service: "cognito-idp",
        // `userpool/<user-pool-id>`
        segments: &[Some("userpool"), None],
        resource_type: "COGNITO_USER_POOL",
    },
    ProtectedResourceForm {
        service: "apprunner",
        // `service/<apprunner-service-name>/<apprunner-service-id>`
        segments: &[Some("service"), None, None],
        resource_type: "APP_RUNNER_SERVICE",
    },
    ProtectedResourceForm {
        service: "ec2",
        // `verified-access-instance/<instance-id>`
        segments: &[Some("verified-access-instance"), None],
        resource_type: "VERIFIED_ACCESS_INSTANCE",
    },
    ProtectedResourceForm {
        service: "amplify",
        // `apps/<app-id>`
        segments: &[Some("apps"), None],
        resource_type: "AMPLIFY",
    },
    ProtectedResourceForm {
        service: "bedrock-agentcore",
        // `gateway/<gateway-id>`
        segments: &[Some("gateway"), None],
        resource_type: "AGENTCORE_GATEWAY",
    },
];

/// The `ResourceType` a listing selects when the request names none: "if you don't
/// provide a resource type, the call uses the resource type
/// APPLICATION_LOAD_BALANCER" (`ListResourcesForWebACLRequest$ResourceType`).
pub const DEFAULT_RESOURCE_TYPE: &str = "APPLICATION_LOAD_BALANCER";

/// The `ResourceType` of the resource `arn` names, or `None` when a web ACL cannot
/// be put in front of it.
///
/// `None` covers both ways an ARN fails this parameter, which WAF reports as one
/// refusal ("your request references an ARN that is malformed, or corresponds to a
/// resource with which a web ACL can't be associated",
/// `WAFInvalidParameterException`): a string that is not an ARN at all, and a
/// well-formed ARN of a resource kind [`PROTECTED_RESOURCE_FORMS`] does not
/// catalogue.
pub fn protected_resource_type(arn: &str) -> Option<&'static str> {
    let mut segments = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() {
        return None;
    }
    // An API Gateway stage's resource part begins with a slash
    // (`::/restapis/<api-id>/stages/<stage-name>`); every other form does not.
    let parts: Vec<&str> = resource.trim_start_matches('/').split('/').collect();
    PROTECTED_RESOURCE_FORMS
        .iter()
        .find(|form| form.matches(service, &parts))
        .map(|form| form.resource_type)
}

/// The region segment of a scope's ARNs.
///
/// A CLOUDFRONT-scoped resource is always reached through one endpoint — "API and
/// SDKs - For all calls, use the Region endpoint us-east-1" (the `Scope` member
/// doc) — so its ARN carries that region whatever the caller signed for.
fn arn_region<'a>(scope: &str, request_region: &'a str) -> &'a str {
    if scope == SCOPE_CLOUDFRONT {
        DEFAULT_REGION
    } else {
        request_region
    }
}

/// The first resource segment of a wafv2 ARN: `global` for a CLOUDFRONT-scoped
/// resource, `regional` for a REGIONAL one.
fn scope_path(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        "global"
    } else {
        "regional"
    }
}

/// `arn:aws:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`, where
/// `kind` is the resource type segment (`ipset`, `webacl`, …).
pub fn resource_arn(
    scope: &str,
    request_region: &str,
    kind: &str,
    name: &str,
    id: &str,
) -> String {
    ::engine::emulator_arn(
        "wafv2",
        arn_region(scope, request_region),
        &format!("{}/{kind}/{name}/{id}", scope_path(scope)),
    )
}

/// `awswaf:<account ID>:<kind>:<entity name>:` — the prefix the `LabelNamespace`
/// member of a label-bearing entity specifies for every label a rule in it adds.
///
/// One helper for every entity kind, because both docs give the same shape with
/// only the kind segment differing: `WebACL$LabelNamespace` is
/// "awswaf:<account ID>:webacl:<web ACL name>:" and `RuleGroup$LabelNamespace` is
/// "awswaf:<account ID>:rulegroup:<rule group name>:". `kind` is the same resource
/// segment an ARN carries, so pass the `…_ARN_KIND` constant for the entity.
pub fn label_namespace(kind: &str, name: &str) -> String {
    format!(
        "awswaf:{}:{kind}:{name}:",
        ::engine::emulator_account_id()
    )
}

/// Check one `Addresses` entry against the set's declared `IPAddressVersion`.
///
/// `IPSet$Addresses` documents the rule: "All addresses must be specified using
/// Classless Inter-Domain Routing (CIDR) notation. WAF supports all IPv4 and IPv6
/// CIDR ranges except for `/0`", and `IPAddressVersion` fixes which family the
/// entries belong to. `Err` carries the reason, for the refusal's message.
///
/// What the doc does NOT say is anything about the host bits below the prefix
/// length, so `10.0.0.1/8` is deliberately accepted and stored verbatim: the
/// documented rule is the notation, the family and the `/0` exclusion, and this
/// check goes no further than the rule it can cite.
pub fn validate_cidr(address: &str, ip_address_version: &str) -> Result<(), String> {
    let Some((literal, prefix)) = address.split_once('/') else {
        return Err(format!(
            "{address:?} is not in CIDR notation; expected <address>/<prefix length>"
        ));
    };
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{address:?} has a non-numeric CIDR prefix length"));
    }
    let Ok(prefix_len) = prefix.parse::<u32>() else {
        return Err(format!("{address:?} has an out-of-range CIDR prefix length"));
    };
    // `/0` is the one range WAF documents as unsupported, for either family.
    let (parsed, max_prefix) = match ip_address_version {
        "IPV4" => (
            literal.parse::<::std::net::Ipv4Addr>().is_ok(),
            32_u32,
        ),
        "IPV6" => (
            literal.parse::<::std::net::Ipv6Addr>().is_ok(),
            128_u32,
        ),
        other => {
            return Err(format!("{other:?} is not a known IPAddressVersion"));
        }
    };
    if !parsed {
        return Err(format!(
            "{address:?} is not an {ip_address_version} address in CIDR notation"
        ));
    }
    if prefix_len == 0 || prefix_len > max_prefix {
        return Err(format!(
            "{address:?} has prefix length /{prefix_len}; {ip_address_version} \
             CIDR ranges must be between /1 and /{max_prefix}"
        ));
    }
    Ok(())
}

/// The `Description` a WAFv2 entity reports when the save that created it supplied
/// none.
///
/// Every WAFv2 response that carries an entity, or one of its `…Summary`
/// projections, carries the `Description` member: the recorded wire responses show
/// `""` for an entity saved without a description, rather than the member being
/// omitted. So the empty string is what gets STORED, and `Get`, `Create` and
/// `List` all report it from state instead of each deciding whether to emit the
/// member. (`EntityDescription` is modelled `length 1..256`, which bounds what a
/// REQUEST may carry; the empty description a RESPONSE reports is recorded
/// behaviour.)
pub fn stored_description(requested: Option<String>) -> Option<String> {
    Some(requested.unwrap_or_default())
}

/// The DDoS-protection mode a web ACL carries when the save names none.
///
/// `OnSourceDDoSProtectionConfig$ALBLowReputationMode` documents
/// `ACTIVE_UNDER_DDOS` as "enabled by default whenever a web ACL is associated with
/// an Application Load Balancer", and the member is `required` inside the
/// structure, so the account default is a real stored value — not an absent
/// structure — and a read reports it.
pub const DEFAULT_ALB_LOW_REPUTATION_MODE: &str = "ACTIVE_UNDER_DDOS";

/// The evaluation window a rate-based statement carries when the save names none.
///
/// `RateBasedStatement$EvaluationWindowSec` documents "Valid settings are 60, 120,
/// 300, and 600" and "Default: `300` (5 minutes)". The member's modelled
/// zero-value default is `0`, which is not one of the valid settings, so a request
/// that omits it arrives here as `0` and WAF's documented default is what gets
/// stored.
pub const DEFAULT_EVALUATION_WINDOW_SEC: i64 = 300;

/// The longest evaluation window a rate-based statement can carry, in seconds.
///
/// The last of the four settings `RateBasedStatement$EvaluationWindowSec` documents
/// ("Valid settings are 60, 120, 300, and 600"), so no rate count has to be kept over
/// a longer span than this — which is what bounds the per-second buckets one
/// aggregation instance holds (`crate::service::edge::RateBucket`).
pub const MAX_EVALUATION_WINDOW_SEC: i64 = 600;

/// The smallest `ImmunityTime` WAF accepts for a CHALLENGE immunity, in seconds.
///
/// Source: `ImmunityTimeProperty$ImmunityTime`'s own documentation — "The amount of
/// time, in seconds, that a `CAPTCHA` or challenge timestamp is considered valid by
/// WAF. The default setting is 300. For the Challenge action, the minimum setting
/// is 300." The member's modelled `@range` (60..259200, from `TimeWindowSecond`) is
/// the bound the request boundary enforces and is the CAPTCHA floor; the Challenge
/// floor is this higher one, which only the service knows, so it is checked here.
pub const MIN_CHALLENGE_IMMUNITY_TIME_SEC: i64 = 300;

/// The `ErrorReason` a challenge immunity below [`MIN_CHALLENGE_IMMUNITY_TIME_SEC`]
/// carries: the rule the value broke, in the words the model states that rule in
/// (`smithy_query(query="doc",shape="ImmunityTimeProperty",member="ImmunityTime")` —
/// "For the Challenge action, the minimum setting is 300.").
///
/// The `Reason` of a `WAFInvalidParameterException` is request-derived prose beside
/// the `Field` and the `Parameter` the response also carries, and the sentence WAF
/// itself words for this class is not in the model, so what is emitted here is the
/// model's own statement of the broken rule.
pub const CHALLENGE_IMMUNITY_BELOW_MINIMUM_REASON: &str =
    "For the Challenge action, the minimum setting is 300.";

/// The `ImmunityTime` of the first challenge configuration in this save that is
/// below the Challenge floor, if any.
///
/// A `ChallengeConfig` "specifies how WAF should handle Challenge evaluations. This
/// is available at the web ACL level and in each rule", so every one of them in a
/// save is judged: the entity-level configuration `web_acl_challenge_config` carries
/// (a web ACL has one; a rule group has none) and each rule's own. The rules are
/// visited in order, so a save carrying several below-floor values is refused with
/// the same one on every call.
///
/// A CAPTCHA immunity keeps the modelled floor of 60 and is NOT judged here: the
/// 300-second floor is documented for the Challenge action alone.
pub fn below_minimum_challenge_immunity(
    web_acl_challenge_config: Option<&ChallengeConfig>,
    rules: &[Rule],
) -> Option<i64> {
    web_acl_challenge_config
        .into_iter()
        .chain(rules.iter().filter_map(|rule| rule.challenge_config.as_ref()))
        .filter_map(|config| config.immunity_time_property.as_ref())
        .map(|property| property.immunity_time)
        .find(|immunity_time| *immunity_time < MIN_CHALLENGE_IMMUNITY_TIME_SEC)
}

/// Put `rules` in the order WAF evaluates them: ascending `Priority`.
///
/// `Rule$Priority` is the evaluation order itself — "WAF evaluates each request
/// against the Rules in order based on the value of Priority. WAF processes rules
/// with lower priority first. The priorities don't need to be consecutive, but they
/// must all be different" — so a stored rule list is held in that order and every
/// read hands it back in it, whatever order the save submitted the rules in. A
/// client reading an entity back to reason about evaluation order, or diffing it
/// against a template, therefore sees the order the rules actually run in.
///
/// The sort is STABLE, so a list whose order its priorities leave undecided keeps
/// the order it was given — though no stored list is one: a save carrying two rules
/// at one priority is refused before anything is written ("they must all be
/// different"; see [`duplicate_rule_priority`]).
pub fn normalize_rule_order(rules: &mut [Rule]) {
    rules.sort_by_key(|rule| rule.priority);
}

/// Materialise, in place, every default WAF applies inside a saved rule tree.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request, so nothing here may consume stack in proportion to
/// its depth. A default is applied wherever the statement that carries it sits,
/// top level or nested.
pub fn apply_rule_defaults(rules: &mut [Rule]) {
    let mut pending: Vec<&mut Statement> = rules.iter_mut().map(|rule| &mut rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(rate) = statement
            .rate_based_statement
            .as_mut()
            .filter(|rate| rate.evaluation_window_sec == 0)
        {
            rate.evaluation_window_sec = DEFAULT_EVALUATION_WINDOW_SEC;
        }
        pending.extend(nested_statements_mut(statement));
    }
}

/// Materialise, on a web ACL about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// One place, called by both `CreateWebACL` and `UpdateWebACL`, so a read cannot
/// see a web ACL that was created one way and updated another:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — in ascending `Priority` ([`normalize_rule_order`]), and always a
///   list, so a read of a rule-less web ACL answers `[]` rather
///   than omitting the member; `Rules` is "the Rule statements used to identify the
///   web requests that you want to manage", and a web ACL with none has an empty
///   collection of them, not an unknown one.
/// * `OnSourceDDoSProtectionConfig` — [`DEFAULT_ALB_LOW_REPUTATION_MODE`].
/// * `Capacity` — [`web_acl_capacity`] of the stored rules, which is why it is
///   computed AFTER the rule defaults are applied. `rule_group_capacities` prices
///   the rule-group references the tree carries; see [`web_acl_capacity`].
pub fn apply_web_acl_defaults(
    web_acl: &mut WebACL,
    rule_group_capacities: &HashMap<String, i64>,
) {
    web_acl.description = stored_description(web_acl.description.take());
    let mut rules = web_acl.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    normalize_rule_order(&mut rules);
    web_acl.capacity = web_acl_capacity(&rules, rule_group_capacities);
    web_acl.rules = Some(rules);
    web_acl.on_source_d_do_s_protection_config = Some(
        web_acl
            .on_source_d_do_s_protection_config
            .take()
            .unwrap_or_else(|| OnSourceDDoSProtectionConfig {
                alb_low_reputation_mode: DEFAULT_ALB_LOW_REPUTATION_MODE.to_string(),
            }),
    );
}

/// The labels a rule group's rules ADD to matching requests, deduplicated and in
/// a stable order.
///
/// `RuleGroup$AvailableLabels` is "the labels that one or more rules in this rule
/// group add to matching web requests. These labels are defined in the
/// `RuleLabels` for a Rule", so it is DERIVED from the stored rules rather than
/// carried in the request. Each entry is the label as the rule specified it
/// (`LabelSummary$Name` is "an individual label specification"); the prefix every
/// one of them acquires at match time is reported separately, as
/// [`label_namespace`].
pub fn rule_group_available_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    label_summaries(
        rules
            .iter()
            .flat_map(|rule| rule.rule_labels.as_deref().unwrap_or_default())
            .map(|label| label.name.as_str()),
    )
}

/// `names` as `LabelSummary` entries, deduplicated and in a stable order.
///
/// One projection for every list of labels this service reports —
/// [`rule_group_available_labels`] over a stored rule tree,
/// [`rule_group_consumed_labels`] over the label-match keys in one, and
/// [`ManagedRuleGroupDescriptor::available_labels`] over a published group's rules
/// — so the deduplication and the order are decided in one place whatever the
/// labels were read from.
fn label_summaries<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<LabelSummary> {
    owned_label_summaries(names.into_iter().map(str::to_string))
}

/// [`label_summaries`] for labels that had to be COMPOSED rather than read — the
/// fully qualified labels a published managed rule group reports, which carry the
/// group's own namespace ([`ManagedRuleGroupDescriptor::available_labels`]).
fn owned_label_summaries(names: impl IntoIterator<Item = String>) -> Vec<LabelSummary> {
    let unique: ::std::collections::BTreeSet<String> = names.into_iter().collect();
    unique
        .into_iter()
        .map(|name| LabelSummary { name: Some(name) })
        .collect()
}

/// The labels a rule group's rules MATCH AGAINST, deduplicated and in a stable
/// order.
///
/// `RuleGroup$ConsumedLabels` is "the labels that one or more rules in this rule
/// group match against in label match statements. These labels are defined in a
/// `LabelMatchStatement` specification, in the Statement definition of a rule", so
/// it is derived from the stored rules the way [`rule_group_available_labels`] is —
/// and from the whole statement tree, because a `LabelMatchStatement` can sit
/// inside any nesting [`nested_statements`] enumerates.
pub fn rule_group_consumed_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    let mut keys: Vec<&str> = Vec::new();
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(label_match) = statement.label_match_statement.as_ref() {
            keys.push(label_match.key.as_str());
        }
        pending.extend(nested_statements(statement));
    }
    label_summaries(keys)
}

/// Materialise, on a rule group about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// The rule-group counterpart of [`apply_web_acl_defaults`], so a rule group reads
/// back the same way however it was last saved:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — in ascending `Priority` ([`normalize_rule_order`]), and always a
///   list, for the same reason the web ACL's is: a rule group with no rules has an
///   empty collection of them, not an unknown one.
/// * `AvailableLabels` / `ConsumedLabels` — derived from the stored rules by
///   [`rule_group_available_labels`] and [`rule_group_consumed_labels`], which is
///   why they are computed AFTER the rule defaults are applied.
///
/// `Capacity` is NOT derived here: unlike `WebACL$Capacity`, a rule group's is
/// "the web ACL capacity units (WCUs) required for this rule group", which the
/// create call declares and "you cannot change it after creation" — so the
/// declared value is what is stored and reported.
pub fn apply_rule_group_defaults(rule_group: &mut RuleGroup) {
    rule_group.description = stored_description(rule_group.description.take());
    let mut rules = rule_group.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    normalize_rule_order(&mut rules);
    rule_group.available_labels = Some(rule_group_available_labels(&rules));
    rule_group.consumed_labels = Some(rule_group_consumed_labels(&rules));
    rule_group.rules = Some(rules);
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafv2Awswaf20190729State>)
}

// ===========================================================================
// The WAFv2 request-validation layer
// ===========================================================================
//
// Two facts about how WAF refuses a bad parameter live here, and both are about
// the ANSWER rather than about the checking:
//
//   * WAF answers with its OWN modelled `WAFInvalidParameterException`, which the
//     model gives `Field`, `Parameter` and `Reason` members beside the message —
//     so a client can read WHICH input to correct instead of parsing prose.
//     [`invalid_parameter_exception`] builds it, and composes the message from
//     exactly those three the way WAF does.
//   * WAF resolves what a request REFERENCES before it judges the syntax of the
//     request's other members, and it reports a collection whose ELEMENTS are at
//     fault against the collection member's own path. Neither is something the
//     request boundary can do for a handler: the boundary runs every modelled
//     constraint first, and words an element failure at the element's indexed
//     path. An operation therefore hands the members concerned over with
//     `skip_validation` on its input struct (see `src/ops/types/`) and checks them
//     here, at the point in its own sequence where WAF checks them.
//
// NOTHING about constraint CHECKING is re-implemented. The constraints a member's
// `skip_validation` strips off its generated member schema are RE-DECLARED below
// in the framework's own representation — a [`TraitMap`] of
// `LengthTrait`/`PatternTrait`/`ElementConstraintsTrait`, the exact form
// `vendor/smithy-macros` would have attached — hung on a member schema of the same
// name, and the check itself is
// [`::aws_smithy_schema_server::constraints::validate`], the SAME walk the
// boundary runs. So the clause, the elision of a long value, the violation cap and
// the lowerCamel path stay the framework's, and a member refused here reads
// identically to one refused at the boundary.

/// The `ParameterExceptionField` value naming a resource ARN
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `RESOURCE_ARN` among its values).
pub const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `ParameterExceptionField` value naming a web ACL. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_WEB_ACL: &str = "WEB_ACL";

/// The `ParameterExceptionField` value naming a rule group. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP: &str = "RULE_GROUP";

/// The `ParameterExceptionField` value naming a `RuleGroupReferenceStatement`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP_REFERENCE_STATEMENT: &str = "RULE_GROUP_REFERENCE_STATEMENT";

/// The `ParameterExceptionField` value naming a managed rule group, as the enum
/// spells it (`MANAGED_RULE_SET`). See [`FIELD_RESOURCE_ARN`].
pub const FIELD_MANAGED_RULE_SET: &str = "MANAGED_RULE_SET";

/// The `ParameterExceptionField` value naming a managed-rule-group statement
/// (`MANAGED_RULE_SET_STATEMENT`, as the enum spells it). See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_MANAGED_RULE_SET_STATEMENT: &str = "MANAGED_RULE_SET_STATEMENT";

/// The `ParameterExceptionField` value naming a `Scope`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `ParameterExceptionField` value naming a regex pattern set. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_REGEX_PATTERN_SET: &str = "REGEX_PATTERN_SET";

/// The `ParameterExceptionField` value naming a rule statement. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_STATEMENT: &str = "STATEMENT";

/// The `ParameterExceptionField` value naming an IP address. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_IP_ADDRESS: &str = "IP_ADDRESS";

/// The `ParameterExceptionField` value naming a per-call entity limit. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_ENTITY_LIMIT: &str = "ENTITY_LIMIT";

/// The `ParameterExceptionField` value naming a token domain. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_TOKEN_DOMAIN: &str = "TOKEN_DOMAIN";

/// The `ParameterExceptionField` value naming a challenge configuration. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_CHALLENGE_CONFIG: &str = "CHALLENGE_CONFIG";

/// The `ParameterExceptionField` value naming a logging destination. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_LOG_DESTINATION: &str = "LOG_DESTINATION";

/// The `ErrorReason` WAF carries for an ARN it will not accept, as the recorded
/// WAF responses show it.
///
/// One sentence for the whole class — an ARN that is malformed, and an ARN that is
/// well formed but names nothing WAF can use for this parameter — because that is
/// what WAF answers for both: the `Parameter` member carries the rejected value,
/// so the reason does not have to distinguish them.
pub const ARN_NOT_VALID_REASON: &str = "The ARN isn't valid. A valid ARN begins with arn: \
     and includes other information separated by colons or slashes.";

/// The resource-kind segment a rule group's ARN carries
/// (`arn:…:wafv2:…:<global|regional>/rulegroup/<name>/<id>`, the form
/// [`resource_arn`] mints).
pub const RULE_GROUP_ARN_KIND: &str = "rulegroup";

/// The resource-kind segment a web ACL's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const WEB_ACL_ARN_KIND: &str = "webacl";

/// The resource-kind segment an IP set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const IP_SET_ARN_KIND: &str = "ipset";

/// The resource-kind segment a regex pattern set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const REGEX_PATTERN_SET_ARN_KIND: &str = "regexpatternset";

/// `WAFInvalidParameterException` as WAF returns it: the structured `Field`,
/// `Parameter` and `Reason` the model gives this one exception, and the message
/// WAF composes from exactly those three.
///
/// This is the one WAFv2 exception whose body describes the CALL (see the note
/// above the constant sentences at the top of this module): `Field` is "the
/// settings where the invalid parameter was found", `Parameter` is "the invalid
/// parameter that resulted in the exception" and `Reason` is "additional
/// information about the exception" (each member's own doc). The message is built
/// from the response's own three members and carries nothing else from the
/// request, so every refusal of this type reads alike whatever tripped it.
pub fn invalid_parameter_exception(
    field: &str,
    parameter: &str,
    reason: &str,
) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "Error reason: {reason}, field: {field}, parameter: {parameter}"
        )),
        field: Some(field.to_string()),
        parameter: Some(parameter.to_string()),
        reason: Some(reason.to_string()),
    }
}

/// [`invalid_parameter_exception`] for an ARN WAF will not accept: `Field`
/// [`FIELD_RESOURCE_ARN`], `Parameter` the rejected ARN and `Reason`
/// [`ARN_NOT_VALID_REASON`].
pub fn invalid_arn_exception(arn: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID_REASON)
}

/// The prefix a logging destination's own NAME has to carry, from the
/// `PutLoggingConfiguration` and `LoggingConfiguration` documentation
/// (`smithy_query(query="doc",shape="LoggingConfiguration")`): "the name that you
/// give the destination must start with `aws-waf-logs-`".
pub const LOG_DESTINATION_NAME_PREFIX: &str = "aws-waf-logs-";

/// The NAME of the logging destination `value` addresses, or `None` when `value` is
/// not an ARN at all.
///
/// Every destination kind a configuration can name — a CloudWatch Logs log group, an
/// S3 bucket, a Firehose delivery stream — addresses the destination by its name in
/// the LAST segment of the ARN's resource part, whether that part separates its
/// segments with colons (`…:log-group:<name>`) or with a slash
/// (`…:deliverystream/<name>`) or is the bare name (`arn:aws:s3:::<name>`). A
/// log-group ARN written with the trailing `:*` that addresses every stream inside
/// the group still resolves to the group's name: the wildcard is not a name of its
/// own.
pub fn log_destination_name(value: &str) -> Option<&str> {
    // An ARN's resource part is everything after the fifth colon and may itself
    // contain colons, so the split stops there.
    let mut segments = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() || service.is_empty() {
        return None;
    }
    let addressed = resource.strip_suffix(":*").unwrap_or(resource);
    let name = addressed.split([':', '/']).next_back()?;
    (!name.is_empty()).then_some(name)
}

/// `true` when `value` names a logging destination WAF delivers to: an ARN whose
/// destination NAME carries [`LOG_DESTINATION_NAME_PREFIX`].
pub fn is_acceptable_log_destination(value: &str) -> bool {
    log_destination_name(value).is_some_and(|name| name.starts_with(LOG_DESTINATION_NAME_PREFIX))
}

/// [`invalid_parameter_exception`] for a logging destination WAF will not deliver
/// to: `Field` [`FIELD_LOG_DESTINATION`] (the settings the destination was declared
/// in), `Parameter` the rejected destination as the request wrote it and `Reason`
/// [`ARN_NOT_VALID_REASON`] — the one sentence that class carries, a destination
/// that is no ARN and a well-formed ARN WAF cannot use as a destination alike.
pub fn invalid_log_destination_exception(destination: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_LOG_DESTINATION, destination, ARN_NOT_VALID_REASON)
}

/// The `ErrorReason` WAF carries when a rule list costs more web ACL capacity
/// units than the capacity it has to fit inside, as the recorded WAF responses
/// show it.
///
/// One sentence for the whole class — a rule group created under the price of its
/// rules and one updated past its immutable capacity alike — because the
/// `Parameter` member carries the capacity the rules actually cost, so the reason
/// does not have to distinguish them.
pub const CAPACITY_LIMIT_EXCEEDED_REASON: &str =
    "You exceeded the capacity limit for a rule group or web ACL.";

/// [`invalid_parameter_exception`] for a rule list that costs more WCUs than the
/// capacity it must fit inside: `Field` [`FIELD_RULE_GROUP`] (the settings the
/// capacity was declared in), `Parameter` the capacity the rules actually cost —
/// the figure [`web_acl_capacity`] derives and `CheckCapacity` publishes, which is
/// the value a caller re-declares to make the save fit, NOT the under-declared one
/// the request sent — and `Reason` [`CAPACITY_LIMIT_EXCEEDED_REASON`].
///
/// This is a PARAMETER failure and not [`limits_exceeded_exception`]: the capacity
/// a rule group declares is a member of the request, so a rule list that does not
/// fit it is corrected by re-declaring that member, while the quota exception is
/// the account/resource-count failure ("the maximum number of WebACL objects that
/// you can create for an Amazon Web Services account") that no request member can
/// correct.
pub fn capacity_limit_exceeded_exception(required_capacity: i64) -> WAFInvalidParameterException {
    invalid_parameter_exception(
        FIELD_RULE_GROUP,
        &required_capacity.to_string(),
        CAPACITY_LIMIT_EXCEEDED_REASON,
    )
}

/// The `ErrorReason` WAF carries for a parameter whose text it cannot read in the
/// form that parameter has to take, as the recorded WAF responses show it.
///
/// One sentence for the whole class — a value that does not decode as an API key, a
/// regular expression that does not parse — because the `Parameter` member carries
/// the rejected text and `Field` the settings it was found in, so the reason does
/// not have to distinguish them.
pub const FORMAT_NOT_VALID_REASON: &str = "The parameter contains formatting \
     that is not valid.";

/// [`invalid_parameter_exception`] for an API key value WAF cannot decode: the
/// rejected value as `Parameter`, [`FORMAT_NOT_VALID_REASON`] as `Reason` and NO
/// `Field` — `ParameterExceptionField` declares no value naming an API key
/// (`smithy_query(query="shape",shape="ParameterExceptionField")`), so WAF leaves the
/// member out, which its message renders as `field: null`.
///
/// A value that does not decode to a key ([`is_api_key_shaped`]) is refused this way
/// by every operation that takes one, and is a different failure from a well-formed
/// key naming nothing stored, which is [`nonexistent_item_exception`].
pub fn invalid_key_format_exception(api_key: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception_without_field(api_key, FORMAT_NOT_VALID_REASON)
}

/// [`invalid_parameter_exception`] for a parameter `ParameterExceptionField`
/// declares no value for: the rejected value as `Parameter`, `reason` as `Reason`
/// and NO `Field`, which the message renders as `field: null`.
///
/// The model's `ParameterExceptionField` enum names the SETTINGS a parameter lives
/// in, and not every parameter has one (`smithy_query(query="shape",shape="ParameterExceptionField")`),
/// so WAF leaves the member out rather than reporting a field the parameter is not
/// in.
pub fn invalid_parameter_exception_without_field(
    parameter: &str,
    reason: &str,
) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "Error reason: {reason}, field: null, parameter: {parameter}"
        )),
        field: None,
        parameter: Some(parameter.to_string()),
        reason: Some(reason.to_string()),
    }
}

/// `true` when `value` is one of the currencies the model declares
/// (`smithy_query(query="shape",shape="Currency")`), the denominations a
/// monetization query may ask its amounts in.
pub fn is_known_currency(value: &str) -> bool {
    CURRENCY_VALUES.contains(&value)
}

/// The largest page a paginated WAFv2 list answers with when the request names no
/// `Limit`: the upper bound of the model's `PaginationLimit`
/// (`smithy_query(query="shape",shape="PaginationLimit")` → `range 1..100`), which
/// every paginated operation of this service shares.
pub const MAX_PAGE_SIZE: i32 = 100;

/// How many entries one page holds, for a request whose `Limit` is `limit`.
///
/// Absent `Limit`, a page holds as many as [`MAX_PAGE_SIZE`] allows; a `Limit`
/// outside the model's `PaginationLimit` range is REFUSED rather than clamped, so
/// no caller gets a silently empty terminal page. One reading of that range for
/// every paginated operation, so a third one cannot grow a third copy of it.
///
/// `noun` is what the page holds (`"summaries"`, `"versions"`), for the refusal's
/// `Reason`; the refusal itself is the modelled `WAFInvalidParameterException`
/// carrying `Field` [`FIELD_ENTITY_LIMIT`] and the rejected value.
pub fn page_size(limit: Option<i32>, noun: &str) -> Result<usize, WAFInvalidParameterException> {
    let requested = match limit {
        None => MAX_PAGE_SIZE,
        Some(limit) if (1..=MAX_PAGE_SIZE).contains(&limit) => limit,
        Some(limit) => {
            return Err(invalid_parameter_exception(
                FIELD_ENTITY_LIMIT,
                &limit.to_string(),
                &format!("a page holds between 1 and {MAX_PAGE_SIZE} {noun}"),
            ));
        }
    };
    // The match above leaves `requested` in `1..=MAX_PAGE_SIZE`, so this cannot
    // fail: the conversion is here to state that invariant, not to hide a case.
    Ok(usize::try_from(requested).expect("a page size in 1..=MAX_PAGE_SIZE fits a usize"))
}

/// The resource-kind segment of a wafv2 resource ARN (`webacl`, `ipset`,
/// `rulegroup`, …), or `None` when `value` is not one at all.
///
/// The ARN layout is the one [`is_wafv2_resource_arn`] pins:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`,
/// so the kind is the second slash-separated segment of the resource part.
pub fn arn_resource_kind(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(5)?.split('/').nth(1)
}

/// This service's Smithy namespace, for the shadow member schemas below.
const NAMESPACE: &str = "com.amazonaws.wafv2";

/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$` — the `@pattern` shared by
/// `LockToken` and `EntityId`
/// (`smithy_query(query="shape",shape="LockToken,EntityId")`), the form
/// [`new_uuid`] mints.
const UUID_PATTERN: &str = r"^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$";

/// `^[\w\.\-/]+$` — `TokenDomain`'s `@pattern`
/// (`smithy_query(query="shape",shape="TokenDomain")`).
const TOKEN_DOMAIN_PATTERN: &str = r"^[\w\.\-/]+$";

/// `^[\w#:\.\-/]+$` — `VersionKeyString`'s `@pattern`
/// (`smithy_query(query="shape",shape="VersionKeyString")`).
const VERSION_KEY_PATTERN: &str = r"^[\w#:\.\-/]+$";

/// `LockToken`: `@length(1..36)` plus [`UUID_PATTERN`].
static LOCK_TOKEN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(36))));
    traits.insert(Box::new(PatternTrait::new(UUID_PATTERN)));
    traits
});

/// `TokenDomain`: `@length(1..253)` plus [`TOKEN_DOMAIN_PATTERN`].
static TOKEN_DOMAIN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(253))));
    traits.insert(Box::new(PatternTrait::new(TOKEN_DOMAIN_PATTERN)));
    traits
});

/// `APIKeyTokenDomains`: `@length(min 1)` ELEMENTS, with `TokenDomain`'s own
/// constraints carried for each element exactly as codegen carries them
/// (`smithy_query(query="shape",shape="APIKeyTokenDomains")`).
static TOKEN_DOMAINS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), None)));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TOKEN_DOMAIN_TRAITS)));
    traits
});

/// `VersionKeyString`: `@length(1..64)` plus [`VERSION_KEY_PATTERN`].
static VERSION_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(64))));
    traits.insert(Box::new(PatternTrait::new(VERSION_KEY_PATTERN)));
    traits
});

/// `LockToken`, as every operation that takes an optimistic-lock token declares
/// it. The walk reads only a member schema's NAME (for the violation path) and its
/// trait map, so one declaration serves every such operation.
pub static M_LOCK_TOKEN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$LockToken",
        NAMESPACE,
        "LockToken",
    ),
    ShapeType::String,
    "LockToken",
    0,
)
.with_traits(&LOCK_TOKEN_TRAITS);

/// `TokenDomains`, which targets `APIKeyTokenDomains`.
pub static M_TOKEN_DOMAINS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$TokenDomains",
        NAMESPACE,
        "TokenDomains",
    ),
    ShapeType::List,
    "TokenDomains",
    1,
)
.with_traits(&TOKEN_DOMAINS_TRAITS);

/// `VersionsToPublish`, whose KEYS are `VersionKeyString`s.
///
/// A map's keys carry the key shape's constraints, and the request boundary's walk
/// visits a map's VALUES: a key that violates `VersionKeyString`'s own constraints
/// reaches the handler, which checks it here against those constraints re-declared
/// in the framework's own representation — so a version key refused by the handler
/// reads exactly like a member refused at the boundary. The clause names the map
/// member, because the key is part of that member's value.
pub static M_VERSIONS_TO_PUBLISH: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$VersionsToPublish",
        NAMESPACE,
        "VersionsToPublish",
    ),
    ShapeType::String,
    "VersionsToPublish",
    2,
)
.with_traits(&VERSION_KEY_TRAITS);

/// `TokenDomain`'s constraints as the aggregated clause lists them — every
/// constraint of the element shape, in one set, whichever of them an element
/// failed.
///
/// Spelled out because the framework's `Violation::requirement` wording is not
/// public: each sentence here is that function's own
/// (`Member must have length less than or equal to …`,
/// `Member must satisfy regular expression pattern: …`) and each bound is
/// `TokenDomain`'s, declared once in [`TOKEN_DOMAIN_TRAITS`] above. The ORDER is
/// the order WAF's request validator lists them: the length ceiling, the length
/// floor, then the pattern.
pub const TOKEN_DOMAIN_CONSTRAINTS: &[&str] = &[
    "Member must have length less than or equal to 253",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ^[\w\.\-/]+$",
];

/// `VersionKeyString`'s constraints as the aggregated clause lists them.
///
/// The counterpart of [`TOKEN_DOMAIN_CONSTRAINTS`] for a MAP KEY: the same
/// framework wording for each requirement, each bound `VersionKeyString`'s own
/// (declared once in [`VERSION_KEY_TRAITS`] above), in the order WAF's request
/// validator lists them — the length ceiling, the length floor, then the pattern.
pub const VERSION_KEY_CONSTRAINTS: &[&str] = &[
    "Member must have length less than or equal to 64",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ^[\w#:\.\-/]+$",
];

/// The root of the shadow input the walk is driven over. Its member list is empty
/// on purpose: the walk visits what [`ConstrainedMembers`] WRITES, which is
/// whichever members the calling operation handed over, and a root struct schema
/// carries no member name so it contributes no path segment.
static CONSTRAINED_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput",
        NAMESPACE,
        "ConstrainedInput",
    ),
    ShapeType::Structure,
    &[],
);

/// One value handed to the validating walk, written the way the codec writes that
/// shape so the walk builds the same path and runs the same checks.
pub enum ConstrainedValue<'a> {
    /// A string leaf.
    Text(&'a str),
    /// A list of strings; the walk names each element `<member>.<n>.member`.
    Texts(&'a [String]),
}

/// The members of ONE request whose constraints their operation handed over,
/// written for [`::aws_smithy_schema_server::constraints::validate`].
pub struct ConstrainedMembers<'a> {
    members: &'a [(&'static Schema, ConstrainedValue<'a>)],
}

impl SerializableStruct for ConstrainedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        for (schema, value) in self.members {
            match value {
                // A scalar list element is written against the COLLECTION member
                // schema, which is what `vendor/smithy-macros` emits and what lets
                // the walk tell the collection's element-count bound from the
                // elements' own bounds (the latter travel in
                // `ElementConstraintsTrait`).
                ConstrainedValue::Text(text) => s.write_string(schema, text)?,
                ConstrainedValue::Texts(texts) => s.write_list(schema, &|sl| {
                    for text in texts.iter() {
                        sl.write_string(schema, text)?;
                    }
                    Ok(())
                })?,
            }
        }
        Ok(())
    }
}

/// Run the framework's own constraint walk over the members an operation handed
/// over, and report every violation it found.
pub fn validate_members(members: &[(&'static Schema, ConstrainedValue<'_>)]) -> Vec<Violation> {
    validate(&ConstrainedMembers { members }, &CONSTRAINED_INPUT_SCHEMA)
}

/// The framework's own `ValidationException` envelope for `violations`, so a
/// member refused by a handler reads exactly like one refused at the boundary.
pub fn constraint_envelope(violations: &[Violation]) -> String {
    ::aws_smithy_schema_server::envelope(violations)
        .unwrap_or_else(|| "invalid request".to_string())
}

/// The envelope WAF's request validator answers for a collection of scalars,
/// given the violations the framework walk found in it — or `None` when it found
/// none.
///
/// WAF reports an ELEMENT that fails the element shape's own constraints against
/// the COLLECTION member's path, with the value rendered as the whole list and
/// every constraint of the element shape listed in one bracketed set: ONE clause
/// for the member, however many elements failed and however many constraints each
/// failed. A violation of the collection ITSELF — its element-count `@length` — is
/// a different constraint about a different value, so it keeps the framework's own
/// clause.
///
/// `member_path` is the collection member's path as the envelope names it (use
/// [`member_path`]), `elements` is the list as the request sent it, and
/// `element_constraints` is the element shape's constraint set (e.g.
/// [`TOKEN_DOMAIN_CONSTRAINTS`]).
pub fn collection_constraint_envelope(
    member_path: &str,
    elements: &[String],
    element_constraints: &[&str],
    violations: &[Violation],
) -> Option<String> {
    // The value every clause about this member echoes: the list, the way WAF
    // renders one.
    let rendered = format!("[{}]", echoed(&elements.join(", ")));
    let element_prefix = format!("{member_path}.");
    let mut clauses: Vec<String> = Vec::new();
    let mut element_clause_emitted = false;
    for violation in violations {
        // The collection ITSELF (its element-count `@length`): the framework's own
        // requirement wording, restated against the list as the value — the walk
        // renders a count there (`3 items`), and WAF renders the list.
        if violation.path == member_path {
            clauses.push(
                Violation {
                    path: violation.path.clone(),
                    kind: violation.kind.clone(),
                    value: Some(rendered.clone()),
                    sensitive: violation.sensitive,
                }
                .message(),
            );
            continue;
        }
        if !violation.path.starts_with(&element_prefix) {
            clauses.push(violation.message());
            continue;
        }
        // Every element failure of this member collapses into the one clause, so
        // it is emitted at the FIRST of them and skipped thereafter.
        if element_clause_emitted {
            continue;
        }
        element_clause_emitted = true;
        clauses.push(format!(
            "Value '{rendered}' at '{member_path}' failed to satisfy constraint: \
             Member must satisfy constraint: [{}]",
            element_constraints.join(", ")
        ));
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The envelope WAF's request validator answers for MAP KEYS that fail the key
/// shape's own constraints — or `None` when none of them do.
///
/// The map-key counterpart of [`collection_constraint_envelope`]: a map's keys carry
/// the key shape's constraints, and WAF reports a key that breaks one against the
/// MAP member's path, with the KEY as the value and every constraint of the key
/// shape listed in one bracketed set. One clause per offending key, since each key
/// is its own value.
///
/// `member_path` is the map member's path as the envelope names it (use
/// [`member_path`]), `keys` are the keys the request sent, `key_constraints` is the
/// key shape's constraint set (e.g. [`VERSION_KEY_CONSTRAINTS`]) and `key_schema` is
/// the shadow member schema carrying those constraints, over which the framework's
/// own walk decides which keys are at fault.
pub fn map_key_constraint_envelope(
    member_path: &str,
    keys: &[String],
    key_constraints: &[&str],
    key_schema: &'static Schema,
) -> Option<String> {
    let clauses: Vec<String> = keys
        .iter()
        .filter(|key| {
            !validate_members(&[(key_schema, ConstrainedValue::Text(key.as_str()))]).is_empty()
        })
        .map(|key| {
            format!(
                "Value '{}' at '{member_path}' failed to satisfy constraint: \
                 Map keys must satisfy constraint: [{}]",
                echoed(key),
                key_constraints.join(", ")
            )
        })
        .collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// A member's path as a validation clause names it: the Smithy member name in
/// lowerCamel.
///
/// Delegates to the framework, which owns that casing rule, so a clause a handler
/// writes cannot spell a position differently from one the boundary writes.
pub fn member_path(smithy_name: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(smithy_name)
}

/// The rejected value as it appears in a clause: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// The cap and its rendering are the framework's own (its `MAX_ECHOED_VALUE`, 256
/// bytes), re-stated here only because that helper is private. A caller controls
/// the length of what a clause echoes, so an unelided echo turns a large request
/// into an arbitrarily large response. It is applied to the RENDERING only: every
/// check runs on the value the caller actually sent.
fn echoed(value: &str) -> String {
    const MAX_ECHOED_VALUE: usize = 256;
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}
