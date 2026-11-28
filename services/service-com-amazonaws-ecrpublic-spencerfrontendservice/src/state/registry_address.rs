// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS this public registry PUBLISHES — for the registry itself and for every
//! repository in it — and the reading of a gallery host a client wrote a reference on.
//!
//! # What the two published URIs have to be
//!
//! `Registry$registryUri` is "The URI of a public registry. The URI contains a universal
//! prefix and the registry alias", and `Repository$repositoryUri` is "The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations". So
//! each is read two ways at once: a container client DIALS it to move image bytes, and a
//! client PARSES it for the registry that holds the repository and composes a tag or a digest
//! on it.
//!
//! So the authority each one carries is a PUBLIC-REGISTRY NAME
//! ([`published_gallery_authority`]): the fixed labels every public-registry authority AWS
//! publishes leads with ([`GALLERY_HOST_LABELS`], `Registry$registryUri`'s "universal
//! prefix") over a DNS zone, and no port, which is the form AWS publishes too — so a client
//! that PARSES the value reads a global public registry out of it rather than a private
//! `<account>.dkr.ecr.<region>` one or a bare socket, and splits registry from repository path
//! exactly as it does on AWS. The composition AWS documents is kept on it: the registry's URI
//! is that authority scoped by the registry's own alias ([`registry_uri`]) — "a universal
//! prefix and the registry alias" — and a repository's URI is the registry's URI followed by
//! the repository name ([`repository_uri`]).
//!
//! The ZONE those fixed labels are written over is the one this emulator may announce a name
//! of its own in ([`announced_gallery_zone`], the tree's one reading of that question over the
//! endpoint this gallery answers the caller at): the loopback zone RFC 6761 reserves for the
//! interface the core binds its listeners on, or the name an operator serves this process
//! under. So what a client resolves stays inside the emulated environment, and the reading of
//! a container reference recognises the published name as this gallery's
//! ([`names_this_emulator`], [`names_the_public_gallery`]), so publishing and reading cannot
//! disagree.
//!
//! The zone AWS serves its OWN gallery in ([`AWS_GALLERY_ZONE`], which [`super::GALLERY_HOST`]
//! is the fixed labels over) is never the zone announced, and neither is a zone AWS publishes
//! its own endpoints in: a name under either resolves to the real public registry or to other
//! genuine AWS infrastructure rather than to the machine this emulator runs on. Which endpoint
//! the zone is read off is the one the request reached whenever this gallery answers requests
//! there ([`answers_gallery_requests_at`]) — the emulator's own socket or a loopback name — and
//! otherwise the address this emulator is served at.
//!
//! What the published name does NOT carry is the port this emulator happens to be served on,
//! which is an ephemeral one unless an operator chose it: AWS publishes a name in both members
//! and a client reads the registry out of it, so a port would be read as part of the registry's
//! own name and would change with every boot. A client that has to open a connection on this
//! machine is handed the emulator's own endpoint by the API it is already talking to.
//!
//! Nothing a client reads such a URI FOR is lost: the registry alias is reported beside it
//! (`Registry$aliases`) and in the path of both URIs, and the owning account and the
//! resource's identity are reported on the very response that carries the URI
//! (`Repository$registryId`, `Repository$repositoryArn`) — those are LABELS, built from the
//! stored record, and no endpoint changes them. The URI is read per RESPONSE rather than
//! stored with the repository, so the address a reader is handed is the one that reader can
//! reach, while the record itself is untouched by who read it.
//!
//! # Reading a gallery host a client wrote
//!
//! A reference a client brings IN is the other side of this. It may be written on the
//! authority this registry published — an address of this emulator ([`names_this_emulator`])
//! — or by hand on the gallery AWS serves, as a container definition written for AWS spells
//! it ([`names_the_public_gallery`]), so both are read as addressing this gallery
//! ([`super::gallery_pull::parse_gallery_reference`]) and the alias segment that follows says
//! WHICH public registry the reference names.

/// The two fixed labels every ECR Public registry authority AWS publishes LEADS with, as
/// `Registry$registryUri`'s "universal prefix" spells them (`public.ecr.<dns suffix>`, the
/// authority AWS serves its public gallery at).
///
/// They are fixed text rather than anything about the account or the Region: the account
/// appears only in `Repository$registryId` and the registry's alias in the path, and a
/// public registry is global, so no Region appears in the authority at all. They are what a
/// client reads to tell a public registry URI apart from a private
/// `<account>.dkr.ecr.<region>` one, which is why a reference written on such an authority is
/// read as naming this gallery ([`names_the_public_gallery`]).
pub const GALLERY_HOST_LABELS: [&str; 2] = ["public", "ecr"];

/// The zone AWS serves its OWN public gallery in — what follows the fixed
/// [`GALLERY_HOST_LABELS`] in [`super::GALLERY_HOST`].
///
/// It is a zone this emulator must never publish a name in: such a name resolves to the real
/// public registry rather than to this process (see the module docs), so an endpoint served
/// there is read as naming no endpoint of this emulator ([`answers_gallery_requests_at`]).
pub const AWS_GALLERY_ZONE: &str = "aws";

/// `public.ecr.<zone>` — a public-registry host in `zone`: the authority layout AWS publishes
/// for a public registry, with the zone left open so the one reading of such a host
/// ([`gallery_zone_of`]) and the one composer of one are inverses — the gallery AWS serves is
/// this layout over AWS's own zone ([`AWS_GALLERY_ZONE`]), and a reference a client wrote may
/// carry it over any other.
pub fn gallery_host_in_zone(zone: &str) -> String {
    let [public, ecr] = GALLERY_HOST_LABELS;
    format!("{public}.{ecr}.{zone}")
}

/// The AUTHORITY every URI this registry publishes is built on: a PUBLIC-REGISTRY NAME —
/// the fixed labels AWS leads one with ([`GALLERY_HOST_LABELS`]) over the zone this emulator
/// announces its own names in ([`announced_gallery_zone`]), and no port.
///
/// A `host` with no scheme and no port — which is the form both modelled URIs carry on AWS —
/// so a client splits it from the path exactly as it splits the value AWS publishes, reads a
/// global public registry out of its labels, and resolves a name that leads back to this
/// process (see the module docs).
pub fn published_gallery_authority(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    gallery_host_in_zone(&announced_gallery_zone(ctx))
}

/// The DNS ZONE this service writes every public-registry name it publishes over, for the
/// caller `ctx`: the zone of the endpoint this gallery answers that caller at
/// ([`answered_gallery_endpoint`]), read through the tree's one statement of which zone this
/// emulator may announce a name of its own in
/// ([`::service_common::data_plane_addressing::announced_zone_of_authority`]).
///
/// That reading answers the loopback zone RFC 6761 reserves for the interface the core binds
/// its listeners on — for a process reached at an address literal or at a host AWS publishes
/// its own endpoints at — and the operator's own name for a process reached under one. So the
/// public-registry host composed over it ([`published_gallery_authority`]) names a public
/// registry AND a zone that leads back to this process, while no name in a zone AWS serves is
/// ever published: such a name resolves at the real public registry, which would send the next
/// push or pull out of the emulated environment carrying the credential it is holding.
///
/// The zone AWS serves its own gallery in ([`names_an_aws_gallery_zone`]) is held out on top of
/// that reading, because a public-registry name is composed over the zone rather than under it:
/// the fixed labels written over AWS's own zone spell the gallery AWS serves
/// ([`super::GALLERY_HOST`]) itself, so a process served under a name there announces the
/// loopback zone instead.
///
/// A zone that is ITSELF a public-registry name — an endpoint an operator serves this process
/// under, or a URI this registry published, written as the address again — has those fixed
/// labels taken off it first ([`zone_under_any_gallery_prefix`]), so composing a name over the
/// zone is idempotent: this composer and its one reading ([`gallery_zone_of`], which
/// [`names_this_emulator`] strips with the same walk) stay inverses however often the prefix
/// was written.
pub fn announced_gallery_zone(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let announced = ::service_common::data_plane_addressing::announced_zone_of_authority(
        &answered_gallery_endpoint(ctx),
    );
    let zone = zone_under_any_gallery_prefix(&announced);
    match names_an_aws_gallery_zone(zone) {
        true => ::service_common::data_plane_addressing::LOOPBACK_ZONE.to_string(),
        false => zone.to_string(),
    }
}

/// `zone` with every public-registry prefix written over it removed ([`gallery_zone_of`],
/// applied until the name carries none) — the zone a public-registry name is composed over
/// ([`announced_gallery_zone`]).
///
/// A name already carrying the fixed labels is a name this registry published, and publishing
/// over it again would nest them (`public.ecr.public.ecr.<zone>`), which no client splits as
/// AWS's value. Each pass strips at least two labels, so the walk is bounded by the name's own
/// length rather than by how deeply a caller wrote them — the same walk the reading side takes
/// ([`names_this_emulator`]).
fn zone_under_any_gallery_prefix(zone: &str) -> &str {
    let mut rest = zone;
    while let Some(inner) = gallery_zone_of(rest) {
        rest = inner;
    }
    rest
}

/// Whether `zone` is the one AWS serves its own public gallery in ([`AWS_GALLERY_ZONE`]) or a
/// name inside it — the zone this service may never write a public-registry name over
/// ([`announced_gallery_zone`]).
///
/// Compared without regard to case, as a host is; the leading dot of the inner case is what
/// keeps a zone merely ENDING in those letters from being read as a name inside it.
fn names_an_aws_gallery_zone(zone: &str) -> bool {
    let zone = zone.to_ascii_lowercase();
    zone == AWS_GALLERY_ZONE || zone.ends_with(&format!(".{AWS_GALLERY_ZONE}"))
}

/// The endpoint this emulator answers THIS caller's gallery requests at, whose zone every
/// published public-registry name is written over ([`announced_gallery_zone`]).
///
/// Two endpoints can satisfy that (see the module docs): the endpoint the CALLER reached this
/// emulator on, whenever this gallery answers a request addressed there
/// ([`answers_gallery_requests_at`]), and otherwise the address this emulator is served at.
fn answered_gallery_endpoint(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let parts = ctx.parts();
    ::service_common::data_plane_addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .filter(|reached| answers_gallery_requests_at(reached))
    .unwrap_or_else(|| {
        ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx)
    })
}

/// Whether a gallery request addressed to `reached` — the `host[:port]` a caller stated — is
/// one THIS emulator answers, which is what makes that endpoint the one the zone of every
/// published name is read off ([`announced_gallery_zone`]).
///
/// Three readings, each the tree's own:
///
/// * it has to be an ADDRESS at all
///   (`service_common::data_plane_addressing::is_host_shaped`), so nothing a caller wrote that
///   is not a `host[:port]` reaches a URI this service publishes;
/// * it must name no SERVICE
///   (`service_common::data_plane_addressing::names_a_service`): a host the front door
///   resolves to a service routes there, so the address is of no use to a registry client —
///   which is also what keeps the gallery AWS serves ([`super::GALLERY_HOST`], whose first
///   label that reader takes) and every endpoint host AWS publishes out of every URI this
///   registry publishes;
/// * and it must be an address this emulator really answers on ([`names_this_emulator`]) —
///   its loopback interface in any spelling, the base it reports while it is served on no
///   port, or the single interface an operator bound it on. That is the READING side of the
///   publishing, so the two cannot disagree: a container reference a client composes on a
///   URI this registry published is recognised as naming this gallery
///   ([`super::gallery_pull::parse_gallery_reference`]). An address outside it — a routable
///   literal a caller reached a wildcard bind by, say — has the zone of the address this
///   emulator is served at read off it instead.
pub fn answers_gallery_requests_at(reached: &str) -> bool {
    use ::service_common::data_plane_addressing as addressing;

    addressing::is_host_shaped(reached)
        && !addressing::names_a_service(reached)
        && names_this_emulator(reached)
}

/// The zone a public-registry authority is served in — what follows the fixed
/// [`GALLERY_HOST_LABELS`] — and `None` for an authority that does not lead with them, which
/// names no public registry.
///
/// The ONE reading of such an authority, and the inverse of the one composer of one
/// ([`gallery_host_in_zone`]): the authority this registry publishes and one a client wrote
/// in a zone of its own differ only in that zone, so both read back as naming a public
/// registry.
///
/// Any `:port` belongs to the address rather than to the name and is removed first
/// (`service_common::data_plane_addressing::host_only`); the labels are compared without
/// regard to case, as a host is.
pub fn gallery_zone_of(authority: &str) -> Option<&str> {
    let mut rest = ::service_common::data_plane_addressing::host_only(authority);
    for fixed in GALLERY_HOST_LABELS {
        let (label, tail) = rest.split_once('.')?;
        if !label.eq_ignore_ascii_case(fixed) {
            return None;
        }
        rest = tail;
    }
    (!rest.is_empty()).then_some(rest)
}

/// Whether `authority` names an ECR Public registry: an authority leading with the fixed
/// [`GALLERY_HOST_LABELS`] over a zone ([`gallery_zone_of`]).
///
/// The gallery AWS serves ([`super::GALLERY_HOST`]) is such a name, which is what lets a
/// container reference a client wrote BY HAND — the form a container definition written for
/// AWS carries — be read as addressing this gallery
/// ([`super::gallery_pull::parse_gallery_reference`]) — the alias segment that
/// follows then says WHICH public registry it names, and only this registry's own alias is
/// answered.
pub fn names_the_public_gallery(authority: &str) -> bool {
    gallery_zone_of(authority).is_some()
}

/// `<authority>/<registry alias>` — `Registry$registryUri`, "a universal prefix and the
/// registry alias": the authority this registry is published on (read with
/// [`published_gallery_authority`]) scoped by the account's primary alias.
pub fn registry_uri(gallery_authority: &str, registry_alias: &str) -> String {
    format!(
        "{}/{registry_alias}",
        gallery_authority.trim_end_matches('/')
    )
}

/// `<registry URI>/<repository name>` — the URI a client pushes the repository
/// `repository_name` to and pulls it from.
///
/// The repository's own name is appended to the registry's URI exactly as AWS appends it,
/// including any `a/b/c` namespacing the modelled `RepositoryName` pattern admits, so a
/// client composes the same tag, digest or registry path on what it was handed.
pub fn repository_uri(registry_uri: &str, repository_name: &str) -> String {
    format!("{registry_uri}/{repository_name}")
}

/// Whether `host` — a `host[:port]` — names an address THIS emulator answers on, which is
/// the authority every URI this registry publishes is built on
/// ([`published_gallery_authority`]).
///
/// This is the READING side of the publishing above, and it exists because the two have to
/// agree: a container reference a client composes on a `repositoryUri` it was handed carries
/// the address this registry published itself at, so the reading of such a reference
/// ([`super::gallery_pull::parse_gallery_reference`]) recognises that address as naming this
/// gallery. Without it a client that pushed to the URI the API gave it would be told the
/// registry knows nothing about the reference it was handed.
///
/// Every reading is the tree's own, and each is an address this emulator really answers on:
/// the LOOPBACK interface the core binds its listeners on
/// (`service_common::data_plane_addressing::names_the_loopback_interface`, which admits every
/// spelling of that interface and every name under the zone RFC 6761 reserves for it), the
/// host of the base this emulator reports while it is served on no
/// port at all ([`::smithy_server_runtime::UNSERVED_URL_BASE`], a name RFC 6761 reserves so
/// nothing resolves it), and the interface an operator bound this emulator on
/// ([`::emulator_server::bind_address_from_env`]) — the last only when that bind names ONE
/// address, since a wildcard bind names every interface and so says nothing about which
/// registry a reference addresses. That last point is why
/// `service_common::data_plane_addressing::host_names_the_bound_interface` is not the reading
/// here: it admits ANY host on a wildcard bind, which is right for deciding whether a request
/// reached a socket of this emulator and wrong for deciding whose registry a reference names.
///
/// Any `:port` is removed first ([`::service_common::data_plane_addressing::host_only`]): a
/// client may have dialled this emulator's own socket and written its port into a reference,
/// and it is the host that says whose registry a reference addresses.
pub fn names_this_emulator(host: &str) -> bool {
    let mut host = ::service_common::data_plane_addressing::host_only(host);
    // A PUBLIC-REGISTRY authority is an address of this emulator exactly when the zone it is
    // served in is one — the fixed labels written over an address of this emulator — while the
    // gallery AWS serves and any other registry's name are served in a zone that names
    // nothing here. Each reading strips at least the two fixed labels, so the walk is bounded
    // by the host's own length rather than by how deeply a caller wrote those labels.
    while let Some(zone) = gallery_zone_of(host) {
        host = zone;
    }
    if ::service_common::data_plane_addressing::names_the_loopback_interface(host) {
        return true;
    }
    let unserved = ::service_common::data_plane_addressing::url_authority(
        ::smithy_server_runtime::UNSERVED_URL_BASE,
    );
    if host.eq_ignore_ascii_case(unserved) {
        return true;
    }
    let Ok(bound) = ::emulator_server::bind_address_from_env() else {
        return false;
    };
    !bound.is_unspecified()
        && host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|named| named == bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{default_registry_alias, GALLERY_HOST};

    /// The alias this emulator's registry is assigned, which the composition rows scope a
    /// registry URI by.
    fn alias() -> String {
        default_registry_alias(::engine::EMULATOR_ACCOUNT_ID)
    }

    #[test]
    fn the_gallery_aws_serves_is_named_by_the_fixed_labels_and_its_own_zone() {
        // The host AWS publishes the gallery at is the fixed prefix over AWS's own zone, so
        // the one reading of such an authority recognises it.
        let [public, ecr] = GALLERY_HOST_LABELS;
        assert_eq!(
            gallery_zone_of(GALLERY_HOST),
            Some(AWS_GALLERY_ZONE),
            "expected {GALLERY_HOST:?} to lead with {public}.{ecr}. and name its own zone"
        );
        assert!(
            names_the_public_gallery(GALLERY_HOST),
            "expected {GALLERY_HOST:?} to name a public registry"
        );
        // The one composer of such a host and the one reading of it are inverses, so the
        // gallery AWS serves is recognised however it is spelled.
        assert_eq!(
            gallery_host_in_zone(AWS_GALLERY_ZONE),
            GALLERY_HOST,
            "expected the composer to spell the gallery AWS serves"
        );
        // It is NOT an address this emulator answers on, so no URI is published there.
        assert!(
            !names_this_emulator(GALLERY_HOST),
            "expected {GALLERY_HOST:?} to name no address of this emulator"
        );
        assert!(
            !answers_gallery_requests_at(GALLERY_HOST),
            "expected {GALLERY_HOST:?} to be answered by AWS rather than by this emulator"
        );
    }

    #[test]
    fn an_authority_that_names_no_public_registry_is_told_from_one_that_does() {
        // The rows span the authorities a container reference can carry: the gallery AWS
        // serves, another registry altogether, an Amazon ECR private registry host, and the
        // prefix written without a zone behind it.
        let cases: [(&str, bool); 8] = [
            (GALLERY_HOST, true),
            ("PUBLIC.ECR.AWS", true),
            ("public.ecr.gallery.example", true),
            ("public.ecr.aws:443", true),
            ("public.ecr.", false),
            ("public.ecr", false),
            ("000000000000.dkr.ecr.us-east-1.amazonaws.com", false),
            ("quay.io", false),
        ];
        for (authority, expected) in cases {
            let named = names_the_public_gallery(authority);
            assert_eq!(
                named, expected,
                "case {authority:?}: expected names_the_public_gallery to be {expected}, got \
                 {named}"
            );
        }
    }

    #[test]
    fn an_address_this_emulator_answers_on_is_told_from_one_it_does_not() {
        // The rows span the hosts a published URI can carry — the loopback forms a core
        // listener admits, with and without a port, the base the emulator reports while it is
        // served on no port at all, and the public-registry authorities this service announces
        // over each of those zones — against the registry hosts a reference names instead: the
        // gallery AWS serves, a public-registry name served in somebody else's zone, another
        // registry altogether, and an Amazon ECR private registry host.
        let cases: [(&str, bool); 14] = [
            ("127.0.0.1:9297", true),
            ("127.0.0.1", true),
            ("127.3.2.1:8443", true),
            ("localhost:39355", true),
            ("[::1]:9297", true),
            ("emulator.invalid", true),
            ("public.ecr.localhost:9297", true),
            ("PUBLIC.ECR.LOCALHOST", true),
            ("public.ecr.emulator.invalid", true),
            (GALLERY_HOST, false),
            ("public.ecr.gallery.example", false),
            ("quay.io", false),
            ("000000000000.dkr.ecr.us-east-1.amazonaws.com", false),
            ("10.1.2.3:5000", false),
        ];
        for (host, expected) in cases {
            let answered = names_this_emulator(host);
            assert_eq!(
                answered, expected,
                "case {host:?}: expected names_this_emulator to be {expected}, got {answered}"
            );
        }
    }

    #[test]
    fn an_endpoint_this_gallery_answers_on_is_told_from_one_it_does_not() {
        // RULE's reading side: which endpoint a published URI may be built on.
        // The rows span the endpoints a request can arrive on: the emulator's own socket and
        // loopback names, against this service's own AWS endpoint host, another zone AWS
        // publishes its endpoints in, the gallery AWS serves, a public-registry name in
        // another zone, a host whose first label names a service, a routable literal a caller
        // reached a wildcard bind by (an address the reading side does not recognise, so no
        // URI is published on it), and a value that is no address at all.
        let cases: [(&str, bool); 11] = [
            ("127.0.0.1:9297", true),
            ("localhost:39355", true),
            ("[::1]:38001", true),
            ("127.0.0.1", true),
            ("api.ecr-public.us-east-1.amazonaws.com", false),
            ("api.ecr-public.us-east-1.api.aws", false),
            (GALLERY_HOST, false),
            ("public.ecr.localhost:9297", false),
            ("dynamodb.localhost:39355", false),
            ("10.1.2.3:5000", false),
            ("elsewhere.example.com/gallery", false),
        ];
        for (endpoint, expected) in cases {
            let answered = answers_gallery_requests_at(endpoint);
            assert_eq!(
                answered, expected,
                "case {endpoint:?}: expected answers_gallery_requests_at to be {expected}, \
                 got {answered}"
            );
            // The two sides agree by construction: every endpoint a URI is published on is
            // an address the reading of a container reference recognises.
            if answered {
                assert!(
                    names_this_emulator(endpoint),
                    "case {endpoint:?}: a published endpoint must read back as this emulator's"
                );
            }
        }
    }

    #[test]
    fn no_name_in_the_zone_aws_serves_its_gallery_in_is_ever_announced() {
        // The rows span the zones a served address can name: AWS's own gallery zone and names
        // inside it (never announced, since the fixed labels over them spell the gallery AWS
        // serves or another name AWS resolves), against the zones this emulator may announce a
        // name of its own in — the reserved loopback zone, an operator's own name, and a name
        // merely ENDING in those letters without being inside that zone.
        let cases: [(&str, bool); 8] = [
            (AWS_GALLERY_ZONE, true),
            ("AWS", true),
            ("ecr.aws", true),
            ("gallery.example.aws", true),
            (::service_common::data_plane_addressing::LOOPBACK_ZONE, false),
            ("gallery.internal.example", false),
            ("paws", false),
            ("aws.example", false),
        ];
        for (zone, held_out) in cases {
            let named = names_an_aws_gallery_zone(zone);
            assert_eq!(
                named, held_out,
                "case {zone:?}: expected names_an_aws_gallery_zone to be {held_out}, got {named}"
            );
        }
        // The gallery AWS serves is exactly the fixed labels over its own zone, which is why
        // that zone is the one held out.
        assert_eq!(
            gallery_host_in_zone(AWS_GALLERY_ZONE),
            GALLERY_HOST,
            "expected the composer over AWS's own zone to spell the gallery AWS serves"
        );
    }

    #[test]
    fn composing_a_public_registry_name_over_a_zone_is_idempotent() {
        // A zone that already carries the fixed labels is a name this registry published, so
        // the labels come off before it is composed over again: publishing is idempotent and
        // the composer stays the inverse of its one reading. The rows span the zones a served
        // address can name — a plain zone, a name with the prefix once and twice, the prefix
        // over the loopback zone, and a zone merely carrying those labels further in.
        let cases: [(&str, &str); 6] = [
            ("gallery.internal.example", "gallery.internal.example"),
            ("public.ecr.gallery.internal.example", "gallery.internal.example"),
            (
                "public.ecr.public.ecr.gallery.internal.example",
                "gallery.internal.example",
            ),
            ("public.ecr.localhost", "localhost"),
            ("PUBLIC.ECR.LOCALHOST", "LOCALHOST"),
            ("gallery.public.ecr.example", "gallery.public.ecr.example"),
        ];
        for (zone, expected) in cases {
            let stripped = zone_under_any_gallery_prefix(zone);
            assert_eq!(
                stripped, expected,
                "case {zone:?}: expected the zone under every public-registry prefix to be \
                 {expected:?}, got {stripped:?}"
            );
            // Composed over the stripped zone, the name carries the fixed labels exactly once,
            // and reading it back yields that same zone.
            let composed = gallery_host_in_zone(stripped);
            assert_eq!(
                gallery_zone_of(&composed).map(str::to_string),
                Some(expected.to_string()),
                "case {zone:?}: expected {composed:?} to read back as {expected:?}"
            );
            assert_eq!(
                zone_under_any_gallery_prefix(&composed),
                expected,
                "case {zone:?}: expected composing over {expected:?} to be idempotent"
            );
        }
    }

    #[test]
    fn a_repository_uri_extends_the_registry_uri_it_is_held_in() {
        // The rows span what the composition generalises over: the public-registry names this
        // service publishes — the fixed labels over each zone this emulator announces a name
        // of its own in (the loopback zone, a name under it, the zone of the base reported
        // while this process is served on no port, and an operator's own name) — crossed with
        // the repository-name forms the modelled `RepositoryName` pattern admits.
        let endpoints = [
            gallery_host_in_zone(::service_common::data_plane_addressing::LOOPBACK_ZONE),
            gallery_host_in_zone("emulator.localhost"),
            gallery_host_in_zone("emulator.invalid"),
            gallery_host_in_zone("gallery.internal.example"),
        ];
        let names = ["ab", "nginx", "team.app", "project-a/nginx-web-app"];
        let alias = alias();
        for authority in &endpoints {
            let authority = authority.as_str();
            let registry = registry_uri(authority, &alias);
            assert_eq!(
                registry,
                format!("{authority}/{alias}"),
                "case {authority:?}: expected the registry URI to be the published authority \
                 scoped by the alias, got {registry:?}"
            );
            for name in names {
                let uri = repository_uri(&registry, name);
                let case = format!("{authority} {name}");
                assert_eq!(
                    uri,
                    format!("{registry}/{name}"),
                    "case {case:?}: expected the repository URI under the registry URI, got \
                     {uri:?}"
                );
                // The authority half is the registry's own name, and the rest is the
                // path it extends: the alias that says which public registry holds the
                // repository, then the repository's own name.
                let (dialled, path) = uri
                    .split_once('/')
                    .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no path"));
                assert_eq!(
                    dialled, authority,
                    "case {case:?}: expected the published authority as the dialled one, got \
                     {dialled:?}"
                );
                assert!(
                    names_the_public_gallery(dialled),
                    "case {case:?}: expected a public-registry name, got {dialled:?}"
                );
                assert!(
                    !dialled.contains(':'),
                    "case {case:?}: a published registry name carries no port, got {dialled:?}"
                );
                assert_eq!(
                    path,
                    format!("{alias}/{name}"),
                    "case {case:?}: expected the alias-scoped repository path, got {path:?}"
                );
            }
        }
    }
}
