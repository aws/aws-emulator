// SPDX-License-Identifier: CC0-1.0

//! Content delivery network distributions: the published bundle catalog they are
//! put on, the origin each caches, the cache behaviour it serves under and the
//! cache resets asked of it.

use super::*;

/// The resource portion a distribution ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISTRIBUTION_ARN_RESOURCE_PREFIX: &str = "Distribution/";

/// The Region every distribution is located in, whichever Region its origin is
/// in and whichever Region a request is made from.
///
/// From the modelled `LightsailDistribution$location` documentation: "Lightsail
/// distributions are global resources that can reference an origin in any
/// Amazon Web Services Region, and distribute its content globally. However,
/// all distributions are located in the `us-east-1` Region."
pub const DISTRIBUTION_REGION_NAME: &str = "us-east-1";

/// `LightsailDistribution$status` while the configuration a call stored is
/// still being deployed to the CDN, and once it has been.
///
/// The member is a plain string in the model ("The status of the
/// distribution"). A Lightsail distribution IS an Amazon CloudFront
/// distribution, and it carries CloudFront's own distribution status values:
/// `InProgress` while a change propagates and `Deployed` once, as the
/// CloudFront API documents for `Distribution$Status`, "the distribution's
/// information is fully propagated to all CloudFront edge locations".
pub const DISTRIBUTION_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`DISTRIBUTION_STATUS_IN_PROGRESS`].
pub const DISTRIBUTION_STATUS_DEPLOYED: &str = "Deployed";

/// The status of a reset of a distribution's cached content while the reset is
/// still being applied across the CDN, and once it has been.
///
/// `ResetDistributionCacheResult$status` ("The status of the reset cache
/// request") and `GetDistributionLatestCacheResetResult$status` ("The status of
/// the last cache reset") are plain strings in the model with no enum behind
/// them. A Lightsail distribution IS an Amazon CloudFront distribution (which is
/// why its own status carries CloudFront's distribution status values, see
/// [`DISTRIBUTION_STATUS_IN_PROGRESS`]) and deleting its cached content is a
/// CloudFront invalidation, so a reset carries CloudFront's own
/// `Invalidation$Status`: `InProgress` while the invalidation is being applied
/// and `Completed` once it has been, as the CloudFront API documents for that
/// member ("When the invalidation batch is finished, the status is Completed").
/// That a LATER read reports the status of the last reset at all is what says
/// the value changes after the call that started it: a status fixed for the life
/// of the record would be nothing to go back and read.
pub const CACHE_RESET_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`CACHE_RESET_STATUS_IN_PROGRESS`].
pub const CACHE_RESET_STATUS_COMPLETED: &str = "Completed";

/// The most distributions this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISTRIBUTIONS: usize = 100;

/// The most per-path cache behaviours one distribution may carry.
///
/// Emulator resource guard: the modelled `CacheBehaviorList` declares no length,
/// so this is where the bound on a caller-sized list lives.
pub const MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION: usize = 25;

/// The most custom error responses one distribution may carry.
///
/// Emulator resource guard, for the same reason as
/// [`MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION`]: the modelled
/// `DistributionCustomErrorResponseList` declares no length.
pub const MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION: usize = 25;

/// The `errorCachingMinTTL`, in seconds, a custom error response that names none
/// of its own is cached for.
///
/// From the modelled `DistributionCustomErrorResponse$errorCachingMinTTL`
/// documentation: "The minimum time, in seconds, that the distribution caches the
/// custom error response before requesting the object again from the origin. If
/// you don't specify a value, the default is 10 seconds."
pub const DEFAULT_ERROR_CACHING_MIN_TTL_SECONDS: i64 = 10;

/// The character a custom error response's `responsePagePath` has to begin with.
///
/// The member is modelled as a bare `string`, and its documentation states the
/// form the model cannot express: "The path to the custom error page that the
/// distribution returns to the viewer (for example, /404.html). The path must
/// begin with a forward slash (/) and reference an object that is available from
/// the origin."
pub(super) const CUSTOM_ERROR_PAGE_PATH_PREFIX: char = '/';

/// How many distributions one `GetDistributions` page carries.
///
/// The operation is paginated — its `pageToken` is "The token to advance to the
/// next page of results from your request" — but the model states no page size,
/// so this is the store's own, small enough that a modest number of
/// distributions already exercises the paging.
pub const DISTRIBUTION_PAGE_SIZE: usize = 10;

/// The smallest and largest `responseTimeout`, in seconds, an origin may be
/// given, and the one an origin that names none gets.
///
/// From the modelled `InputOrigin$responseTimeout` documentation: "The minimum
/// timeout is 1 second, the maximum is 60 seconds, and the default (if you
/// don't specify otherwise) is 30 seconds."
pub const ORIGIN_RESPONSE_TIMEOUT_SECONDS_RANGE: (i32, i32) = (1, 60);
/// See [`ORIGIN_RESPONSE_TIMEOUT_SECONDS_RANGE`].
pub const DEFAULT_ORIGIN_RESPONSE_TIMEOUT_SECONDS: i32 = 30;

/// The DNS suffix a distribution's `domainName` is published under.
///
/// AWS publishes it in CloudFront's own `cloudfront.net` zone. That HOSTNAME is
/// environment-specific and no caller may assume it, while the promise
/// `LightsailDistribution$domainName` makes — "The domain name of the
/// distribution" — is not: it must never be a name that sends the next client
/// OUT of the emulator. This emulator owns no DNS zone, so the name keeps the
/// CloudFront SHAPE (`d<token>`) under a suffix that belongs to the emulator and
/// cannot resolve off-machine, exactly as a load balancer's `dnsName` does (see
/// [`LOAD_BALANCER_DNS_SUFFIX`]): `.local` is reserved for link-local multicast
/// name resolution (RFC 6762 §3).
///
/// This emulator serves no CDN data plane on it: a distribution caches and
/// serves nothing. Everything the control plane owns — the record, its status,
/// its bundle, its origin, its cache configuration and its operation log entry
/// — is real.
pub(super) const DISTRIBUTION_DOMAIN_SUFFIX: &str = "cdn.lightsail.local";

/// How many characters of the distribution's own id go into its `domainName`:
/// the 13 CloudFront puts after the leading `d` of a distribution domain name
/// (`d111111abcdef8.cloudfront.net`).
pub(super) const DISTRIBUTION_DOMAIN_ID_LEN: usize = 13;

/// One Lightsail **distribution** bundle: the monthly cost and monthly network
/// transfer quota a distribution gets from that plan.
///
/// This is the service's published price list for CDN distributions, so it is
/// the same for every caller and no call changes it — `GetDistributionBundles`
/// reports it, and `CreateDistribution`/`UpdateDistributionBundle` name an entry
/// of it by `bundle_id`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistributionBundleSpec {
    /// The id a caller sends for this plan. A distribution bundle id is
    /// `<size>_<major>_<minor>`, the same form a bucket bundle id carries.
    pub bundle_id: &'static str,
    /// `DistributionBundle$transferPerMonthInGb` — "The monthly network transfer
    /// quota of the bundle", in GB.
    pub transfer_per_month_in_gb: i32,
    /// `DistributionBundle$price` — "The monthly price, in US dollars, of the
    /// bundle."
    pub price_usd_per_month: f32,
    /// `DistributionBundle$isActive` — "Indicates whether the bundle is active,
    /// and can be specified for a new or existing distribution."
    pub is_active: bool,
}

/// Every distribution bundle this service publishes, cheapest first: the
/// Lightsail pricing page's CDN distribution table — 50 GB of data transfer for
/// $2.50 a month, 200 GB for $5 and 500 GB for $10 — each the first (`_1_0`)
/// generation of its size.
///
/// Lightsail has retired no distribution plan, so every published plan is
/// active. A retired generation belongs in this table with `is_active: false`:
/// that is what makes it unavailable to a new distribution while one already
/// running on it still resolves.
pub const DISTRIBUTION_BUNDLE_SPECS: &[DistributionBundleSpec] = &[
    DistributionBundleSpec {
        bundle_id: "small_1_0",
        transfer_per_month_in_gb: 50,
        price_usd_per_month: 2.5,
        is_active: true,
    },
    DistributionBundleSpec {
        bundle_id: "medium_1_0",
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 5.0,
        is_active: true,
    },
    DistributionBundleSpec {
        bundle_id: "large_1_0",
        transfer_per_month_in_gb: 500,
        price_usd_per_month: 10.0,
        is_active: true,
    },
];

/// The label Lightsail gives a CDN plan, with `transfer` the plan's monthly
/// network transfer quota in GB.
///
/// SOURCE: the `DistributionBundle$name` values Lightsail reports, which name
/// the plan's transfer quota (`50GB`). The label is therefore DERIVED from the
/// quota rather than carried per entry: a plan added to
/// [`DISTRIBUTION_BUNDLE_SPECS`] is labelled the same way.
pub fn distribution_bundle_name(transfer_per_month_in_gb: i32) -> String {
    format!("{transfer_per_month_in_gb}GB")
}

impl DistributionBundleSpec {
    /// This plan as the modelled `DistributionBundle`.
    ///
    /// `name` is "The name of the distribution bundle", so it is DERIVED from
    /// [`Self::transfer_per_month_in_gb`] by [`distribution_bundle_name`] rather
    /// than carried a second time.
    pub fn view(&self) -> DistributionBundle {
        DistributionBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(distribution_bundle_name(self.transfer_per_month_in_gb)),
            price: Some(self.price_usd_per_month),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            is_active: Some(self.is_active),
        }
    }
}

// ---------------------------------------------------------------------------
// The Web Application Firewall of a distribution
// ---------------------------------------------------------------------------

/// The bundles a distribution may have the Web Application Firewall enabled on.
///
/// From the modelled `enableWaf` documentation: "WAF is available only for
/// distributions that use the `medium_1_0` or `large_1_0` bundle." The ids are
/// those of [`DISTRIBUTION_BUNDLE_SPECS`].
pub const WAF_BUNDLE_IDS: &[&str] = &["medium_1_0", "large_1_0"];

/// The WAF rule groups a distribution on the smaller of the two WAF bundles may
/// enable, from the modelled `wafRules` documentation: "`medium_1_0` – Supports
/// the `AWSManagedRulesCommonRuleSet`, `AWSManagedRulesKnownBadInputsRuleSet`,
/// and `AWSManagedRulesAnonymousIpList` rule groups."
pub const WAF_RULE_GROUPS_MEDIUM: &[&str] = &[
    "AWSManagedRulesCommonRuleSet",
    "AWSManagedRulesKnownBadInputsRuleSet",
    "AWSManagedRulesAnonymousIpList",
];

/// The WAF rule groups only a distribution on the larger WAF bundle may enable,
/// from the same documentation: "`large_1_0` – Supports the rule groups for
/// `medium_1_0`, plus …" — so the groups that bundle offers are these beside
/// [`WAF_RULE_GROUPS_MEDIUM`].
pub const WAF_RULE_GROUPS_LARGE_ONLY: &[&str] = &[
    "AWSManagedRulesAdminProtectionRuleSet",
    "AWSManagedRulesSQLiRuleSet",
    "AWSManagedRulesLinuxRuleSet",
    "AWSManagedRulesUnixRuleSet",
    "AWSManagedRulesWindowsRuleSet",
    "AWSManagedRulesPHPRuleSet",
    "AWSManagedRulesWordPressRuleSet",
    "AWSManagedRulesAmazonIpReputationList",
    WAF_RULE_GROUP_ATP,
    WAF_RULE_GROUP_ACFP,
    WAF_RULE_GROUP_BOT_CONTROL,
];

/// The account takeover prevention rule group, whose own configuration is
/// "required when the `name` is `AWSManagedRulesATPRuleSet`" and may be defined
/// "only when" it is (the modelled `WafRule$atpConfig` documentation).
pub const WAF_RULE_GROUP_ATP: &str = "AWSManagedRulesATPRuleSet";
/// The account creation fraud prevention rule group, whose
/// `WafRule$acfpConfig` carries the same constraint.
pub const WAF_RULE_GROUP_ACFP: &str = "AWSManagedRulesACFPRuleSet";
/// The bot control rule group, whose `WafRule$botControlConfig` carries the same
/// constraint.
pub const WAF_RULE_GROUP_BOT_CONTROL: &str = "AWSManagedRulesBotControlRuleSet";

/// The most WAF rule groups one distribution's record holds.
///
/// Every group this service offers, on the plan that offers the most of them
/// ([`WAF_RULE_GROUPS_MEDIUM`] plus [`WAF_RULE_GROUPS_LARGE_ONLY`]): a group is
/// either enabled on the distribution or it is not, so a list longer than that
/// names one twice, which [`waf_refusal`] refuses. It is the bound a create and
/// an update check the caller-sized `wafRules` list against before they store
/// it, so no request can grow a stored record past the groups that exist.
pub const MAX_WAF_RULES_PER_DISTRIBUTION: usize =
    WAF_RULE_GROUPS_MEDIUM.len() + WAF_RULE_GROUPS_LARGE_ONLY.len();

/// The WAF rule groups a distribution on the plan `bundle_id` names may enable:
/// none for a plan WAF is not available on, the medium set for the smaller WAF
/// plan, and that set plus the large-only groups for the larger one.
pub fn waf_rule_groups_for_bundle(bundle_id: &str) -> Vec<&'static str> {
    if !WAF_BUNDLE_IDS.contains(&bundle_id) {
        return Vec::new();
    }
    let mut groups = WAF_RULE_GROUPS_MEDIUM.to_vec();
    // The larger plan supports "the rule groups for `medium_1_0`, plus" its own.
    if bundle_id == "large_1_0" {
        groups.extend_from_slice(WAF_RULE_GROUPS_LARGE_ONLY);
    }
    groups
}

/// The refusal for a WAF configuration a distribution on the plan `bundle_id`
/// may not serve under, and `None` when it may — the crate's ONE owner of the
/// WAF rules, asked by the create and by the update.
///
/// What the model cannot express and its documentation states: WAF itself is
/// only available on [`WAF_BUNDLE_IDS`]; `wafRules` can be specified "only when
/// WAF is enabled for the distribution"; the groups a distribution may enable
/// depend on its plan ([`waf_rule_groups_for_bundle`]); and each of the three
/// rule groups that takes a configuration of its own requires it and is the only
/// group that may carry it.
pub fn waf_refusal(bundle_id: &str, enabled: bool, rules: &[WafRule]) -> Option<String> {
    if enabled && !WAF_BUNDLE_IDS.contains(&bundle_id) {
        return Some(format!(
            "enableWaf is only available for a distribution on the {} bundle, and this one is \
             on '{bundle_id}'",
            quoted_options_phrase(WAF_BUNDLE_IDS)
        ));
    }
    if !enabled && !rules.is_empty() {
        return Some(
            "wafRules can be specified only when WAF is enabled for the distribution"
                .to_string(),
        );
    }
    let offered = waf_rule_groups_for_bundle(bundle_id);
    for (position, rule) in rules.iter().enumerate() {
        if !offered.contains(&rule.name.as_str()) {
            return Some(format!(
                "wafRules names the rule group '{}', which a distribution on the '{bundle_id}' \
                 bundle does not offer; it offers {}",
                rule.name,
                quoted_options_phrase(&offered)
            ));
        }
        if rules[..position]
            .iter()
            .any(|earlier| earlier.name == rule.name)
        {
            return Some(format!(
                "wafRules names the rule group '{}' more than once",
                rule.name
            ));
        }
        for (group, configured) in [
            (WAF_RULE_GROUP_ATP, rule.atp_config.is_some()),
            (WAF_RULE_GROUP_ACFP, rule.acfp_config.is_some()),
            (WAF_RULE_GROUP_BOT_CONTROL, rule.bot_control_config.is_some()),
        ] {
            if (rule.name == group) != configured {
                return Some(format!(
                    "wafRules names the rule group '{}', and the configuration of '{group}' is \
                     defined only for that rule group and required when the rule group is it",
                    rule.name
                ));
            }
        }
    }
    None
}

/// The bundles a `GetDistributionBundles` call reports out of `specs`: EVERY
/// plan the catalog holds, retired ones included.
///
/// The request carries no member to select a slice with — unlike the bucket and
/// database catalogs, which have an `includeInactive` flag — and
/// `DistributionBundle$isActive` is what tells a caller which of the reported
/// plans a new or existing distribution may be put on, so leaving a retired plan
/// out would hide the flag's only subject.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the rule be exercised against a catalog that holds
/// a retired plan, which the published one does not yet.
pub fn distribution_bundles_from(specs: &[DistributionBundleSpec]) -> Vec<DistributionBundle> {
    specs.iter().map(DistributionBundleSpec::view).collect()
}

/// The distribution bundles a `GetDistributionBundles` call reports:
/// [`distribution_bundles_from`] over this service's published catalog,
/// [`DISTRIBUTION_BUNDLE_SPECS`].
pub fn distribution_bundles() -> Vec<DistributionBundle> {
    distribution_bundles_from(DISTRIBUTION_BUNDLE_SPECS)
}

/// The distribution bundle `bundle_id` names — active or not, since a
/// distribution already running on a retired plan still has to resolve — or
/// `None` when this service publishes no such plan.
pub fn distribution_bundle_spec(bundle_id: &str) -> Option<DistributionBundleSpec> {
    DISTRIBUTION_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

/// Whether a distribution may be put on the plan `bundle_id` names, out of the
/// catalog `specs`: the plan has to be one the catalog holds AND still active,
/// because `DistributionBundle$isActive` is what says a bundle "can be specified
/// for a new or existing distribution".
///
/// Taking the catalog as an argument is what lets the rule be exercised against a
/// catalog that holds a retired plan, which the published one does not yet — the
/// same reason [`distribution_bundles_from`] takes one.
pub fn distribution_bundle_is_applicable_in(
    specs: &[DistributionBundleSpec],
    bundle_id: &str,
) -> bool {
    specs
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .is_some_and(|spec| spec.is_active)
}

/// Whether a distribution may be put on the plan `bundle_id` names:
/// [`distribution_bundle_is_applicable_in`] over this service's published
/// catalog, [`DISTRIBUTION_BUNDLE_SPECS`].
///
/// One rule over both calls that name a plan — `CreateDistribution` for a new
/// distribution and `UpdateDistributionBundle` for one already running — so a
/// retired plan is refused the same way whichever of them names it.
pub fn distribution_bundle_is_applicable(bundle_id: &str) -> bool {
    distribution_bundle_is_applicable_in(DISTRIBUTION_BUNDLE_SPECS, bundle_id)
}

/// The ARN of the distribution `id`. Every distribution is located in
/// [`DISTRIBUTION_REGION_NAME`], so that is the Region its ARN carries.
pub fn distribution_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DISTRIBUTION_REGION_NAME,
        &format!("{DISTRIBUTION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `domainName` of the distribution minted as `id`. See
/// [`DISTRIBUTION_DOMAIN_SUFFIX`].
pub fn distribution_domain_name(id: &str) -> String {
    let token: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(DISTRIBUTION_DOMAIN_ID_LEN)
        .collect();
    format!("d{token}.{DISTRIBUTION_DOMAIN_SUFFIX}")
}

/// The custom error responses `requested` asks a distribution to serve, each with
/// every member it left out filled in from that member's documented default, or a
/// refusal when the list carries more entries than one distribution holds or an
/// entry states a page path the distribution could not fetch from its origin.
///
/// A member the model documents a default for is one the SERVICE fills in, so a
/// stored — and therefore reported — entry carries it whether or not the request
/// did, exactly as the origin's `responseTimeout` and the distribution's
/// `ipAddressType` do. `errorCachingMinTTL` is the only member of
/// `DistributionCustomErrorResponse` whose documentation states a default; the
/// other three are reported as the request stated them.
///
/// `responsePagePath` is the path of an object of the origin, which is what
/// beginning with [`CUSTOM_ERROR_PAGE_PATH_PREFIX`] says, so an entry stating one
/// in any other form states no page the distribution could serve. An entry that
/// carries no page path states no path to check.
///
/// The ONE owner of that rule, so `CreateDistribution` and `UpdateDistribution`
/// cannot record the list two different ways.
pub fn custom_error_responses_from_request(
    requested: &[DistributionCustomErrorResponse],
) -> Result<Vec<DistributionCustomErrorResponse>, String> {
    if requested.len() > MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION {
        return Err(format!(
            "customErrorResponses carries {} entries, more than the {} one distribution holds",
            requested.len(),
            MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
        ));
    }
    for (index, response) in requested.iter().enumerate() {
        let Some(path) = response.response_page_path.as_deref() else {
            continue;
        };
        if !path.starts_with(CUSTOM_ERROR_PAGE_PATH_PREFIX) {
            return Err(format!(
                "customErrorResponses[{index}].responsePagePath '{path}' is not the path to an \
                 object of the origin: the path must begin with a forward slash \
                 ({CUSTOM_ERROR_PAGE_PATH_PREFIX}) and reference an object that is available from \
                 the origin"
            ));
        }
    }
    Ok(requested
        .iter()
        .map(|response| {
            let mut recorded = response.clone();
            recorded.error_caching_min_ttl = Some(
                recorded
                    .error_caching_min_ttl
                    .unwrap_or(DEFAULT_ERROR_CACHING_MIN_TTL_SECONDS),
            );
            recorded
        })
        .collect())
}

/// The method sets `CacheSettings$allowedHTTPMethods` admits, and the ones
/// `CacheSettings$cachedHTTPMethods` admits.
///
/// Both members are modelled as a free-form `NonEmptyString`, so the closed
/// domain lives only in each member's own documentation — "You can specify the
/// following options": three sets for the methods a distribution "processes and
/// forwards to the distribution's origin", and two for "the HTTP method
/// responses that are cached by your distribution". A distribution cannot
/// advertise a method set the service does not offer, so a string outside its
/// member's list is refused the same way an unpublished TLS policy name is.
///
/// SOURCE: the two members of `CacheSettings` —
/// `smithy_query(query="shape", shape="com.amazonaws.lightsail#CacheSettings")`.
pub const ALLOWED_HTTP_METHODS_OPTIONS: &[&str] = &[
    "GET,HEAD",
    "GET,HEAD,OPTIONS",
    "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE",
];
/// See [`ALLOWED_HTTP_METHODS_OPTIONS`].
pub const CACHED_HTTP_METHODS_OPTIONS: &[&str] = &["GET,HEAD", "GET,HEAD,OPTIONS"];

/// The `ForwardValues` member that says a distribution forwards every request
/// header to its origin: "`all` - Forward all headers to your origin"
/// (`HeaderObject$option`).
pub const FORWARD_ALL_HEADERS_OPTION: &str = "all";

/// The `minimumTTL` a distribution that forwards all headers to its origin has
/// to be given.
///
/// From the modelled `CacheSettings$minimumTTL` documentation: "A value of `0`
/// must be specified for `minimumTTL` if the distribution is configured to
/// forward all headers to the origin."
pub const MINIMUM_TTL_FORWARDING_ALL_HEADERS: i64 = 0;

/// The refusal message for a cache settings object outside what the members of
/// `CacheSettings` document, and `None` when the settings are ones a
/// distribution may serve under.
///
/// Two of that structure's rules the model itself cannot express:
///
/// * `allowedHTTPMethods` and `cachedHTTPMethods` each carry a closed list of
///   option strings in a free-form member — see
///   [`ALLOWED_HTTP_METHODS_OPTIONS`];
/// * `minimumTTL` has to be [`MINIMUM_TTL_FORWARDING_ALL_HEADERS`] when the
///   settings forward all headers to the origin, because the cache key then
///   varies with every header value. A request that states no `minimumTTL`
///   states no value that rule can be broken by.
///
/// The ONE owner of them, so `CreateDistribution` and `UpdateDistribution`
/// cannot admit a cache configuration two different ways: the update replaces
/// the whole object ("The `cacheBehaviorSettings` specified in your
/// `UpdateDistributionRequest` will replace your distribution's existing
/// settings"), so the settings the request carries are the settings the
/// distribution would serve under and are held to exactly the same rules.
pub fn cache_behavior_settings_refusal(settings: &CacheSettings) -> Option<String> {
    for (member, value, options) in [
        (
            "allowedHTTPMethods",
            settings.allowed_http_methods.as_deref(),
            ALLOWED_HTTP_METHODS_OPTIONS,
        ),
        (
            "cachedHTTPMethods",
            settings.cached_http_methods.as_deref(),
            CACHED_HTTP_METHODS_OPTIONS,
        ),
    ] {
        let Some(value) = value else {
            continue;
        };
        if !options.contains(&value) {
            return Some(format!(
                "cacheBehaviorSettings.{member} '{value}' is not an HTTP method set a \
                 distribution offers: you can specify {}",
                quoted_options_phrase(options)
            ));
        }
    }
    let forwards_all_headers = settings
        .forwarded_headers
        .as_ref()
        .and_then(|headers| headers.option.as_deref())
        == Some(FORWARD_ALL_HEADERS_OPTION);
    if forwards_all_headers {
        if let Some(minimum_ttl) = settings.minimum_ttl {
            if minimum_ttl != MINIMUM_TTL_FORWARDING_ALL_HEADERS {
                return Some(format!(
                    "cacheBehaviorSettings.minimumTTL is {minimum_ttl}: a value of \
                     {MINIMUM_TTL_FORWARDING_ALL_HEADERS} must be specified for minimumTTL if the \
                     distribution is configured to forward all headers to the origin, which \
                     cacheBehaviorSettings.forwardedHeaders.option \
                     '{FORWARD_ALL_HEADERS_OPTION}' asks for"
                ));
            }
        }
    }
    None
}

/// `options` as a refusal lists a closed set of option strings: each one
/// quoted, the last introduced by "or".
fn quoted_options_phrase(options: &[&str]) -> String {
    let quoted: Vec<String> = options.iter().map(|option| format!("'{option}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{}, or {last}", leading.join(", ")),
    }
}

/// What an origin name resolves to: the kind of resource it addresses, and the
/// public DNS name this store publishes for that resource, if it publishes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOrigin {
    /// The resolved resource's own `ResourceType` value, which is what
    /// `Origin$resourceType` reports.
    pub resource_type: &'static str,
    /// `Origin$originPublicDNS`, "The public DNS of the origin" — filled only
    /// for a resource this store publishes a public DNS NAME for (a load
    /// balancer's `dnsName`). An instance is published at an IP address and a
    /// bucket and a container service at an address under the endpoint the
    /// request arrived at, so neither has a public DNS name to report and the
    /// optional member is left out rather than guessed at.
    pub public_dns: Option<String>,
}

/// The last reset of ONE distribution's cached content: what a
/// `ResetDistributionCache` call started, and the whole of what
/// `GetDistributionLatestCacheReset` reports.
///
/// A distribution holds the LATEST reset only — both operations answer a single
/// timestamp and a single status, "the last cache reset" — so a new reset
/// replaces the record of the one before it rather than appending to a history
/// nothing reports.
#[derive(Debug, Clone, Copy)]
pub struct StoredCacheReset {
    /// When the reset was requested: `createTime`, "The timestamp of the reset
    /// cache request".
    pub created_at_ms: i64,
    /// One of [`CACHE_RESET_STATUS_IN_PROGRESS`] /
    /// [`CACHE_RESET_STATUS_COMPLETED`].
    pub status: &'static str,
    /// The status the clock owes this reset, while one is outstanding.
    pub due: Option<DueNamedState>,
}

impl StoredCacheReset {
    /// The reset a `ResetDistributionCache` call at `now_ms` starts: the
    /// invalidation is being applied, and the clock owes it
    /// [`CACHE_RESET_STATUS_COMPLETED`].
    ///
    /// The span is non-zero, so a caller that resets and reads straight back
    /// still observes the reset in progress, exactly as it does against AWS.
    pub fn started(now_ms: i64) -> Self {
        Self {
            created_at_ms: now_ms,
            status: CACHE_RESET_STATUS_IN_PROGRESS,
            due: Some(DueNamedState::after(now_ms, CACHE_RESET_STATUS_COMPLETED)),
        }
    }
}

/// One stored distribution: the CDN distribution `CreateDistribution` created.
///
/// What the caller asked for — name, bundle, origin, cache behaviour and
/// settings, custom error responses, IP address type, certificate name, minimum
/// viewer TLS version, default root object and tags — is kept as asked for, with
/// the documented default of a member the request left out filled in; the ARN,
/// support code, creation time, domain name, status and the origin's resolved
/// resource type are assigned by this store from the request plus its own
/// allocators.
#[derive(Debug, Clone)]
pub struct StoredDistribution {
    /// The distribution name the request named, kept verbatim — it is this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the distribution lives: [`DISTRIBUTION_REGION_NAME`], and no
    /// availability zone, because a distribution is a global resource rather
    /// than a zonal one.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The address this distribution is published at, minted by
    /// [`distribution_domain_name`].
    pub domain_name: String,
    /// One of [`DISTRIBUTION_STATUS_IN_PROGRESS`] /
    /// [`DISTRIBUTION_STATUS_DEPLOYED`].
    pub status: &'static str,
    /// The status the clock owes this distribution, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// Whether the distribution is enabled. `CreateDistribution` exposes no
    /// `isEnabled` parameter and `UpdateDistribution`'s documentation gives it
    /// as the way to "enable the distribution", so a created distribution is
    /// enabled and that operation is what turns it off.
    pub is_enabled: bool,
    /// The bundle currently applied, an id of [`DISTRIBUTION_BUNDLE_SPECS`].
    pub bundle_id: String,
    /// When this distribution's bundle was last changed; `None` on one that has
    /// never been moved. A distribution's bundle can be updated "only one time
    /// within your monthly Amazon Web Services billing cycle"
    /// (`UpdateDistributionBundle`), so the instant of that change is what says
    /// whether another one is available — see
    /// [`StoredDistribution::able_to_update_bundle`].
    pub bundle_updated_at_ms: Option<i64>,
    /// The SSL/TLS certificate attached to the distribution, if the request
    /// named one or `AttachCertificateToDistribution` attached one.
    pub certificate_name: Option<String>,
    /// The alternate domain names of the distribution, which come from the
    /// domains of an attached certificate: a distribution with none attached has
    /// none of its own.
    pub alternative_domain_names: Vec<String>,
    /// The origin the distribution pulls content from: what the request asked
    /// for, plus the resource type this store resolved the origin name to.
    pub origin: Origin,
    /// See [`ResolvedOrigin::public_dns`].
    pub origin_public_dns: Option<String>,
    pub default_cache_behavior: CacheBehavior,
    /// The cache behaviour settings the request carried, or `None` when it
    /// carried none — the model documents no default for any member of
    /// `CacheSettings`, so nothing is invented for a request that set none.
    /// What a request DOES carry is held to the rules of that structure's
    /// members by [`cache_behavior_settings_refusal`] before it is recorded, so
    /// no stored settings advertise a method set the service does not offer or a
    /// minimum TTL the headers it forwards rule out.
    pub cache_behavior_settings: Option<CacheSettings>,
    pub cache_behaviors: Vec<CacheBehaviorPerPath>,
    pub ip_address_type: String,
    pub viewer_minimum_tls_protocol_version: Option<String>,
    pub default_root_object: Option<String>,
    /// The custom error responses this distribution serves, as
    /// [`custom_error_responses_from_request`] recorded them: the entries the
    /// request carried, with each member the model documents a default for
    /// filled in.
    pub custom_error_responses: Vec<DistributionCustomErrorResponse>,
    /// The last reset of this distribution's cached content, once one has been
    /// requested: `None` on a distribution whose cache has never been reset,
    /// which has no timestamp and no status of a reset to report.
    pub latest_cache_reset: Option<StoredCacheReset>,
    /// Whether the Web Application Firewall inspects this distribution's viewer
    /// requests, which `enableWaf` sets on the create and the update.
    pub waf_enabled: bool,
    /// The WAF rule groups enabled on this distribution, which `wafRules`
    /// replaces — EMPTY on a distribution whose WAF is enabled with no rule
    /// group ("the distribution has no WAF rule groups enabled and doesn't
    /// block any requests") and on one whose WAF is disabled, because disabling
    /// it "removes its WAF rule groups". What a request carries is held to the
    /// rules of the WAF documentation by [`waf_refusal`] before it is recorded.
    pub waf_rules: Vec<WafRule>,
}

/// The attachment of one SSL/TLS certificate to a distribution: the certificate
/// the distribution names, and the domains it accepts HTTPS traffic for.
///
/// The pair travels as ONE value — built from the certificate itself by
/// [`StoredCertificate::distribution_attachment`] and applied by
/// [`StoredDistribution::attach_certificate`] — so no caller can record the
/// certificate without the domains it covers.
#[derive(Debug, Clone)]
pub struct DistributionCertificateAttachment {
    /// The certificate's own name, as this account addresses it.
    pub(super) certificate_name: String,
    /// Every domain that certificate covers, from
    /// [`StoredCertificate::domain_names`].
    pub(super) alternative_domain_names: Vec<String>,
}

impl StoredDistribution {
    /// Record the SSL/TLS certificate `attachment` on this distribution, so it
    /// names that certificate and publishes every domain the certificate covers
    /// — the ONE owner of that rule.
    ///
    /// The domains a distribution accepts HTTPS traffic for come from the
    /// certificate attached to it ("your distribution accepts HTTPS traffic for
    /// all of the domains that are associated with the certificate"), so making
    /// the attachment writes BOTH members, never one without the other. Every
    /// operation that attaches one — the create that names a certificate for a
    /// new distribution as much as the attach and the update that set one later
    /// — asks here, so the attachment cannot drift between them. A distribution
    /// holds ONE attached certificate, so this replaces whatever was attached
    /// before.
    pub fn attach_certificate(&mut self, attachment: DistributionCertificateAttachment) {
        self.certificate_name = Some(attachment.certificate_name);
        self.alternative_domain_names = attachment.alternative_domain_names;
    }

    /// Release the SSL/TLS certificate this distribution serves HTTPS traffic
    /// with, so it names none and publishes none of that certificate's domains —
    /// the ONE owner of that rule.
    ///
    /// A distribution that serves no certificate of its own publishes no
    /// alternate domain names: the domains it accepts HTTPS traffic for come
    /// from the certificate attached to it ("your distribution accepts HTTPS
    /// traffic for all of the domains that are associated with the
    /// certificate"), so releasing the attachment clears BOTH members, never
    /// one without the other — the exact reverse of
    /// [`Self::attach_certificate`]. Every operation that releases the
    /// attachment asks here, so the release cannot drift between them.
    /// Idempotent: a distribution that has no certificate attached already
    /// satisfies it.
    pub fn detach_certificate(&mut self) {
        self.certificate_name = None;
        self.alternative_domain_names = Vec::new();
    }

    /// Whether the one bundle change this distribution's monthly billing cycle
    /// allows is available at `now_ms` — the `ableToUpdateBundle` a read reports.
    pub fn able_to_update_bundle(&self, now_ms: i64) -> bool {
        bundle_change_available(self.bundle_updated_at_ms, now_ms)
    }

    /// This record as the modelled `LightsailDistribution`, with `subdomain`
    /// the Lightsail subdomain this distribution is reachable at.
    ///
    /// The subdomain is passed in because the association lives on the
    /// SUBDOMAIN record, so only the store can resolve it — see
    /// [`ComAmazonawsLightsailLightsail20161128State::distribution_view`],
    /// which is the one door a reporting operation renders a distribution
    /// through.
    pub fn view(
        &self,
        subdomain: Option<LightsailDistributionSubdomain>,
    ) -> LightsailDistribution {
        LightsailDistribution {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISTRIBUTION.to_string()),
            // A list member is reported EMPTY rather than left out: an absent
            // member hands a typed caller a null where it iterates.
            alternative_domain_names: Some(self.alternative_domain_names.clone()),
            status: Some(self.status.to_string()),
            is_enabled: Some(self.is_enabled),
            domain_name: Some(self.domain_name.clone()),
            bundle_id: Some(self.bundle_id.clone()),
            certificate_name: self.certificate_name.clone(),
            origin: Some(self.origin.clone()),
            origin_public_dns: self.origin_public_dns.clone(),
            default_cache_behavior: Some(self.default_cache_behavior.clone()),
            cache_behavior_settings: self.cache_behavior_settings.clone(),
            cache_behaviors: Some(self.cache_behaviors.clone()),
            able_to_update_bundle: Some(self.able_to_update_bundle(now_ms())),
            ip_address_type: Some(self.ip_address_type.clone()),
            tags: Some(self.tags.clone()),
            viewer_minimum_tls_protocol_version: self
                .viewer_minimum_tls_protocol_version
                .clone(),
            default_root_object: self.default_root_object.clone(),
            custom_error_responses: Some(self.custom_error_responses.clone()),
            waf: Some(WafConfig {
                is_waf_enabled: Some(self.waf_enabled),
                // A list member is reported EMPTY rather than left out, for the
                // same reason the alternate domain names are.
                waf_rules: Some(self.waf_rules.clone()),
            }),
            subdomain,
        }
    }
}
