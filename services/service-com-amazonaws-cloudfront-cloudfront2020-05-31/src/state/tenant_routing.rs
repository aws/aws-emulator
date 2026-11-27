// SPDX-License-Identifier: CC0-1.0

//! WHERE a viewer's request has to arrive for a CloudFront resource to answer it.
//!
//! Three values a caller states decide that, and each is held on the record the call wrote:
//! the arrival addresses one of a distribution tenant's domains is served on
//! (`DomainItem$ServeOn`), what becomes of a domain another CloudFront resource already
//! serves (`CreateDistributionTenant$OnDomainTakeover`), and which distribution's
//! certificate answers a viewer that arrives on a connection group's Anycast static IPs
//! without Server Name Indication (`ConnectionGroup$ViewerFallback`). Nothing in this
//! module reads or writes the store.

use super::*;

/// `ServeOn` for a domain served wherever a request for it arrives: "`any` – CloudFront
/// routes requests for this domain to the distribution tenant regardless of the IP address
/// that they arrive on. This is the default if you don't specify a value."
pub const SERVE_ON_ANY: &str = "any";

/// `ServeOn` for a domain served only where the tenant's Anycast static IPs answer:
/// "`anycast-ip-list-only` – CloudFront routes requests for this domain to the distribution
/// tenant only when they arrive on the Anycast static IPs of the distribution tenant's
/// connection group. CloudFront treats requests that arrive on other IP addresses as
/// requests for an unknown domain."
pub const SERVE_ON_ANYCAST_IP_LIST_ONLY: &str = "anycast-ip-list-only";

/// `OnDomainTakeover` for a call that MOVES a domain another CloudFront resource already
/// serves: "`allow` – CloudFront moves the domain to this distribution tenant when the
/// domain's DNS points to this distribution tenant."
pub const ON_DOMAIN_TAKEOVER_ALLOW: &str = "allow";

/// `OnDomainTakeover` for a call that does not, which is what a call stating none asks for:
/// "`reject` – CloudFront doesn't move the domain, and the request fails if the domain is
/// associated with another resource. This is the default if you don't specify a value."
pub const ON_DOMAIN_TAKEOVER_REJECT: &str = "reject";

/// One domain a distribution tenant holds, as this emulator persists it: the name, and the
/// arrival addresses the tenant is served at it on.
///
/// The pair is stored TOGETHER because that is how it is submitted (`DomainItem`) and how it
/// is reported (`DomainResult`): one stored element per association, so a read of the tenant
/// answers both members from the record the call that bound the domain wrote.
#[derive(Debug, Clone)]
pub struct TenantDomain {
    /// `DomainItem$Domain` — "the domain name", which is also the `DomainResult$Domain` every
    /// projection of the tenant reports the association under.
    pub domain: String,
    /// `DomainItem$ServeOn`, RESOLVED: the value the call stated, or [`SERVE_ON_ANY`] where it
    /// stated none, since that is "the default if you don't specify a value" — so the record
    /// answers the member rather than the request that is being served. A value outside the
    /// model's `ServeOn` enum is refused at the request boundary by the enum's own
    /// constraint, so only the two documented values reach a record.
    pub serve_on: String,
}

impl TenantDomain {
    /// The association a submitted `DomainItem` binds, with the stated-or-default `ServeOn`
    /// resolved. ONE conversion, so the create and the update of a tenant cannot disagree
    /// about what a submitted domain means.
    pub fn from_item(item: &DomainItem) -> Self {
        Self {
            domain: item.domain.clone(),
            serve_on: item
                .serve_on
                .clone()
                .unwrap_or_else(|| SERVE_ON_ANY.to_string()),
        }
    }

    /// Whether this association is the one for `domain`, compared as DNS names are
    /// ([`same_domain`]): a name is the same name in any case.
    pub fn is(&self, domain: &str) -> bool {
        same_domain(&self.domain, domain)
    }

    /// Whether a request for this domain is only answered where the tenant's connection
    /// group's Anycast static IPs are: [`SERVE_ON_ANYCAST_IP_LIST_ONLY`].
    pub fn anycast_ips_only(&self) -> bool {
        self.serve_on == SERVE_ON_ANYCAST_IP_LIST_ONLY
    }
}

/// The names `domains` binds, in the order they were stated — what every rule that is about
/// the NAME alone (the admission of a domain, the domains a certificate covers, the DNS
/// records a verification reads) is evaluated over.
pub fn tenant_domain_names(domains: &[TenantDomain]) -> Vec<String> {
    domains.iter().map(|held| held.domain.clone()).collect()
}

/// Whether a call stating `on_domain_takeover` asks CloudFront to MOVE a domain another
/// resource already serves.
///
/// "`reject` – CloudFront doesn't move the domain, and the request fails if the domain is
/// associated with another resource. This is the default if you don't specify a value", so a
/// call that states nothing asks for the refusal. A value outside the model's
/// `OnDomainTakeover` enum is refused at the request boundary by the enum's own constraint.
pub fn takes_over_domains(on_domain_takeover: Option<&str>) -> bool {
    on_domain_takeover == Some(ON_DOMAIN_TAKEOVER_ALLOW)
}

/// The distribution a submitted `ViewerFallback` names, or `None` when it names none.
///
/// `ViewerFallback$DistributionId` is "the ID of the distribution whose certificate
/// CloudFront uses to establish TLS connections for viewer requests that don't include SNI",
/// so an element carrying no id — or the empty one, which identifies no distribution —
/// describes no fallback at all and is retained as none, the way a customization naming
/// nothing is ([`retained_customizations`]). ONE reading, so the existence check a create and
/// an update make and the value they store cannot drift apart.
pub fn viewer_fallback_distribution_id(fallback: Option<&ViewerFallback>) -> Option<&str> {
    fallback?
        .distribution_id
        .as_deref()
        .filter(|id| !id.is_empty())
}
