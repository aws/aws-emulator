// SPDX-License-Identifier: CC0-1.0

//! The web identity tokens this emulator ITSELF issued: the registry the service that
//! REDEEMS one reads, written by the service that ISSUED it.
//!
//! A web identity token is a bearer of an identity, and the two halves of its life live in
//! two different services: Amazon Cognito Identity mints one (`GetOpenIdToken`,
//! `GetOpenIdTokenForDeveloperIdentity`), and STS is handed it back by a client that wants
//! credentials for it (`AssumeRoleWithWebIdentity`). The redeemer's whole question is
//! whether the token is one it can trust — against AWS that is answered by verifying the
//! token's signature with the key its issuer publishes at the issuer's own key-set
//! endpoint, which this emulator does not fetch, because it makes no request outside
//! itself.
//!
//! What it CAN answer is whether the token is one it issued, and that is what this registry
//! holds: one record per issued token, naming the issuer, the audience and the subject the
//! issuing call recorded, plus the window the issuance is valid in. So the registry is the
//! shared type the ManagementEventLog is — held once in the one engine both services reach,
//! rather than as a second copy in either service's own state, because a token recorded in
//! one place and looked for in another is a token nobody can redeem.
//!
//! No secret lives here: a token IS its own bearer credential, and a registry keyed BY the
//! token stores nothing about it the holder does not already have. It is bounded twice, as
//! every request-driven store in this tree is — by the expiry of each record and by
//! [`MAX_ISSUED_TOKENS`] — and both bounds are materialised on the WRITE path, so a read
//! never changes what a later read sees.

use std::collections::HashMap;
use std::time::SystemTime;

/// The most issuances this registry holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: every issuing call appends one record, so without
/// a ceiling a caller looping on an issuance would grow it without end. The issuance
/// closest to its own expiry is given up first, so the tokens a live workflow just minted
/// are the ones that stay redeemable.
pub const MAX_ISSUED_TOKENS: usize = 10_000;

/// What one issued web identity token stands for: the three claims a redeemer reports back
/// (`AssumeRoleWithWebIdentityResponse$Provider`, `$Audience`,
/// `$SubjectFromWebIdentityToken`), the authentication methods the issuance was made under,
/// the principal tags it carries, and the instant the issuance stops being valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedWebIdentity {
    /// The `iss` the token carries: the issuing authority, as the redeemer reports it.
    pub issuer: String,
    /// The `aud` the token carries: who the token was issued FOR.
    pub audience: String,
    /// The `sub` the token carries: the identity it names.
    pub subject: String,
    /// What authenticated the identity this token names, as the issuing call recorded it —
    /// the `amr` claim of the token (RFC 8176's authentication methods references). The
    /// redeemer publishes one condition key per method to the trust policy it decides
    /// against, so an issuer that records none publishes none and a clause written over
    /// that key stays undecided.
    pub authentication_methods: Vec<String>,
    /// When the issuance stops being valid, which is the token's own `exp`.
    pub expires_at: SystemTime,
    /// The PRINCIPAL TAGS this token carries, as the issuance recorded them, in tag-key
    /// order.
    ///
    /// An issuer that tags the identity its token names carries those pairs IN the token, so
    /// the redeemer attaches them to the session it mints as that session's tags — which is
    /// what `aws:PrincipalTag/<key>` answers for every request the vended credential signs.
    /// Empty for an issuance that recorded none, which is a token whose redemption attaches
    /// no tag at all.
    pub principal_tags: Vec<(String, String)>,
}

/// Every token this emulator issued and has not pruned, keyed by the token itself.
#[derive(Default)]
pub struct IssuedWebIdentityTokens {
    issued: HashMap<String, IssuedWebIdentity>,
}

/// Reports how many issuances are held and nothing else. The KEY of this registry is the
/// token, which is a bearer of the identity it names — so a derived rendering would put
/// every outstanding token into whatever formatted it, a log line included. The identities
/// are still readable through [`IssuedWebIdentityTokens::issued`] by a caller that already
/// holds the token.
impl ::std::fmt::Debug for IssuedWebIdentityTokens {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("IssuedWebIdentityTokens")
            .field("issued", &self.issued.len())
            .finish_non_exhaustive()
    }
}

impl IssuedWebIdentityTokens {
    /// Record that this emulator issued `token` for `issued`.
    ///
    /// Prunes first: a record the clock has already expired is dropped, which is owed at
    /// this instant whether or not anybody issued anything, and at the ceiling the record
    /// expiring soonest gives way.
    pub fn record(&mut self, token: String, issued: IssuedWebIdentity, now: SystemTime) {
        self.prune(now);
        while self.issued.len() >= MAX_ISSUED_TOKENS {
            let soonest = self
                .issued
                .iter()
                .min_by_key(|(_, held)| held.expires_at)
                .map(|(key, _)| key.clone());
            match soonest {
                Some(key) => {
                    self.issued.remove(&key);
                }
                None => break,
            }
        }
        self.issued.insert(token, issued);
    }

    /// What `token` was issued for, when this emulator issued it and the issuance has not
    /// expired at `now`. `None` for a token nothing here issued — which is every token
    /// some other issuer signed, and every issuance the clock has run out on.
    ///
    /// A READ: nothing is dropped here, so two redeemers asking about one token are
    /// answered alike.
    #[must_use]
    pub fn issued(&self, token: &str, now: SystemTime) -> Option<&IssuedWebIdentity> {
        self.issued
            .get(token)
            .filter(|held| now < held.expires_at)
    }

    /// How many issuances the registry holds, expired records included until the next
    /// write prunes them.
    #[must_use]
    pub fn len(&self) -> usize {
        self.issued.len()
    }

    /// Whether the registry holds no issuance at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.issued.is_empty()
    }

    /// Drop every issuance made for `audience` — the audience the issuing call recorded —
    /// because the thing that audience names no longer exists.
    ///
    /// The issuer owns that judgement: a token is evidence of an identity, and when the
    /// resource the token was issued FOR is deleted the token names nothing, so the
    /// redeemer must stop resolving it. Called by the issuing service's own delete path,
    /// which is what keeps this registry from outliving the records it mirrors.
    pub fn forget_audience(&mut self, audience: &str) {
        self.issued.retain(|_, held| held.audience != audience);
    }

    /// Drop every issuance made BY `issuer` — the issuing authority the issuance recorded —
    /// because that issuer no longer exists.
    ///
    /// The same judgement [`Self::forget_audience`] makes, one issuer rather than one
    /// audience: an issuer this emulator HOSTS goes away when the resource publishing it is
    /// deleted (an Amazon EKS cluster and the OIDC issuer it announced), and a token it
    /// signed can be verified against no key set afterwards — so the record that would still
    /// resolve it to a role goes with the issuer. The comparison is on the issuer as
    /// recorded, which is the `iss` the token itself carries.
    pub fn forget_issuer(&mut self, issuer: &str) {
        self.issued.retain(|_, held| held.issuer != issuer);
    }

    /// Drop every issuance made for `subject` — the identity the issuing call recorded —
    /// because that identity no longer exists.
    ///
    /// The same judgement [`Self::forget_audience`] makes, one identity rather than
    /// everything an audience holds: an issuing service that removes a single identity (it
    /// was deleted, or merged into another) gives up the tokens that named it, so the
    /// redeemer stops resolving them to a role.
    pub fn forget_subject(&mut self, subject: &str) {
        self.issued.retain(|_, held| held.subject != subject);
    }

    /// Drop what the clock has already made unredeemable.
    fn prune(&mut self, now: SystemTime) {
        self.issued.retain(|_, held| now < held.expires_at);
    }
}

#[cfg(test)]
mod tests {
    use super::{IssuedWebIdentity, IssuedWebIdentityTokens, MAX_ISSUED_TOKENS};
    use std::time::{Duration, SystemTime};

    fn issuance(subject: &str, expires_at: SystemTime) -> IssuedWebIdentity {
        IssuedWebIdentity {
            issuer: "https://cognito-identity.amazonaws.com".to_string(),
            audience: "us-east-1:pool".to_string(),
            subject: subject.to_string(),
            authentication_methods: vec!["authenticated".to_string()],
            expires_at,
            principal_tags: Vec::new(),
        }
    }

    /// A token this emulator issued is redeemable until its own expiry, and nothing else
    /// is.
    #[test]
    fn only_a_token_this_emulator_issued_and_has_not_expired_is_redeemable() {
        let mut registry = IssuedWebIdentityTokens::default();
        let now = SystemTime::now();
        registry.record(
            "live-token".to_string(),
            issuance("identity-1", now + Duration::from_secs(600)),
            now,
        );
        registry.record(
            "expiring-token".to_string(),
            issuance("identity-2", now + Duration::from_secs(60)),
            now,
        );

        // One row: the case, the token presented, the instant it is presented at, and the
        // subject the registry answers (`None` for a token it cannot redeem).
        let rows: Vec<(&str, &str, SystemTime, Option<&str>)> = vec![
            ("the token as issued", "live-token", now, Some("identity-1")),
            (
                "the same token inside its window",
                "live-token",
                now + Duration::from_secs(599),
                Some("identity-1"),
            ),
            (
                "the same token at its expiry",
                "live-token",
                now + Duration::from_secs(600),
                None,
            ),
            (
                "another issuance, past its own shorter window",
                "expiring-token",
                now + Duration::from_secs(61),
                None,
            ),
            ("a token nothing here issued", "forged-token", now, None),
            ("a token one character off an issued one", "live-tokeN", now, None),
        ];

        for (case, token, at, expected) in rows {
            let actual = registry.issued(token, at).map(|held| held.subject.as_str());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the subject {expected:?}, got {actual:?}"
            );
        }
    }

    /// A read reports the same answer twice: redeeming is not consuming. An issuance the
    /// ISSUER gives up, because the thing its audience names is gone, stops resolving.
    #[test]
    fn redeeming_a_token_does_not_consume_it() {
        let mut registry = IssuedWebIdentityTokens::default();
        let now = SystemTime::now();
        registry.record(
            "token".to_string(),
            issuance("identity-1", now + Duration::from_secs(600)),
            now,
        );
        let first = registry.issued("token", now).cloned();
        let second = registry.issued("token", now).cloned();
        assert_eq!(
            first, second,
            "case two reads: expected one answer twice, got {first:?} then {second:?}"
        );
        assert_eq!(
            registry.len(),
            1,
            "case two reads: the registry must still hold the issuance"
        );
        registry.forget_audience("another-pool");
        assert!(
            registry.issued("token", now).is_some(),
            "case another audience: an issuance for a different audience must stay"
        );
        registry.forget_audience(&issuance("identity-1", now).audience);
        assert_eq!(
            registry.issued("token", now),
            None,
            "case its own audience: an issuance the issuer gave up must stop resolving"
        );
    }

    /// RULE: an issuance the ISSUER gives up for its SUBJECT stops resolving, and an
    /// issuance for any other subject does not.
    #[test]
    fn an_issuance_whose_subject_the_issuer_gave_up_stops_resolving() {
        let mut registry = IssuedWebIdentityTokens::default();
        let now = SystemTime::now();
        // (the token, the subject it was issued for)
        let rows: Vec<(&str, &str)> = vec![
            ("merged-away", "identity-1"),
            ("still-there", "identity-2"),
            ("also-merged-away", "identity-1"),
        ];
        for (token, subject) in &rows {
            registry.record(
                (*token).to_string(),
                issuance(subject, now + Duration::from_secs(600)),
                now,
            );
        }

        registry.forget_subject("identity-1");
        for (token, subject) in &rows {
            let resolves = registry.issued(token, now).is_some();
            let expected = *subject != "identity-1";
            assert_eq!(
                resolves, expected,
                "case {token:?} issued for {subject:?}: expected resolvable == {expected}, got \
                 {resolves}"
            );
        }
    }

    /// RULE: an issuance whose ISSUER no longer exists stops resolving, and an issuance of
    /// any other issuer does not.
    #[test]
    fn an_issuance_of_an_issuer_that_no_longer_exists_stops_resolving() {
        let mut registry = IssuedWebIdentityTokens::default();
        let now = SystemTime::now();
        // (the token, the issuer that minted it)
        let rows: Vec<(&str, &str)> = vec![
            ("of-the-gone-issuer", "https://127.0.0.1:9297/id/AAAA"),
            ("of-another-issuer", "https://127.0.0.1:9297/id/BBBB"),
            ("of-the-gone-issuer-again", "https://127.0.0.1:9297/id/AAAA"),
            ("of-a-third-party", "https://accounts.google.com"),
        ];
        for (token, issuer) in &rows {
            let mut held = issuance("identity-1", now + Duration::from_secs(600));
            held.issuer = (*issuer).to_string();
            registry.record((*token).to_string(), held, now);
        }

        registry.forget_issuer("https://127.0.0.1:9297/id/AAAA");
        for (token, issuer) in &rows {
            let resolves = registry.issued(token, now).is_some();
            let expected = *issuer != "https://127.0.0.1:9297/id/AAAA";
            assert_eq!(
                resolves, expected,
                "case {token:?} issued by {issuer:?}: expected resolvable == {expected}, got \
                 {resolves}"
            );
        }
    }

    /// The write path materialises both bounds: an expired record is dropped, and the
    /// registry never grows past its ceiling.
    #[test]
    fn the_write_path_prunes_expiries_and_holds_the_ceiling() {
        let mut registry = IssuedWebIdentityTokens::default();
        let now = SystemTime::now();
        registry.record(
            "already-expired".to_string(),
            issuance("identity-0", now - Duration::from_secs(1)),
            now - Duration::from_secs(600),
        );
        assert_eq!(
            registry.len(),
            1,
            "case nothing due yet: the record was written at an instant it was still live"
        );
        registry.record(
            "live".to_string(),
            issuance("identity-1", now + Duration::from_secs(600)),
            now,
        );
        assert_eq!(
            registry.issued("already-expired", now),
            None,
            "case expired: the next write must drop it"
        );

        for index in 0..MAX_ISSUED_TOKENS {
            registry.record(
                format!("token-{index}"),
                issuance(
                    "identity-n",
                    now + Duration::from_secs(600 + u64::try_from(index).unwrap_or(0)),
                ),
                now,
            );
        }
        assert!(
            registry.len() <= MAX_ISSUED_TOKENS,
            "case at the ceiling: expected at most {MAX_ISSUED_TOKENS} records, got {}",
            registry.len()
        );
        assert!(
            !registry.is_empty(),
            "case at the ceiling: the registry must still hold the newest issuances"
        );
    }
}
