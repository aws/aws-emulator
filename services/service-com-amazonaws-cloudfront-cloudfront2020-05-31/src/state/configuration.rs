// SPDX-License-Identifier: CC0-1.0

//! RESOLVING a submitted configuration, and the rules one is ADMITTED by.
//!
//! Two halves of the same concern, which is why they live together: what the service
//! itself fills in on a configuration a request left members out of — so a later read
//! answers with the configuration the resource actually runs under rather than a
//! re-derivation of it — and what a submitted configuration is refused for, with the
//! sentence each refusal states: the `Quantity`/`Items` agreements, the per-list quotas,
//! and the members whose values only make sense together.
//!
//! The parent module holds the records these configurations are stored on; this module
//! reads and writes nothing itself.

use super::*;

/// Resolve the members the service supplies on the default cache behaviour of a
/// distribution whose configuration is multi-tenant when `multi_tenant` says so.
pub(super) fn resolve_default_cache_behavior(
    behavior: DefaultCacheBehavior,
    multi_tenant: bool,
) -> DefaultCacheBehavior {
    resolve_behavior_members!(behavior, multi_tenant)
}

/// Resolve the members the service supplies on one ordered cache behaviour of a
/// distribution whose configuration is multi-tenant when `multi_tenant` says so.
pub(super) fn resolve_cache_behavior(behavior: CacheBehavior, multi_tenant: bool) -> CacheBehavior {
    resolve_behavior_members!(behavior, multi_tenant)
}

/// The methods a cache behaviour allows and caches: the ones it names, or
/// [`DEFAULT_ALLOWED_METHODS`] when it names none. A behaviour that names the allowed
/// methods but not the cached ones caches the same default set.
///
/// Either list is reported in [`EMITTED_METHOD_ORDER`] rather than in the order the
/// request listed it: `MethodsList` is an ordered XML list, and the service emits the
/// methods of the set selected in its own fixed order. The default set is written in that
/// order already, so the reordering is what a SUBMITTED list goes through.
pub(super) fn resolve_allowed_methods(allowed: Option<AllowedMethods>) -> AllowedMethods {
    let mut allowed = allowed.unwrap_or_else(|| AllowedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
        cached_methods: None,
    });
    allowed.items = in_emitted_order(&allowed.items);
    let mut cached = allowed.cached_methods.unwrap_or_else(|| CachedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
    });
    cached.items = in_emitted_order(&cached.items);
    allowed.cached_methods = Some(cached);
    allowed
}

/// [`DEFAULT_ALLOWED_METHODS`] as a `MethodsList`.
pub(super) fn default_methods() -> Vec<String> {
    DEFAULT_ALLOWED_METHODS
        .iter()
        .map(|method| (*method).to_string())
        .collect()
}

/// Resolve the list wrappers inside a legacy `ForwardedValues`: an absent `Headers` or
/// `QueryStringCacheKeys` forwards none and keys on none, which is the empty wrapper.
pub(super) fn resolve_forwarded_values(mut forwarded: ForwardedValues) -> ForwardedValues {
    forwarded.headers = Some(forwarded.headers.unwrap_or_default());
    forwarded.query_string_cache_keys = Some(forwarded.query_string_cache_keys.unwrap_or_default());
    forwarded
}

/// Resolve the members the service supplies on one custom error response.
pub(super) fn resolve_custom_error_response(mut response: CustomErrorResponse) -> CustomErrorResponse {
    response.error_caching_min_ttl = Some(
        response
            .error_caching_min_ttl
            .unwrap_or(DEFAULT_ERROR_CACHING_MIN_TTL),
    );
    response
}

/// Resolve the access-logging configuration. Every member of `LoggingConfig` carries a
/// Smithy `@default` — `Enabled` and `IncludeCookies` `false`, `Bucket` and `Prefix` the
/// empty string — which is also what a configuration that names no `Logging` at all gets:
/// "If you don't want to enable logging when you create a distribution ... specify `false`
/// for `Enabled`, and specify empty `Bucket` and `Prefix` elements."
pub(super) fn resolve_logging(logging: Option<LoggingConfig>) -> LoggingConfig {
    let mut logging = logging.unwrap_or_default();
    logging.enabled = Some(logging.enabled.unwrap_or(false));
    logging.include_cookies = Some(logging.include_cookies.unwrap_or(false));
    logging.bucket = Some(logging.bucket.unwrap_or_default());
    logging.prefix = Some(logging.prefix.unwrap_or_default());
    logging
}

/// Resolve the distribution's viewer certificate.
///
/// A configuration that names no certificate of its own is served on CloudFront's own
/// certificate: "If the distribution doesn't use `Aliases` ... set
/// `CloudFrontDefaultCertificate` to `true` and leave all other fields empty." The method
/// viewers connect over then follows HOW they reach the distribution, which is why
/// `serves_own_domain` decides it: a distribution answering under a CloudFront domain name
/// of its own is served at CloudFront's own addresses, where no viewer has to send the name
/// it asked for ([`SSL_SUPPORT_METHOD_DEDICATED_IP`]), while a multi-tenant one is reached
/// through a connection group that answers for the tenant whose name the viewer sent
/// ([`SSL_SUPPORT_METHOD_SNI_ONLY`]). The deprecated
/// `Certificate` and `CertificateSource` members report which certificate is in use and
/// where it is kept, so they follow from the certificate the configuration chose.
///
/// `MinimumProtocolVersion` is both SUPPLIED and OVERRIDDEN, and the certificate decides
/// which: under CloudFront's own certificate the security policy is the automatic
/// [`MINIMUM_PROTOCOL_VERSION_DEFAULT_CERTIFICATE`] — "If the distribution uses the
/// CloudFront domain name such as `d111111abcdef8.cloudfront.net` (you set
/// `CloudFrontDefaultCertificate` to `true`), CloudFront automatically sets the security
/// policy to `TLSv1` regardless of the value that you set here." A configuration that
/// DECLARED a policy keeps it unless both halves of that sentence hold, so
/// `serves_own_domain` carries the first: a distribution served on a CloudFront domain name
/// of its own has its declared policy overridden, while a multi-tenant one, which has no
/// CloudFront domain name at all and whose viewers reach it through a connection group,
/// keeps what it declared. One that declared nothing is served under the automatic policy
/// either way — the member is never left unset, since the distribution is served over a
/// security policy whether or not the request named one.
pub(super) fn resolve_viewer_certificate(
    certificate: Option<ViewerCertificate>,
    serves_own_domain: bool,
) -> ViewerCertificate {
    let mut resolved = certificate.unwrap_or_default();
    let acm = resolved.acm_certificate_arn.clone();
    let iam = resolved.iam_certificate_id.clone();
    if acm.is_none() && iam.is_none() {
        resolved.cloud_front_default_certificate =
            Some(resolved.cloud_front_default_certificate.unwrap_or(true));
    }
    if resolved.cloud_front_default_certificate == Some(true) {
        let served_over = if serves_own_domain {
            SSL_SUPPORT_METHOD_DEDICATED_IP
        } else {
            SSL_SUPPORT_METHOD_SNI_ONLY
        };
        resolved.ssl_support_method = Some(
            resolved
                .ssl_support_method
                .unwrap_or_else(|| served_over.to_string()),
        );
        if serves_own_domain || resolved.minimum_protocol_version.is_none() {
            resolved.minimum_protocol_version =
                Some(MINIMUM_PROTOCOL_VERSION_DEFAULT_CERTIFICATE.to_string());
        }
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_CLOUDFRONT.to_string());
    } else if let Some(arn) = acm {
        resolved.certificate = Some(arn);
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_ACM.to_string());
    } else if let Some(id) = iam {
        resolved.certificate = Some(id);
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_IAM.to_string());
    }
    resolved
}

/// The `InconsistentQuantities` message for one wrapper whose `Quantity` disagrees with
/// the length of its `Items`, or `None` when the two agree.
///
/// The sentence is CloudFront's own for this refusal, built from the request: it names
/// the wrapper, the `Quantity` the request stated and the number of `Items` it actually
/// carried, in that order — "The specified quantity of <wrapper> (<stated>) does not
/// match the actual quantity supplied (<carried>)." The model's doc text for the shape
/// ("The value of Quantity and the size of Items don't match.") describes the condition
/// rather than the message the service sends.
pub(super) fn quantity_disagreement(label: &str, quantity: i32, len: usize) -> Option<String> {
    if i64::from(quantity) == i64::try_from(len).unwrap_or(i64::MAX) {
        return None;
    }
    Some(format!(
        "The specified quantity of {label} ({quantity}) does not match the actual quantity \
         supplied ({len})."
    ))
}

/// The `InvalidArgument` message for a SWITCHED list wrapper whose `Enabled` flag and `Items`
/// disagree, or `None` when they agree.
///
/// A wrapper that carries an `Enabled` flag beside its list states a switch over that list,
/// and the two members are not independent: the switch is what the list is FOR, so an enabled
/// wrapper with nothing in it switches nothing on and a disabled one carrying entries states
/// entries nothing reads. CloudFront refuses both directions, and the two sentences are
/// distinct so a caller is told which way round its wrapper is wrong; the only variable part
/// of either is the wrapper's own member name, as the model declares it.
///
/// `len` is the number of `Items` the wrapper really carries — the same count
/// [`quantity_disagreement`] compares `Quantity` against, and read AFTER it, so a wrapper
/// whose stated length is wrong is refused on that first.
pub(super) fn enabled_items_disagreement(label: &str, enabled: bool, len: usize) -> Option<String> {
    match (enabled, len) {
        (true, 0) => Some(format!("The parameter {label} must contain items if enabled.")),
        (false, 1..) => Some(format!(
            "The parameter {label} must not contain items if disabled."
        )),
        _ => None,
    }
}

/// The list wrappers and the direct-only scalars one cache behaviour carries.
///
/// `DefaultCacheBehavior` and `CacheBehavior` declare the same members, so both are
/// checked through this one view rather than through two copies of the same walk.
pub(super) struct BehaviorLists<'a> {
    pub(super) trusted_signers: Option<&'a TrustedSigners>,
    pub(super) trusted_key_groups: Option<&'a TrustedKeyGroups>,
    pub(super) allowed_methods: Option<&'a AllowedMethods>,
    pub(super) lambda_function_associations: Option<&'a LambdaFunctionAssociations>,
    pub(super) function_associations: Option<&'a FunctionAssociations>,
    pub(super) forwarded_values: Option<&'a ForwardedValues>,
    /// Whether the behaviour states `SmoothStreaming`, one of the members documented as
    /// supported only on a standard distribution.
    pub(super) states_smooth_streaming: bool,
    /// Whether the behaviour states each of the three deprecated per-behaviour TTLs, which
    /// carry the same note: `MinTTL`, `DefaultTTL` and `MaxTTL`, in that order.
    pub(super) states_ttls: [bool; 3],
}

impl<'a> From<&'a DefaultCacheBehavior> for BehaviorLists<'a> {
    fn from(behavior: &'a DefaultCacheBehavior) -> Self {
        Self {
            trusted_signers: behavior.trusted_signers.as_ref(),
            trusted_key_groups: behavior.trusted_key_groups.as_ref(),
            allowed_methods: behavior.allowed_methods.as_ref(),
            lambda_function_associations: behavior.lambda_function_associations.as_ref(),
            function_associations: behavior.function_associations.as_ref(),
            forwarded_values: behavior.forwarded_values.as_ref(),
            states_smooth_streaming: behavior.smooth_streaming.is_some(),
            states_ttls: [
                behavior.min_ttl.is_some(),
                behavior.default_ttl.is_some(),
                behavior.max_ttl.is_some(),
            ],
        }
    }
}

impl<'a> From<&'a CacheBehavior> for BehaviorLists<'a> {
    fn from(behavior: &'a CacheBehavior) -> Self {
        Self {
            trusted_signers: behavior.trusted_signers.as_ref(),
            trusted_key_groups: behavior.trusted_key_groups.as_ref(),
            allowed_methods: behavior.allowed_methods.as_ref(),
            lambda_function_associations: behavior.lambda_function_associations.as_ref(),
            function_associations: behavior.function_associations.as_ref(),
            forwarded_values: behavior.forwarded_values.as_ref(),
            states_smooth_streaming: behavior.smooth_streaming.is_some(),
            states_ttls: [
                behavior.min_ttl.is_some(),
                behavior.default_ttl.is_some(),
                behavior.max_ttl.is_some(),
            ],
        }
    }
}

impl BehaviorLists<'_> {
    /// The first member of this behaviour that a MULTI-TENANT distribution may not state,
    /// or `None` when it states none of them. See
    /// [`multi_tenant_direct_only_member`] for the rule and the member list.
    pub(super) fn direct_only_member(&self) -> Option<&'static str> {
        if self.forwarded_values.is_some() {
            return Some("ForwardedValues");
        }
        for (stated, member) in self
            .states_ttls
            .iter()
            .zip(["MinTTL", "DefaultTTL", "MaxTTL"])
        {
            if *stated {
                return Some(member);
            }
        }
        if self.states_smooth_streaming {
            return Some("SmoothStreaming");
        }
        if self
            .trusted_signers
            .is_some_and(|signers| signers.quantity > 0 || items_len(&signers.items) > 0)
        {
            return Some("TrustedSigners");
        }
        None
    }

    /// Record every SWITCHED wrapper this behaviour carries — the ones whose `Enabled` flag
    /// states a switch over their own list — each under its own member name, beside the
    /// number of `Items` it carries.
    ///
    /// The two signer wrappers are those: `TrustedSigners` and `TrustedKeyGroups` each state
    /// "whether … is enabled" beside the list the switch governs
    /// (`smithy_query(query="shape",shape="TrustedSigners,TrustedKeyGroups")`), and no other
    /// wrapper a cache behaviour carries has such a flag.
    pub(super) fn collect_switches(&self, switches: &mut Vec<(&'static str, bool, usize)>) {
        if let Some(signers) = self.trusted_signers {
            switches.push(("TrustedSigners", signers.enabled, items_len(&signers.items)));
        }
        if let Some(groups) = self.trusted_key_groups {
            switches.push(("TrustedKeyGroups", groups.enabled, items_len(&groups.items)));
        }
    }

    /// Record every wrapper this behaviour carries, each under its OWN member name.
    pub(super) fn collect(&self, counts: &mut Vec<(&'static str, i32, usize)>) {
        if let Some(signers) = self.trusted_signers {
            counts.push((
                "TrustedSigners",
                signers.quantity,
                items_len(&signers.items),
            ));
        }
        if let Some(groups) = self.trusted_key_groups {
            counts.push((
                "TrustedKeyGroups",
                groups.quantity,
                items_len(&groups.items),
            ));
        }
        if let Some(methods) = self.allowed_methods {
            counts.push(("AllowedMethods", methods.quantity, methods.items.len()));
            if let Some(cached) = &methods.cached_methods {
                counts.push(("CachedMethods", cached.quantity, cached.items.len()));
            }
        }
        if let Some(lambdas) = self.lambda_function_associations {
            counts.push((
                "LambdaFunctionAssociations",
                lambdas.quantity,
                items_len(&lambdas.items),
            ));
        }
        if let Some(functions) = self.function_associations {
            counts.push((
                "FunctionAssociations",
                functions.quantity,
                items_len(&functions.items),
            ));
        }
        if let Some(forwarded) = self.forwarded_values {
            if let Some(names) = &forwarded.cookies.whitelisted_names {
                counts.push((
                    "WhitelistedNames",
                    names.quantity,
                    items_len(&names.items),
                ));
            }
            if let Some(headers) = &forwarded.headers {
                counts.push(("Headers", headers.quantity, items_len(&headers.items)));
            }
            if let Some(keys) = &forwarded.query_string_cache_keys {
                counts.push((
                    "QueryStringCacheKeys",
                    keys.quantity,
                    items_len(&keys.items),
                ));
            }
        }
    }
}

/// The sentence CloudFront refuses a configuration whose alternate domain names no
/// certificate it names can serve with: the whole text is fixed — it names neither the
/// alternate domain names the configuration declared nor the certificate members it
/// stated — so every such configuration is refused with the one sentence, which points at
/// the alternate-domain-name requirements rather than restating them. Recorded behaviour.
pub const INVALID_VIEWER_CERTIFICATE_MESSAGE: &str =
    "To add an alternate domain name (CNAME) to a CloudFront distribution, you must attach \
     a trusted certificate that validates your authorization to use the domain name. For \
     more details, see: https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/\
     CNAMEs.html#alternate-domain-names-requirements";

/// The `InvalidViewerCertificate` message for a configuration whose alternate domain
/// names no certificate IT names can serve, or `None` when the two agree.
///
/// `ViewerCertificate` states the rule for both directions: "If the distribution doesn't
/// use `Aliases` (also known as alternate domain names or CNAMEs) … set
/// `CloudFrontDefaultCertificate` to `true` and leave all other fields empty", and "If you
/// want viewers to use HTTPS with an alternate domain name … you must choose one of
/// `ACMCertificateArn` or `IAMCertificateId`" — the certificate has to validate the
/// authorization to serve the domain name, which CloudFront's own
/// `*.cloudfront.net` certificate cannot do for a name that is not under it. So a
/// configuration that declares at least one alias has to name exactly ONE custom
/// certificate, and a configuration that names none of them — the default certificate, by
/// itself or by omission — may declare no alias.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, so no stored
/// distribution ever carries an alias its certificate does not cover: the condition is a
/// property of the submitted configuration alone, which is why it lives here beside
/// [`config_quantity_mismatch`] rather than in one handler.
pub fn alias_certificate_mismatch(config: &DistributionConfig) -> Option<&'static str> {
    let aliases: &[String] = config
        .aliases
        .as_ref()
        .and_then(|aliases| aliases.items.as_deref())
        .unwrap_or_default();
    if aliases.is_empty() {
        return None;
    }
    let certificate = config.viewer_certificate.as_ref();
    let (acm, iam) = custom_certificate_members(certificate);
    let default = certificate
        .is_none_or(|certificate| certificate.cloud_front_default_certificate == Some(true));
    // Exactly ONE custom certificate, and CloudFront's own certificate not stated beside
    // it: the configuration a distribution with an alternate domain name is created under.
    // Anything else — no custom certificate at all (the default certificate, by itself or
    // by omission), both custom members at once, or a custom member beside the default one
    // — names no single certificate that validates the authorization to serve the name,
    // and every one of them is refused with the ONE sentence the service states for the
    // class ([`INVALID_VIEWER_CERTIFICATE_MESSAGE`]): it carries no part of the request, so
    // it cannot describe which of the forms was submitted.
    if (acm != iam) && !default {
        return None;
    }
    Some(INVALID_VIEWER_CERTIFICATE_MESSAGE)
}

/// The `InvalidLambdaFunctionAssociation` message for a configuration carrying a
/// Lambda@Edge association whose ARN names no published function version, or `None` when
/// every association it carries names one.
///
/// `LambdaFunctionAssociation$LambdaFunctionARN` states the rule: "You must specify the ARN
/// of a function version; you can't specify an alias or $LATEST." A function version is
/// addressed by the function's ARN carrying that version's NUMBER as its qualifier, so an
/// association whose value is no Lambda function ARN, carries no qualifier at all, or
/// carries one that is not a published version number — the `$LATEST` of a function nobody
/// published, or an alias name — names no version at all.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, and for EVERY
/// cache behaviour of it (the default one and each ordered one), so no stored distribution
/// ever carries an edge association that names nothing invocable: the condition is a
/// property of the submitted configuration alone, which is why it lives here beside
/// [`alias_certificate_mismatch`] rather than in one handler.
///
/// The message names the FIRST association that breaks the rule, read in the order the
/// configuration states them — the default cache behaviour, then the ordered ones — and
/// echoes that entry's `LambdaFunctionARN` as submitted, since that value is what tells a
/// caller which of a configuration's associations was refused
/// ([`invalid_lambda_function_association_message`]).
pub fn lambda_association_without_version(config: &DistributionConfig) -> Option<String> {
    behavior_views(config)
        .iter()
        .filter_map(|behavior| behavior.lambda_function_associations)
        .flat_map(|associations| associations.items.as_deref().unwrap_or_default())
        .find(|association| !names_lambda_function_version(&association.lambda_function_arn))
        .map(|association| {
            invalid_lambda_function_association_message(&association.lambda_function_arn)
        })
}

/// The first member a MULTI-TENANT distribution's configuration may not state, or `None`
/// when it states none of them — and `None` for a configuration that describes a STANDARD
/// distribution, which is the kind every one of these members belongs to.
///
/// The model documents each of them the same way — "This field only supports standard
/// distributions. You can't specify this field for multi-tenant distributions", or, for the
/// ones a connection group carries instead, "To use this field for a multi-tenant
/// distribution, use a connection group instead" — so a multi-tenant configuration that
/// states one describes a distribution CloudFront would not serve, and it is refused with
/// the sentence that names the member ([`multi_tenant_direct_only_message`]). The members,
/// in the order a configuration is walked for them: `Aliases`, `PriceClass`,
/// `IsIPV6Enabled`, `ContinuousDeploymentPolicyId`, `Staging` and `AnycastIpListId` on the
/// configuration itself, and then, for the default cache behaviour and each ordered one in
/// turn, the legacy cache-key group (`ForwardedValues` and the three deprecated TTLs that
/// travel with it), `SmoothStreaming` and `TrustedSigners`
/// ([`BehaviorLists::direct_only_member`]).
///
/// A member whose value says NOTHING is not a statement of it: a list wrapper carrying no
/// entry ("specify an empty element") and an empty identifier name nothing, exactly as they
/// name nothing in the rules that read those members
/// ([`origin_with_both_access_mechanisms`] reads an empty `OriginAccessIdentity` that way).
/// A scalar the configuration carries at all is one it states.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, so no stored
/// multi-tenant distribution ever carries a member only a standard one has: the condition is
/// a property of the submitted configuration alone, which is why it lives here beside
/// [`alias_certificate_mismatch`] rather than in one handler. It is also what makes the
/// resolution of a multi-tenant configuration materialise none of them
/// ([`resolve_distribution_config`]): a member the service refuses on the way in is not one
/// it supplies a default for.
pub fn multi_tenant_direct_only_member(config: &DistributionConfig) -> Option<&'static str> {
    if !is_multi_tenant(config) {
        return None;
    }
    let names = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    };
    if config
        .aliases
        .as_ref()
        .is_some_and(|aliases| aliases.quantity > 0 || items_len(&aliases.items) > 0)
    {
        return Some("Aliases");
    }
    if config.price_class.is_some() {
        return Some("PriceClass");
    }
    if config.is_ipv6_enabled.is_some() {
        return Some("IsIPV6Enabled");
    }
    if names(&config.continuous_deployment_policy_id) {
        return Some("ContinuousDeploymentPolicyId");
    }
    if config.staging.is_some() {
        return Some("Staging");
    }
    if names(&config.anycast_ip_list_id) {
        return Some("AnycastIpListId");
    }
    behavior_views(config)
        .iter()
        .find_map(BehaviorLists::direct_only_member)
}

/// The sentence CloudFront refuses a multi-tenant distribution's configuration for stating
/// `member` with, on the wire: the fixed text naming the member the configuration may not
/// carry, which is the one variable part of it and comes from the MODEL's own name for that
/// member ([`multi_tenant_direct_only_member`]).
pub fn multi_tenant_direct_only_message(member: &str) -> String {
    format!("You can't specify the {member} field for a multi-tenant distribution.")
}

/// The refusal a create is answered with at this emulator's distribution bound
/// ([`MAX_DISTRIBUTIONS`]) — the one sentence both the store's own check and a handler's
/// pre-check answer it with, so the two cannot word the same bound differently.
pub fn too_many_distributions() -> CreateRefusal {
    CreateRefusal::TooManyDistributions(format!(
        "This emulator holds at most {MAX_DISTRIBUTIONS} distributions; delete one before \
         creating another"
    ))
}

/// Whether an origin of `config` names BOTH an origin access control and an origin access
/// identity, which is the configuration `IllegalOriginAccessConfiguration` refuses: "An
/// origin cannot contain both an origin access control (OAC) and an origin access identity
/// (OAI)."
///
/// The two are the two ways ONE origin reaches a private Amazon S3 bucket — `Origin$
/// OriginAccessControlId` names the control whose credentials sign the origin request, and
/// `S3OriginConfig$OriginAccessIdentity` names the identity CloudFront reaches the bucket
/// as — so an origin naming both says nothing about which of them signs it, and the
/// configuration is refused rather than stored. Either member is documented EMPTY for an
/// origin that names none ("specify an empty `OriginAccessIdentity` element"), so an
/// absent or empty value is not a reference and an origin carrying one of them is fine.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, and for EVERY
/// origin of it, so no stored origin ever carries both: the condition is a property of the
/// submitted configuration alone, which is why it lives here beside
/// [`alias_certificate_mismatch`] and [`lambda_association_without_version`] rather than
/// in one handler.
pub fn origin_with_both_access_mechanisms(config: &DistributionConfig) -> bool {
    config.origins.items.iter().any(|origin| {
        let control = origin
            .origin_access_control_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
        let identity = origin
            .s3_origin_config
            .as_ref()
            .is_some_and(|s3| !s3.origin_access_identity.trim().is_empty());
        control && identity
    })
}

/// Whether `arn` names a PUBLISHED Lambda function version.
///
/// A version is addressed by the function's own ARN —
/// `arn:<partition>:lambda:<region>:<account>:function:<name>` — qualified with the version,
/// and a published version's qualifier is the number that was assigned when it was
/// published, counting from 1. So every other value names no version: one that is not a
/// Lambda function ARN at all, the unqualified function ARN, the `$LATEST` qualifier of the
/// unpublished function, and an alias name.
pub(super) fn names_lambda_function_version(arn: &str) -> bool {
    // The eight segments of a function-version ARN, read in order and without collecting
    // them: a value with fewer segments leaves a `None` behind, and one with more is
    // refused by the final `is_none`.
    let mut segments = arn.split(':');
    let prefix = segments.next();
    let partition = segments.next();
    let service = segments.next();
    let region = segments.next();
    let account = segments.next();
    let resource = segments.next();
    let name = segments.next();
    let qualifier = segments.next();
    let named = |segment: Option<&str>| segment.is_some_and(|segment| !segment.is_empty());
    prefix == Some("arn")
        && service == Some("lambda")
        && resource == Some("function")
        && named(partition)
        && named(region)
        && named(account)
        && named(name)
        && qualifier.is_some_and(is_published_version)
        && segments.next().is_none()
}

/// Whether `qualifier` is a published version number: a decimal number counting from 1,
/// so an empty qualifier, one carrying any non-digit (`$LATEST` and every alias name do)
/// and one written with a leading zero are all not one.
pub(super) fn is_published_version(qualifier: &str) -> bool {
    !qualifier.is_empty()
        && !qualifier.starts_with('0')
        && qualifier.bytes().all(|byte| byte.is_ascii_digit())
}

/// Which of the two CUSTOM certificate members a `ViewerCertificate` names: the ACM
/// certificate ARN and the IAM certificate id, in that order.
///
/// An empty member names no certificate: it is "left empty" exactly as the
/// default-certificate wording asks for. ONE reading of the pair, so the rule that binds a
/// distribution's aliases to its certificate ([`alias_certificate_mismatch`]) and the
/// question of whether a distribution carries a custom certificate at all
/// ([`names_custom_certificate`]) cannot disagree about one configuration.
pub(super) fn custom_certificate_members(certificate: Option<&ViewerCertificate>) -> (bool, bool) {
    let named = |value: Option<&String>| value.is_some_and(|value| !value.is_empty());
    (
        named(certificate.and_then(|certificate| certificate.acm_certificate_arn.as_ref())),
        named(certificate.and_then(|certificate| certificate.iam_certificate_id.as_ref())),
    )
}

/// Whether `certificate` names a CUSTOM certificate — one kept in ACM or in IAM — rather
/// than CloudFront's own default certificate.
///
/// `ViewerCertificate` is the certificate a distribution serves its viewers over, and a
/// custom one is what "validates the authorization to serve the domain name": CloudFront's
/// own default certificate "serves only the distribution's own CloudFront domain name", so
/// it covers no alternate domain name at all. Which names a certificate kept in ACM or IAM
/// covers is no record of this service's, so a custom one counts for the domain asked
/// about and the default certificate for none — the same reading
/// [`StoredDistributionTenant::certificate_covers_domain`] gives a tenant's certificate
/// ARN.
pub fn names_custom_certificate(certificate: Option<&ViewerCertificate>) -> bool {
    let (acm, iam) = custom_certificate_members(certificate);
    acm || iam
}

/// Whether anything a distribution-tenant call has to hand provides a TLS certificate for the
/// domains that tenant claims: a certificate its `Customizations` name, or a custom
/// certificate on the multi-tenant distribution it sits under.
///
/// This is the question the sentence for a domain nothing proves ENUMERATES
/// ([`MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE`] — "at least one certificate in the distribution
/// tenant or distribution, or … a managed certificate request"), so it is what decides WHICH
/// of the two refusals that condition carries: with none of the three present the caller is
/// told the requirement it has not met, and with one of them present the refusal is about the
/// DOMAIN, which the certificate it holds validates nothing for.
pub fn names_a_certificate(
    customizations: Option<&Customizations>,
    distribution_certificate: Option<&ViewerCertificate>,
) -> bool {
    customizations.is_some_and(|customizations| customizations.certificate.is_some())
        || names_custom_certificate(distribution_certificate)
}

/// The refusal a submitted configuration carries BY ITSELF, with nothing of the store or of
/// another service read: the quantity agreements ([`config_quantity_mismatch`]), the ports its
/// origins name ([`origin_port_refusal`]), the per-list quotas ([`config_limit_exceeded`]) and
/// then the domain every one of its own members has to be inside
/// ([`config_member_refusal`]), in that order.
///
/// The ONE reading of those, so the shared create path
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::create_distribution`]) and a handler
/// that answers them BEFORE it resolves the configuration's Lambda@Edge associations against
/// the services that hold the function and its execution role
/// ([`super::edge_association_role_refusal`]) cannot order or word them differently. A
/// request refused here costs nothing but this walk of its own body.
pub fn config_refusal_without_state(config: &DistributionConfig) -> Option<CreateRefusal> {
    if let Some(message) = config_quantity_mismatch(config) {
        return Some(CreateRefusal::InconsistentQuantities(message));
    }
    if let Some(message) = config_switch_disagreement(config) {
        return Some(CreateRefusal::InvalidArgument(message));
    }
    if let Some(message) = origin_port_refusal(config) {
        return Some(CreateRefusal::InvalidArgument(message));
    }
    if let Some(limit) = config_limit_exceeded(config) {
        return Some(CreateRefusal::ConfigLimit(limit));
    }
    config_member_refusal(config).map(CreateRefusal::ConfigMember)
}

/// The two ports every origin of CloudFront's reaches a backend on by DEFAULT, from
/// `CustomOriginConfig$HTTPPort` and `$HTTPSPort` ("The default value is `80`" and "`443`"),
/// which are also the two privileged ports an origin may be reached on at all
/// ([`is_origin_port`]).
pub const DEFAULT_ORIGIN_PORTS: [i32; 2] = [80, 443];

/// The lowest UNPRIVILEGED port an origin may be reached on: every port from here up to
/// [`MAX_ORIGIN_PORT`] is one CloudFront connects to an origin on.
pub const MIN_UNPRIVILEGED_ORIGIN_PORT: i32 = 1024;

/// The highest port a TCP endpoint has, and so the highest an origin can be reached on.
pub const MAX_ORIGIN_PORT: i32 = 65_535;

/// Whether `port` is a port CloudFront connects to an origin on: one of the two defaults
/// ([`DEFAULT_ORIGIN_PORTS`]) or an unprivileged port
/// ([`MIN_UNPRIVILEGED_ORIGIN_PORT`]..=[`MAX_ORIGIN_PORT`]).
///
/// The set is the service's own, stated for the ports an origin configuration names — the
/// documented defaults, 80 and 443, plus the unprivileged range — so every member of a
/// configuration that carries a port is read with this one predicate, whether the origin is a
/// custom origin of a distribution or the endpoint of a VPC origin, and a port outside it is
/// refused with the one sentence this service states for that condition
/// ([`ORIGIN_PORT_OUT_OF_RANGE_MESSAGE`]).
pub fn is_origin_port(port: i32) -> bool {
    DEFAULT_ORIGIN_PORTS.contains(&port)
        || (MIN_UNPRIVILEGED_ORIGIN_PORT..=MAX_ORIGIN_PORT).contains(&port)
}

/// The sentence CloudFront refuses a port no origin can be reached on with: the whole text is
/// fixed — it names the kind of parameter rather than the member that carried it or the value
/// it carried — so every port of every origin configuration is refused the same way. Recorded
/// behaviour.
pub const ORIGIN_PORT_OUT_OF_RANGE_MESSAGE: &str =
    "The parameter origin port is not within allowed range.";

/// The modelled `InvalidArgument` (HTTP 400) for a port no origin can be reached on.
pub fn origin_port_out_of_range() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(ORIGIN_PORT_OUT_OF_RANGE_MESSAGE.to_string()),
    }
}

/// The refusal `config` carries when one of its origins names a port no origin can be reached
/// on, or `None` when every port it states is one ([`is_origin_port`]).
///
/// Only a CUSTOM origin states ports — an S3 origin is reached over the bucket's own endpoint
/// and a VPC origin names the VPC origin whose own endpoint configuration carries them — so
/// those are the members walked here, both of them, since the rule is about a port rather than
/// about which member it arrived in.
pub fn origin_port_refusal(config: &DistributionConfig) -> Option<String> {
    config
        .origins
        .items
        .iter()
        .filter_map(|origin| origin.custom_origin_config.as_ref())
        .flat_map(|custom| [custom.http_port, custom.https_port])
        .any(|port| !is_origin_port(port))
        .then(|| ORIGIN_PORT_OUT_OF_RANGE_MESSAGE.to_string())
}

/// A count in a submitted configuration that exceeds what this service stores, and the
/// message that says so. Each variant names the modelled error the handler answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLimit {
    /// More ordered cache behaviours than [`MAX_CACHE_BEHAVIORS`] —
    /// `TooManyCacheBehaviors`.
    CacheBehaviors(String),
    /// More trusted signers than [`MAX_TRUSTED_SIGNERS`] — `TooManyTrustedSigners`.
    TrustedSigners(String),
    /// More trusted key groups than [`MAX_TRUSTED_KEY_GROUPS`] —
    /// `TooManyKeyGroupsAssociatedToDistribution`.
    TrustedKeyGroups(String),
}

/// The first count in `config` that exceeds this service's maximum, or `None` when every
/// one of them is within it.
///
/// A stored configuration is walked on every `Distribution` payload — the default cache
/// behaviour and each ordered one, and the trusted signers and key groups they name — so a
/// configuration's counts bound the work every later read of that distribution does. They
/// are therefore checked on the way IN, before anything is stored, against the published
/// quotas ([`MAX_CACHE_BEHAVIORS`], [`MAX_TRUSTED_SIGNERS`], [`MAX_TRUSTED_KEY_GROUPS`]),
/// and refused with the error CloudFront models for each.
pub fn config_limit_exceeded(config: &DistributionConfig) -> Option<ConfigLimit> {
    let behaviors = config
        .cache_behaviors
        .as_ref()
        .map_or(0, |behaviors| items_len(&behaviors.items));
    if behaviors > MAX_CACHE_BEHAVIORS {
        return Some(ConfigLimit::CacheBehaviors(
            TOO_MANY_CACHE_BEHAVIORS_MESSAGE.to_string(),
        ));
    }

    let lists = behavior_views(config);
    // ASSOCIATED, not referenced: a trusted signer and a trusted key group are each
    // associated WITH THE DISTRIBUTION, and a behaviour names one by id — so the same account
    // or the same group named by several behaviours is one association, counted once, which
    // is also the set the distribution reports as active
    // ([`StoredDistribution::active_trusted_signers`],
    // [`StoredDistribution::active_trusted_key_groups`], both built with [`first_seen`]).
    // Counting references instead would refuse the ordinary signed-URL configuration, where
    // every behaviour trusts the one key group.
    let signers = first_seen(
        lists
            .iter()
            .filter_map(|behavior| behavior.trusted_signers)
            .flat_map(|signers| signers.items.as_deref().unwrap_or_default()),
    );
    if signers.len() > MAX_TRUSTED_SIGNERS {
        return Some(ConfigLimit::TrustedSigners(
            TOO_MANY_TRUSTED_SIGNERS_MESSAGE.to_string(),
        ));
    }

    let key_groups = first_seen(
        lists
            .iter()
            .filter_map(|behavior| behavior.trusted_key_groups)
            .flat_map(|groups| groups.items.as_deref().unwrap_or_default()),
    );
    if key_groups.len() > MAX_TRUSTED_KEY_GROUPS {
        return Some(ConfigLimit::TrustedKeyGroups(
            TOO_MANY_KEY_GROUPS_ASSOCIATED_MESSAGE.to_string(),
        ));
    }

    None
}

/// The sentence CloudFront refuses a configuration stating more cache behaviours than the
/// distribution quota admits with, on the wire.
///
/// FIXED text, naming the MEMBER whose count broke the quota: the condition is a property of
/// that member of the request rather than of any count in it, so neither the number submitted
/// nor the quota itself belongs in it — which is how the recorded service words this refusal.
pub const TOO_MANY_CACHE_BEHAVIORS_MESSAGE: &str =
    "Your request contains more CacheBehaviors than are allowed per distribution.";

/// The sentence CloudFront refuses a configuration associating more trusted signers with the
/// distribution than the quota admits with — the modelled `TooManyTrustedSigners`, worded as
/// the model documents that error shape ("Your request contains more trusted signers than are
/// allowed per distribution.") and fixed for the reason
/// [`TOO_MANY_CACHE_BEHAVIORS_MESSAGE`] is.
pub const TOO_MANY_TRUSTED_SIGNERS_MESSAGE: &str =
    "Your request contains more trusted signers than are allowed per distribution.";

/// The sentence CloudFront refuses a configuration associating more key groups with the
/// distribution than the quota admits with — the modelled
/// `TooManyKeyGroupsAssociatedToDistribution`.
///
/// The WIRE text, which differs from the sentence the MODEL's documentation of the error
/// shape uses ("The number of key groups referenced by this distribution is more than the
/// maximum allowed."): that one documents the condition, this one is what the service
/// answers. It states what the request would do to the account's quota rather than any count,
/// and is fixed for the reason [`TOO_MANY_CACHE_BEHAVIORS_MESSAGE`] is. Do not "correct" this
/// constant to the documentation sentence.
///
/// NOT [`TOO_MANY_KEY_GROUPS_MESSAGE`], which is the ACCOUNT's own key-group quota.
pub const TOO_MANY_KEY_GROUPS_ASSOCIATED_MESSAGE: &str =
    "Processing your request will cause you to exceed the maximum number of key groups that \
     can be associated to a distribution.";

/// Every cache behaviour of `config` as the view that names the list wrappers they share:
/// the default cache behaviour first, then the ordered ones.
pub(super) fn behavior_views(config: &DistributionConfig) -> Vec<BehaviorLists<'_>> {
    let mut out = vec![BehaviorLists::from(&config.default_cache_behavior)];
    if let Some(behaviors) = &config.cache_behaviors {
        for behavior in behaviors.items.as_deref().unwrap_or_default() {
            out.push(BehaviorLists::from(behavior));
        }
    }
    out
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in `config`
/// whose `Quantity` disagrees with the length of its `Items`, or `None` when every
/// wrapper agrees.
///
/// Every CloudFront list travels in a wrapper that states its own length (`Quantity`)
/// beside the list itself (`Items`), and the service refuses a request where the two
/// disagree: "The value of Quantity and the size of Items don't match." The rule holds
/// for EVERY wrapper, not for a chosen few, so this walks all of the ones a
/// `DistributionConfig` reaches.
///
/// Each wrapper is named by its OWN member name, as the model declares it, and by nothing of
/// the path that reaches it: the condition is a property of that one wrapper — its stated
/// length against the list beside it — so the behaviour or the origin that happens to hold
/// it is no part of what the caller has to fix.
pub fn config_quantity_mismatch(config: &DistributionConfig) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(aliases) = &config.aliases {
        counts.push(("Aliases", aliases.quantity, items_len(&aliases.items)));
    }
    counts.push((
        "Origins",
        config.origins.quantity,
        config.origins.items.len(),
    ));
    for origin in &config.origins.items {
        if let Some(headers) = &origin.custom_headers {
            counts.push((
                "CustomHeaders",
                headers.quantity,
                items_len(&headers.items),
            ));
        }
        if let Some(protocols) = origin
            .custom_origin_config
            .as_ref()
            .and_then(|custom| custom.origin_ssl_protocols.as_ref())
        {
            counts.push((
                "OriginSslProtocols",
                protocols.quantity,
                protocols.items.len(),
            ));
        }
    }
    if let Some(groups) = &config.origin_groups {
        counts.push(("OriginGroups", groups.quantity, items_len(&groups.items)));
        for group in groups.items.as_deref().unwrap_or_default() {
            let codes = &group.failover_criteria.status_codes;
            counts.push(("StatusCodes", codes.quantity, codes.items.len()));
            counts.push((
                "Members",
                group.members.quantity,
                group.members.items.len(),
            ));
        }
    }
    BehaviorLists::from(&config.default_cache_behavior).collect(&mut counts);
    if let Some(behaviors) = &config.cache_behaviors {
        counts.push((
            "CacheBehaviors",
            behaviors.quantity,
            items_len(&behaviors.items),
        ));
        for behavior in behaviors.items.as_deref().unwrap_or_default() {
            BehaviorLists::from(behavior).collect(&mut counts);
        }
    }
    if let Some(responses) = &config.custom_error_responses {
        counts.push((
            "CustomErrorResponses",
            responses.quantity,
            items_len(&responses.items),
        ));
    }
    if let Some(restrictions) = &config.restrictions {
        let geo = &restrictions.geo_restriction;
        counts.push(("GeoRestriction", geo.quantity, items_len(&geo.items)));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InvalidArgument` message for the first SWITCHED list wrapper in `config`
/// whose `Enabled` flag and `Items` disagree, or `None` when every one of them agrees.
///
/// The switched wrappers are the signer ones a cache behaviour carries
/// ([`BehaviorLists::collect_switches`]), and the rule holds for EVERY behaviour of a
/// configuration — the default one and each ordered one — so this walks all of them in the
/// order the configuration states them, which is the order a caller reads its own document
/// in. Each is reported by its own member name and by nothing of the path that reaches it:
/// the condition is a property of that one wrapper ([`enabled_items_disagreement`]).
///
/// Read AFTER the stated lengths agree with the lists beside them
/// ([`config_quantity_mismatch`]), so a wrapper that is wrong about its own length is refused
/// on that rather than on the switch.
pub fn config_switch_disagreement(config: &DistributionConfig) -> Option<String> {
    let mut switches: Vec<(&'static str, bool, usize)> = Vec::new();
    for behavior in behavior_views(config) {
        behavior.collect_switches(&mut switches);
    }
    switches
        .iter()
        .find_map(|(label, enabled, len)| enabled_items_disagreement(label, *enabled, *len))
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// response headers policy configuration whose `Quantity` disagrees with the length of
/// its `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`] applies to a distribution configuration:
/// every CloudFront list travels in a wrapper stating its own length beside the list
/// itself, and the service refuses a request where the two disagree, naming the wrapper by
/// its own member name. This walks all of the wrappers a `ResponseHeadersPolicyConfig`
/// reaches — the three CORS lists that are required when `CorsConfig` is named, the optional
/// exposed-headers list, and the custom-header and remove-header lists.
pub fn response_headers_policy_quantity_mismatch(
    config: &ResponseHeadersPolicyConfig,
) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(cors) = &config.cors_config {
        let origins = &cors.access_control_allow_origins;
        counts.push((
            "AccessControlAllowOrigins",
            origins.quantity,
            origins.items.len(),
        ));
        let headers = &cors.access_control_allow_headers;
        counts.push((
            "AccessControlAllowHeaders",
            headers.quantity,
            headers.items.len(),
        ));
        let methods = &cors.access_control_allow_methods;
        counts.push((
            "AccessControlAllowMethods",
            methods.quantity,
            methods.items.len(),
        ));
        if let Some(exposed) = &cors.access_control_expose_headers {
            counts.push((
                "AccessControlExposeHeaders",
                exposed.quantity,
                items_len(&exposed.items),
            ));
        }
    }
    if let Some(custom) = &config.custom_headers_config {
        counts.push((
            "CustomHeadersConfig",
            custom.quantity,
            items_len(&custom.items),
        ));
    }
    if let Some(remove) = &config.remove_headers_config {
        counts.push((
            "RemoveHeadersConfig",
            remove.quantity,
            items_len(&remove.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The most characters a response headers policy's `Content-Security-Policy` header value
/// may carry, over which the policy is refused with the modelled
/// `TooLongCSPInResponseHeadersPolicy`.
///
/// CloudFront's published quota for "Maximum number of characters in the
/// Content-Security-Policy header value" is 1,783, and the error itself points at that quota
/// page ("For more information, see Quotas … in the Amazon CloudFront Developer Guide"), so
/// the bound is that number rather than one chosen here. It is a property of the
/// CONFIGURATION, so every payload that states one is held to it: the create and the update
/// of a policy refuse the same value.
pub const MAX_CSP_IN_RESPONSE_HEADERS_POLICY: usize = 1783;

/// Whether `config` states a `Content-Security-Policy` header value past
/// [`MAX_CSP_IN_RESPONSE_HEADERS_POLICY`].
///
/// The value is the one `ResponseHeadersPolicyContentSecurityPolicy$ContentSecurityPolicy`
/// carries — "The policy directives and their values that CloudFront includes as values for
/// the Content-Security-Policy HTTP response header" — and a configuration that names no
/// security headers section, or one whose section sets no content security policy, states no
/// value at all and so is within the bound. Measured in CHARACTERS, as the quota states its
/// maximum.
pub fn response_headers_policy_csp_too_long(config: &ResponseHeadersPolicyConfig) -> bool {
    config
        .security_headers_config
        .as_ref()
        .and_then(|section| section.content_security_policy.as_ref())
        .is_some_and(|csp| {
            csp.content_security_policy.chars().count() > MAX_CSP_IN_RESPONSE_HEADERS_POLICY
        })
}

/// The modelled `TooLongCSPInResponseHeadersPolicy` (HTTP 400) for a configuration whose
/// `Content-Security-Policy` value is past the published quota
/// ([`MAX_CSP_IN_RESPONSE_HEADERS_POLICY`]).
///
/// The sentence states the CONDITION — the request carries a longer value than is allowed —
/// so nothing from the request belongs in it, neither the value nor its length, and every
/// refusal of this condition reads the same.
pub fn too_long_csp_in_response_headers_policy(
) -> crate::ops::types::TooLongCSPInResponseHeadersPolicy {
    crate::ops::types::TooLongCSPInResponseHeadersPolicy {
        message: Some(TOO_LONG_CSP_IN_RESPONSE_HEADERS_POLICY_MESSAGE.to_string()),
    }
}

/// `TooLongCSPInResponseHeadersPolicy`'s message, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape uses
/// for the same condition ("The length of the Content-Security-Policy header value in the
/// response headers policy exceeds the maximum. For more information, see Quotas …"): that
/// one documents the condition and points at the quota page, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const TOO_LONG_CSP_IN_RESPONSE_HEADERS_POLICY_MESSAGE: &str =
    "Your request contains longer Content Security Policy value in the Response Headers \
     Policy than is allowed.";

/// The modelled `InconsistentQuantities` message for the first list wrapper in an origin
/// request policy configuration whose `Quantity` disagrees with the length of its `Items`,
/// or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`] and
/// [`response_headers_policy_quantity_mismatch`] apply: every CloudFront list travels in a
/// wrapper stating its own length beside the list itself, and the service refuses a
/// request where the two disagree. An `OriginRequestPolicyConfig` reaches exactly three of
/// them — the optional `Headers`, `Cookies` and `QueryStrings` lists its three behaviour
/// configurations carry.
///
/// The sentence names the WRAPPER MEMBER whose `Quantity` disagreed — `Headers`, `Cookies`,
/// `QueryStrings` — rather than the path to it, as every other wrapper this service reports
/// is named (`Origins`, `Aliases`, `Paths`, `ContentTypeProfiles`): recorded behaviour for
/// this operation's refusal.
pub fn origin_request_policy_quantity_mismatch(
    config: &OriginRequestPolicyConfig,
) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(headers) = &config.headers_config.headers {
        counts.push(("Headers", headers.quantity, items_len(&headers.items)));
    }
    if let Some(cookies) = &config.cookies_config.cookies {
        counts.push(("Cookies", cookies.quantity, items_len(&cookies.items)));
    }
    if let Some(query_strings) = &config.query_strings_config.query_strings {
        counts.push((
            "QueryStrings",
            query_strings.quantity,
            items_len(&query_strings.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in a cache
/// policy configuration whose `Quantity` disagrees with the length of its `Items`, or
/// `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`],
/// [`response_headers_policy_quantity_mismatch`] and
/// [`origin_request_policy_quantity_mismatch`] apply: every CloudFront list travels in a
/// wrapper stating its own length beside the list itself, and the service refuses a
/// request where the two disagree. A `CachePolicyConfig` reaches exactly three of them —
/// the optional `Headers`, `Cookies` and `QueryStrings` lists the three behaviour
/// configurations under `ParametersInCacheKeyAndForwardedToOrigin` carry — and reaches
/// none at all when the configuration names no cache key parameters.
pub fn cache_policy_quantity_mismatch(config: &CachePolicyConfig) -> Option<String> {
    let parameters = config
        .parameters_in_cache_key_and_forwarded_to_origin
        .as_ref()?;
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(headers) = &parameters.headers_config.headers {
        counts.push((
            "HeadersConfig.Headers",
            headers.quantity,
            items_len(&headers.items),
        ));
    }
    if let Some(cookies) = &parameters.cookies_config.cookies {
        counts.push((
            "CookiesConfig.Cookies",
            cookies.quantity,
            items_len(&cookies.items),
        ));
    }
    if let Some(query_strings) = &parameters.query_strings_config.query_strings {
        counts.push((
            "QueryStringsConfig.QueryStrings",
            query_strings.quantity,
            items_len(&query_strings.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// Which of the three caller-chosen lists a cache policy or an origin request policy
/// carries names more entries than its published quota allows.
///
/// One value per list rather than per error shape: each policy kind models its OWN
/// `TooMany…` error for the same three lists, so a handler maps the answer onto the error
/// its own operation declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyList {
    /// `HeadersConfig.Headers`, bounded by [`MAX_HEADERS_IN_POLICY`].
    Headers,
    /// `CookiesConfig.Cookies`, bounded by [`MAX_COOKIES_IN_POLICY`].
    Cookies,
    /// `QueryStringsConfig.QueryStrings`, bounded by [`MAX_QUERY_STRINGS_IN_POLICY`].
    QueryStrings,
}

impl PolicyList {
    /// How many entries this list may name.
    pub const fn quota(self) -> usize {
        match self {
            PolicyList::Headers => MAX_HEADERS_IN_POLICY,
            PolicyList::Cookies => MAX_COOKIES_IN_POLICY,
            PolicyList::QueryStrings => MAX_QUERY_STRINGS_IN_POLICY,
        }
    }
}

/// The first list in `counts` that names more entries than its quota allows, as
/// `(the list, the count it stated)`, or `None` when every list is within its quota.
///
/// An absent list names nothing, so it is within every quota. The count a wrapper STATES
/// and the number of entries it carries both take part, for the reason
/// [`policy_config_invalid_argument`] takes both: by the time a handler reaches here the
/// wrapper walk has already refused a `Quantity` that disagrees with its `Items`, so the
/// two spellings cannot differ — this simply does not depend on that order.
pub(super) fn list_over_quota(counts: [(PolicyList, Option<(i32, usize)>); 3]) -> Option<PolicyList> {
    counts.into_iter().find_map(|(list, stated)| {
        let (quantity, carried) = stated?;
        let count = i64::from(quantity).max(i64::try_from(carried).unwrap_or(0));
        (count > i64::try_from(list.quota()).unwrap_or(i64::MAX)).then_some(list)
    })
}

/// The cache key list a cache policy configuration names more entries in than CloudFront's
/// published quota allows, or `None` when every list is within it.
///
/// A configuration that names no cache key parameters reaches no list at all.
pub fn cache_policy_list_over_quota(config: &CachePolicyConfig) -> Option<PolicyList> {
    let parameters = config
        .parameters_in_cache_key_and_forwarded_to_origin
        .as_ref()?;
    list_over_quota([
        (
            PolicyList::Headers,
            parameters
                .headers_config
                .headers
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
        (
            PolicyList::Cookies,
            parameters
                .cookies_config
                .cookies
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
        (
            PolicyList::QueryStrings,
            parameters
                .query_strings_config
                .query_strings
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
    ])
}

/// The same walk over an origin request policy configuration's three lists, which carry the
/// SAME published quotas ([`MAX_HEADERS_IN_POLICY`]) and are reached directly on the
/// configuration rather than under a cache key parameters member.
pub fn origin_request_policy_list_over_quota(
    config: &OriginRequestPolicyConfig,
) -> Option<PolicyList> {
    list_over_quota([
        (
            PolicyList::Headers,
            config
                .headers_config
                .headers
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
        (
            PolicyList::Cookies,
            config
                .cookies_config
                .cookies
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
        (
            PolicyList::QueryStrings,
            config
                .query_strings_config
                .query_strings
                .as_ref()
                .map(|list| (list.quantity, items_len(&list.items))),
        ),
    ])
}

/// The sentence `TooManyHeadersInCachePolicy` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with — and fixed whole: the
/// condition is that the request named more headers than the policy may hold, and neither
/// the count submitted nor the quota is part of it.
pub const TOO_MANY_HEADERS_IN_CACHE_POLICY_MESSAGE: &str =
    "Your request contains more headers in the Cache Policy than are allowed.";

/// The sentence `TooManyCookiesInCachePolicy` carries, as
/// [`TOO_MANY_HEADERS_IN_CACHE_POLICY_MESSAGE`] states the same condition about the
/// policy's other cache key list.
pub const TOO_MANY_COOKIES_IN_CACHE_POLICY_MESSAGE: &str =
    "Your request contains more cookies in the Cache Policy than are allowed.";

/// The sentence `TooManyQueryStringsInCachePolicy` carries, worded as
/// [`TOO_MANY_HEADERS_IN_CACHE_POLICY_MESSAGE`] is.
pub const TOO_MANY_QUERY_STRINGS_IN_CACHE_POLICY_MESSAGE: &str =
    "Your request contains more query strings in the Cache Policy than are allowed.";

/// The sentence `TooManyHeadersInOriginRequestPolicy` carries, on the wire: the same
/// condition [`TOO_MANY_HEADERS_IN_CACHE_POLICY_MESSAGE`] states, named for the other policy
/// kind — which CloudFront words without the article.
pub const TOO_MANY_HEADERS_IN_ORIGIN_REQUEST_POLICY_MESSAGE: &str =
    "Your request contains more headers in Origin Request Policy than are allowed.";

/// The sentence `TooManyCookiesInOriginRequestPolicy` carries, worded as
/// [`TOO_MANY_HEADERS_IN_ORIGIN_REQUEST_POLICY_MESSAGE`] is.
pub const TOO_MANY_COOKIES_IN_ORIGIN_REQUEST_POLICY_MESSAGE: &str =
    "Your request contains more cookies in Origin Request Policy than are allowed.";

/// The sentence `TooManyQueryStringsInOriginRequestPolicy` carries, worded as
/// [`TOO_MANY_HEADERS_IN_ORIGIN_REQUEST_POLICY_MESSAGE`] is.
pub const TOO_MANY_QUERY_STRINGS_IN_ORIGIN_REQUEST_POLICY_MESSAGE: &str =
    "Your request contains more query strings in Origin Request Policy than are allowed.";

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// field-level encryption profile configuration whose `Quantity` disagrees with the length
/// of its `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`], [`cache_policy_quantity_mismatch`] and the
/// other wrapper walks apply: every CloudFront list travels in a wrapper stating its own
/// length beside the list itself, and the service refuses a request where the two disagree.
/// A `FieldLevelEncryptionProfileConfig` reaches its required `EncryptionEntities` wrapper
/// and the `FieldPatterns` wrapper each entity in it carries, so the walk covers the nested
/// ones too rather than only the outermost.
pub fn field_level_encryption_profile_quantity_mismatch(
    config: &FieldLevelEncryptionProfileConfig,
) -> Option<String> {
    let entities = &config.encryption_entities;
    let mut counts: Vec<(String, i32, usize)> = vec![(
        "EncryptionEntities".to_string(),
        entities.quantity,
        items_len(&entities.items),
    )];
    for entity in entities.items.as_deref().unwrap_or_default() {
        counts.push((
            format!("EncryptionEntities.Items[{}].FieldPatterns", entity.public_key_id),
            entity.field_patterns.quantity,
            items_len(&entity.field_patterns.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InvalidArgument` (HTTP 400) for an encryption entity whose `FieldPatterns`
/// OVERLAP, or `None` when no entity's patterns do.
///
/// `EncryptionEntity$FieldPatterns`: "You can provide the full field name, or any beginning
/// characters followed by a wildcard (*). You can't overlap field patterns. For example, you
/// can't have both ABC* and AB*. Note that field patterns are case-sensitive." So a pattern
/// ending in the wildcard COVERS every field name beginning with the characters in front of
/// it, and an entity cannot carry both that pattern and another pattern it covers — the field
/// a request encrypts would then match two patterns and no key could be said to encrypt it.
/// Checked WITHIN each entity, which is the scope the sentence states: two entities of one
/// profile encrypt with different keys and are free to name the same field.
///
/// Compared byte for byte, the patterns being case-sensitive. Each entity's patterns are
/// compared with one another, which is work the caller's own list length drives: the counts
/// this operation admits at all are bounded ahead of it
/// ([`field_level_encryption_profile_over_quota`]), so what this costs is bounded too.
pub fn field_level_encryption_profile_overlapping_field_patterns(
    config: &FieldLevelEncryptionProfileConfig,
) -> Option<crate::ops::types::InvalidArgument> {
    config
        .encryption_entities
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|entity| {
            overlapping_field_patterns(entity.field_patterns.items.as_deref().unwrap_or_default())
        })
        .then(|| crate::ops::types::InvalidArgument {
            message: Some(OVERLAPPING_FIELD_PATTERNS_MESSAGE.to_string()),
        })
}

/// Which list of a field-level encryption profile configuration is past the count this
/// service admits, or `None` when every one of them is within it.
///
/// The entities are stored whole inside the profile record and are what the admission compares
/// with one another, so both counts are bounded BEFORE anything walks them
/// ([`MAX_FIELD_LEVEL_ENCRYPTION_ENCRYPTION_ENTITIES`],
/// [`MAX_FIELD_LEVEL_ENCRYPTION_FIELD_PATTERNS`]) and a configuration past either is refused
/// with the modelled error for that list.
pub fn field_level_encryption_profile_over_quota(
    config: &FieldLevelEncryptionProfileConfig,
) -> Option<FieldLevelEncryptionProfileQuota> {
    let entities = config
        .encryption_entities
        .items
        .as_deref()
        .unwrap_or_default();
    if entities.len() > MAX_FIELD_LEVEL_ENCRYPTION_ENCRYPTION_ENTITIES {
        return Some(FieldLevelEncryptionProfileQuota::EncryptionEntities);
    }
    entities
        .iter()
        .any(|entity| {
            items_len(&entity.field_patterns.items) > MAX_FIELD_LEVEL_ENCRYPTION_FIELD_PATTERNS
        })
        .then_some(FieldLevelEncryptionProfileQuota::FieldPatterns)
}

/// WHICH list of a field-level encryption profile configuration is past its count, which is
/// what decides the modelled error the refusal carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldLevelEncryptionProfileQuota {
    /// `EncryptionEntities` carries more entities than this service admits.
    EncryptionEntities,
    /// One entity's `FieldPatterns` carries more patterns than this service admits.
    FieldPatterns,
}

/// The sentence `TooManyFieldLevelEncryptionEncryptionEntities` carries, which is the one the
/// model states for the condition.
pub const TOO_MANY_ENCRYPTION_ENTITIES_MESSAGE: &str =
    "The maximum number of encryption entities for field-level encryption have been created.";

/// The sentence `TooManyFieldLevelEncryptionFieldPatterns` carries, which is the one the model
/// states for the condition.
pub const TOO_MANY_FIELD_PATTERNS_MESSAGE: &str =
    "The maximum number of field patterns for field-level encryption have been created.";


/// Whether any of `patterns` is covered by a PREFIX pattern among them.
///
/// A pattern ending in the wildcard covers the field names beginning with the characters in
/// front of it, so it covers another pattern exactly when that pattern begins with those
/// characters — the pattern `AB*` covers `ABC*` and `ABC` alike. A pattern never counts as
/// covering itself, so the walk compares each pattern against the OTHER entries by position.
fn overlapping_field_patterns(patterns: &[String]) -> bool {
    patterns.iter().enumerate().any(|(covering, pattern)| {
        let Some(prefix) = pattern.strip_suffix(FIELD_PATTERN_WILDCARD) else {
            return false;
        };
        patterns
            .iter()
            .enumerate()
            .any(|(covered, other)| covered != covering && other.starts_with(prefix))
    })
}

/// The character a field pattern names "any beginning characters followed by a wildcard"
/// with, which is what makes a pattern cover more than the one field it spells.
const FIELD_PATTERN_WILDCARD: &str = "*";

/// The sentence CloudFront refuses overlapping field patterns with, on the wire: it states
/// the rule the member broke, and carries no pattern out of the request.
pub const OVERLAPPING_FIELD_PATTERNS_MESSAGE: &str =
    "The parameter FieldPattern cannot start with a prefix field pattern already present in \
     the encryption entity.";

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// field-level encryption CONFIGURATION whose `Quantity` disagrees with the length of its
/// `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`field_level_encryption_profile_quantity_mismatch`] and the other wrapper
/// walks apply: every CloudFront list travels in a wrapper stating its own length beside the
/// list itself, and the service refuses a request where the two disagree. A
/// `FieldLevelEncryptionConfig` reaches two such wrappers — the `QueryArgProfiles` of its
/// `QueryArgProfileConfig` and the `ContentTypeProfiles` of its `ContentTypeProfileConfig` —
/// each of them optional, so a wrapper the request leaves out states no quantity to
/// disagree with.
pub fn field_level_encryption_config_quantity_mismatch(
    config: &FieldLevelEncryptionConfig,
) -> Option<String> {
    let mut counts: Vec<(&str, i32, usize)> = Vec::new();
    if let Some(profiles) = config
        .query_arg_profile_config
        .as_ref()
        .and_then(|query_arg| query_arg.query_arg_profiles.as_ref())
    {
        counts.push((
            "QueryArgProfiles",
            profiles.quantity,
            items_len(&profiles.items),
        ));
    }
    if let Some(profiles) = config
        .content_type_profile_config
        .as_ref()
        .and_then(|content_type| content_type.content_type_profiles.as_ref())
    {
        counts.push((
            "ContentTypeProfiles",
            profiles.quantity,
            items_len(&profiles.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The sentence CloudFront refuses a cache configuration whose three TTLs are not ordered
/// with: it states the ORDERING RULE over all three members — which is what attributes the
/// refusal, since `InvalidArgument` is this service's catch-all — and nothing from the
/// request, so a `DefaultTTL` that breaks the chain is attributable from the same sentence a
/// `MaxTTL` below `MinTTL` is.
///
/// The rule in the words the service states it in: "less than or equal to" rather than a
/// symbol, and the three members named in the order they are written.
pub const TTL_ORDER_MESSAGE: &str =
    "MinTTL, MaxTTL and DefaultTTL should follow order MinTTL less than or equal to \
     DefaultTTL less than or equal to MaxTTL";

/// Why a cache policy configuration is unusable, as the sentence the modelled
/// `InvalidArgument` carries — or `None` when every member is within what the model and
/// the documentation allow.
///
/// Two rules, each one the documentation states. The TTL ordering is reported as the RULE it
/// broke ([`TTL_ORDER_MESSAGE`], fixed text); the remaining sentences name the member and the
/// constraint it broke, which is the one thing the caller cannot work out for itself.
///
/// * The three TTLs are ORDERED: each is "The {minimum,default,maximum} amount of time, in
///   seconds, that objects stay in the CloudFront cache", so the minimum cannot exceed the
///   default or the maximum and the default cannot exceed the maximum. Only the members
///   the request SUPPLIES are compared: a member the request leaves out carries
///   CloudFront's own documented default for it (86400 or `MinTTL`, whichever is larger,
///   for `DefaultTTL`; 31536000 or `DefaultTTL`, whichever is larger, for `MaxTTL`), and
///   refusing a configuration whose only disorder comes from that default would refuse
///   what the service accepts. Every break of the chain is the one ordering sentence
///   ([`TTL_ORDER_MESSAGE`]), which states the rule rather than the pair that exposed it.
/// * A TTL is an amount of time in seconds, so a negative one names no duration. The model
///   puts no `@range` on `long`, so nothing refuses it before a handler runs.
///
/// `CachePolicyConfig$Comment`'s documented bound is checked beside these, through the one
/// [`comment_too_long`] sentence every comment bound in this service is refused with.
pub fn cache_policy_config_invalid(config: &CachePolicyConfig) -> Option<String> {
    if let Some(comment) = &config.comment {
        let length = comment.chars().count();
        if length > MAX_CACHE_POLICY_COMMENT {
            return comment_too_long().message;
        }
    }

    // A TTL is an amount of time in seconds, so a negative one is outside the range the member
    // admits, and the refusal names the PARAMETER that carried it through the one sentence this
    // service states for that condition ([`parameter_out_of_range_message`]) — the same one a
    // cache BEHAVIOUR's three TTLs are held to (`super::config_members`) and the one a port no
    // origin can be reached on is refused with ([`ORIGIN_PORT_OUT_OF_RANGE_MESSAGE`], recorded
    // behaviour), so no surface of this service words one rule differently. The modelled
    // `InvalidArgument` is what carries it here, as it does there; nothing of the submitted
    // value reaches the wire, because the parameter is the whole of what the caller has to fix.
    for (label, value) in [
        ("MinTTL", Some(config.min_ttl)),
        ("DefaultTTL", config.default_ttl),
        ("MaxTTL", config.max_ttl),
    ] {
        if let Some(value) = value {
            if value < 0 {
                return Some(parameter_out_of_range_message(label));
            }
        }
    }

    // The pairs the ordering rule constrains, each compared only where the request
    // supplies both members of it. Every break of the chain is the ONE ordering sentence
    // ([`TTL_ORDER_MESSAGE`]): the rule is over all three members, so which pair exposed
    // the break is no part of what the refusal states.
    let pairs: [(Option<i64>, Option<i64>); 3] = [
        (Some(config.min_ttl), config.default_ttl),
        (Some(config.min_ttl), config.max_ttl),
        (config.default_ttl, config.max_ttl),
    ];
    for (lower, upper) in pairs {
        let (Some(lower), Some(upper)) = (lower, upper) else {
            continue;
        };
        if lower > upper {
            return Some(TTL_ORDER_MESSAGE.to_string());
        }
    }

    None
}

/// The modelled `InconsistentQuantities` message for an invalidation batch whose
/// `Paths.Quantity` disagrees with the number of paths it carries, or `None`.
pub fn batch_quantity_mismatch(batch: &InvalidationBatch) -> Option<String> {
    quantity_disagreement(
        "Paths",
        batch.paths.quantity,
        items_len(&batch.paths.items),
    )
}

/// How many paths an invalidation batch names, which [`MAX_INVALIDATION_PATHS`] bounds.
pub fn batch_path_count(batch: &InvalidationBatch) -> usize {
    items_len(&batch.paths.items)
}

/// Resolve an invalidation batch into the one the service STORES: its paths as a SET.
///
/// An invalidation batch IS the set of objects the invalidation removes from the caches —
/// "the objects that you want to invalidate" — so naming one object twice asks for nothing
/// more than naming it once, and the stored batch carries each distinct path once, in the
/// order the request first named it, with `Paths$Quantity` ("the number of invalidation
/// paths specified for the objects that you want to invalidate") stating how many that is.
///
/// Paths are compared as CloudFront matches them: a path is taken verbatim, so two paths
/// differing only in the case of a character name different objects and both stay in the
/// set.
///
/// The resolution happens once, where the batch is stored, so the create's own answer, every
/// later read of that invalidation and the repeat a caller reference makes the same request
/// again all report the one stored batch.
pub fn resolve_invalidation_batch(mut batch: InvalidationBatch) -> InvalidationBatch {
    let submitted = batch.paths.items.take().unwrap_or_default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut distinct: Vec<String> = Vec::with_capacity(submitted.len());
    for path in submitted {
        if seen.insert(path.clone()) {
            distinct.push(path);
        }
    }
    batch.paths.quantity = i32::try_from(distinct.len()).unwrap_or(i32::MAX);
    batch.paths.items = Some(distinct);
    batch
}

/// Whether `path` names objects of a distribution the way an invalidation path does.
///
/// "The path is relative to the distribution and must begin with a slash (/)", and "if the
/// path includes non-ASCII characters or unsafe characters … specify the characters as
/// URL-encoded", so a path carrying unescaped one of the characters RFC 1738 calls unsafe in
/// a URL ([`UNSAFE_PATH_CHARACTERS`]), or a `%` that starts no escape, is not a path either
/// (the Amazon CloudFront Developer Guide, "Invalidating files"). A value breaking either
/// names no object of the distribution, so there is nothing for it to invalidate.
///
/// The `*` wildcard is NOT placed by this rule: CloudFront takes an invalidation path
/// verbatim, so a `*` anywhere in it is part of the pattern it matches with.
pub fn is_invalidation_path(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if UNSAFE_PATH_CHARACTERS.contains(&character) {
            return false;
        }
        // An escape is a `%` and two hexadecimal digits, so a `%` carrying anything else
        // escapes nothing.
        if character == '%'
            && !characters
                .next()
                .zip(characters.next())
                .is_some_and(|(high, low)| high.is_ascii_hexdigit() && low.is_ascii_hexdigit())
        {
            return false;
        }
    }
    true
}

/// What an invalidation item carries instead of a path when it names a CACHE TAG rather than
/// an object: the one character that tells the two apart, which is why it is no part of a
/// path ([`UNSAFE_PATH_CHARACTERS`]).
pub const CACHE_TAG_PREFIX: char = '#';

/// Whether `item` of an invalidation batch names a cache TAG.
///
/// `CacheTagConfig` is "the HTTP header name from which CloudFront extracts cache tags from
/// origin responses … You can then invalidate cached objects by tag using the
/// `CreateInvalidation` API", and the header "value must contain comma-separated tag values
/// (for example, `product:electronics, category:tv, brand:example`)" — so a tag is one entry
/// of that list: a non-empty value carrying neither the comma that separates two tags nor
/// whitespace, which the header's own spelling of a tag cannot carry either. An item naming
/// one opens with [`CACHE_TAG_PREFIX`], the one character an object path cannot carry.
pub fn is_cache_tag_item(item: &str) -> bool {
    let Some(tag) = item.strip_prefix(CACHE_TAG_PREFIX) else {
        return false;
    };
    !tag.is_empty()
        && !tag.contains(',')
        && !tag.contains(CACHE_TAG_PREFIX)
        && !tag.chars().any(char::is_whitespace)
}

/// The cache tag an invalidation item names, or `None` when it names an object path.
///
/// The tag itself, without the [`CACHE_TAG_PREFIX`] the item carries it under, so it compares
/// against the tags an origin response listed ([`cache_tags_of_header_value`]) as the same
/// value.
pub fn cache_tag_of_item(item: &str) -> Option<&str> {
    is_cache_tag_item(item).then(|| item.trim_start_matches(CACHE_TAG_PREFIX))
}

/// The cache tags one origin response listed in the header `CacheTagConfig` names: "the
/// header value must contain comma-separated tag values", so each entry is one tag, with the
/// whitespace that may follow a comma trimmed off.
pub fn cache_tags_of_header_value(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
}

/// The characters an invalidation path cannot carry unescaped: the ones RFC 1738 calls
/// unsafe in a URL, which CloudFront's own documentation requires URL-ENCODED. The `%` it
/// also lists is admitted by [`is_invalidation_path`] only as the start of an escape.
pub(super) const UNSAFE_PATH_CHARACTERS: &[char] =
    &[' ', '"', '<', '>', '#', '{', '}', '|', '\\', '^', '[', ']', '`'];

/// Whether an invalidation batch names no object to invalidate: one carrying a value that is
/// neither an invalidation path ([`is_invalidation_path`]) nor, where the resource extracts
/// cache tags from its origin responses, a cache tag ([`is_cache_tag_item`]) — or none at
/// all, since a batch IS the set of objects the invalidation removes from the caches, so an
/// empty one describes no invalidation.
///
/// `extracts_cache_tags` is whether the distribution the batch is submitted against carries
/// a `CacheTagConfig`: tag-based invalidation is what adding that member buys ("You can then
/// invalidate cached objects by tag using the `CreateInvalidation` API"), so on a
/// distribution that extracts no tags from its origin responses there is no tag for such an
/// item to name.
pub fn batch_names_invalid_path(batch: &InvalidationBatch, extracts_cache_tags: bool) -> bool {
    let paths = batch.paths.items.as_deref().unwrap_or_default();
    paths.is_empty()
        || paths.iter().any(|path| {
            !is_invalidation_path(path) && !(extracts_cache_tags && is_cache_tag_item(path))
        })
}

/// Whether `config` has CloudFront extract cache tags from its origin responses — the
/// `CacheTagConfig` member, "the HTTP header name from which CloudFront extracts cache tags
/// from origin responses".
pub fn extracts_cache_tags(config: &DistributionConfig) -> bool {
    config.cache_tag_config.is_some()
}

/// The sentence CloudFront refuses a batch naming a path it cannot invalidate with, on the
/// wire.
///
/// FIXED text: what the refusal states is the RULE the paths broke, and the condition is a
/// property of the batch rather than of any one path in it, so no path from the request
/// belongs in it.
pub const INVALID_INVALIDATION_PATHS_MESSAGE: &str =
    "Your request contains one or more invalid invalidation paths.";

/// The modelled `InvalidArgument` (HTTP 400) for a batch naming a path that is no
/// invalidation path.
pub fn invalid_invalidation_paths() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_INVALIDATION_PATHS_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a batch whose `CallerReference` already names another
/// invalidation of the same distribution with, on the wire.
///
/// FIXED service text: what the refusal states is that the reference is taken for this
/// distribution, which is a property of the error class — so neither the reference the
/// request submitted nor the id of the invalidation that holds it belongs in it, exactly as
/// the caller-reference conflict of every other resource kind states its own fixed sentence
/// ([`ORIGIN_ACCESS_IDENTITY_ALREADY_EXISTS_MESSAGE`] and its neighbours).
pub const INVALIDATION_CALLER_REFERENCE_REUSED_MESSAGE: &str =
    "Your request contains a caller reference that was used for a previous invalidation \
     batch for the same distribution.";

/// The modelled `InvalidArgument` (HTTP 400) for a batch submitted under a
/// `CallerReference` that already names another invalidation of the same distribution
/// ([`INVALIDATION_CALLER_REFERENCE_REUSED_MESSAGE`]).
///
/// Takes no identifier, so the refusal reads the same whichever reference collided and on
/// whichever of the two invalidation surfaces it was submitted.
pub fn invalidation_caller_reference_reused() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALIDATION_CALLER_REFERENCE_REUSED_MESSAGE.to_string()),
    }
}

/// `TrafficConfig$Type` selecting a share of traffic, from
/// [`crate::ops::types::CONTINUOUS_DEPLOYMENT_POLICY_TYPE_VALUES`] — the values the model
/// says the member accepts.
pub const TRAFFIC_TYPE_SINGLE_WEIGHT: &str = "SingleWeight";

/// `TrafficConfig$Type` selecting requests by header, from the same enum.
pub const TRAFFIC_TYPE_SINGLE_HEADER: &str = "SingleHeader";

/// The prefix `ContinuousDeploymentSingleHeaderConfig$Header` must carry: "The request
/// header name that you want CloudFront to send to your staging distribution. The header
/// must contain the prefix `aws-cf-cd-`."
pub const SINGLE_HEADER_PREFIX: &str = "aws-cf-cd-";

/// The modelled `InconsistentQuantities` message for a continuous deployment policy
/// configuration whose `StagingDistributionDnsNames.Quantity` disagrees with the number of
/// domain names it carries, or `None`.
///
/// The one wrapper a `ContinuousDeploymentPolicyConfig` reaches, checked under the same
/// rule every other CloudFront list wrapper is ([`quantity_disagreement`]).
pub fn policy_config_quantity_mismatch(
    config: &ContinuousDeploymentPolicyConfig,
) -> Option<String> {
    let names = &config.staging_distribution_dns_names;
    quantity_disagreement(
        "StagingDistributionDnsNames",
        names.quantity,
        items_len(&names.items),
    )
}

/// Why a continuous deployment policy configuration is unusable, as the sentence the
/// modelled `InvalidArgument` carries — or `None` when every member is within what the
/// model and the documentation allow.
///
/// Each sentence is BUILT FROM THE REQUEST: it names the member and the constraint it
/// broke, which is the one thing the caller cannot work out for itself. The constraints
/// are the documented ones:
///
/// * `TrafficConfig$Type` "The type of traffic configuration" selects WHICH of the two
///   traffic configurations applies, so the one it names has to be there — and the value
///   has to be one the enum admits;
/// * `ContinuousDeploymentSingleHeaderConfig$Header` "must contain the prefix
///   `aws-cf-cd-`";
/// * `ContinuousDeploymentSingleWeightConfig$Weight` is "expressed as a decimal number
///   between 0 and 0.15";
/// * `SessionStickinessConfig$IdleTTL` and `$MaximumTTL` each have "Allowed values …
///   300–3600 seconds", and `IdleTTL` "must be less than or equal to `MaximumTTL`";
/// * `StagingDistributionDnsNames` carries at most
///   [`MAX_STAGING_DISTRIBUTION_DNS_NAMES`] domain names — the one caller-chosen LENGTH
///   in this configuration, bounded on the way IN like every other count this service
///   stores ([`MAX_CACHE_BEHAVIORS`], [`MAX_INVALIDATION_PATHS`]).
pub fn policy_config_invalid_argument(
    config: &ContinuousDeploymentPolicyConfig,
) -> Option<String> {
    // The LENGTH of the submitted domain name list is caller-chosen, and the record is
    // stored and later scanned name by name, so the count is refused FIRST — before any
    // other member is walked, before the store is locked and before anything is minted.
    // `Quantity` is taken into the count as well, defensively: it is the SAME bound, not a
    // second rule, and `policy_config_quantity_mismatch` has already refused a `Quantity`
    // that disagrees with `Items` by the time a handler reaches here, so the two spellings
    // cannot disagree — this one simply does not depend on that order.
    let names = &config.staging_distribution_dns_names;
    let stated = i64::from(names.quantity).max(i64::try_from(items_len(&names.items)).unwrap_or(0));
    if stated > i64::try_from(MAX_STAGING_DISTRIBUTION_DNS_NAMES).unwrap_or(i64::MAX) {
        return Some(format!(
            "StagingDistributionDnsNames names {stated} staging distribution domain names; at \
             most {MAX_STAGING_DISTRIBUTION_DNS_NAMES} are allowed."
        ));
    }
    // `None` from this function means "nothing to refuse", so each absent OPTIONAL member
    // below ends the walk rather than failing it: a configuration that names no traffic
    // configuration, or no session stickiness, has no constraint left to break. The `?` is
    // what clippy's `question_mark` lint requires here, so it reads as that early end.
    let traffic = config.traffic_config.as_ref()?;
    let kind = traffic.r#type.as_str();
    if kind == TRAFFIC_TYPE_SINGLE_WEIGHT {
        let Some(weight_config) = traffic.single_weight_config.as_ref() else {
            return Some(format!(
                "TrafficConfig.Type {TRAFFIC_TYPE_SINGLE_WEIGHT} requires \
                 TrafficConfig.SingleWeightConfig."
            ));
        };
        let weight = weight_config.weight;
        if !(MIN_TRAFFIC_WEIGHT..=MAX_TRAFFIC_WEIGHT).contains(&weight) {
            return Some(format!(
                "TrafficConfig.SingleWeightConfig.Weight ({weight}) must be a decimal number \
                 between {MIN_TRAFFIC_WEIGHT} and {MAX_TRAFFIC_WEIGHT}."
            ));
        }
        // No session stickiness named: nothing left to refuse (see above).
        let stickiness = weight_config.session_stickiness_config.as_ref()?;
        for (member, ttl) in [
            ("IdleTTL", stickiness.idle_ttl),
            ("MaximumTTL", stickiness.maximum_ttl),
        ] {
            if !(MIN_SESSION_STICKINESS_TTL..=MAX_SESSION_STICKINESS_TTL).contains(&ttl) {
                return Some(format!(
                    "TrafficConfig.SingleWeightConfig.SessionStickinessConfig.{member} ({ttl}) \
                     must be between {MIN_SESSION_STICKINESS_TTL} and \
                     {MAX_SESSION_STICKINESS_TTL} seconds."
                ));
            }
        }
        if stickiness.idle_ttl > stickiness.maximum_ttl {
            return Some(format!(
                "TrafficConfig.SingleWeightConfig.SessionStickinessConfig.IdleTTL ({}) must be \
                 less than or equal to MaximumTTL ({}).",
                stickiness.idle_ttl, stickiness.maximum_ttl
            ));
        }
        return None;
    }
    if kind == TRAFFIC_TYPE_SINGLE_HEADER {
        let Some(header_config) = traffic.single_header_config.as_ref() else {
            return Some(format!(
                "TrafficConfig.Type {TRAFFIC_TYPE_SINGLE_HEADER} requires \
                 TrafficConfig.SingleHeaderConfig."
            ));
        };
        if !header_config.header.starts_with(SINGLE_HEADER_PREFIX) {
            return Some(format!(
                "TrafficConfig.SingleHeaderConfig.Header ({}) must contain the prefix \
                 {SINGLE_HEADER_PREFIX}.",
                header_config.header
            ));
        }
        return None;
    }
    // A value outside `ContinuousDeploymentPolicyType` is already refused at the request
    // boundary by the model's own enum constraint, so this is the fallback for a `Type`
    // that reached here some other way — never a second spelling of that refusal.
    Some(format!(
        "TrafficConfig.Type ({kind}) must be one of {}.",
        crate::ops::types::CONTINUOUS_DEPLOYMENT_POLICY_TYPE_VALUES.join(", ")
    ))
}

/// Whether a continuous deployment policy configuration names a staging distribution
/// domain name that is no distribution of this account's.
///
/// `StagingDistributionDnsNames` carries "the CloudFront domain name of the staging
/// distribution. For example: `d111111abcdef8.cloudfront.net`" — the DEFAULT domain name
/// CloudFront ASSIGNED one of this account's distributions
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_domain_name`] mints it), and
/// the policy moves traffic to THAT distribution. So a name no distribution of this account
/// is served on names no staging distribution the policy could move traffic to, whether it
/// is a domain of another zone, an alternate domain name a distribution also answers to, or
/// a name in the assigned shape that was never assigned here. Resolved against the STORED
/// distributions rather than a second index beside them, so it cannot drift out of step with
/// what a read of those distributions reports, and the collection is bounded by
/// [`MAX_DISTRIBUTIONS`]. DNS names are case-insensitive, so the comparison is
/// ([`same_domain`]).
pub fn names_foreign_staging_distribution_domain(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    config: &ContinuousDeploymentPolicyConfig,
) -> bool {
    staging_distribution_dns_names(config)
        .iter()
        .any(|name| store.distribution_by_domain_name(name).is_none())
}

/// Whether a continuous deployment policy configuration states no staging distribution
/// domain name at all.
///
/// `StagingDistributionDnsNames` is the REQUIRED member naming the staging distribution the
/// policy moves traffic to, so a wrapper carrying no name names no distribution and there is
/// no policy to build from it.
pub fn names_no_staging_distribution_domain(
    config: &ContinuousDeploymentPolicyConfig,
) -> bool {
    staging_distribution_dns_names(config).is_empty()
}

/// The modelled `InvalidArgument` (HTTP 400) for a `StagingDistributionDnsNames` that
/// carries no domain name.
///
/// FIXED service text ([`EMPTY_STAGING_DISTRIBUTION_DNS_NAMES_MESSAGE`]): the refusal states
/// which member was left empty, and nothing from the request.
pub fn empty_staging_distribution_dns_names() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(EMPTY_STAGING_DISTRIBUTION_DNS_NAMES_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a `StagingDistributionDnsNames` carrying no domain name
/// with, on the wire.
pub const EMPTY_STAGING_DISTRIBUTION_DNS_NAMES_MESSAGE: &str =
    "The parameter StagingDistributionDnsNames is null or empty.";

/// The zone the domain names CloudFront assigns a distribution live under, with the
/// separating dot: `d111111abcdef8.cloudfront.net` is one label under it.
pub const CLOUDFRONT_DOMAIN_SUFFIX: &str = ".cloudfront.net";

/// The modelled `InvalidArgument` (HTTP 400) for a `StagingDistributionDnsNames` naming a
/// domain CloudFront did not assign a distribution.
///
/// FIXED service text ([`FOREIGN_STAGING_DISTRIBUTION_DOMAIN_MESSAGE`]): what the refusal
/// states is the RULE the member broke, so no domain from the request belongs in it.
pub fn foreign_staging_distribution_domain() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(FOREIGN_STAGING_DISTRIBUTION_DOMAIN_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a staging distribution domain name of its own zone with,
/// on the wire: it names the member that carried the value, and nothing else.
pub const FOREIGN_STAGING_DISTRIBUTION_DOMAIN_MESSAGE: &str =
    "The parameter StagingDistributionDnsNames is not a default CloudFront domain name.";

/// Whether a continuous deployment policy configuration names a staging distribution that is
/// itself DISABLED.
///
/// `StagingDistributionDnsNames` names the distribution the policy moves traffic to, and
/// `DistributionConfig$Enabled` is "whether the distribution is enabled to accept user
/// requests for content" — a disabled one accepts none, so a policy over it could never route
/// a viewer request and the configuration describes a deployment that cannot happen. Resolved
/// against the STORED distributions, for the reason
/// [`names_foreign_staging_distribution_domain`] is, and read AFTER that check: a name no
/// distribution answers to has no `Enabled` state to read.
pub fn names_disabled_staging_distribution(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    config: &ContinuousDeploymentPolicyConfig,
) -> bool {
    staging_distribution_dns_names(config).iter().any(|name| {
        store
            .distribution_by_domain_name(name)
            .is_some_and(|distribution| !distribution.config.enabled)
    })
}

/// The modelled `InvalidArgument` (HTTP 400) for a `StagingDistributionDnsNames` naming a
/// distribution that is disabled.
///
/// FIXED service text ([`DISABLED_STAGING_DISTRIBUTION_MESSAGE`]): what the refusal states is
/// the RULE the member broke, so no domain name from the request belongs in it.
pub fn disabled_staging_distribution() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(DISABLED_STAGING_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a disabled staging distribution with, on the wire.
pub const DISABLED_STAGING_DISTRIBUTION_MESSAGE: &str =
    "The parameter Staging distributions contains disabled distributions.";

/// `NoSuchContinuousDeploymentPolicy`'s message for an id no stored policy answers to, on
/// the wire.
///
/// The WIRE text — what the recorded service answers this code with, in the form this
/// service states that condition in for every other named resource
/// ([`NO_SUCH_CACHE_POLICY_MESSAGE`], [`NO_SUCH_DISTRIBUTION_MESSAGE`],
/// [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`]): "The specified <resource> does not exist."
/// It differs from the sentence the MODEL's documentation of the error shape uses for the
/// same rule ("The continuous deployment policy doesn't exist."), which documents the
/// condition rather than stating what is served, so do not "correct" this constant to it.
pub const NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE: &str =
    "The specified continuous deployment policy does not exist.";

/// The modelled `NoSuchContinuousDeploymentPolicy` for a policy id the store does not
/// hold. Fixed service text, for the same reason [`no_such_distribution`] is.
pub fn no_such_continuous_deployment_policy() -> crate::ops::types::NoSuchContinuousDeploymentPolicy
{
    crate::ops::types::NoSuchContinuousDeploymentPolicy {
        message: Some(NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE.to_string()),
    }
}

/// `ContinuousDeploymentPolicyInUse`'s message, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. Like [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from the sentence
/// the MODEL's documentation of the error shape uses for the same condition ("You cannot
/// delete a continuous deployment policy that is associated with a primary distribution."):
/// that one documents the condition, this one is served. Do not "correct" this constant to the
/// documentation sentence.
///
/// A property of the CONDITION rather than of the request, so nothing from the request or
/// from the store belongs in it and every refusal of it reads the same, exactly as
/// [`CACHE_POLICY_IN_USE_MESSAGE`] words the same condition for a cache policy.
pub const CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE: &str =
    "The specified continuous deployment policy is currently associated with a distribution.";

/// The modelled `ContinuousDeploymentPolicyInUse` (HTTP 409) for a delete of a policy a
/// stored distribution still attaches.
pub fn continuous_deployment_policy_in_use()
-> crate::ops::types::ContinuousDeploymentPolicyInUse {
    crate::ops::types::ContinuousDeploymentPolicyInUse {
        message: Some(CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// The sentence `StagingDistributionInUse` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE`] it
/// differs from the sentence the MODEL's documentation of the error shape uses for the
/// same condition ("A continuous deployment policy for this staging distribution already
/// exists."): that one documents the condition, this one is served. Do not "correct" this
/// constant to the documentation sentence.
///
/// The WHOLE sentence is fixed: what the refusal states is that some staging distribution
/// of this account is already another policy's, so neither the policy that holds it nor
/// the distribution it was asked for is any part of it.
pub const STAGING_DISTRIBUTION_IN_USE_MESSAGE: &str =
    "One or more staging distributions are associated with another continuous deployment \
     policy in your account.";

/// The modelled `StagingDistributionInUse` (HTTP 409) for a call whose staging
/// distribution another policy already routes traffic to.
///
/// Takes no identifier: the sentence is fixed service text
/// ([`STAGING_DISTRIBUTION_IN_USE_MESSAGE`]), so every entry point into the rule refuses
/// this condition with the one text and none of them leaks the id of the policy that holds
/// the staging distribution.
pub fn staging_distribution_in_use() -> crate::ops::types::StagingDistributionInUse {
    crate::ops::types::StagingDistributionInUse {
        message: Some(STAGING_DISTRIBUTION_IN_USE_MESSAGE.to_string()),
    }
}

/// The modelled `ContinuousDeploymentPolicyAlreadyExists` (HTTP 409) for a create
/// repeating a configuration the store already holds — "A continuous deployment policy
/// with this configuration already exists" — naming the policy that holds it.
pub fn continuous_deployment_policy_already_exists(
    policy_id: &str,
) -> crate::ops::types::ContinuousDeploymentPolicyAlreadyExists {
    crate::ops::types::ContinuousDeploymentPolicyAlreadyExists {
        message: Some(format!(
            "A continuous deployment policy with this configuration already exists: {policy_id}"
        )),
    }
}

/// The modelled `TooManyContinuousDeploymentPolicies` (HTTP 400) for a create at this
/// emulator's bound ([`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`]).
///
/// The sentence states the BOUND and nothing else: the status and the code are the
/// contract, the text is free, and it advises no remedy — deleting a policy is
/// `DeleteContinuousDeploymentPolicy`, still a scaffold in this emulator.
pub fn too_many_continuous_deployment_policies()
-> crate::ops::types::TooManyContinuousDeploymentPolicies {
    crate::ops::types::TooManyContinuousDeploymentPolicies {
        message: Some(format!(
            "This emulator holds at most {MAX_CONTINUOUS_DEPLOYMENT_POLICIES} continuous \
             deployment policies for one account"
        )),
    }
}

/// `NoSuchDistribution`'s message: the sentence the model states on the shape itself,
/// "The specified distribution does not exist."
pub const NO_SUCH_DISTRIBUTION_MESSAGE: &str = "The specified distribution does not exist.";

/// `NoSuchInvalidation`'s message: the sentence the model states on the shape itself,
/// "The specified invalidation does not exist."
pub const NO_SUCH_INVALIDATION_MESSAGE: &str = "The specified invalidation does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for an invalidation id that is not in the scope
/// of the distribution tenant the request named.
///
/// The CODE is the tenant surface's own — the family every tenant-scoped read resolves its
/// tenant under — while the SENTENCE is the one this condition carries whichever code reports
/// it ([`NO_SUCH_INVALIDATION_MESSAGE`], which the distribution-scoped read answers with):
/// what it states is that the store holds no such invalidation, which is a property of the
/// store rather than of the operation asking.
pub fn invalidation_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_INVALIDATION_MESSAGE.to_string()),
    }
}

/// `BatchTooLarge`'s message: the sentence the model states on the shape itself,
/// "Invalidation batch specified is too large."
///
/// A property of the batch the request carried rather than of its contents, so the paths it
/// named and the bound it broke are no part of the text.
pub const BATCH_TOO_LARGE_MESSAGE: &str = "Invalidation batch specified is too large.";

/// The modelled `BatchTooLarge` (HTTP 413) for an invalidation batch this API cannot read as
/// a batch at all.
///
/// The condition is the SIZE of the request DOCUMENT, a property of the request alone, and it
/// is decided before any operation runs — on the bytes the request carries
/// (`crate::service::envelope::MAX_REQUEST_BODY_BYTES`), which is why a document past that
/// ceiling is answered with the transport's 413 carrying no error document at all. The objects
/// a resource may have invalidating at one time are the other, quota-driven condition and
/// carry the quota's own code ([`too_many_invalidations_in_progress`], HTTP 400): a client
/// tells the two apart to decide whether retrying the same batch can ever succeed.
///
/// NO handler constructs this today, and that is the point: the condition is decided before
/// any operation runs, so the 413 a caller reads for it is the envelope's, carrying no
/// document. The shape and its sentence stay here because the model declares both, so a
/// reader looking for what this service answers an oversized batch with finds the answer and
/// where it is made.
pub fn batch_too_large() -> crate::ops::types::BatchTooLarge {
    crate::ops::types::BatchTooLarge {
        message: Some(BATCH_TOO_LARGE_MESSAGE.to_string()),
    }
}

/// `TooManyInvalidationsInProgress`'s message, as CloudFront sends it: "Processing your
/// request will cause you to exceed the maximum number of in-progress invalidations."
///
/// The model's doc text for the shape states the CONDITION ("You have exceeded the maximum
/// number of allowable InProgress invalidation batch requests, or invalidation objects.");
/// this is the sentence the service puts on the wire. Fixed service text that names no
/// request value, for the same reason [`PRECONDITION_FAILED_MESSAGE`] is: the quota is the
/// service's own, and a caller learns what it asked for from the request it sent.
pub const TOO_MANY_INVALIDATIONS_MESSAGE: &str =
    "Processing your request will cause you to exceed the maximum number of in-progress \
     invalidations.";

/// The modelled `TooManyInvalidationsInProgress` for a batch the invalidation quota cannot
/// admit: one whose paths, beside the objects the resource already has in flight, would pass
/// what one resource may have invalidating at one time ([`MAX_INVALIDATION_PATHS`]) — or one
/// that would take the invalidations held for a resource past what this emulator keeps. The
/// SIZE of the request document is the other condition and carries [`batch_too_large`].
pub fn too_many_invalidations_in_progress() -> crate::ops::types::TooManyInvalidationsInProgress {
    crate::ops::types::TooManyInvalidationsInProgress {
        message: Some(TOO_MANY_INVALIDATIONS_MESSAGE.to_string()),
    }
}

/// `PreconditionFailed`'s message, as CloudFront sends it on the resource-DOCUMENT surface:
/// "The request failed because it didn't meet the preconditions in one or more
/// request-header fields."
///
/// The model's doc text for the shape states the condition ("The precondition in one or
/// more of the request fields evaluated to `false`."); this is the sentence the service
/// puts on the wire. Either way it is fixed service text that names no request value: a
/// caller learns WHICH precondition failed from the `If-Match` it sent, not from a
/// version token echoed back to it.
///
/// The family this one serves is the resource DOCUMENT a caller conditionally rewrites: a
/// distribution, an origin access identity, a function, a field-level encryption
/// configuration or profile, a public key, a key value store, an Anycast IP list, a
/// continuous deployment policy. The two siblings below word the same condition for the
/// other two families.
pub const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

/// `PreconditionFailed`'s message on the key group surface: see
/// [`PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE`], which the key group surface shares
/// with the policy surface.
/// `PreconditionFailed`'s message on the POLICY surface, as CloudFront sends it: "The
/// request failed because it didn't meet the preconditions for this operation."
///
/// CloudFront words this one refusal per API FAMILY, and the sentence is fixed text that
/// names no request value either way. The reusable configuration objects a distribution
/// references by id — the cache policies, the origin request policies and the key groups —
/// report the precondition of the OPERATION, which is the sentence below; the resource
/// DOCUMENTS a caller conditionally rewrites (a distribution, an identity, a function, a
/// field-level encryption configuration, a key value store) report it of the request's header
/// fields ([`PRECONDITION_FAILED_MESSAGE`]), and the distribution-tenant surface reports the
/// request as missing a requirement
/// ([`PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE`]). Recorded behaviour decides which
/// family an operation belongs to: do not move one between them.
pub const PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions for this operation.";

/// `PreconditionFailed`'s message on the DISTRIBUTION-TENANT surface — the tenants, the
/// connection groups and connection functions that route them, the web-ACL associations of
/// both, and the mutual-TLS TRUST STORES — as CloudFront sends it: "Your request is missing
/// requirements for this operation. Update your request and try again."
///
/// The THIRD wording of the one condition, because CloudFront words this refusal per API
/// FAMILY: this family reports the request as missing a requirement rather than reporting the
/// precondition at all. Fixed text that names no request value either way, for the reason
/// [`PRECONDITION_FAILED_MESSAGE`] names none.
pub const PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE: &str =
    "Your request is missing requirements for this operation. Update your request and try \
     again.";

/// `InvalidIfMatchVersion`'s message, as CloudFront sends it: "The If-Match version is
/// missing or not valid for the resource."
///
/// ONE constant for every operation that evaluates an `If-Match`, because the condition
/// is a property of the HEADER the request carried rather than of the resource it names:
/// a distribution, a policy or a key group refused for it learns the same sentence, and
/// nothing about the request belongs in the text.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. It deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("The If-Match version
/// is missing or not valid.").
pub const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";

/// The sentence `DistributionAlreadyExists` carries, up to the distribution the caller
/// reference is already associated with, which CloudFront names at the end of it.
pub(super) const DISTRIBUTION_ALREADY_EXISTS_PREFIX: &str =
    "The caller reference that you are using to create a distribution is associated with \
     another distribution. Already exists: ";

/// `InvalidArgument`'s message for a `MaxItems` a page size cannot be taken from.
///
/// One sentence for every unusable value — a value that is no integer at all, zero, or a
/// negative one — because it states the CONSTRAINT on the parameter rather than the
/// value that broke it: "MaxItems must be a non-zero positive integer."
pub const MAX_ITEMS_INVALID_MESSAGE: &str = "MaxItems must be a non-zero positive integer.";

/// The modelled `NoSuchDistribution` for a distribution id the store does not hold.
///
/// The id is NOT in the message: CloudFront's sentence for this refusal is fixed service
/// text, and the caller already knows which distribution it asked for.
pub fn no_such_distribution() -> NoSuchDistribution {
    NoSuchDistribution {
        message: Some(NO_SUCH_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchDistribution` an operation ADDRESSED by a distribution identifier
/// answers an identifier it could not resolve — the split this resource kind's recorded
/// contract makes, under the one error class this family's models bind for both conditions.
///
/// A value written in the alphabet CloudFront writes an entity identifier in
/// ([`is_entity_id_form`]) is an identifier of that mint, and WHICH one was ever issued is a
/// property of the STORE: such a value the account holds no distribution under is the
/// distribution reported ABSENT ([`no_such_distribution`]). A value written OUTSIDE it — in
/// lower case, around a separator, as a UUID, as a word or as a domain name — was never an
/// identifier at all, so it is the identifier the route reports it cannot READ
/// ([`id_format_invalid_message`]).
///
/// `member` is the MODEL's name for the member that carried the value, the one variable part
/// of that sentence: [`ADDRESSED_BY_ID`] for the distribution's own routes, and
/// [`ADDRESSED_BY_DISTRIBUTION_ID`] for the routes addressed by the distribution a resource
/// is held under. Nothing the request supplied reaches the wire — both sentences are fixed
/// text.
pub fn unresolved_distribution_id(member: &str, id: &str) -> NoSuchDistribution {
    if is_entity_id_form(id) {
        return no_such_distribution();
    }
    NoSuchDistribution {
        message: Some(id_format_invalid_message(member)),
    }
}

/// The modelled `NoSuchInvalidation` for an invalidation id a distribution does not
/// hold. Fixed service text, for the same reason [`no_such_distribution`] is.
pub fn no_such_invalidation() -> NoSuchInvalidation {
    NoSuchInvalidation {
        message: Some(NO_SUCH_INVALIDATION_MESSAGE.to_string()),
    }
}

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version
/// token but not the current one.
pub fn precondition_failed() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_MESSAGE.to_string()),
    }
}

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version token
/// but not the current one, worded as the POLICY surface words it
/// ([`PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE`]).
pub fn precondition_failed_for_this_operation() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE.to_string()),
    }
}

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version token
/// but not the current one, worded as the DISTRIBUTION-TENANT surface words it
/// ([`PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE`]) — the surface the trust stores
/// belong to as well.
pub fn precondition_failed_missing_requirements() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidIfMatchVersion` for an `If-Match` that is missing where the
/// operation requires one, or carries something that is no version token at all.
///
/// Fixed service text ([`INVALID_IF_MATCH_VERSION_MESSAGE`]), for the same reason
/// [`precondition_failed`] is: the caller learns which header failed from the one it
/// sent, never from a value echoed back to it.
pub fn invalid_if_match_version() -> crate::ops::types::InvalidIfMatchVersion {
    crate::ops::types::InvalidIfMatchVersion {
        message: Some(INVALID_IF_MATCH_VERSION_MESSAGE.to_string()),
    }
}

/// The sentence `DistributionAlreadyExists` carries: CloudFront's own wording, up to the
/// distribution the caller reference is already associated with, which it names at the
/// end — the one piece of request-derived detail in it, and the one a caller cannot work
/// out for itself.
///
/// A message rather than the shape, because the create path this service shares between
/// `CreateDistribution` and `CreateDistributionWithTags` reports its refusals as
/// [`CreateRefusal`], which each operation then lifts into its own modelled error.
pub fn distribution_already_exists_message(existing_id: &str) -> String {
    format!("{DISTRIBUTION_ALREADY_EXISTS_PREFIX}{existing_id}")
}

/// The modelled `InvalidArgument` for a `MaxItems` value no page size can be taken from.
pub fn invalid_max_items() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MAX_ITEMS_INVALID_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a path identifier that is not in the shape its resource
/// type's ids take with — the ONE producer of that sentence in this crate.
///
/// `member` is the MODEL's name for the request member that carried the identifier, which
/// is the only variable part of the text: an operation that addresses its resource by `Id`
/// answers "Id format is invalid", while `GetInvalidationForDistributionTenant` — whose
/// tenant arrives in `DistributionTenantId` — names that member instead. Nothing the
/// request SUPPLIED reaches the wire; the offending value is never echoed back.
///
/// [`RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE`] is this sentence's `Id` form as a
/// `&'static str`, for the suites that assert it in a const table; nothing builds a refusal
/// from it any more.
pub fn id_format_invalid_message(member: &str) -> String {
    format!("{member} format is invalid")
}

/// The modelled `InvalidArgument` (HTTP 400) for a path identifier outside the shape its
/// resource type's ids take, naming the member that carried it
/// ([`id_format_invalid_message`]).
///
/// The shape of an identifier is a property of the REQUEST, so an operation decides it
/// before it resolves the identifier against the store: a value that was never an id of
/// that resource kind is an argument CloudFront cannot read, not a resource that is
/// absent.
pub fn invalid_id_format(member: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(id_format_invalid_message(member)),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for the same argument defect
/// [`invalid_id_format`] reports, on an operation whose model serves no `InvalidArgument`:
/// the code and the status are then the operation's own, and the MESSAGE is all that tells
/// an unreadable identifier from a resource that is simply not there.
pub fn entity_not_found_id_format(member: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(id_format_invalid_message(member)),
    }
}

/// The modelled `NoSuchResource` (HTTP 404) an operation that ADDRESSES a key group by `Id`
/// answers when no stored group answers to it.
///
/// `GetKeyGroup`'s model declares this one error, so one code and one status cover both
/// outcomes and the MESSAGE is what tells them apart — the same split
/// [`unresolved_response_headers_policy_id`] makes: an `Id` outside the shape every key
/// group id takes ([`is_key_group_id`]) is refused as an unreadable identifier, while a
/// well-formed id the store holds no group for is the absent-group sentence
/// ([`no_such_key_group`]). Both are fixed text.
pub fn unresolved_key_group_id(id: &str) -> NoSuchResource {
    if is_key_group_id(id) {
        no_such_key_group()
    } else {
        NoSuchResource {
            message: Some(id_format_invalid_message("Id")),
        }
    }
}

/// The sentence a read of a key value store the store does not hold answers with: the
/// CONDITION, carrying nothing from the request, as every other not-found sentence of this
/// service states its own condition. CloudFront names the resource by its MODELLED shape
/// name (`KeyValueStore`) in this one, as the recorded contract carries it, rather than in
/// the prose form the other sentences use.
pub const KEY_VALUE_STORE_NOT_FOUND_MESSAGE: &str = "The specified KeyValueStore does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for a key value store name the store does
/// not hold — "The entity was not found."
///
/// Takes no identifier: what the refusal states is that the store holds no such resource,
/// which is a property of the store rather than of the value submitted, so the name asked
/// about is no part of the sentence.
pub fn key_value_store_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(KEY_VALUE_STORE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `CannotDeleteEntityWhileInUse` (HTTP 409) for a delete of a resource
/// something this store holds still names.
///
/// FIXED service text ([`ENTITY_IN_USE_MESSAGE`]), as the model states it on the shape
/// itself and for the reason [`function_in_use`] carries one: what the refusal states is a
/// property of the CONDITION — the resource is still in use — rather than of the request,
/// so nothing from the request or from the store belongs in the sentence and every delete
/// this condition refuses reads the same.
pub fn entity_in_use() -> crate::ops::types::CannotDeleteEntityWhileInUse {
    crate::ops::types::CannotDeleteEntityWhileInUse {
        message: Some(ENTITY_IN_USE_MESSAGE.to_string()),
    }
}

/// `CannotDeleteEntityWhileInUse`'s message, as the model documents the shape: "The entity
/// cannot be deleted while it is in use."
pub const ENTITY_IN_USE_MESSAGE: &str = "The entity cannot be deleted while it is in use.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a key value store name that is
/// already taken.
///
/// Carries NO message: the recorded service answers this conflict with the code and the
/// status alone, so there is no sentence to put on the wire and none is invented. The model
/// documents the shape ("The entity already exists. You must provide a unique entity."); that
/// prose states the condition and is not served.
pub fn key_value_store_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists { message: None }
}

/// The modelled `CannotDeleteEntityWhileInUse` (HTTP 409) for a delete of a key value store
/// a function still associates.
///
/// The sentence CloudFront serves for THIS resource kind names the store the request
/// addressed — its one variable part, taken from the request — and nothing else: no function
/// and no ARN, so a caller learns no identifier the request did not already carry. The
/// neighbouring kinds whose conflict is the same code serve the fixed [`ENTITY_IN_USE_MESSAGE`]
/// instead.
pub fn key_value_store_in_use(name: &str) -> crate::ops::types::CannotDeleteEntityWhileInUse {
    crate::ops::types::CannotDeleteEntityWhileInUse {
        message: Some(format!(
            "Cannot delete KeyValueStore {name} because it is associated with a function"
        )),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for a distribution id no stored distribution
/// carries, for the operations whose model names `EntityNotFound` rather than
/// `NoSuchDistribution` for that condition.
///
/// The same sentence [`NO_SUCH_DISTRIBUTION_MESSAGE`] states, and for the same reason: the
/// condition is the one both codes report, and it carries no part of the request.
pub fn distribution_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an ARN, an id or a name —
/// no stored distribution tenant answers to.
///
/// CloudFront's wording states the CONDITION and echoes no part of the request, so it
/// takes no identifier: the same sentence answers an unknown ARN, an unknown id and an
/// unknown name, which is also why every operation that resolves a tenant shares it.
/// Recorded behaviour — the model states only `EntityNotFound`'s own "The entity was not
/// found."
pub fn distribution_tenant_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some("The specified distribution tenant doesn't exist.".to_string()),
    }
}

/// The sentence CloudFront answers `ResourceNotDisabled` with for a distribution tenant,
/// on the wire.
///
/// The WIRE text — what the recorded service answers this code with for THIS resource
/// kind, and what this emulator therefore serves. Like
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from the sentence the MODEL's
/// documentation of the error shape uses ("The specified CloudFront resource hasn't been
/// disabled yet."): that one documents the condition for every resource kind the shape
/// serves, this one is what a tenant's delete answers. Do not "correct" this constant to
/// the documentation sentence.
///
/// A property of the resource's own state rather than of the request, so nothing from the
/// request belongs in it and every refusal of it reads the same.
pub const DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE: &str =
    "You must disable the distribution tenant before you can delete it.";

/// The modelled `ResourceNotDisabled` (HTTP 409) for a delete of a distribution tenant that
/// is still enabled: "To delete a distribution tenant, you must first disable the
/// distribution tenant by using the UpdateDistributionTenant API operation."
pub fn distribution_tenant_not_disabled() -> crate::ops::types::ResourceNotDisabled {
    crate::ops::types::ResourceNotDisabled {
        message: Some(DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a certificate request whose domain ownership it could
/// not verify with: the condition is a property of the domain's DNS rather than of any
/// value the caller sent, so the sentence names none of them — no domain, no tenant name
/// and no id appears in it. Recorded behaviour.
pub const DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE: &str =
    "The provided Domain Name is not valid. Could not verify Domain Name ownership. It may \
     not be pointing to a valid CloudFront resource.";

/// The modelled `InvalidArgument` (HTTP 400) for a CloudFront managed ACM certificate
/// request that asks CloudFront itself to serve the HTTP validation token of a domain whose
/// ownership this service cannot verify
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::unverifiable_certificate_domain`]).
pub fn domain_ownership_unverified() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a domain no certificate of this account covers with: what
/// the refusal states is the REQUIREMENT the caller has to satisfy — a certificate on the
/// tenant or on the distribution, or a managed certificate request to mint one — rather than
/// anything about the domain string submitted, so the sentence names no domain, no
/// certificate ARN and no tenant.
///
/// The one sentence for that condition wherever the tenant surface reaches it: a tenant
/// create or update claiming a domain nothing proves, and a domain-conflict listing whose
/// domain control validation resource carries no covering certificate.
pub const MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE: &str =
    "There must be at least one certificate in the distribution tenant or distribution, or \
     there must be a managed certificate request";

/// The modelled `InvalidArgument` (HTTP 400) for a domain no certificate of this account
/// covers ([`MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE`]).
pub fn missing_certificate_for_domain() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a value that is no domain name at all with: the condition
/// is a property of the value submitted rather than of the account's certificates, so the
/// sentence states that and names the value nowhere. Recorded behaviour.
pub const INVALID_DOMAIN_NAME_MESSAGE: &str = "The request contains an invalid domain name.";

/// The modelled `InvalidArgument` (HTTP 400) for a value that is no domain name at all.
pub fn invalid_domain_name() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_DOMAIN_NAME_MESSAGE.to_string()),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for a distribution tenant that carries no
/// CloudFront managed ACM certificate: the tenant exists, but no certificate was ever
/// requested for it, so there is no certificate for a read of one to answer from.
pub fn managed_certificate_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(
            "The specified distribution tenant has no CloudFront managed certificate."
                .to_string(),
        ),
    }
}

/// The sentence a read of a connection group this store does not hold answers with, as the
/// recorded `GetConnectionGroupByRoutingEndpoint` contract carries it: the whole sentence
/// is fixed, and no identifier of the group asked about appears in it.
pub const CONNECTION_GROUP_NOT_FOUND_MESSAGE: &str =
    "The specified connection group does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an id, a name, an ARN or a
/// routing endpoint — no connection group this store holds answers to.
///
/// Takes no identifier, so the same sentence answers every form the lookup admits: what
/// the refusal states is that the store holds no such group.
pub fn connection_group_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(CONNECTION_GROUP_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `ResourceNotDisabled` with for a connection group, on
/// the wire.
///
/// The WIRE text — what the recorded service answers this code with for THIS resource
/// kind, and what this emulator therefore serves. `ResourceNotDisabled` is shared by
/// several resource kinds and the served sentence names the kind the caller has to
/// disable, so each kind carries its own constant
/// ([`DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE`] is the tenant's); the sentence the
/// MODEL's documentation of the shape uses ("The specified CloudFront resource hasn't
/// been disabled yet.") documents the condition for all of them and is not served. Do not
/// "correct" this constant to the documentation sentence.
pub const CONNECTION_GROUP_NOT_DISABLED_MESSAGE: &str =
    "You must disable the specified connection group before you can delete it.";

/// The modelled `ResourceNotDisabled` (HTTP 409) for a delete of a connection group that
/// is still enabled.
///
/// FIXED service text ([`CONNECTION_GROUP_NOT_DISABLED_MESSAGE`]), for the reason
/// [`entity_in_use`]'s is fixed: what the refusal states is a property of the CONDITION —
/// the resource has not been disabled — rather than of the request, so nothing from the
/// request or from the store belongs in the sentence and every delete this condition
/// refuses reads the same.
pub fn connection_group_not_disabled() -> crate::ops::types::ResourceNotDisabled {
    crate::ops::types::ResourceNotDisabled {
        message: Some(CONNECTION_GROUP_NOT_DISABLED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a connection group name that is already taken with, on
/// the wire.
///
/// The WIRE text — what the recorded service answers `EntityAlreadyExists` with for THIS
/// resource kind, and what this emulator therefore serves. `EntityAlreadyExists` is shared
/// by several resource kinds and the served sentence names the kind whose name is taken, so
/// each kind carries its own constant ([`DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE`] is
/// the tenant's); the sentence the MODEL's documentation of the shape uses ("The entity
/// already exists. You must provide a unique entity.") documents the condition for all of
/// them and is not served. Do not "correct" this constant to the documentation sentence.
///
/// The WHOLE sentence is fixed: what the refusal states is that the account already holds a
/// group of that name, which is a property of the CONDITION rather than of the request, so
/// neither the submitted name nor the id of the group holding it is any part of it.
pub const CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "A connection group with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a connection group name that is
/// already taken: the name is "unique within your Amazon Web Services account".
///
/// Takes no name: the sentence is fixed service text
/// ([`CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`]), so every refusal of this condition reads
/// the same.
pub fn connection_group_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message CloudFront answers an `AssociationFilter` whose id it cannot use with, on
/// the wire: one CONSTANT sentence for the condition, carrying nothing from the request —
/// the recorded `ListConnectionGroups` contract pins this text, and it is a property of
/// the filter rather than of the value submitted.
pub const INVALID_ASSOCIATION_FILTER_MESSAGE: &str = "The association filter id is invalid.";

/// The modelled `InvalidArgument` (HTTP 400) for an `AssociationFilter` naming an id this
/// service cannot resolve — "An argument is invalid."
pub fn invalid_association_filter() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_ASSOCIATION_FILTER_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for an `AnycastIpListId` that names no
/// Anycast static IP list this service holds — "An argument is invalid."
///
/// The recorded `ListConnectionGroups` contract answers this condition with
/// `InvalidArgument` rather than `EntityNotFound`: the id is an argument CloudFront cannot
/// resolve, so every operation that takes one reports it the same way. Where the argument
/// arrives on an `AssociationFilter`, the sentence is that filter's own
/// ([`invalid_association_filter`]); elsewhere the member it arrived on is named, which a
/// caller cannot work out for itself.
pub fn invalid_anycast_ip_list(id: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: AnycastIpListId {id} is not an Anycast static IP list of \
             this account"
        )),
    }
}

/// The sentence a read of an Anycast IP list this store does not hold answers with, as the
/// recorded `GetAnycastIpList` contract carries it — down to the resource's wording in it
/// and to the absent full stop: the whole sentence is fixed, and the id asked about is no
/// part of it.
pub const ANYCAST_IP_LIST_NOT_FOUND_MESSAGE: &str = "The specified Anycast IP list does not exist";

/// The modelled `EntityNotFound` (HTTP 404) for an Anycast static IP list id no stored
/// list carries — "The entity was not found."
///
/// Takes no id: what the refusal states is that the store holds no such list, which is a
/// property of the store rather than of the value submitted.
pub fn anycast_ip_list_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(ANYCAST_IP_LIST_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `EntityAlreadyExists` (HTTP 409) for an Anycast static IP list name that is
/// already taken: a name identifies ONE list for its whole life, worded as every other
/// already-taken name of this service is.
pub fn anycast_ip_list_already_exists(name: &str) -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(format!(
            "An Anycast static IP list with the name {name} already exists in this account"
        )),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for an `IpCount` no Anycast static IP list can
/// be allocated for.
///
/// FIXED service text ([`invalid_anycast_ip_count_message`]): what the refusal states is the
/// CONSTRAINT on the parameter — the value set the member documents ([`ANYCAST_IP_COUNTS`]) —
/// which a caller cannot work out from the model alone, since the member carries no
/// `@range`. The value SUBMITTED is no part of it: the caller already holds it, and the
/// sentence states the rule it broke.
pub fn invalid_anycast_ip_count() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(invalid_anycast_ip_count_message()),
    }
}

/// The sentence CloudFront refuses an `IpCount` outside the documented value set with, on the
/// wire: it names the parameter the quantity arrives under and then the two counts a list may
/// be allocated for, which are this service's own ([`ANYCAST_IP_COUNTS`]) rather than numbers
/// repeated into the text.
pub fn invalid_anycast_ip_count_message() -> String {
    let admitted: Vec<String> = ANYCAST_IP_COUNTS.iter().map(i32::to_string).collect();
    format!(
        "The parameter Quantity Invalid quantity provided. The quantity of IPs in an \
         AnycastIpList should be either {}.",
        admitted.join(" or ")
    )
}

/// The modelled `EntityLimitExceeded` (HTTP 400) for a create this account's Anycast static
/// IP entitlement cannot be served from — in this emulator, one there are no addresses left
/// to allocate for ([`ANYCAST_IP_POOL`]).
///
/// The sentence is the WIRE text CloudFront answers this code with on this operation
/// ([`ANYCAST_IP_LIST_LIMIT_EXCEEDED_MESSAGE`]): the entitlement is a Service Quotas quota,
/// so what the refusal states is how to have it raised rather than any value from the
/// request.
pub fn anycast_ip_pool_exhausted() -> crate::ops::types::EntityLimitExceeded {
    crate::ops::types::EntityLimitExceeded {
        message: Some(ANYCAST_IP_LIST_LIMIT_EXCEEDED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a `CreateAnycastIpList` the account's entitlement cannot
/// serve with, on the wire: the Service Quotas page of the quota that bounds it
/// (`L-6A19EDFD`), beside the instruction to ask AWS Support for an increase.
///
/// CloudFront is a global service whose quotas are held in `us-east-1`, so that is the
/// console the page lives on for every account; the quota code is the one that bounds this
/// resource. Nothing from the request belongs in the text.
pub const ANYCAST_IP_LIST_LIMIT_EXCEEDED_MESSAGE: &str =
    "To request a limit increase for your Anycast static IP list, contact AWS Support: \
     https://us-east-1.console.aws.amazon.com/servicequotas/home/services/cloudfront/quotas/\
     L-6A19EDFD";

/// The sentence CloudFront answers a distribution tenant name that is already taken with,
/// on the wire: the WIRE text for `EntityAlreadyExists` on THIS resource kind, worded as
/// [`CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`] words it for the neighbouring one, and fixed
/// whole for the reason that one is — nothing from the request or from the store belongs in
/// it.
pub const DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE: &str =
    "A distribution tenant with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a distribution tenant name that is
/// already taken: the name is "unique within your Amazon Web Services account".
///
/// Takes no name, for the reason [`connection_group_already_exists`] takes none.
pub fn distribution_tenant_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The sentence `CNAMEAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with — and the WHOLE of it:
/// CloudFront states the condition without naming any of the names the request submitted,
/// so nothing from the request is appended to it. Like
/// [`NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE`] it differs from the sentence the MODEL's
/// documentation of the error shape uses for the same rule ("The CNAME specified is already
/// defined for CloudFront."); do not "correct" this constant to the documentation sentence.
pub const CNAME_ALREADY_EXISTS_MESSAGE: &str =
    "One or more of the CNAMEs you provided are already associated with a different resource.";

/// The modelled `CNAMEAlreadyExists` (HTTP 409) for a domain another CloudFront resource
/// already serves.
pub fn cname_already_exists() -> CNAMEAlreadyExists {
    CNAMEAlreadyExists {
        message: Some(CNAME_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message `NoSuchCloudFrontOriginAccessIdentity` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE`], it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("The specified origin access identity does not exist."): the served sentence names the
/// resource as the API does, a CLOUDFRONT origin access identity, while the documentation
/// states the condition in the shorter form. Do not "correct" this constant to the
/// documentation sentence.
pub const NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE: &str =
    "The specified CloudFront origin access identity does not exist.";

/// The modelled `NoSuchCloudFrontOriginAccessIdentity` (HTTP 404) for an identity id the
/// store does not hold.
///
/// The id is NOT in the message, for the same reason it is not in
/// [`no_such_distribution`]'s: the sentence is fixed service text, and the caller already
/// knows which identity it asked for.
pub fn no_such_origin_access_identity() -> NoSuchCloudFrontOriginAccessIdentity {
    NoSuchCloudFrontOriginAccessIdentity {
        message: Some(NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `CloudFrontOriginAccessIdentityInUse` with, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from
/// the sentence the MODEL's documentation of the error shape uses for the same condition
/// ("The Origin Access Identity specified is already in use."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
///
/// The sentence names the resource KIND and nothing else: the condition is that something
/// still references the identity, which is a property of the store rather than of the
/// request, and the referencing distribution's own id is NOT part of it — the same shape
/// [`ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE`] takes for the other way an origin reaches a
/// bucket.
pub const ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE: &str =
    "The CloudFront origin access identity is still being used.";

/// The modelled `CloudFrontOriginAccessIdentityInUse` (HTTP 409) for a delete of an
/// identity an Amazon S3 origin still names.
///
/// FIXED service text ([`ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE`]), so every refusal of this
/// condition reads the same and none of them leaks the identifier of the resource holding
/// the reference.
pub fn origin_access_identity_in_use() -> crate::ops::types::CloudFrontOriginAccessIdentityInUse {
    crate::ops::types::CloudFrontOriginAccessIdentityInUse {
        message: Some(ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE.to_string()),
    }
}

/// The sentence `CloudFrontOriginAccessIdentityAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the prose the MODEL's documentation of the shape uses
/// (which states the condition rather than a served sentence): this one states what the
/// condition IS — the caller reference the request carried already names an identity.
///
/// The WHOLE sentence is fixed, as [`PUBLIC_KEY_ALREADY_EXISTS_MESSAGE`] is for the
/// neighbouring resource: the conflict is over the reference the REQUEST submitted, so the
/// caller already holds it, and the identifier of the identity that holds it is no part of
/// the text.
pub const ORIGIN_ACCESS_IDENTITY_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference you are using to create the CloudFront origin access identity is \
     associated with another identity.";

/// The modelled `InvalidArgument` for a `ListStreamingDistributions` `Marker` that is no
/// resume point this service ever handed out.
///
/// States the CONSTRAINT on the parameter rather than the value that broke it, the way
/// [`invalid_origin_access_identity_marker`] and [`invalid_public_key_marker`] do for the
/// listings of their own resource kinds.
pub fn invalid_streaming_distribution_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE.to_string()),
    }
}

/// The sentence [`invalid_streaming_distribution_marker`] carries: a `Marker` for this
/// listing is a streaming distribution id.
///
/// The WIRE text, as the recorded service answers this refusal with — the same shape
/// [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] takes for its own listing, down to naming the
/// resource kind the id belongs to.
pub const INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE: &str =
    "Marker must be a valid streaming distribution Id";

/// The modelled `NoSuchStreamingDistribution` for a streaming distribution id the store
/// does not hold.
///
/// The id is NOT in the message, for the same reason it is not in
/// [`no_such_distribution`]'s: the sentence is fixed service text, and the caller already
/// knows which distribution it asked for.
pub fn no_such_streaming_distribution() -> crate::ops::types::NoSuchStreamingDistribution {
    crate::ops::types::NoSuchStreamingDistribution {
        message: Some(NO_SUCH_STREAMING_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// `NoSuchStreamingDistribution`'s message: the sentence the model states on the shape
/// itself, "The specified streaming distribution does not exist."
pub const NO_SUCH_STREAMING_DISTRIBUTION_MESSAGE: &str =
    "The specified streaming distribution does not exist.";

/// The sentence `StreamingDistributionAlreadyExists` carries, up to the streaming
/// distribution the caller reference is already associated with, which is named at the end
/// — the one piece of request-derived detail in it, and the one a caller cannot work out
/// for itself. The same shape [`distribution_already_exists_message`] takes, for the same
/// rule applied to the other resource.
pub fn streaming_distribution_already_exists_message(existing_id: &str) -> String {
    format!(
        "The caller reference that you are using to create a streaming distribution is \
         associated with another streaming distribution. Already exists: {existing_id}"
    )
}

/// The longest comment an origin access identity carries.
///
/// `CloudFrontOriginAccessIdentityConfig$Comment`: "A comment to describe the origin
/// access identity. The comment cannot be longer than 128 characters." The member carries
/// no `@length` in the model, so the bound is not refused before a handler runs and the
/// create path checks it itself.
pub const MAX_ORIGIN_ACCESS_IDENTITY_COMMENT: usize = 128;

/// The modelled `InvalidArgument` for a `Comment` longer than the member admits — the ONE
/// refusal every comment bound in this service is served with.
///
/// Recorded behaviour: CloudFront refuses the condition as the PARAMETER the value arrived in
/// ([`parameter_too_big_message`]), named as the model names the member
/// ([`COMMENT_MEMBER`]). The whole sentence is fixed text: neither the bound nor the length
/// submitted is echoed, so one comment past the bound and another far past it read alike, and
/// every surface that carries a `Comment` refuses it with this one sentence.
pub fn comment_too_long() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(comment_too_long_message()),
    }
}

/// The member a comment arrives in, as every configuration of this service names it.
pub const COMMENT_MEMBER: &str = "Comment";

/// The message a comment past its member's bound is refused with.
pub fn comment_too_long_message() -> String {
    parameter_too_big_message(COMMENT_MEMBER)
}

/// The modelled `NoSuchResponseHeadersPolicy` (HTTP 404) for an id neither source of
/// response headers policies holds one for.
///
/// This is the refusal of the STORE: it states that no policy answers to the id submitted,
/// and it says nothing about the id's shape. It is what an operation that resolves such an
/// id as a lookup KEY — one whose own resource is something else, so it never addresses a
/// policy by that id — answers for every id no readable policy exists for, whatever shape
/// the id takes; the sibling policy families answer their own one sentence the same way
/// ([`no_such_cache_policy`], [`no_such_origin_request_policy`]). The shape of the
/// identifier is examined only where an operation ADDRESSES the policy resource itself by
/// it, which is [`unresolved_response_headers_policy_id`].
pub fn no_such_response_headers_policy() -> NoSuchResponseHeadersPolicy {
    NoSuchResponseHeadersPolicy {
        message: Some(NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchResponseHeadersPolicy` (HTTP 404) an operation that ADDRESSES the
/// policy resource by `id` answers when no readable policy answers to it.
///
/// Such an operation checks the SHAPE of the identifier before it looks anything up, and ONE
/// code (and one status) covers both outcomes, so the MESSAGE is what tells them apart: an
/// `id` outside the shape every response headers policy id takes ([`is_minted_uuid`]) is
/// refused as an unusable identifier ([`RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE`]), while
/// a well-formed id that names no policy — neither one this account created nor one AWS
/// published — is the absent-policy sentence ([`no_such_response_headers_policy`]). Both are
/// fixed text: each states a property of the identifier or of the store rather than echoing
/// the value submitted, so nothing from the request reaches the wire.
pub fn unresolved_response_headers_policy_id(id: &str) -> NoSuchResponseHeadersPolicy {
    if is_minted_uuid(id) {
        no_such_response_headers_policy()
    } else {
        NoSuchResponseHeadersPolicy {
            message: Some(id_format_invalid_message("Id")),
        }
    }
}

/// The message `NoSuchResponseHeadersPolicy` carries for an identifier that is not in the
/// shape a response headers policy id takes at all.
///
/// The WIRE text CloudFront answers such a request with: it states that the identifier
/// SUBMITTED is unusable, which is a different condition from a well-formed id that names no
/// policy ([`NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE`]) — the two share the code and the
/// status, so the message is all a caller has to tell them apart. Fixed text; it interpolates
/// nothing, and in particular never echoes the id back.
///
/// Only an operation that ADDRESSES the policy resource by the identifier serves it, through
/// [`unresolved_response_headers_policy_id`]: the precheck belongs to that operation, not to
/// the error code.
///
/// The one PRODUCER of the sentence is [`id_format_invalid_message`], which spells it for any
/// member name; this constant is its `Id` form, kept for the suites that assert the sentence
/// as a `&'static str`, and `id_format_invalid_message_matches_the_constant_form` below pins
/// the two together so neither can drift.
pub const RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE: &str = "Id format is invalid";

/// The modelled `IllegalUpdate` (HTTP 400) for an update that would move a resource's
/// `CallerReference` onto another value.
///
/// A `CallerReference` "uniquely identifies a request to create a resource" and names that
/// resource for its whole life, so no update changes one — whichever resource is being
/// updated. The sentence states that rule and NOTHING from the request or the stored
/// record: it is the one fixed sentence the service refuses this modification with, so
/// every refusal of it reads the same.
pub fn illegal_update_caller_reference() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for an update that would change a `CallerReference`, on the
/// wire.
pub const ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE: &str =
    "You cannot update the value of CallerReference.";

/// The modelled `IllegalUpdate` (HTTP 400) for an update that would move a DISTRIBUTION's
/// `CallerReference` onto another value.
///
/// The same RULE [`illegal_update_caller_reference`] states — a caller reference names its
/// resource for the resource's whole life, so no update moves one — reported in the sentence
/// the DISTRIBUTION surface words it with
/// ([`ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE`]). The wording of a refusal is
/// the service's own per surface, as [`DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE`] and the
/// other wire sentences here are; the sentence takes nothing from the request or the stored
/// record, so every refusal of it reads the same.
pub fn illegal_update_distribution_caller_reference() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for an update that would change a DISTRIBUTION's
/// `CallerReference`, on the wire.
pub const ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE: &str =
    "The caller reference cannot be modified.";

/// The modelled `MonitoringSubscriptionAlreadyExists` (HTTP 409) for a distribution that
/// already carries a monitoring subscription.
///
/// A property of the distribution's state rather than of the request, so nothing from the
/// request belongs in the text, exactly as in [`no_such_distribution`]: one CONSTANT per
/// error code.
pub fn monitoring_subscription_already_exists()
-> crate::ops::types::MonitoringSubscriptionAlreadyExists {
    crate::ops::types::MonitoringSubscriptionAlreadyExists {
        message: Some(MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message `MonitoringSubscriptionAlreadyExists` carries, on the wire.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. Like
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`], it deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("A monitoring
/// subscription already exists for the specified distribution."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
pub const MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE: &str =
    "A monitoring subscription already exists for this distribution";

/// The modelled `NoSuchMonitoringSubscription` (HTTP 404) for a distribution that carries
/// no monitoring subscription. Fixed service text, for the same reason
/// [`monitoring_subscription_already_exists`]'s is.
pub fn no_such_monitoring_subscription() -> NoSuchMonitoringSubscription {
    NoSuchMonitoringSubscription {
        message: Some(NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE.to_string()),
    }
}

/// The message `NoSuchMonitoringSubscription` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE`] and
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`], it differs from the sentence the MODEL's
/// documentation of the error shape uses for the same rule ("A monitoring subscription does
/// not exist for the specified distribution."): that one documents the condition, this one
/// is served. Do not "correct" this constant to the documentation sentence.
pub const NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE: &str =
    "The monitoring subscription does not exist.";

/// The message `NoSuchResponseHeadersPolicy` carries for an id no readable policy answers
/// to, on the wire.
///
/// The WIRE text CloudFront answers this code with when the store holds no policy for the id
/// submitted, in the form this service states that condition in for every other named
/// resource ([`NO_SUCH_CACHE_POLICY_MESSAGE`], [`NO_SUCH_DISTRIBUTION_MESSAGE`],
/// [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`]): "The specified <resource> does not exist."
/// An operation that addresses the policy resource by the identifier serves it for a
/// well-formed id only, since it prechecks the shape
/// ([`unresolved_response_headers_policy_id`]); one that resolves the id as a lookup key
/// serves it for every unknown id ([`no_such_response_headers_policy`]).
/// It differs from the sentence the MODEL's documentation of the error shape uses for the
/// same rule ("The response headers policy does not exist."), which documents the condition
/// rather than stating what is served, so do not "correct" this constant to it.
pub const NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE: &str =
    "The specified response headers policy does not exist.";

/// The modelled `NoSuchOriginAccessControl` (HTTP 404) for an origin access control id
/// the store does not hold, which is what an operation ADDRESSED by such an id answers a
/// WELL-FORMED one it cannot resolve (`GetOriginAccessControl`,
/// `GetOriginAccessControlConfig`, `UpdateOriginAccessControl`,
/// `DeleteOriginAccessControl`, through
/// [`unresolved_origin_access_control_id`], which words the other half).
///
/// `Id` is a plain `string` `@httpLabel` with no `@pattern` and no `@length`
/// (`smithy_query(query="operation",operation="GetOriginAccessControlConfig")`), so no route
/// refuses a value for breaking a modelled constraint; what the recorded service does read is
/// the ALPHABET the value is written in, and a value written in the one an entity identifier is
/// written in is resolved against the STORE alone — every id of that alphabet the account holds
/// no control under is the control reported ABSENT, whatever width or namespace letter it
/// carries, since the four kinds of that namespace share the letter.
///
/// The sentence is a property of the store rather than of the request, so nothing from
/// the request belongs in the text.
pub fn no_such_origin_access_control() -> NoSuchOriginAccessControl {
    NoSuchOriginAccessControl {
        message: Some(NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string()),
    }
}

/// `NoSuchOriginAccessControl`'s message, in the form CloudFront answers a `NoSuch…` with
/// for a named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_DISTRIBUTION_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE`] carry
/// it. Each fills `<resource>` with the resource as the API names it, which for the origin
/// access identity is the longer "CloudFront origin access identity". The recorded service
/// answers this code with that sentence, which the shape's own documentation states in its
/// shorter form.
pub const NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE: &str =
    "The specified origin access control does not exist.";

/// The modelled `InvalidOriginAccessControl` (HTTP 400) for a distribution configuration
/// whose origin names an origin access control this account does not hold.
///
/// The sentence states the condition the configuration broke — the control it names is not
/// one that can be resolved — which is a property of the store rather than of the request,
/// so nothing from the request belongs in the text.
pub fn invalid_origin_access_control() -> crate::ops::types::InvalidOriginAccessControl {
    crate::ops::types::InvalidOriginAccessControl {
        message: Some(INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string()),
    }
}

/// The message `InvalidOriginAccessControl` carries for an origin that names a control the
/// store cannot resolve, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("The origin access control is not valid."): that one documents the condition, this one
/// is served. Do not "correct" this constant to the documentation sentence.
pub const INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE: &str =
    "The specified origin access control does not exist or is not valid.";

/// The modelled `IllegalOriginAccessConfiguration` (HTTP 400) for a configuration whose
/// origin names BOTH an origin access control and an origin access identity.
///
/// The sentence states the condition the configuration broke — the two mechanisms are
/// mutually exclusive on one origin — which is a property of the configuration's shape
/// rather than of any value in it, so no origin id, control id or identity id belongs in
/// the text.
pub fn illegal_origin_access_configuration() -> crate::ops::types::IllegalOriginAccessConfiguration
{
    crate::ops::types::IllegalOriginAccessConfiguration {
        message: Some(ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE.to_string()),
    }
}

/// The message `IllegalOriginAccessConfiguration` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE`] it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("An origin cannot contain both an origin access control (OAC) and an origin access
/// identity (OAI)."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE: &str =
    "Cannot use both Origin Access Control and Origin Access Identity on an origin";

/// The message `InvalidLambdaFunctionAssociation` carries for the association whose ARN
/// names no published function version: the fixed sentence stating the FORM the ARN has to
/// have — the function ARN qualified with the number of a published version — and then the
/// value the request supplied for that association, echoed verbatim after " ARN: ".
///
/// A configuration may carry an association on the default cache behaviour and on every
/// ordered one, so the sentence alone does not say WHICH entry was refused; the ARN named
/// at the end is the only thing in the response that identifies it, and it is the request's
/// own string (its partition, Region, account, function name and qualifier all come from
/// the request, never from a constant here). Like [`INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE`]
/// the sentence is the SERVED text, which differs from the one the model's documentation of
/// the error shape uses for the same rule ("The specified Lambda@Edge function association
/// is invalid."): that one describes the condition, this one is sent.
pub fn invalid_lambda_function_association_message(lambda_function_arn: &str) -> String {
    format!("{INVALID_LAMBDA_FUNCTION_ASSOCIATION_PREFIX}{lambda_function_arn}")
}

/// The fixed part of [`invalid_lambda_function_association_message`], up to and including
/// the " ARN: " separator the request's value follows.
pub const INVALID_LAMBDA_FUNCTION_ASSOCIATION_PREFIX: &str =
    "The function ARN must reference a specific function version. (The ARN must end with \
     the version number.) ARN: ";

/// The sentence `OriginAccessControlAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. Like [`CNAME_ALREADY_EXISTS_MESSAGE`] it differs from the sentence the
/// MODEL's documentation of the error shape uses for the same condition ("An origin access
/// control with the specified parameters already exists."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
///
/// The WHOLE sentence is fixed: what the refusal states is that the account already holds
/// such a control, so neither the submitted configuration nor the id of the control that
/// holds it is any part of it.
///
/// This constant is the SENTENCE the refusal carries, not the predicate that raises it:
/// what makes a submitted configuration a conflict is
/// [`ComAmazonawsCloudfrontCloudfront20200531State::origin_access_control_named`]'s own
/// rule, which this does not decide. The recorded service words the sentence around the
/// control's NAME, which is the value that predicate compares.
pub const ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE: &str =
    "An origin access control with the same name already exists.";

/// The modelled `OriginAccessControlAlreadyExists` (HTTP 409) for a create or an update
/// naming a control the account already holds.
///
/// Takes no identifier: the sentence is fixed service text
/// ([`ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE`]), so both entry points into the
/// uniqueness rule refuse this condition with the one text and neither leaks the id of the
/// control that holds the configuration.
pub fn origin_access_control_already_exists() -> crate::ops::types::OriginAccessControlAlreadyExists
{
    crate::ops::types::OriginAccessControlAlreadyExists {
        message: Some(ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `OriginAccessControlInUse` with, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from
/// the sentence the MODEL's documentation of the error shape uses for the same condition
/// ("Cannot delete the origin access control because it's in use by one or more
/// distributions."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
///
/// The sentence names the resource KIND and nothing else, for the reason
/// [`ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE`] does: the condition is that something still
/// references the control, and the referencing distribution's own id is no part of it.
pub const ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE: &str =
    "The CloudFront origin access control is still being used.";

/// The modelled `OriginAccessControlInUse` (HTTP 409) for a delete of a control an origin
/// still names.
///
/// FIXED service text ([`ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE`]), so every refusal of this
/// condition reads the same and none of them leaks the identifier of the resource holding
/// the reference.
pub fn origin_access_control_in_use() -> crate::ops::types::OriginAccessControlInUse {
    crate::ops::types::OriginAccessControlInUse {
        message: Some(ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` for an `OriginAccessControlConfig$Name` longer than the
/// documented bound.
///
/// The message states the CONSTRAINT the request broke and the length it submitted, which
/// is the rule the refusal is built from rather than text echoed out of the request.
pub fn origin_access_control_name_too_long(length: usize) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: the origin access control name cannot be longer than \
             {MAX_ORIGIN_ACCESS_CONTROL_NAME} characters but the request submits {length}"
        )),
    }
}

/// The sentence `ResponseHeadersPolicyAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the error
/// shape uses for the same rule ("A response headers policy with this name already exists.
/// You must provide a unique name. To modify an existing response headers policy, use
/// UpdateResponseHeadersPolicy."): that one documents the condition and tells the caller
/// what to do instead, this one is served. Do not "correct" this constant to the
/// documentation sentence.
///
/// The WHOLE sentence is fixed, as [`CACHE_POLICY_ALREADY_EXISTS_MESSAGE`] and
/// [`ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE`] are for the same rule over the other two
/// policy families: nothing from the request or from the store belongs in it.
pub const RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another response headers policy with the same name already exists within the aws account.";

/// The modelled `ResponseHeadersPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken.
///
/// Takes no name: the sentence is fixed service text
/// ([`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`]), so the create and the update refuse
/// a taken name with the one text.
pub fn response_headers_policy_already_exists() -> ResponseHeadersPolicyAlreadyExists {
    ResponseHeadersPolicyAlreadyExists {
        message: Some(RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchOriginRequestPolicy` (HTTP 404) for an origin request policy id the
/// store does not hold.
///
/// The sentence states a property of the store rather than of the request, so nothing from
/// the request belongs in the text.
pub fn no_such_origin_request_policy() -> NoSuchOriginRequestPolicy {
    NoSuchOriginRequestPolicy {
        message: Some(NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchOriginRequestPolicy`'s message, in the form CloudFront answers a `NoSuch…` with for
/// a named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_CACHE_POLICY_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] carry it.
/// The recorded service answers this code with that sentence — which is what a replay of the
/// origin request policy surface holds it to — and the shape's own documentation states it in
/// its shorter form.
pub const NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE: &str =
    "The specified origin request policy does not exist.";

/// The sentence `OriginRequestPolicyAlreadyExists` carries, on the wire: the WIRE text for
/// this code, worded as [`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`] words the same
/// rule for the other policy family, and fixed whole for the reason that one is. It differs
/// from the sentence the MODEL's documentation of the shape uses ("An origin request policy
/// with this name already exists. You must provide a unique name. To modify an existing
/// origin request policy, use UpdateOriginRequestPolicy."); do not "correct" this constant
/// to the documentation sentence.
pub const ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another origin request policy with the same name already exists within the aws account.";

/// The modelled `OriginRequestPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken.
///
/// Takes no name, for the reason [`response_headers_policy_already_exists`] takes none.
pub fn origin_request_policy_already_exists() -> OriginRequestPolicyAlreadyExists {
    OriginRequestPolicyAlreadyExists {
        message: Some(ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The longest comment a cache policy carries.
///
/// `CachePolicyConfig$Comment`: "A comment to describe the cache policy. The comment
/// cannot be longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_CACHE_POLICY_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchCachePolicy` (HTTP 404) for a cache policy id the store does not
/// hold.
///
/// The sentence is the one the model states on the shape itself — "The cache policy does
/// not exist." — a property of the store rather than of the request, so nothing from the
/// request belongs in the text.
pub fn no_such_cache_policy() -> NoSuchCachePolicy {
    NoSuchCachePolicy {
        message: Some(NO_SUCH_CACHE_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchCachePolicy`'s message, in the form CloudFront answers a `NoSuch…` with for a
/// named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_DISTRIBUTION_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] carry it.
/// The recorded service answers this code with that sentence — which is what a replay of
/// the cache policy surface holds it to — and the shape's own documentation states it in its
/// shorter form.
pub const NO_SUCH_CACHE_POLICY_MESSAGE: &str = "The specified cache policy does not exist.";

/// The modelled `CachePolicyInUse` (HTTP 409) for a delete of a policy a cache behaviour
/// still attaches.
///
/// The sentence is a property of the CONDITION rather than of the request — the policy is
/// associated with a cache behaviour, and the caller is told to disassociate it first — so
/// nothing from the request or from the store belongs in it, and every refusal of this
/// condition reads the same.
pub fn cache_policy_in_use() -> crate::ops::types::CachePolicyInUse {
    crate::ops::types::CachePolicyInUse {
        message: Some(CACHE_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// `CachePolicyInUse`'s message: what CloudFront answers a delete of a cache policy that a
/// cache behaviour still associates.
pub const CACHE_POLICY_IN_USE_MESSAGE: &str =
    "The specified cache policy is currently associated with a cache behavior. Please \
     disassociate the policy before deleting.";

/// The modelled `OriginRequestPolicyInUse` (HTTP 409) for a delete of a policy a cache
/// behaviour still attaches.
///
/// The SAME rule [`cache_policy_in_use`] states for the other policy a cache behaviour
/// attaches: the sentence is a property of the CONDITION rather than of the request — the
/// policy is associated with a cache behaviour, and the caller is told to disassociate it
/// first — so nothing from the request or from the store belongs in it, and every refusal of
/// this condition reads the same.
pub fn origin_request_policy_in_use() -> crate::ops::types::OriginRequestPolicyInUse {
    crate::ops::types::OriginRequestPolicyInUse {
        message: Some(ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// `OriginRequestPolicyInUse`'s message: what CloudFront answers a delete of an origin
/// request policy that a cache behaviour still associates, worded as
/// [`CACHE_POLICY_IN_USE_MESSAGE`] words the same condition for the other policy.
pub const ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE: &str =
    "The specified origin request policy is currently associated with a cache behavior. \
     Please disassociate the policy before deleting.";

/// The modelled `ResponseHeadersPolicyInUse` (HTTP 409) for a delete of a policy a cache
/// behaviour still attaches.
///
/// The SAME rule [`cache_policy_in_use`] states for the other policies a cache behaviour
/// attaches: the sentence is a property of the CONDITION rather than of the request — the
/// policy is associated with a cache behaviour, and the caller is told to disassociate it
/// first — so nothing from the request or from the store belongs in it, and every refusal of
/// this condition reads the same.
pub fn response_headers_policy_in_use() -> crate::ops::types::ResponseHeadersPolicyInUse {
    crate::ops::types::ResponseHeadersPolicyInUse {
        message: Some(RESPONSE_HEADERS_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// `ResponseHeadersPolicyInUse`'s message: what CloudFront answers a delete of a response
/// headers policy that a cache behaviour still associates, worded as
/// [`CACHE_POLICY_IN_USE_MESSAGE`] words the same condition for the other policies a cache
/// behaviour attaches.
///
/// The WIRE family of this surface, not the sentence the MODEL's documentation of the shape
/// uses for the same condition ("Cannot delete the response headers policy because it is
/// attached to one or more cache behaviors in a CloudFront distribution."): that one
/// documents the condition, this one is served — the same split
/// [`CACHE_POLICY_IN_USE_MESSAGE`] records for its own shape.
pub const RESPONSE_HEADERS_POLICY_IN_USE_MESSAGE: &str =
    "The specified response headers policy is currently associated with a cache behavior. \
     Please disassociate the policy before deleting.";

/// The modelled `IllegalDelete` (HTTP 400) for a delete whose target is a policy AWS
/// published rather than one this account created.
///
/// A published policy is READ-ONLY to the account that reads it — nothing a caller does
/// creates, changes or removes one — so a delete of one is refused for what the REQUEST
/// asked for rather than answered "does not exist": the resource is there, and a caller
/// told it is missing would try to create it. The sentence states that condition and names
/// nothing from the request, so every refusal of it reads the same.
pub fn illegal_delete_managed_policy() -> crate::ops::types::IllegalDelete {
    crate::ops::types::IllegalDelete {
        message: Some(ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE.to_string()),
    }
}

/// `IllegalDelete`'s message, in the form CloudFront answers a delete of a policy AWS owns
/// with: delete is not allowed for it.
pub const ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE: &str =
    "The request failed because delete is not allowed for this policy.";

/// The modelled `IllegalUpdate` (HTTP 400) for an update whose target is a policy AWS
/// published rather than one this account created.
///
/// The SAME rule [`illegal_delete_managed_policy`] states, over the other mutation a policy
/// takes: a published policy is read-only to this account, so the update is refused for
/// what the request asked for and the policy keeps the configuration AWS gave it. The
/// sentence is that one worded for this mutation.
pub fn illegal_update_managed_policy() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for a policy AWS owns: update is not allowed for it.
pub const ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE: &str =
    "The request failed because update is not allowed for this policy.";

/// The sentence `CachePolicyAlreadyExists` carries, on the wire: the WIRE text for this
/// code, worded as [`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`] words the same rule
/// for the other policy family, and fixed whole for the reason that one is. It differs from
/// the sentence the MODEL's documentation of the shape uses ("A cache policy with this name
/// already exists. You must provide a unique name. To modify an existing cache policy, use
/// UpdateCachePolicy."); do not "correct" this constant to the documentation sentence.
pub const CACHE_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another cache policy with the same name already exists within the aws account.";

/// The modelled `CachePolicyAlreadyExists` (HTTP 409) for a policy name that is already
/// taken.
///
/// Takes no name, for the reason [`response_headers_policy_already_exists`] takes none.
pub fn cache_policy_already_exists() -> CachePolicyAlreadyExists {
    CachePolicyAlreadyExists {
        message: Some(CACHE_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The longest comment a public key carries.
///
/// `PublicKeyConfig$Comment`: "A comment to describe the public key. The comment cannot be
/// longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_PUBLIC_KEY_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The longest name a public key carries.
///
/// `PublicKeyConfig$Name` is "a name to help identify the public key" and carries no `@length`
/// in the model, so the bound is not refused before a handler runs and the public key path
/// checks it itself. What is RECORDED is that CloudFront refuses a longer name as the parameter
/// it arrived in ([`public_key_name_too_long`]) and that every name a recorded upload is SERVED
/// for is inside this bound — not the number itself, which no recorded refusal states (the
/// sentence carries neither the submitted name nor the limit). So the value held to is the 128
/// characters this configuration's other free-text member states
/// ([`MAX_PUBLIC_KEY_COMMENT`]) — one bound for the configuration's text rather than a second
/// literal, exactly as the key group surface holds its own name to its comment's bound
/// ([`MAX_KEY_GROUP_NAME`]). A later reader should take the bound as THAT and not as a recorded
/// number.
pub const MAX_PUBLIC_KEY_NAME: usize = MAX_PUBLIC_KEY_COMMENT;

/// The name the WIRE reports `PublicKeyConfig$Name` under, which is the name CloudFront's
/// refusal of that member carries: the member named for the resource it identifies rather than
/// the bare `Name` the payload spells it.
pub const PUBLIC_KEY_NAME_PARAMETER: &str = "PublicKeyName";

/// The modelled `InvalidArgument` (HTTP 400) for a public key name longer than
/// [`MAX_PUBLIC_KEY_NAME`].
///
/// Recorded behaviour: the whole sentence is fixed text naming the PARAMETER the name arrived
/// in ([`parameter_too_big_message`]) — neither the submitted name nor the bound is echoed, so
/// the refusal reads the same for every name past it.
pub fn public_key_name_too_long() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(public_key_name_too_long_message()),
    }
}

/// The message a public key write naming a name past [`MAX_PUBLIC_KEY_NAME`] is refused with.
pub fn public_key_name_too_long_message() -> String {
    parameter_too_big_message(PUBLIC_KEY_NAME_PARAMETER)
}

/// Whether `name` is a name a public key of this account can be identified by: a name built
/// from the characters CloudFront admits in one.
///
/// `PublicKeyConfig$Name` is "a name to help identify the public key" and carries no `@pattern`
/// in the model, so the alphabet is not enforced before a handler runs. Recorded behaviour on
/// both sides of it: CloudFront refuses a name carrying a character outside the alphabet
/// ([`public_key_name_illegal_characters`]), and every name a recorded upload is SERVED for is
/// written from ASCII letters, digits, `-` and `_` — the SAME alphabet the sibling name of this
/// surface is held to ([`is_key_group_name`]), read through that one reading rather than a
/// second of its own.
///
/// The rule is about the CHARACTERS a name carries, so the empty name is not refused here, for
/// the reason [`is_key_group_name`] does not refuse it either.
pub fn is_public_key_name(name: &str) -> bool {
    is_key_group_name(name)
}

/// The name the WIRE reports the offending member under when a public key name carries a
/// character CloudFront does not admit: the member qualified by the resource whose
/// configuration carries it, as the recorded refusal of that condition carries it — which is
/// not how the recorded refusal of the same member's LENGTH names it
/// ([`PUBLIC_KEY_NAME_PARAMETER`]), so the two are separate constants rather than one.
pub const PUBLIC_KEY_NAME_MEMBER: &str = "PublicKey:Name";

/// The modelled `InvalidArgument` (HTTP 400) for a public key name carrying a character
/// CloudFront does not admit in one ([`is_public_key_name`]).
///
/// Recorded behaviour: the whole sentence is fixed text naming the member the value arrived in
/// and nothing of the value itself, so the refusal reads the same for every such name.
pub fn public_key_name_illegal_characters() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(public_key_name_illegal_characters_message()),
    }
}

/// The message a public key write naming a name outside [`is_public_key_name`] is refused with.
pub fn public_key_name_illegal_characters_message() -> String {
    format!("The parameter {PUBLIC_KEY_NAME_MEMBER} contains illegal characters.")
}

/// The modelled `NoSuchPublicKey` (HTTP 404) for a public key id the store does not hold.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// public key — so nothing from the request belongs in the text and every path that raises
/// this code reads the same.
///
/// This is the answer an operation gives an `Id` the store holds no key under once the value
/// could be an identifier CloudFront issued for a public key at all: an identifier that
/// ADDRESSES one is read for its form first ([`unresolved_public_key`]), and WHICH identifier of
/// this kind was ever issued is a property of the STORE rather than of the value. A public key
/// id borne by a submitted DOCUMENT rather than by the request's path addresses nothing, so it
/// is resolved against the store alone and reports the absent key whatever its shape.
pub fn no_such_public_key() -> crate::ops::types::NoSuchPublicKey {
    crate::ops::types::NoSuchPublicKey {
        message: Some(NO_SUCH_PUBLIC_KEY_MESSAGE.to_string()),
    }
}

/// `NoSuchPublicKey`'s message, as CloudFront sends it on the wire — observed in recorded
/// CloudFront responses to this refusal, which is the source this wording comes from. The
/// service names the resource "CloudFront public key" and words the condition "does not
/// exist", as [`NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE`] words the same condition for
/// another resource of this service, rather than in the model's documentation prose ("The
/// specified public key doesn't exist."), and the wire wording is what a client reads. Do not
/// "correct" this constant to the documentation sentence.
pub const NO_SUCH_PUBLIC_KEY_MESSAGE: &str = "The specified CloudFront public key does not exist.";

/// The modelled `PublicKeyInUse` (HTTP 409) for a delete of a public key a stored record
/// still references.
///
/// The SAME rule [`cache_policy_in_use`] states for a policy a cache behaviour attaches: the
/// sentence is a property of the CONDITION rather than of the request — the key is still
/// referenced — so nothing from the request or from the store belongs in it, and every
/// refusal of this condition reads the same.
pub fn public_key_in_use() -> crate::ops::types::PublicKeyInUse {
    crate::ops::types::PublicKeyInUse {
        message: Some(PUBLIC_KEY_IN_USE_MESSAGE.to_string()),
    }
}

/// `PublicKeyInUse`'s message, as CloudFront sends it on the wire — observed in a recorded
/// CloudFront response to this refusal, which is the source this wording comes from: the key
/// is associated with either a key group or a field-level encryption profile and the caller is
/// told to disassociate it before deleting, worded as
/// [`FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE`] and [`CACHE_POLICY_IN_USE_MESSAGE`] word
/// the same condition for the resources they gate, rather than in the model's documentation
/// prose ("The specified public key is in use."). Do not "correct" this constant to the
/// documentation sentence.
pub const PUBLIC_KEY_IN_USE_MESSAGE: &str =
    "The Cloudfront public key is currently associated with either Key Group or a field level \
     encryption profile. Please disassociate the key before deleting.";

/// The longest comment a field-level encryption profile carries.
///
/// `FieldLevelEncryptionProfileConfig$Comment`: "An optional comment for the field-level
/// encryption profile. The comment cannot be longer than 128 characters." The SAME bound,
/// stated in the same words, as [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that
/// constant rather than a second literal, and a comment past it is refused with the one
/// [`comment_too_long`] sentence.
pub const MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchFieldLevelEncryptionProfile` (HTTP 404) for a profile id the store
/// does not hold.
///
/// ONE sentence for the whole condition, whatever body the id takes:
/// `GetFieldLevelEncryptionProfile` and its siblings model this one not-found code and no
/// argument error
/// (`smithy_query(query="operation",operation="GetFieldLevelEncryptionProfile")` reports
/// `errors: [AccessDenied, NoSuchFieldLevelEncryptionProfile]`), and the recorded answers of
/// this surface report every id written in the form a profile identifier is MINTED in that the
/// account holds no profile under as the profile that is absent. A profile id borne
/// by a submitted DOCUMENT addresses nothing, so it is resolved against the store alone and
/// reports the absent profile whatever shape it takes; an id that ADDRESSES one is read for
/// its form first ([`unresolved_field_level_encryption_profile`]). The same rule the
/// public key reads keep ([`no_such_public_key`]) and the one
/// [`no_such_field_level_encryption_config`] keeps for the other field-level encryption
/// resource.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// profile — so nothing from the request belongs in the text and every path that raises this
/// code reads the same.
pub fn no_such_field_level_encryption_profile() -> crate::ops::types::NoSuchFieldLevelEncryptionProfile
{
    crate::ops::types::NoSuchFieldLevelEncryptionProfile {
        message: Some(NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE.to_string()),
    }
}

/// `NoSuchFieldLevelEncryptionProfile`'s message, as CloudFront sends it on the wire —
/// observed in a recorded CloudFront response to this refusal, which is the source this
/// wording comes from. The service words it "field level encryption profile does not exist",
/// as [`NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE`] words the same condition for the
/// other field-level encryption resource, rather than in the model's documentation prose
/// ("The specified profile for field-level encryption doesn't exist."), and the wire wording
/// is what a client reads. Do not "correct" this constant to the documentation sentence.
pub const NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE: &str =
    "The specified field level encryption profile does not exist.";

/// The modelled `NoSuchFieldLevelEncryptionProfile` (HTTP 404) an operation ADDRESSED by a
/// field-level encryption profile `Id` in its path answers an `Id` it cannot resolve.
///
/// These operations model this one not-found code and no argument error, so one code and one
/// status cover two conditions and the MESSAGE is what tells them apart, decided from the value
/// alone and therefore before the store is consulted. What the value has to say for the format
/// refusal to be the answer is that it could never be an identifier CloudFront issued for a
/// PROFILE ([`id_format_refused_for`]): it is written outside the alphabet an identifier is
/// written in at all, it is written wider than an identifier of this mint is issued, or it is
/// written exactly as an identifier is ISSUED under another kind's namespace letter, in which
/// case it is that other kind's identifier. Either way the route
/// reports the identifier it cannot READ ([`id_format_invalid_message`]). Every other value is
/// carried through to the store — WHICH identifier of this kind was ever issued is a property of
/// the STORE rather than of the value, and so is a value of the alphabet written narrower than
/// the issued width —
/// and the profile the store holds none under is the profile that is simply ABSENT.
///
/// Only an identifier that ADDRESSES the operation is judged this way: a profile id borne by a
/// submitted CONFIGURATION (a query-arg or content-type mapping's `ProfileId`) addresses
/// nothing, so it is resolved against the store alone and reports the absent profile whatever
/// its shape ([`no_such_field_level_encryption_profile`]).
pub fn unresolved_field_level_encryption_profile(
    id: &str,
) -> crate::ops::types::NoSuchFieldLevelEncryptionProfile {
    if id_format_refused_for(FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX, id) {
        crate::ops::types::NoSuchFieldLevelEncryptionProfile {
            message: Some(id_format_invalid_message(ADDRESSED_BY_ID)),
        }
    } else {
        no_such_field_level_encryption_profile()
    }
}

/// The modelled `FieldLevelEncryptionProfileInUse` (HTTP 409) for a delete of a profile a
/// stored field-level encryption configuration still names.
///
/// The SAME rule [`field_level_encryption_config_in_use`] states for the configuration a
/// cache behaviour names: the sentence is a property of the CONDITION rather than of the
/// request — the profile is still referenced — so nothing from the request or from the store
/// belongs in it, and every refusal of this condition reads the same.
pub fn field_level_encryption_profile_in_use(
) -> crate::ops::types::FieldLevelEncryptionProfileInUse {
    crate::ops::types::FieldLevelEncryptionProfileInUse {
        message: Some(FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE.to_string()),
    }
}

/// `FieldLevelEncryptionProfileInUse`'s message, as CloudFront sends it on the wire —
/// observed in a recorded CloudFront response to this refusal, which is the source this
/// wording comes from: the profile is associated with a field-level encryption
/// configuration and the caller is told to disassociate it before deleting, worded as
/// [`FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE`] and [`CACHE_POLICY_IN_USE_MESSAGE`] word
/// the same condition for the resources they gate, rather than in the model's documentation
/// prose ("The specified profile for field-level encryption is in use."). Do not "correct"
/// this constant to the documentation sentence.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE: &str =
    "The field level encryption profile is currently associated with a field level encryption \
     config. Please disassociate the profile before deleting.";

/// The sentence `FieldLevelEncryptionProfileAlreadyExists` carries when the CALLER REFERENCE
/// a create submits is one another profile already holds, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape uses
/// ("The specified profile for field-level encryption already exists."): that one documents
/// the condition, this one states what the condition IS — the caller reference the request
/// carried already names a profile of this account.
///
/// The WHOLE sentence is fixed, for the reason
/// [`FIELD_LEVEL_ENCRYPTION_PROFILE_NAME_ALREADY_EXISTS_MESSAGE`] is: the conflict is over the
/// reference the REQUEST submitted, so the caller already holds it, and the identifier of the
/// profile that holds it is no part of the text.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference that you are using to create a field level encryption profile is \
     associated with another field level encryption profile in your account.";

/// The sentence `FieldLevelEncryptionProfileAlreadyExists` carries when it is the profile
/// NAME that is already taken, on the wire.
///
/// A profile is unique by its name as well as by its caller reference, and the two
/// conditions share the error code — so the sentence is what tells a caller which of its
/// values it has to change, as [`PUBLIC_KEY_NAME_ALREADY_EXISTS_MESSAGE`] does for the
/// neighbouring resource. Fixed whole: the caller already holds the name, and the
/// identifier of the profile that holds it is no part of the text.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The name that you are using to create a field level encryption profile is associated \
     with another field level encryption profile in your account.";

/// The modelled `TooManyFieldLevelEncryptionProfiles` (HTTP 400) for a create that would
/// take the store past [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`].
///
/// The sentence is the one the model states on the shape itself — "The maximum number of
/// profiles for field-level encryption have been created." — a property of the account's
/// stored profiles rather than of the request, so nothing from the request belongs in the
/// text; it lives here beside every other modelled error text of this service rather than in
/// the handler.
pub fn too_many_field_level_encryption_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionProfiles`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_PROFILES_MESSAGE: &str =
    "The maximum number of profiles for field-level encryption have been created.";

/// The longest comment a field-level encryption configuration carries.
///
/// `FieldLevelEncryptionConfig$Comment`: "An optional comment about the configuration. The
/// comment cannot be longer than 128 characters." The SAME bound, stated in the same words,
/// as [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchFieldLevelEncryptionConfig` (HTTP 404) for a configuration id the
/// store does not hold.
///
/// ONE sentence for the whole condition, whatever body the id carries — the SAME rule
/// [`no_such_field_level_encryption_profile`] keeps for the other field-level encryption
/// resource: these operations model this one not-found code and no argument error
/// (`smithy_query(query="operation",operation="GetFieldLevelEncryptionConfig")` reports
/// `errors: [AccessDenied, NoSuchFieldLevelEncryptionConfig]`), and each route addressed by a
/// configuration id reports every id of the form IT reads that the account holds no
/// configuration under as the configuration that is absent
/// ([`unresolved_field_level_encryption_config`]) — the configuration's own RESOURCE read
/// (`GetFieldLevelEncryption`) reading no form at all, so it answers this sentence for every id
/// it cannot resolve.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// configuration — so nothing from the request belongs in the text and every refusal of this
/// condition reads the same.
pub fn no_such_field_level_encryption_config() -> crate::ops::types::NoSuchFieldLevelEncryptionConfig
{
    crate::ops::types::NoSuchFieldLevelEncryptionConfig {
        message: Some(NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE.to_string()),
    }
}

/// `NoSuchFieldLevelEncryptionConfig`'s message, as CloudFront sends it on the wire — observed
/// in a recorded CloudFront response to this refusal, which is the source this wording comes
/// from. The service words it "config does not exist" rather than in the model's documentation
/// prose ("The specified configuration for field-level encryption doesn't exist."), and the
/// wire wording is what a client reads.
pub const NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE: &str =
    "The specified field level encryption config does not exist.";

/// The modelled `NoSuchFieldLevelEncryptionConfig` (HTTP 404) the operations addressed by a
/// field-level encryption configuration `Id` that read or write the CONFIGURATION DOCUMENT —
/// its `/config` read, its update and its delete — answer an `Id` they cannot resolve: the
/// split [`unresolved_field_level_encryption_profile`] makes for the other field-level
/// encryption record, over the alphabet an identifier is written in, the width one is issued at
/// and the namespace letter an identifier written as an ISSUED one carries
/// ([`id_format_refused_for`]).
///
/// The RESOURCE read of the same kind (`GetFieldLevelEncryption`, `GET
/// /2020-05-31/field-level-encryption/{Id}`) reads NO form of the value at all, as its own
/// recorded answers report: it carries every `Id` through to the store and answers
/// [`no_such_field_level_encryption_config`] for the one it cannot resolve. How much of an
/// identifier's form a route reads is each route's recorded contract, so do not fold the two
/// into one.
pub fn unresolved_field_level_encryption_config(
    id: &str,
) -> crate::ops::types::NoSuchFieldLevelEncryptionConfig {
    if id_format_refused_for(FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX, id) {
        crate::ops::types::NoSuchFieldLevelEncryptionConfig {
            message: Some(id_format_invalid_message(ADDRESSED_BY_ID)),
        }
    } else {
        no_such_field_level_encryption_config()
    }
}

/// The modelled `FieldLevelEncryptionConfigInUse` (HTTP 409) for a delete of a configuration
/// a cache behaviour still names.
///
/// The SAME rule [`cache_policy_in_use`] states for a policy a cache behaviour attaches: the
/// sentence is a property of the CONDITION rather than of the request — the configuration is
/// associated with a cache behaviour, and the caller is told to disassociate it first — so
/// nothing from the request or from the store belongs in it, and every refusal of this
/// condition reads the same.
pub fn field_level_encryption_config_in_use(
) -> crate::ops::types::FieldLevelEncryptionConfigInUse {
    crate::ops::types::FieldLevelEncryptionConfigInUse {
        message: Some(FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE.to_string()),
    }
}

/// `FieldLevelEncryptionConfigInUse`'s message, as CloudFront sends it on the wire — observed
/// in a recorded CloudFront response to this refusal, which is the source this wording comes
/// from: the configuration is associated with a cache behaviour and the caller is told to
/// disassociate it before deleting, worded as [`CACHE_POLICY_IN_USE_MESSAGE`] words the same
/// condition for a policy a cache behaviour attaches, rather than in the model's documentation
/// prose ("The specified configuration for field-level encryption is in use.").
pub const FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE: &str =
    "The field level encryption config is currently associated with a cache behavior. Please \
     disassociate the config before deleting.";

/// The sentence `FieldLevelEncryptionConfigAlreadyExists` carries when the CALLER REFERENCE a
/// create submits is one another configuration already holds, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape uses
/// ("The specified configuration for field-level encryption already exists."): that one
/// documents the condition, this one states what the condition IS — the caller reference the
/// request carried already names a configuration of this account. The service words it
/// "field level encryption config", as [`NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE`] does.
///
/// The WHOLE sentence is fixed, as
/// [`FIELD_LEVEL_ENCRYPTION_PROFILE_ALREADY_EXISTS_MESSAGE`] is for the neighbouring resource:
/// the conflict is over the reference the REQUEST submitted, so the caller already holds it,
/// and the identifier of the configuration that holds it is no part of the text.
pub const FIELD_LEVEL_ENCRYPTION_CONFIG_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference that you are using to create a field level encryption config is \
     associated with another field level encryption config in your account.";

/// The modelled `TooManyFieldLevelEncryptionConfigs` (HTTP 400) for a create that would take
/// the store past [`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`].
///
/// The sentence is the one the model states on the shape itself — "The maximum number of
/// configurations for field-level encryption have been created." — a property of the
/// account's stored configurations rather than of the request, so nothing from the request
/// belongs in the text.
pub fn too_many_field_level_encryption_configs(
) -> crate::ops::types::TooManyFieldLevelEncryptionConfigs {
    crate::ops::types::TooManyFieldLevelEncryptionConfigs {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONFIGS_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionConfigs`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONFIGS_MESSAGE: &str =
    "The maximum number of configurations for field-level encryption have been created.";

/// The modelled `TooManyFieldLevelEncryptionQueryArgProfiles` (HTTP 400) for a configuration
/// naming more query argument-profile mappings than
/// [`MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES`].
///
/// The sentence is the one the model states on the shape itself, a property of the bound
/// rather than of the request.
pub fn too_many_field_level_encryption_query_arg_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionQueryArgProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionQueryArgProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionQueryArgProfiles`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES_MESSAGE: &str =
    "The maximum number of query arg profiles for field-level encryption have been created.";

/// The modelled `TooManyFieldLevelEncryptionContentTypeProfiles` (HTTP 400) for a
/// configuration naming more content type-profile mappings than
/// [`MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES`].
///
/// The sentence is the one the model states on the shape itself, a property of the bound
/// rather than of the request.
pub fn too_many_field_level_encryption_content_type_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionContentTypeProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionContentTypeProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionContentTypeProfiles`'s message, as the model documents the
/// shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES_MESSAGE: &str =
    "The maximum number of content type profiles for field-level encryption have been created.";

/// The modelled `QueryArgProfileEmpty` (HTTP 400) for a field-level encryption
/// configuration that maps no query argument to a profile it could encrypt with.
///
/// The sentence is fixed service text ([`QUERY_ARG_PROFILE_EMPTY_MESSAGE`]), stating the
/// RULE the configuration broke, so nothing from the request belongs in it.
pub fn query_arg_profile_empty() -> crate::ops::types::QueryArgProfileEmpty {
    crate::ops::types::QueryArgProfileEmpty {
        message: Some(QUERY_ARG_PROFILE_EMPTY_MESSAGE.to_string()),
    }
}

/// `QueryArgProfileEmpty`'s message, on the wire.
///
/// The WIRE text — what the recorded service answers this code with — which differs from
/// the sentence the MODEL's documentation of the shape uses for the same code ("No profile
/// specified for the field-level encryption query argument."): that one documents the
/// condition, this one states the rule and is what is served. Do not "correct" this
/// constant to the documentation sentence.
pub const QUERY_ARG_PROFILE_EMPTY_MESSAGE: &str =
    "The query arg profile in field level encryption config cannot be empty when content \
     type profile is empty or when no default profile id is given in any content type \
     profile.";

/// Whether a field-level encryption configuration leaves CloudFront no profile to encrypt a
/// request's fields under.
///
/// A configuration encrypts matching fields under the profile a `QueryArgProfile` maps the
/// request's query argument to, and falls back to the profile a `ContentTypeProfile` names
/// for the request's content type; `ContentTypeProfile$ProfileId` is optional, so a content
/// type mapping may name none. So the query argument mappings have to name a profile unless
/// some content type mapping does: a configuration where neither does encrypts nothing, and
/// so does one whose query argument mapping states an empty profile id — the same condition
/// seen from the mapping's side, and the one `QueryArgProfileEmpty` refuses.
pub fn query_arg_profile_missing(config: &FieldLevelEncryptionConfig) -> bool {
    let query_arg_profiles: &[QueryArgProfile] = config
        .query_arg_profile_config
        .as_ref()
        .and_then(|query_arg| query_arg.query_arg_profiles.as_ref())
        .and_then(|profiles| profiles.items.as_deref())
        .unwrap_or_default();
    if query_arg_profiles
        .iter()
        .any(|profile| profile.profile_id.trim().is_empty())
    {
        return true;
    }
    if !query_arg_profiles.is_empty() {
        return false;
    }
    let content_type_profiles: &[ContentTypeProfile] = config
        .content_type_profile_config
        .as_ref()
        .and_then(|content_type| content_type.content_type_profiles.as_ref())
        .and_then(|profiles| profiles.items.as_deref())
        .unwrap_or_default();
    !content_type_profiles.iter().any(|profile| {
        profile
            .profile_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
    })
}

/// The modelled `IllegalUpdate` (HTTP 400) for an update to a field-level encryption
/// configuration that leaves out the `QueryArgProfileConfig` the service requires on every
/// update of one.
///
/// FIXED service text ([`QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE`]), naming the member
/// that has to be there and nothing from the request.
pub fn illegal_update_query_arg_profile_config_required() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses an update of a field-level encryption configuration that
/// states no `QueryArgProfileConfig` with: an update replaces the configuration WHOLE, so
/// the member cannot be dropped by a payload that omits it.
pub const QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE: &str =
    "QueryArgProfileConfig is required for updates";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListPublicKeys` `Marker` that is no
/// resume point this service handed out.
///
/// ONE CONSTANT for the condition rather than a sentence built from the request: what the
/// refusal states is the CONSTRAINT on the parameter — a marker is a public key id — which
/// is a property of the parameter and not of the value submitted, so nothing from the
/// request belongs in the text. The value below is the WIRE text, what the recorded
/// service answers this code with, exactly as
/// [`MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE`] is; do not reword it into a sentence
/// about the submitted value.
pub fn invalid_public_key_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_PUBLIC_KEY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListPublicKeys` refuses an unusable `Marker` with, on the wire.
pub const INVALID_PUBLIC_KEY_MARKER_MESSAGE: &str = "Marker must be a valid CF public key Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListFieldLevelEncryptionConfigs` `Marker`
/// that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is a field-level
/// encryption configuration id — which is a property of the parameter rather than of the
/// value submitted, so nothing from the request belongs in the text.
pub fn invalid_field_level_encryption_config_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListFieldLevelEncryptionConfigs` refuses an unusable `Marker` with: the
/// constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the
/// same constraint on the other listing's marker, naming this listing's own resource.
pub const INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE: &str =
    "Marker must be a valid CF field-level encryption config Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListFieldLevelEncryptionProfiles`
/// `Marker` that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is a field-level
/// encryption profile id — which is a property of the parameter rather than of the value
/// submitted, so nothing from the request belongs in the text.
pub fn invalid_field_level_encryption_profile_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListFieldLevelEncryptionProfiles` refuses an unusable `Marker` with: the
/// constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the
/// same constraint on the other listing's marker, naming this listing's own resource.
pub const INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE: &str =
    "Marker must be a valid CF field-level encryption profile Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListCloudFrontOriginAccessIdentities`
/// `Marker` that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is an origin access
/// identity id — which is a property of the parameter rather than of the value submitted, so
/// nothing from the request belongs in the text.
pub fn invalid_origin_access_identity_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListCloudFrontOriginAccessIdentities` refuses an unusable `Marker` with:
/// the constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words
/// the same constraint on the other listing's marker.
pub const INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE: &str =
    "Marker must be a valid CF origin access identity Id";

/// The modelled `TooManyPublicKeys` (HTTP 400) for a create that would take the store past
/// [`MAX_PUBLIC_KEYS`] — "The maximum number of public keys for field-level encryption have
/// been created. To create a new public key, delete one of the existing keys."
///
/// The sentence states the CONSTRAINT and the number this emulator holds to, which is the
/// rule the refusal is built from rather than text echoed out of the request; it lives here
/// beside every other modelled error text of this service rather than in the handler.
pub fn too_many_public_keys() -> crate::ops::types::TooManyPublicKeys {
    crate::ops::types::TooManyPublicKeys {
        message: Some(format!(
            "The maximum number of public keys have been created ({MAX_PUBLIC_KEYS}). To \
             create a new public key, delete one of the existing keys."
        )),
    }
}

/// The sentence `PublicKeyAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape uses
/// ("The specified public key already exists."): that one documents the condition, this one
/// states what the condition IS — the caller reference the request carried already names a
/// key of this account. Do not "correct" this constant to the documentation sentence.
///
/// The WHOLE sentence is fixed: the conflict is over the reference the REQUEST submitted, so
/// the caller already holds it, and the identifier of the key that holds it is no part of the
/// text.
pub const PUBLIC_KEY_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference that you are using to create a CloudFront public key is associated \
     with another public key in your account.";

/// The sentence `PublicKeyAlreadyExists` carries when it is the NAME that is already taken,
/// on the wire.
///
/// The same sentence [`PUBLIC_KEY_ALREADY_EXISTS_MESSAGE`] states for the other value a key
/// is unique by, worded around the name instead: the two conditions share the error code, so
/// the sentence is what tells a caller which of its values it has to change. Fixed whole,
/// for the reason that one is.
pub const PUBLIC_KEY_NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The name that you are using to create a CloudFront public key is associated with \
     another public key in your account.";

/// The longest comment a key group carries.
///
/// `KeyGroupConfig$Comment`: "A comment to describe the key group. The comment cannot be
/// longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_KEY_GROUP_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `KeyGroupAlreadyExists` (HTTP 409) for a key group name that is already
/// taken.
///
/// The sentence is fixed service text ([`KEY_GROUP_ALREADY_EXISTS_MESSAGE`]) and it names
/// no part of the request: what it reports is that the account already holds a group of
/// that name, which is a property of the store rather than of the value submitted.
pub fn key_group_already_exists() -> crate::ops::types::KeyGroupAlreadyExists {
    crate::ops::types::KeyGroupAlreadyExists {
        message: Some(KEY_GROUP_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// `KeyGroupAlreadyExists`'s message, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape
/// uses for the same rule ("A key group with this name already exists. You must provide a
/// unique name. To modify an existing key group, use UpdateKeyGroup."): that one documents
/// the condition and tells the caller what to do instead, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const KEY_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "Another key group with the same name already exists within the aws account.";

/// The modelled `NoSuchResource` (HTTP 404) for a key group id the store does not hold.
///
/// `NoSuchResource` is the one error shape every operation of the key group surface models
/// for a missing resource, and CloudFront words it for the resource that operation
/// ADDRESSES — a key group, here — rather than with the generic sentence the tagging
/// surface answers for an arbitrary ARN ([`no_such_resource`]). The sentence states a
/// property of the store, so nothing from the request belongs in it.
pub fn no_such_key_group() -> NoSuchResource {
    NoSuchResource {
        message: Some(NO_SUCH_KEY_GROUP_MESSAGE.to_string()),
    }
}

/// The key group surface's `NoSuchResource` message, in the form CloudFront answers a
/// missing named resource with — "The specified <resource> does not exist.", as
/// [`NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE`] and [`NO_SUCH_CACHE_POLICY_MESSAGE`] carry it.
pub const NO_SUCH_KEY_GROUP_MESSAGE: &str = "The specified key group does not exist.";

/// The modelled `ResourceInUse` (HTTP 409) for a delete of a key group a cache behaviour
/// still references.
///
/// `ResourceInUse` is the one conflict shape the key group surface models, and CloudFront
/// words a refusal of this surface for the resource the operation ADDRESSES — a key group,
/// here — the way [`no_such_key_group`] words the absent-resource one. The sentence states
/// the CONDITION, so nothing from the request or from the store belongs in it — not the
/// group, and not the distribution that holds the reference — and every refusal of this
/// condition reads the same, as [`CACHE_POLICY_IN_USE_MESSAGE`] does for the other
/// configuration objects a cache behaviour names.
pub fn key_group_in_use() -> crate::ops::types::ResourceInUse {
    crate::ops::types::ResourceInUse {
        message: Some(KEY_GROUP_IN_USE_MESSAGE.to_string()),
    }
}

/// `ResourceInUse`'s message on the key group surface: what CloudFront answers a delete of a
/// key group a cache behaviour still references, worded as [`CACHE_POLICY_IN_USE_MESSAGE`]
/// words the same condition for the other configuration objects a cache behaviour names.
pub const KEY_GROUP_IN_USE_MESSAGE: &str =
    "The specified key group is currently associated with a cache behavior. Please \
     disassociate the key group before deleting.";

/// The modelled `TooManyKeyGroups` (HTTP 400) for a create that would take the store past
/// [`MAX_KEY_GROUPS`].
///
/// The sentence is the one the model states on the shape itself — a property of the
/// account's stored key groups rather than of the request, so nothing from the request
/// belongs in the text; it lives here beside every other modelled error text of this
/// service rather than in the handler.
pub fn too_many_key_groups() -> crate::ops::types::TooManyKeyGroups {
    crate::ops::types::TooManyKeyGroups {
        message: Some(TOO_MANY_KEY_GROUPS_MESSAGE.to_string()),
    }
}

/// `TooManyKeyGroups`'s message, as the model documents the shape.
pub const TOO_MANY_KEY_GROUPS_MESSAGE: &str =
    "You have reached the maximum number of key groups for this Amazon Web Services \
     account. For more information, see Quotas (formerly known as limits) in the Amazon \
     CloudFront Developer Guide.";

/// The modelled `TooManyPublicKeysInKeyGroup` (HTTP 400) for a configuration that names
/// more public keys than [`MAX_PUBLIC_KEYS_IN_KEY_GROUP`].
///
/// The sentence is the one the model states on the shape itself: what it reports is the
/// published quota on one group's key list, which is a property of the parameter rather
/// than of the value submitted.
pub fn too_many_public_keys_in_key_group() -> crate::ops::types::TooManyPublicKeysInKeyGroup {
    crate::ops::types::TooManyPublicKeysInKeyGroup {
        message: Some(TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE.to_string()),
    }
}

/// `TooManyPublicKeysInKeyGroup`'s message, as the model documents the shape.
pub const TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE: &str =
    "The number of public keys in this key group is more than the maximum allowed. For \
     more information, see Quotas (formerly known as limits) in the Amazon CloudFront \
     Developer Guide.";

/// The modelled `InvalidArgument` (HTTP 400) for a key group configuration that names no
/// public key at all.
///
/// `CreateKeyGroup` documents the requirement — "To create a key group, you must specify
/// at least one public key for the key group" — and the model's `PublicKeyIdList` carries
/// no length constraint, so the bound is not refused before a handler runs and the key group
/// path checks it itself. CloudFront reports the condition against the PARAMETER the list
/// arrived in ([`invalid_parameter_message`]), named as the wire reports that member
/// ([`PUBLIC_KEY_IDS_LIST_PARAMETER`]): recorded behaviour, and the form this service's other
/// refusals of an unusable parameter already take. Nothing of the request or of the store is
/// echoed — the parameter is what the caller has to fix — and the sentence is about the
/// CONFIGURATION rather than about the call carrying it, so the create and the update read
/// alike.
pub fn key_group_without_a_public_key() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(key_group_without_a_public_key_message()),
    }
}

/// The name the WIRE reports `KeyGroupConfig$Items` under, which is the name CloudFront's
/// refusal of that member carries.
pub const PUBLIC_KEY_IDS_LIST_PARAMETER: &str = "publicKeyIdsList";

/// The message a key group write naming no public key is refused with.
pub fn key_group_without_a_public_key_message() -> String {
    invalid_parameter_message(PUBLIC_KEY_IDS_LIST_PARAMETER)
}

/// The sentence CloudFront refuses a PARAMETER it cannot use the value of with: fixed text
/// around the name of the member, and nothing of the value it carried.
///
/// ONE template for every such refusal this service serves, so no two of them can word the
/// same condition differently.
pub fn invalid_parameter_message(parameter: &str) -> String {
    format!("The parameter {parameter} is invalid.")
}

/// The sentence CloudFront refuses a PARAMETER whose value is outside the range it admits with:
/// fixed text around the name of the member, and nothing of the value it carried or of the range
/// itself.
///
/// The companion of [`invalid_parameter_message`] for a value CloudFront can read and will not
/// take — a count or an amount of time outside the domain of the member — stated once for every
/// member refused that way. It is the sentence this service already states for a port no origin
/// can be reached on ([`ORIGIN_PORT_OUT_OF_RANGE_MESSAGE`]), which names the kind of parameter
/// rather than the member that carried it.
pub fn parameter_out_of_range_message(parameter: &str) -> String {
    format!("The parameter {parameter} is not within allowed range.")
}

/// The sentence CloudFront refuses a PARAMETER whose value is longer than it admits with:
/// fixed text around the name of the member, and nothing of the value or of the bound.
///
/// The companion of [`invalid_parameter_message`] for the one condition CloudFront words
/// differently — a value that is too big rather than one it cannot read — stated once for
/// every member refused that way.
pub fn parameter_too_big_message(parameter: &str) -> String {
    format!("The parameter {parameter} is too big.")
}

/// The longest name a key group carries.
///
/// `KeyGroupConfig$Name` is "a name to identify the key group" and carries no `@length` in the
/// model, so the bound is not refused before a handler runs and the key group path checks it
/// itself. Recorded behaviour on both sides of it: CloudFront refuses a longer name as the
/// parameter it arrived in ([`key_group_name_too_long`]), and the names a recorded create is
/// SERVED for are inside this bound — so the value held to is the 128 characters the key group
/// surface states for the other free-text member of the same configuration
/// ([`MAX_KEY_GROUP_COMMENT`]), one bound for the configuration's text rather than a second
/// literal. A longer limit would admit a name the recording has the service refuse; a shorter
/// one would refuse a name it has the service store.
pub const MAX_KEY_GROUP_NAME: usize = MAX_KEY_GROUP_COMMENT;

/// The name the WIRE reports `KeyGroupConfig$Name` under, which is the name CloudFront's
/// refusal of that member carries.
pub const KEY_GROUP_NAME_PARAMETER: &str = "KeyGroupName";

/// The modelled `InvalidArgument` (HTTP 400) for a key group name longer than
/// [`MAX_KEY_GROUP_NAME`].
///
/// Recorded behaviour: the whole sentence is fixed text naming the PARAMETER the name arrived
/// in ([`parameter_too_big_message`]) — neither the submitted name nor the bound is echoed, so
/// the refusal reads the same for every name past it.
pub fn key_group_name_too_long() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(key_group_name_too_long_message()),
    }
}

/// The message a key group write naming a name past [`MAX_KEY_GROUP_NAME`] is refused with.
pub fn key_group_name_too_long_message() -> String {
    parameter_too_big_message(KEY_GROUP_NAME_PARAMETER)
}

/// Whether `name` is a name a key group of this account can be identified by: a name built
/// from the characters CloudFront admits in one.
///
/// `KeyGroupConfig$Name` is "a name to identify the key group" and carries no `@pattern` in the
/// model, so the alphabet is not enforced before a handler runs. Recorded behaviour on both
/// sides of it: CloudFront refuses a name carrying a character outside the alphabet
/// ([`key_group_name_illegal_characters`]), and every name a recorded create or update is SERVED
/// for is written from ASCII letters, digits, `-` and `_` — which is also the alphabet this
/// service's model states for the other name it constrains, `KeyValueStoreName`'s
/// `^[a-zA-Z0-9-_]{1,64}$`, read through [`is_key_value_store_name`], so one alphabet serves
/// both rather than a second reading of its own.
///
/// The rule is about the CHARACTERS a name carries, so the empty name is not refused here: the
/// member is `@required`, which is what refuses a name that is absent, and this service has no
/// recorded refusal of an empty one to word — a name no group is identified by is left to the
/// uniqueness rule, which the group holding it then owns.
pub fn is_key_group_name(name: &str) -> bool {
    name.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The name the WIRE reports the offending member under when a key group name carries a
/// character CloudFront does not admit: the member's own name, as the recorded refusal of that
/// condition carries it.
pub const KEY_GROUP_NAME_MEMBER: &str = "Name";

/// The modelled `InvalidArgument` (HTTP 400) for a key group name carrying a character
/// CloudFront does not admit in one ([`is_key_group_name`]).
///
/// Recorded behaviour: the whole sentence is fixed text naming the member the value arrived in
/// and nothing of the value itself, so the refusal reads the same for every such name.
pub fn key_group_name_illegal_characters() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(key_group_name_illegal_characters_message()),
    }
}

/// The message a key group write naming a name outside [`is_key_group_name`] is refused with.
pub fn key_group_name_illegal_characters_message() -> String {
    format!("The parameter {KEY_GROUP_NAME_MEMBER} contains illegal characters.")
}

/// The modelled `InvalidArgument` (HTTP 400) for a key group configuration whose `Items`
/// name a public key this account holds none of.
///
/// `KeyGroupConfig$Items` is "a list of the identifiers of the public keys in the key group",
/// and a key group is what makes those keys the signers a cache behaviour trusts — so an
/// entry resolving to nothing names no signer at all, and CloudFront treats such a
/// configuration as unusable rather than storing a reference to a key that is not there. The
/// sentence states the CONDITION — the configuration names a public key that does not exist
/// — so nothing from the request or from the store belongs in it, as
/// [`key_group_without_a_public_key_message`] carries nothing of the value for the other rule
/// about the same member.
pub fn absent_public_key_in_key_group() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(ABSENT_PUBLIC_KEY_IN_KEY_GROUP_MESSAGE.to_string()),
    }
}

/// The message a key group write naming an absent public key is refused with, as CloudFront
/// sends it on the wire — observed in a recorded CloudFront response to this refusal, which
/// is the source this wording comes from: the service names the resource "CloudFront public
/// key" and words the condition "does not exist", as [`NO_SUCH_PUBLIC_KEY_MESSAGE`] does for
/// a read of the key itself, and says where the unresolvable reference was found. Fixed text
/// throughout: it carries no identifier.
pub const ABSENT_PUBLIC_KEY_IN_KEY_GROUP_MESSAGE: &str =
    "The specified CloudFront public key does not exist in key group.";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListKeyGroups` `Marker` that is no
/// resume point this service handed out.
///
/// Recorded behaviour: this listing states the condition over the KIND of resource it pages
/// ([`invalid_resource_marker_message`]) — the family the policy listings of this service
/// already state, reached through that one renderer so no two listings word the same
/// condition differently — rather than the constraint the value has to satisfy, which is what
/// the listings of the resources whose marker the API publishes as an id state
/// ([`invalid_public_key_marker`]). Which family a listing states is a property of the
/// LISTING, and so is the sentence: fixed text throughout, carrying nothing of the marker
/// submitted and nothing out of the store, so every refusal of this condition reads the same.
pub fn invalid_key_group_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(invalid_key_group_marker_message()),
    }
}

/// The message `ListKeyGroups` refuses an unusable `Marker` with: the kind of resource this
/// listing pages ([`KEY_GROUP_MARKER_RESOURCE`]) in the sentence this service states that
/// condition in.
pub fn invalid_key_group_marker_message() -> String {
    invalid_resource_marker_message(KEY_GROUP_MARKER_RESOURCE)
}

/// Mint the request id ONE response names the request it answers with — the `RequestId`
/// element of this service's `<ErrorResponse>` envelope (`crate::service::envelope`).
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes and the same form
/// [`ComAmazonawsCloudfrontCloudfront20200531State::allocate_key_value_store_id`] renders
/// an id in. The VALUE is per-response and is not a requirement: nothing can predict it
/// and no caller may depend on a particular one. What it must do is name ONE response, so
/// it is drawn from a per-process counter run through [`scramble`] — the bijection every
/// token this service mints is drawn through, so two responses never carry the same id —
/// mixed with the instant the response was answered, so two runs of this emulator do not
/// mint the same sequence. It is an identifier for correlation, never a secret: nothing
/// authenticates with it.
///
/// A free function rather than a mint on the store: a failure the request boundary
/// refused never reaches a handler, so the envelope that answers it has no state to reach.
pub fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mix a counter so consecutive values render as unrelated tokens.
///
/// This is SplitMix64's finaliser applied to the counter times the 64-bit golden ratio.
/// Every step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the
/// whole function is one: distinct counters never collide.
pub(super) fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render `value` as `len` characters of the id alphabet, least significant first.
///
/// For `len >= 13` the rendering is injective on `u64` (36^13 > 2^64), which is what
/// makes a minted id or version token unique.
pub(super) fn render_token(value: u64, len: usize) -> String {
    let mut rest = value;
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let digit = (rest % 36) as usize;
        rest /= 36;
        out.push(char::from(TOKEN_ALPHABET[digit]));
    }
    out
}

/// Resolve every member of a submitted `DistributionConfig` that the SERVICE supplies.
///
/// A `DistributionConfig` member a request leaves out is not unset on the distribution: it
/// takes the value the service applies, and the distribution then runs under — and reports
/// — the fully resolved configuration. So the resolution happens once, when the
/// configuration is STORED (by `CreateDistribution` and by
/// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_config`]), and every later read,
/// list and read-modify-write round trip answers from the resolved record rather than from
/// the request that created it.
///
/// Three kinds of member are resolved, and nothing else:
///
/// * a LIST WRAPPER (`Aliases`, `CustomHeaders`, `OriginGroups`, `CacheBehaviors`,
///   `CustomErrorResponses`, `TrustedSigners`, `TrustedKeyGroups`,
///   `LambdaFunctionAssociations`, `FunctionAssociations`) — an absent wrapper means the
///   list is empty, which is the wrapper stating `Quantity` 0;
/// * a member with a DOCUMENTED default (the origin timeouts and attempts, the TTLs,
///   `PriceClass`, `HttpVersion`, `Logging`'s four `@default` members, the geo restriction
///   type, the allowed methods, the origin SSL protocols) — the documented value;
/// * a member whose absence has exactly one meaning (`DefaultRootObject`, `OriginPath`,
///   `OriginAccessControlId`, `WebACLId`, `FieldLevelEncryptionId` and
///   `ContinuousDeploymentPolicyId` name nothing — the empty string; `OriginShield`,
///   `GrpcConfig`, `SmoothStreaming`, `Compress` and `Staging` are off; `ViewerCertificate`
///   is CloudFront's own certificate).
///
/// A member that is a CHOICE the configuration has not made (a cache policy id, a
/// real-time log configuration, an anycast ip list, a tenant or connection-mode setting)
/// is left absent: the service applies nothing to it.
///
/// Which KIND of distribution the configuration describes decides which of them are its
/// members at all: a member documented as supported only on a standard distribution
/// (`Aliases`, `PriceClass`, `IsIPV6Enabled`, `ContinuousDeploymentPolicyId`, `Staging`, and
/// a cache behaviour's `TrustedSigners`, `SmoothStreaming` and legacy TTLs) is no member of a
/// multi-tenant one, so a multi-tenant configuration has none of them supplied — a member
/// the service refuses on the way in ([`multi_tenant_direct_only_member`]) is not one it
/// supplies a default for, which is what makes the configuration a read reports one a later
/// update admits back unchanged.
pub fn resolve_distribution_config(mut config: DistributionConfig) -> DistributionConfig {
    // `TenantConfig$ParameterDefinitions` is "the parameters that you specify for a
    // distribution tenant", and a list carrying no entry specifies none: it is retained as
    // ABSENT rather than as an empty element, the same way a listing that selects nothing
    // reports no `Items`. The `TenantConfig` element itself is the one the request stated,
    // so the configuration a read reports is the one the request described
    // ([`retained_tenant_config`]).
    config.tenant_config = config.tenant_config.map(retained_tenant_config);
    // Which KIND of distribution the configuration describes decides which members the
    // service supplies at all: a member documented as supported only on a standard
    // distribution is no member of a multi-tenant one, which states none of them
    // ([`multi_tenant_direct_only_member`] refuses one that does), so none of them is
    // materialised for it either.
    let multi_tenant = is_multi_tenant(&config);
    if !multi_tenant {
        config.aliases = Some(config.aliases.unwrap_or_default());
    }
    config.default_root_object = Some(config.default_root_object.unwrap_or_default());

    let origins = std::mem::take(&mut config.origins.items);
    config.origins.items = origins.into_iter().map(resolve_origin).collect();
    config.origin_groups = Some(config.origin_groups.unwrap_or_default());

    config.default_cache_behavior = resolve_default_cache_behavior(
        std::mem::take(&mut config.default_cache_behavior),
        multi_tenant,
    );
    let mut behaviors = config.cache_behaviors.take().unwrap_or_default();
    if let Some(items) = behaviors.items.take() {
        behaviors.items = Some(
            items
                .into_iter()
                .map(|behavior| resolve_cache_behavior(behavior, multi_tenant))
                .collect(),
        );
    }
    config.cache_behaviors = Some(behaviors);

    let mut responses = config.custom_error_responses.take().unwrap_or_default();
    if let Some(items) = responses.items.take() {
        responses.items = Some(
            items
                .into_iter()
                .map(resolve_custom_error_response)
                .collect(),
        );
    }
    config.custom_error_responses = Some(responses);

    config.logging = Some(resolve_logging(config.logging.take()));
    if !multi_tenant {
        config.price_class = Some(
            config
                .price_class
                .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
        );
    }
    config.viewer_certificate = Some(resolve_viewer_certificate(
        config.viewer_certificate.take(),
        !multi_tenant,
    ));
    config.restrictions = Some(config.restrictions.unwrap_or_else(|| Restrictions {
        geo_restriction: GeoRestriction {
            restriction_type: GEO_RESTRICTION_NONE.to_string(),
            quantity: 0,
            items: None,
        },
    }));
    config.web_acl_id = Some(config.web_acl_id.unwrap_or_default());
    config.http_version = Some(
        config
            .http_version
            .unwrap_or_else(|| DEFAULT_HTTP_VERSION.to_string()),
    );
    if !multi_tenant {
        config.is_ipv6_enabled = Some(config.is_ipv6_enabled.unwrap_or(DEFAULT_IS_IPV6_ENABLED));
        config.continuous_deployment_policy_id =
            Some(config.continuous_deployment_policy_id.unwrap_or_default());
        config.staging = Some(config.staging.unwrap_or(DEFAULT_STAGING));
    }
    config
}

/// Resolve the members the service supplies on ONE origin.
pub(super) fn resolve_origin(mut origin: Origin) -> Origin {
    origin.origin_path = Some(origin.origin_path.unwrap_or_default());
    origin.custom_headers = Some(origin.custom_headers.unwrap_or_default());
    origin.connection_attempts = Some(
        origin
            .connection_attempts
            .unwrap_or(DEFAULT_CONNECTION_ATTEMPTS),
    );
    origin.connection_timeout = Some(
        origin
            .connection_timeout
            .unwrap_or(DEFAULT_CONNECTION_TIMEOUT),
    );
    // "CloudFront Origin Shield" is off unless the configuration turns it on, and an
    // origin shield that is off names no region.
    origin.origin_shield = Some(origin.origin_shield.unwrap_or_default());
    origin.origin_access_control_id = Some(origin.origin_access_control_id.unwrap_or_default());
    origin.custom_origin_config = origin
        .custom_origin_config
        .map(resolve_custom_origin_config);
    origin.s3_origin_config = origin.s3_origin_config.map(resolve_s3_origin_config);
    origin
}

/// Resolve the members the service supplies on an S3 origin's configuration.
///
/// `S3OriginConfig$OriginReadTimeout` is "how long, in seconds, CloudFront waits for a
/// response from the origin ... the default (if you don't specify otherwise) is 30
/// seconds", the same response timeout a custom origin carries
/// ([`DEFAULT_ORIGIN_READ_TIMEOUT`]) — so an S3 origin that names none runs under that
/// value and reports it, exactly as [`resolve_custom_origin_config`] does for the member of
/// the same name. `OriginAccessIdentity` is required, so nothing is supplied for it.
pub(super) fn resolve_s3_origin_config(mut s3: S3OriginConfig) -> S3OriginConfig {
    s3.origin_read_timeout = Some(s3.origin_read_timeout.unwrap_or(DEFAULT_ORIGIN_READ_TIMEOUT));
    s3
}

/// Resolve every member of a submitted `CachePolicyConfig` that the SERVICE supplies.
///
/// The same rule [`resolve_distribution_config`] states, over a cache policy: a member a
/// request leaves out is not unset on the policy — it takes the value CloudFront applies,
/// and the policy then runs under, and reports, the resolved configuration. So the
/// resolution happens once, when the configuration is STORED (by `CreateCachePolicy` and by
/// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_cache_policy_config`]), and the
/// create, every later read and every read-modify-write round trip answer from the resolved
/// record.
///
/// Three members are resolved, each one the documentation states a value for:
///
/// * `DefaultTTL` — "The default value for this field is 86400 seconds (one day). If the
///   value of `MinTTL` is more than 86400 seconds, then the default value for this field is
///   the same as the value of `MinTTL`" ([`DEFAULT_DEFAULT_TTL`] is that one day);
/// * `MaxTTL` — "The default value for this field is 31536000 seconds (one year). If the
///   value of `MinTTL` or `DefaultTTL` is more than 31536000 seconds, then the default value
///   for this field is the same as the value of `DefaultTTL`" ([`DEFAULT_MAX_TTL`] is that
///   one year), compared against the `DefaultTTL` resolved above, which is never below
///   `MinTTL`;
/// * `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli` — the flag's off
///   position, which is what a cache key block naming only the gzip flag describes: "If both
///   of these fields are `false`, then CloudFront treats the `Accept-Encoding` header the
///   same as any other HTTP header in the viewer request."
///
/// Nothing else is: `Comment` is free text the policy was created with, and the cache key
/// block as a whole is a CHOICE the configuration either made or did not, so a policy that
/// states no cache key parameters is stored with none rather than with a cache key invented
/// here.
///
/// The TTL ORDERING a configuration is admitted under is checked over the members the
/// request SUPPLIES, before this resolution and for the reason
/// [`cache_policy_config_invalid`] states.
pub fn resolve_cache_policy_config(mut config: CachePolicyConfig) -> CachePolicyConfig {
    let default_ttl = config
        .default_ttl
        .unwrap_or_else(|| config.min_ttl.max(DEFAULT_DEFAULT_TTL));
    config.default_ttl = Some(default_ttl);
    config.max_ttl = Some(
        config
            .max_ttl
            .unwrap_or_else(|| default_ttl.max(DEFAULT_MAX_TTL)),
    );
    if let Some(parameters) = config
        .parameters_in_cache_key_and_forwarded_to_origin
        .as_mut()
    {
        parameters.enable_accept_encoding_brotli = Some(
            parameters
                .enable_accept_encoding_brotli
                .unwrap_or(DEFAULT_ENABLE_ACCEPT_ENCODING_BROTLI),
        );
    }
    config
}

/// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli` for a cache key
/// block that names only the gzip flag: the flag's off position — "If both of these fields
/// are `false`, then CloudFront treats the `Accept-Encoding` header the same as any other
/// HTTP header in the viewer request. By default, it's not included in the cache key and
/// it's not included in origin requests."
pub const DEFAULT_ENABLE_ACCEPT_ENCODING_BROTLI: bool = false;

/// Resolve every member of a submitted `FieldLevelEncryptionConfig` that the SERVICE
/// supplies.
///
/// [`resolve_distribution_config`]'s rule, over a field-level encryption configuration:
///
/// * `Comment` is "An optional comment about the configuration" — free text the
///   configuration carries, which for one that states none is the empty text rather than no
///   member at all, so a round trip through `UpdateFieldLevelEncryptionConfig` does not turn
///   the member's presence into its absence;
/// * `QueryArgProfileConfig` is the mapping every stored configuration carries — an update
///   of one cannot leave it out ([`QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE`]) — so a
///   configuration created without one carries the mapping that maps nothing
///   ([`default_query_arg_profile_config`]);
/// * `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` is the SERVICE's where the
///   mapping maps anything at all ([`forwards_an_unknown_query_arg_profile`]): a
///   configuration that maps at least one query argument to a profile is stored forwarding
///   the requests whose profile it cannot resolve, whatever the request asked for. Recorded
///   behaviour.
/// * `QueryArgProfileConfig$QueryArgProfiles$Items` is the list that wrapper's `Quantity`
///   counts, carried by every mapping this service stores — empty where the wrapper maps
///   nothing ([`resolve_query_arg_profiles`]), so a read reports the empty list beside the
///   quantity rather than leaving the member out.
///
/// `ContentTypeProfileConfig` is left exactly as submitted: it is a CHOICE a configuration
/// either made or did not, and neither the model nor the update rule makes it a member of
/// every stored configuration — `ForwardWhenContentTypeIsUnknown` inside one it DID state is
/// stored as stated, since the rules above are the query-argument mapping's own. A
/// `QueryArgProfileConfig` the request DID state is otherwise stored as stated, down to
/// whether it states a `QueryArgProfiles` wrapper at all: what the service supplies for a
/// configuration that states no mapping is the whole mapping, and inside a wrapper the
/// request DID state only that wrapper's `Items` list
/// ([`resolve_query_arg_profiles`]).
pub fn resolve_field_level_encryption_config(
    mut config: FieldLevelEncryptionConfig,
) -> FieldLevelEncryptionConfig {
    config.comment = Some(config.comment.unwrap_or_default());
    config.query_arg_profile_config = Some(
        config
            .query_arg_profile_config
            .map(resolve_query_arg_profile_config)
            .unwrap_or_else(default_query_arg_profile_config),
    );
    config
}

/// Resolve the members of a submitted `QueryArgProfileConfig` the SERVICE supplies:
/// `ForwardWhenQueryArgProfileIsUnknown` where the mapping maps a query argument to a profile
/// ([`forwards_an_unknown_query_arg_profile`]), and the `Items` list of the wrapper the
/// section states ([`resolve_query_arg_profiles`]). Which query arguments the mapping maps is
/// stored as submitted.
fn resolve_query_arg_profile_config(mut config: QueryArgProfileConfig) -> QueryArgProfileConfig {
    config.forward_when_query_arg_profile_is_unknown =
        forwards_an_unknown_query_arg_profile(&config);
    config.query_arg_profiles = config.query_arg_profiles.map(resolve_query_arg_profiles);
    config
}

/// Resolve the `Items` list of a `QueryArgProfiles` wrapper a request states: the mapping
/// CloudFront stores carries its list, so a wrapper that carries no entries carries the EMPTY
/// list rather than no list member at all.
///
/// The wrapper is "a `Quantity` beside the `Items` the quantity counts", and the stored
/// mapping that maps nothing is the one this service supplies for a configuration stating no
/// mapping at all ([`empty_query_arg_profiles`]) — `Quantity` 0 beside an empty list, which is
/// the mapping CloudFront's own answers report for such a configuration. A request that states
/// the wrapper without its optional `Items` describes that same mapping, so it is stored as
/// that mapping and every later read reports the empty list beside the quantity rather than
/// turning a list the request stated as empty into a member that is absent.
fn resolve_query_arg_profiles(mut profiles: QueryArgProfiles) -> QueryArgProfiles {
    profiles.items = Some(profiles.items.unwrap_or_default());
    profiles
}

/// What `ForwardWhenQueryArgProfileIsUnknown` is STORED as for the mapping `config`
/// describes.
///
/// The flag is "Flag to set if you want a request to be forwarded to the origin even if the
/// profile specified by the field-level encryption query argument, fle-profile, is unknown",
/// and a mapping that maps at least one query argument to a profile is stored forwarding such
/// a request whatever the request submitted: the mapping names the profiles this
/// configuration encrypts for, so a request naming any OTHER profile is one the configuration
/// has nothing to do with rather than one it means to drop. Recorded behaviour. A mapping
/// that maps NOTHING carries the value the request stated — the configuration states nothing
/// about any profile, so there is no mapping for the flag to follow — and the mapping this
/// service supplies for a configuration that states none forwards too
/// ([`default_query_arg_profile_config`]).
pub fn forwards_an_unknown_query_arg_profile(config: &QueryArgProfileConfig) -> bool {
    config.forward_when_query_arg_profile_is_unknown
        || config
            .query_arg_profiles
            .as_ref()
            .is_some_and(|profiles| items_len(&profiles.items) > 0)
}

/// The `QueryArgProfiles` wrapper of a mapping that maps no query argument to a profile:
/// `Quantity` 0 beside no entries.
fn empty_query_arg_profiles() -> QueryArgProfiles {
    QueryArgProfiles {
        quantity: 0,
        items: Some(Vec::new()),
    }
}

/// The query argument-profile mapping a field-level encryption configuration that states
/// none carries: a mapping of no query argument to any profile, under which a request
/// naming a profile CloudFront cannot resolve is still forwarded to the origin.
///
/// `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` is REQUIRED by the model, so
/// the mapping cannot be half-present: a configuration that leaves the whole block out
/// still carries the flag, and the value it carries is the one that forwards — "Flag to set
/// if you want a request to be forwarded to the origin even if the profile specified by the
/// field-level encryption query argument, fle-profile, is unknown" — since a configuration
/// that maps no query argument to a profile cannot have meant to drop the requests it does
/// not recognise.
pub fn default_query_arg_profile_config() -> QueryArgProfileConfig {
    QueryArgProfileConfig {
        forward_when_query_arg_profile_is_unknown: true,
        query_arg_profiles: Some(empty_query_arg_profiles()),
    }
}

/// Resolve every member of a submitted `FieldLevelEncryptionProfileConfig` that the SERVICE
/// supplies.
///
/// `Comment` is "An optional comment for the field-level encryption profile" — free text the
/// profile carries, which for one that states none is the empty text rather than no member
/// at all, so a round trip through `UpdateFieldLevelEncryptionProfile` does not turn the
/// member's presence into its absence. `Name`, `CallerReference` and `EncryptionEntities`
/// are required, so nothing is supplied for them.
pub fn resolve_field_level_encryption_profile_config(
    mut config: FieldLevelEncryptionProfileConfig,
) -> FieldLevelEncryptionProfileConfig {
    config.comment = Some(config.comment.unwrap_or_default());
    config
}

/// Resolve every member of a submitted `PublicKeyConfig` that the SERVICE supplies.
///
/// `Comment` is "A comment to describe the public key" — free text the key carries, which
/// for one uploaded without a comment is the empty text rather than no member at all, so a
/// round trip through `UpdatePublicKey` (whose "only value you can change is the comment")
/// does not turn the member's presence into its absence. `CallerReference`, `Name` and
/// `EncodedKey` are required, so nothing is supplied for them.
pub fn resolve_public_key_config(mut config: PublicKeyConfig) -> PublicKeyConfig {
    config.comment = Some(config.comment.unwrap_or_default());
    config
}

/// Resolve the members the service supplies on a custom origin's configuration.
pub(super) fn resolve_custom_origin_config(mut custom: CustomOriginConfig) -> CustomOriginConfig {
    custom.origin_ssl_protocols = Some(custom.origin_ssl_protocols.unwrap_or_else(|| {
        OriginSslProtocols {
            quantity: i32::try_from(DEFAULT_ORIGIN_SSL_PROTOCOLS.len()).unwrap_or(i32::MAX),
            items: DEFAULT_ORIGIN_SSL_PROTOCOLS
                .iter()
                .map(|protocol| (*protocol).to_string())
                .collect(),
        }
    }));
    custom.origin_read_timeout = Some(
        custom
            .origin_read_timeout
            .unwrap_or(DEFAULT_ORIGIN_READ_TIMEOUT),
    );
    custom.origin_keepalive_timeout = Some(
        custom
            .origin_keepalive_timeout
            .unwrap_or(DEFAULT_ORIGIN_KEEPALIVE_TIMEOUT),
    );
    custom
}
