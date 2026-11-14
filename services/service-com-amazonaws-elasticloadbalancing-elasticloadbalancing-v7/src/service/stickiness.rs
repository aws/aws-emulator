// SPDX-License-Identifier: CC0-1.0

//! The SESSION STICKINESS a listener's policies give a client: the cookie a load balancer
//! hands back naming the registered instance that was selected, and the pin a client
//! carries in it on every later request.
//!
//! `CreateLBCookieStickinessPolicy` "[g]enerates a stickiness policy with sticky session
//! lifetimes controlled by the lifetime of the browser (user-agent) or a specified
//! expiration period", and `CreateAppCookieStickinessPolicy` documents the mechanism both
//! kinds share: "the load balancer uses a special cookie, `AWSELB`, to map the session to
//! the instance". So the cookie's NAME is Elastic Load Balancing's own
//! ([`STICKINESS_COOKIE`]) and what it carries is which instance answered, which is why a
//! later request carrying it reaches that same instance.
//!
//! A policy governs a LISTENER, not the load balancer: `SetLoadBalancerPoliciesOfListener`
//! "[r]eplaces the current set of policies for the specified load balancer port", so
//! whether a request is pinned at all is read off the listener it reached
//! ([`listener_stickiness`]) — a listener carrying no stickiness policy mints no cookie,
//! however many policies the load balancer holds.
//!
//! The cookie carries a TOKEN of the chosen instance rather than its id
//! ([`instance_token`]): a later request is pinned by taking the token of each instance the
//! load balancer may send it to and comparing, so the pin needs nothing stored — serving
//! traffic writes nothing — and nothing of the account's own naming travels to the client.

use ::sha2::{Digest as _, Sha256};

use crate::state::{ListenerEntry, LoadBalancer, PolicyKind};

/// The cookie Elastic Load Balancing maps a sticky session to an instance with, from
/// `CreateAppCookieStickinessPolicy`'s own documentation: "the load balancer uses a special
/// cookie, `AWSELB`, to map the session to the instance".
pub const STICKINESS_COOKIE: &str = "AWSELB";

/// The header a client carries its cookies back under, lower-cased as HTTP names compare.
pub const COOKIE_HEADER: &str = "cookie";

/// The most cookies of one request's `Cookie` header the pin is looked for among.
///
/// The emulator's OWN bound: far above the cookies any client carries, and what stops the
/// work of finding the pin growing with the size of the header a request chooses to send.
const MAX_COOKIES_READ: usize = 64;

/// What the policies of one listener make of a request's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stickiness {
    /// The listener carries no stickiness policy: every request is distributed on its own.
    None,
    /// A duration-based policy (`CreateLBCookieStickinessPolicy`): the load balancer hands
    /// a cookie-less client the cookie naming the instance it selected, for
    /// `expiration_period_seconds` — 0 being the documented default, "the sticky session
    /// should last for the duration of the browser session".
    Duration { expiration_period_seconds: i64 },
    /// An application-controlled policy (`CreateAppCookieStickinessPolicy`): the pin a
    /// client already carries is honoured, and a NEW cookie is minted only when "the
    /// application response includes a new application cookie" — a response from the
    /// application itself, which this load balancer has no backend server to receive one
    /// from, so none is minted here.
    Application { cookie_name: String },
}

/// What `listener`'s own policies make of a request's session, read off the load balancer
/// that holds both.
///
/// `Listener$PolicyNames` names the policies of `record`, so a name the load balancer holds
/// no policy for governs nothing, and a policy that is not one of the two stickiness kinds
/// governs no session. The FIRST stickiness policy the listener names decides, in the order
/// the listener carries its policy names: a listener carrying two of them is one
/// `SetLoadBalancerPoliciesOfListener` wrote in that order.
pub fn listener_stickiness(record: &LoadBalancer, listener: &ListenerEntry) -> Stickiness {
    listener
        .policy_names
        .iter()
        .filter_map(|policy_name| record.policies.get(policy_name))
        .find_map(|policy| match &policy.kind {
            PolicyKind::LbCookieStickiness {
                cookie_expiration_period,
            } => Some(Stickiness::Duration {
                expiration_period_seconds: *cookie_expiration_period,
            }),
            PolicyKind::AppCookieStickiness { cookie_name } => Some(Stickiness::Application {
                cookie_name: cookie_name.clone(),
            }),
            PolicyKind::Other { .. } => None,
        })
        .unwrap_or(Stickiness::None)
}

/// The token the stickiness cookie carries for `instance_id` of `load_balancer_name`.
///
/// A digest of the load balancer's name and the instance's id, so the cookie names the
/// instance to THIS load balancer — the token of each instance it may send a request to is
/// taken and compared — and names nothing of the account's own resources to the client
/// holding it. One instance registered with two load balancers therefore carries a
/// different pin with each, as a session of one is no session of the other.
pub fn instance_token(load_balancer_name: &str, instance_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(load_balancer_name.as_bytes());
    hasher.update(b"/");
    hasher.update(instance_id.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// The token this request is pinned to, read off the `Cookie` header it carried: the value
/// of [`STICKINESS_COOKIE`], or `None` for a request carrying no such cookie.
pub fn pinned_token(cookie_header: Option<&str>) -> Option<&str> {
    cookie_header?
        .split(';')
        .take(MAX_COOKIES_READ)
        .filter_map(|cookie| cookie.split_once('='))
        .find(|(name, _)| name.trim() == STICKINESS_COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

/// The `Set-Cookie` field that pins a client to the instance `token` names.
///
/// It carries the pin for the whole load balancer's paths (`Path=/`) and, where the policy
/// states an expiration period, for that many seconds — "the time period, in seconds, after
/// which the cookie should be considered stale". A policy at the documented default of 0
/// states none, "which indicates that the sticky session should last for the duration of
/// the browser session", so the cookie carries no lifetime and the browser drops it with
/// the session.
pub fn set_cookie(token: &str, expiration_period_seconds: i64) -> String {
    let mut field = format!("{STICKINESS_COOKIE}={token}; Path=/");
    if expiration_period_seconds > 0 {
        field.push_str(&format!("; Max-Age={expiration_period_seconds}"));
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pin is read off the `Cookie` header whatever else it carries, and only under the
    /// load balancer's own cookie name.
    #[test]
    fn the_pin_is_the_value_of_the_load_balancer_s_own_cookie() {
        // The rows span what the reading generalises over: no header at all, another
        // cookie, the pin alone, the pin beside others, spacing around either side, a name
        // that merely starts the same, and a cookie carrying no value.
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("no cookie header", None, None),
            ("another cookie only", Some("session=abc"), None),
            ("the pin alone", Some("AWSELB=pinned"), Some("pinned")),
            (
                "the pin beside others",
                Some("a=1; AWSELB=pinned; b=2"),
                Some("pinned"),
            ),
            ("the pin with spaces", Some(" AWSELB = pinned "), Some("pinned")),
            ("a name that only starts the same", Some("AWSELBX=no"), None),
            ("a cookie with no value at all", Some("AWSELB"), None),
            ("an empty value", Some("AWSELB="), None),
        ];
        for (case, header, expected) in cases {
            let read = pinned_token(*header);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected the pin {expected:?}, got {read:?}"
            );
        }
    }

    /// The token names one instance of one load balancer: it is stable for the same pair,
    /// and differs as either side differs.
    #[test]
    fn the_token_names_one_instance_of_one_load_balancer() {
        let base = instance_token("web-elb", "i-0123456789abcdef0");
        assert_eq!(
            base,
            instance_token("web-elb", "i-0123456789abcdef0"),
            "case the same pair: the token must be stable, got {base}"
        );
        for (case, load_balancer, instance) in [
            ("another instance", "web-elb", "i-0123456789abcdef1"),
            ("another load balancer", "api-elb", "i-0123456789abcdef0"),
        ] {
            let other = instance_token(load_balancer, instance);
            assert_ne!(
                other, base,
                "case {case:?}: the token must name that pair alone, got {other}"
            );
        }
        assert!(
            !base.contains("i-0123456789abcdef0") && !base.contains("web-elb"),
            "case the token's content: nothing of the account's naming may travel in it, \
             got {base}"
        );
    }

    /// The cookie carries the pin for every path, and a lifetime only where the policy
    /// states an expiration period.
    #[test]
    fn the_cookie_carries_the_pin_and_the_period_the_policy_states() {
        // The rows span the periods `CookieExpirationPeriod` takes: the documented default
        // of 0 (the browser session), a short period, a long one, and a value below zero,
        // which states no lifetime either.
        let cases: &[(&str, i64, &str)] = &[
            ("the browser session", 0, "AWSELB=t; Path=/"),
            ("one minute", 60, "AWSELB=t; Path=/; Max-Age=60"),
            ("one day", 86_400, "AWSELB=t; Path=/; Max-Age=86400"),
            ("below zero", -1, "AWSELB=t; Path=/"),
        ];
        for (case, period, expected) in cases {
            let field = set_cookie("t", *period);
            assert_eq!(
                field, *expected,
                "case {case:?}: expected {expected:?}, got {field:?}"
            );
        }
    }
}
