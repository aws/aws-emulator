// SPDX-License-Identifier: CC0-1.0

//! How a resource of this service is NAMED, ADDRESSED and COMPARED.
//!
//! One concern, in four shapes: the ARNs, resource paths and `Location` URIs a resource is
//! addressed by; the predicates that say whether a value is an identifier — or a version
//! token — of a given kind, which is what separates "no such resource" from "that is not an
//! identifier at all"; the `If-Match` check every conditional mutation goes through; and
//! the equality a submitted value is compared with a stored one by, which is what makes a
//! repeated create the same request again rather than a conflict.
//!
//! The names a DOMAIN carries are the same concern, so the domain, customization and
//! parameter refusals a tenant's names are admitted by live here too. Nothing in this
//! module reads or writes the store.

use super::*;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudfrontCloudfront20200531State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudfrontCloudfront20200531State>)
}

/// The ARN of a distribution. CloudFront is a GLOBAL service (`arnNamespace`
/// `cloudfront`, no regional endpoint), so the ARN's region segment is empty:
/// `arn:aws:cloudfront::<account>:distribution/<Id>`.
pub fn distribution_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("distribution/{id}"))
}

/// The path of a distribution resource under this service's API version.
pub fn distribution_path(id: &str) -> String {
    format!("/2020-05-31/distribution/{id}")
}

/// The ARN of a streaming distribution. CloudFront is a GLOBAL service, so the ARN's
/// region segment is empty, and the resource part names the RTMP resource kind:
/// `arn:aws:cloudfront::<account>:streaming-distribution/<Id>`.
pub fn streaming_distribution_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("streaming-distribution/{id}"))
}

/// The path of a streaming distribution resource under this service's API version — the
/// URI `CreateStreamingDistribution` reports as the `Location` of what it created.
pub fn streaming_distribution_path(id: &str) -> String {
    format!("/2020-05-31/streaming-distribution/{id}")
}

/// The path of one invalidation of one distribution.
pub fn invalidation_path(distribution_id: &str, invalidation_id: &str) -> String {
    format!("/2020-05-31/distribution/{distribution_id}/invalidation/{invalidation_id}")
}

/// The path of one invalidation of one distribution TENANT — the URI the tenant
/// invalidation operations are bound to.
pub fn distribution_tenant_invalidation_path(tenant_id: &str, invalidation_id: &str) -> String {
    format!("/2020-05-31/distribution-tenant/{tenant_id}/invalidation/{invalidation_id}")
}

/// The ARN of a key value store. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:key-value-store/<Id>`. This is the
/// identifier the key value store DATA plane (`cloudfront-keyvaluestore`) addresses the
/// store by, so the resource part is the store's minted UUID id.
pub fn key_value_store_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("key-value-store/{id}"))
}

/// The path of a key value store resource under this service's API version.
///
/// A create's `Location` addresses the new resource by the identifier the SERVICE minted
/// for it, never by the `Name` the caller supplied — and the identifier a key value store
/// is addressed by is its ARN ([`key_value_store_arn`]), whose own last segment is the
/// generated `Id`. That is what the recorded `CreateKeyValueStore` response carries, and
/// it is the identifier the whole key value store surface uses for a store: the control
/// plane reports it as `KeyValueStore$ARN`, and the data plane
/// (`cloudfront-keyvaluestore`) takes it as the `KvsARN` label of every one of its
/// routes. So `identifier` is a store ARN, not a name and not a bare id.
///
/// The ARN rather than the bare `Id` is what the recorded corpus holds this header to:
/// replaying `cloudfront-keyvaluestore-put-get-describe-etag-chain#setup#1` with the
/// bare `Id` as the last segment still reported `$headers.location` diverging, and with
/// the ARN that step left the divergence set. Both forms end in the generated `Id`; the
/// recording is what distinguishes them.
pub fn key_value_store_path(identifier: &str) -> String {
    format!("/2020-05-31/key-value-store/{identifier}")
}

/// The ARN of an Anycast static IP list. CloudFront is a GLOBAL service, so the ARN's
/// region segment is empty: `arn:aws:cloudfront::<account>:anycast-ip-list/<Id>`, the
/// resource type this service's own `/anycast-ip-list` routes name.
pub fn anycast_ip_list_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("anycast-ip-list/{id}"))
}

/// "The static IP addresses that are allocated to the Anycast static IP list": the slots
/// `hosts` holds, rendered in the families `ip_address_type` admits.
///
/// `ipv4` reports IPv4 addresses, `ipv6` reports IPv6 ones, and `dualstack` is "a list of
/// both IPv4 and IPv6 addresses", so it renders alternate slots from the two blocks and
/// holds at least one of each. The ALLOCATION is the list's own
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_anycast_ip_hosts`]) and is
/// untouched here, so a list whose address type changes reports the same slots in the new
/// families rather than taking a second allocation out of the pool.
pub fn anycast_ips_of(hosts: &[u32], ip_address_type: &str) -> Vec<String> {
    hosts
        .iter()
        .enumerate()
        .map(|(offset, host)| {
            let ipv6 = match ip_address_type {
                IP_ADDRESS_TYPE_IPV6 => true,
                IP_ADDRESS_TYPE_DUALSTACK => offset % 2 == 1,
                _ => false,
            };
            if ipv6 {
                format!("{ANYCAST_IPV6_PREFIX}{host:x}")
            } else {
                format!("{ANYCAST_IPV4_PREFIX}{host}")
            }
        })
        .collect()
}

/// Why the domain list a call would bind to a distribution tenant is no list of domains a
/// tenant can hold, or `None` when it is one.
///
/// "You must specify at least one domain", "Each distribution tenant can have up to 5
/// domains", and a domain is where the tenant's viewers reach it — so an empty list, a list
/// over [`MAX_DOMAINS_PER_TENANT`], a member that is no domain name
/// ([`domain_name_refusal`]) and a domain named twice (which describes no single
/// association) are each refused. The count is decided FIRST, so neither the per-domain walk
/// nor anything after it grows with a list the caller chose the length of.
///
/// ONE rule for every call that binds a tenant's domains, and decided from the request
/// alone, so a caller's list is bounded before any store lock is taken. The reason carries
/// no error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn tenant_domains_refusal(domains: &[String]) -> Option<String> {
    if domains.is_empty() {
        return Some("you must specify at least one domain for a distribution tenant".to_string());
    }
    if domains.len() > MAX_DOMAINS_PER_TENANT {
        return Some(format!(
            "a distribution tenant holds at most {MAX_DOMAINS_PER_TENANT} domains but the \
             request names {}",
            domains.len()
        ));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for domain in domains {
        if let Some(reason) = domain_name_refusal(domain) {
            return Some(reason);
        }
        if !seen.insert(domain.to_ascii_lowercase()) {
            return Some(format!("the domain {domain} is named more than once"));
        }
    }
    None
}

/// Why `parameters` is more parameter values than a distribution tenant's record carries, or
/// `None` when it is not.
///
/// The values END UP IN THE STORED RECORD, so the count is bounded
/// ([`MAX_PARAMETERS_PER_TENANT`]) before anything walks them: neither the work a request
/// causes nor the size of the record it stores grows with a list the caller chose the length
/// of. Whether the values SATISFY the distribution's definitions is the separate rule
/// [`parameter_refusal`] states, which needs the stored distribution.
///
/// ONE rule for every call that supplies a tenant's parameter values. The reason carries no
/// error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn tenant_parameter_count_refusal(parameters: &[Parameter]) -> Option<String> {
    (parameters.len() > MAX_PARAMETERS_PER_TENANT).then(|| {
        format!(
            "a distribution tenant carries at most {MAX_PARAMETERS_PER_TENANT} parameter values \
             but the request names {}",
            parameters.len()
        )
    })
}

/// The path of a continuous deployment policy resource under this service's API version.
/// The resource is addressed by id, as `GetContinuousDeploymentPolicy`'s `{Id}` label is.
pub fn continuous_deployment_policy_path(id: &str) -> String {
    format!("/2020-05-31/continuous-deployment-policy/{id}")
}

/// The ARN of a distribution tenant. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:distribution-tenant/<Id>`, the
/// resource type this service's own `/distribution-tenant` routes name.
pub fn distribution_tenant_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("distribution-tenant/{id}"))
}

/// The ARN of a connection group. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:connection-group/<Id>`, the resource
/// type this service's own `/connection-group` routes name.
pub fn connection_group_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("connection-group/{id}"))
}

/// The resource type a resource policy may be attached to, as the resource part of an ARN
/// spells it.
///
/// CloudFront's resource-policy surface scopes cross-account sharing of ONE kind of
/// resource, a VPC origin, and the ARN of one spells that type WITHOUT the separator its
/// route carries (`/2020-05-31/vpc-origin/{Id}`): the model's own VPC origin examples carry
/// `arn:aws:cloudfront::<account>:vpcorigin/<Id>`. So this is the one spelling both the
/// minting ([`vpc_origin_arn`]) and every resolution of a submitted ARN read
/// ([`resource_policy_resource`]), and neither derives it from a route.
pub const RESOURCE_POLICY_RESOURCE_TYPE: &str = "vpcorigin";

/// The ARN of a VPC origin. CloudFront is a GLOBAL service, so the ARN's region segment is
/// empty: `arn:aws:cloudfront::<account>:vpcorigin/<Id>`, the resource type the model's own
/// VPC origin examples spell ([`RESOURCE_POLICY_RESOURCE_TYPE`]).
pub fn vpc_origin_arn(id: &str) -> String {
    ::engine::emulator_arn(
        "cloudfront",
        "",
        &format!("{RESOURCE_POLICY_RESOURCE_TYPE}/{id}"),
    )
}

/// The path of a VPC origin resource under this service's API version — the route
/// `GetVpcOrigin` serves (`/2020-05-31/vpc-origin/{Id}`), which is therefore the resource
/// the create's `Location` names.
pub fn vpc_origin_path(id: &str) -> String {
    format!("/2020-05-31/vpc-origin/{id}")
}

/// The resource an ARN of THIS service names, as the `(type, id)` pair its resource part
/// spells, or `None` where the value is not an ARN of this service naming a resource.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` and this service's
/// resources spell their resource part `<type>/<id>` ([`arn_resource_type`]), so what is
/// read here is `arn:`, a named partition and this service's own namespace — the region and
/// account segments that follow are the owner's and narrow nothing about WHICH resource the
/// ARN names — and then that pair. One reading of an ARN for every caller that has to
/// resolve one against this store, so no two of them can disagree about which resource an
/// ARN names.
pub fn cloudfront_arn_resource(arn: &str) -> Option<(&str, &str)> {
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" || segments.next()?.is_empty() || segments.next()? != "cloudfront"
    {
        return None;
    }
    let (kind, id) = segments.nth(2)?.split_once('/')?;
    (!kind.is_empty() && !id.is_empty()).then_some((kind, id))
}

/// The resource a `ResourceArn` names for the resource-policy surface — the id of the VPC
/// origin it addresses — or `None` where the ARN names no such resource.
///
/// `ResourceArn` is modelled as a plain `string` with no pattern, so nothing is refused
/// before a handler runs and reading the ARN is this check: it is an ARN of this service
/// ([`cloudfront_arn_resource`]) whose resource part names [`RESOURCE_POLICY_RESOURCE_TYPE`]
/// and carries an id. A string that is no ARN at all, an ARN of another service and an ARN
/// naming another resource type all answer `None` — which is not one refusal but two, told
/// apart by whether the value can be READ as an ARN at all
/// ([`resource_policy_arn_refusal`]): a value that cannot is refused on its form, and every
/// ARN that names another type is refused on that TYPE.
pub fn resource_policy_resource(arn: &str) -> Option<&str> {
    let (kind, id) = cloudfront_arn_resource(arn)?;
    (kind == RESOURCE_POLICY_RESOURCE_TYPE).then_some(id)
}

/// The sentence either resource-policy WRITE — the one that attaches a policy and the one
/// that removes it — refuses a `ResourceArn` that is not a VPC origin's with: the whole text
/// is fixed, so nothing of the request — neither the ARN nor the type it names — reaches it.
/// Recorded behaviour.
pub const INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE: &str =
    "The specified resource type isn't valid. Specify a VPC origin and try again.";

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy WRITE naming a resource type
/// the surface does not attach policies to is refused with — "An argument is invalid."
///
/// Both writes of the surface word the condition this way: what a caller has to fix is the
/// resource type it named, whether it was attaching a policy or removing one.
///
/// Decided from the PARSED ARN, before it is resolved against the store: the type a
/// `ResourceArn` names is a property of the value the request carried, so it is judged
/// without asking what the store holds, and the absent-resource answer
/// ([`resource_policy_resource_absent`]) is reserved for an ARN of the type the surface does
/// accept.
pub fn invalid_resource_policy_resource_type() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE.to_string()),
    }
}

/// The sentence a resource-policy READ refuses a `ResourceArn` that is not a VPC origin's
/// with.
///
/// The CONDITION the three operations of the surface refuse is one
/// ([`resource_policy_arn_refusal`]); the sentence each puts on the wire is the
/// service's own, and the read words it as a demand for a usable ARN rather than naming the
/// resource type the write asks for. Recorded behaviour; the whole text is fixed, so nothing
/// of the request reaches it.
pub const INVALID_RESOURCE_POLICY_ARN_MESSAGE: &str =
    "The ResourceArn parameter is required. Specify a valid ARN and try again.";

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy READ naming a resource type
/// the surface does not attach policies to is refused with.
pub fn invalid_resource_policy_arn() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESOURCE_POLICY_ARN_MESSAGE.to_string()),
    }
}

/// What every ARN opens with, whatever partition, service or resource follows it.
///
/// A value that does not open with it is no ARN at all, which is the one condition
/// [`MALFORMED_ARN_MESSAGE`] states.
pub const ARN_PREFIX: &str = "arn:";

/// The sentence an operation taking an ARN refuses a value that is no ARN AT ALL with: the
/// whole text is fixed, and it states the FORM the parameter has to take rather than the
/// value it was given. Recorded behaviour.
pub const MALFORMED_ARN_MESSAGE: &str = "ARNs must start with 'arn:'";

/// The modelled `InvalidArgument` (HTTP 400) for a parameter that is no ARN at all —
/// "An argument is invalid."
///
/// A value that does not open with [`ARN_PREFIX`] cannot be READ as an ARN, so the refusal
/// is about its form and nothing of the value reaches the sentence.
pub fn malformed_arn() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MALFORMED_ARN_MESSAGE.to_string()),
    }
}

/// The sentence family the OWNED-RESOURCE listing refuses a `ResourceArn` it cannot READ
/// with: the fixed opening says the value's SYNTAX could not be read, `reason` names the part
/// of the value that could not be read, and the one variable part is the value the request
/// carried, echoed verbatim at the end so a caller submitting several ARNs learns WHICH one it
/// has to fix.
///
/// TWO readings of such a value fail this way, so ONE sentence family states both: the value
/// does not open with [`ARN_PREFIX`] and is no ARN at all
/// ([`invalid_resource_arn_syntax_message`]), or it is an ARN of this service whose resource
/// part names a resource type CloudFront writes nothing under
/// ([`invalid_arn_resource_type_syntax_message`]), so there is no CloudFront resource in it to
/// resolve. Recorded behaviour.
pub fn invalid_resource_syntax_message(reason: &str, resource_arn: &str) -> String {
    format!("Resource has invalid syntax: {reason}: {resource_arn}")
}

/// The sentence the OWNED-RESOURCE listing refuses a `ResourceArn` that is no ARN at all
/// with — the [`invalid_resource_syntax_message`] family under the service-wide form
/// condition ([`MALFORMED_ARN_MESSAGE`]): the value does not open with [`ARN_PREFIX`].
pub fn invalid_resource_arn_syntax_message(resource_arn: &str) -> String {
    invalid_resource_syntax_message(MALFORMED_ARN_MESSAGE, resource_arn)
}

/// The modelled `InvalidArgument` (HTTP 400) the owned-resource listing refuses a
/// `ResourceArn` that cannot be READ as an ARN with — "An argument is invalid."
///
/// Decided from the REQUEST alone, before anything is resolved against the store: a value
/// that is no ARN names no resource to look up.
pub fn invalid_resource_arn_syntax(resource_arn: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(invalid_resource_arn_syntax_message(resource_arn)),
    }
}

/// What the [`invalid_resource_syntax_message`] family names as the part it could not read,
/// for an ARN of this service whose resource part spells a resource type CloudFront writes no
/// resource under: the type is the one part of such an ARN that is read before the resource it
/// names can be, and a word no type of this service is spelled with leaves nothing to read.
/// Recorded behaviour.
pub const INVALID_ARN_RESOURCE_TYPE_REASON: &str = "invalid resource type";

/// The sentence the OWNED-RESOURCE listing refuses a `ResourceArn` of this service whose
/// resource part names NO resource type CloudFront writes with: the
/// [`invalid_resource_syntax_message`] family under [`INVALID_ARN_RESOURCE_TYPE_REASON`], so
/// the submitted value is echoed as it is for a value that is no ARN at all.
pub fn invalid_arn_resource_type_syntax_message(resource_arn: &str) -> String {
    invalid_resource_syntax_message(INVALID_ARN_RESOURCE_TYPE_REASON, resource_arn)
}

/// The modelled `InvalidArgument` (HTTP 400) the owned-resource listing refuses a
/// `ResourceArn` whose resource type cannot be read as one of this service's with — "An
/// argument is invalid."
///
/// Decided from the REQUEST alone: whether the value spells a type this service writes is a
/// property of the value, so nothing is resolved against the store for it.
pub fn invalid_arn_resource_type_syntax(
    resource_arn: &str,
) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(invalid_arn_resource_type_syntax_message(resource_arn)),
    }
}

/// The resource TYPES CloudFront's ARN namespace writes a resource under, as the resource part
/// of an ARN spells each of them.
///
/// Every kind of resource this service's API addresses, spelled the way that resource's ARN
/// spells its type rather than the way its route does — a VPC origin's ARN carries
/// [`RESOURCE_POLICY_RESOURCE_TYPE`] where its route carries `vpc-origin`. The types this
/// service MINTS an ARN under are the ones with an ARN constructor beside this
/// ([`distribution_arn`], [`anycast_ip_list_arn`], [`vpc_origin_arn`],
/// [`distribution_tenant_arn`], [`connection_group_arn`], [`key_value_store_arn`],
/// [`streaming_distribution_arn`], the function, connection-function, real-time log
/// configuration and trust store ones); the rest are the kinds its other routes create and
/// address (`/cache-policy`, `/origin-request-policy`, `/response-headers-policy`,
/// `/continuous-deployment-policy`, `/origin-access-control`, `/origin-access-identity`,
/// `/public-key`, `/key-group`, `/field-level-encryption`, `/field-level-encryption-profile`,
/// `/monitoring-subscription`), which carry an ARN of this namespace too.
///
/// What this decides is whether a submitted ARN can be READ as naming a CloudFront resource at
/// all ([`arn_resource_type_is_readable`]) — which is a property of the VALUE — never whether
/// an operation accepts that type, which is each operation's own question
/// ([`resource_policy_resource`], [`TAGGABLE_RESOURCE_TYPES`]).
///
/// Sorted, so the list reads as the SET it is.
pub const CLOUDFRONT_ARN_RESOURCE_TYPES: [&str; 22] = [
    "anycast-ip-list",
    "cache-policy",
    "connection-function",
    "connection-group",
    "continuous-deployment-policy",
    "distribution",
    "distribution-tenant",
    "field-level-encryption-config",
    "field-level-encryption-profile",
    "function",
    "key-group",
    "key-value-store",
    "monitoring-subscription",
    "origin-access-control",
    "origin-access-identity",
    "origin-request-policy",
    "public-key",
    "realtime-log-config",
    "response-headers-policy",
    "streaming-distribution",
    "trust-store",
    RESOURCE_POLICY_RESOURCE_TYPE,
];

/// Whether the resource TYPE `arn` names can be READ at all — the first thing an operation
/// taking the ARN of a resource of this service reads of the value after its form
/// ([`is_readable_arn`]).
///
/// An ARN of THIS service claims to name a CloudFront resource, so its resource part has to
/// spell one of the types this service's namespace writes
/// ([`CLOUDFRONT_ARN_RESOURCE_TYPES`]): a word no kind of CloudFront resource is written under
/// names nothing there is to resolve, which is a value that cannot be READ rather than a
/// resource type an operation declines to cover.
///
/// Everything else is left to whoever resolves the ARN. An ARN of ANOTHER service names a
/// resource of that service's namespace, which this one has no spelling to check it against,
/// and an ARN carrying no type token at all ([`arn_resource_type`]) names no type for this
/// reading to judge — so both are carried through to the operation's own question about WHICH
/// type it covers.
pub fn arn_resource_type_is_readable(arn: &str) -> bool {
    if arn_service(arn) != Some(CLOUDFRONT_ARN_SERVICE) {
        return true;
    }
    arn_resource_type(arn).is_none_or(|kind| CLOUDFRONT_ARN_RESOURCE_TYPES.contains(&kind))
}

/// The `service` segment an ARN of THIS service carries — the model's `arnNamespace`, which is
/// the namespace every ARN [`cloudfront_arn_resource`] reads is written in.
pub const CLOUDFRONT_ARN_SERVICE: &str = "cloudfront";

/// The service an ARN names — its third segment — or `None` where the value carries no such
/// segment.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, so this is the one
/// segment that says WHOSE namespace the rest of the value is written in, read without
/// demanding anything of the resource part ([`cloudfront_arn_resource`] demands a `<type>/<id>`
/// pair and so cannot answer this for a degenerate ARN).
pub fn arn_service(arn: &str) -> Option<&str> {
    let mut segments = arn.splitn(6, ':');
    (segments.next()? == "arn").then_some(())?;
    segments.nth(1)
}

/// The sentence the OWNED-RESOURCE listing refuses a well-formed `ResourceArn` naming a
/// resource TYPE it does not cover with: the whole text is fixed — it names the parameter
/// that is wrong and the part of it that is wrong — so nothing of the request or of the store
/// reaches it. Recorded behaviour.
pub const INVALID_RESOURCE_TYPE_PARAMETER_MESSAGE: &str =
    "Invalid parameter received. Resource Type";

/// The modelled `InvalidArgument` (HTTP 400) the owned-resource listing refuses an ARN whose
/// resource part names a type other than the shareable one with — "An argument is invalid."
///
/// Decided from the PARSED ARN, before it is resolved against the store: the type a
/// `ResourceArn` names is a property of the value submitted, so an ARN of an unsupported type
/// is refused the same way whether or not this service holds anything it could name — which
/// is what keeps the absent answer ([`resource_policy_resource_absent`]) for an ARN of the
/// type this listing does cover.
pub fn invalid_resource_type_parameter() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESOURCE_TYPE_PARAMETER_MESSAGE.to_string()),
    }
}

/// Whether `value` can be READ as an ARN at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`
/// ([`cloudfront_arn_resource`] reads one of THIS service), so a value that can be read as one
/// opens with [`ARN_PREFIX`] and carries all six segments, with a partition, a service and a
/// resource part that name something; the region and account segments are empty on the ARNs of
/// a global service, so neither is a value this reading demands.
///
/// This is ALL the FORM of an ARN decides — which service it names, and what its resource part
/// spells, is a question for whoever resolves it ([`resource_policy_resource`]) — and it is
/// what an operation taking an ARN in one of its members reads that member with before the
/// value is used for anything.
pub fn is_readable_arn(value: &str) -> bool {
    let mut segments = value.splitn(6, ':');
    segments.next() == Some("arn")
        && segments.next().is_some_and(|partition| !partition.is_empty())
        && segments.next().is_some_and(|service| !service.is_empty())
        && segments.next().is_some()
        && segments.next().is_some()
        && segments.next().is_some_and(|resource| !resource.is_empty())
}

/// The sentence CloudFront refuses an input MEMBER whose value its own argument validation
/// cannot read with, for `member_path` — the path of that member in the request, spelled from
/// the enclosing shapes inwards and separated by spaces (`VpcOriginEndpointConfig Arn`).
///
/// Fixed text around one variable part: "The parameter " and " ValidationError." are the
/// service's own words for a member that failed validation, and the member path is taken from
/// the REQUEST — it is what tells a caller which of its values to fix. Nothing else of the
/// request reaches the sentence: neither the value that failed nor anything the store holds.
/// Recorded behaviour.
pub fn parameter_validation_error_message(member_path: &str) -> String {
    format!("The parameter {member_path} ValidationError.")
}

/// The modelled `InvalidArgument` (HTTP 400) for an input member whose value fails CloudFront's
/// argument validation — "An argument is invalid."
///
/// Raised BEFORE the request is acted on, so a create that carries such a member mints nothing
/// and persists nothing: the refusal is a property of the value submitted, decided without
/// asking what the store holds.
pub fn parameter_validation_error(member_path: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(parameter_validation_error_message(member_path)),
    }
}

/// The sentence CloudFront refuses an input MEMBER whose value is not even in the FORM that
/// member takes with, for `member_path` — the same member path
/// [`parameter_validation_error_message`] names.
///
/// Two conditions, two sentences: this one says the value could not be READ as what the member
/// holds (a string that is no ARN at all, in a member that takes an ARN), and the other says a
/// readable value did not pass that member's validation. Fixed text around the one variable
/// part, as the other is. Recorded behaviour.
pub fn parameter_invalid_format_message(member_path: &str) -> String {
    format!("The parameter {member_path} Invalid Format.")
}

/// The modelled `InvalidArgument` (HTTP 400) for an input member whose value is not in the form
/// that member takes — "An argument is invalid."
pub fn parameter_invalid_format(member_path: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(parameter_invalid_format_message(member_path)),
    }
}

/// The sentence an ASSOCIATION refuses a web ACL CloudFront may not be put behind with:
/// the whole text is fixed — it states that the web ACL named is not one for CloudFront and
/// what to name instead — so nothing of the request reaches it. Recorded behaviour.
pub const INVALID_WEB_ACL_MESSAGE: &str = "The specified web ACL isn't valid. Specify a web \
     ACL that was previously validated and try again.";

/// The modelled `InvalidArgument` (HTTP 400) an association naming a web ACL outside the
/// namespace CloudFront is protected by is refused with — "An argument is invalid."
pub fn invalid_web_acl() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_WEB_ACL_MESSAGE.to_string()),
    }
}

/// Whether `web_acl_arn` names a web ACL a distribution — or a distribution tenant — may
/// be put behind.
///
/// CloudFront is protected by the web ACLs of ONE WAFv2 namespace: the `Scope` value
/// `CLOUDFRONT` is the one "for a global resource type, such as a Amazon CloudFront
/// distribution", and a web ACL of the REGIONAL namespace protects regional resources, so
/// WAF would never enforce it in front of a distribution. The namespace a web ACL belongs
/// to is carried in its own ARN, which is read by the service that mints it
/// (`service_com_amazonaws_wafv2_awswaf_20190729::state::arn_scope`), so this service holds
/// no second reading of a WAFv2 ARN: a value that IS one is associable exactly where it
/// names a WEB ACL of the global namespace.
///
/// A value that is no WAFv2 resource ARN at all is not judged here. `WebACLId` carries
/// either generation — "to specify a web ACL created using the latest version of WAF, use
/// the ACL ARN … To specify a web ACL created using WAF Classic, use the ACL ID" — so a
/// WAF Classic identifier names no WAFv2 namespace to be outside of, and whether a value
/// that is no identifier at all was accepted is a different question about the same member.
pub fn associable_web_acl_arn(web_acl_arn: &str) -> bool {
    let Some(scope) = ::service_com_amazonaws_wafv2_awswaf_20190729::state::arn_scope(web_acl_arn)
    else {
        return true;
    };
    scope == ::service_com_amazonaws_wafv2_awswaf_20190729::state::SCOPE_CLOUDFRONT
        && ::service_com_amazonaws_wafv2_awswaf_20190729::state::arn_resource_kind(web_acl_arn)
            == Some(::service_com_amazonaws_wafv2_awswaf_20190729::state::WEB_ACL_ARN_KIND)
}

/// Why the resource-policy surface refuses `arn`, or `None` when it names a resource the
/// surface attaches policies to.
///
/// THREE conditions, in the order the service decides them — the ARGUMENT is judged from the
/// value the request carried, and only an argument the surface can act on is resolved against
/// the store:
///
/// * [`ResourceArnRefusal::Malformed`] — a value that is no ARN at all cannot be read as
///   one, so it is refused on its FORM. What "no ARN at all" is, is the condition the
///   sentence for it states ([`MALFORMED_ARN_MESSAGE`]): the value does not OPEN with
///   [`ARN_PREFIX`];
/// * [`ResourceArnRefusal::UnreadableType`] — a value that DOES open with it is read for the
///   resource it names, and an ARN of this service whose resource part spells a type
///   CloudFront writes no resource under ([`arn_resource_type_is_readable`]) names nothing
///   there is to resolve: the type is a property of the value, so this too is a value that
///   cannot be READ rather than a type an operation declines to cover;
/// * [`ResourceArnRefusal::WrongType`] — an ARN whose resource part names anything other
///   than [`RESOURCE_POLICY_RESOURCE_TYPE`] ([`resource_policy_resource`]) is refused on the
///   resource TYPE it names, whether or not this service holds anything for it: the surface
///   attaches policies to ONE resource type, so what a caller has to fix is the type its
///   parameter named, which the parsed ARN alone decides. A degenerate ARN — one carrying no
///   resource part, or none this service's namespace spells — names no accepted type and is
///   this refusal rather than a second form refusal this service has no recorded sentence
///   for;
/// * [`ResourceArnRefusal::AbsentResource`] — an ARN of the accepted type naming no resource
///   this service holds ([`ComAmazonawsCloudfrontCloudfront20200531State::holds_resource`])
///   is reported ABSENT: a policy is attached TO a resource, so the resource has to exist
///   first.
///
/// Whether the resource that exists carries a policy is NOT one of these conditions: a
/// policy is a property OF the resource, so a resource of the accepted type that carries
/// none reads as the empty document ([`EMPTY_RESOURCE_POLICY_DOCUMENT`]) and its removal is
/// a no-op — there is no absent POLICY to report once the resource is there.
///
/// ONE rule for every operation that takes the one `ResourceArn`, so no two of them can
/// disagree about which values they accept. The SENTENCE each puts on the
/// wire is the service's own per operation: see [`malformed_arn`],
/// [`invalid_resource_policy_resource_type`], [`invalid_resource_policy_arn`],
/// [`invalid_arn_resource_type_syntax`] and [`resource_policy_resource_absent`].
pub fn resource_policy_arn_refusal(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    arn: &str,
) -> Option<ResourceArnRefusal> {
    if !arn.starts_with(ARN_PREFIX) {
        return Some(ResourceArnRefusal::Malformed);
    }
    if !arn_resource_type_is_readable(arn) {
        return Some(ResourceArnRefusal::UnreadableType);
    }
    if resource_policy_resource(arn).is_none() {
        return Some(ResourceArnRefusal::WrongType);
    }
    if !store.holds_resource(arn) {
        return Some(ResourceArnRefusal::AbsentResource);
    }
    None
}

/// Why a `ResourceArn` names no resource the resource-policy surface attaches policies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnRefusal {
    /// The value is no ARN at all: it does not open with [`ARN_PREFIX`].
    Malformed,
    /// The value is an ARN of this service whose resource part spells a resource type
    /// CloudFront writes no resource under ([`CLOUDFRONT_ARN_RESOURCE_TYPES`]), so the type
    /// it names cannot be read at all — a property of the value, like [`Self::Malformed`].
    UnreadableType,
    /// The value is an ARN whose resource part names a type other than
    /// [`RESOURCE_POLICY_RESOURCE_TYPE`] — a property of the ARN alone, so this is the answer
    /// whether or not this service holds a resource of that type.
    WrongType,
    /// The value is an ARN of the accepted type, and this service holds no resource it names.
    AbsentResource,
}

/// The modelled `EntityNotFound` (HTTP 404) a resource-policy call naming — under the
/// resource type the surface accepts — a resource this service holds none of is refused with
/// — "The entity was not found."
///
/// The sentence is the one CloudFront answers for the class ([`NO_SUCH_RESOURCE_MESSAGE`]):
/// it states that the resource asked about is not there and names no part of the request, so
/// nothing in it is built out of the call.
pub fn resource_policy_resource_absent() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_RESOURCE_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy write that would grow the
/// store past [`MAX_RESOURCE_POLICIES`] is refused with, beside the other refusals of this
/// surface rather than built at its one call site.
///
/// The BOUND is this emulator's, so the sentence is too: real CloudFront bounds the
/// policies an account can hold through the quota on the resources they are attached to and
/// models no limit error on this operation at all.
pub fn too_many_resource_policies() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "This emulator holds resource policies for at most {MAX_RESOURCE_POLICIES} \
             resources; delete one before attaching another"
        )),
    }
}

/// Whether two domain names are the same name. DNS names are case-insensitive, so the
/// comparison is too — a domain claimed in one case is claimed in every case.
pub fn same_domain(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Why `domain` is no domain name a CloudFront resource can be reached at, or `None` when
/// it is one.
///
/// `DomainItem$Domain` is modelled as a plain `string`, so the shape of a DNS name is
/// this check rather than a modelled constraint: a name is a dot-separated sequence of at
/// least two non-empty labels, each at most [`MAX_DOMAIN_LABEL_LEN`] characters and
/// carrying no whitespace, and the whole name is at most [`MAX_DOMAIN_NAME_LEN`]
/// characters (RFC 1035). The character set of a label is deliberately NOT narrowed to
/// ASCII: an internationalised domain name is a domain name.
///
/// ONE rule for every operation that associates a domain with a resource, so the create
/// and a later association cannot drift apart. The reason carries no error wording of its
/// own: the caller prefixes it with the wording of whichever refusal its own model
/// declares.
pub fn domain_name_refusal(domain: &str) -> Option<String> {
    if domain.is_empty() {
        return Some("a domain name cannot be empty".to_string());
    }
    if domain.chars().count() > MAX_DOMAIN_NAME_LEN {
        return Some(format!(
            "the domain {domain} is longer than the {MAX_DOMAIN_NAME_LEN} characters a domain \
             name may carry"
        ));
    }
    // The root label a fully qualified name ends in carries nothing, so a trailing dot is
    // not a second, empty label.
    let labels: Vec<&str> = domain.trim_end_matches('.').split('.').collect();
    if labels.len() < 2 {
        return Some(format!(
            "the domain {domain} is a single label, not a domain name"
        ));
    }
    for label in labels {
        if label.is_empty() {
            return Some(format!("the domain {domain} carries an empty label"));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_LEN {
            return Some(format!(
                "the domain {domain} carries a label longer than the {MAX_DOMAIN_LABEL_LEN} \
                 characters a label may carry"
            ));
        }
        if label.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Some(format!(
                "the domain {domain} carries whitespace, which no domain name does"
            ));
        }
    }
    None
}

/// `DistributionResourceType`'s value for a standard or multi-tenant DISTRIBUTION, as the
/// model's enum spells it (`crate::ops::types::DISTRIBUTION_RESOURCE_TYPE_VALUES`, which
/// `smithy_query(query="shape",shape="DistributionResourceType")` reports the same values
/// for).
pub const DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION: &str = "distribution";

/// `DistributionResourceType`'s value for a DISTRIBUTION TENANT — the other value the
/// model's enum admits beside [`DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION`].
pub const DISTRIBUTION_RESOURCE_TYPE_TENANT: &str = "distribution-tenant";

/// The key one reported domain conflict is ordered and deduplicated by: the conflicting
/// domain, compared as DNS names are, and then the id of the resource holding it.
///
/// A domain can be associated with more than one resource in a store this service never
/// refused the second association to, so the domain alone is no key; the pair is, because
/// one resource holds one domain once.
pub(super) fn conflict_order(conflict: &DomainConflict) -> (String, String) {
    (
        conflict.domain.to_lowercase(),
        conflict.resource_id.clone(),
    )
}

/// Whether the alternate domain name `pattern` is a WILDCARD name that covers `name`.
///
/// A CloudFront wildcard alternate domain name replaces the whole leftmost label of a
/// name: "if you provide `*.example.com` as input, the returned list can include ... any
/// alternate domain names covered by that wildcard (for example, `www.example.com`,
/// `test.example.com`, `dev.example.com`, and so on)". So a wildcard covers exactly the
/// names that carry ONE label in place of the `*`, a name that is not a wildcard covers
/// nothing but itself, and a wildcard does not cover another wildcard. The suffix is
/// compared as DNS names are ([`same_domain`]).
pub fn wildcard_covers(pattern: &str, name: &str) -> bool {
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    let Some((label, rest)) = name.split_once('.') else {
        return false;
    };
    !label.is_empty() && label != "*" && same_domain(rest, suffix)
}

/// Whether the domain association `held` conflicts with the domain `requested`.
///
/// "Lists existing domain associations that conflict with the domain that you specify":
/// two names conflict when CloudFront cannot route them separately — they are the same
/// name, or one of them is a wildcard alternate domain name that covers the other, which
/// is why the relation is symmetric. ONE rule for every surface that reports or refuses an
/// overlapping domain.
pub fn domains_conflict(held: &str, requested: &str) -> bool {
    same_domain(held, requested)
        || wildcard_covers(held, requested)
        || wildcard_covers(requested, held)
}

/// Whether a certificate that covers the names in `covered` covers `domain`.
///
/// A certificate covers the names it was issued over, and a wildcard name among them
/// covers every name under it ([`wildcard_covers`]) — the direction is what separates this
/// from [`domains_conflict`]: a certificate for one name does not cover the wildcard that
/// covers that name.
pub fn certificate_covers<'a, I>(covered: I, domain: &str) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    covered
        .into_iter()
        .any(|name| same_domain(name, domain) || wildcard_covers(name, domain))
}

/// Why `customizations` is no set of customizations a distribution tenant can carry, or
/// `None` when it is one.
///
/// A tenant's customizations are "specific values that you can override or disable from
/// the multi-tenant distribution", and the only list among them is the geographic
/// restriction's locations, bounded by [`MAX_GEO_RESTRICTION_LOCATIONS`] — so no caller
/// chooses how much work one customization costs, nor how large the record that retains it
/// is. Every other member is a single value the model already constrains.
///
/// ONE rule for every operation that accepts a tenant's customizations. The reason carries
/// no error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn customizations_refusal(customizations: &Customizations) -> Option<String> {
    let locations = customizations
        .geo_restrictions
        .as_ref()
        .and_then(|geo| geo.locations.as_deref())
        .unwrap_or_default();
    if locations.len() > MAX_GEO_RESTRICTION_LOCATIONS {
        return Some(format!(
            "a geographic restriction names at most {MAX_GEO_RESTRICTION_LOCATIONS} locations \
             but the request names {}",
            locations.len()
        ));
    }
    None
}

/// The refusal `parameters` earns against the `TenantConfig$ParameterDefinitions` of the
/// multi-tenant distribution a tenant is created or updated under, or `None` when the list
/// satisfies them.
///
/// "A parameter is specified as a key-value pair. A valid parameter value must exist for
/// any parameter that is marked as required in the multi-tenant distribution" — so three
/// things make a parameter list invalid: a value for a parameter the distribution defines
/// nothing for, one parameter named twice (which describes no single value), and a
/// definition marked `Required` for which neither the request nor the definition's own
/// `DefaultValue` supplies a VALID value — a value being valid means a non-empty one, so
/// an empty string satisfies a required parameter no more than an absent one does.
///
/// A definition carrying no `StringSchema` states no `Required` flag at all
/// (`ParameterDefinitionSchema`'s only member is optional), so there is nothing to require
/// of the request for it.
///
/// ONE rule for every operation that supplies a tenant's parameter values, and ONE producer
/// of the refusal each half of it is reported with: the two halves that describe no parameter
/// LIST state the constraint the request broke after the error shape's own sentence
/// ([`invalid_tenant_parameters`]), while a required parameter left without a value is the
/// condition this service has a fixed sentence of its own for
/// ([`missing_tenant_parameter`]) — so the whole message is that sentence, and no operation
/// prefixes anything onto it.
pub fn parameter_refusal(
    definitions: &[ParameterDefinition],
    parameters: &[Parameter],
) -> Option<crate::ops::types::InvalidArgument> {
    // The definitions are INDEXED once, so the check costs one logarithmic lookup per
    // submitted value rather than a scan of the definitions per value: its cost is the
    // SUM of the two lengths, never their product.
    let defined: BTreeMap<&str, Option<&StringSchemaConfig>> = definitions
        .iter()
        .map(|definition| {
            (
                definition.name.as_str(),
                definition.definition.string_schema.as_ref(),
            )
        })
        .collect();
    let mut submitted: BTreeMap<&str, &str> = BTreeMap::new();
    for parameter in parameters {
        if submitted
            .insert(parameter.name.as_str(), parameter.value.as_str())
            .is_some()
        {
            return Some(invalid_tenant_parameters(format!(
                "the parameter {} is named more than once",
                parameter.name
            )));
        }
        if !defined.contains_key(parameter.name.as_str()) {
            return Some(invalid_tenant_parameters(format!(
                "the distribution defines no parameter named {}",
                parameter.name
            )));
        }
    }
    for (name, schema) in &defined {
        let Some(schema) = schema else {
            continue;
        };
        let supplied = submitted.get(name).is_some_and(|value| !value.is_empty())
            || schema
                .default_value
                .as_deref()
                .is_some_and(|value| !value.is_empty());
        if schema.required && !supplied {
            return Some(missing_tenant_parameter());
        }
    }
    None
}

/// The modelled `InvalidArgument` (HTTP 400) for a tenant request that supplies no value for
/// a parameter the multi-tenant distribution requires.
///
/// The sentence is this service's fixed text for the condition
/// ([`MISSING_TENANT_PARAMETER_MESSAGE`]): WHICH parameter a request has to supply is a
/// property of the distribution's own `TenantConfig$ParameterDefinitions`, which the caller
/// reads from the distribution, so the refusal states the condition and takes nothing from
/// the request — every refusal of it reads the same, exactly as
/// [`missing_certificate_for_domain`]'s does.
pub fn missing_tenant_parameter() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MISSING_TENANT_PARAMETER_MESSAGE.to_string()),
    }
}

/// The message a tenant request that leaves a parameter the distribution requires without a
/// value is refused with, on the wire.
///
/// Fixed text: it interpolates nothing, and in particular never names the parameter or the
/// distribution.
pub const MISSING_TENANT_PARAMETER_MESSAGE: &str =
    "A parameter referenced in the multi-tenant distribution configuration is missing. \
     Specify the parameter and try again.";

/// The modelled `InvalidArgument` (HTTP 400) for a submitted parameter list that describes no
/// list of values at all — one name twice, or a name the distribution defines nothing for —
/// worded as the model documents the shape ("An argument is invalid.") followed by the
/// constraint the request broke, which is how this crate words that family of refusal
/// ([`invalid_realtime_log_config`] words its own the same way).
fn invalid_tenant_parameters(reason: String) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!("An argument is invalid: {reason}")),
    }
}

/// The path of one origin access identity under this service's API version — the route
/// `GetCloudFrontOriginAccessIdentity` serves (`/origin-access-identity/cloudfront/{Id}`),
/// which is therefore the resource the create's `Location` names.
pub fn origin_access_identity_path(id: &str) -> String {
    format!("/2020-05-31/origin-access-identity/cloudfront/{id}")
}

/// The value a distribution's configuration carries to attach the origin access identity
/// `id` to an Amazon S3 origin.
///
/// `S3OriginConfig$OriginAccessIdentity` documents the form exactly — "The format of the
/// value is: `origin-access-identity/cloudfront/ID-of-origin-access-identity`", where the
/// id is "the value that CloudFront returned in the `ID` element when you created the
/// origin access identity" — and `S3Origin$OriginAccessIdentity` names the same identity
/// the same way, so both are built here rather than spelled out at each use site.
pub fn origin_access_identity_reference(id: &str) -> String {
    format!("origin-access-identity/cloudfront/{id}")
}

/// The path of a response headers policy resource under this service's API version. The
/// resource is addressed by ID, as `GetResponseHeadersPolicy`'s `{Id}` label is.
pub fn response_headers_policy_path(id: &str) -> String {
    format!("/2020-05-31/response-headers-policy/{id}")
}

/// The path of an origin access control resource under this service's API version — the
/// route `GetOriginAccessControl` serves (`/origin-access-control/{Id}`), which is
/// therefore the resource the create's `Location` names.
pub fn origin_access_control_path(id: &str) -> String {
    format!("/2020-05-31/origin-access-control/{id}")
}

/// The path of an origin request policy resource under this service's API version. The
/// resource is addressed by ID, as `GetOriginRequestPolicy`'s `{Id}` label is.
pub fn origin_request_policy_path(id: &str) -> String {
    format!("/2020-05-31/origin-request-policy/{id}")
}

/// The path of a cache policy resource under this service's API version. The resource is
/// addressed by ID, as `GetCachePolicy`'s `{Id}` label is.
pub fn cache_policy_path(id: &str) -> String {
    format!("/2020-05-31/cache-policy/{id}")
}

/// The path of a public key resource under this service's API version. The resource is
/// addressed by ID, as `GetPublicKey`'s `{Id}` label is.
pub fn public_key_path(id: &str) -> String {
    format!("/2020-05-31/public-key/{id}")
}

/// The path of a key group resource under this service's API version — the route
/// `GetKeyGroup` serves (`/key-group/{Id}`), which is therefore the resource the create's
/// `Location` names.
pub fn key_group_path(id: &str) -> String {
    format!("/2020-05-31/key-group/{id}")
}

/// The path of a field-level encryption profile resource under this service's API version.
/// The resource is addressed by ID, as `GetFieldLevelEncryptionProfile`'s `{Id}` label is.
pub fn field_level_encryption_profile_path(id: &str) -> String {
    format!("/2020-05-31/field-level-encryption-profile/{id}")
}

/// The path of a field-level encryption configuration resource under this service's API
/// version — the route `GetFieldLevelEncryption` serves (`/field-level-encryption/{Id}`),
/// which is therefore the resource the create's `Location` names.
pub fn field_level_encryption_config_path(id: &str) -> String {
    format!("/2020-05-31/field-level-encryption/{id}")
}

/// Whether `value` has the shape of a distribution id this service mints (`E` followed
/// by [`ID_BODY_LEN`] uppercase alphanumerics).
///
/// `ListDistributions`'s `Marker` is a distribution id, and the model constrains it no
/// further than `string`, so this is what tells a resume point the service issued from
/// a value a client made up.
pub fn is_distribution_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of a streaming distribution id this service mints.
///
/// `ListStreamingDistributions`'s `Marker` is "the value you can use for the `Marker`
/// request parameter to continue listing your RTMP distributions where they left off",
/// which is a streaming distribution id this service handed out, and the model constrains
/// it no further than `string` — so this is what tells a resume point the service issued
/// from a value a client made up. The same shape [`is_distribution_id`] describes, because
/// CloudFront draws every distribution id from it.
pub fn is_streaming_distribution_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of a connection group id this service mints
/// ([`CONNECTION_GROUP_ID_PREFIX`] followed by an opaque base62 body of
/// [`OPAQUE_ID_BODY_LEN`] characters, [`is_opaque_id_body`]).
///
/// `ListConnectionGroups`'s `Marker` is "the marker for the next set of connection groups
/// to retrieve", which is a value this service handed out, and the model constrains it no
/// further than `string` — so this is what tells a resume point the service issued from a
/// value a client made up.
pub fn is_connection_group_id(value: &str) -> bool {
    value
        .strip_prefix(CONNECTION_GROUP_ID_PREFIX)
        .is_some_and(is_opaque_id_body)
}

/// Whether `value` has the shape of a distribution tenant id this service MINTS
/// ([`DISTRIBUTION_TENANT_ID_PREFIX`] followed by an opaque base62 body of
/// [`OPAQUE_ID_BODY_LEN`] characters, [`is_opaque_id_body`]).
///
/// A distribution tenant listing's `Marker` is "the marker for the next set of results",
/// which is a tenant id this service handed out, and the model constrains it no further
/// than `string` — so this is what tells a resume point the service issued from a value a
/// client made up.
///
/// NOT what an operation that ADDRESSES a tenant reads its path identifier with: the width
/// and the alphabet here are this mint's, not CloudFront's, so refusing an identifier on
/// them would answer an argument failure for a tenant that is merely absent. That decision
/// is [`is_namespaced_resource_id`]'s.
pub fn is_distribution_tenant_id(value: &str) -> bool {
    value
        .strip_prefix(DISTRIBUTION_TENANT_ID_PREFIX)
        .is_some_and(is_opaque_id_body)
}

/// The longest a namespace label may be in an identifier [`is_namespaced_resource_id`]
/// admits.
///
/// The labels CloudFront writes these ids under are short: two letters for a distribution
/// tenant ([`DISTRIBUTION_TENANT_ID_PREFIX`]) or a connection group
/// ([`CONNECTION_GROUP_ID_PREFIX`]), three for an Anycast static IP list
/// ([`ANYCAST_IP_LIST_ID_PREFIX`]) — so this is the longest label any id of this service's
/// resources carries, and a longer word followed by a separator is not a namespace at all.
pub(super) const MAX_ID_NAMESPACE_LABEL_LEN: usize = 3;

/// Whether `value` could be an opaque identifier CloudFront issued for a namespaced
/// resource: a lowercase namespace label of at most [`MAX_ID_NAMESPACE_LABEL_LEN`] letters,
/// a separator (`_` or `-`), and a non-empty body of identifier characters (ASCII
/// alphanumerics and those same separators).
///
/// This is ALL a request alone decides about such an identifier, and it is what EVERY
/// operation that addresses one by a path member reads it with — the Anycast static IP list's
/// (`GetAnycastIpList$Id`, `UpdateAnycastIpList$Id`) as much as the distribution tenant's
/// (`DeleteDistributionTenant$Id`, …), so no two routes of the family disagree about what an
/// identifier IS. Those members are plain `string` in the
/// model — no `@pattern`, no `@length` — so neither the label, nor the width of the body,
/// nor its alphabet is a constraint this service may refuse a value on: the ids it MINTS are
/// one form of a namespace, never the only form CloudFront writes one in. Whether an
/// identifier of this form was ever ISSUED, and to which resource, is a property of the
/// STORE — so a value of this form that nothing answers to is a resource that is ABSENT (the
/// operation's not-found code, status and fixed sentence), while a value carrying NO namespace
/// at all — a bare word, a run of characters around no separator, a UUID, an identifier of a
/// kind whose ids are not namespaced — was never an identifier of such a resource and is the
/// identifier that cannot be read at all ([`invalid_id_format`]).
pub fn is_namespaced_resource_id(value: &str) -> bool {
    let Some(separator) = value.find(['_', '-']) else {
        return false;
    };
    let (label, rest) = value.split_at(separator);
    let body = &rest[1..];
    !label.is_empty()
        && label.len() <= MAX_ID_NAMESPACE_LABEL_LEN
        && label.bytes().all(|b| b.is_ascii_lowercase())
        && !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `value` is written in the characters CloudFront writes an ENTITY IDENTIFIER in:
/// a non-empty run of the id alphabet ([`is_token_byte`] — decimal digits and uppercase
/// ASCII letters), carrying no separator and no lower case.
///
/// The ALPHABET alone, with no namespace letter and no width — so this says whether a value
/// is WRITTEN as an identifier of this service's mint, not whether that identifier could have
/// been issued. The members that carry one are plain `string` `@httpLabel`s in the model — no
/// `@pattern`, no `@length` — so what each route does with the answer is that resource kind's
/// own recorded contract (see the `path_identifiers` module): a route that refuses an
/// identifier on its form refuses it only within this alphabet
/// ([`id_format_refused_for`]), because a value of other characters makes no claim to be an
/// identifier of this mint at all, while a route that refuses no form at all carries every
/// value through to the store.
pub fn is_entity_id_form(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_token_byte)
}

/// Whether `value` has the shape of a key group id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_key_group_id`] draws a UUID).
///
/// A key group id is a UUID, whose form is fully specified — five hex groups of fixed width
/// — so unlike a namespaced id ([`is_namespaced_resource_id`]) the BODY is what says the
/// value could be one at all, and this predicate is what a read of a key group reads its
/// `Id` with.
pub fn is_key_group_id(value: &str) -> bool {
    is_minted_uuid(value)
}

/// Whether `value` has the shape of a continuous deployment policy id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_continuous_deployment_policy_id`]
/// draws a UUID), for the reason [`is_key_group_id`] exists.
pub fn is_continuous_deployment_policy_id(value: &str) -> bool {
    is_minted_uuid(value)
}

/// Whether `value` has the shape of a version token (`ETag`) this service mints.
///
/// `If-Match` carries one, and the model constrains it no further than `string`: a value
/// outside this shape was never a distribution version, which is a different failure
/// from a version that is simply no longer current.
///
/// DELIBERATELY the same predicate as [`is_distribution_id`]: CloudFront draws a
/// distribution id and a version token from one shape (`E` + 13), so the two namespaces
/// are indistinguishable by shape alone and nothing here can tell them apart either.
/// What separates them is the comparison that follows — a token is the CURRENT version
/// only if it equals the one stored.
pub fn is_version_token(value: &str) -> bool {
    is_distribution_id(value)
}

/// Whether `value` has the shape of an origin access control id this service mints.
///
/// `ListOriginAccessControls`'s `Marker` is an origin access control id — "set this
/// field's value to the value of `NextMarker` from the current page's response" — and the
/// model constrains it no further than `string`, so this is what tells a resume point the
/// service issued from a value a client made up. The same shape
/// [`is_distribution_id`] describes, because CloudFront draws every entity id from it.
pub fn is_origin_access_control_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of an origin access identity id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_origin_access_identity_id`]).
///
/// `ListCloudFrontOriginAccessIdentities`'s `Marker` is an origin access identity id —
/// "set the `Marker` to the value of the `NextMarker` from the current page's response
/// (which is also the ID of the last identity on that page)" — and the model constrains it
/// no further than `string`, so this is what tells a resume point the service issued from a
/// value a client made up. The same shape [`is_distribution_id`] describes, because
/// CloudFront draws every entity id of this family from it.
pub fn is_origin_access_identity_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape every id this service MINTS AS A UUID takes
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`]): the 8-4-4-4-12
/// lowercase hex of a UUID.
///
/// Nothing here is specific to one resource kind — a key value store, a response headers
/// policy and a continuous deployment policy all carry an id of this shape — so one
/// predicate serves every listing whose `Marker` is such an id.
/// `ListContinuousDeploymentPolicies`'s `Marker` is a policy id and the model constrains
/// it no further than `string`, so this is what tells a resume point the service issued
/// from a value a client made up.
pub fn is_minted_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
}

/// Whether `value` has the shape of a public key id this service mints
/// ([`PUBLIC_KEY_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters of the id alphabet).
///
/// `ListPublicKeys`'s `Marker` is a public key id — "set the Marker to the value of the
/// NextMarker from the current page's response (which is also the ID of the last public
/// key on that page)" — and the model constrains it no further than `string`, so this is
/// what tells a resume point the service issued from a value a client made up.
pub fn is_public_key_id(value: &str) -> bool {
    is_prefixed_token(value, PUBLIC_KEY_ID_PREFIX)
}

/// Whether `value` has the shape of a field-level encryption CONFIGURATION id this service
/// mints ([`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters
/// of the id alphabet).
///
/// `ListFieldLevelEncryptionConfigs`'s `Marker` is a configuration id — "set the Marker to
/// the value of the NextMarker from the current page's response (which is also the ID of the
/// last configuration on that page)" — and the model constrains it no further than `string`,
/// so this is what tells a resume point the service issued from a value a client made up.
pub fn is_field_level_encryption_config_id(value: &str) -> bool {
    is_prefixed_token(value, FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX)
}

/// Whether `value` has the shape of a field-level encryption profile id this service mints
/// ([`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters of
/// the id alphabet).
///
/// `ListFieldLevelEncryptionProfiles`'s `Marker` is a profile id — "set the Marker to the
/// value of the NextMarker from the current page's response (which is also the ID of the
/// last profile on that page)" — and the model constrains it no further than `string`, so
/// this is what tells a resume point the service issued from a value a client made up.
pub fn is_field_level_encryption_profile_id(value: &str) -> bool {
    is_prefixed_token(value, FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX)
}

/// The longest name a key value store carries, and the length its own name constraint
/// bounds every store name by.
///
/// `KeyValueStoreName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` (from
/// `smithy_query` on `CreateKeyValueStoreRequest$Name`), so this is the model's bound
/// rather than a number this emulator chose.
pub const MAX_KEY_VALUE_STORE_NAME: usize = 64;

/// Whether `value` is a name a key value store of this account can be held under: the
/// `KeyValueStoreName` constraint, `length 1..`[`MAX_KEY_VALUE_STORE_NAME`] over the
/// characters its pattern admits (ASCII letters, digits, `-` and `_`).
///
/// `ListKeyValueStores`'s `Marker` is the name the previous page ended at, and the model
/// constrains that parameter no further than `string`, so this is what tells a resume
/// point this service handed out from a value a client made up.
pub fn is_key_value_store_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_KEY_VALUE_STORE_NAME
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `value` is `prefix` followed by [`ID_BODY_LEN`] characters of the id
/// alphabet.
pub(super) fn is_prefixed_token(value: &str, prefix: char) -> bool {
    match value.strip_prefix(prefix) {
        Some(body) => is_token_body(body),
        None => false,
    }
}

/// The namespace letter CloudFront writes the identifiers of the SHARED-form family under —
/// a distribution, a streaming distribution, an origin access identity and an origin access
/// control — and the letter this service mints each of them with
/// (`allocate_distribution_id` and its siblings).
pub const ENTITY_ID_PREFIX: char = 'E';

/// The namespace letters this service MINTS an entity identifier of a `prefix` + body shape
/// under, each one the letter of exactly one resource kind: [`ENTITY_ID_PREFIX`] for the
/// shared-form family, [`PUBLIC_KEY_ID_PREFIX`] for a public key,
/// [`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`] for a field-level encryption profile and
/// [`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`] for a field-level encryption configuration.
///
/// A letter in this set says WHICH namespace the identifier that carries it was issued in; a
/// letter outside it says nothing about the kind, because CloudFront's namespaces are not
/// this emulator's mint ([`id_format_refused_for`]).
pub const ID_NAMESPACE_LETTERS: [char; 4] = [
    ENTITY_ID_PREFIX,
    PUBLIC_KEY_ID_PREFIX,
    FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX,
    FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX,
];

/// Whether `value` is written under one of the namespace letters this service issues
/// identifiers under ([`ID_NAMESPACE_LETTERS`]) — all a REQUEST alone says about WHICH
/// namespace a value came from.
///
/// A value that identifies no namespace of this service says nothing about which kind it
/// was issued for, so it is carried through to the STORE; one that identifies a namespace
/// came from that namespace, and whether it could name the addressed kind is then decided by
/// the namespace itself ([`id_format_refused_for`]) or, where several kinds share one letter,
/// by the store.
pub fn identifies_a_namespace(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|letter| ID_NAMESPACE_LETTERS.contains(&letter))
}

/// How many characters CloudFront writes an entity identifier in: the namespace letter the
/// kind issues under over a body of [`ID_BODY_LEN`], the width every identifier this service
/// mints that way carries (`allocate_field_level_encryption_config_id` and its siblings).
pub const ISSUED_ENTITY_ID_LEN: usize = ID_BODY_LEN + 1;

/// Whether a route addressed by an identifier of the kind this service mints under `prefix`
/// refuses `value` on its FORM rather than resolving it against the store — all a REQUEST
/// alone decides about such an identifier, and the whole of what separates the two sentences
/// the one not-found code carries.
///
/// A route refuses the form of every value that could never be an identifier CloudFront issued
/// FOR THIS KIND, which the value alone says in two ways:
///
/// * it is written OUTSIDE the characters CloudFront writes an identifier in
///   ([`is_entity_id_form`] — decimal digits and uppercase ASCII letters, carrying no separator
///   and no lower case): in lower case, around a separator or as a word of other characters, it
///   claims to be no identifier of this mint at all, so there is nothing in it for the service
///   to resolve;
/// * or it IS an identifier of ANOTHER kind of this service
///   ([`is_issued_for_another_kind`]): written exactly the way an identifier is ISSUED — that
///   alphabet at the issued width — under the NAMESPACE LETTER of another kind this service
///   issues identifiers under, so no record of the addressed kind could ever answer to it.
///
/// Either way the route reports the identifier it cannot READ ([`invalid_id_format`]).
///
/// A value written in the OTHER form CloudFront issues an identifier in — the UUID
/// ([`is_minted_uuid`]) — is not one of those: that form carries no namespace marker at all and
/// is SHARED by every kind identified by one (a key group, the policy kinds, a key value
/// store), so it says which kind issued it no more than a letter no kind issues under does. It
/// is therefore carried through to the STORE, exactly as the values below are, and the record
/// the store holds none under is the resource that is ABSENT.
///
/// EVERY other value is carried through to the STORE, and the record the store holds none under
/// is the resource that is ABSENT: a value under the addressed kind's OWN letter, since WHICH
/// identifier of this kind was ever issued is a property of the store rather than of the value;
/// a value under a letter no kind of this service issues an identifier under, which says nothing
/// about the kind it was issued for; and a value of this alphabet written at ANY OTHER WIDTH
/// than the issued one, whatever letter it opens with, since the width this service issues an
/// identifier at is one width CloudFront writes one in, never the only one, so away from that
/// width the leading letter names no identifier of another kind either and the store is what
/// answers for it — which is what the recorded answers to these routes report for a value of
/// the alphabet written wider than one is issued.
///
/// So the two refusals keep the conditions they describe: a value this reports `true` for is
/// the identifier a route reports it cannot READ, and every other value the store holds nothing
/// under is the resource that is ABSENT.
///
/// `prefix` is the namespace letter the addressed kind mints under; a kind whose identifiers
/// come from the namespace its four kinds SHARE ([`ENTITY_ID_PREFIX`]) reads no form of its own
/// instead ([`super::no_such_origin_access_control`]), since there the letter says nothing about
/// which of the four kinds issued the identifier. A kind whose recorded answers read NO form of
/// the value beyond that one identifier claim — the public key's routes — reads
/// [`is_issued_for_another_kind`] alone instead ([`super::unresolved_public_key`]).
pub fn id_format_refused_for(prefix: char, value: &str) -> bool {
    if is_minted_uuid(value) {
        return false;
    }
    if !is_entity_id_form(value) {
        return true;
    }
    is_issued_for_another_kind(prefix, value)
}

/// Whether `value` IS an identifier CloudFront issued for a kind of this service OTHER than the
/// one minted under `prefix`: the id alphabet at the width an identifier is issued at
/// ([`is_prefixed_token`]) under one of the other namespace letters this service issues
/// identifiers under ([`ID_NAMESPACE_LETTERS`]).
///
/// Such a value names a record of ANOTHER kind, so no record of the addressed kind could ever
/// answer to it, whichever route is addressed by it — which is the whole of what the public
/// key's routes read of an identifier before they resolve it
/// ([`super::unresolved_public_key`]), and one of the two things the field-level encryption
/// records' routes read ([`id_format_refused_for`]).
pub fn is_issued_for_another_kind(prefix: char, value: &str) -> bool {
    ID_NAMESPACE_LETTERS
        .iter()
        .any(|letter| *letter != prefix && is_prefixed_token(value, *letter))
}

/// Whether `body` is [`ID_BODY_LEN`] characters of the id alphabet — the body every
/// token this service mints carries, whatever prefix it is minted under.
pub(super) fn is_token_body(body: &str) -> bool {
    body.len() == ID_BODY_LEN && body.bytes().all(is_token_byte)
}

/// Whether `body` could be the body of an OPAQUE namespace-prefixed identifier this
/// service MINTS: [`OPAQUE_ID_BODY_LEN`] characters of the base62 alphabet CloudFront
/// writes such a body in (`dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa`).
pub(super) fn is_opaque_id_body(body: &str) -> bool {
    body.len() == OPAQUE_ID_BODY_LEN && body.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A byte of the id alphabet: a decimal digit or an uppercase ASCII letter.
pub(super) fn is_token_byte(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_uppercase()
}

/// What an `If-Match` header says about a stored version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfMatch {
    /// The header carries the distribution's current version: the mutation may proceed.
    Current,
    /// The header is absent or carries something that is not a version token at all —
    /// `InvalidIfMatchVersion`.
    Invalid,
    /// The header carries a version token, but not the current one —
    /// `PreconditionFailed`.
    Stale,
}

/// The version token an `If-Match` header carries, or `None` when the header is absent or
/// carries something that is not a version token at all.
///
/// The FORM of the header, decided against nothing but the header itself: a mutation can
/// therefore tell "missing or not valid" ([`invalid_if_match_version`]) from "not the
/// current version" without looking a resource up, which is what lets an operation refuse a
/// header no version could make valid BEFORE it decides whether the resource exists.
///
/// The value may be quoted, as HTTP entity tags normally are, or bare, as CloudFront's own
/// `ETag` header reports it, and may carry the weak-comparison prefix.
pub fn if_match_version(if_match: Option<&str>) -> Option<&str> {
    let value = if_match?.trim().trim_start_matches("W/").trim_matches('"');
    is_version_token(value).then_some(value)
}

/// Compare an `If-Match` header against a distribution's current version.
///
/// `InvalidIfMatchVersion` is documented as "the `If-Match` version is missing or not
/// valid", so an absent header and a value that is not a version token answer the same
/// way ([`if_match_version`]); a well-formed token that is not the current one is a failed
/// precondition instead.
///
/// That split is what every mutation of this service answers with EXCEPT
/// `DeleteContinuousDeploymentPolicy`, whose recorded behaviour refuses a well-formed token
/// that is not the policy's with `InvalidIfMatchVersion` as well — "missing or not valid"
/// read as "not valid FOR THIS RESOURCE". That operation therefore compares the token itself
/// (see its handler) rather than calling this, and the difference is deliberate: do not
/// "correct" either side into the other.
pub fn check_if_match(current: &str, if_match: Option<&str>) -> IfMatch {
    let Some(value) = if_match_version(if_match) else {
        return IfMatch::Invalid;
    };
    if value == current {
        IfMatch::Current
    } else {
        IfMatch::Stale
    }
}

/// Whether two invalidation batches ask for the same thing, which is what decides
/// whether a repeated `CallerReference` is the same request again.
pub fn same_batch(left: &InvalidationBatch, right: &InvalidationBatch) -> bool {
    same_paths(&left.paths, &right.paths)
}

/// Whether two distribution configurations ask for the same DISTRIBUTION, which is what
/// decides whether a repeated `CallerReference` is the same create again or a second
/// distribution under a reference that is already taken.
///
/// Compared over the whole configuration rather than a chosen few members: the question is
/// about "the content of the config", so every member a caller can submit takes part, down
/// to the members a request left out. Comparing the DESERIALIZED configurations also makes
/// the comparison insensitive to the request document's whitespace and member order, which
/// is what "identical … (ignoring white space)" asks for.
///
/// EXCEPT the access-logging configuration ([`without_access_logging`]): where the two
/// configurations describe the same distribution and differ only in `Logging`, the repeat is
/// the same create again — the recorded service answers it with the distribution the
/// reference already names, carrying the logging configuration that distribution was created
/// with, and mints nothing. Access logging is a delivery setting of a distribution that
/// already exists, turned on and retargeted through `UpdateDistribution`, so it is not part
/// of what a create asks a caller reference to name.
///
/// The comparison goes through the shapes' derived `Debug` rendering, which spells out
/// every member in declaration order with its value, because the alternative — deriving
/// `PartialEq` on the ~45 shapes a `DistributionConfig` reaches — means editing
/// `src/ops/types/shared_types.rs`, and that file carries the model's
/// `ReferrerPolicyList` value `unsafe-url`, which the merge gate's deterministic scanners
/// read as `unsafe` Rust and refuse at HIGH. So the comparison lives here instead, and
/// what it rests on — that the rendering keeps every member, including the `@sensitive`
/// `Comment` — is pinned by `same_config_separates_configurations_that_differ` below: a
/// rendering that hid or truncated a member would make two configurations differing in it
/// compare equal, and that test fails the moment it does.
pub fn same_config(left: &DistributionConfig, right: &DistributionConfig) -> bool {
    let (left, right) = (without_access_logging(left), without_access_logging(right));
    format!("{left:?}") == format!("{right:?}")
}

/// `config` with its access-logging configuration set aside, which is the configuration
/// [`same_config`] compares: the distribution the create asks for, without the delivery
/// setting that is not part of naming it.
fn without_access_logging(config: &DistributionConfig) -> DistributionConfig {
    let mut config = config.clone();
    config.logging = None;
    config
}

/// Whether two continuous deployment policy configurations ask for the same thing, which
/// is what decides whether a create repeats a policy the store already holds.
///
/// Compared MEMBER BY MEMBER over the whole configuration rather than through a rendering
/// of it: `ContinuousDeploymentPolicyConfig` reaches few enough shapes to spell the
/// comparison out, so a create costs no allocation per stored policy (unlike
/// [`same_config`], which renders a `DistributionConfig`'s ~45 shapes because deriving
/// `PartialEq` on them is not available here). Every member a caller can submit takes
/// part, down to the members a request left out.
pub fn same_policy_config(
    left: &ContinuousDeploymentPolicyConfig,
    right: &ContinuousDeploymentPolicyConfig,
) -> bool {
    left.enabled == right.enabled
        && left.staging_distribution_dns_names.quantity
            == right.staging_distribution_dns_names.quantity
        && staging_distribution_dns_names(left) == staging_distribution_dns_names(right)
        && same_traffic_config(left.traffic_config.as_ref(), right.traffic_config.as_ref())
}

/// Whether two optional traffic configurations ask for the same routing. An absent one
/// matches only an absent one: a configuration that names no `TrafficConfig` is not the
/// configuration that names one.
pub(super) fn same_traffic_config(left: Option<&TrafficConfig>, right: Option<&TrafficConfig>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.r#type == right.r#type
                && same_single_weight_config(
                    left.single_weight_config.as_ref(),
                    right.single_weight_config.as_ref(),
                )
                && same_single_header_config(
                    left.single_header_config.as_ref(),
                    right.single_header_config.as_ref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional single-weight configurations name the same share of traffic.
///
/// The weights are compared by their BITS: two configurations are the same request again
/// only if the weight submitted is the same value, and the bit pattern is what says so
/// without an approximate comparison between floats.
pub(super) fn same_single_weight_config(
    left: Option<&ContinuousDeploymentSingleWeightConfig>,
    right: Option<&ContinuousDeploymentSingleWeightConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.weight.to_bits() == right.weight.to_bits()
                && same_session_stickiness_config(
                    left.session_stickiness_config.as_ref(),
                    right.session_stickiness_config.as_ref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional session stickiness configurations name the same session window.
pub(super) fn same_session_stickiness_config(
    left: Option<&SessionStickinessConfig>,
    right: Option<&SessionStickinessConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.idle_ttl == right.idle_ttl && left.maximum_ttl == right.maximum_ttl
        }
        _ => false,
    }
}

/// Whether two optional single-header configurations select the same requests.
pub(super) fn same_single_header_config(
    left: Option<&ContinuousDeploymentSingleHeaderConfig>,
    right: Option<&ContinuousDeploymentSingleHeaderConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.header == right.header && left.value == right.value,
        _ => false,
    }
}

/// Whether two origin access identity configurations ask for the same thing, which is
/// what decides whether a repeated `CallerReference` is the same create again or a second
/// identity under a reference that is already taken.
///
/// The same comparison [`same_config`] makes, for the same reasons: every member a caller
/// can submit takes part, and comparing the DESERIALIZED configurations is what makes it
/// insensitive to the request document's whitespace and member order — "the content of the
/// `CloudFrontOriginAccessIdentityConfig` is identical to the original request (ignoring
/// white space)".
pub fn same_origin_access_identity_config(
    left: &CloudFrontOriginAccessIdentityConfig,
    right: &CloudFrontOriginAccessIdentityConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// The member of a public key's configuration an update would CHANGE, out of the members
/// that are fixed for the key's life, or `None` when it changes none of them.
///
/// "Update public key information. Note that the only value you can change is the comment."
/// — so `CallerReference`, `Name` and `EncodedKey` are fixed once the key is uploaded, and a
/// request submitting a different value for any one of them asks for a change the key does
/// not take ([`cannot_change_immutable_public_key_fields`]). The comparison is against the
/// STORED configuration, so what decides it is the record rather than the request alone, and
/// a request that resubmits the stored values unchanged beside a new comment is no change to
/// an immutable member at all.
///
/// The member's name is reported for the caller that needs to know WHICH value it may not
/// move; the modelled refusal's own sentence states the condition.
pub fn immutable_public_key_change(
    stored: &PublicKeyConfig,
    submitted: &PublicKeyConfig,
) -> Option<&'static str> {
    [
        (
            "CallerReference",
            &stored.caller_reference,
            &submitted.caller_reference,
        ),
        ("Name", &stored.name, &submitted.name),
        ("EncodedKey", &stored.encoded_key, &submitted.encoded_key),
    ]
    .into_iter()
    .find_map(|(member, held, asked)| (held != asked).then_some(member))
}

/// `CannotChangeImmutablePublicKeyFields`'s message, on the wire: the sentence names the
/// members that are fixed for a key's life — which is what the refusal is about, and what
/// tells a caller that `Comment` is updatable while `Name` and `EncodedKey` are not.
///
/// The WIRE text, which differs from the sentence the MODEL's documentation of the shape uses
/// ("You can't change the value of a public key."); do not "correct" this constant to the
/// documentation sentence. It carries nothing from the request either way.
pub const CANNOT_CHANGE_IMMUTABLE_PUBLIC_KEY_FIELDS_MESSAGE: &str =
    "You cannot modify encoded material and name of a public key once created.";

/// The modelled `CannotChangeImmutablePublicKeyFields` (HTTP 400) for an update that would
/// move a member fixed for the key's life ([`immutable_public_key_change`]).
pub fn cannot_change_immutable_public_key_fields()
-> crate::ops::types::CannotChangeImmutablePublicKeyFields {
    crate::ops::types::CannotChangeImmutablePublicKeyFields {
        message: Some(CANNOT_CHANGE_IMMUTABLE_PUBLIC_KEY_FIELDS_MESSAGE.to_string()),
    }
}

/// Whether two field-level encryption configurations ask for the same thing, which is what
/// decides whether a repeated `CallerReference` is the same create again or a second
/// configuration under a reference that is already taken.
///
/// The same comparison [`same_origin_access_identity_config`] makes, for the same
/// reasons: every member a caller can submit takes part — down to the members a request left
/// out — and comparing the DESERIALIZED configurations is what makes it insensitive to the
/// request document's whitespace and member order.
pub fn same_field_level_encryption_config(
    left: &FieldLevelEncryptionConfig,
    right: &FieldLevelEncryptionConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// Whether two `Paths` name the same objects, in the same order.
pub(super) fn same_paths(left: &Paths, right: &Paths) -> bool {
    left.quantity == right.quantity
        && left.items.as_deref().unwrap_or_default() == right.items.as_deref().unwrap_or_default()
}

/// How many entries an optional list carries. An absent list carries none, so its
/// wrapper's `Quantity` must be zero.
pub(super) fn items_len<T>(items: &Option<Vec<T>>) -> usize {
    items.as_ref().map_or(0, Vec::len)
}

/// The values of `values`, each kept once, at the position it first appears.
///
/// Membership goes through a set rather than a scan of what has been kept, so the cost is
/// linear in the number of entries (times the log of the distinct ones) however many
/// entries a configuration states.
pub(super) fn first_seen<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen: BTreeSet<&'a str> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if seen.insert(value.as_str()) {
            out.push(value.clone());
        }
    }
    out
}
