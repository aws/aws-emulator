// SPDX-License-Identifier: CC0-1.0

//! The ORDER SNS reports one subscription's attributes in.
//!
//! The attribute map of `GetSubscriptionAttributes` is an entry LIST on the wire (the
//! awsQuery protocol serializes a map as `<entry><key/><value/></entry>` elements), so
//! the sequence of the entries is part of the response a client reads — positionally, or
//! by diffing two reads of one subscription. SNS answers the same attribute set in the
//! same sequence every time, and that sequence is neither the order a caller set the
//! attributes in nor an alphabetical one.
//!
//! This module is that sequence, and the map key that carries it.

use super::{
    CONFIRMATION_WAS_AUTHENTICATED_ATTRIBUTE, EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE,
    ENDPOINT_ATTRIBUTE, OWNER_ATTRIBUTE, PENDING_CONFIRMATION_ATTRIBUTE, PROTOCOL_ATTRIBUTE,
    RAW_MESSAGE_DELIVERY_ATTRIBUTE, SUBSCRIPTION_ARN_ATTRIBUTE,
    SUBSCRIPTION_PRINCIPAL_ATTRIBUTE, TOPIC_ARN_ATTRIBUTE,
};

/// The order SNS reports a subscription's attributes in, as recorded
/// `GetSubscriptionAttributes` responses carry it: the subscribing principal, the owner,
/// the raw-delivery flag, the endpoint, the topic, the effective delivery policy, the
/// protocol, the confirmation state, whether the confirmation was authenticated, and the
/// subscription's own ARN. Every other attribute a subscription holds — a filter, delivery,
/// redrive or replay policy its owner set — follows these, ordered by name.
///
/// Recorded anchor: the `EffectiveDeliveryPolicy` document of an http/https subscription
/// whose map carries exactly these ten attributes is the SIXTH entry of the entry list.
///
/// This is the subscription counterpart of [`super::REPORTED_TOPIC_ATTRIBUTE_ORDER`], and
/// it is carried the same way: the reported map is keyed by [`SubscriptionAttributeName`],
/// whose ordering IS this order, rather than by a `String` (whose `BTreeMap` would report
/// the entries alphabetically and whose `HashMap` would report them differently on every
/// call).
pub const REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER: &[&str] = &[
    SUBSCRIPTION_PRINCIPAL_ATTRIBUTE,
    OWNER_ATTRIBUTE,
    RAW_MESSAGE_DELIVERY_ATTRIBUTE,
    ENDPOINT_ATTRIBUTE,
    TOPIC_ARN_ATTRIBUTE,
    EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE,
    PROTOCOL_ATTRIBUTE,
    PENDING_CONFIRMATION_ATTRIBUTE,
    CONFIRMATION_WAS_AUTHENTICATED_ATTRIBUTE,
    SUBSCRIPTION_ARN_ATTRIBUTE,
];

/// A subscription attribute name as the key of a reported attribute map: a string on the
/// wire, ordered by [`REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER`] rather than alphabetically,
/// so a `BTreeMap` keyed by it serializes its entries in the order SNS reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionAttributeName(String);

impl SubscriptionAttributeName {
    /// Where SNS reports this attribute: its place in
    /// [`REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER`], or past the end for a name that order
    /// does not list (which then sorts by name).
    fn reported_position(&self) -> usize {
        REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER
            .iter()
            .position(|name| *name == self.0)
            .unwrap_or(REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER.len())
    }
}

impl Ord for SubscriptionAttributeName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.reported_position()
            .cmp(&other.reported_position())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for SubscriptionAttributeName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<str> for SubscriptionAttributeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SubscriptionAttributeName {
    fn from(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl From<String> for SubscriptionAttributeName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

impl std::str::FromStr for SubscriptionAttributeName {
    type Err = std::convert::Infallible;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self(name.to_string()))
    }
}
