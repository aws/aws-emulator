// SPDX-License-Identifier: CC0-1.0

//! What a `ResourceARN` the tagging family addresses resolves to.
//!
//! The three ARN-addressed operations — `TagResource`, `UntagResource` and
//! `ListTagsForResource` — name the resource they act on by the scalable-target ARN this
//! service minted for it, so each of them has three conditions to tell apart, and each is a
//! different answer:
//!
//! * the ARN names a scalable target this Region holds right now, and the call is served;
//! * the ARN names a scalable-target IDENTITY of the endpoint serving the request — its own
//!   partition, account and Region, and an identifier of the form every scalable-target ARN
//!   carries — and no registered target answers to it: the resource the ARN names is reported
//!   ABSENT, whether this endpoint ever issued that ARN or not. What the caller asked about is a
//!   scalable target of this account and Region, and the answer is that there is none;
//! * the ARN names no such identity — another partition, account or Region, or an identifier the
//!   service's identifier space does not contain — so there is no resource for this endpoint to
//!   report on: the call is refused without the service speaking about a resource at all,
//!   disclosing nothing about whether one exists.
//!
//! What separates the last two is the ARN itself: which endpoint it belongs to, and whether its
//! last segment is an identifier of a scalable target
//! ([`crate::state::is_scalable_target_id`]) — never whether this process happened to mint it.
//! An endpoint answers for the scalable targets it holds NOW, so a target registered before a
//! restart, or deregistered since, is equally one it does not hold, and its ARN is answered like
//! any other identifier of this endpoint that nothing answers to. The live index is therefore
//! the only record consulted about EXISTENCE.

use super::{ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState as Store, TargetKey};

/// The service name every scalable-target ARN of this endpoint carries, which is this
/// service's own ARN namespace.
const ARN_SERVICE: &str = "application-autoscaling";

/// The resource-type prefix a scalable-target ARN carries, from the modelled `ResourceARN`
/// pattern (`^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$`) and
/// from this service's own mint (`super::scalable_target_arn`).
const ARN_RESOURCE_TYPE: &str = "scalable-target/";

/// What a `ResourceARN` of the tagging family resolves to at one Region's endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedTargetArn {
    /// A scalable target this Region holds, named by its identity.
    Registered(TargetKey),
    /// An ARN naming a scalable-target identity of this endpoint that no registered target
    /// answers to: the scalable target it names is absent.
    Absent,
    /// An ARN naming no scalable-target identity of this endpoint at all.
    NotServedHere,
}

/// Whether `resource_arn` names a scalable-target identity of the endpoint serving a request at
/// `region`: this emulator's partition and account, that Region, the scalable-target resource
/// type, and an identifier of the form a scalable target's own
/// ([`crate::state::is_scalable_target_id`]).
///
/// The prefix is built through the same ARN builder the mint uses
/// (`super::scalable_target_arn`), so the two cannot disagree about which ARNs this endpoint
/// issues.
pub fn names_an_identity_of_this_endpoint(resource_arn: &str, region: &str) -> bool {
    let prefix = ::emulator_server::emulator_arn(ARN_SERVICE, region, ARN_RESOURCE_TYPE);
    resource_arn
        .strip_prefix(&prefix)
        .is_some_and(super::is_scalable_target_id)
}

impl Store {
    /// What `resource_arn` names for a request served at `region`.
    ///
    /// The one resolution the whole tagging family shares, so the three operations cannot tell
    /// the same ARN apart in three ways. The live index DECIDES whether the call is served: an
    /// entry in it whose target the store still holds is the resource the caller asked about,
    /// and everything else is one of the two refusals — the scalable target reported absent for
    /// an ARN that names an identity of this endpoint, and the call refused outright for an ARN
    /// that names none ([`names_an_identity_of_this_endpoint`]).
    pub fn resolve_tagged_target_arn(&self, resource_arn: &str, region: &str) -> TaggedTargetArn {
        let registered = self
            .arn_index
            .get(resource_arn)
            .filter(|key| key.region == region && self.targets.contains_key(*key));
        if let Some(key) = registered {
            return TaggedTargetArn::Registered(key.clone());
        }
        if names_an_identity_of_this_endpoint(resource_arn, region) {
            TaggedTargetArn::Absent
        } else {
            TaggedTargetArn::NotServedHere
        }
    }

    /// Stop `arn` resolving: the target it was minted for is no longer registered, so the ARN
    /// names a scalable target this endpoint does not hold.
    pub fn retire_target_arn(&mut self, arn: &str) {
        self.arn_index.remove(arn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ARN of `region`, as this service's own minting shapes one: the identifier is the
    /// hexadecimal one every scalable-target ARN carries.
    fn arn(region: &str, id: usize) -> String {
        ::emulator_server::emulator_arn(
            ARN_SERVICE,
            region,
            &format!("{ARN_RESOURCE_TYPE}{id:036x}"),
        )
    }

    /// An ARN of this endpoint carrying an identifier of a scalable target is one it answers for
    /// — absent rather than unreachable — and an ARN of another Region, account or partition, or
    /// one whose last segment is in no form an identifier takes, is not.
    #[test]
    fn an_arn_of_this_endpoint_is_told_apart_from_one_of_another() {
        let region = "us-east-1";
        let mine = arn(region, 1);
        assert!(
            names_an_identity_of_this_endpoint(&mine, region),
            "case \"this endpoint's own ARN\": expected it to name an identity of this endpoint, \
             got {mine}"
        );
        let cases = [
            ("another Region", arn("eu-west-1", 1)),
            (
                "another account",
                mine.replace(::emulator_server::emulator_account_id(), "123456789012"),
            ),
            ("another partition", mine.replace("arn:aws:", "arn:aws-cn:")),
            (
                "another resource type",
                mine.replace(ARN_RESOURCE_TYPE, "scaling-policy/"),
            ),
            (
                "no resource id at all",
                ::emulator_server::emulator_arn(ARN_SERVICE, region, ARN_RESOURCE_TYPE),
            ),
            (
                "an id in no form an identifier takes",
                ::emulator_server::emulator_arn(
                    ARN_SERVICE,
                    region,
                    &format!("{ARN_RESOURCE_TYPE}00000000-0000-0000-0000-000000000000"),
                ),
            ),
        ];
        for (case, other) in cases {
            assert!(
                !names_an_identity_of_this_endpoint(&other, region),
                "case {case:?}: expected {other} not to name an identity of this endpoint"
            );
        }
    }
}
