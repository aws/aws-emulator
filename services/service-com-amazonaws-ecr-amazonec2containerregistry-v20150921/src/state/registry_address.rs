// SPDX-License-Identifier: CC0-1.0

//! The registry ADDRESS this service REPORTS for a repository — the one a container client
//! writes into an image reference — and the reading of a registry host a client wrote a
//! reference in.
//!
//! # What a repository's own URI has to be
//!
//! `Repository$repositoryUri` is "The URI for the repository. You can use this URI for
//! container image `push` and `pull` operations": the value is an ADDRESS, read by splitting
//! it on the first `/` — a container client DIALS the authority ahead of that `/` and asks
//! for the repository the path names. A client writes it into the image reference of a task
//! definition or a function and PULLS what it reads there, so the one property the value has
//! to carry is that a connection opened to the authority it names reaches THIS process.
//!
//! So the authority is the ENDPOINT this emulator answers that caller's registry requests at,
//! host and port whole ([`announced_registry_endpoint`]), with the repository's own name as
//! the path ([`repository_uri`]) — the same endpoint the URL a `docker login` is pointed at
//! is built on ([`registry_login_url`]), so the two addresses this service publishes cannot
//! disagree. It is an address rather than a NAME because a name is resolved before it is
//! dialled and nothing here answers for one: the host AWS publishes a registry under
//! ([`registry_host`]) resolves at genuine Amazon ECR, which would send the next push out of
//! the emulated environment carrying the credential its `docker login` was issued, and a
//! registry-shaped name in a zone of this emulator's own is served by nothing either — the
//! core binds its listeners on the loopback interface and its front door admits only a
//! loopback `Host` there, so such a name is refused even by a client that resolves it, and it
//! carries no port for the ephemeral socket this process is served on. This registry's `/v2`
//! plane then claims a request addressed to the published endpoint whatever host it arrived
//! on (`crate::service::registry_v2`), so a reference a client composes on the value it was
//! handed — a tag, a digest or a `/v2` path — reaches the repository it names
//! (`super::registry_pull::parse_reference`).
//!
//! Which registry HOLDS the repository is reported BESIDE the address on the very response
//! that carries it — `Repository$registryId` is the owning account and
//! `Repository$repositoryArn` names the Region — both off the same stored record, so no
//! reader's own endpoint moves either. The published endpoint names no registry of its own
//! and answers for every registry this emulator serves; which one a `/v2` request addresses
//! is said by the credential it presents
//! (`super::registry_v2::RegistryCredential::region`), obtained per registry.
//!
//! A reference a client brings IN is the other side of this. It may be written on the address
//! this registry published, or by hand on the registry host AWS publishes, and the ONE reading
//! of a registry host ([`registry_host_parts`]) reads the latter back as naming the registry
//! its leading labels name — the account, the fixed labels and the Region, whatever zone
//! follows them. A bare address of this process names no registry, and then the credential the
//! request presents says which registry it addresses
//! (`super::registry_v2::RegistryCredential::region`). This registry's `/v2` data plane claims
//! a request whatever host it arrived on (`crate::service::registry_v2`), so either way the
//! client reaches the very repository the reference names.
//!
//! # Which address a client reaches the registry by
//!
//! One: the endpoint this emulator answers that caller's registry requests at. Both members
//! this service publishes are built on it — the repository's URI as that authority followed
//! by the repository's name, the login URL as that authority behind a scheme — so a client
//! that was handed either opens its connection to the socket this process is served on, port
//! included, with no name to resolve. The same `/v2` plane answers there, for the same
//! credential, for every repository every registry this emulator serves holds.
//!
//! # What a `docker login` is pointed at
//!
//! `AuthorizationData$proxyEndpoint` ("The registry URL to use for this authorization token
//! in a `docker login` command") is a URL rather than a name a client reads anything out of:
//! it is handed to `docker login` and DIALLED as given, with the credential the token carries,
//! so it is built on the endpoint this emulator answers THAT caller's registry requests at
//! ([`announced_registry_endpoint`]) behind a scheme ([`registry_login_url`]) — the one
//! address a client on this machine can open a connection to without resolving a name at all,
//! port included. A repository's own URI is that same endpoint with the repository's name as
//! its path (see above), and the credential this URL was issued with is honoured whichever of
//! the two a client dials: the `/v2` plane claims the request whatever host it arrived on.
//!
//! An Amazon ECR registry is one account's in ONE Region, which is why a repository is keyed
//! by Region (`super::repository_key`) and why its ARN names that Region: a `/v2` request
//! names the repository in the path and the registry through the host it was addressed to
//! ([`dialled_registry_region`]) or, for an endpoint naming no registry, the credential it
//! presents (`super::registry_v2::RegistryCredential::region`), which is the registry the
//! token was issued for.
//!
//! # Reading a registry host a client wrote
//!
//! A reference a client brings IN is another matter: it is written in the registry URL form
//! AWS publishes, so a pull through cache rule's upstream URL, an in-process pull's image
//! reference and the host a `/v2` request was addressed to are read back through the one
//! reading of that layout ([`registry_host_parts`]) — the owning account's id, the two fixed
//! labels, the Region, then the zone the registry is served in. That reading is what says
//! which Region's registry a client dialled ([`dialled_registry_region`]), and this
//! registry's `/v2` data plane claims a request whatever host it arrived on
//! (`crate::service::registry_v2`), so both a reference written on AWS's spelling and one
//! written on an address this emulator is reached at — which is the address this service
//! reports ([`repository_uri`]) — reach the very repository they name.

/// The endpoint this emulator answers THIS caller's registry requests at: the address the
/// registry URL a `docker login` of that caller is pointed at is built on
/// ([`registry_login_url`]) — its host and its port — and the authority every repository of
/// this registry is published at (`super::Repo::to_repository`, over [`repository_uri`]).
///
/// Two endpoints can satisfy that:
///
/// - the endpoint the CALLER reached this emulator on, whenever this registry answers a
///   `/v2` request addressed there for every registry it serves
///   ([`answers_registry_requests_at`]) — the emulator's own socket, a loopback name, and
///   this service's own endpoint host, because a request routed to THIS service by its host
///   is served by this service's registry plane (`crate::service`'s
///   `override_routing_error`). Such a caller is reported the address it is already talking
///   to, which is the one address it is known to reach;
/// - otherwise the address this emulator is served at: a caller that came in on ANOTHER
///   service's endpoint host would be handed an address a registry client cannot use,
///   because that host routes to that service and no registry request addressed to it
///   reaches this plane; so would a caller that came in on a host inside a zone AWS
///   publishes its OWN endpoints in (`addresses_an_aws_endpoint`), which resolves nowhere
///   near this process; and so would a caller that came in on a host naming ONE registry,
///   which answers for that Region alone. Taking the served address WHOLE, host and port
///   together, is what keeps the reported address dialable — a `/v2` request presenting a
///   `docker login` credential is this registry's whatever host it arrived on
///   (`crate::service`'s registry claim).
pub fn announced_registry_endpoint(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let parts = ctx.parts();
    ::service_common::data_plane_addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .filter(|reached| answers_registry_requests_at(reached, ctx.identity().signing_name()))
    .unwrap_or_else(|| {
        ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx)
    })
}

/// Whether a `/v2` request addressed to `reached` — the `host[:port]` a caller stated — is
/// one THIS registry answers for every registry it serves, which is what makes that authority
/// reportable as the address a `docker login` is pointed at
/// ([`announced_registry_endpoint`]).
///
/// Four readings, each the tree's own:
///
/// - it has to be an ADDRESS at all
///   (`service_common::data_plane_addressing::is_host_shaped`), so nothing a caller wrote
///   that is not a `host[:port]` reaches a URL this service announces;
/// - it must not be a host AWS publishes its OWN endpoints at
///   (`addresses_an_aws_endpoint`): such a name resolves nowhere near this process or, worse,
///   at genuine AWS infrastructure, which sends the next client out of the emulated
///   environment carrying the credential it is holding;
/// - it must name no REGISTRY ([`registry_host_parts`]): such a host pins ONE account and ONE
///   Region, and a request addressed there is answered for that Region's registry
///   ([`dialled_registry_region`]) — so reporting it would point every `docker login` of this
///   caller, whichever registry it is for, at one Region's registry address. The address
///   this emulator is served at names no registry and answers for all of them, the presented
///   credential saying which (`super::registry_v2::RegistryCredential::region`), so that is
///   what such a caller is reported;
/// - it must name no OTHER service than `this_service`
///   (`service_common::data_plane_addressing::service_named_by_host`, the reading written
///   for exactly this question — "for a plane that has to tell its OWN service's endpoint
///   from another service's"): a host the front door resolves to another service routes
///   there, so no registry request addressed to it reaches this plane, while a host that
///   resolves to THIS service reaches it (`crate::service`), as does one that names no
///   service at all.
fn answers_registry_requests_at(reached: &str, this_service: &str) -> bool {
    use ::service_common::data_plane_addressing as addressing;

    if !addressing::is_host_shaped(reached)
        || addressing::addresses_an_aws_endpoint(reached)
        || registry_host_parts(reached).is_some()
    {
        return false;
    }
    addressing::service_named_by_host(reached).is_none_or(|named| named == this_service)
}

/// The Region of the registry a request to one of this registry's own DATA-PLANE endpoints
/// DIALLED: the Region named by the registry host it was addressed to, read through the one
/// reading of that host this service states ([`registry_host_parts`], held to the account
/// this registry serves by `super::registry_pull::host_region`), and `None` for an endpoint
/// that names no registry — this emulator's own socket, which serves every Region's
/// registry.
///
/// A data-plane request carries no SigV4 credential scope of its own — a registry client
/// presents the `docker login` credential as HTTP Basic authorization — so the endpoint it
/// dialled is what says which Region's registry it addresses, exactly as an image reference
/// does for an in-process pull (`super::registry_pull::parse_reference`). Where the endpoint
/// names none, the credential presented does: a `docker login` credential is obtained per
/// registry (`super::registry_v2::RegistryCredential::region`).
///
/// `authority` is the endpoint the request arrived on, host and optional port.
pub fn dialled_registry_region(authority: &str) -> Option<String> {
    super::registry_pull::host_region(authority)
}

/// `<authority>/<name>` — `Repository$repositoryUri`, the URI a client pushes the repository
/// `name` to and pulls it from: the authority that repository's registry is published at
/// followed by the repository's own name as the path, and nothing else — no scheme, as the
/// form AWS publishes the member in carries none.
///
/// `authority` is the endpoint this emulator answers that caller's registry requests at, host
/// and port whole ([`announced_registry_endpoint`], passed in by
/// `super::Repo::to_repository`): the authority a client splits off is the one it DIALS, and
/// an address of this process is what a connection opened to it reaches.
///
/// The path after the first `/` is exactly the repository name, so a client composes a tag, a
/// digest or a `/v2` path on what it was handed and the reference reads back as the
/// repository it names (`super::registry_pull::parse_reference`, which reads a reference
/// written on an address of this emulator and one written on the host AWS publishes alike).
pub fn repository_uri(authority: &str, name: &str) -> String {
    format!("{}/{name}", authority.trim_end_matches('/'))
}

/// The host of the Amazon ECR registry `registry_id` owns in `region`, in the spelling AWS
/// PUBLISHES: the owning account's id, the two fixed labels ([`REGISTRY_HOST_LABELS`]), the
/// Region, and the DNS suffix that Region's partition is served under
/// ([`registry_dns_suffix`]).
///
/// The registry URL format the model states, `aws_account_id.dkr.ecr.region.amazonaws.com`
/// (`AuthorizationData$proxyEndpoint`), with the partition's own suffix in place of the
/// commercial one — so a registry of any partition's Region is named the way that partition
/// publishes it. It is the inverse of the one reading of such a host
/// ([`registry_host_parts`]), so a reference written in that spelling reads back as the
/// registry it names.
///
/// It is the host every repository of that registry is addressed under ON AWS, so it is the
/// spelling a client's own reference may be written in, and this is the INVERSE of the one
/// reading of such a host ([`registry_host_parts`]): it exists for READING a reference a
/// client wrote in AWS's spelling, and for composing one in it (a compute service's image
/// reference). It is NOT an address this emulator ANNOUNCES: what this registry publishes for
/// a repository is the endpoint it answers that caller at
/// ([`announced_registry_endpoint`], over `super::Repo::to_repository`), because nothing here
/// answers for a name AWS serves.
pub fn registry_host(registry_id: &str, region: &str) -> String {
    registry_host_in_zone(registry_id, region, registry_dns_suffix(region))
}

/// `<registry id>.dkr.ecr.<region>.<zone>` — an Amazon ECR registry host in `zone`: the
/// layout AWS publishes for a registry ([`registry_host`] is its case for the zone AWS serves
/// a Region's registries in), with the zone left open because a client may have been pointed
/// at one registry host of that layout in a zone of an operator's own.
///
/// The ONE composer of such a host, so no reader of one spells its labels or their order a
/// second way, and the one reading of such a host ([`registry_host_parts`]) reads either
/// back.
pub fn registry_host_in_zone(registry_id: &str, region: &str, zone: &str) -> String {
    let [dkr, ecr] = REGISTRY_HOST_LABELS;
    format!("{registry_id}.{dkr}.{ecr}.{region}.{zone}")
}

/// The two fixed labels every Amazon ECR registry host carries between the owning account's
/// id and the registry's Region, as the registry URL format
/// `AuthorizationData$proxyEndpoint` documents spells them
/// (`https://aws_account_id.dkr.ecr.region.amazonaws.com`).
///
/// The one reading of such a host ([`registry_host_parts`]) is written on them, so a
/// reference a client wrote in AWS's spelling parses back as a reference to the registry it
/// names.
pub const REGISTRY_HOST_LABELS: [&str; 2] = ["dkr", "ecr"];

/// What a registry host NAMES, read off an authority ([`registry_host_parts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryHostParts<'host> {
    /// The account whose registry the host names — its first label, a 12-digit account id.
    pub registry_id: &'host str,
    /// The Region that registry serves.
    pub region: &'host str,
    /// The zone the registry is served in: AWS's own partition suffix for the host AWS
    /// publishes, and a zone of its own for a host an operator points a client at.
    pub zone: &'host str,
}

/// The account, the Region and the zone the registry host `authority` names, and `None` for
/// every authority of another shape.
///
/// The ONE reading of a registry host's layout: what makes a host a registry's is the account
/// label, the two fixed labels ([`REGISTRY_HOST_LABELS`]) and the Region label — the zone
/// that follows is wherever the registry is SERVED, AWS's own partition suffix for the host
/// AWS publishes and a zone of its own for a host an operator points a client at, so every
/// spelling of one registry reads back as that registry. A pull asks which Region and
/// account the reference it carries addresses
/// (`super::registry_pull::parse_registry_reference`) and the data plane which Region the
/// client dialled ([`dialled_registry_region`]), so neither walks the labels itself.
///
/// A zone AWS publishes its OWN endpoints in, other than the Region's own partition suffix,
/// names no registry: AWS serves a Region's registries only under that suffix
/// ([`registry_dns_suffix`]), so `<account>.dkr.ecr.<a commercial Region>.amazonaws.com.cn`
/// and a deeper name in such a zone ([`names_an_aws_endpoint_zone`]) are hosts nothing
/// answers on. Every other zone is one a client may have been pointed at — the loopback zone,
/// or the name an operator serves this emulator at — which no static list can enumerate.
///
/// Any userinfo and `:port` belong to the address rather than to the name and are removed
/// first; the fixed labels are compared without regard to case, as a host is.
///
/// What this does NOT answer is whether a URL names Amazon ECR as the PUBLIC registry a pull
/// through cache rule can mirror: that is the host AWS publishes, its own partition's suffix
/// included (`super::ecr_registry_host_parts`).
pub fn registry_host_parts(authority: &str) -> Option<RegistryHostParts<'_>> {
    let host = ::service_common::data_plane_addressing::host_only(authority);
    // The zone is the remainder: it carries labels of its own (`amazonaws.com` has two), so
    // only the labels ahead of it are split off.
    let mut labels = host.splitn(5, '.');
    // The account label is read by the tree's own reading of an account id
    // (`service_common::arn::is_account_id`, "for a member that carries an account id on its
    // own — … a registry or owner id"), so this reading and every other reader of one cannot
    // disagree.
    let registry_id = labels
        .next()
        .filter(|label| ::service_common::arn::is_account_id(label))?;
    for fixed in REGISTRY_HOST_LABELS {
        if !labels
            .next()
            .is_some_and(|label| label.eq_ignore_ascii_case(fixed))
        {
            return None;
        }
    }
    let region = labels.next().filter(|region| !region.is_empty())?;
    let zone = labels.next().filter(|zone| !zone.is_empty())?;
    let serves_the_region = zone.eq_ignore_ascii_case(registry_dns_suffix(region))
        || !names_an_aws_endpoint_zone(zone);
    serves_the_region.then_some(RegistryHostParts {
        registry_id,
        region,
        zone,
    })
}

/// Whether `zone` is one AWS publishes its OWN endpoints in: a DNS suffix a partition
/// declares ([`COMMERCIAL_DNS_SUFFIX`], [`PARTITION_DNS_SUFFIXES`]), or a deeper name inside
/// one (`service_common::data_plane_addressing::addresses_an_aws_endpoint`, which reads the
/// same suffixes with a label in front of them).
fn names_an_aws_endpoint_zone(zone: &str) -> bool {
    ::std::iter::once(COMMERCIAL_DNS_SUFFIX)
        .chain(PARTITION_DNS_SUFFIXES.iter().map(|(_, suffix)| *suffix))
        .any(|suffix| zone.eq_ignore_ascii_case(suffix))
        || ::service_common::data_plane_addressing::addresses_an_aws_endpoint(zone)
}

/// The DNS suffix the registries of `region`'s partition are served under: the `dnsSuffix`
/// that partition publishes in the AWS SDK partition metadata, which every regional
/// endpoint of the partition is built from.
///
/// A Region outside [`PARTITION_DNS_SUFFIXES`] is of the commercial partition, whose suffix
/// is the one the registry URL format above spells out.
pub fn registry_dns_suffix(region: &str) -> &'static str {
    PARTITION_DNS_SUFFIXES
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or(COMMERCIAL_DNS_SUFFIX, |(_, suffix)| *suffix)
}

/// The DNS suffix of the commercial partition, which the registry URL format
/// `AuthorizationData$proxyEndpoint` documents is written with.
pub const COMMERCIAL_DNS_SUFFIX: &str = "amazonaws.com";

/// The Region prefix of each partition whose endpoints are NOT under
/// [`COMMERCIAL_DNS_SUFFIX`], with the `dnsSuffix` that partition publishes in the AWS SDK
/// partition metadata (`endpoints.json`): `aws-cn` `amazonaws.com.cn`, `aws-eusc`
/// `amazonaws.eu`, `aws-iso` `c2s.ic.gov`, `aws-iso-b` `sc2s.sgov.gov`, `aws-iso-e`
/// `cloud.adc-e.uk`, `aws-iso-f` `csp.hci.ic.gov` — the same suffix set the core's own
/// reading of a partition host carries (`smithy_server_runtime::virtual_host`). GovCloud
/// (`us-gov-`) publishes `amazonaws.com`, so it needs no entry.
///
/// Every prefix ends in a hyphen, so `us-iso-` cannot swallow `us-isob-` and the order of
/// the entries carries no meaning.
pub const PARTITION_DNS_SUFFIXES: [(&str, &str); 6] = [
    ("cn-", "amazonaws.com.cn"),
    ("eusc-", "amazonaws.eu"),
    ("us-iso-", "c2s.ic.gov"),
    ("us-isob-", "sc2s.sgov.gov"),
    ("us-isof-", "csp.hci.ic.gov"),
    ("eu-isoe-", "cloud.adc-e.uk"),
];

/// `https://<endpoint>` — the registry URL a `docker login` is pointed at: the endpoint this
/// emulator answers the caller at ([`announced_registry_endpoint`]), behind a scheme.
///
/// `AuthorizationData$proxyEndpoint` states the shape — "The Amazon ECR registry URL
/// format is `https://aws_account_id.dkr.ecr.region.amazonaws.com`" — a scheme followed by
/// the registry's address, which here is the address this emulator answers the registry API
/// on, because that URL is DIALLED as it is handed over, with the credential the token
/// carries, and the endpoint the caller already reached is the one address this service
/// knows it can reach.
pub fn registry_login_url(announced_endpoint: &str) -> String {
    format!("https://{announced_endpoint}")
}
