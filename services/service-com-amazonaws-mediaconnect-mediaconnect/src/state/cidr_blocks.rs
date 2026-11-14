// SPDX-License-Identifier: CC0-1.0

//! The one place this crate judges a request member that names an IP ADDRESS RANGE.
//!
//! Several members of this service's requests carry a range of IP addresses the service
//! has to parse to configure the resource — the gateway's egress range
//! (`CreateGatewayRequest$EgressCidrBlocks`) and each of its networks
//! (`GatewayNetwork$CidrBlock`), the range allowed to contribute to a source
//! (`SetSourceRequest$WhitelistCidr`, `UpdateFlowSourceRequest$WhitelistCidr`) and the
//! range allowed to request an output (`AddOutputRequest$CidrAllowList`,
//! `UpdateFlowOutputRequest$CidrAllowList`). Each is documented the same way: the value
//! "should be in the form of a Classless Inter-Domain Routing (CIDR) block; for example,
//! 10.0.0.0/16".
//!
//! That form is what [`is_cidr_block`] encodes, and the one member of this model whose
//! range IS constrained on the wire states it exactly:
//! `PublicRouterNetworkInterfaceRule$Cidr` carries
//! `@pattern("^(?:[0-9]{1,3}\\.){3}[0-9]{1,3}/([0-9]|[12][0-9]|3[0-2])$")`
//! (`smithy_query(query="shape",shape="PublicRouterNetworkInterfaceRule")`) — four dotted
//! decimal octets and a prefix length of 0 to 32. The members listed above carry no
//! pattern, so the framework's boundary walk cannot refuse a value that is not a range at
//! all and the check belongs to the handler, exactly as the tag and maintenance-window
//! formats do.

use std::net::Ipv4Addr;

/// The widest prefix length an IPv4 CIDR block carries: a /32 block is one address.
///
/// The bound the modelled `PublicRouterNetworkInterfaceRule$Cidr` pattern publishes for
/// this service's CIDR members (`([0-9]|[12][0-9]|3[0-2])`, i.e. 0 to 32).
pub const MAX_CIDR_PREFIX_LENGTH: u8 = 32;

/// Whether `value` is a CIDR block: an IPv4 address in dotted decimal, `/`, and a prefix
/// length of 0 to [`MAX_CIDR_PREFIX_LENGTH`].
///
/// Host bits are NOT required to be zero — the published pattern admits a block whose
/// address is not the network address of the range it names — and no other separator,
/// sign, space or suffix is admitted, because a value the service cannot parse as a range
/// configures no range.
pub fn is_cidr_block(value: &str) -> bool {
    let Some((address, prefix_length)) = value.split_once('/') else {
        return false;
    };
    // `Ipv4Addr`'s own parser is the dotted-decimal grammar: four octets of 0..=255, no
    // leading zeros, no octal or hexadecimal form, nothing around them.
    address.parse::<Ipv4Addr>().is_ok() && is_cidr_prefix_length(prefix_length)
}

/// Whether `text` is the prefix length of a CIDR block: decimal digits, no leading zero
/// and no sign, naming a value of 0 to [`MAX_CIDR_PREFIX_LENGTH`].
fn is_cidr_prefix_length(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
        && text
            .parse::<u8>()
            .is_ok_and(|length| length <= MAX_CIDR_PREFIX_LENGTH)
}

/// The detail a request is refused with when `value` — carried by the request member
/// `member` — is not a CIDR block, or `None` when it is (including when the request
/// carried no value at all, because an omitted member names no range to configure).
///
/// Every operation that STORES a range of IP addresses refuses through this one function,
/// so one value cannot be admitted by one of them and refused by another, and nothing
/// that is not a range is ever stored.
pub fn cidr_block_rejection(member: &str, value: Option<&str>) -> Option<String> {
    let value = value?;
    if is_cidr_block(value) {
        return None;
    }
    Some(format!(
        "Invalid {member} {value:?}. A range of IP addresses must be in the form of a \
         Classless Inter-Domain Routing (CIDR) block — an IPv4 address and a prefix length \
         of 0 to {MAX_CIDR_PREFIX_LENGTH} — for example, 10.0.0.0/16."
    ))
}

/// [`cidr_block_rejection`] over a LIST member: the detail the first entry that is not a
/// CIDR block is refused with, naming the entry by its index in `member`, or `None` when
/// every entry is one.
pub fn cidr_block_list_rejection(member: &str, values: Option<&[String]>) -> Option<String> {
    values?.iter().enumerate().find_map(|(index, value)| {
        cidr_block_rejection(&format!("{member}[{index}]"), Some(value))
    })
}
