// SPDX-License-Identifier: CC0-1.0

//! What a pre-token-generation function's answer changes about the tokens that generation
//! mints.
//!
//! A trigger of this service can "modify the outcome of the operations that invoked them"
//! (`LambdaConfigType`, this service's model), and the outcome a pre-token-generation
//! trigger precedes is the token triple itself: the function answers with the claims, the
//! scopes and the group configuration the tokens are to carry, and Amazon Cognito mints them
//! with exactly that. So the answer is not decoration the minting may ignore — it is an
//! input to it, which is what this module turns the answered event into
//! ([`TokenOverrides::read`]) and what [`crate::state`]'s token minting applies.
//!
//! The answer carries the overrides under the member of the trigger CONTRACT it was written
//! for, and that member states which tokens it customises:
//!
//! * `response.claimsOverrideDetails`, the first contract, customises the identity token's
//!   own claims: `claimsToAddOrOverride` and `claimsToSuppress` reach that token alone, with
//!   `groupOverrideDetails` beside them.
//! * `response.claimsAndScopeOverrideDetails`, the second, customises each token's claims
//!   separately and the access token's scopes: `idTokenGeneration` and
//!   `accessTokenGeneration`, each with their own `claimsToAddOrOverride` and
//!   `claimsToSuppress`, the access token's `scopesToAdd` and `scopesToSuppress`, and the
//!   same `groupOverrideDetails` beside them.
//!
//! `groupOverrideDetails` is the member BOTH contracts share, and it is not one token's: the
//! group configuration is the user's membership, so the replacement reaches the
//! `cognito:groups` claim of both tokens, exactly as a stored membership does
//! (`AdminAddUserToGroup`: the claim is "to their access and identity tokens"), while the
//! `cognito:roles` and `cognito:preferred_role` claims it also replaces exist in the identity
//! token alone.
//!
//! Three rules the whole contract follows, whichever member carried it:
//!
//! * the claims that identify a token, its subject and its lifetime are RESERVED
//!   ([`RESERVED_CLAIMS`]): a function cannot override or suppress one, so a token's
//!   identity is never the function's to rewrite;
//! * a claim named in both `claimsToAddOrOverride` and `claimsToSuppress` is ADDED, so a
//!   suppression that the same answer also gives a value for is ignored;
//! * `groupOverrideDetails` REPLACES the group configuration the event reported: the
//!   groups, the roles and the preferred role become the ones it names, a member it does
//!   not name is suppressed, and an answer carrying no such object at all leaves the stored
//!   configuration standing.
//!
//! An answer is a function's OUTPUT, so every list and map it carries is caller-driven. Each
//! of them is therefore BOUNDED where it is read ([`MAX_OVERRIDE_ENTRIES`],
//! [`MAX_OVERRIDE_TEXT_BYTES`], [`MAX_CLAIM_VALUE_BYTES`]), which is before the minting
//! takes this service's state lock: what one answer can cost every other request of this
//! service is a constant, whatever the function returned.

use std::collections::HashSet;

use ::serde_json::{Map, Value};

use crate::state::GroupRoleClaims;

/// The half of an answered event the overrides live in: the response the function filled in.
const RESPONSE: &str = "response";

/// The first trigger contract's member, which customises the identity token.
const CLAIMS_OVERRIDE_DETAILS: &str = "claimsOverrideDetails";

/// The second trigger contract's member, which customises both tokens and the access
/// token's scopes.
const CLAIMS_AND_SCOPE_OVERRIDE_DETAILS: &str = "claimsAndScopeOverrideDetails";

/// The second contract's member for what the IDENTITY token carries.
const ID_TOKEN_GENERATION: &str = "idTokenGeneration";

/// The second contract's member for what the ACCESS token carries.
const ACCESS_TOKEN_GENERATION: &str = "accessTokenGeneration";

/// The claims a function gives values for.
const CLAIMS_TO_ADD_OR_OVERRIDE: &str = "claimsToAddOrOverride";

/// The claims a function takes out of the token.
const CLAIMS_TO_SUPPRESS: &str = "claimsToSuppress";

/// The scopes a function adds to the access token's own.
const SCOPES_TO_ADD: &str = "scopesToAdd";

/// The scopes a function takes out of the access token's.
const SCOPES_TO_SUPPRESS: &str = "scopesToSuppress";

/// The group configuration a function replaces the user's with.
const GROUP_OVERRIDE_DETAILS: &str = "groupOverrideDetails";

/// The group names that replace the user's memberships in the tokens' `cognito:groups`.
const GROUPS_TO_OVERRIDE: &str = "groupsToOverride";

/// The role ARNs that replace the identity token's `cognito:roles`.
const IAM_ROLES_TO_OVERRIDE: &str = "iamRolesToOverride";

/// The role that replaces the identity token's `cognito:preferred_role`.
const PREFERRED_ROLE: &str = "preferredRole";

/// The claims a pre-token-generation function may NOT give a value for or suppress, so an
/// answer naming one leaves the claim the minting computed standing. Two kinds, marked
/// below:
///
/// * the claims Amazon Cognito reserves for itself, which state a token's issuer, audience,
///   subject, lifetime and use, and which a verifier reads to tell one token of a sign-in
///   from another;
/// * the claims another member of the same contract OWNS, which is why they are not these
///   members' to write: the three group claims are [`GROUP_OVERRIDE_DETAILS`]'s
///   ([`TokenOverrides::group_claims`]) and `scope` is [`SCOPES_TO_ADD`] /
///   [`SCOPES_TO_SUPPRESS`]'s ([`TokenOverrides::access_scopes`]).
const RESERVED_CLAIMS: &[&str] = &[
    // Reserved by the service.
    "acr",
    "amr",
    "at_hash",
    "aud",
    "auth_time",
    "azp",
    "client_id",
    "cognito:username",
    "exp",
    "iat",
    "identities",
    "iss",
    "jti",
    "nbf",
    "nonce",
    "origin_jti",
    "sub",
    "token_use",
    "username",
    // Owned by another member of the contract.
    "cognito:groups",
    "cognito:preferred_role",
    "cognito:roles",
    "scope",
];

/// Whether `claim` is one Amazon Cognito reserves ([`RESERVED_CLAIMS`]).
fn is_reserved(claim: &str) -> bool {
    RESERVED_CLAIMS.contains(&claim)
}

/// The most entries this service reads from ONE member of an answer — a claim map, a claim
/// name list, a scope list, a group list or a role list. Entries past it are not read.
///
/// A bound rather than a modelled limit: the member is a function's own output, and what the
/// minting does with it runs under this service's state lock, so the number of entries one
/// answer can put through that work is capped here instead of being whatever the function
/// returned.
const MAX_OVERRIDE_ENTRIES: usize = 50;

/// The longest text this service reads as one entry of an answer: a claim name, a group
/// name, a role ARN or a scope. The longest of those the model bounds is an ARN, "min 20,
/// max 2048" (`smithy_query(query="shape",shape="ArnType")`), against `GroupNameType`'s 128
/// and `ScopeType`'s 256 — so a longer entry names none of them and is not read.
const MAX_OVERRIDE_TEXT_BYTES: usize = 2048;

/// The largest claim VALUE this service reads from an answer, as the JSON the function
/// stated it as. A claim is one statement about the user, and a token is a bearer credential
/// a client carries in a header, so a larger value is not read.
const MAX_CLAIM_VALUE_BYTES: usize = 4096;

/// How many bytes a claim value takes as the JSON the function answered it as.
fn claim_value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        stated => stated.to_string().len(),
    }
}

/// What a pre-token-generation function's answer changes about one token: the claims it
/// gives values for, and the claims it takes out.
#[derive(Debug, Clone, Default, PartialEq)]
struct ClaimOverrides {
    /// The claims the function gave values for, reserved names dropped. The value is the
    /// JSON the function answered with, so a claim it stated as a list or a number is
    /// carried as one.
    add_or_override: Map<String, Value>,
    /// The claims the function took out, reserved names dropped.
    suppress: Vec<String>,
}

impl ClaimOverrides {
    /// What `details` — one contract member's object — changes about its token.
    fn read(details: Option<&Value>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };
        let add_or_override = details
            .get(CLAIMS_TO_ADD_OR_OVERRIDE)
            .and_then(Value::as_object)
            .map(|members| {
                members
                    .iter()
                    .filter(|(name, _)| {
                        !is_reserved(name) && name.len() <= MAX_OVERRIDE_TEXT_BYTES
                    })
                    .filter(|(_, value)| claim_value_bytes(value) <= MAX_CLAIM_VALUE_BYTES)
                    .take(MAX_OVERRIDE_ENTRIES)
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let suppress = read_text_list(details.get(CLAIMS_TO_SUPPRESS))
            .into_iter()
            .filter(|name| !is_reserved(name))
            .collect();
        Self {
            add_or_override,
            suppress,
        }
    }

    /// Apply this answer to the `claims` the minting computed: a suppressed claim is taken
    /// out, and a claim the answer gives a value for carries that value — including one it
    /// also named for suppression, which is the suppression Amazon Cognito ignores.
    fn apply(&self, claims: &mut Value) {
        let Some(members) = claims.as_object_mut() else {
            return;
        };
        for name in &self.suppress {
            members.remove(name);
        }
        for (name, value) in &self.add_or_override {
            members.insert(name.clone(), value.clone());
        }
    }
}

/// The group configuration a function replaced the user's with: what the tokens'
/// `cognito:groups`, and the identity token's `cognito:roles` and `cognito:preferred_role`,
/// carry instead of the user's stored memberships.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GroupOverride {
    groups: Vec<String>,
    roles: Vec<String>,
    preferred_role: Option<String>,
}

/// What a pre-token-generation function's answer changes about the tokens the generation
/// that invoked it mints — and, for a generation no such function ran for, nothing
/// ([`TokenOverrides::none`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenOverrides {
    id_claims: ClaimOverrides,
    access_claims: ClaimOverrides,
    scopes_to_add: Vec<String>,
    scopes_to_suppress: Vec<String>,
    /// The replacement group configuration, and `None` where the answer carried no
    /// `groupOverrideDetails` at all — which asks for no change, so the user's stored
    /// memberships stand.
    group_override: Option<GroupOverride>,
}

impl TokenOverrides {
    /// What a generation that invoked no function changes: nothing, so every claim, scope
    /// and group claim is the one the stored state computes.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// What the function that answered `answered` asks the tokens to carry.
    ///
    /// The overrides are read from the contract member the answer actually filled in, since
    /// that member names the contract they are written for; an answer that filled in
    /// neither, or that is not a document at all, asks for no change.
    #[must_use]
    pub fn read(answered: &Value) -> Self {
        let Some(response) = answered.get(RESPONSE) else {
            return Self::none();
        };
        if let Some(details) = response.get(CLAIMS_AND_SCOPE_OVERRIDE_DETAILS) {
            let access = details.get(ACCESS_TOKEN_GENERATION);
            return Self {
                id_claims: ClaimOverrides::read(details.get(ID_TOKEN_GENERATION)),
                access_claims: ClaimOverrides::read(access),
                scopes_to_add: read_text_list(access.and_then(|access| access.get(SCOPES_TO_ADD))),
                scopes_to_suppress: read_text_list(
                    access.and_then(|access| access.get(SCOPES_TO_SUPPRESS)),
                ),
                group_override: group_override(details),
            };
        }
        match response.get(CLAIMS_OVERRIDE_DETAILS) {
            // The first contract customises the identity token's own claims, so the access
            // token of such a generation carries the claims the stored state computed for
            // it — bar the group configuration, which is both tokens' (see this module's
            // header).
            Some(details) => Self {
                id_claims: ClaimOverrides::read(Some(details)),
                access_claims: ClaimOverrides::default(),
                scopes_to_add: Vec::new(),
                scopes_to_suppress: Vec::new(),
                group_override: group_override(details),
            },
            None => Self::none(),
        }
    }

    /// The group configuration the minted tokens carry: the one the function replaced it
    /// with, or `stored` where it asked for no replacement.
    #[must_use]
    pub(crate) fn group_claims(
        &self,
        stored_groups: Vec<String>,
        stored_roles: GroupRoleClaims,
    ) -> (Vec<String>, GroupRoleClaims) {
        match &self.group_override {
            None => (stored_groups, stored_roles),
            Some(replacement) => (
                replacement.groups.clone(),
                GroupRoleClaims {
                    roles: replacement.roles.clone(),
                    preferred_role: replacement.preferred_role.clone(),
                },
            ),
        }
    }

    /// The scopes the minted access token is authorized for: the ones `granted` by the
    /// sign-in, plus the ones the function added, less the ones it suppressed. A scope the
    /// same answer both adds and suppresses is added, as a claim in both members is.
    ///
    /// Every membership question is answered against a SET, so the merge is linear in the
    /// scopes it carries — each list's entry count is bounded by [`MAX_OVERRIDE_ENTRIES`]
    /// already, and this keeps the work linear in them rather than quadratic.
    #[must_use]
    pub(crate) fn access_scopes(&self, granted: Vec<String>) -> Vec<String> {
        let added: HashSet<&str> = self.scopes_to_add.iter().map(String::as_str).collect();
        let suppressed: HashSet<&str> =
            self.scopes_to_suppress.iter().map(String::as_str).collect();
        // The grant's own scopes first, in the order it authorized them, less the ones the
        // answer suppressed without also giving a value for.
        let mut scopes: Vec<String> = granted
            .into_iter()
            .filter(|scope| {
                !suppressed.contains(scope.as_str()) || added.contains(scope.as_str())
            })
            .collect();
        // Then the ones the answer added that the list does not carry yet, in the order it
        // named them.
        let mut carried: HashSet<&str> = scopes.iter().map(String::as_str).collect();
        let fresh: Vec<String> = self
            .scopes_to_add
            .iter()
            .filter(|scope| carried.insert(scope.as_str()))
            .map(String::clone)
            .collect();
        scopes.extend(fresh);
        scopes
    }

    /// Apply what the function asked of the IDENTITY token to the claims the minting
    /// computed for it.
    pub(crate) fn apply_to_id_token(&self, claims: &mut Value) {
        self.id_claims.apply(claims);
    }

    /// Apply what the function asked of the ACCESS token to the claims the minting computed
    /// for it.
    pub(crate) fn apply_to_access_token(&self, claims: &mut Value) {
        self.access_claims.apply(claims);
    }
}

/// The group configuration `details` replaces the user's with, and `None` where it names no
/// `groupOverrideDetails` at all.
///
/// A member the object does not name contributes nothing, so an EMPTY object — and one the
/// answer stated as `null`, or as anything that is not an object — replaces the whole
/// configuration with no groups, no roles and no preferred role.
fn group_override(details: &Value) -> Option<GroupOverride> {
    let stated = details.get(GROUP_OVERRIDE_DETAILS)?;
    let Some(members) = stated.as_object() else {
        return Some(GroupOverride::default());
    };
    Some(GroupOverride {
        groups: read_text_list(members.get(GROUPS_TO_OVERRIDE)),
        roles: read_text_list(members.get(IAM_ROLES_TO_OVERRIDE)),
        preferred_role: members
            .get(PREFERRED_ROLE)
            .and_then(Value::as_str)
            .filter(|role| !role.is_empty())
            .map(str::to_string),
    })
}

/// The text values a member states as a list, in the order it stated them, bounded by
/// [`MAX_OVERRIDE_ENTRIES`] entries of [`MAX_OVERRIDE_TEXT_BYTES`] each. A member that is
/// absent, is not a list, or carries an entry that is not text, is too long, or is past the
/// entry bound contributes nothing for that entry.
fn read_text_list(stated: Option<&Value>) -> Vec<String> {
    stated
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .filter(|text| text.len() <= MAX_OVERRIDE_TEXT_BYTES)
                .take(MAX_OVERRIDE_ENTRIES)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim values an answer asked for, as this test reads them back.
    fn claims(members: &[(&str, Value)]) -> Map<String, Value> {
        members
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    /// Text values as a list of owned strings.
    fn texts(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// Both contracts are read from the member the answer filled in, and each member
    /// customises the tokens Amazon Cognito documents it for: the first the identity token
    /// alone, the second both tokens and the access token's scopes.
    #[test]
    fn each_contract_member_customises_the_tokens_it_states() {
        // (case, the response half the function answered with, expected overrides)
        let rows = [
            (
                "a second-contract answer overriding the groups",
                ::serde_json::json!({
                    "claimsAndScopeOverrideDetails": {
                        "groupOverrideDetails": { "groupsToOverride": ["one", "two"] },
                    },
                }),
                TokenOverrides {
                    group_override: Some(GroupOverride {
                        groups: texts(&["one", "two"]),
                        ..GroupOverride::default()
                    }),
                    ..TokenOverrides::none()
                },
            ),
            (
                "a second-contract answer customising both tokens and the scopes",
                ::serde_json::json!({
                    "claimsAndScopeOverrideDetails": {
                        "idTokenGeneration": {
                            "claimsToAddOrOverride": { "tenant": "acme" },
                            "claimsToSuppress": ["email"],
                        },
                        "accessTokenGeneration": {
                            "claimsToAddOrOverride": { "tier": "gold" },
                            "claimsToSuppress": ["custom:legacy"],
                            "scopesToAdd": ["store/read"],
                            "scopesToSuppress": ["store/write"],
                        },
                    },
                }),
                TokenOverrides {
                    id_claims: ClaimOverrides {
                        add_or_override: claims(&[("tenant", Value::from("acme"))]),
                        suppress: texts(&["email"]),
                    },
                    access_claims: ClaimOverrides {
                        add_or_override: claims(&[("tier", Value::from("gold"))]),
                        suppress: texts(&["custom:legacy"]),
                    },
                    scopes_to_add: texts(&["store/read"]),
                    scopes_to_suppress: texts(&["store/write"]),
                    group_override: None,
                },
            ),
            (
                "a first-contract answer, which customises the identity token alone",
                ::serde_json::json!({
                    "claimsOverrideDetails": {
                        "claimsToAddOrOverride": { "tenant": "acme" },
                        "claimsToSuppress": ["email"],
                        "groupOverrideDetails": {
                            "groupsToOverride": ["admins"],
                            "iamRolesToOverride": ["arn:aws:iam::000000000000:role/admins"],
                            "preferredRole": "arn:aws:iam::000000000000:role/admins",
                        },
                    },
                }),
                TokenOverrides {
                    id_claims: ClaimOverrides {
                        add_or_override: claims(&[("tenant", Value::from("acme"))]),
                        suppress: texts(&["email"]),
                    },
                    group_override: Some(GroupOverride {
                        groups: texts(&["admins"]),
                        roles: texts(&["arn:aws:iam::000000000000:role/admins"]),
                        preferred_role: Some(
                            "arn:aws:iam::000000000000:role/admins".to_string(),
                        ),
                    }),
                    ..TokenOverrides::none()
                },
            ),
            (
                "an answer whose group object is empty, which suppresses the groups",
                ::serde_json::json!({
                    "claimsAndScopeOverrideDetails": { "groupOverrideDetails": {} },
                }),
                TokenOverrides {
                    group_override: Some(GroupOverride::default()),
                    ..TokenOverrides::none()
                },
            ),
            (
                "an answer naming reserved claims, which are not a function's to write",
                ::serde_json::json!({
                    "claimsAndScopeOverrideDetails": {
                        "idTokenGeneration": {
                            "claimsToAddOrOverride": { "sub": "someone-else", "cognito:groups": ["x"] },
                            "claimsToSuppress": ["exp", "token_use"],
                        },
                    },
                }),
                TokenOverrides::none(),
            ),
            (
                "an answer that filled in neither contract member",
                ::serde_json::json!({}),
                TokenOverrides::none(),
            ),
        ];

        for (case, response, expected) in rows {
            let answered = ::serde_json::json!({ RESPONSE: response });
            let read = TokenOverrides::read(&answered);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// One row of the scope table: what the answer adds, what it suppresses, and the
    /// scopes the minted access token must then be authorized for.
    struct ScopeRow {
        case: &'static str,
        add: &'static [&'static str],
        suppress: &'static [&'static str],
        expected: &'static [&'static str],
    }

    /// A scope list is the grant's, plus what the answer added, less what it suppressed —
    /// and a scope in both members is added, as a claim in both is.
    #[test]
    fn the_access_scopes_are_the_grants_plus_added_less_suppressed() {
        let rows = [
            ScopeRow {
                case: "an answer changing no scope",
                add: &[],
                suppress: &[],
                expected: &["granted"],
            },
            ScopeRow {
                case: "an answer adding one",
                add: &["store/read"],
                suppress: &[],
                expected: &["granted", "store/read"],
            },
            ScopeRow {
                case: "an answer suppressing the grant's own",
                add: &[],
                suppress: &["granted"],
                expected: &[],
            },
            ScopeRow {
                case: "an answer adding one it already carries",
                add: &["granted"],
                suppress: &[],
                expected: &["granted"],
            },
            ScopeRow {
                case: "an answer that both adds and suppresses one",
                add: &["store/read"],
                suppress: &["store/read"],
                expected: &["granted", "store/read"],
            },
        ];

        for row in rows {
            let overrides = TokenOverrides {
                scopes_to_add: texts(row.add),
                scopes_to_suppress: texts(row.suppress),
                ..TokenOverrides::none()
            };
            let scopes = overrides.access_scopes(texts(&["granted"]));
            assert_eq!(
                scopes,
                texts(row.expected),
                "case {:?}: expected {:?}, got {scopes:?}",
                row.case,
                row.expected
            );
        }
    }

    /// Applying an answer to a token's claims: a suppressed claim goes, an overridden one
    /// carries the function's own JSON, and a claim in both members is the added one.
    #[test]
    fn applying_an_answer_rewrites_only_the_claims_it_names() {
        let overrides = TokenOverrides::read(&::serde_json::json!({
            RESPONSE: {
                "claimsAndScopeOverrideDetails": {
                    "idTokenGeneration": {
                        "claimsToAddOrOverride": {
                            "tenant": "acme",
                            "seats": 3,
                            "email": "override@example.test",
                        },
                        "claimsToSuppress": ["email", "nickname", "sub"],
                    },
                },
            },
        }));
        let mut claims = ::serde_json::json!({
            "sub": "the-users-own-id",
            "email": "stored@example.test",
            "nickname": "stored",
        });
        overrides.apply_to_id_token(&mut claims);

        // (case, claim, expected value)
        let expected = [
            (
                "a claim the answer gave a value for",
                "tenant",
                Some(Value::from("acme")),
            ),
            (
                "a claim the answer stated as a number",
                "seats",
                Some(Value::from(3)),
            ),
            (
                "a claim named for suppression and for a value",
                "email",
                Some(Value::from("override@example.test")),
            ),
            ("a suppressed claim", "nickname", None),
            (
                "a reserved claim the answer named",
                "sub",
                Some(Value::from("the-users-own-id")),
            ),
        ];
        for (case, claim, want) in expected {
            let got = claims.get(claim).cloned();
            assert_eq!(
                got, want,
                "case {case:?}: expected {claim} to be {want:?}, got {got:?}"
            );
        }
    }

    /// Every list and map an answer carries is read up to its bound and no further, so the
    /// work one answer puts through the minting is a constant: entries past
    /// [`MAX_OVERRIDE_ENTRIES`], text past [`MAX_OVERRIDE_TEXT_BYTES`] and a claim value
    /// past [`MAX_CLAIM_VALUE_BYTES`] are not read.
    #[test]
    fn an_answer_is_read_only_up_to_its_bounds() {
        // Each member is answered with one entry MORE than its bound, one entry of text
        // one byte too long, and one claim value one byte too long.
        let over_the_entry_bound: Vec<String> = (0..MAX_OVERRIDE_ENTRIES + 1)
            .map(|index| format!("scope-{index}"))
            .collect();
        let too_long_text = "g".repeat(MAX_OVERRIDE_TEXT_BYTES + 1);
        let too_long_value = "v".repeat(MAX_CLAIM_VALUE_BYTES + 1);
        let mut claims_asked = Map::new();
        claims_asked.insert("kept".to_string(), Value::from("x"));
        claims_asked.insert("too-long".to_string(), Value::from(too_long_value));
        claims_asked.insert(too_long_text.clone(), Value::from("x"));
        let overrides = TokenOverrides::read(&::serde_json::json!({
            RESPONSE: {
                "claimsAndScopeOverrideDetails": {
                    "idTokenGeneration": { "claimsToAddOrOverride": claims_asked },
                    "accessTokenGeneration": { "scopesToAdd": over_the_entry_bound },
                    "groupOverrideDetails": {
                        "groupsToOverride": ["kept-group", too_long_text.clone()],
                    },
                },
            },
        }));

        assert_eq!(
            overrides.scopes_to_add.len(),
            MAX_OVERRIDE_ENTRIES,
            "case \"a list past the entry bound\": expected {MAX_OVERRIDE_ENTRIES} entries, \
             got {}",
            overrides.scopes_to_add.len()
        );
        let groups = overrides
            .group_override
            .as_ref()
            .map(|replacement| replacement.groups.clone())
            .unwrap_or_default();
        assert_eq!(
            groups,
            vec!["kept-group".to_string()],
            "case \"a list entry longer than any name the model admits\": expected only the \
             entry inside the bound, got {groups:?}"
        );
        let claims: Vec<&String> = overrides.id_claims.add_or_override.keys().collect();
        assert_eq!(
            claims,
            vec![&"kept".to_string()],
            "case \"a claim map with an oversized name and an oversized value\": expected \
             only the claim inside both bounds, got {claims:?}"
        );
    }
}
