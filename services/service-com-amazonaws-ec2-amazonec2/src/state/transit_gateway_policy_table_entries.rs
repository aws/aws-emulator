// SPDX-License-Identifier: CC0-1.0

//! The entries of a transit gateway policy table: what one is stored as, the rule a request
//! may describe, the state a created entry is in, what a create and a reader are refused
//! with, the store's bounds and its one writer.
//!
//! The tables themselves — their id form, their own states and the refusals for an id that
//! names none — live in the "Transit gateway policy tables" section of
//! [`crate::state`], beside the other stores; this module holds only what an ENTRY of one
//! adds to them. The entries are kept inside
//! [`super::TransitGatewayPolicyTableRecord::entries`], keyed by rule number, as the
//! modelled [`TransitGatewayPolicyTableEntry`] shape itself, so every member a writer sets
//! round-trips through `GetTransitGatewayPolicyTableEntries` without a field-by-field copy
//! that can silently drop one.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use super::{
    ComAmazonawsEc2Amazonec2State, MAX_STORED_RECORDS_PER_STORE, STORED_RECORD_OVERHEAD_BYTES,
    canonical_cidr_block, echoed_resource_id, invalid_parameter_value,
    metering_policy_port_range_is_well_formed,
};
use crate::ops::types::{
    TransitGatewayPolicyRule, TransitGatewayPolicyRuleMetaData, TransitGatewayPolicyTableEntry,
    TransitGatewayRequestPolicyRule,
};
use crate::scaffolded::Scaffolded;

/// An entry that is in force: the `active` member of the modelled
/// `TransitGatewayPolicyTableEntryState`
/// ([`crate::ops::types::TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_VALUES`], whose only
/// members are `active` and `deleted`).
///
/// That catalog names no transient state, so a created entry is reported in this, its
/// terminal, state straight away and the clock owes it no move — unlike the table it belongs
/// to, whose own catalog does carry a `pending` member.
pub const TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE: &str = "active";

/// An entry a delete has taken out of its table: that catalog's other member, the state the
/// delete reports the entry it removed in.
pub const TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_DELETED: &str = "deleted";

/// EC2's code for an entry the policy table already holds at that rule number.
///
/// Named the way EC2 names the same condition for its other numbered entries
/// ([`super::NETWORK_ACL_ENTRY_ALREADY_EXISTS`]): a rule number identifies one entry of one
/// table, so a create never overwrites one —
/// `ModifyTransitGatewayPolicyTableEntry` is the operation that changes an entry in place.
pub const TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_ALREADY_EXISTS: &str =
    "TransitGatewayPolicyTableEntryAlreadyExists";

/// EC2's code for an entry the policy table does not hold, named in the `Invalid<Kind>.NotFound`
/// spelling this service answers an unresolvable resource with
/// ([`super::INVALID_NETWORK_ACL_ENTRY_NOT_FOUND`] is the same code for an ACL's entries).
pub const INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_NOT_FOUND: &str =
    "InvalidTransitGatewayPolicyTableEntry.NotFound";

/// The smallest rule number a policy table entry may take.
///
/// "The rule number for the policy table entry. Lower rule numbers are evaluated first and
/// take precedence" (`CreateTransitGatewayPolicyTableEntryRequest$PolicyRuleNumber`), so the
/// number is an ordinal position in that evaluation order, and EC2's numbered entries count
/// from one ("Positive integer from 1 to 32766", `CreateNetworkAclEntryRequest$RuleNumber`,
/// the same family's documented bound). No upper bound is published for THIS resource, so
/// none is enforced: the number a signed 32-bit integer holds is the ceiling.
pub const MIN_TRANSIT_GATEWAY_POLICY_TABLE_RULE_NUMBER: i32 = 1;

/// The value every member of a policy rule takes to mean "match anything": "`*` for all
/// protocols" (`TransitGatewayRequestPolicyRule$Protocol`), and the value a port range must
/// carry for a protocol that has no ports (`$SourcePortRange`, `$DestinationPortRange`).
pub const TRANSIT_GATEWAY_POLICY_RULE_ANY: &str = "*";

/// The protocols a policy rule may match on, as the member that publishes the catalog spells
/// them: "The protocol for the policy rule. Valid values are `1` (ICMP), `6` (TCP), `17`
/// (UDP), `47` (GRE), or `*` for all protocols"
/// (`smithy_query(query="doc",shape="TransitGatewayRequestPolicyRule",member="Protocol")`).
///
/// A protocol number this service accepts elsewhere (a security group rule takes any of
/// 0..=255) is NOT one a policy rule may name: this catalog is the published one, so it is
/// the one enforced.
pub const TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS: [&str; 5] = ["1", "6", "17", "47", "*"];

/// The protocol of TCP, one of the two a policy rule may carry a port range for: "You can
/// specify a port range only when `Protocol` is `6` (TCP) or `17` (UDP)"
/// (`TransitGatewayRequestPolicyRule$SourcePortRange`).
pub const TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_TCP: &str = "6";

/// The protocol of UDP, the other one (same member).
pub const TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_UDP: &str = "17";

/// Whether a policy rule carrying `protocol` may name a range of ports: only TCP and UDP
/// carry ports, "for all other protocols, this value must be `*`"
/// (`TransitGatewayRequestPolicyRule$SourcePortRange`) — so a rule that names no protocol
/// at all names no ports either, there being no protocol for the ports to belong to.
pub fn transit_gateway_policy_rule_has_ports(protocol: Option<&str>) -> bool {
    matches!(
        protocol,
        Some(TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_TCP | TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_UDP)
    )
}

/// The rule number `value` names, as the number it is ordered by — or a refusal when it
/// names no rule number at all.
///
/// `PolicyRuleNumber` is modelled as a string, and "lower rule numbers are evaluated first
/// and take precedence" (`CreateTransitGatewayPolicyTableEntryRequest$PolicyRuleNumber`), so
/// the value is read as the number it orders by rather than compared as text — otherwise
/// rule 10 would be evaluated before rule 2. The number is also what identifies an entry
/// within its table, so every operation that names one reads it HERE and two spellings of
/// one number name one entry.
pub fn transit_gateway_policy_table_rule_number<E>(value: &str) -> Result<i32, Scaffolded<E>> {
    let number = value
        .parse::<i32>()
        .ok()
        .filter(|number| *number >= MIN_TRANSIT_GATEWAY_POLICY_TABLE_RULE_NUMBER);
    number.ok_or_else(|| {
        invalid_parameter_value(format!(
            "Value ({echoed}) for parameter policyRuleNumber is invalid. A rule number is a \
             positive integer, from {first}",
            echoed = echoed_resource_id(value),
            first = MIN_TRANSIT_GATEWAY_POLICY_TABLE_RULE_NUMBER,
        ))
    })
}

/// The rule a request describes, in the form it is STORED and reported in.
///
/// The ONE reading of a `TransitGatewayRequestPolicyRule`, so the create and the modify
/// cannot accept different rules: every member is optional, each CIDR block is stored in the
/// canonical form every EC2 `CidrBlock` member documents ("we modify the specified CIDR
/// block to its canonical form"), the protocol must be one the member's own published catalog
/// admits ([`TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS`]), and a port range may name ports only
/// for a protocol that HAS ports — "for all other protocols, this value must be `*`"
/// (`TransitGatewayRequestPolicyRule$SourcePortRange`).
///
/// An entry always HAS matching criteria: a criterion the request does not narrow matches
/// anything, and the word this rule's own members carry for that is `*` ("`*` for all
/// protocols", `$Protocol`; "this value must be `*`", `$SourcePortRange`), so each of the
/// five criteria is recorded, narrowed where the request narrowed it and
/// [`TRANSIT_GATEWAY_POLICY_RULE_ANY`] where it did not. The rule's metadata is not a
/// criterion — it is a key-value pair the caller attaches — so it is recorded only when the
/// request names one.
pub fn requested_transit_gateway_policy_rule<E>(
    requested: Option<&TransitGatewayRequestPolicyRule>,
) -> Result<Option<TransitGatewayPolicyRule>, Scaffolded<E>> {
    updated_transit_gateway_policy_rule(None, requested)
}

/// The rule an entry carries once a request has UPDATED it: the one the request describes
/// laid over the one the entry holds, in the form it is stored and reported in.
///
/// "The updated matching criteria for the policy table entry. UNSPECIFIED FIELDS RETAIN THEIR
/// CURRENT VALUES" (`ModifyTransitGatewayPolicyTableEntryRequest$PolicyRule`), so the merge is
/// per FIELD rather than per structure: a field the request names replaces the one the entry
/// held, and a field it leaves out keeps the value the entry already carried — including the
/// two fields of the rule's metadata. The merged rule is then held to exactly the same
/// constraints a created one is ([`requested_transit_gateway_policy_rule`]), so an update
/// cannot leave an entry carrying a rule a create would have refused.
pub fn updated_transit_gateway_policy_rule<E>(
    current: Option<&TransitGatewayPolicyRule>,
    requested: Option<&TransitGatewayRequestPolicyRule>,
) -> Result<Option<TransitGatewayPolicyRule>, Scaffolded<E>> {
    let requested = match requested {
        Some(requested) => ::std::borrow::Cow::Borrowed(requested),
        // A request that names no rule at all narrows nothing, which is the same rule as one
        // that names an empty one: every criterion matches anything.
        None => ::std::borrow::Cow::Owned(TransitGatewayRequestPolicyRule::default()),
    };
    let requested = requested.as_ref();
    // ec2Query carries an unset string member as an absent one, and a caller that sent the
    // key with nothing after it has named no value either — so such a field is one the
    // request did not specify, and it retains the value the entry holds.
    let named = |given: Option<&String>, held: Option<&String>| -> Option<String> {
        given
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| held.cloned())
    };
    let current_meta_data = current.and_then(|rule| rule.meta_data.as_ref());

    let protocol = narrowed_or_any(named(
        requested.protocol.as_ref(),
        current.and_then(|rule| rule.protocol.as_ref()),
    ));
    if let Some(protocol) = protocol.as_deref() {
        if !TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS.contains(&protocol) {
            return Err(invalid_parameter_value(format!(
                "Value ({echoed}) for parameter protocol is invalid. Valid values are {valid}",
                echoed = echoed_resource_id(protocol),
                valid = TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS.join(", "),
            )));
        }
    }

    let source_cidr_block = matched_cidr_block(
        narrowed_or_any(named(
            requested.source_cidr_block.as_ref(),
            current.and_then(|rule| rule.source_cidr_block.as_ref()),
        ))
        .as_deref(),
        "sourceCidrBlock",
    )?;
    let destination_cidr_block = matched_cidr_block(
        narrowed_or_any(named(
            requested.destination_cidr_block.as_ref(),
            current.and_then(|rule| rule.destination_cidr_block.as_ref()),
        ))
        .as_deref(),
        "destinationCidrBlock",
    )?;
    let source_port_range = matched_port_range(
        narrowed_or_any(named(
            requested.source_port_range.as_ref(),
            current.and_then(|rule| rule.source_port_range.as_ref()),
        ))
        .as_deref(),
        protocol.as_deref(),
        "sourcePortRange",
    )?;
    let destination_port_range = matched_port_range(
        narrowed_or_any(named(
            requested.destination_port_range.as_ref(),
            current.and_then(|rule| rule.destination_port_range.as_ref()),
        ))
        .as_deref(),
        protocol.as_deref(),
        "destinationPortRange",
    )?;
    let requested_meta_data = requested.meta_data.as_ref();
    let meta_data = Some(TransitGatewayPolicyRuleMetaData {
        meta_data_key: named(
            requested_meta_data.and_then(|meta_data| meta_data.meta_data_key.as_ref()),
            current_meta_data.and_then(|meta_data| meta_data.meta_data_key.as_ref()),
        ),
        meta_data_value: named(
            requested_meta_data.and_then(|meta_data| meta_data.meta_data_value.as_ref()),
            current_meta_data.and_then(|meta_data| meta_data.meta_data_value.as_ref()),
        ),
    })
    .filter(|meta_data| meta_data.meta_data_key.is_some() || meta_data.meta_data_value.is_some());

    let rule = TransitGatewayPolicyRule {
        source_cidr_block,
        source_port_range,
        destination_cidr_block,
        destination_port_range,
        protocol,
        meta_data,
    };
    Ok(Some(rule))
}

/// The criterion a rule records for a member: the value the request or the entry narrowed it
/// to, or [`TRANSIT_GATEWAY_POLICY_RULE_ANY`] when neither did — the word this rule's members
/// carry for "matches anything".
fn narrowed_or_any(value: Option<String>) -> Option<String> {
    Some(value.unwrap_or_else(|| TRANSIT_GATEWAY_POLICY_RULE_ANY.to_string()))
}

/// A CIDR block a policy rule matches on, in the canonical form it is stored and reported
/// in, or `None` when the request named none.
fn matched_cidr_block<E>(
    cidr_block: Option<&str>,
    member: &str,
) -> Result<Option<String>, Scaffolded<E>> {
    let Some(cidr_block) = cidr_block else {
        return Ok(None);
    };
    // A criterion the request did not narrow is carried as [`TRANSIT_GATEWAY_POLICY_RULE_ANY`]
    // ([`narrowed_or_any`]), which is not an address range: it is stored as it stands rather
    // than read as one, so only a block a request DID name is held to the CIDR form.
    if cidr_block == TRANSIT_GATEWAY_POLICY_RULE_ANY {
        return Ok(Some(cidr_block.to_string()));
    }
    let canonical = canonical_cidr_block(cidr_block).ok_or_else(|| {
        invalid_parameter_value(format!(
            "Value ({echoed}) for parameter {member} is invalid. This is not a valid CIDR block.",
            echoed = echoed_resource_id(cidr_block),
        ))
    })?;
    Ok(Some(canonical))
}

/// A port range a policy rule matches on, as it is stored, or `None` when the request named
/// none.
///
/// "You can specify a port range only when `Protocol` is `6` (TCP) or `17` (UDP); for all
/// other protocols, this value must be `*`" — so the value is held to BOTH halves of that
/// sentence: a range at all only for a protocol that has ports
/// ([`transit_gateway_policy_rule_has_ports`]), and a range that IS given has to name ports,
/// read by the one reading of a port-range string this crate has
/// ([`metering_policy_port_range_is_well_formed`]).
fn matched_port_range<E>(
    port_range: Option<&str>,
    protocol: Option<&str>,
    member: &str,
) -> Result<Option<String>, Scaffolded<E>> {
    let Some(port_range) = port_range else {
        return Ok(None);
    };
    if port_range == TRANSIT_GATEWAY_POLICY_RULE_ANY {
        return Ok(Some(port_range.to_string()));
    }
    if !transit_gateway_policy_rule_has_ports(protocol) {
        return Err(invalid_parameter_value(format!(
            "Value ({echoed}) for parameter {member} is invalid. You can specify a port range \
             only when Protocol is {tcp} (TCP) or {udp} (UDP); for all other protocols, this \
             value must be {any}",
            echoed = echoed_resource_id(port_range),
            tcp = TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_TCP,
            udp = TRANSIT_GATEWAY_POLICY_RULE_PROTOCOL_UDP,
            any = TRANSIT_GATEWAY_POLICY_RULE_ANY,
        )));
    }
    if !metering_policy_port_range_is_well_formed(port_range) {
        return Err(invalid_parameter_value(format!(
            "Value ({echoed}) for parameter {member} is invalid. A port range is a port, or a \
             first and a last port between 0 and {max}",
            echoed = echoed_resource_id(port_range),
            max = super::MAX_PORT,
        )));
    }
    Ok(Some(port_range.to_string()))
}

/// Refuse an entry the table already holds at that rule number: HTTP 400
/// [`TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_ALREADY_EXISTS`], naming the rule number and the
/// table the request named.
pub fn transit_gateway_policy_table_entry_already_exists<E>(
    transit_gateway_policy_table_id: &str,
    rule_number: i32,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_ALREADY_EXISTS,
        400,
        format!(
            "The transit gateway policy table {transit_gateway_policy_table_id} already holds \
             an entry at rule number {rule_number}."
        ),
    )
}

/// The kind an entry-writing refusal names a policy table id by: the word the `ResourceType`
/// enum spells this resource with
/// (`smithy_query(query="shape",shape="ResourceType")`), followed by `ID` — the way an EC2
/// "does not exist" sentence names the kind of the id it could not resolve, as
/// [`super::id_not_found`]'s other callers spell theirs.
pub const TRANSIT_GATEWAY_POLICY_TABLE_ID_KIND: &str = "transit-gateway-policy-table ID";

/// Refuse a request that WRITES an entry of a transit gateway policy table id no policy table
/// of the account has: HTTP 400 [`super::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND`],
/// worded through [`super::id_not_found`] — "The `<kind>` '`<id>`' does not exist".
///
/// The same condition, code and status as
/// [`super::invalid_transit_gateway_policy_table_id_not_found`], which the policy table's own
/// operations and the REPORT of its entries answer: the operations that write an entry word
/// the sentence around the resource type and the id the request gave instead. Each wording
/// lives in one place, so no handler writes its own.
pub fn invalid_transit_gateway_policy_table_id_not_found_for_entry<E>(
    transit_gateway_policy_table_id: &str,
) -> Scaffolded<E> {
    super::id_not_found(
        super::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        TRANSIT_GATEWAY_POLICY_TABLE_ID_KIND,
        transit_gateway_policy_table_id,
    )
}

/// Refuse a request naming an entry the table does not hold: HTTP 400
/// [`INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_NOT_FOUND`].
///
/// The sentence names the two facts that say WHICH entry was looked for and did not answer —
/// there is one such entry per rule number per table — each taken from the REQUEST, the way
/// [`super::invalid_network_acl_entry_not_found`] names them for an ACL's entries.
pub fn invalid_transit_gateway_policy_table_entry_not_found<E>(
    transit_gateway_policy_table_id: &str,
    rule_number: i32,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_NOT_FOUND,
        400,
        format!(
            "There is no entry with rule number {rule_number} in transit gateway policy table \
             {transit_gateway_policy_table_id}"
        ),
    )
}

/// The caller-chosen bytes ONE stored policy table entry holds: the text of its rule's
/// matching criteria and metadata, of the route table it targets, and of the rule number and
/// state as they are reported.
///
/// The ONE definition of that charge, the way
/// [`super::transit_gateway_metering_policy_entry_payload_bytes`] is a metering entry's, so
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_policy_table_entry`] charges
/// exactly this, every later writer of one gives back exactly this, and no path can store an
/// entry uncharged.
pub fn transit_gateway_policy_table_entry_payload_bytes(
    entry: &TransitGatewayPolicyTableEntry,
) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    let rule = entry.policy_rule.as_ref().map_or(0, |rule| {
        text(rule.source_cidr_block.as_deref())
            .saturating_add(text(rule.source_port_range.as_deref()))
            .saturating_add(text(rule.destination_cidr_block.as_deref()))
            .saturating_add(text(rule.destination_port_range.as_deref()))
            .saturating_add(text(rule.protocol.as_deref()))
            .saturating_add(rule.meta_data.as_ref().map_or(0, |meta_data| {
                text(meta_data.meta_data_key.as_deref())
                    .saturating_add(text(meta_data.meta_data_value.as_deref()))
            }))
    });
    text(entry.policy_rule_number.as_deref())
        .saturating_add(text(entry.target_route_table_id.as_deref()))
        .saturating_add(text(entry.state.as_deref()))
        .saturating_add(rule)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the policy table `transit_gateway_policy_table_id` already holds an entry at
    /// `rule_number`.
    ///
    /// One rule number names one entry of one table — a delete and a modify each resolve one
    /// by its table and that number alone — so this is what a create asks before it writes.
    pub fn transit_gateway_policy_table_holds_entry(
        &self,
        transit_gateway_policy_table_id: &str,
        rule_number: i32,
    ) -> bool {
        self.transit_gateway_policy_tables
            .get(transit_gateway_policy_table_id)
            .is_some_and(|table| table.entries.contains_key(&rule_number))
    }

    /// How many entries the policy table `transit_gateway_policy_table_id` holds.
    pub fn transit_gateway_policy_table_entry_count(
        &self,
        transit_gateway_policy_table_id: &str,
    ) -> usize {
        self.transit_gateway_policy_tables
            .get(transit_gateway_policy_table_id)
            .map_or(0, |table| table.entries.len())
    }

    /// Whether the policy table `transit_gateway_policy_table_id` can hold another entry.
    ///
    /// The same contract as [`Self::has_transit_gateway_metering_policy_entry_capacity`]: a
    /// create asks first and refuses with [`super::resource_limit_exceeded`], and the writer
    /// below asks the same predicate, so the two cannot drift apart. Nothing AWS publishes
    /// bounds the entries of one policy table, so the bound is this emulator's own
    /// [`MAX_STORED_RECORDS_PER_STORE`] — the ceiling every other store of this crate holds
    /// its records to, because each entry is a record in this process's memory.
    pub fn has_transit_gateway_policy_table_entry_capacity(
        &self,
        transit_gateway_policy_table_id: &str,
    ) -> bool {
        self.transit_gateway_policy_table_entry_count(transit_gateway_policy_table_id)
            < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `view` as the entry of the policy table `transit_gateway_policy_table_id` at
    /// `rule_number`, and answer it as it was stored — or `None`, with NOTHING written, for
    /// a table this store does not hold, one that already holds an entry at that rule
    /// number, or one that holds as many entries as it can.
    ///
    /// Everything the request does not decide is decided HERE rather than by the handler, so
    /// a later writer of the kind cannot record a fresh entry differently: the rule number as
    /// it is REPORTED (the number it is ordered by, written out), and the terminal
    /// [`TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE`] its catalog admits.
    pub fn insert_transit_gateway_policy_table_entry(
        &mut self,
        transit_gateway_policy_table_id: &str,
        rule_number: i32,
        mut view: TransitGatewayPolicyTableEntry,
    ) -> Option<TransitGatewayPolicyTableEntry> {
        if !self
            .transit_gateway_policy_tables
            .contains_key(transit_gateway_policy_table_id)
            || self.transit_gateway_policy_table_holds_entry(
                transit_gateway_policy_table_id,
                rule_number,
            )
            || !self
                .has_transit_gateway_policy_table_entry_capacity(transit_gateway_policy_table_id)
        {
            return None;
        }
        view.policy_rule_number = Some(rule_number.to_string());
        view.state = Some(TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE.to_string());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + transit_gateway_policy_table_entry_payload_bytes(&view),
        );
        let table = self
            .transit_gateway_policy_tables
            .get_mut(transit_gateway_policy_table_id)?;
        table.entries.insert(rule_number, view);
        table.entries.get(&rule_number).cloned()
    }

    /// Change the entry of the policy table `transit_gateway_policy_table_id` at
    /// `rule_number` in place, and answer it as it now stands — or `None`, with NOTHING
    /// written, when that table holds no entry at that rule number or when what the change
    /// would store does not fit the store's byte budget.
    ///
    /// `policy_rule` and `target_route_table_id` are the values the entry carries AFTER the
    /// update: "unspecified fields retain their current values"
    /// (`ModifyTransitGatewayPolicyTableEntryRequest$PolicyRule`) is settled by the caller,
    /// which reads the entry as it stands and lays the request over it
    /// ([`updated_transit_gateway_policy_rule`]), so this writer stores what it is given
    /// rather than deciding retention a second way. The rule number and the state are not
    /// modifiable members, so they are left exactly as the create recorded them.
    ///
    /// The budget is asked about the NET change BEFORE anything is mutated: a change stores
    /// no further entry, so the per-table record ceiling cannot bound it, while the rule it
    /// carries is caller-chosen text — so a replacement that does not fit is declined here
    /// as well as by its caller, and no path can write past [`super::MAX_STORED_BYTES`].
    pub fn modify_transit_gateway_policy_table_entry(
        &mut self,
        transit_gateway_policy_table_id: &str,
        rule_number: i32,
        policy_rule: Option<TransitGatewayPolicyRule>,
        target_route_table_id: Option<String>,
    ) -> Option<TransitGatewayPolicyTableEntry> {
        let current = self
            .transit_gateway_policy_tables
            .get(transit_gateway_policy_table_id)?
            .entries
            .get(&rule_number)?;
        let refund = transit_gateway_policy_table_entry_payload_bytes(current);
        let mut updated = current.clone();
        updated.policy_rule = policy_rule;
        updated.target_route_table_id = target_route_table_id;
        let charge = transit_gateway_policy_table_entry_payload_bytes(&updated);
        if self.would_exceed_stored_bytes_after_refund(charge, refund) {
            return None;
        }
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        let table = self
            .transit_gateway_policy_tables
            .get_mut(transit_gateway_policy_table_id)?;
        table.entries.insert(rule_number, updated);
        table.entries.get(&rule_number).cloned()
    }

    /// Take the entry at `rule_number` out of the policy table
    /// `transit_gateway_policy_table_id`, and answer it as the delete reports it — or `None`,
    /// with nothing removed, when that table holds no entry at that rule number.
    ///
    /// The removed entry is reported in the terminal
    /// [`TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_DELETED`] its catalog admits, and the bytes
    /// it held are given back, exactly the charge
    /// [`Self::insert_transit_gateway_policy_table_entry`] made, so a table emptied and
    /// refilled does not grow the store's budget.
    pub fn remove_transit_gateway_policy_table_entry(
        &mut self,
        transit_gateway_policy_table_id: &str,
        rule_number: i32,
    ) -> Option<TransitGatewayPolicyTableEntry> {
        let table = self
            .transit_gateway_policy_tables
            .get_mut(transit_gateway_policy_table_id)?;
        let mut removed = table.entries.remove(&rule_number)?;
        self.release_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + transit_gateway_policy_table_entry_payload_bytes(&removed),
        );
        removed.state = Some(TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_DELETED.to_string());
        Some(removed)
    }
}
