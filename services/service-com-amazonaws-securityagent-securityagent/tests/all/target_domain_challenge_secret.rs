// SPDX-License-Identifier: CC0-1.0

//! The ownership-verification CHALLENGE a target domain's owner is told to publish —
//! `verificationDetails.dnsTxt.token` and `verificationDetails.httpRoute.token` — driven
//! end to end over the wire through the real `MultiServiceRouter` the emulator boots
//! (see `crate::support`), so nothing here passes against a handler answering a canned
//! body.
//!
//! RULE: the challenge is a SECRET the service DRAWS for the domain out of its own
//! entropy, never a value the request decides. Within one account it is fixed by the
//! registered domain name alone — the member it is published through and however many
//! other resources the mint drew for in between leave it as it was, and each name has its
//! own — while two independent MINTS driven through byte-identical calls in the same
//! order are told to publish DIFFERENT challenges for one domain name. A proof of control
//! that could be computed from public information (the domain name, the account, the order
//! of the calls) would be no proof at all, which is why the value is drawn once and then
//! held rather than derived from what the caller sent.
//!
//! `crate::target_domain_registration` owns the stability half of that rule (one domain,
//! one challenge, across its registrations with either method, the tags beside them, the
//! spellings that name it, its reads and the deletion of them), and
//! `crate::minted_identifiers` owns that no two resources of an account are handed one
//! drawn value. What these cases add is the other half, over the SHAPE of the domain name:
//! the value is the ACCOUNT's own draw, so nothing a caller sends — and nothing a third
//! party can see — decides it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    VERIFICATION_METHOD_DNS_TXT, VERIFICATION_METHOD_HTTP_ROUTE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{emulator, member, ok};

/// How many characters a challenge carries, whatever the name it is the challenge for.
///
/// `HttpVerification.token` and `DnsVerification.token` are unconstrained `String`s in
/// the model (`smithy_query(query="shape",shape="HttpVerification")`), so the form is the
/// service's own; what a case may assert about it is that the form the service hands out
/// is opaque hex of ONE length for every domain name, which is what makes it impossible
/// for the value to carry the name or the size of it.
const CHALLENGE_LEN: usize = 64;

/// The domain names the cases span: the shape of the name is what the rule generalises
/// over, so the rows carry an apex domain, a deeper name, a punycode IDN label, digits
/// and hyphens, a mixed-case spelling and a name far longer than the rest.
fn domain_rows() -> Vec<(&'static str, String)> {
    vec![
        ("an apex domain", "challenge-apex.example".to_string()),
        (
            "a deeper name",
            "api.eu.challenge.example.co.uk".to_string(),
        ),
        (
            "a punycode IDN label",
            "xn--80ak6aa92e.challenge.example".to_string(),
        ),
        (
            "digits and hyphens in the labels",
            "77-api.challenge-two.example".to_string(),
        ),
        (
            "a mixed-case spelling",
            "ChAllenge.Example.COM".to_string(),
        ),
        (
            "a name far longer than the others",
            format!(
                "{}.{}.challenge.example",
                "q".repeat(60),
                "a-very-long-second-label".repeat(2)
            ),
        ),
    ]
}

/// The verification methods that publish a challenge at all, with the member each one
/// publishes it under (`smithy_query(query="shape",shape="VerificationDetails")`:
/// `dnsTxt` and `httpRoute`). `PRIVATE_VPC`, the third member of
/// `DomainVerificationMethod`, is proven by reaching the domain from the VPC and
/// publishes nothing, so it has no challenge for these cases to assert on.
fn published_challenges() -> [(&'static str, &'static str); 2] {
    [
        (VERIFICATION_METHOD_DNS_TXT, "dnsTxt"),
        (VERIFICATION_METHOD_HTTP_ROUTE, "httpRoute"),
    ]
}

/// `CreateTargetDomain` for `domain` with `method`, requiring HTTP 200.
async fn register(
    router: &MultiServiceRouter,
    domain: &str,
    method: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": method}),
        case,
    )
    .await
}

/// The challenge a registration reports, read out of the member its verification method
/// publishes, with the form every challenge carries asserted on the way through.
fn challenge(payload: &Value, method: &str, published_under: &str, case: &str) -> String {
    let details = payload.get("verificationDetails").unwrap_or_else(|| {
        panic!("case {case:?}: the registration reports no verificationDetails: {payload}")
    });
    let published = details.get(published_under).unwrap_or_else(|| {
        panic!(
            "case {case:?}: a {method} registration publishes its challenge under \
             {published_under}, got {details}"
        )
    });
    let token = member(published, "token").unwrap_or_else(|| {
        panic!("case {case:?}: the challenge carries no token to publish: {published}")
    });
    assert_eq!(
        token.len(),
        CHALLENGE_LEN,
        "case {case:?}: expected a {CHALLENGE_LEN}-character challenge for every domain \
         name, got {token:?}"
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected an opaque lowercase hex challenge, got {token:?}"
    );
    token
}

/// Registrations of names no case asserts on, so the account's mint has drawn for other
/// resources between two registrations of the name under test: a challenge that moved
/// because of them would be the ordinal of the call rather than the domain's own.
async fn register_other_domains(router: &MultiServiceRouter, round: usize, case: &str) {
    for index in 0..3 {
        register(
            router,
            &format!("filler-{round}-{index}.example"),
            VERIFICATION_METHOD_DNS_TXT,
            case,
        )
        .await;
    }
}

/// RULE, within one account: the challenge belongs to the registered domain NAME and to
/// nothing else about the call — the member it is published through and the draws the mint
/// made for other resources in between leave it as it was — and no two names share one.
/// (That the tags and the SPELLING of the name leave it alone too is the half
/// `crate::target_domain_registration` owns.)
#[tokio::test]
async fn an_accounts_challenge_for_a_domain_is_fixed_by_the_domain_alone() {
    let router = emulator();
    let mut held: Vec<(String, String)> = Vec::new();

    for (round, (domain_case, domain)) in domain_rows().into_iter().enumerate() {
        let mut challenges: BTreeSet<String> = BTreeSet::new();
        for (method, published_under) in published_challenges() {
            let case = format!("{domain_case}, {method}");
            let created = register(&router, &domain, method, &case).await;
            challenges.insert(challenge(&created, method, published_under, &case));
            // The next registration of this name draws after the mint has handed values
            // to other resources of the account.
            register_other_domains(&router, round, &case).await;
            // ...and the name registered again, after those draws, keeps the challenge it
            // already has.
            let again = register(&router, &domain, method, &case).await;
            challenges.insert(challenge(&again, method, published_under, &case));
        }

        assert_eq!(
            challenges.len(),
            1,
            "case {domain_case:?}: {domain:?} has ONE challenge however it is registered, \
             got {challenges:?}"
        );
        let challenge = challenges
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {domain_case:?}: the one challenge is missing"));
        for (other_case, other) in &held {
            assert_ne!(
                &challenge, other,
                "case {domain_case:?}: {domain:?} is told to publish the challenge \
                 {challenge:?} that {other_case} already holds"
            );
        }
        held.push((domain_case.to_string(), challenge));
    }

    assert_eq!(
        held.len(),
        domain_rows().len(),
        "every domain row must have been registered, got {held:?}"
    );
}

/// RULE, across independent MINTS: the challenge is the store's own draw, so two stores
/// driven through byte-identical calls in the same order are told to publish different
/// challenges for one domain name — which is what a value computed from the domain name,
/// from the account's identity or from the order of the calls could not be, and what
/// makes the challenge a proof of control rather than public information.
#[tokio::test]
async fn independent_mints_are_told_to_publish_different_challenges_for_one_domain() {
    // Three independent mints: a fresh store each behind the same wire surface, driven
    // through the same table in the same order.
    let mints = [
        ("the first mint", emulator()),
        ("a second mint", emulator()),
        ("a third mint", emulator()),
    ];

    // What each mint's store was told to publish, for every row of the table.
    let mut per_mint: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    for (mint_case, router) in &mints {
        let mut told: Vec<(String, String)> = Vec::new();
        for (domain_case, domain) in domain_rows() {
            for (method, published_under) in published_challenges() {
                let case = format!("{domain_case}, {method}, {mint_case}");
                let created = register(router, &domain, method, &case).await;
                told.push((
                    format!("{domain_case} ({method})"),
                    challenge(&created, method, published_under, &case),
                ));
            }
        }
        per_mint.push((mint_case, told));
    }

    // Row for row, no two mints agree on the challenge one domain name must publish.
    for (index, (mint_case, told)) in per_mint.iter().enumerate() {
        for (other_case, other_told) in per_mint.iter().skip(index + 1) {
            assert_eq!(
                told.len(),
                other_told.len(),
                "the mints must have been driven through the same table, got {} rows \
                 for {mint_case} and {} for {other_case}",
                told.len(),
                other_told.len()
            );
            for ((row, value), (other_row, other_value)) in told.iter().zip(other_told) {
                assert_eq!(
                    row, other_row,
                    "the two walks of the table must line up, got {row:?} against \
                     {other_row:?}"
                );
                assert_ne!(
                    value, other_value,
                    "case {row:?}: {mint_case} and {other_case} are both told to \
                     publish {value:?}, so the challenge is public information rather \
                     than the store's own draw"
                );
            }
        }
    }

    // And no mint holds a challenge another mint holds, for any name: within one store the
    // two methods of one domain report that domain's one challenge, which is why the names
    // are counted per mint rather than across them.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (mint_case, told) in &per_mint {
        let mine: BTreeSet<&str> = told.iter().map(|(_, value)| value.as_str()).collect();
        for value in &mine {
            assert!(
                seen.insert(value),
                "case {mint_case:?}: the challenge {value:?} is already another mint's"
            );
        }
    }
}
