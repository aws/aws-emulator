// SPDX-License-Identifier: CC0-1.0

//! The security-group rule store, the rule validation every authorize and revoke shares, and
//! the page-token codec every paginated describe encodes its position with.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The marker every token this service mints carries, ahead of the checksum and the
/// cursor itself: a token a caller assembled carries neither, so it is detectable rather
/// than read as a cursor.
pub(super) const PAGE_TOKEN_VERSION: &str = "ec2-1";

/// Encode a pagination cursor — a resource id — as an opaque token.
///
/// WHICH id is the CALLER's reading, and the two in this crate differ: a describe paged by
/// [`page_from_cursor`] encodes the LAST item of the page it answered and resumes strictly
/// after it, while a describe that still hand-rolls its page encodes the FIRST item of the
/// next page and resumes at it. Nothing here depends on which: the cursor is opaque and
/// only ever presented back to the operation that minted it, so each operation reads its
/// own tokens. A NEW call site should take the exclusive reading, which is the one that can
/// also name a page nothing follows.
/// The token is the hex of `<version>:<checksum>:<id>`, so it is opaque to the caller and
/// a token this service did not mint fails verification in [`decode_page_token`] instead
/// of being treated as an id. Hex alone would not do that: any even-length hex string
/// decodes to something.
pub fn encode_page_token(start_at: &str) -> String {
    let checksum = page_token_checksum(start_at);
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{checksum:016x}:{start_at}").as_bytes())
}

/// Decode a pagination cursor minted by [`encode_page_token`]; `None` for anything else —
/// a token that is not hex, does not carry this service's marker, or whose checksum does
/// not match the cursor it carries.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let marked = decoded
        .strip_prefix(PAGE_TOKEN_VERSION)
        .and_then(|rest| rest.strip_prefix(':'))?;
    let (checksum, start_at) = marked.split_once(':')?;
    let expected = format!("{:016x}", page_token_checksum(start_at));
    (checksum == expected).then(|| start_at.to_string())
}

/// What separates the SCOPE a page cursor was minted for from the position inside that
/// scope, in the cursor a scoped token carries: a character no resource id, Region or
/// checksum of this service carries, so the two halves can never run together.
pub const CURSOR_SCOPE_SEPARATOR: char = '#';

/// Encode a cursor that is only valid inside `scope` — the report, search or VPC the page
/// belongs to — as an opaque token.
///
/// A plain [`encode_page_token`] names a position and nothing else, so a token minted
/// while paging one report resumes inside another report that happens to order its items
/// the same way. A SCOPED token carries what it was minted for beside the position, and
/// [`decode_scoped_page_token`] refuses it for any other scope.
pub fn encode_scoped_page_token(scope: &str, position: &str) -> String {
    encode_page_token(&format!("{scope}{CURSOR_SCOPE_SEPARATOR}{position}"))
}

/// The position inside `scope` that `token` names; `None` for anything else — a token this
/// service did not mint ([`decode_page_token`]), one carrying no scope at all, or one
/// minted for a different scope, which a caller must be refused rather than resumed from.
pub fn decode_scoped_page_token(token: &str, scope: &str) -> Option<String> {
    let cursor = decode_page_token(token)?;
    let (carried, position) = cursor.split_once(CURSOR_SCOPE_SEPARATOR)?;
    (carried == scope).then(|| position.to_string())
}

/// The checksum a token carries over its cursor.
///
/// A token is only ever presented back to the process that minted it — pagination is
/// resumed within one run — so the standard library's hasher is enough to tell a token
/// this service built from one a caller assembled. It is not a secret and nothing depends
/// on it being unforgeable: the cursor it protects is a resource id of the caller's own
/// account, so the check exists to refuse a malformed token, not to withhold anything.
pub(super) fn page_token_checksum(start_at: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = ::std::hash::DefaultHasher::new();
    PAGE_TOKEN_VERSION.hash(&mut hasher);
    start_at.hash(&mut hasher);
    hasher.finish()
}

/// When a paged EC2 describe mints the `NextToken` that tells a caller to ask again.
///
/// The two readings differ only on a page the caller's `MaxResults` did NOT fill, and EC2
/// does not answer them the same way for every operation — which is a fact of each
/// operation's own backend, so the operation names the one it was recorded answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenRule {
    /// A token only when the selected set holds a further item AFTER this page: the
    /// answer looks ahead, so the last page of a walk carries none. This is what every
    /// describe of this service answered before one of them was recorded doing otherwise,
    /// and it is what "this value is null when there are no more items to return" reads
    /// as on its own.
    WhenAnotherItemFollows,
    /// A token whenever the page reported an item at all, whether or not the limit filled
    /// it and without looking ahead: the token means "resume AFTER what I just gave you",
    /// so a walk ends on the TRAILING EMPTY page that token leads to rather than on the
    /// last page that carried items.
    ///
    /// Recorded behaviour, not something the model states: a vpc-scoped, paged
    /// `DescribeSecurityGroups` read answers a `NextToken` on a page that did not reach
    /// `MaxResults`, and the page after it is empty and carries none. A caller must
    /// therefore follow the token rather than stop at a short page — which is the whole
    /// reason this is modelled instead of assumed away.
    WhenThePageHeldItems,
}

/// One page of `ordered`, and the `NextToken` that page carries.
///
/// `ordered` is the whole selected set in the stable order the cursor names a position
/// in — the id order each describe sorts into — `cursor` is the cursor
/// [`decode_page_token`] read out of the request's `NextToken`, and `limit` is the page
/// size: `None` for an operation whose `MaxResults` doc says a request naming none is not
/// paged at all, which is then reported in one answer with no token whichever `rule` is
/// named.
///
/// A cursor names the LAST item of the page it was issued with and the next page resumes
/// AFTER it, so a token can be minted without there being a known item to name — which is
/// what [`PageTokenRule::WhenThePageHeldItems`] needs — and a cursor whose resource has
/// been deleted since still resumes at the right place.
///
/// Paging on the cursor rather than on an index is what keeps a walk correct when the
/// store changes under it: a token names an id, so a resource inserted or deleted ahead
/// of the cursor cannot shift the page's contents, and the cursor always advances because
/// the order is total.
///
/// MIGRATION, so the two readings in the crate are not mistaken for a bug: the describes
/// that have not moved onto this helper yet still hand-roll the page with an INCLUSIVE
/// cursor — their token names the FIRST item of the NEXT page — which cannot express a
/// token for a page nothing follows. Nothing breaks while both exist, because a token is
/// opaque and only ever presented back to the operation that minted it; each describe
/// moves over as the issue that owns its behaviour reaches it.
pub fn page_from_cursor<'a, T>(
    ordered: &'a [T],
    cursor: Option<&str>,
    limit: Option<usize>,
    rule: PageTokenRule,
    id_of: impl Fn(&T) -> &str,
) -> (&'a [T], Option<String>) {
    let (page, position) = page_and_position(ordered, cursor, limit, rule, &id_of);
    (page, position.map(encode_page_token))
}

/// One page of `ordered`, and the `NextToken` that page carries SCOPED to `scope` — the
/// report the page belongs to.
///
/// [`page_from_cursor`]'s rule exactly, token minted through [`encode_scoped_page_token`]
/// instead: an operation that reports the items of ONE resource (the entries of one prefix
/// list, the routes of one endpoint) has one position order per resource, so a token minted
/// while paging one of them must not resume inside another's page. The scope is what
/// [`decode_scoped_page_token`] checks the token back against.
pub fn scoped_page_from_cursor<'a, T>(
    scope: &str,
    ordered: &'a [T],
    cursor: Option<&str>,
    limit: Option<usize>,
    rule: PageTokenRule,
    id_of: impl Fn(&T) -> &str,
) -> (&'a [T], Option<String>) {
    let (page, position) = page_and_position(ordered, cursor, limit, rule, &id_of);
    (
        page,
        position.map(|position| encode_scoped_page_token(scope, position)),
    )
}

/// One page of `ordered`, and the POSITION the token that page carries names — `None` when
/// no token is due.
///
/// The one reading of the paging rule, so the plain and the scoped form above cannot differ
/// on where a page starts, how long it is or when a token is due: they differ only in how
/// the position is encoded.
pub(super) fn page_and_position<'a, T>(
    ordered: &'a [T],
    cursor: Option<&str>,
    limit: Option<usize>,
    rule: PageTokenRule,
    id_of: &impl Fn(&T) -> &str,
) -> (&'a [T], Option<&'a str>) {
    // The cursor is a POSITION in `ordered`, found by binary search, so an unsorted set
    // would silently skip items rather than fail: a caller that has not sorted by the id
    // `id_of` reads is the bug, and it fails loudly here in a debug or test build.
    debug_assert!(
        ordered
            .windows(2)
            .all(|pair| id_of(&pair[0]) <= id_of(&pair[1])),
        "page_from_cursor needs its set in ascending id order"
    );
    let from = match cursor {
        None => 0,
        Some(cursor) => ordered.partition_point(|item| id_of(item) <= cursor),
    };
    let rest = ordered.get(from..).unwrap_or_default();
    let Some(limit) = limit else {
        return (rest, None);
    };
    let page = rest.get(..limit.min(rest.len())).unwrap_or_default();
    let due = match rule {
        PageTokenRule::WhenAnotherItemFollows => rest.len() > page.len(),
        PageTokenRule::WhenThePageHeldItems => !page.is_empty(),
    };
    // `page.last()` is `None` only for an empty page, which no rule above mints a token
    // for: there is no item for the cursor to name.
    let position = page.last().filter(|_| due).map(id_of);
    (page, position)
}

// ---------------------------------------------------------------------------
// Network interfaces
// ---------------------------------------------------------------------------

// The status an unattached interface sits in and the default `NetworkInterfaceType`
// ([`NETWORK_INTERFACE_STATUS_AVAILABLE`] and [`NETWORK_INTERFACE_TYPE_INTERFACE`]) are
// defined in the network-interface section further down, beside the attachment statuses the
// store derives an interface's status from, so one definition serves every operation.

/// The interface type that carries no IP addresses at all: "If you specify `efa-only`, do
/// not assign any IP addresses to the network interface. EFA-only network interfaces do
/// not support IP addresses" (`CreateNetworkInterfaceRequest$InterfaceType`).
pub const NETWORK_INTERFACE_TYPE_EFA_ONLY: &str = "efa-only";

/// The `ResourceType` a `CreateNetworkInterface` tag specification may name: the network
/// interface it creates. Also the type `RunInstances` accepts for the interfaces of a
/// launch (see [`RUN_INSTANCES_TAGGABLE_RESOURCES`]).
pub const RESOURCE_TYPE_NETWORK_INTERFACE: &str = "network-interface";

/// The largest number of network interfaces this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`] and [`MAX_SUBNETS`], not an AWS
/// quota (AWS's "Network interfaces per Region" quota is adjustable): each interface is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_network_interface_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`NETWORK_INTERFACE_LIMIT_EXCEEDED`].
pub const MAX_NETWORK_INTERFACES: usize = 5_000;

/// The most private IPv4 addresses one network interface may hold here.
///
/// CHOSEN: "The number of IP addresses you can assign to a network interface varies by
/// instance type" (`CreateNetworkInterface`), and 50 is the largest per-interface IPv4
/// count AWS publishes for any instance type (Amazon EC2 User Guide, "IP addresses per
/// network interface per instance type"). It is the bound a request's
/// `SecondaryPrivateIpAddressCount` and `PrivateIpAddresses` are held to, because the
/// model declares no `@range` on either and each address is a record this store keeps.
pub const MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE: usize = 50;

/// EC2's error code for a create that would take the account past the number of network
/// interfaces it may hold: "You've reached the limit on the number of network interfaces
/// that you can create" (EC2 API error codes).
pub const NETWORK_INTERFACE_LIMIT_EXCEEDED: &str = "NetworkInterfaceLimitExceeded";

/// `NetworkInterfaceLimitExceeded` (HTTP 400): the store already holds
/// [`MAX_NETWORK_INTERFACES`] interfaces.
pub fn network_interface_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        NETWORK_INTERFACE_LIMIT_EXCEEDED,
        400,
        format!(
            "This emulator holds at most {MAX_NETWORK_INTERFACES} network interfaces, \
             so it cannot create another one"
        ),
    )
}

// ---------------------------------------------------------------------------
// The VPC-scoped network resource lifecycle
// ---------------------------------------------------------------------------
//
// A subnet and a security group are resources INSIDE a VPC: each is created into one
// named VPC, reported by that resource's describe, and deleted once nothing depends on
// it — and the VPC itself can only be deleted once it holds none of them, because
// "you must detach or delete all gateways and resources that are associated with the
// VPC before you can delete it … When you delete the VPC, it deletes the default
// security group, network ACL, and route table for the VPC" (`DeleteVpc`). What those
// operations share — the dependency question, the removal that gives the record's
// bytes back, and the error codes a refusal carries — lives here, so one rule cannot be
// enforced two ways. The security-group CREATE path's own helpers (its text alphabet,
// its id form) are below, beside the group create that reads them.

/// A subnet AWS is still creating: the `SubnetState` enum's `pending` member
/// (`smithy_query(query="shape",shape="SubnetState")`), which is what `CreateSubnet`
/// answers with until [`TRANSITION_DELAY`] has passed — the subnet counterpart of
/// [`VPC_STATE_PENDING`].
pub const SUBNET_STATE_PENDING: &str = "pending";

/// The `ResourceType` a `CreateSubnet` tag specification may name: the subnet it creates.
pub const RESOURCE_TYPE_SUBNET: &str = "subnet";

/// The `ResourceType` a `CreateSecurityGroup` tag specification may name: the group it
/// creates (`ResourceType` enum member `security-group`).
pub const RESOURCE_TYPE_SECURITY_GROUP: &str = "security-group";

/// How many characters a security group name or description may carry: "Up to 255
/// characters in length" (`CreateSecurityGroupRequest$GroupName` and `$Description`).
/// Both members carry the same bound, which is why one constant serves both.
pub const MAX_SECURITY_GROUP_TEXT_CHARS: usize = 255;

/// The punctuation a security group name or description may carry beside letters, digits
/// and spaces: "Valid characters: a-z, A-Z, 0-9, spaces, and ._-:/()#,@[]+=&;{}!$*"
/// (`CreateSecurityGroupRequest$GroupName` and `$Description`).
pub const SECURITY_GROUP_PUNCTUATION: &str = "._-:/()#,@[]+=&;{}!$*";

/// The prefix a security group name may NOT start with: "Can't start with sg-"
/// (`CreateSecurityGroupRequest$GroupName`), because that is the shape of a group id.
pub const SECURITY_GROUP_ID_PREFIX: &str = "sg-";

/// The largest number of security groups this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each group is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_security_group_capacity`] is the predicate a
/// writer asks first; a create past it refuses with [`SECURITY_GROUP_LIMIT_EXCEEDED`].
pub const MAX_SECURITY_GROUPS: usize = 2_500;

/// How many rules one security group may hold in ONE direction, per IP address family.
///
/// A published AWS quota, not a capacity bound invented here: "Inbound or outbound rules
/// per security group: 60" (Amazon VPC quotas, "Security groups"), the same value this
/// emulator's Service Quotas surface publishes for the `vpc` quota of that name, so the two
/// surfaces agree on what a caller may add. The quota is counted per direction AND per
/// address family — the published quota applies to a group's IPv4 rules and to its IPv6
/// rules separately — which together are the buckets
/// [`ComAmazonawsEc2Amazonec2State::has_security_group_rule_capacity`] weighs a request
/// against.
pub const MAX_SECURITY_GROUP_RULES_PER_DIRECTION: usize = 60;

/// The IP address families a group's rules are counted in separately, one bucket of a
/// direction each.
pub const SECURITY_GROUP_RULE_FAMILIES: [AddressFamily; 2] =
    [AddressFamily::Ipv4, AddressFamily::Ipv6];

/// The largest number of security group rules this in-memory store holds.
///
/// Derived rather than chosen: the published per-direction quota, in each address family of
/// both directions, for every group the store can hold ([`MAX_SECURITY_GROUPS`]). A ceiling
/// is needed for the reason [`MAX_STORED_INSTANCES`] gives — nothing reclaims a stored rule
/// except the delete of its group — and a per-group quota alone would not bound a store that
/// can hold many groups. The bytes each rule retains are charged against
/// [`MAX_STORED_BYTES`] as well ([`security_group_rule_payload_bytes`]), because a record
/// count alone is not a memory bound.
pub const MAX_SECURITY_GROUP_RULES: usize = MAX_SECURITY_GROUPS
    * 2
    * SECURITY_GROUP_RULE_FAMILIES.len()
    * MAX_SECURITY_GROUP_RULES_PER_DIRECTION;

/// EC2's error code for an authorize that would take a security group past its
/// per-direction rule quota (EC2 API error codes, "RulesPerSecurityGroupLimitExceeded").
pub const RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED: &str = "RulesPerSecurityGroupLimitExceeded";

/// The message EC2 refuses an over-quota authorize with, in full.
///
/// Entirely FIXED text with no variable part: the rules-per-security-group quota is
/// adjustable per account (Service Quotas, service code `vpc`), so the value in force is a
/// property of the account rather than of the refusal, and the service does not name it
/// here. A number interpolated into this message would be a value a caller could parse and
/// depend on, and it would differ from account to account for the same refusal.
pub const RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED_MESSAGE: &str =
    "The maximum number of rules per security group has been reached.";

/// Refuse an authorize that would take a security group past
/// [`MAX_SECURITY_GROUP_RULES_PER_DIRECTION`] rules in one direction: HTTP 400
/// `RulesPerSecurityGroupLimitExceeded`. A quota, not something the request got wrong, so it
/// carries the quota code an SDK classifies as one, with the service's fixed message
/// ([`RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED_MESSAGE`]).
pub fn rules_per_security_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED,
        400,
        RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED_MESSAGE,
    )
}

/// EC2's error code for a create that would take the account past its security group
/// quota (EC2 API error codes, "SecurityGroupLimitExceeded").
pub const SECURITY_GROUP_LIMIT_EXCEEDED: &str = "SecurityGroupLimitExceeded";

/// EC2's error code for a delete the resource's own dependencies forbid — "the operation
/// fails with `DependencyViolation`" (`DeleteSecurityGroup`).
pub const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// EC2's error code for a delete of a resource the service owns rather than the caller:
/// the default security group a VPC comes with, which only `DeleteVpc` removes.
pub const CANNOT_DELETE: &str = "CannotDelete";

/// EC2's error code for a subnet CIDR block that overlaps one already in the VPC: "A
/// subnet CIDR block must not overlap the CIDR block of an existing subnet in the VPC"
/// (`CreateSubnet`).
pub const INVALID_SUBNET_CONFLICT: &str = "InvalidSubnet.Conflict";

/// EC2's error code for a subnet CIDR block that is not inside its VPC's: a subnet is a
/// range OF the VPC's range, so a block outside it names no part of the VPC.
pub const INVALID_SUBNET_RANGE: &str = "InvalidSubnet.Range";

/// The smallest IPv4 prefix length a SUBNET CIDR block may have: "The allowed size for an
/// IPv4 subnet is between a /28 netmask (16 IP addresses) and a /16 netmask (65,536 IP
/// addresses)" (`CreateSubnet`) — the same published bounds as a VPC's own block, named
/// separately because they come from a different sentence and could move apart.
pub const MIN_SUBNET_IPV4_PREFIX: u8 = 16;

/// The largest IPv4 prefix length a subnet CIDR block may have (see
/// [`MIN_SUBNET_IPV4_PREFIX`]).
pub const MAX_SUBNET_IPV4_PREFIX: u8 = 28;

/// A requested IPv4 SUBNET CIDR block in the canonical form AWS stores it in.
///
/// "We modify the specified CIDR block to its canonical form; for example, if you specify
/// 100.68.0.18/18, we modify it to 100.68.0.0/18" (`CreateSubnetRequest$CidrBlock`) — the
/// same canonicalisation as a VPC's block, held to the subnet block size
/// ([`MIN_SUBNET_IPV4_PREFIX`]..=[`MAX_SUBNET_IPV4_PREFIX`]).
pub fn canonical_subnet_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_SUBNET_IPV4_PREFIX..=MAX_SUBNET_IPV4_PREFIX).contains(&prefix_length) {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// The one prefix length an IPv6 SUBNET CIDR block may have.
///
/// "The IPv6 CIDR block size is fixed at /64" (Amazon VPC User Guide, "IP addressing for
/// your VPCs and subnets" → "Subnet CIDR blocks"), so unlike the IPv4 side this is a
/// single size rather than a range: an IPv6 subnet block is a /64 of one of the IPv6
/// blocks associated with its VPC.
pub const SUBNET_IPV6_PREFIX_LENGTH: u8 = 64;

/// A requested IPv6 SUBNET CIDR block in the canonical form AWS stores it in.
///
/// The same canonicalisation every IPv6 block of this crate goes through
/// ([`canonical_ipv6_cidr`] — the host bits cleared, the address rendered the one way
/// `Ipv6Addr` renders it), held to the one published subnet block size
/// ([`SUBNET_IPV6_PREFIX_LENGTH`]). It is the IPv6 counterpart of
/// [`canonical_subnet_ipv4_cidr`], so the two families differ only in the published size.
pub fn canonical_subnet_ipv6_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv6_cidr(value)?;
    let (_, prefix_length) = split_ipv6_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if prefix_length != SUBNET_IPV6_PREFIX_LENGTH {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// The smallest IPv4 prefix length a SECONDARY NETWORK CIDR block may have: "The allowed
/// size for a secondary network CIDR block is between /28 netmask (16 IP addresses) and /12
/// netmask (1,048,576 IP addresses)" (`CreateSecondaryNetwork`), also stated on the member
/// itself — "The CIDR block size must be between /12 and /28"
/// (`CreateSecondaryNetworkRequest$Ipv4CidrBlock`). Named separately from the VPC and
/// subnet bounds because it comes from a different sentence.
pub const MIN_SECONDARY_NETWORK_IPV4_PREFIX: u8 = 12;

/// The largest IPv4 prefix length a secondary network CIDR block may have (see
/// [`MIN_SECONDARY_NETWORK_IPV4_PREFIX`]).
pub const MAX_SECONDARY_NETWORK_IPV4_PREFIX: u8 = 28;

/// A requested IPv4 SECONDARY NETWORK CIDR block in the canonical form AWS stores it in:
/// the same canonicalisation as a VPC's and a subnet's block, held to the secondary
/// network block size
/// ([`MIN_SECONDARY_NETWORK_IPV4_PREFIX`]..=[`MAX_SECONDARY_NETWORK_IPV4_PREFIX`]).
pub fn canonical_secondary_network_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_SECONDARY_NETWORK_IPV4_PREFIX..=MAX_SECONDARY_NETWORK_IPV4_PREFIX)
        .contains(&prefix_length)
    {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// `value` as an IPv4 CIDR block with its host bits cleared, with NO rule on the prefix
/// length: the one reader [`canonical_vpc_ipv4_cidr`], [`canonical_subnet_ipv4_cidr`] and
/// [`canonical_secondary_network_ipv4_cidr`] put their own published block size on top of.
///
/// Public, like its IPv6 twin [`canonical_ipv6_cidr`], for the ranges the model and the
/// docs publish no size for at all — a subnet CIDR reservation's range, whose only stated
/// rule is the subnet range it has to sit inside — where a bound invented here would refuse
/// blocks AWS accepts.
pub fn canonical_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let (address, prefix_length) = value.split_once('/').ok_or(CidrProblem::Malformed)?;
    let base = u32::from(
        address
            .parse::<::std::net::Ipv4Addr>()
            .map_err(|_| CidrProblem::Malformed)?,
    );
    let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
    if prefix_length > 32 {
        // No IPv4 block has a prefix longer than its address, so the value is malformed
        // rather than a block of a disallowed size.
        return Err(CidrProblem::Malformed);
    }
    let mask = u32::MAX
        .checked_shl(u32::from(32 - prefix_length))
        .unwrap_or_default();
    Ok(format!(
        "{}/{prefix_length}",
        ::std::net::Ipv4Addr::from(base & mask)
    ))
}

/// `block` as its base address and prefix length, for a block already in canonical form.
pub(super) fn split_ipv4_cidr(block: &str) -> Option<(u32, u8)> {
    let (address, prefix_length) = block.split_once('/')?;
    let base = u32::from(address.parse::<::std::net::Ipv4Addr>().ok()?);
    let prefix_length: u8 = prefix_length.parse().ok()?;
    (prefix_length <= 32).then_some((base, prefix_length))
}

/// The first and last address of `block`, or `None` when it is not an IPv4 CIDR block.
pub(super) fn ipv4_cidr_bounds(block: &str) -> Option<(u32, u32)> {
    let (base, prefix_length) = split_ipv4_cidr(block)?;
    let size = 1u64 << (32 - u32::from(prefix_length));
    let last = u64::from(base).checked_add(size - 1)?;
    Some((base, u32::try_from(last).ok()?))
}

/// Do `left` and `right` share any address?
///
/// Two CIDR blocks of the same family overlap exactly when one's range meets the other's,
/// which is the question "A subnet CIDR block must not overlap the CIDR block of an
/// existing subnet in the VPC" (`CreateSubnet`) asks. A block this emulator cannot read
/// overlaps nothing, rather than being guessed at.
pub fn ipv4_cidr_blocks_overlap(left: &str, right: &str) -> bool {
    match (ipv4_cidr_bounds(left), ipv4_cidr_bounds(right)) {
        (Some((left_first, left_last)), Some((right_first, right_last))) => {
            left_first <= right_last && right_first <= left_last
        }
        _ => false,
    }
}

/// Is every address of `inner` an address of `outer`?
///
/// A subnet's block has to be a block OF its VPC's block, so this is the containment the
/// create path checks before the overlap one. An unreadable block contains nothing and is
/// contained by nothing.
pub fn ipv4_cidr_block_contains(outer: &str, inner: &str) -> bool {
    match (ipv4_cidr_bounds(outer), ipv4_cidr_bounds(inner)) {
        (Some((outer_first, outer_last)), Some((inner_first, inner_last))) => {
            outer_first <= inner_first && inner_last <= outer_last
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// CIDR blocks of EITHER family, for the rules that are written for both
// ---------------------------------------------------------------------------

/// `address` as the family it names and the integer its bits make, or `None` when it is not
/// an IP address at all.
///
/// The family-agnostic counterpart of the IPv4-only readers above, for the rules the model
/// writes for both families at once ("The peer address and transit gateway address must be
/// the same IP address family (IPv4 or IPv6)", `CreateTransitGatewayConnectPeer`). The
/// address itself is parsed by the standard library's own `IpAddr` parser, the same parser
/// those readers use for each family alone.
pub(super) fn ip_address_bits(address: &str) -> Option<(bool, u128)> {
    match address.parse::<::std::net::IpAddr>().ok()? {
        ::std::net::IpAddr::V4(address) => Some((false, u128::from(u32::from(address)))),
        ::std::net::IpAddr::V6(address) => Some((true, u128::from(address))),
    }
}

/// Whether `address` is an IPv6 address, or `None` when it is not an address of either
/// family.
pub fn ip_address_is_ipv6(address: &str) -> Option<bool> {
    ip_address_bits(address).map(|(is_ipv6, _)| is_ipv6)
}

/// `block` as its family, its first address and its last, with the host bits of the base
/// cleared, or `None` when it is not a CIDR block of either family.
///
/// The family-agnostic form of the IPv4 bounds above, for the two questions the Connect peer
/// rules ask of a block of either family: which addresses it holds
/// ([`nth_address_in_cidr_block`]) and whether another block or address lies inside it
/// ([`cidr_block_contains_block`], [`cidr_block_contains_address`]).
pub(super) fn cidr_block_bounds(block: &str) -> Option<(bool, u128, u128)> {
    let (address, prefix_length) = block.split_once('/')?;
    let (is_ipv6, base) = ip_address_bits(address)?;
    let width: u32 = if is_ipv6 { 128 } else { 32 };
    let prefix_length: u32 = prefix_length.parse().ok()?;
    if prefix_length > width {
        return None;
    }
    let host_bits = width - prefix_length;
    let span = 1u128.checked_shl(host_bits).unwrap_or_default().wrapping_sub(1);
    let first = base & !span;
    Some((is_ipv6, first, first | span))
}

/// The prefix length `block` names, or `None` when it is not a CIDR block of either family.
pub fn cidr_block_prefix_length(block: &str) -> Option<u32> {
    cidr_block_bounds(block)?;
    let (_, prefix_length) = block.split_once('/')?;
    prefix_length.parse().ok()
}

/// Does `block` name a RANGE rather than an address inside one — are the host bits of the
/// address it names clear?
///
/// Asked of the numbers rather than of the spelling, so two spellings of one range are one
/// range and only a misaligned block is a different thing from the range it lies in. `false`
/// for anything that is not a CIDR block of either family.
pub fn cidr_block_names_its_range(block: &str) -> bool {
    let named = block
        .split_once('/')
        .and_then(|(address, _)| ip_address_bits(address));
    match (named, cidr_block_bounds(block)) {
        (Some((_, named)), Some((_, first, _))) => named == first,
        _ => false,
    }
}

/// `block`'s own base address in canonical form, or `None` when it is not a CIDR block.
///
/// The spelling a block is REPORTED in, whichever of its equivalent spellings a request
/// named it by; whether it names its range at all is [`cidr_block_names_its_range`].
pub fn canonical_cidr_block(block: &str) -> Option<String> {
    let prefix_length = cidr_block_prefix_length(block)?;
    Some(format!(
        "{}/{prefix_length}",
        nth_address_in_cidr_block(block, 0)?
    ))
}

/// The address `index` places into `block`, or `None` when `block` is not a CIDR block or
/// does not reach that far.
///
/// Counted from the block's own base address, which is address 0.
pub fn nth_address_in_cidr_block(block: &str, index: u32) -> Option<String> {
    let (is_ipv6, first, last) = cidr_block_bounds(block)?;
    let address = first.checked_add(u128::from(index))?;
    if address > last {
        return None;
    }
    address_text(is_ipv6, address)
}

/// The address `value` as its family spells it: the ONE formatting of a numeric address in
/// this crate, so a block cut out of a range and an address counted into one read the same.
fn address_text(is_ipv6: bool, value: u128) -> Option<String> {
    if is_ipv6 {
        return Some(Ipv6Addr::from(value).to_string());
    }
    Some(::std::net::Ipv4Addr::from(u32::try_from(value).ok()?).to_string())
}

/// The `index`th block of `netmask_length` bits inside `range`, or `None` when `range` is not
/// a CIDR block, the size is not one `range` can be cut into, or the range does not reach that
/// far.
///
/// Counted from the range's own base address, which is block 0, through the ONE reading of a
/// block's numeric bounds in this crate ([`cidr_block_bounds`]) and the one formatting of an
/// address of either family ([`nth_address_in_cidr_block`]) — so a block chosen out of a range
/// is spelled exactly as every other block of this crate is.
///
/// What IPAM hands a pool that asks for a SIZE rather than a range
/// (`crate::ops::provision_ipam_pool_cidr`): the caller walks the index to find a free one.
pub fn nth_cidr_block_in(range: &str, netmask_length: u32, index: u128) -> Option<String> {
    let (is_ipv6, first, last) = cidr_block_bounds(range)?;
    let width: u32 = if is_ipv6 { 128 } else { 32 };
    // A netmask of 0 names the whole address space rather than a block inside this range, and
    // one past the family's width names no block at all.
    if netmask_length == 0 || netmask_length > width {
        return None;
    }
    let size = 1u128.checked_shl(width - netmask_length)?;
    let base = index.checked_mul(size)?.checked_add(first)?;
    let block_last = base.checked_add(size - 1)?;
    if block_last > last {
        return None;
    }
    Some(format!(
        "{}/{netmask_length}",
        address_text(is_ipv6, base)?
    ))
}

/// Is every address of `inner` an address of `outer`?
///
/// The family-agnostic form of [`ipv4_cidr_block_contains`]: blocks of different families
/// share no address, and a block this emulator cannot read contains nothing and is contained
/// by nothing.
pub fn cidr_block_contains_block(outer: &str, inner: &str) -> bool {
    match (cidr_block_bounds(outer), cidr_block_bounds(inner)) {
        (Some((outer_ipv6, outer_first, outer_last)), Some((inner_ipv6, inner_first, inner_last))) => {
            outer_ipv6 == inner_ipv6 && outer_first <= inner_first && inner_last <= outer_last
        }
        _ => false,
    }
}

/// Is `address` an address of `block`? An address of another family is not, and neither is
/// anything this emulator cannot read as an address.
pub fn cidr_block_contains_address(block: &str, address: &str) -> bool {
    match (cidr_block_bounds(block), ip_address_bits(address)) {
        (Some((block_ipv6, first, last)), Some((address_ipv6, value))) => {
            block_ipv6 == address_ipv6 && (first..=last).contains(&value)
        }
        _ => false,
    }
}

/// The addresses of `cidr_block` a network interface may be given, as a range of numbers.
///
/// "AWS reserves five IP addresses in each subnet CIDR block"
/// ([`SUBNET_RESERVED_ADDRESSES`]) — the first four and the last — so the window starts
/// four above the block's base and stops below its broadcast address. The ONE computation
/// of that window: the point question ([`is_assignable_subnet_address`]) and the search
/// ([`ComAmazonawsEc2Amazonec2State::free_subnet_address`]) both read it, so the two cannot
/// drift. A block this emulator cannot read has no window at all, and one too small to
/// hold an assignable address has an empty one, rather than a guess.
pub(super) fn assignable_subnet_window(cidr_block: &str) -> Option<::std::ops::Range<u32>> {
    let (base, prefix_length) = parse_ipv4_cidr_block(cidr_block)?;
    let size = 1u64 << (32 - u64::from(prefix_length));
    // The block's own base, masked here rather than trusted from the parser, so the window
    // is the block's even if a stored CIDR ever carries host bits.
    let base = u64::from(base) & !(size - 1);
    let lowest = u32::try_from(base + u64::from(SUBNET_RESERVED_ADDRESSES) - 1).ok()?;
    let broadcast = u32::try_from(base + size - 1).ok()?;
    Some(lowest..broadcast)
}

/// Is `address` an address of `cidr_block` that a network interface may be given?
///
/// "AWS reserves five IP addresses in each subnet CIDR block"
/// ([`SUBNET_RESERVED_ADDRESSES`]) — the first four and the last — so an address of the
/// block is assignable exactly when it is neither of those
/// ([`assignable_subnet_window`]). The ONE definition of that question:
/// [`ComAmazonawsEc2Amazonec2State::mint_private_ip_in_cidr`] draws from exactly these
/// addresses, so a service-chosen address and a caller-named one are judged the same way.
/// An unreadable block has no assignable address, rather than a guess.
pub fn is_assignable_subnet_address(cidr_block: &str, address: &str) -> bool {
    let Some(window) = assignable_subnet_window(cidr_block) else {
        return false;
    };
    let Ok(candidate) = address.parse::<::std::net::Ipv4Addr>() else {
        return false;
    };
    window.contains(&u32::from(candidate))
}

// ---------------------------------------------------------------------------
// The IPv6 side of a subnet's addresses: the block a subnet holds, which of its
// addresses a network interface may be given, which prefixes may be delegated
// out of it, and what is taken already
// ---------------------------------------------------------------------------

/// Does an association in this `SubnetCidrBlockStateCode` / `VpcCidrBlockStateCode` state
/// still HOLD the block it names?
///
/// `associating` and `associated` do — the block is on its way in or in — while
/// `disassociating`, `disassociated`, `failing` and `failed` do not: the block is on its
/// way out, gone, or was never taken. An association carrying no state at all is read as
/// holding its block, since every writer of this crate sets one. The two enums share these
/// words (`smithy_query(query="shape",shape="SubnetCidrBlockStateCode,VpcCidrBlockStateCode")`),
/// so ONE reader answers for a subnet's association and for its VPC's: every operation that
/// asks whether a block is held reads this rather than keeping a copy of the rule.
pub fn cidr_association_holds_its_block(state: Option<&str>) -> bool {
    match state {
        None => true,
        Some(state) => {
            state == CIDR_BLOCK_STATE_ASSOCIATING || state == CIDR_BLOCK_STATE_ASSOCIATED
        }
    }
}

/// How many IPv6 addresses at the BOTTOM of a subnet's CIDR block are not available to a
/// network interface.
///
/// AWS reserves the first four addresses of a subnet's IPv6 CIDR block — the network
/// address and three it keeps for itself (Amazon VPC User Guide, "Subnet CIDR blocks") —
/// and, unlike the IPv4 side ([`SUBNET_RESERVED_ADDRESSES`]), nothing at the TOP of the
/// block: IPv6 has no broadcast address.
pub const SUBNET_RESERVED_IPV6_ADDRESSES: u128 = 4;

/// The prefix length of an IPv6 prefix this service delegates to a network interface when
/// the request asks for a NUMBER of prefixes instead of naming them: Prefix Delegation
/// hands out a /80 of the subnet's block (Amazon EC2 User Guide, "Assigning prefixes to
/// network interfaces", which `AssignIpv6Addresses` points at for its `Ipv6PrefixCount`).
pub const IPV6_PREFIX_DELEGATION_PREFIX_LENGTH: u8 = 80;

/// `address` in the one text [`Ipv6Addr`] renders an IPv6 address in, or `None` when it is
/// not an IPv6 address at all.
///
/// One address has many spellings — `2600:1F16:0:0:0:0:0:5` and `2600:1f16::5` are the same
/// address — so an address is canonicalised on the way into the store and compared in that
/// form, the way [`canonical_ipv6_cidr`] canonicalises a block.
pub fn canonical_ipv6_address(address: &str) -> Option<String> {
    address
        .parse::<Ipv6Addr>()
        .ok()
        .map(|address| address.to_string())
}

/// The IPv6 addresses of `block` a network interface may be given, as a range of numbers.
///
/// The block's addresses less the [`SUBNET_RESERVED_IPV6_ADDRESSES`] AWS reserves at its
/// bottom. The ONE computation of that window: the point question
/// ([`is_assignable_subnet_ipv6_address`]) and the search
/// ([`free_subnet_ipv6_addresses`]) both read it, so a service-chosen address is always one
/// a caller could have named. A block this emulator cannot read has no window at all, and
/// one too small to hold an assignable address has none either, rather than a guess.
pub(super) fn assignable_subnet_ipv6_window(block: &str) -> Option<::std::ops::RangeInclusive<u128>> {
    let (base, prefix_length) = split_ipv6_cidr(block)?;
    let size = 1u128
        .checked_shl(u32::from(128 - prefix_length))
        .unwrap_or_default();
    // The block's own base, masked here rather than trusted from the parser, so the window
    // is the block's even if a stored CIDR ever carries host bits. A `/0` spans the whole
    // space, whose size does not fit a `u128`.
    let (base, last) = match size {
        0 => (0, u128::MAX),
        size => (base & !(size - 1), (base & !(size - 1)).checked_add(size - 1)?),
    };
    let lowest = base.checked_add(SUBNET_RESERVED_IPV6_ADDRESSES)?;
    (lowest <= last).then_some(lowest..=last)
}

/// Is `address` an address of `block` that a network interface may be given?
///
/// The IPv6 counterpart of [`is_assignable_subnet_address`], on the same terms: an address
/// of the block is assignable exactly when it is outside the reserved window
/// ([`assignable_subnet_ipv6_window`]), and an unreadable block has no assignable address.
pub fn is_assignable_subnet_ipv6_address(block: &str, address: &str) -> bool {
    let Some(window) = assignable_subnet_ipv6_window(block) else {
        return false;
    };
    let Ok(candidate) = address.parse::<Ipv6Addr>() else {
        return false;
    };
    window.contains(&u128::from(candidate))
}

/// Every IPv6 address something already holds, as the merged, sorted ranges of the address
/// space they cover.
///
/// One range per address listed on an interface, one per prefix delegated to one, merged so
/// adjoining and overlapping holdings become one span. That is what lets the two searches
/// below SKIP a whole holding in one step instead of walking its addresses: a `/80` covers
/// 2^48 of them, so an address-at-a-time scan past one is not work this emulator will do for
/// a single request.
pub(super) fn taken_ipv6_ranges(taken_addresses: &[String], taken_prefixes: &[String]) -> Vec<(u128, u128)> {
    let mut ranges: Vec<(u128, u128)> = Vec::with_capacity(
        taken_addresses.len().saturating_add(taken_prefixes.len()),
    );
    for address in taken_addresses {
        if let Ok(parsed) = address.parse::<Ipv6Addr>() {
            let value = u128::from(parsed);
            ranges.push((value, value));
        }
    }
    for prefix in taken_prefixes {
        if let Some(bounds) = ipv6_cidr_bounds(prefix) {
            ranges.push(bounds);
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(u128, u128)> = Vec::with_capacity(ranges.len());
    for (first, last) in ranges {
        match merged.last_mut() {
            Some((_, held_last)) if first <= held_last.saturating_add(1) => {
                *held_last = (*held_last).max(last);
            }
            _ => merged.push((first, last)),
        }
    }
    merged
}

/// The merged range of `taken` that covers `candidate`, or `None` when nothing holds it.
///
/// `taken` is sorted and non-overlapping ([`taken_ipv6_ranges`]), so the covering range is
/// found by bisection rather than by a scan: the question is asked once per candidate and
/// once per holding skipped.
pub(super) fn taken_ipv6_range_at(taken: &[(u128, u128)], candidate: u128) -> Option<(u128, u128)> {
    let after = taken.partition_point(|(first, _)| *first <= candidate);
    taken
        .get(after.checked_sub(1)?)
        .copied()
        .filter(|(_, last)| candidate <= *last)
}

/// Is the IPv6 address `address` already held — listed on a network interface, or inside a
/// prefix delegated to one?
///
/// A delegated prefix is the interface's whole range ("One or more IPv6 prefixes assigned
/// to the network interface"), so its addresses are as held as the ones listed one by one.
/// The ONE definition of "held" for an IPv6 address, so what a caller may name and what the
/// service may choose cannot disagree about who holds what.
pub fn ipv6_address_is_taken(
    address: &str,
    taken_addresses: &[String],
    taken_prefixes: &[String],
) -> bool {
    let Some(canonical) = canonical_ipv6_address(address) else {
        return false;
    };
    let Ok(parsed) = canonical.parse::<Ipv6Addr>() else {
        return false;
    };
    taken_ipv6_range_at(
        &taken_ipv6_ranges(taken_addresses, taken_prefixes),
        u128::from(parsed),
    )
    .is_some()
}

/// `how_many` assignable IPv6 addresses of `block` that nothing holds, lowest first — or
/// `None` when the block has that many nowhere, or when `how_many` is past the per-interface
/// ceiling [`MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`].
///
/// The counterpart of [`ComAmazonawsEc2Amazonec2State::free_subnet_address`] for the IPv6
/// family: the candidates are the window [`assignable_subnet_ipv6_window`] admits, judged
/// against the one definition of held ([`ipv6_address_is_taken`] reads the same ranges). The
/// walk takes one step per address it chooses and one per holding it skips
/// ([`taken_ipv6_ranges`] makes a holding one step however wide it is), so the width of a
/// `/64` cannot turn one call into unbounded work.
pub fn free_subnet_ipv6_addresses(
    block: &str,
    taken_addresses: &[String],
    taken_prefixes: &[String],
    how_many: usize,
) -> Option<Vec<String>> {
    // The ceiling is applied HERE and not only by each caller: `how_many` comes from a
    // request, so the walk itself is where the bound on one request's work and allocation
    // has to hold, whichever handler asks. A count no interface could hold is answered
    // `None` rather than walked.
    if how_many > MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE {
        return None;
    }
    let window = assignable_subnet_ipv6_window(block)?;
    let taken = taken_ipv6_ranges(taken_addresses, taken_prefixes);
    let last = *window.end();
    let mut candidate = *window.start();
    // Grown as each one is chosen, inside the ceiling above.
    let mut chosen: Vec<String> = Vec::new();
    while chosen.len() < how_many && candidate <= last {
        let next = match taken_ipv6_range_at(&taken, candidate) {
            // Something holds this address, and everything up to the end of its range:
            // the next candidate is the address after that range.
            Some((_, held_last)) => held_last.checked_add(1),
            None => {
                chosen.push(Ipv6Addr::from(candidate).to_string());
                candidate.checked_add(1)
            }
        };
        match next {
            Some(next) => candidate = next,
            // The top of the address space: there is nothing above this candidate.
            None => break,
        }
    }
    (chosen.len() == how_many).then_some(chosen)
}

/// Is the IPv6 prefix `prefix` free — overlapping no prefix already delegated and covering
/// no address an interface already holds?
///
/// Two interfaces cannot be delegated overlapping ranges, and a range cannot swallow an
/// address something else holds, so this is the prefix counterpart of
/// [`ipv6_address_is_taken`] and reads the same ranges.
pub fn ipv6_prefix_is_free(
    prefix: &str,
    taken_addresses: &[String],
    taken_prefixes: &[String],
) -> bool {
    let Some((first, last)) = ipv6_cidr_bounds(prefix) else {
        return false;
    };
    let taken = taken_ipv6_ranges(taken_addresses, taken_prefixes);
    // A range is free when nothing holds its first address and no holding STARTS inside it.
    taken_ipv6_range_at(&taken, first).is_none()
        && taken
            .get(taken.partition_point(|(held_first, _)| *held_first <= first))
            .is_none_or(|(held_first, _)| *held_first > last)
}

/// `how_many` free IPv6 prefixes of `block`, lowest first — or `None` when it has that many
/// nowhere, or when `how_many` is past the per-interface ceiling
/// [`MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`].
///
/// Each one is a [`IPV6_PREFIX_DELEGATION_PREFIX_LENGTH`] block aligned inside `block`, and
/// the first such sub-block is skipped: it holds the addresses AWS reserves at the bottom of
/// the subnet ([`SUBNET_RESERVED_IPV6_ADDRESSES`]), which a delegated prefix cannot take. The
/// choice is judged by [`ipv6_prefix_is_free`], and the walk steps over a holding in one step
/// the way the address search does, so one call's work is bounded by what it hands out plus
/// what the account already holds.
pub fn free_subnet_ipv6_prefixes(
    block: &str,
    taken_addresses: &[String],
    taken_prefixes: &[String],
    how_many: usize,
) -> Option<Vec<String>> {
    // The same ceiling, on the same terms as [`free_subnet_ipv6_addresses`]: the bound on
    // what one request can make this walk do holds in the walk, not only in its callers.
    if how_many > MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE {
        return None;
    }
    let (base, prefix_length) = split_ipv6_cidr(block)?;
    if prefix_length >= IPV6_PREFIX_DELEGATION_PREFIX_LENGTH {
        // A block no larger than one delegated prefix has no aligned sub-block to hand out
        // above its reserved addresses.
        return None;
    }
    let size = 1u128.checked_shl(u32::from(128 - prefix_length))?;
    let base = base & !(size - 1);
    let step = 1u128.checked_shl(u32::from(128 - IPV6_PREFIX_DELEGATION_PREFIX_LENGTH))?;
    let last = base.checked_add(size - 1)?;
    let taken = taken_ipv6_ranges(taken_addresses, taken_prefixes);
    // The second aligned sub-block: the first holds the subnet's reserved addresses.
    let mut start = base.checked_add(step)?;
    // Grown as each one is chosen, inside the ceiling above.
    let mut chosen: Vec<String> = Vec::new();
    while chosen.len() < how_many && start < last {
        let end = start.checked_add(step - 1)?;
        let held = taken_ipv6_range_at(&taken, start).map(|(_, held_last)| held_last).or_else(|| {
            taken
                .get(taken.partition_point(|(held_first, _)| *held_first <= start))
                .filter(|(held_first, _)| *held_first <= end)
                .map(|(_, held_last)| *held_last)
        });
        let next = match held {
            // Something holds part of this candidate, and everything up to the end of its
            // range: the next candidate is the first aligned block above that range.
            Some(held_last) => held_last
                .checked_add(1)?
                .checked_add(step - 1)
                .map(|above| above - (above % step)),
            None => {
                chosen.push(format!(
                    "{}/{IPV6_PREFIX_DELEGATION_PREFIX_LENGTH}",
                    Ipv6Addr::from(start)
                ));
                start.checked_add(step)
            }
        };
        match next {
            Some(next) if next > start => start = next,
            // The top of the address space, or a jump that did not advance.
            _ => break,
        }
    }
    (chosen.len() == how_many).then_some(chosen)
}

/// Refuse a request that asked for more IPv6 addresses than the subnet's block has free:
/// HTTP 400 `InsufficientFreeAddressesInSubnet`.
///
/// The IPv6 counterpart of [`crate::scaffolded::Scaffolded::insufficient_free_addresses`],
/// which says the same about the subnet's private IPv4 addresses.
pub fn insufficient_free_ipv6_addresses<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        "InsufficientFreeAddressesInSubnet",
        400,
        "The subnet does not have enough free IPv6 addresses to satisfy the request",
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The IPv6 CIDR block the subnet `subnet_id` holds, or `None` when it holds none (or
    /// the store holds no such subnet).
    ///
    /// "You can only associate a single IPv6 CIDR block with your subnet"
    /// (`AssociateSubnetCidrBlock`), so the one association that still HOLDS its block
    /// ([`cidr_association_holds_its_block`]) is the subnet's IPv6 range — what every
    /// operation that assigns an IPv6 address out of a subnet reads.
    pub fn subnet_ipv6_cidr_block(&self, subnet_id: &str) -> Option<String> {
        self.subnets
            .get(subnet_id)?
            .ipv6_cidr_block_association_set
            .iter()
            .flatten()
            .filter(|association| {
                cidr_association_holds_its_block(
                    association
                        .ipv6_cidr_block_state
                        .as_ref()
                        .and_then(|state| state.state.as_deref()),
                )
            })
            .find_map(|association| association.ipv6_cidr_block.clone())
    }

    /// Every IPv6 address the account's network interfaces hold, in store order.
    ///
    /// An assigned IPv6 address lives on the interface record itself
    /// (`NetworkInterface$Ipv6Addresses`), so the records ARE the in-use pool — there is no
    /// second copy of them to drift from.
    pub fn assigned_ipv6_addresses(&self) -> Vec<String> {
        self.network_interfaces
            .values()
            .flat_map(|interface| interface.ipv6_addresses.iter().flatten())
            .filter_map(|address| address.ipv6_address.clone())
            .collect()
    }

    /// Every IPv6 prefix delegated to one of the account's network interfaces, in store
    /// order — read off `NetworkInterface$Ipv6Prefixes` for the same reason as
    /// [`Self::assigned_ipv6_addresses`].
    pub fn assigned_ipv6_prefixes(&self) -> Vec<String> {
        self.network_interfaces
            .values()
            .flat_map(|interface| interface.ipv6_prefixes.iter().flatten())
            .filter_map(|prefix| prefix.ipv6_prefix.clone())
            .collect()
    }
}

/// The tags a create assigns to the ONE resource it creates, from the request's
/// `TagSpecifications`.
///
/// "You must specify the resource type to tag, otherwise the request will fail"
/// (`TagSpecification`), and the only type an action may name is the resource it creates —
/// so a specification naming anything else is refused rather than silently dropped.
///
/// The tag payload is bounded here the way `CreateTags` bounds its own: at most the
/// documented number of tags per resource, each key and value within its documented
/// length, no key given twice ("Tag keys must be unique per resource", so two entries for
/// one key have no defined winner), and no key in the reserved `aws:` namespace. A count
/// of resources is not a bound on the caller-chosen bytes each one holds, which is why
/// every create charges [`tag_list_bytes`] of the result against the store's byte budget.
pub fn tags_from_specifications<E>(
    specifications: Option<&[TagSpecification]>,
    resource_type: &str,
    operation: &str,
) -> Result<Vec<Tag>, Scaffolded<E>> {
    let mut tags: Vec<Tag> = Vec::new();
    for specification in specifications.unwrap_or_default() {
        match specification.resource_type.as_deref() {
            Some(named) if named == resource_type => {
                tags.extend(specification.tags.clone().unwrap_or_default());
            }
            Some(other) => {
                return Err(invalid_parameter_value(format!(
                    "'{other}' is not a valid resource type for tagging on {operation}"
                )))
            }
            None => {
                return Err(missing_parameter(
                    "Tag specification resource type must have a value",
                ))
            }
        }
    }
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(Scaffolded::tag_limit_exceeded(format!(
            "The request carries {} tags, above the maximum of {max} per resource",
            tags.len(),
            max = MAX_TAGS_PER_RESOURCE,
        )));
    }
    let mut keys_seen: BTreeSet<&str> = BTreeSet::new();
    for tag in &tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err(invalid_parameter_value("A tag key cannot be empty"));
        }
        if key.starts_with(RESERVED_TAG_PREFIX) {
            return Err(invalid_parameter_value(format!(
                "Tag key '{key}' is invalid: the '{prefix}' prefix is reserved",
                prefix = RESERVED_TAG_PREFIX,
            )));
        }
        if !keys_seen.insert(key) {
            return Err(invalid_parameter_value(format!(
                "Tag key '{key}' is given more than once; tag keys must be unique"
            )));
        }
        validate_tag_size(key, tag.value.as_deref().unwrap_or_default())?;
    }
    Ok(tags)
}

/// Refuse a delete the resource's own dependencies forbid: HTTP 400
/// `DependencyViolation`. `message` names the resource and what still depends on it, so
/// the caller knows what to delete first.
pub fn dependency_violation<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(DEPENDENCY_VIOLATION, 400, message)
}

/// Refuse a delete of the default security group of a VPC: HTTP 400 `CannotDelete`. That
/// group exists for as long as its VPC does — "When you delete the VPC, it deletes the
/// default security group" (`DeleteVpc`) — so only `DeleteVpc` removes it.
pub fn cannot_delete<E>(group_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        CANNOT_DELETE,
        400,
        format!(
            "The security group '{group_id}' is the default group of its VPC \
             and cannot be deleted"
        ),
    )
}

/// What one network interface is created with, beyond everything the service itself owns
/// (its id, MAC address, owning account, private DNS names and status), which
/// [`ComAmazonawsEc2Amazonec2State::insert_network_interface`] derives.
#[derive(Debug, Clone, Default)]
pub struct NetworkInterfaceSpec {
    /// The subnet the interface sits in.
    pub subnet_id: String,
    /// That subnet's VPC.
    pub vpc_id: String,
    /// That subnet's Availability Zone.
    pub availability_zone: String,
    /// That zone's id.
    pub availability_zone_id: String,
    /// The Region the request was made in, which the private DNS names are built for.
    pub region: String,
    /// "A description for the network interface"
    /// (`CreateNetworkInterfaceRequest$Description`).
    /// AWS reports an EMPTY description rather than none for an interface created without
    /// one, so a writer that has none leaves this empty rather than deciding what absence
    /// looks like.
    pub description: String,
    /// One of the `NetworkInterfaceType` enum's values,
    /// [`NETWORK_INTERFACE_TYPE_INTERFACE`] unless the request named another.
    pub interface_type: String,
    /// The interface's private IPv4 addresses, PRIMARY FIRST: "Indicates whether this IPv4
    /// address is the primary private IPv4 address of the network interface"
    /// (`NetworkInterfacePrivateIpAddress$Primary`), and only one address can be primary.
    /// Empty for an interface that carries none ([`NETWORK_INTERFACE_TYPE_EFA_ONLY`]).
    pub private_ip_addresses: Vec<String>,
    /// The security groups of the interface, as the describe reports them.
    pub groups: Vec<GroupIdentifier>,
    /// "The network interface attachment" (`NetworkInterface$Attachment`), for an interface
    /// something already holds; `None` for one attached to nothing, which is what a create
    /// stores. The `Status` the store derives follows from it, so a stored interface cannot
    /// claim to be `available` while something holds it.
    pub attachment: Option<NetworkInterfaceAttachment>,
    /// "The alias or Amazon Web Services account ID of the principal or service that created
    /// the network interface" (`NetworkInterface$RequesterId`), for an interface ANOTHER AWS
    /// service created to serve a resource of the account — which is also what makes the
    /// interface one "being managed by Amazon Web Services"
    /// (`NetworkInterface$RequesterManaged`). `None` for an interface the caller created for
    /// itself, which nothing but the caller manages.
    pub requester: Option<String>,
    /// The tags the create asked for.
    pub tags: Vec<Tag>,
}

/// The MAC address of the network interface whose store counter value is `counter`.
///
/// Derived from the counter, never from a clock or a random source, so it is the same
/// across reads and across runs and no two interfaces share one. The first octet is `02`:
/// bit 1 set marks a locally administered address, which is what a virtualised NIC gets.
pub(super) fn mac_address_for(counter: u64) -> String {
    let octets = counter.to_be_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[3], octets[4], octets[5], octets[6], octets[7]
    )
}

// The reserved-address arithmetic a network interface's addresses are judged by is
// [`assignable_subnet_window`] / [`is_assignable_subnet_address`] further up, and the
// free-address search and take are the store's own `free_subnet_address` /
// `mint_private_ip_in_cidr` / `reserve_private_ip`: one computation, one definition of
// "held", so a service-chosen address and a caller-named one cannot be judged differently.

/// A boolean attribute as an EC2 filter value compares against it: the wire rendering of
/// the member, `true` or `false`. A member that is not set carries no value for the filter
/// to match, like any other absent attribute.
///
/// Beside [`matches_every_filter`], which consumes what it answers, so every describe
/// renders a boolean attribute the same way.
pub fn boolean_filter_values(value: Option<bool>) -> Vec<Cow<'static, str>> {
    value
        .map(|set| Cow::Borrowed(if set { "true" } else { "false" }))
        .into_iter()
        .collect()
}

/// A numeric attribute as an EC2 filter value compares against it: its decimal rendering,
/// which is how the wire carries it. The integer companion of
/// [`boolean_filter_values`].
pub fn integer_filter_values(value: Option<i32>) -> Vec<Cow<'static, str>> {
    value
        .map(|number| Cow::Owned(number.to_string()))
        .into_iter()
        .collect()
}

/// Do two tag lists ask for the same tags?
///
/// What a replayed `ClientToken` is compared on, beside the scalar members, by every create
/// of this crate that honors one. A tag set is unordered — "Tag keys must be unique per
/// resource", so a list is a SET of key/value pairs and the order a client sent them in is
/// not part of what it asked for — so both lists are compared as sorted pairs rather than
/// positionally. The members are compared as the `Option`s they are, so a tag that carries
/// no value stays distinguishable from one whose value is the empty string. The modelled
/// `Tag` carries no `PartialEq`, hence the explicit pairs.
pub fn same_tags(stored: &[Tag], asked: &[Tag]) -> bool {
    let pairs = |tags: &[Tag]| -> Vec<(Option<String>, Option<String>)> {
        let mut pairs: Vec<(Option<String>, Option<String>)> = tags
            .iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect();
        pairs.sort();
        pairs
    };
    pairs(stored) == pairs(asked)
}

/// Do two security-group lists name the same groups?
///
/// The group-list companion of [`same_tags`], for a replayed `ClientToken`. A group list is
/// a SET of ids — "the IDs of one or more security groups" — so the order a client sent them
/// in is not part of what it asked for, and the comparison is on the ids, the only part of a
/// `GroupIdentifier` a request chooses.
pub fn same_group_ids(stored: &[GroupIdentifier], asked: &[GroupIdentifier]) -> bool {
    let ids = |groups: &[GroupIdentifier]| -> Vec<Option<String>> {
        let mut ids: Vec<Option<String>> = groups
            .iter()
            .map(|group| group.group_id.clone())
            .collect();
        ids.sort();
        ids
    };
    ids(stored) == ids(asked)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another network interface fits inside the store's ceiling
    /// ([`MAX_NETWORK_INTERFACES`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`network_interface_limit_exceeded`] when it answers `false`.
    pub fn has_network_interface_capacity(&self) -> bool {
        self.network_interfaces.len() < MAX_NETWORK_INTERFACES
    }

    /// Store a network interface, and answer the `NetworkInterfaceId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id, the MAC address built from it, the owning account, the private DNS
    /// name of each address and the `available` status of an unattached interface — so two
    /// writers cannot report the same interface differently.
    ///
    /// The addresses the interface takes leave the subnet: "`AvailableIpAddressCount` - The
    /// number of IPv4 addresses in the subnet that are available"
    /// (`DescribeSubnetsRequest$Filters`), so the subnet's count is decremented here rather
    /// than by each writer, and they are recorded in the one in-use pool
    /// [`Self::allocate_private_ip`] and [`Self::private_ip_in_use`] read, so the instance
    /// path cannot hand out an address an interface holds. Like [`Self::insert_vpc`], the
    /// caller-chosen bytes of `spec.tags` are charged against the store's byte budget here
    /// too.
    ///
    /// [`Self::remove_network_interface`] is the exact reverse: it gives the record's bytes,
    /// its addresses and the subnet's count back on the terms this method took them on, so
    /// a create followed by a delete leaves the store where it started.
    ///
    /// `None` means NOTHING was stored because the store is full — at
    /// [`MAX_NETWORK_INTERFACES`] interfaces or at [`MAX_STORED_RECORDS_PER_STORE`] records
    /// — so a writer that forgets to ask [`Self::has_network_interface_capacity`] cannot
    /// grow the store without bound either.
    pub fn insert_network_interface(&mut self, spec: NetworkInterfaceSpec) -> Option<String> {
        if !self.has_network_interface_capacity()
            || self.network_interfaces.len() >= MAX_STORED_RECORDS_PER_STORE
        {
            return None;
        }
        let network_interface_id = self.next_id(NETWORK_INTERFACE_ID_PREFIX);
        let mac_address = mac_address_for(self.id_counter);
        let primary_address = spec.private_ip_addresses.first().cloned();
        let private_ip_addresses: Vec<NetworkInterfacePrivateIpAddress> = spec
            .private_ip_addresses
            .iter()
            .enumerate()
            .map(|(index, address)| NetworkInterfacePrivateIpAddress {
                // No Elastic IP address is associated until something associates one.
                association: None,
                primary: Some(index == 0),
                private_dns_name: Some(private_dns_name(address, &spec.region)),
                private_ip_address: Some(address.clone()),
            })
            .collect();
        let assigned_addresses = private_ip_addresses.len();
        // The caller-chosen bytes this record will retain, counted from the SPEC before it
        // is taken apart — the one formula the create's own byte-budget question asks and
        // [`Self::remove_network_interface`] gives back.
        let retained_bytes =
            network_interface_spec_bytes(&spec).saturating_add(STORED_RECORD_OVERHEAD_BYTES);
        // "The status of the network interface" (`NetworkInterface$Status`): an interface
        // something holds is `in-use`, one attached to nothing is `available`, and an
        // attachment that has reached `detached` holds nothing. Derived here rather than
        // taken from the writer, so a stored interface cannot claim to be `available` while
        // something holds it — a create, which attaches nothing, always gets `available`.
        let held = spec
            .attachment
            .as_ref()
            .is_some_and(|attachment| !attachment_is_detached(attachment));
        let status = if held {
            NETWORK_INTERFACE_STATUS_IN_USE
        } else {
            NETWORK_INTERFACE_STATUS_AVAILABLE
        };
        let interface = NetworkInterface {
            // No Elastic IP address is associated until something associates one.
            association: None,
            attachment: spec.attachment,
            availability_zone: Some(spec.availability_zone),
            availability_zone_id: Some(spec.availability_zone_id),
            // Set only by a request that carries a connection tracking specification.
            connection_tracking_configuration: None,
            // AWS reports an empty description rather than none for an interface created
            // without one, which is also what a `description` filter compares against.
            description: Some(spec.description),
            groups: Some(spec.groups),
            interface_type: Some(spec.interface_type),
            // No IPv6 address or prefix is assigned until something assigns one.
            ipv6_addresses: Some(Vec::new()),
            ipv6_address: None,
            ipv6_native: Some(false),
            ipv4_prefixes: Some(Vec::new()),
            ipv6_prefixes: Some(Vec::new()),
            mac_address: Some(mac_address),
            network_interface_id: Some(network_interface_id.clone()),
            // An interface is on an Outpost only when it was created in an Outpost subnet.
            outpost_arn: None,
            owner_id: Some(::engine::emulator_account_id().to_string()),
            private_dns_name: primary_address
                .as_deref()
                .map(|address| private_dns_name(address, &spec.region)),
            // A public hostname exists only for an interface with a public address.
            public_dns_name: None,
            public_ip_dns_name_options: None,
            private_ip_address: primary_address,
            private_ip_addresses: Some(private_ip_addresses),
            // "The alias or Amazon Web Services account ID of the principal or service that
            // created the network interface" (`NetworkInterface$RequesterId`): reported for
            // an interface an AWS service created to serve a resource of the account, and
            // `None` for one the caller created for itself. An interface a service created
            // is the one that service manages, so `RequesterManaged` follows from the same
            // member rather than being decided twice.
            requester_id: spec.requester.clone(),
            requester_managed: Some(spec.requester.is_some()),
            // "Indicates whether source/destination checking is enabled": on by default,
            // the value an interface has to be changed away from to do NAT.
            source_dest_check: Some(true),
            status: Some(status.to_string()),
            subnet_id: Some(spec.subnet_id.clone()),
            tag_set: Some(spec.tags),
            vpc_id: Some(spec.vpc_id),
            // Reported only for an IPv6 interface, which this slice does not create.
            deny_all_igw_traffic: None,
            // No service provider manages an interface the caller created for itself,
            // which is the answer every resource of this store gives for its `Operator`
            // ([`unmanaged_operator`]).
            operator: Some(unmanaged_operator()),
            associated_subnets: Some(vec![spec.subnet_id.clone()]),
        };
        // Every caller-chosen byte the record retains — its tags, its description and the
        // addresses it holds — plus [`STORED_RECORD_OVERHEAD_BYTES`], so an interface with
        // no description and no tags is not free against the budget either. The same
        // quantity [`Self::remove_network_interface`] gives back, counted by the one
        // function both read.
        self.add_stored_bytes(retained_bytes);
        // The addresses leave the one pool every address path of this store draws from, so
        // a later `RunInstances` can neither be handed an address an interface holds nor
        // accept one a caller names that an interface already holds
        // ([`Self::allocate_private_ip`] and [`Self::private_ip_in_use`] both read it).
        for address in &spec.private_ip_addresses {
            self.assigned_private_ips.insert(address.clone());
        }
        if let Some(subnet) = self.subnets.get_mut(&spec.subnet_id) {
            let taken = i32::try_from(assigned_addresses).unwrap_or(i32::MAX);
            subnet.available_ip_address_count = Some(
                subnet
                    .available_ip_address_count
                    .unwrap_or_default()
                    .saturating_sub(taken)
                    .max(0),
            );
        }
        // The index of "which interfaces does this instance hold", written here beside the
        // record itself so the two cannot drift: an interface something already holds when
        // it is stored is indexed under the instance that holds it, and one attached to
        // nothing is indexed nowhere.
        if let Some(instance_id) = interface
            .attachment
            .as_ref()
            .filter(|attachment| !attachment_is_detached(attachment))
            .and_then(|attachment| attachment.instance_id.clone())
        {
            self.network_interfaces_by_instance
                .entry(instance_id)
                .or_default()
                .insert(network_interface_id.clone());
        }
        self.network_interfaces
            .insert(network_interface_id.clone(), interface);
        Some(network_interface_id)
    }
}

/// Refuse a subnet CIDR block that overlaps one already in the VPC: HTTP 400
/// `InvalidSubnet.Conflict`.
///
/// The message names the block the REQUEST asked for and nothing else: the id of the subnet
/// that already holds an overlapping block is state of the account the refusal does not
/// report, so a client reading the wording sees the value it sent and no identifier it did
/// not ask about.
pub fn invalid_subnet_conflict<E>(cidr_block: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SUBNET_CONFLICT,
        400,
        format!("The CIDR '{cidr_block}' conflicts with another subnet"),
    )
}

/// Refuse a subnet CIDR block the subnet's range rules do not admit: HTTP 400
/// `InvalidSubnet.Range`, whichever rule it fails — a block of a size the service does not
/// permit (the published IPv4 subnet block size
/// [`MIN_SUBNET_IPV4_PREFIX`]..=[`MAX_SUBNET_IPV4_PREFIX`], or the one size an IPv6 subnet
/// block may have, [`SUBNET_IPV6_PREFIX_LENGTH`]) and one outside the block of the VPC the
/// subnet would sit in. ONE wording site for that code, the way [`invalid_vpc_range`] is the
/// one site for a VPC's.
///
/// ONE sentence too, whichever rule the block fails: the service states that the block is not
/// valid and names the block from the request. WHICH rule it missed is what the error code
/// stands for, and neither the published size nor the VPC whose block it is outside of is
/// part of the message ([`invalid_vpc_range`], the same class one resource up, words it the
/// same way).
pub fn invalid_subnet_range<E>(cidr_block: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SUBNET_RANGE,
        400,
        format!("The CIDR '{cidr_block}' is invalid."),
    )
}

/// EC2's error code for a request that would take a resource past the number of CIDR
/// blocks it may have associated with it — "CidrLimitExceeded: You've reached the limit on
/// the number of CIDR blocks you can associate with the resource" (EC2 API "Error codes").
/// For a subnet that limit is one IPv6 block: "You can only associate a single IPv6 CIDR
/// block with your subnet" (`AssociateSubnetCidrBlock`).
pub const CIDR_LIMIT_EXCEEDED: &str = "CidrLimitExceeded";

/// Refuse an association past the number of CIDR blocks the resource may hold: HTTP 400
/// [`CIDR_LIMIT_EXCEEDED`].
pub fn cidr_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(CIDR_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a create that would take the store past [`MAX_SUBNETS`]: HTTP 400
/// `SubnetLimitExceeded`.
pub fn subnet_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        SUBNET_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of subnets has been reached ({MAX_SUBNETS})"),
    )
}

/// EC2's error code for a second security group with a name the VPC already holds: "You
/// can't have two security groups for the same VPC with the same name"
/// (`CreateSecurityGroup`).
pub const INVALID_GROUP_DUPLICATE: &str = "InvalidGroup.Duplicate";

/// Whether `id` is a well-formed security group id: `sg-` followed by 8 or 17 lowercase
/// hexadecimal digits — the short and the long id form, the two
/// [`prefixed_id_is_well_formed`] reads.
pub fn security_group_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(SECURITY_GROUP_ID_PREFIX) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `text` is within the alphabet a security group name and description are
/// documented to accept (see [`SECURITY_GROUP_PUNCTUATION`]).
pub fn is_security_group_text(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || SECURITY_GROUP_PUNCTUATION.contains(c))
}

/// Refuse a second security group with a name the VPC already holds: HTTP 400
/// `InvalidGroup.Duplicate`.
///
/// The group is named by the two attributes that IDENTIFY it — the name the request sent and
/// the VPC it would have been created in — because a group's name is unique only within its
/// VPC ("You can't have two security groups for the same VPC with the same name" —
/// `CreateSecurityGroup`), so the name alone does not tell a caller which group it collided
/// with. `vpc_id` is the VPC the create resolved: the one the request named, or the Region's
/// default when it named none ("Required for a nondefault VPC" —
/// `CreateSecurityGroupRequest$VpcId`), which is the only way the caller learns which VPC the
/// fall-back chose. Never the colliding group's own id: the caller has not seen it.
pub fn invalid_group_duplicate<E>(group_name: &str, vpc_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_GROUP_DUPLICATE,
        400,
        format!("The security group '{group_name}' already exists for VPC '{vpc_id}'"),
    )
}

/// Refuse a create that named no VPC in an account that has no default VPC to fall back
/// on: HTTP 400 `VPCIdNotSpecified` — "The ID of the VPC. Required for a nondefault VPC"
/// (`CreateSecurityGroupRequest$VpcId`).
pub fn vpc_id_not_specified<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_ID_NOT_SPECIFIED,
        400,
        "No default VPC for this user, so the request must name a VpcId",
    )
}

/// EC2's error code for a request that left out a VPC id the account has no default VPC to
/// supply (EC2 API error codes, "VPCIdNotSpecified").
pub const VPC_ID_NOT_SPECIFIED: &str = "VPCIdNotSpecified";

/// Refuse a create that would take the store past [`MAX_SECURITY_GROUPS`]: HTTP 400
/// `SecurityGroupLimitExceeded`.
pub fn security_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        SECURITY_GROUP_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of security groups has been reached ({MAX_SECURITY_GROUPS})"),
    )
}

// ---------------------------------------------------------------------------
// Security group VPC associations
// ---------------------------------------------------------------------------
//
// "Associates a security group with another VPC in the same Region. This enables you to use
// the same security group with network interfaces and instances in the specified VPC"
// (`AssociateSecurityGroupVpc`). The association is therefore the (group, VPC) PAIR and
// nothing else: it carries no id of its own and no caller-chosen bytes, which is why the
// store below is keyed by the pair and why an association costs the byte budget nothing.

/// An association that is being established: the `associating` member of
/// `SecurityGroupVpcAssociationState`
/// (`smithy_query(query="shape",shape="SecurityGroupVpcAssociationState")`).
pub const SECURITY_GROUP_VPC_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association that is established: that same enum's `associated` member.
pub const SECURITY_GROUP_VPC_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// An association that is being taken down: that same enum's `disassociating` member.
pub const SECURITY_GROUP_VPC_ASSOCIATION_STATE_DISASSOCIATING: &str = "disassociating";

/// An association that has been taken down: that same enum's `disassociated` member — the
/// state `DisassociateSecurityGroupVpc`'s clock-owed move ends in, and the one in which the
/// pair holds the group in no VPC and may be associated again.
pub const SECURITY_GROUP_VPC_ASSOCIATION_STATE_DISASSOCIATED: &str = "disassociated";

/// The largest number of security group VPC associations this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_SECURITY_GROUPS`], not an AWS quota: each
/// association is a record in this process's memory and nothing but a delete of one of its
/// two sides reclaims it, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_security_group_vpc_association_capacity`] is the
/// predicate a writer asks first; an associate past it is refused with
/// [`RESOURCE_LIMIT_EXCEEDED`], the code this service answers for a ceiling it publishes no
/// resource-specific code for.
pub const MAX_SECURITY_GROUP_VPC_ASSOCIATIONS: usize = 2_500;

/// One stored security group VPC association: the instants the clock owes it its
/// `associating` -> `associated` and `disassociating` -> `disassociated` moves.
///
/// The pair is the key of
/// [`ComAmazonawsEc2Amazonec2State::security_group_vpc_associations`] and is not repeated on
/// the record. `State` is a fact of the record's age, so [`Self::state_at`] derives it on
/// read rather than letting two writers store two different words for one association, and
/// the owning accounts the modelled shape reports are derived from the group and the VPC
/// themselves ([`ComAmazonawsEc2Amazonec2State::security_group_vpc_association_view`]).
#[derive(Debug, Clone)]
pub struct SecurityGroupVpcAssociationRecord {
    /// The instant from which this association's `associating` -> `associated` move is due.
    /// Monotonic ([`Instant`]) for the reason every other owed move of this store is: it is
    /// only ever compared against the clock, never reported, so a wall-clock adjustment
    /// cannot move an association back to `associating`.
    pub associated_at: Instant,
    /// The instant from which this association's `disassociating` -> `disassociated` move is
    /// due, and `None` for as long as nothing has asked for the association to be taken
    /// down. Monotonic on the same terms as [`Self::associated_at`]; set once, by the
    /// disassociate, and cleared only by an associate that establishes the pair afresh.
    pub disassociated_at: Option<Instant>,
}

impl SecurityGroupVpcAssociationRecord {
    /// The association's `SecurityGroupVpcAssociationState` at `now`.
    ///
    /// A disassociation takes over the record's state: once one is owed, the association is
    /// `disassociating` until it falls due and `disassociated` after it, whatever the
    /// association's own move had reached.
    pub fn state_at(&self, now: Instant) -> &'static str {
        match self.disassociated_at {
            Some(disassociated_at) if now < disassociated_at => {
                SECURITY_GROUP_VPC_ASSOCIATION_STATE_DISASSOCIATING
            }
            Some(_) => SECURITY_GROUP_VPC_ASSOCIATION_STATE_DISASSOCIATED,
            None if now < self.associated_at => SECURITY_GROUP_VPC_ASSOCIATION_STATE_ASSOCIATING,
            None => SECURITY_GROUP_VPC_ASSOCIATION_STATE_ASSOCIATED,
        }
    }

    /// Whether this association still joins its pair at `now`: everything up to and
    /// including `disassociating` does, and only a completed disassociation does not.
    ///
    /// The ONE definition of "the group is associated with that VPC", so the operation that
    /// establishes an association and the one that takes it down cannot disagree about
    /// whether the account holds one.
    pub fn joins_its_pair_at(&self, now: Instant) -> bool {
        self.state_at(now) != SECURITY_GROUP_VPC_ASSOCIATION_STATE_DISASSOCIATED
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another security group VPC association fits inside the store's record
    /// ceiling ([`MAX_SECURITY_GROUP_VPC_ASSOCIATIONS`]).
    pub fn has_security_group_vpc_association_capacity(&self) -> bool {
        self.security_group_vpc_associations.len() < MAX_SECURITY_GROUP_VPC_ASSOCIATIONS
    }

    /// The association of `group_id` with `vpc_id` as of `now`, as the modelled shape every
    /// reader of it reports.
    ///
    /// "The Amazon Web Services account ID of the owner of the security group" /
    /// "\u2026 of the owner of the VPC" (`SecurityGroupVpcAssociation$GroupOwnerId`,
    /// `$VpcOwnerId`) are facts of the group and of the VPC, so they are read from those
    /// records rather than copied onto the association: a report of one association can
    /// never disagree with a report of either side of it. `StateReason` is "the
    /// association's state reason" (`$StateReason`), which an association that has not
    /// failed has none of.
    pub fn security_group_vpc_association_view(
        &self,
        group_id: &str,
        vpc_id: &str,
        record: &SecurityGroupVpcAssociationRecord,
        now: Instant,
    ) -> SecurityGroupVpcAssociation {
        SecurityGroupVpcAssociation {
            group_id: Some(group_id.to_string()),
            vpc_id: Some(vpc_id.to_string()),
            group_owner_id: self
                .security_groups
                .get(group_id)
                .and_then(|group| group.owner_id.clone()),
            vpc_owner_id: self.vpcs.get(vpc_id).and_then(|vpc| vpc.owner_id.clone()),
            state: Some(record.state_at(now).to_string()),
            state_reason: None,
        }
    }

    /// Associate `group_id` with `vpc_id` from `now`, and answer the association as of that
    /// instant.
    ///
    /// The association is written in the non-terminal `associating` state with the clock
    /// owing it `associated` one [`TRANSITION_DELAY`] later, the same span every other
    /// clock-owed move of this crate takes, so a caller reading straight after the write
    /// still sees `associating`.
    pub fn insert_security_group_vpc_association(
        &mut self,
        group_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> SecurityGroupVpcAssociation {
        let record = SecurityGroupVpcAssociationRecord {
            associated_at: now + TRANSITION_DELAY,
            // An associate ESTABLISHES the pair, so whatever a disassociation of an earlier
            // association of the same pair left owed is no longer owed: the record that
            // replaces it starts its own `associating` -> `associated` move with nothing
            // pending against it.
            disassociated_at: None,
        };
        let view = self.security_group_vpc_association_view(group_id, vpc_id, &record, now);
        self.security_group_vpc_associations
            .insert((group_id.to_string(), vpc_id.to_string()), record);
        view
    }

    /// Whether the account holds an association of `group_id` with `vpc_id` at `now`.
    ///
    /// Asked by both writers of the store: an associate has nothing to establish for a pair
    /// this answers `true` for, and a disassociate has nothing to take down for a pair it
    /// answers `false` for. [`SecurityGroupVpcAssociationRecord::joins_its_pair_at`] is where
    /// "holds" is defined, so neither can read it differently.
    pub fn holds_security_group_vpc_association(
        &self,
        group_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> bool {
        self.security_group_vpc_associations
            .get(&(group_id.to_string(), vpc_id.to_string()))
            .is_some_and(|record| record.joins_its_pair_at(now))
    }

    /// Start the disassociation of `group_id` from `vpc_id` at `now`, and answer the
    /// association as of that instant, or `None` when the account holds no such association.
    ///
    /// "Note that the disassociation is asynchronous and you can check the status of the
    /// request with DescribeSecurityGroupVpcAssociations"
    /// (`DisassociateSecurityGroupVpc`): the association is left in the non-terminal
    /// `disassociating` state with the clock owing it `disassociated` one
    /// [`TRANSITION_DELAY`] later — the same span every other clock-owed move of this crate
    /// takes — so a caller reading straight after the write still sees `disassociating`, and
    /// no second call is needed to finish it.
    pub fn disassociate_security_group_vpc_association(
        &mut self,
        group_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> Option<SecurityGroupVpcAssociation> {
        let key = (group_id.to_string(), vpc_id.to_string());
        let record = self.security_group_vpc_associations.get_mut(&key)?;
        if !record.joins_its_pair_at(now) {
            return None;
        }
        record.disassociated_at = Some(now + TRANSITION_DELAY);
        let record = record.clone();
        Some(self.security_group_vpc_association_view(group_id, vpc_id, &record, now))
    }

    /// Every security group VPC association the account holds as of `now`, in (`GroupId`,
    /// `VpcId`) order \u2014 the order the store itself is kept in, which is the order
    /// `DescribeSecurityGroupVpcAssociations` pages over.
    pub fn security_group_vpc_associations_at(
        &self,
        now: Instant,
    ) -> Vec<SecurityGroupVpcAssociation> {
        self.security_group_vpc_associations
            .iter()
            .map(|((group_id, vpc_id), record)| {
                self.security_group_vpc_association_view(group_id, vpc_id, record, now)
            })
            .collect()
    }
}

/// The id of the security group a request names, whether it named it by `GroupId` or by
/// `GroupName`.
///
/// The ONE place the rule every operation that accepts both members follows is written:
/// "you can specify either the security group name or the security group ID"
/// (`DeleteSecurityGroupRequest$GroupName`), and a NAME only names a group of the DEFAULT
/// VPC — "for security groups in a nondefault VPC, you must specify the security group ID"
/// — because a name is unique per VPC and not per account. Both members together have to
/// name the SAME group: honouring the id and ignoring a name that disagrees would act on a
/// group the request did not ask for.
///
/// Whether the account still HOLDS a group named by id ALONE is the caller's question to
/// ask, so such an id is returned unchecked: an operation that needs the group refuses an
/// unknown one with [`invalid_group_not_found`] itself. An id given BESIDE a name is looked
/// up here, because the two can only be checked against each other through the stored
/// group, and an id no group answers to is "no such group" rather than a complaint about
/// the parameter pair.
pub fn resolve_security_group_id<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    region: &str,
    group_id: Option<&str>,
    group_name: Option<&str>,
) -> Result<String, Scaffolded<E>> {
    match (group_id, group_name) {
        (None, None) => Err(missing_parameter(
            "The request must contain either the parameter GroupId or GroupName",
        )),
        (Some(group_id), None) => Ok(group_id.to_string()),
        (None, Some(group_name)) => {
            // The Region's ambient default network is the account's whatever it has
            // created, so it is materialised before a name is resolved in that VPC.
            store.ensure_default_network(region);
            let default_vpc = store
                .default_vpc_id(region)
                .ok_or_else(|| invalid_group_not_found(group_name))?;
            store
                .security_group_id_for_name(&default_vpc, group_name)
                .ok_or_else(|| invalid_group_not_found(group_name))
        }
        (Some(group_id), Some(group_name)) => {
            let Some(group) = store.security_groups.get(group_id) else {
                // An id this account holds no group for is "no such group", whatever name
                // the request put beside it: a caller that named a group that is gone has
                // to read that, not a complaint about its parameters.
                return Err(invalid_group_not_found(group_id));
            };
            if !group
                .group_name
                .as_deref()
                .is_some_and(|held| held.eq_ignore_ascii_case(group_name))
            {
                return Err(invalid_parameter_combination(format!(
                    "The security group '{group_id}' is not named '{group_name}'"
                )));
            }
            Ok(group_id.to_string())
        }
    }
}

/// EC2's error code for a security group rule a request named that the group does not
/// hold: "InvalidPermission.NotFound — The specified permission does not exist" (EC2 API
/// error codes).
pub const INVALID_PERMISSION_NOT_FOUND: &str = "InvalidPermission.NotFound";

/// Refuse a revoke whose rule the group does not hold: HTTP 400
/// [`INVALID_PERMISSION_NOT_FOUND`], and nothing is revoked.
///
/// "For a non-default VPC, if the values you specify do not match the existing rule's
/// values, an `InvalidPermission.NotFound` client error is returned, and no rules are
/// revoked" (`RevokeSecurityGroupIngress`). The message names no rule of the request: the
/// refusal is about the group's rule SET, and the documented wording for this code is "the
/// specified rule does not exist in this security group".
pub fn invalid_permission_not_found<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PERMISSION_NOT_FOUND,
        400,
        "The specified rule does not exist in this security group.",
    )
}

/// EC2's error code for a value that is not a security group rule id at all (EC2 API error
/// codes, "InvalidSecurityGroupRuleId.Malformed").
pub const INVALID_SECURITY_GROUP_RULE_ID_MALFORMED: &str = "InvalidSecurityGroupRuleId.Malformed";

// ---------------------------------------------------------------------------
// Security group rules
// ---------------------------------------------------------------------------

/// The `ResourceType` an `AuthorizeSecurityGroup{Ingress,Egress}` tag specification may
/// name: the rule it creates (`ResourceType` enum member `security-group-rule`), which is
/// what "the tags applied to the security group rule" (`$TagSpecifications`) are applied to.
pub const RESOURCE_TYPE_SECURITY_GROUP_RULE: &str = "security-group-rule";

/// EC2's error code for a rule the group already holds (EC2 API error codes,
/// "InvalidPermission.Duplicate": "the specified inbound or outbound rule already exists
/// for that security group").
pub const INVALID_PERMISSION_DUPLICATE: &str = "InvalidPermission.Duplicate";

/// EC2's error code for a rule whose own members do not hold together (EC2 API error codes,
/// "InvalidPermission.Malformed": "the specified rule is not valid").
pub const INVALID_PERMISSION_MALFORMED: &str = "InvalidPermission.Malformed";

/// The `IpProtocol` of TCP, one of the protocols that "must specify a port range"
/// (`IpPermission$IpProtocol`).
pub const IP_PROTOCOL_TCP: &str = "tcp";

/// The `IpProtocol` of UDP (see [`IP_PROTOCOL_TCP`]).
pub const IP_PROTOCOL_UDP: &str = "udp";

/// The `IpProtocol` of ICMP (see [`IP_PROTOCOL_TCP`]).
pub const IP_PROTOCOL_ICMP: &str = "icmp";

/// The `IpProtocol` of ICMPv6, for which "the port range is optional; if you omit the port
/// range, traffic for all types and codes is allowed" (`IpPermission$IpProtocol`).
pub const IP_PROTOCOL_ICMPV6: &str = "icmpv6";

/// The protocols a rule MUST carry a port range for: "For `tcp`, `udp`, and `icmp`, you
/// must specify a port range" (`IpPermission$IpProtocol`). For ICMP that range is the type
/// and the code rather than ports, which is why the bounds below differ by protocol.
pub const PORT_RANGE_PROTOCOLS: [&str; 3] = [IP_PROTOCOL_TCP, IP_PROTOCOL_UDP, IP_PROTOCOL_ICMP];

/// The largest TCP or UDP port number.
pub const MAX_PORT: i32 = 65_535;

/// The largest ICMP type or code number: ICMP carries both in one byte (RFC 792), and the
/// model admits `-1` beside them for "all ICMP types"/"all ICMP codes"
/// (`IpPermission$FromPort` and `$ToPort`).
pub const MAX_ICMP_TYPE: i32 = 255;

/// The value that stands for "all ICMP types" in `FromPort` and "all ICMP codes" in
/// `ToPort` (`IpPermission$FromPort` and `$ToPort`), and for a whole port range on a
/// protocol that has none.
pub const ALL_PORTS: i32 = -1;

/// Whether `protocol` is one the model requires a port range for ([`PORT_RANGE_PROTOCOLS`]).
pub fn protocol_requires_port_range(protocol: &str) -> bool {
    PORT_RANGE_PROTOCOLS
        .iter()
        .any(|named| protocol.eq_ignore_ascii_case(named))
}

/// Whether `protocol` names ICMP or ICMPv6, whose `FromPort`/`ToPort` are an ICMP type and
/// code rather than ports (`IpPermission$FromPort`).
pub fn protocol_is_icmp(protocol: &str) -> bool {
    protocol.eq_ignore_ascii_case(IP_PROTOCOL_ICMP) || protocol.eq_ignore_ascii_case(IP_PROTOCOL_ICMPV6)
}

/// `protocol` in the one form a stored rule carries it in.
///
/// The model spells every protocol name in lower case (`tcp`, `udp`, `icmp`, `icmpv6` —
/// `IpPermission$IpProtocol`), so a name is stored folded to it the way a CIDR is stored
/// canonicalised: one rule then has one stored form however the request spelled it, and a
/// duplicate is recognised as one. A protocol NUMBER has only one spelling and is kept as
/// it was written.
pub fn canonical_ip_protocol(protocol: &str) -> String {
    for named in [
        IP_PROTOCOL_TCP,
        IP_PROTOCOL_UDP,
        IP_PROTOCOL_ICMP,
        IP_PROTOCOL_ICMPV6,
    ] {
        if protocol.eq_ignore_ascii_case(named) {
            return named.to_string();
        }
    }
    protocol.to_string()
}

/// A requested IPv4 CIDR range of a security group rule in the canonical form AWS stores
/// it in, with NO rule on the prefix length.
///
/// "Amazon Web Services canonicalizes IPv4 and IPv6 CIDRs. For example, if you specify
/// 100.68.0.18/18 for the CIDR block, Amazon Web Services canonicalizes the CIDR block to
/// 100.68.0.0/18. Any subsequent DescribeSecurityGroups and DescribeSecurityGroupRules
/// calls will return the canonicalized form of the CIDR block"
/// (`AuthorizeSecurityGroupIngressRequest$CidrIp`) — so the host bits are cleared rather
/// than the value being refused. Deliberately no block-size bound, unlike a VPC's or a
/// subnet's block: a rule may name a single address (`/32`) or every address (`/0`), and
/// the model states no size for one.
pub fn canonical_security_group_cidr(value: &str) -> Result<String, CidrProblem> {
    canonical_ipv4_cidr(value)
}

/// Refuse a security group rule whose address range is not a CIDR block: HTTP 400
/// [`INVALID_PARAMETER_VALUE`], `CIDR block <value> is malformed`.
///
/// The block is named as the REQUEST wrote it — the value the caller has to fix — and the
/// sentence is the one AWS answers a rule's unreadable range with, whichever address family
/// the range named: an `IpRange$CidrIp` and an `Ipv6Range$CidrIpv6` are the same member of
/// the same rule pointing at the two address spaces, so one unreadable block is one refusal.
///
/// The echo is the bounded one [`crate::ops::delete_coip_cidr::echoed_cidr`] builds — the
/// ceiling this crate already holds a refused CIDR block's echo to, far above any block of
/// either family — so a caller-sized value cannot grow the refusal it earned.
pub fn malformed_security_group_rule_cidr<E>(cidr: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "CIDR block {echoed} is malformed",
        echoed = crate::ops::delete_coip_cidr::echoed_cidr(cidr),
    ))
}

/// Refuse a rule the group would hold twice: HTTP 400 `InvalidPermission.Duplicate`.
///
/// The message names the rule the way the REQUEST described it —
/// [`SecurityGroupRuleAtom::descriptor`], its peer, protocol, port range and action — so a
/// caller is told which of the rules it asked for is the one that already exists, whether
/// that rule is one the group already held or one the same request described twice. A
/// request that describes a rule twice earns the same refusal for the same reason: the rule
/// would be added twice.
pub fn invalid_permission_duplicate<E>(atom: &SecurityGroupRuleAtom) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PERMISSION_DUPLICATE,
        400,
        format!(
            "the specified rule \"{descriptor}\" already exists",
            descriptor = atom.descriptor(),
        ),
    )
}

/// Refuse a rule whose members do not hold together: HTTP 400
/// `InvalidPermission.Malformed`. `message` says which member is at odds with which, so the
/// caller can fix the request from the text alone.
pub fn invalid_permission_malformed<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_PERMISSION_MALFORMED, 400, message)
}

// ---------------------------------------------------------------------------
// Reading one AUTHORIZED permission
// ---------------------------------------------------------------------------
//
// What `AuthorizeSecurityGroupIngress` and `AuthorizeSecurityGroupEgress` both do before
// they write: read every `IpPermission` of the request into the form its rules will be
// STORED in, and refuse the request as a whole if any one of them describes a rule it may
// not have. An inbound rule and an outbound rule are the same rule shape pointing the
// other way — "you must specify exactly one of the following sources/destinations: an IPv4
// or IPv6 address range, a prefix list, or a security group", with the same protocol, port
// range and description rules on both — so the reading lives HERE, once, and neither
// direction can validate or canonicalise a permission differently from the other.

/// The largest IANA protocol number a rule may name: protocol numbers are a single byte
/// (`IpPermission$IpProtocol`, "or number (see Protocol Numbers)").
pub const MAX_PROTOCOL_NUMBER: i32 = 255;

/// The statuses a referenced group's `PeeringStatus` may name: "the status of a VPC peering
/// connection, if applicable" (`UserIdGroupPair$PeeringStatus`), which is the model's
/// `VpcPeeringConnectionStateReasonCode` vocabulary — named through the generated constant,
/// so this validation cannot drift from the model.
pub const PEERING_STATUSES: &[&str] =
    crate::ops::types::VPC_PEERING_CONNECTION_STATE_REASON_CODE_VALUES;

/// The protocol `permission` names, refused when it names none or names one no rule may
/// carry. `member` is the REQUEST PATH the permission arrived on — `ipPermissions.<n>` for an
/// authorize, `securityGroupRules.<n>.securityGroupRule` for a modify — so a refusal names
/// the member of the caller's own request it has to fix rather than a member of another
/// operation's.
///
/// "The IP protocol name (`tcp`, `udp`, `icmp`, `icmpv6`) or number (see Protocol Numbers).
/// Use `-1` to specify all protocols" (`IpPermission$IpProtocol`) — so a protocol is one of
/// those four names, a number a single byte holds, or `-1`.
pub fn validated_ip_protocol<'a, E>(
    permission: &'a IpPermission,
    member: &str,
) -> Result<&'a str, Scaffolded<E>> {
    let protocol = permission
        .ip_protocol
        .as_deref()
        .filter(|protocol| !protocol.is_empty())
        .ok_or_else(|| {
            missing_parameter(format!(
                "The request must contain the parameter {member}.ipProtocol"
            ))
        })?;
    if !ip_protocol_is_known(protocol) {
        return Err(invalid_parameter_value(format!(
            "Invalid IP protocol '{protocol}' for {member}: use a protocol \
             name ({IP_PROTOCOL_TCP}, {IP_PROTOCOL_UDP}, {IP_PROTOCOL_ICMP}, \
             {IP_PROTOCOL_ICMPV6}), a protocol number between 0 and {MAX_PROTOCOL_NUMBER}, \
             or {ALL_IP_PROTOCOLS} for all protocols"
        )));
    }
    Ok(protocol)
}

/// Whether `protocol` is a protocol a rule may name: one of the four names the model
/// lists, a protocol number, or `-1` for every protocol (`IpPermission$IpProtocol`).
pub fn ip_protocol_is_known(protocol: &str) -> bool {
    if protocol == ALL_IP_PROTOCOLS {
        return true;
    }
    if [
        IP_PROTOCOL_TCP,
        IP_PROTOCOL_UDP,
        IP_PROTOCOL_ICMP,
        IP_PROTOCOL_ICMPV6,
    ]
    .iter()
    .any(|named| protocol.eq_ignore_ascii_case(named))
    {
        return true;
    }
    protocol
        .parse::<i32>()
        .is_ok_and(|number| (0..=MAX_PROTOCOL_NUMBER).contains(&number))
}

/// Hold one permission's port range to what the model says the protocol admits.
///
/// "For `tcp`, `udp`, and `icmp`, you must specify a port range. For `icmpv6`, the port
/// range is optional", and "specifying `-1` or a protocol number other than `tcp`, `udp`,
/// `icmp`, or `icmpv6` allows traffic on all ports, regardless of any port range you
/// specify" (`IpPermission$IpProtocol`) — so a port range is required only for the first
/// three, and a range that IS given has to be a range: for ICMP a type and a code, where
/// "if the start port is -1 (all ICMP types), then the end port must be -1 (all ICMP
/// codes)" (`IpPermission$ToPort`), and for everything else a pair of port numbers in
/// order.
pub fn validate_ip_permission_port_range<E>(
    protocol: &str,
    member: &str,
    from_port: Option<i32>,
    to_port: Option<i32>,
) -> Result<(), Scaffolded<E>> {
    if protocol_requires_port_range(protocol) && (from_port.is_none() || to_port.is_none()) {
        return Err(invalid_parameter_value(format!(
            "{member} must specify both fromPort and toPort for protocol \
             '{protocol}'"
        )));
    }
    let (Some(from_port), Some(to_port)) = (from_port, to_port) else {
        // One end without the other is a half-described range on a protocol that needs
        // none; neither end is a range at all, which is the shape of an all-ports rule.
        return match (from_port, to_port) {
            (None, None) => Ok(()),
            _ => Err(invalid_permission_malformed(format!(
                "{member} carries one end of a port range: fromPort and \
                 toPort are given together or not at all"
            ))),
        };
    };
    let highest = if protocol_is_icmp(protocol) {
        MAX_ICMP_TYPE
    } else {
        MAX_PORT
    };
    for (port_member, value) in [("fromPort", from_port), ("toPort", to_port)] {
        if value != ALL_PORTS && !(0..=highest).contains(&value) {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{value}' for {member}.{port_member}: it must be \
                 between 0 and {highest}, or {ALL_PORTS} for all values"
            )));
        }
    }
    if from_port == ALL_PORTS && to_port != ALL_PORTS {
        return Err(invalid_permission_malformed(format!(
            "{member} starts at {ALL_PORTS} (all values), so toPort must be \
             {ALL_PORTS} too"
        )));
    }
    if from_port != ALL_PORTS
        && to_port != ALL_PORTS
        && from_port > to_port
        // For ICMP the two members are a TYPE and a CODE rather than the ends of a range
        // ("this is the ICMP type", "this is the ICMP code" — `IpPermission$FromPort` and
        // `$ToPort`), so there is no order between them to hold.
        && !protocol_is_icmp(protocol)
    {
        return Err(invalid_permission_malformed(format!(
            "{member} ends at {to_port}, before it starts at {from_port}"
        )));
    }
    Ok(())
}

/// Hold a rule's description to the bounds the model publishes for one.
///
/// "Constraints: Up to 255 characters in length. Allowed characters are a-z, A-Z, 0-9,
/// spaces, and ._-:/()#,@[]+=&;{}!$*" (`IpRange$Description`, and the same sentence on
/// `Ipv6Range`, `PrefixListId` and `UserIdGroupPair`) — the same bound and alphabet a
/// group's own name and description carry, so one reader serves both.
pub fn validate_security_group_rule_description<E>(
    description: Option<&String>,
) -> Result<(), Scaffolded<E>> {
    let Some(description) = description else {
        return Ok(());
    };
    if description.chars().count() > MAX_SECURITY_GROUP_TEXT_CHARS {
        return Err(invalid_parameter_value(format!(
            "Value ({description}) for parameter description is invalid. \
             A security group rule description accepts at most \
             {MAX_SECURITY_GROUP_TEXT_CHARS} characters"
        )));
    }
    if !is_security_group_text(description) {
        return Err(invalid_parameter_value(format!(
            "Value ({description}) for parameter description is invalid. \
             Valid characters: a-z, A-Z, 0-9, spaces, and {SECURITY_GROUP_PUNCTUATION}"
        )));
    }
    Ok(())
}

/// `permission` in the form the rules it grants are STORED in, every member of it validated
/// as the member it came from documents.
///
/// `is_egress` is the DIRECTION the permission is being authorized in, which is what the
/// one refusal built here words: a rule's one grant is "exactly one of the following
/// sources" inbound (`AuthorizeSecurityGroupIngress`) and "exactly one of the following
/// destinations" outbound (`AuthorizeSecurityGroupEgress`), so the caller is told to name
/// the one its own operation documents.
///
/// A CIDR range is canonicalised here — "Amazon Web Services canonicalizes IPv4 and IPv6
/// CIDRs … any subsequent DescribeSecurityGroups and DescribeSecurityGroupRules calls will
/// return the canonicalized form" — so the rule is stored and reported in the form AWS
/// stores it in, and a second rule for the same block is the duplicate AWS calls it. A
/// referenced group named only by NAME is resolved to its id inside the authorizing group's
/// VPC, because "names are case-insensitive and must be unique within the VPC" and so name
/// no group outside it, and because a rule references a group by id.
pub fn prepared_ip_permission<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    vpc_id: &str,
    permission: &IpPermission,
    protocol: &str,
    member: &str,
    is_egress: bool,
) -> Result<IpPermission, Scaffolded<E>> {
    let mut ip_ranges: Vec<IpRange> = Vec::new();
    for range in permission.ip_ranges.iter().flatten() {
        validate_security_group_rule_description(range.description.as_ref())?;
        let cidr = range.cidr_ip.as_deref().unwrap_or_default();
        let canonical = canonical_security_group_cidr(cidr)
            .map_err(|_| malformed_security_group_rule_cidr(cidr))?;
        ip_ranges.push(IpRange {
            cidr_ip: Some(canonical),
            description: range.description.clone(),
        });
    }

    let mut ipv6_ranges: Vec<Ipv6Range> = Vec::new();
    for range in permission.ipv6_ranges.iter().flatten() {
        validate_security_group_rule_description(range.description.as_ref())?;
        let cidr = range.cidr_ipv6.as_deref().unwrap_or_default();
        let canonical =
            canonical_ipv6_cidr(cidr).map_err(|_| malformed_security_group_rule_cidr(cidr))?;
        ipv6_ranges.push(Ipv6Range {
            cidr_ipv6: Some(canonical),
            description: range.description.clone(),
        });
    }

    let mut prefix_list_ids: Vec<PrefixListId> = Vec::new();
    for entry in permission.prefix_list_ids.iter().flatten() {
        validate_security_group_rule_description(entry.description.as_ref())?;
        let prefix_list_id = entry.prefix_list_id.as_deref().unwrap_or_default();
        // A rule granting a prefix list grants exactly the ranges THAT LIST holds, so a value
        // naming no prefix list the caller can see grants nothing and can never grant
        // anything. A value carrying this resource's own prefix NAMES a prefix list whatever
        // follows it — a prefix list id is not one of this API's `<prefix>-<hex>` resource ids,
        // AWS's own published lists carrying names rather than hexadecimal — so "no list of
        // that id" is the only question there is to ask about it, and the answer is the prefix
        // list's OWN not-found class: the one answer every path that resolves a rule's
        // `PrefixListId` gives
        // (`crate::ops::authorize_security_group_egress::referenced_prefix_lists`, which asks
        // it of the Region's published catalog as well). A value carrying no prefix at all
        // names no resource of any kind, which is a malformed member rather than a missing
        // list.
        if !prefix_list_id_is_well_formed(prefix_list_id) {
            let names_a_prefix_list =
                prefix_list_id.starts_with(&format!("{PREFIX_LIST_ID_PREFIX}-"));
            if !names_a_prefix_list {
                return Err(invalid_parameter_value(format!(
                    "Invalid prefix list id '{prefix_list_id}' for {member}"
                )));
            }
            return Err(invalid_prefix_list_id_not_found(prefix_list_id));
        }
        prefix_list_ids.push(PrefixListId {
            prefix_list_id: Some(prefix_list_id.to_string()),
            description: entry.description.clone(),
        });
    }

    let mut user_id_group_pairs: Vec<UserIdGroupPair> = Vec::new();
    for pair in permission.user_id_group_pairs.iter().flatten() {
        validate_security_group_rule_description(pair.description.as_ref())?;
        user_id_group_pairs.push(prepared_user_id_group_pair(store, vpc_id, pair, member)?);
    }

    // "You must specify exactly one of the following sources" (an inbound rule) or
    // "destinations" (an outbound one) — of a RULE, so a permission that names none
    // describes no rule at all, and the refusal names the one ITS direction documents.
    if ip_ranges.is_empty()
        && ipv6_ranges.is_empty()
        && prefix_list_ids.is_empty()
        && user_id_group_pairs.is_empty()
    {
        let grant = if is_egress { "destination" } else { "source" };
        return Err(missing_parameter(format!(
            "The request must name a {grant} for {member}: an IPv4 range \
             (ipRanges), an IPv6 range (ipv6Ranges), a prefix list (prefixListIds) or a \
             security group (groups)"
        )));
    }

    Ok(IpPermission {
        // Stored in the one form a protocol has, the way the CIDRs above are stored
        // canonicalised, so one rule has one stored form however the request spelled it.
        ip_protocol: Some(canonical_ip_protocol(protocol)),
        from_port: permission.from_port,
        to_port: permission.to_port,
        user_id_group_pairs: Some(user_id_group_pairs),
        ip_ranges: Some(ip_ranges),
        ipv6_ranges: Some(ipv6_ranges),
        prefix_list_ids: Some(prefix_list_ids),
    })
}

/// One referenced-group grant in the form the rule that names it is stored in: the group by
/// the ID a rule references it by, and the account that owns it.
///
/// "For a referenced security group in another VPC, the account ID of the referenced security
/// group is returned in the response" (`UserIdGroupPair$UserId`): the owner is this account
/// unless the request named another one. A group of ANOTHER account is not in this store, so
/// whether it exists is not a question this emulator can answer; one of this account's has to
/// exist.
pub fn prepared_user_id_group_pair<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    vpc_id: &str,
    pair: &UserIdGroupPair,
    member: &str,
) -> Result<UserIdGroupPair, Scaffolded<E>> {
    let user_id = pair
        .user_id
        .clone()
        .filter(|user_id| !user_id.is_empty())
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string());
    if !aws_account_id_is_well_formed(&user_id) {
        return Err(invalid_parameter_value(format!(
            "Invalid account id '{user_id}' for {member}"
        )));
    }
    // "The ID of the VPC for the referenced security group, if applicable"
    // (`UserIdGroupPair$VpcId`) and "the ID of the VPC peering connection, if applicable"
    // (`$VpcPeeringConnectionId`): a value that is not an id of its kind names no resource
    // at all, so it is refused rather than stored on the rule. The prefix each kind is
    // asked about is the prefix ALONE — [`prefixed_id_is_well_formed`] reads the separator
    // itself — so a well-formed id of either kind is stored rather than refused.
    for (id_member, prefix, value) in [
        ("vpcId", "vpc", pair.vpc_id.as_deref()),
        (
            "vpcPeeringConnectionId",
            "pcx",
            pair.vpc_peering_connection_id.as_deref(),
        ),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            if !prefixed_id_is_well_formed(prefix, value) {
                return Err(invalid_parameter_value(format!(
                    "Invalid id '{value}' for {member}.{id_member}"
                )));
            }
        }
    }
    if let Some(status) = pair.peering_status.as_deref().filter(|it| !it.is_empty()) {
        if !PEERING_STATUSES.contains(&status) {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{status}' for {member}.peeringStatus: it must \
                 be one of {known}",
                known = PEERING_STATUSES.join(", "),
            )));
        }
    }

    let group_id = match (
        pair.group_id.as_deref().filter(|id| !id.is_empty()),
        pair.group_name.as_deref().filter(|name| !name.is_empty()),
    ) {
        // A value that is not a security group id names no group in any account, so it is
        // refused whoever the request says owns it.
        (Some(group_id), _) => {
            if !security_group_id_is_well_formed(group_id) {
                return Err(invalid_group_id_malformed(group_id));
            }
            let owned_here = user_id == ::engine::emulator_account_id();
            if owned_here {
                // "A security group and the referenced security group must be in the same VPC,
                // or in peered VPCs" (the EC2 User Guide's security group rules): a rule of a
                // group of one VPC cannot reference a group of another, because the reference
                // is not VISIBLE from inside that network. So the lookup is scoped to the
                // rule's own VPC, and a group outside it is reported as not existing IN that
                // VPC — which is exactly what it is.
                //
                // The two members that make a reference to ANOTHER network legitimate are the
                // ones that identify it: "the ID of the VPC for the referenced security group"
                // (`UserIdGroupPair$VpcId`) — which has to be the VPC that group is really in
                // — and "the ID of the VPC peering connection, if applicable"
                // (`$VpcPeeringConnectionId`), which has to be a connection this account holds
                // BETWEEN the two VPCs ([`ComAmazonawsEc2Amazonec2State::vpc_peering_connection_between`],
                // the one reader of that question). A value naming neither is not permission.
                let held = store
                    .security_groups
                    .get(group_id)
                    .ok_or_else(|| invalid_group_not_found(group_id))?;
                let group_vpc_id = held.vpc_id.clone().unwrap_or_default();
                let names_the_groups_vpc = pair
                    .vpc_id
                    .as_deref()
                    .is_some_and(|named| !named.is_empty() && named == group_vpc_id);
                let joined_by_a_peering = pair
                    .vpc_peering_connection_id
                    .as_deref()
                    .filter(|peering| !peering.is_empty())
                    .is_some_and(|peering| {
                        store.vpc_peering_connection_between(vpc_id, &group_vpc_id)
                            == Some(peering)
                    });
                let visible = vpc_id.is_empty()
                    || group_vpc_id == vpc_id
                    || names_the_groups_vpc
                    || joined_by_a_peering;
                if !visible {
                    // The group exists, in another network the request did not identify, so
                    // from the rule's VPC there is no such group — in EC2's own wording for
                    // this class, which names the network the lookup was scoped to.
                    return Err(Scaffolded::service_error(
                        INVALID_GROUP_NOT_FOUND,
                        400,
                        format!("The security group '{group_id}' does not exist in VPC '{vpc_id}'"),
                    ));
                }
            }
            group_id.to_string()
        }
        (None, Some(group_name)) => store
            .security_group_id_for_name(vpc_id, group_name)
            .ok_or_else(|| invalid_group_not_found(group_name))?,
        (None, None) => {
            return Err(invalid_permission_malformed(format!(
                "{member} names a security group with neither a groupId nor \
                 a groupName"
            )))
        }
    };

    Ok(UserIdGroupPair {
        group_id: Some(group_id),
        group_name: pair.group_name.clone(),
        user_id: Some(user_id),
        description: pair.description.clone(),
        vpc_id: pair.vpc_id.clone(),
        vpc_peering_connection_id: pair.vpc_peering_connection_id.clone(),
        peering_status: pair.peering_status.clone(),
    })
}

/// Refuse an authorize that would add a rule `group_id` already holds on the `is_egress`
/// side, or that one request describes twice — with nothing stored either way.
///
/// "The specified rule already exists for that security group"
/// (`InvalidPermission.Duplicate`), asked of the rules the group holds on THAT side and of
/// the earlier rules of the same request, since one call cannot add the same rule twice
/// either. Whether two rules are ONE rule is [`SecurityGroupRuleAtom::identity`], the same
/// comparison a revoke matches a rule by, so an authorize and a revoke cannot disagree
/// about which rule a request names.
pub fn refuse_duplicate_security_group_rules<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    group_id: &str,
    is_egress: bool,
    prepared: &[IpPermission],
) -> Result<(), Scaffolded<E>> {
    // The group's rules on this side, by identity, so a request of N rules costs N lookups
    // rather than N times the rules the group holds.
    let held: HashSet<SecurityGroupRuleKey> = store
        .security_group_rules
        .values()
        .filter(|record| {
            record.rule.group_id.as_deref() == Some(group_id)
                && record.rule.is_egress == Some(is_egress)
        })
        .filter_map(|record| Some(SecurityGroupRuleAtom::of_stored_rule(record)?.identity()))
        .collect();
    let mut described: HashSet<SecurityGroupRuleKey> = HashSet::new();
    for permission in prepared {
        for atom in security_group_rule_atoms(permission) {
            let identity = atom.identity();
            if held.contains(&identity) || !described.insert(identity) {
                return Err(invalid_permission_duplicate(&atom));
            }
        }
    }
    Ok(())
}

/// The caller-chosen bytes a stored VPC retains: its tags, plus the strings an IPv6
/// association holds. The same quantity `CreateVpc` charged against the store's budget, so
/// [`ComAmazonawsEc2Amazonec2State::remove_vpc`] gives back exactly what was taken.
pub fn vpc_payload_bytes(vpc: &Vpc) -> usize {
    let ipv6_bytes: usize = vpc
        .ipv6_cidr_block_association_set
        .iter()
        .flatten()
        .map(|association| {
            association.ipv6_cidr_block.as_deref().map_or(0, str::len)
                + association.ipv6_pool.as_deref().map_or(0, str::len)
                + association.network_border_group.as_deref().map_or(0, str::len)
        })
        .sum();
    tag_list_bytes(vpc.tags.as_deref().unwrap_or_default()).saturating_add(ipv6_bytes)
}

/// What ONE of a subnet's IPv6 CIDR block associations costs the store's byte budget: the
/// fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, plus the block string
/// the caller chose.
///
/// The ONE definition of that charge, so the writer that takes it
/// ([`ComAmazonawsEc2Amazonec2State::associate_subnet_ipv6_cidr_block`]) and
/// [`subnet_payload_bytes`], which gives it back, cannot disagree — and so a loop of
/// associate calls carrying a short block cannot be nearly free against
/// [`MAX_STORED_BYTES`] while each stored association, its minted id and its state retain
/// far more.
pub fn subnet_ipv6_association_payload_bytes(ipv6_cidr_block: &str) -> usize {
    STORED_RECORD_OVERHEAD_BYTES.saturating_add(ipv6_cidr_block.len())
}

/// The caller-chosen bytes a stored subnet retains: its tags, which is what
/// [`ComAmazonawsEc2Amazonec2State::insert_subnet`] charged, plus what each IPv6
/// association costs ([`subnet_ipv6_association_payload_bytes`]), which is what
/// [`ComAmazonawsEc2Amazonec2State::associate_subnet_ipv6_cidr_block`] charged — so
/// [`ComAmazonawsEc2Amazonec2State::remove_subnet`] gives back exactly what was taken,
/// the same arrangement [`vpc_payload_bytes`] has for a VPC.
pub fn subnet_payload_bytes(subnet: &Subnet) -> usize {
    let ipv6_bytes: usize = subnet
        .ipv6_cidr_block_association_set
        .iter()
        .flatten()
        .map(|association| {
            subnet_ipv6_association_payload_bytes(
                association.ipv6_cidr_block.as_deref().unwrap_or_default(),
            )
        })
        .sum();
    tag_list_bytes(subnet.tags.as_deref().unwrap_or_default()).saturating_add(ipv6_bytes)
}

// ---------------------------------------------------------------------------
// Carrier gateways
// ---------------------------------------------------------------------------
//
// A carrier gateway is a resource INSIDE a VPC, like a subnet or a security group:
// "Creates a carrier gateway" with "the ID of the VPC to associate with the carrier
// gateway" (`CreateCarrierGatewayRequest$VpcId`), reported by
// `DescribeCarrierGateways`. What `CreateCarrierGateway` and `DescribeCarrierGateways`
// share — the id mint, the state the clock owes, the capacity ceiling and the error
// codes a refusal carries — lives here, so the two cannot disagree about one gateway.

/// A carrier gateway AWS is still creating: the `CarrierGatewayState` enum's `pending`
/// member (`smithy_query(query="shape",shape="CarrierGatewayState")`), which is what
/// `CreateCarrierGateway` answers with until [`TRANSITION_DELAY`] has passed — the
/// carrier-gateway counterpart of [`SUBNET_STATE_PENDING`].
pub const CARRIER_GATEWAY_STATE_PENDING: &str = "pending";

/// A carrier gateway that is ready: the `CarrierGatewayState` enum's `available` member,
/// the state the clock owes a `pending` gateway [`TRANSITION_DELAY`] after its create.
pub const CARRIER_GATEWAY_STATE_AVAILABLE: &str = "available";

/// A carrier gateway a `DeleteCarrierGateway` has taken down: the `CarrierGatewayState`
/// enum's `deleting` member, the state that delete answers the gateway with.
pub const CARRIER_GATEWAY_STATE_DELETING: &str = "deleting";

/// The `ResourceType` a `CreateCarrierGateway` tag specification may name: the gateway it
/// creates (`ResourceType` enum member `carrier-gateway`).
pub const RESOURCE_TYPE_CARRIER_GATEWAY: &str = "carrier-gateway";

/// The id prefix every carrier gateway id carries (`cagw-…`, the form the EC2 console and
/// CLI report a carrier gateway under).
pub const CARRIER_GATEWAY_ID_PREFIX: &str = "cagw";

/// The largest number of carrier gateways this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each gateway is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_carrier_gateway_capacity`] is the predicate a
/// writer asks first; a create past it refuses with [`CARRIER_GATEWAY_LIMIT_EXCEEDED`].
pub const MAX_CARRIER_GATEWAYS: usize = 1_000;

/// The error code a create refuses with once the store is full, in the form EC2 names
/// every quota refusal with (`VpcLimitExceeded`, `SubnetLimitExceeded`,
/// `SecurityGroupLimitExceeded`, `VolumeLimitExceeded`): `<Resource>LimitExceeded`. The
/// ceiling it reports is this store's ([`MAX_CARRIER_GATEWAYS`]), not a published AWS
/// quota.
pub const CARRIER_GATEWAY_LIMIT_EXCEEDED: &str = "CarrierGatewayLimitExceeded";

/// EC2's error code for a request that named a carrier gateway id this account holds no
/// gateway for (EC2 API error codes, "InvalidCarrierGatewayID.NotFound").
pub const INVALID_CARRIER_GATEWAY_ID_NOT_FOUND: &str = "InvalidCarrierGatewayID.NotFound";

/// What one carrier gateway is created with, beyond everything the service itself derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_carrier_gateway`] mints the id and fills in
/// the owning account).
#[derive(Debug, Clone)]
pub struct CarrierGatewaySpec {
    /// "The ID of the VPC to associate with the carrier gateway"
    /// (`CreateCarrierGatewayRequest$VpcId`).
    pub vpc_id: String,
    /// The `CarrierGatewayState` the gateway starts in.
    pub state: String,
    /// "The tags to associate with the carrier gateway"
    /// (`CreateCarrierGatewayRequest$TagSpecifications`).
    pub tags: Vec<Tag>,
    /// The `ClientToken` the request carried, if any: the token map keeps it for as long
    /// as the gateway lives, so the bytes it holds are charged with the gateway's.
    pub client_token: Option<String>,
}

/// The caller-chosen bytes a stored carrier gateway holds: its tags and the `ClientToken`
/// the token map keeps for it. Everything else on the record — the id, the account, the
/// VPC id and the state — is derived by this module or bounded by the model, so it is
/// fixed-size.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume:
/// `CreateCarrierGateway` asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_carrier_gateway`] charges exactly it.
pub fn carrier_gateway_payload_bytes(tags: &[Tag], client_token: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(client_token.map_or(0, str::len))
}

/// Refuse a request that named a carrier gateway id no gateway of the account has, the way
/// EC2 does: HTTP 400 `InvalidCarrierGatewayID.NotFound`, with the message EC2 builds from
/// the id the request named — it spells the resource with the modelled shape's own name
/// (`CarrierGateway`), as the real service does.
pub fn invalid_carrier_gateway_id_not_found<E>(carrier_gateway_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CARRIER_GATEWAY_ID_NOT_FOUND,
        400,
        format!("The CarrierGateway ID '{carrier_gateway_id}' does not exist"),
    )
}

/// Refuse a create that would take the store past [`MAX_CARRIER_GATEWAYS`]: HTTP 400
/// `CarrierGatewayLimitExceeded`.
pub fn carrier_gateway_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(CARRIER_GATEWAY_LIMIT_EXCEEDED, 400, message)
}

/// `InvalidVpcId.Malformed` (HTTP 400): the carrier gateway surface's refusal of a `VpcId`
/// it cannot resolve.
///
/// The code is spelled `Id` rather than `ID` and says `Malformed` rather than `NotFound`,
/// which is a DIVERGENCE from [`INVALID_VPC_ID_NOT_FOUND`], the code the rest of this
/// crate answers for a VPC id the account holds no VPC for. It is what a recorded
/// `CreateCarrierGateway` refusal carries: that surface asks one question of the member —
/// does it name a VPC this account has a carrier gateway to attach to — and answers the
/// one code whatever the value's shape, so a well-formed id with no VPC behind it is
/// refused exactly as a value that is no id at all is.
pub const INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED: &str = "InvalidVpcId.Malformed";

/// The most characters of a refused `VpcId` its message repeats back.
///
/// The message names the value so a caller can tell which of its ids was refused, and that
/// value is the request's own bytes — `VpcId` is a bare string the model puts no `@length`
/// on — so the echo is bounded the way [`MAX_ECHOED_PAGE_TOKEN_CHARS`] bounds a refused
/// token's. Far above the longest VPC id EC2 issues (`vpc-` and 17 digits), so a real id is
/// always named in full.
pub const MAX_ECHOED_VPC_ID_CHARS: usize = 128;

/// Refuse a `VpcId` the surfaces recorded answering this refusal cannot resolve: HTTP 400
/// [`INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED`] (`InvalidVpcId.Malformed`), with the sentence
/// those recordings carry — `The vpc ID '<value>' is malformed`, built from the value the
/// request sent and bounded by [`MAX_ECHOED_VPC_ID_CHARS`].
///
/// Answered whatever the value's SHAPE: a well-formed id with no VPC behind it gets it as
/// well as a value that is no VPC id at all, which is what those recordings show — the
/// surface asks ONE question of the member (does it name a VPC this account holds) and
/// answers one code. It is NOT the code for every unresolvable `VpcId` of this service:
/// WHICH refusal an operation answers is a fact of the operation, and the ones recorded
/// answering `InvalidVpcID.NotFound` keep [`invalid_vpc_id_not_found`].
///
/// The ONE definition of this refusal, for both of its call sites —
/// [`carrier_gateway_vpc_id_malformed`] is the same answer reached from the carrier gateway
/// surface — so the two cannot word one unresolvable VPC id differently. It lives in this
/// section because that surface is where the rule was first recorded.
pub fn invalid_vpc_id_malformed<E>(vpc_id: &str) -> Scaffolded<E> {
    let echoed: String = vpc_id.chars().take(MAX_ECHOED_VPC_ID_CHARS).collect();
    Scaffolded::service_error(
        INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED,
        400,
        format!("The vpc ID '{echoed}' is malformed"),
    )
}

/// Refuse a `CreateCarrierGateway` whose `VpcId` names no VPC of the account: HTTP 400
/// [`INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED`], worded the way this surface words it —
/// `The vpc ID '<id>' is malformed`, built from the id the request sent.
///
/// The same refusal [`invalid_vpc_id_malformed`] answers, which is where it is worded: this
/// surface reached the rule first, so the name is kept for its call site. The id it names is
/// bounded there ([`MAX_ECHOED_VPC_ID_CHARS`], far above any id EC2 issues), so a value
/// longer than that bound is named only as far as it.
pub fn carrier_gateway_vpc_id_malformed<E>(vpc_id: &str) -> Scaffolded<E> {
    invalid_vpc_id_malformed(vpc_id)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another carrier gateway fits inside the store's ceiling
    /// ([`MAX_CARRIER_GATEWAYS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`carrier_gateway_limit_exceeded`] when it answers `false`.
    pub fn has_carrier_gateway_capacity(&self) -> bool {
        self.carrier_gateways.len() < MAX_CARRIER_GATEWAYS
    }

    /// Insert a carrier gateway, and answer the `CarrierGatewayId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id and the owning account — so two writers cannot report the same
    /// gateway differently. Like [`Self::insert_subnet`], the caller-chosen bytes the
    /// record retains ([`carrier_gateway_payload_bytes`], the tags plus the client token
    /// the token map keeps) are charged against the store's byte budget here, so a writer
    /// cannot store them uncharged.
    pub fn insert_carrier_gateway(&mut self, spec: CarrierGatewaySpec) -> String {
        let carrier_gateway_id = self.next_id(CARRIER_GATEWAY_ID_PREFIX);
        let carrier_gateway = crate::ops::types::CarrierGateway {
            carrier_gateway_id: Some(carrier_gateway_id.clone()),
            vpc_id: Some(spec.vpc_id),
            state: Some(spec.state),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(carrier_gateway_payload_bytes(
            carrier_gateway.tags.as_deref().unwrap_or_default(),
            spec.client_token.as_deref(),
        ));
        if let Some(token) = spec.client_token {
            self.carrier_gateway_client_tokens
                .insert(token, carrier_gateway_id.clone());
        }
        self.carrier_gateways
            .insert(carrier_gateway_id.clone(), carrier_gateway);
        carrier_gateway_id
    }

    /// Remove the carrier gateway `carrier_gateway_id` and answer it as it was last
    /// reported, or `None` when the store holds no such gateway.
    ///
    /// The bytes the record held go back to the store's budget on the terms
    /// [`carrier_gateway_payload_bytes`] took them on, the `ClientToken` that minted it goes
    /// with it (a token must never answer with a gateway that is no longer stored), and so
    /// does the `available` move the clock may still owe it, so a later materialisation
    /// cannot resurrect a deleted gateway's state.
    pub fn remove_carrier_gateway(
        &mut self,
        carrier_gateway_id: &str,
    ) -> Option<crate::ops::types::CarrierGateway> {
        let carrier_gateway = self.carrier_gateways.remove(carrier_gateway_id)?;
        self.carrier_gateway_available_at.remove(carrier_gateway_id);
        let client_token = self
            .carrier_gateway_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == carrier_gateway_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &client_token {
            self.carrier_gateway_client_tokens.remove(token);
        }
        self.release_stored_bytes(carrier_gateway_payload_bytes(
            carrier_gateway.tags.as_deref().unwrap_or_default(),
            client_token.as_deref(),
        ));
        Some(carrier_gateway)
    }
}
