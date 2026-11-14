// SPDX-License-Identifier: CC0-1.0

//! The SESSION TAGS a vend asks its web-identity assume for, and the condition keys that
//! request publishes to the assumed role's trust policy.
//!
//! An identity pool that maps a provider's user attributes to principal tags
//! (`SetPrincipalTagAttributeMap`) performs its exchange as a TAGGED assume: each mapped
//! attribute's value travels as a requested session tag, which is what a policy written with
//! `aws:PrincipalTag/<key>` reads for a request the vended credential later signs, and what
//! the ROLE's own trust policy decides with `aws:RequestTag/<key>` and `aws:TagKeys` at the
//! instant of the assume — the keys a tag-passing assume publishes about itself (AWS General
//! Reference, *Global condition context keys*). So a trust policy that admits only the
//! identities whose mapped value satisfies a condition refuses the others, and the pool's
//! mapping is what decides which.
//!
//! Where the VALUES come from, taken from the request and the stores and invented nowhere:
//!
//! * for a provider the pool holds a principal-tag attribute map for, the claim each tag key
//!   is mapped to, read out of the token that provider's `Logins` entry presents
//!   ([`super::presented_token_claims`]) — "`PrincipalTags` … enables you to reference user
//!   attributes in your IAM permissions policy" (`SetPrincipalTagAttributeMap`'s own doc), and
//!   a user-pool or OpenID Connect token carries those attributes as its claims;
//! * for a token THIS service minted, the `PrincipalTags` the issuing call asked for, which
//!   its issuance recorded ([`super::IssuedOpenIdToken::principal_tags`]) — the custom-provider
//!   flow, where the developer states the pairs itself rather than naming attributes of a
//!   token to read them from.
//!
//! A mapped attribute the presented token carries no claim for contributes no tag: the value
//! a session tag would carry is not established, and reporting an empty one would state a user
//! attribute the token never carried.

use super::*;

/// The condition-key FAMILY the tags an assume REQUESTS are named in, each member after the
/// tag key it carries: `aws:RequestTag/Department` holds the value the assume asks to attach
/// under the tag key `Department`. It is the key a role's trust policy gates a tagged assume
/// on, and it answers for the tags of THIS request rather than for those the caller already
/// carries.
pub const REQUEST_TAG_CONDITION_KEY_FAMILY: &str = "aws:RequestTag/";

/// The condition key carrying every tag KEY the assume requests, which is what a trust policy
/// that bounds WHICH tags may be passed is written over. Multi-valued, so a policy decides it
/// under one of IAM's set qualifiers.
pub const TAG_KEYS_CONDITION_KEY: &str = "aws:TagKeys";

/// The condition keys a TAGGED assume publishes beside the web-identity ones
/// ([`super::WEB_IDENTITY_PUBLISHED_CONDITION_KEYS`], which names these too): declared so a
/// clause over a tag the vend does NOT request is decided as absent rather than left
/// undecided.
pub const SESSION_TAG_PUBLISHED_CONDITION_KEYS: &[&str] =
    &[REQUEST_TAG_CONDITION_KEY_FAMILY, TAG_KEYS_CONDITION_KEY];

/// The condition keys `tags` publish about the assume requesting them: one
/// `aws:RequestTag/<key>` per tag, and one [`TAG_KEYS_CONDITION_KEY`] value per tag key.
///
/// Appended to the keys the exchange itself publishes
/// ([`super::web_identity_condition_context`]) so ONE context decides the trust policy: the
/// pool, the identity, the authentication methods, and the tags the assume asks to attach.
#[must_use]
pub fn requested_session_tag_conditions(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut context = Vec::with_capacity(tags.len() * 2);
    for (key, value) in tags {
        context.push((
            format!("{REQUEST_TAG_CONDITION_KEY_FAMILY}{key}"),
            value.clone(),
        ));
        context.push((TAG_KEYS_CONDITION_KEY.to_string(), key.clone()));
    }
    context
}

impl ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// The session tags a vend for `identity_pool_id` presenting `logins` asks its assume to
    /// attach, in tag-key order so one exchange publishes one context twice.
    ///
    /// The providers are considered in `Logins` provider-name order, so which provider
    /// contributes a tag key two of them map does not depend on the iteration order of the
    /// request's map; the first contributor of a key keeps it. See the module docs for where
    /// each value comes from. An exchange presenting no login, and one whose providers the
    /// pool maps no principal tags for, asks for none — an UNTAGGED assume, exactly as before
    /// any mapping was written.
    #[must_use]
    pub fn requested_session_tags(
        &self,
        identity_pool_id: &str,
        logins: &HashMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        let mut requested: BTreeMap<String, String> = BTreeMap::new();
        for (provider, token) in presented {
            for (tag_key, value) in self.provider_session_tags(identity_pool_id, provider, token) {
                requested.entry(tag_key).or_insert(value);
            }
        }
        requested.into_iter().collect()
    }

    /// What ONE `Logins` entry contributes to the requested session tags: the pool's mapping
    /// of that provider's attributes read against the token's claims, else the
    /// `PrincipalTags` the issuance of a token this service minted recorded.
    ///
    /// The pool's own mapping comes first: it is the configuration of the provider the login
    /// was made under, and a token this service minted carries the pairs its issuing call
    /// asked for only because no provider mapping states them.
    fn provider_session_tags(
        &self,
        identity_pool_id: &str,
        provider: &str,
        token: &str,
    ) -> Vec<(String, String)> {
        let key = (identity_pool_id.to_string(), provider.to_string());
        if let Some(mapping) = self.principal_tag_attribute_maps.get(&key) {
            let claims = presented_token_claims(token).unwrap_or_default();
            return mapping
                .principal_tags
                .iter()
                .filter_map(|(tag_key, attribute)| {
                    let value = claim_values(&claims, attribute).into_iter().next()?;
                    Some((tag_key.clone(), value))
                })
                .collect();
        }
        if provider != COGNITO_IDENTITY_LOGIN_PROVIDER {
            return Vec::new();
        }
        self.open_id_tokens
            .get(token)
            .and_then(|issued| issued.principal_tags.as_ref())
            .map(|principal_tags| {
                principal_tags
                    .iter()
                    .map(|(tag_key, value)| (tag_key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: what the requested tags are, and the context they publish.
    type ConditionRow = (&'static str, &'static [(&'static str, &'static str)], &'static [(&'static str, &'static str)]);

    /// The keys one tag publishes about the assume requesting it.
    #[test]
    fn a_requested_tag_publishes_its_own_key_and_the_tag_key_list() {
        // (case, the tags requested, the context they publish)
        let rows: &[ConditionRow] = &[
            ("no tags at all", &[], &[]),
            (
                "one tag",
                &[("Department", "eng")],
                &[
                    ("aws:RequestTag/Department", "eng"),
                    ("aws:TagKeys", "Department"),
                ],
            ),
            (
                "two tags",
                &[("Department", "eng"), ("Tenant", "acme")],
                &[
                    ("aws:RequestTag/Department", "eng"),
                    ("aws:TagKeys", "Department"),
                    ("aws:RequestTag/Tenant", "acme"),
                    ("aws:TagKeys", "Tenant"),
                ],
            ),
        ];
        for (case, tags, expected) in rows {
            let requested: Vec<(String, String)> = tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            let actual = requested_session_tag_conditions(&requested);
            let expected: Vec<(String, String)> = expected
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
