// SPDX-License-Identifier: CC0-1.0

//! A subscription's delivery policy AS STORED: the document the caller wrote, expanded to
//! the whole member set of the shape, with every member it did not supply carried as the
//! JSON `null` it is.
//!
//! "the JSON serialization of the subscription's delivery policy"
//! (`GetSubscriptionAttributesResponse$Attributes` doc) is what the SUBSCRIPTION holds, and
//! what it holds is only what its owner configured: a member nobody set has no value on the
//! subscription, so the stored document states it as `null` rather than as a number. The
//! account system defaults belong to the OTHER document — the effective policy
//! ([`super::effective_subscription_delivery_policy`]), "the effective delivery policy that
//! takes into account the topic delivery policy and account system defaults" — which is
//! where a delivery reads the value it actually runs under. Keeping the two apart is what
//! lets a client tell "unset" from "set to this value".
//!
//! Recorded evidence: a subscription written a `healthyRetryPolicy` naming three of its
//! members reads the attribute back with those three kept and `numMaxDelayRetries`,
//! `numNoDelayRetries`, `numMinDelayRetries` and `backoffFunction` all `null`, beside
//! `sicklyRetryPolicy`, `throttlePolicy` and `requestPolicy` as `null` and `guaranteed` as
//! `false`.

use super::*;

/// A subscription's delivery policy AS STORED: every section of the document, each one
/// `null` where its owner configured none.
///
/// Modelled as a struct rather than carried as a [`serde_json::Value`] for the reason
/// [`super::SubscriptionDeliveryPolicy`] is: a `Value` object serializes sorted by key,
/// which no SNS response is, so the members keep the service's own order. Members this
/// emulator gives no meaning to ride in `other`, so a document is reported WITH them rather
/// than silently shortened.
///
/// Each SECTION is the caller's own parsed shape ([`ConfiguredRetryPolicy`],
/// [`ConfiguredRequestPolicy`]), which already states that section's members as the options
/// they are under the same names: the stored document is the parsed document, so there is
/// one shape for it rather than a second copy to keep in step. What this adds is the shape
/// of the document AS A WHOLE — a section nobody wrote is `null`, and the delivery guarantee
/// is a flag rather than a section, so a policy that states none states the flag's own
/// default.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSubscriptionDeliveryPolicy {
    pub healthy_retry_policy: Option<ConfiguredRetryPolicy>,
    pub sickly_retry_policy: Option<serde_json::Value>,
    pub throttle_policy: Option<serde_json::Value>,
    pub request_policy: Option<ConfiguredRequestPolicy>,
    pub guaranteed: bool,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl StoredSubscriptionDeliveryPolicy {
    /// The document `configured` is stored as: each section the caller wrote, and `None`
    /// — the JSON `null` — for each one it left out.
    fn of(configured: ConfiguredSubscriptionDeliveryPolicy) -> Self {
        Self {
            healthy_retry_policy: configured.healthy_retry_policy,
            sickly_retry_policy: configured.sickly_retry_policy,
            throttle_policy: configured.throttle_policy,
            request_policy: configured.request_policy,
            guaranteed: configured.guaranteed.unwrap_or(false),
            other: configured.other,
        }
    }
}

/// The document a subscription's `DeliveryPolicy` is STORED and reported as: what the
/// caller wrote, expanded to the whole shape, with every member it did not supply stated
/// as `null`.
///
/// A value the parse cannot read is returned unchanged — the write path refuses one first
/// ([`super::subscription_delivery_policy_refusal`]), so this is only the fallback that
/// keeps a caller's bytes rather than dropping them.
pub fn normalized_subscription_delivery_policy(value: &str) -> String {
    let Ok(configured) = serde_json::from_str::<ConfiguredSubscriptionDeliveryPolicy>(value) else {
        return value.to_string();
    };
    let stored = StoredSubscriptionDeliveryPolicy::of(configured);
    serde_json::to_string(&stored).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the stored document states every section and every retry member, carrying what
    /// the request supplied and `null` for everything else. The rows differ along HOW MUCH of
    /// the document the request named, which is what the rule generalises over.
    #[test]
    fn an_unsupplied_member_is_stored_as_null() {
        // (case, the policy written, the document it is stored as)
        let rows: &[(&str, &str, &str)] = &[
            (
                "three retry members",
                r#"{"healthyRetryPolicy":{"numRetries":5,"minDelayTarget":1,
                   "maxDelayTarget":10}}"#,
                r#"{"healthyRetryPolicy":{"minDelayTarget":1,"maxDelayTarget":10,
                   "numRetries":5,"numMaxDelayRetries":null,"numNoDelayRetries":null,
                   "numMinDelayRetries":null,"backoffFunction":null},
                   "sicklyRetryPolicy":null,"throttlePolicy":null,"requestPolicy":null,
                   "guaranteed":false}"#,
            ),
            (
                "every retry member",
                r#"{"healthyRetryPolicy":{"minDelayTarget":5,"maxDelayTarget":600,
                   "numRetries":10,"numMaxDelayRetries":2,"numNoDelayRetries":1,
                   "numMinDelayRetries":3,"backoffFunction":"exponential"}}"#,
                r#"{"healthyRetryPolicy":{"minDelayTarget":5,"maxDelayTarget":600,
                   "numRetries":10,"numMaxDelayRetries":2,"numNoDelayRetries":1,
                   "numMinDelayRetries":3,"backoffFunction":"exponential"},
                   "sicklyRetryPolicy":null,"throttlePolicy":null,"requestPolicy":null,
                   "guaranteed":false}"#,
            ),
            (
                "no retry section at all",
                r#"{"throttlePolicy":{"maxReceivesPerSecond":2}}"#,
                r#"{"healthyRetryPolicy":null,"sicklyRetryPolicy":null,
                   "throttlePolicy":{"maxReceivesPerSecond":2},"requestPolicy":null,
                   "guaranteed":false}"#,
            ),
            (
                "an empty document",
                "{}",
                r#"{"healthyRetryPolicy":null,"sicklyRetryPolicy":null,
                   "throttlePolicy":null,"requestPolicy":null,"guaranteed":false}"#,
            ),
            (
                "the guarantee and a request policy",
                r#"{"guaranteed":true,"requestPolicy":{"headerContentType":"application/json"}}"#,
                r#"{"healthyRetryPolicy":null,"sicklyRetryPolicy":null,
                   "throttlePolicy":null,
                   "requestPolicy":{"headerContentType":"application/json"},
                   "guaranteed":true}"#,
            ),
            (
                "a member this service gives no meaning to",
                r#"{"healthyRetryPolicy":{"numRetries":1},"somethingElse":{"a":1}}"#,
                r#"{"healthyRetryPolicy":{"minDelayTarget":null,"maxDelayTarget":null,
                   "numRetries":1,"numMaxDelayRetries":null,"numNoDelayRetries":null,
                   "numMinDelayRetries":null,"backoffFunction":null},
                   "sicklyRetryPolicy":null,"throttlePolicy":null,"requestPolicy":null,
                   "guaranteed":false,"somethingElse":{"a":1}}"#,
            ),
        ];
        for (case, written, expected) in rows {
            let stored = normalized_subscription_delivery_policy(written);
            let stored: serde_json::Value = serde_json::from_str(&stored)
                .unwrap_or_else(|error| panic!("case {case:?}: {stored:?} is no document: {error}"));
            let expected: serde_json::Value =
                serde_json::from_str(expected).expect("the expected document is well formed");
            assert_eq!(
                stored, expected,
                "case {case:?}: expected the stored document {expected}, got {stored}"
            );
        }
    }

    /// RULE: the members keep SNS's own order, which a document sorted by key would not.
    #[test]
    fn the_stored_document_keeps_the_services_member_order() {
        let stored =
            normalized_subscription_delivery_policy(r#"{"healthyRetryPolicy":{"numRetries":1}}"#);
        let expected = concat!(
            r#"{"healthyRetryPolicy":{"minDelayTarget":null,"maxDelayTarget":null,"#,
            r#""numRetries":1,"numMaxDelayRetries":null,"numNoDelayRetries":null,"#,
            r#""numMinDelayRetries":null,"backoffFunction":null},"sicklyRetryPolicy":null,"#,
            r#""throttlePolicy":null,"requestPolicy":null,"guaranteed":false}"#
        );
        assert_eq!(
            stored, expected,
            "expected the document in SNS's own member order, got {stored}"
        );
    }
}
