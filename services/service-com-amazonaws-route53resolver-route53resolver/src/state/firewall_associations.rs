// SPDX-License-Identifier: CC0-1.0

//! What a DNS Firewall rule-group-to-VPC association admits as its `Priority`,
//! and what a rule group that still carries one refuses.
//!
//! Both are about the association as a RESOURCE rather than about the rules the
//! group holds, so they live beside each other: the band a `Priority` has to
//! fall in, stated once for the associate and the update that write one, and
//! the conflict a delete of the group the association points at is refused
//! with, with the lookup that decides it.

use super::*;

/// The smallest `Priority` the service treats as a NUMBER at all: everything at
/// or above it is outside the range of the setting, as opposed to inside it but
/// kept back for the service's own use.
///
/// `Priority` carries no `range` in the model
/// (`smithy_query(query="shape",shape="Priority")`), and
/// `AssociateFirewallRuleGroup$Priority`'s doc publishes only the usable band
/// ("the allowed values for Priority are between 100 and 9900",
/// [`ASSOCIATION_PRIORITY_RANGE`]). The service tells the two refusals apart by
/// this bound: a value below it is one the service RESERVED
/// ([`RSLVR_CODE_ASSOCIATION_PRIORITY_RESERVED`]), a value at or above it is not
/// a valid priority at all ([`RSLVR_CODE_ASSOCIATION_PRIORITY_NOT_VALID`]), and
/// the refusal for the second quotes this bound.
pub const ASSOCIATION_PRIORITY_CEILING: i32 = 10_000;

/// The Route 53 Resolver service error code for a `Priority` that falls inside
/// the numbers the setting takes but outside the band published for callers —
/// one the service keeps for itself.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text.
pub const RSLVR_CODE_ASSOCIATION_PRIORITY_RESERVED: &str = "RSLVR-02017";

/// The Route 53 Resolver service error code for a `Priority` that is not a
/// priority at all, i.e. one at or beyond [`ASSOCIATION_PRIORITY_CEILING`].
pub const RSLVR_CODE_ASSOCIATION_PRIORITY_NOT_VALID: &str = "RSLVR-02018";

/// The `ValidationException` a rule-group association's `Priority` is refused
/// with, or `None` for a priority the service admits.
///
/// The band itself is `AssociateFirewallRuleGroup$Priority`'s published one
/// ([`ASSOCIATION_PRIORITY_RANGE`]), and the update takes the same values: "you
/// can change the priority setting for a rule group association after you create
/// it" says nothing about which values it may take. A priority outside the band
/// is refused in one of TWO ways, which is why this is one function and not a
/// range check in each handler:
///
/// * a value the setting takes but the service keeps back is reported as
///   RESERVED, and the sentence quotes the band a caller may use — never the
///   value that was asked for, which the service does not name;
/// * a value at or beyond [`ASSOCIATION_PRIORITY_CEILING`] is reported as not
///   valid, and the sentence quotes that bound.
///
/// Both sentences are fixed text behind the service's own code for the fault.
pub fn association_priority_fault(priority: i32) -> Option<ValidationException> {
    if ASSOCIATION_PRIORITY_RANGE.contains(&priority) {
        return None;
    }
    let message = if priority >= ASSOCIATION_PRIORITY_CEILING {
        rslvr_message_sentences(
            RSLVR_CODE_ASSOCIATION_PRIORITY_NOT_VALID,
            &[
                "The Priority you provided is not valid",
                &format!("Provide a number smaller than \"{ASSOCIATION_PRIORITY_CEILING}\""),
            ],
        )
    } else {
        rslvr_message_sentences(
            RSLVR_CODE_ASSOCIATION_PRIORITY_RESERVED,
            &[
                "The priority value you provided is reserved",
                &format!(
                    "Provide a number between \"{}\" and \"{}\"",
                    ASSOCIATION_PRIORITY_RANGE.start(),
                    ASSOCIATION_PRIORITY_RANGE.end()
                ),
            ],
        )
    };
    Some(ValidationException {
        message: Some(message),
    })
}

/// The Route 53 Resolver service error code for a DNS Firewall rule group a VPC
/// is still filtering its DNS traffic through.
pub const RSLVR_CODE_FIREWALL_RULE_GROUP_ASSOCIATED: &str = "RSLVR-02101";

/// The `ConflictException` `DeleteFirewallRuleGroup` is refused with while a VPC
/// is still associated with the group it names.
///
/// A rule group a VPC is associated with is still filtering that VPC's DNS
/// traffic, so deleting it is not a state transition the service makes: the
/// association goes first ("delete the association"), which is the teardown
/// order `DisassociateFirewallRuleGroup`/`DeleteFirewallRuleGroup` are meant to
/// be called in, and the model's `ConflictException` ("the requested state
/// transition isn't valid") is the answer for a transition it will not make.
///
/// The sentence names the rule group from the request and closes with the trace
/// clause of the request ([`traced_message`]), as the refusals of a DNS Firewall
/// resource another one still holds do.
pub fn firewall_rule_group_associated_conflict(
    firewall_rule_group_id: &str,
    trace_root: &str,
) -> ConflictException {
    ConflictException {
        message: Some(traced_message(
            rslvr_message_sentences(
                RSLVR_CODE_FIREWALL_RULE_GROUP_ASSOCIATED,
                &[
                    &format!(
                        "The DNS Firewall rule group is associated to a VPC: \
                         \"{firewall_rule_group_id}\""
                    ),
                    "Delete the association and try again later",
                ],
            ),
            trace_root,
        )),
    }
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Whether the store holds an association that puts
    /// `firewall_rule_group_id` on a VPC.
    ///
    /// This is the index `DeleteFirewallRuleGroup` consults before it takes a
    /// group out ([`firewall_rule_group_associated_conflict`]): a group an
    /// association points at cannot be deleted, so no association is ever left
    /// naming a rule group the store no longer holds. The refusal names the
    /// group from the request and no association, so the answer is the question
    /// itself and not a record. The scan is over a store bounded by
    /// [`MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS`].
    pub fn firewall_rule_group_is_associated(&self, firewall_rule_group_id: &str) -> bool {
        self.firewall_rule_group_associations
            .values()
            .any(|record| record.firewall_rule_group_id == firewall_rule_group_id)
    }
}
