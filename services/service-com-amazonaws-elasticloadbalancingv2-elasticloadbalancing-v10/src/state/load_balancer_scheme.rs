// SPDX-License-Identifier: CC0-1.0

//! Which load balancers carry a `Scheme` at all, and the one a create gives the ones that
//! do.
//!
//! `Scheme` says where the nodes of a load balancer are reachable from — "the nodes of an
//! Internet-facing load balancer have public IP addresses … the nodes of an internal load
//! balancer have only private IP addresses" — and the member's own documentation closes with
//! two sentences that are not about a VALUE but about WHICH load balancers the member applies
//! to: "The default is an Internet-facing load balancer" and "You can't specify a scheme for
//! a Gateway Load Balancer" (`CreateLoadBalancerInput$Scheme`).
//!
//! So the FAMILY decides whether the load balancer carries a scheme: a family the member is
//! documented for carries one from the moment it exists — the value the request named, else
//! the documented default — and the family it is documented AGAINST carries none, so a create
//! neither defaults one in nor accepts one, and a read reports no `Scheme` member for it
//! (`LoadBalancer$Scheme` is optional, so an absent member is how "this load balancer has no
//! scheme" is reported).
//!
//! Read here ONCE, so the create that mints a load balancer, the reads that report one and
//! every rule keyed on a scheme agree on which load balancers have one.

use super::{family_type_name, LoadBalancerFamily, Refusal, INTERNET_FACING_SCHEME};

/// Whether a load balancer of `family` carries a `Scheme` at all.
///
/// Every family the member is documented for does; the Gateway family does not — "You can't
/// specify a scheme for a Gateway Load Balancer".
pub fn family_carries_scheme(family: LoadBalancerFamily) -> bool {
    family != LoadBalancerFamily::Gateway
}

/// The scheme a load balancer of `family` created with `requested` carries — `None` for a
/// family that carries none — or the rule the request broke.
///
/// A family that carries a scheme takes the one the request named, else the value the member
/// documents as its default. A family that carries none is refused a request that names one
/// at all, rather than storing a scheme no load balancer of that family can have or dropping
/// a value the caller asked for: it is the two members TOGETHER that no load balancer can
/// hold, which is the class Elastic Load Balancing answers as the modelled "the requested
/// configuration is not valid" ([`Refusal::Configuration`]).
pub fn created_scheme(
    family: LoadBalancerFamily,
    requested: Option<&str>,
) -> Result<Option<String>, Refusal> {
    if !family_carries_scheme(family) {
        return match requested {
            Some(_) => Err(Refusal::Configuration(unsupported_scheme_message(family))),
            None => Ok(None),
        };
    }
    Ok(Some(
        requested.unwrap_or(INTERNET_FACING_SCHEME).to_string(),
    ))
}

/// The sentence a request naming a scheme for a family that carries none earns: what it
/// cannot specify, and the type of load balancer it asked for.
///
/// Worded as this service words a member asked for on a load balancer whose family is not
/// documented for it (see [`super::check_subnet_mapping`], which states the same rule for the
/// addresses a `SubnetMapping` pins).
pub fn unsupported_scheme_message(family: LoadBalancerFamily) -> String {
    format!(
        "You cannot specify a scheme with a load balancer of type '{}'",
        family_type_name(family)
    )
}
