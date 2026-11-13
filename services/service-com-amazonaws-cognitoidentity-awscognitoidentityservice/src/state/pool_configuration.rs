// SPDX-License-Identifier: CC0-1.0

//! What an identity pool's own CONFIGURATION has to satisfy before this service stores it,
//! and how a configuration it will not store is refused.
//!
//! The two operations that write a pool — `CreateIdentityPool` and `UpdateIdentityPool` —
//! take the same configuration members, so every rule about those members lives here once
//! and both read it, rather than each operation carrying its own copy of a bound that
//! would then drift.
//!
//! Three rules, each refused in its own class:
//!
//! * a configuration STRING outside the `@pattern` its shape declares, read as AWS reads
//!   that expression ([`satisfies_modelled_pattern`]) — a constraint the MODEL states, so
//!   the refusal is the protocol-level `ValidationException` the constraint layer answers
//!   with, carrying AWS's own clause for the member ([`pattern_refusal`]);
//! * a TAG whose key or value carries a character tags may not hold
//!   ([`tag_charset_refusal`]) — a rule about the value of a parameter, which is the
//!   modelled `InvalidParameterException`;
//! * more Amazon Cognito user pool providers than the account's quota admits
//!   ([`MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL`]) — a published quota of the resource,
//!   which is the modelled `LimitExceededException`.
//!
//! And one normalisation: a provider map the request carries with NO entry configures no
//! provider, so the pool is stored without the member rather than with an empty map
//! ([`configured_providers`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

/// The MODELLED `@pattern` of `IdentityPoolName`, `^[\w\s+=,.@-]+$`
/// (`smithy_query(query="shape",shape="IdentityPoolName")`).
pub const IDENTITY_POOL_NAME_PATTERN: &str = r"^[\w\s+=,.@-]+$";

/// The MODELLED `@pattern` of `DeveloperProviderName`, `^[\w._-]+$`, from the same query.
pub const DEVELOPER_PROVIDER_NAME_PATTERN: &str = r"^[\w._-]+$";

/// Whether `value` satisfies the modelled `@pattern` `pattern` AS AWS READS IT.
///
/// AWS's constraint layer matches a modelled expression with its `\w`, `\s` and `\d`
/// classes standing for ASCII characters alone, so a value carrying a letter outside ASCII
/// — an accented letter, a letter of another script — satisfies none of the expressions
/// this service's shapes declare, every one of which spells its character class with those
/// shorthands. The matchers are therefore built with Unicode mode OFF
/// (`regex`'s `RegexBuilder::unicode`), which is exactly that reading; each expression is
/// the model's, anchors included, so nothing about what the member admits is restated here.
///
/// The expressions are compiled ONCE, when the first request reaches this rule, and never
/// from anything a request carries: a request's value is the string that is MATCHED, never
/// the expression. A pattern this service holds no compiled expression for decides NOTHING
/// (`true`): the validating walk ahead of the handler has already applied the model's own
/// constraints, and a value is never refused on a failure of this reading.
pub fn satisfies_modelled_pattern(value: &str, pattern: &str) -> bool {
    match MODELLED_EXPRESSIONS.get(pattern) {
        Some(expression) => expression.is_match(value),
        None => true,
    }
}

/// The modelled expressions this service matches a configuration string against, compiled
/// as AWS reads them ([`satisfies_modelled_pattern`]) — one entry per pattern the rules
/// here name, built on first use rather than per request.
static MODELLED_EXPRESSIONS: LazyLock<HashMap<&'static str, Regex>> = LazyLock::new(|| {
    [IDENTITY_POOL_NAME_PATTERN, DEVELOPER_PROVIDER_NAME_PATTERN]
        .into_iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .unicode(false)
                .build()
                .ok()
                .map(|expression| (pattern, expression))
        })
        .collect()
});

/// The message a constraint refusal of `member_path` carries, for a value outside the
/// modelled `pattern`: the envelope AWS counts its violations in
/// ([`super::validation_errors_detected`]) over the one clause naming the member, the value
/// the request gave it and the expression that value had to match.
///
/// The requirement half is the framework's own sentence for a `@pattern`
/// (`service_common::validation_message::requirement`) and the expression is quoted
/// UNANCHORED, as every constraint clause of this service quotes one
/// (`crate::service::validation`) — so a clause this rule words and one the boundary walk
/// states about the same member read alike.
pub fn pattern_refusal(member_path: &str, value: &str, pattern: &str) -> String {
    let requirement = ::service_common::validation_message::requirement(
        ::aws_smithy_schema_server::ViolationKind::Pattern(
            ::service_common::validation::unanchored(pattern),
        ),
    );
    let clause =
        format!("Value '{value}' at '{member_path}' failed to satisfy constraint: {requirement}");
    super::validation_errors_detected(1, &[clause])
}

/// The constraint refusal for the first of `members` whose value is outside its modelled
/// pattern, or `None` when every one of them satisfies it.
///
/// Each entry is `(member path, value, the pattern its shape declares)`, and a member the
/// request left out carries no value and so is not an entry at all. The members are read in
/// the order they are given, which is the order the model declares them in, so a request
/// with two values outside their patterns is refused for the same one every time.
pub fn pattern_refusal_for(members: &[(&str, &str, &str)]) -> Option<String> {
    members
        .iter()
        .find(|(_, value, pattern)| !satisfies_modelled_pattern(value, pattern))
        .map(|(member_path, value, pattern)| pattern_refusal(member_path, value, pattern))
}

/// How AWS words the refusal of a tag whose key or value carries a character tags may not
/// hold: it states the characters a tag admits and nothing of the request, so the rejected
/// value is not echoed back.
///
/// The set it names is the one AWS's tagging rules publish — letters, numbers, spaces and
/// `_ . : / = + - @` — which is the set [`super::is_well_formed_tag_text`] reads.
pub const INVALID_TAG_CHARACTERS_MESSAGE: &str =
    "Tags can only contain letters, numbers, spaces, and the following special characters: \
     _ . : / = + - @";

/// [`INVALID_TAG_CHARACTERS_MESSAGE`] when any entry of `tags` carries a character tags may
/// not hold — in its KEY or in its VALUE, AWS publishing one character set for both halves
/// of a tag — and `None` when every entry is spelled with characters a tag admits.
///
/// The entries are read in the order they are given (key order wherever a handler sorts
/// them, as they all do), so which entry a multi-entry map is refused for does not depend on
/// the iteration order of the request's own map.
pub fn tag_charset_refusal(tags: &[(&str, &str)]) -> Option<&'static str> {
    tags.iter()
        .any(|(key, value)| {
            !super::is_well_formed_tag_text(key) || !super::is_well_formed_tag_text(value)
        })
        .then_some(INVALID_TAG_CHARACTERS_MESSAGE)
}

/// The most Amazon Cognito user pool providers one identity pool may be configured with:
/// AWS publishes "Amazon Cognito user pool providers per identity pool" as a quota of the
/// `cognito-identity` service, defaulting to 50 and adjustable, and a pool configured with
/// one provider past the account's quota is refused with the modelled
/// `LimitExceededException` (HTTP 400) — the error `CreateIdentityPool` and
/// `UpdateIdentityPool` both model for a limit of the resource.
///
/// A quota of the ACCOUNT rather than of this emulator, which is why it is enforced on the
/// configuration a call would leave the pool with rather than on the store's size (the
/// emulator's own ceilings are [`super::MAX_IDENTITY_POOLS`] and its siblings).
pub const MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL: usize = 50;

/// How this service words the refusal of a configuration naming more user pool providers
/// than [`MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL`]: it names the quota that was reached,
/// which is the one thing the caller has to act on, and nothing else of the request.
pub fn too_many_user_pool_providers_message() -> String {
    format!(
        "The maximum number of user pool providers per identity pool \
         ({MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL}) has been reached."
    )
}

/// The member path a constraint clause names `IdentityPoolName` at: the member's own name,
/// in the lowerCamel spelling a clause spells a member in.
const IDENTITY_POOL_NAME_MEMBER_PATH: &str = "identityPoolName";

/// The member path a constraint clause names `DeveloperProviderName` at.
const DEVELOPER_PROVIDER_NAME_MEMBER_PATH: &str = "developerProviderName";

/// The configuration a `CreateIdentityPool` or an `UpdateIdentityPool` request would leave
/// a pool with, reduced to the parts the rules here decide about.
///
/// Both operations take the same configuration members, so both build this and read the one
/// answer ([`PoolConfiguration::refusal`]) rather than each applying its own copy of a rule.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfiguration<'a> {
    /// The pool's name, as the request spelled it.
    pub identity_pool_name: &'a str,
    /// The pool's developer provider name, for a request that carries one.
    pub developer_provider_name: Option<&'a str>,
    /// The pool's whole tag set, for a request that carries the member.
    pub identity_pool_tags: Option<&'a HashMap<String, String>>,
    /// How many Amazon Cognito user pool providers the configuration names — none for a
    /// request that carries no `CognitoIdentityProviders` member.
    pub user_pool_providers: usize,
}

/// Why a pool configuration is not one this service stores: each variant is a CLASS of
/// refusal, which the operation maps onto the error of its own model that reports that
/// class.
#[derive(Debug, Clone)]
pub enum ConfigurationRefusal {
    /// A value outside a constraint the MODEL declares: the protocol-level refusal, HTTP
    /// 400 `ValidationException`, carrying the whole message ([`pattern_refusal`]).
    Constraint(String),
    /// A bad input parameter: HTTP 400 `InvalidParameterException`, carrying this sentence.
    InvalidParameter(&'static str),
    /// A published quota of the resource reached: HTTP 400 `LimitExceededException`,
    /// carrying this sentence.
    LimitExceeded(String),
}

impl PoolConfiguration<'_> {
    /// Why this configuration is not stored, or `None` when it is one this service stores.
    ///
    /// The rules are read in the order AWS decides them in: the values the MODEL constrains
    /// first (the constraint layer runs ahead of the operation), then the parameters this
    /// service has its own rule about, then the quotas of the resource.
    pub fn refusal(&self) -> Option<ConfigurationRefusal> {
        let mut patterned: Vec<(&str, &str, &str)> = vec![(
            IDENTITY_POOL_NAME_MEMBER_PATH,
            self.identity_pool_name,
            IDENTITY_POOL_NAME_PATTERN,
        )];
        if let Some(developer_provider_name) = self.developer_provider_name {
            patterned.push((
                DEVELOPER_PROVIDER_NAME_MEMBER_PATH,
                developer_provider_name,
                DEVELOPER_PROVIDER_NAME_PATTERN,
            ));
        }
        if let Some(message) = pattern_refusal_for(&patterned) {
            return Some(ConfigurationRefusal::Constraint(message));
        }
        // Read in key order, so which entry a multi-entry map is refused for does not
        // depend on the iteration order of the request's own map.
        let mut tags: Vec<(&str, &str)> = self
            .identity_pool_tags
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        tags.sort_unstable();
        if let Some(message) = tag_charset_refusal(&tags) {
            return Some(ConfigurationRefusal::InvalidParameter(message));
        }
        if self.user_pool_providers > MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL {
            return Some(ConfigurationRefusal::LimitExceeded(
                too_many_user_pool_providers_message(),
            ));
        }
        None
    }
}

/// The `SupportedLoginProviders` map a pool is STORED with, for the map a request carried.
///
/// A map with no entry configures no login provider, and AWS reports the pool the same way
/// whether the member was sent empty or not sent at all — so "configured but empty" and "not
/// configured" are one state, and the pool is stored without the member. Normalising it
/// where the pool is WRITTEN (rather than where it is read) is what makes the two
/// indistinguishable to every reader of the pool, the pool's own configuration rules
/// included.
pub fn configured_providers(
    providers: Option<HashMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    providers
        .map(|providers| providers.into_iter().collect::<BTreeMap<String, String>>())
        .filter(|providers| !providers.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The modelled patterns of this service are read as AWS reads them: their `\w` class
    /// is ASCII, so a value carrying a letter outside ASCII satisfies neither.
    #[test]
    fn a_modelled_pattern_admits_ascii_word_characters_alone() {
        // (case, value, satisfies IdentityPoolName's pattern)
        let rows: &[(&str, &str, bool)] = &[
            ("letters, digits and the punctuation the class names", "pool +=,.@-_name 1", true),
            ("one ASCII letter", "a", true),
            ("an accented letter", "pool Café", false),
            ("a letter of another script", "团队", false),
            ("an emoji", "pool \u{1F600}", false),
            ("a character outside the class", "pool/name", false),
        ];
        for (case, value, expected) in rows {
            let actual = satisfies_modelled_pattern(value, IDENTITY_POOL_NAME_PATTERN);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected satisfied == {expected}, got {actual}"
            );
        }
    }

    /// The clause a pattern refusal carries names the member, the value and the expression,
    /// under the envelope that counts the violations.
    #[test]
    fn a_pattern_refusal_states_the_member_the_value_and_the_expression() {
        let message = pattern_refusal("identityPoolName", "pool Café", IDENTITY_POOL_NAME_PATTERN);
        let expected = "1 validation error detected: Value 'pool Café' at 'identityPoolName' \
                        failed to satisfy constraint: Member must satisfy regular expression \
                        pattern: [\\w\\s+=,.@-]+";
        assert_eq!(
            message, expected,
            "case \"a name outside its pattern\": expected {expected:?}, got {message:?}"
        );
    }

    /// An empty provider map is stored as no map at all, and one with entries is stored as
    /// it was sent.
    #[test]
    fn an_empty_provider_map_configures_nothing() {
        assert_eq!(
            configured_providers(Some(HashMap::new())),
            None,
            "case \"an explicitly empty map\": expected the member unset"
        );
        assert_eq!(
            configured_providers(None),
            None,
            "case \"an absent map\": expected the member unset"
        );
        let sent = HashMap::from([("graph.facebook.com".to_string(), "1234567890".to_string())]);
        assert_eq!(
            configured_providers(Some(sent)),
            Some(BTreeMap::from([(
                "graph.facebook.com".to_string(),
                "1234567890".to_string()
            )])),
            "case \"one provider\": expected the map as sent"
        );
    }

    /// A tag is admitted on its characters: letters of any script, digits, separators and
    /// the published punctuation, in the key as in the value.
    #[test]
    fn a_tag_is_refused_for_a_character_tags_may_not_hold() {
        // (case, key, value, refused)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("plain ASCII", "Env", "test", false),
            ("every published punctuation mark", "a_.:/=+-@", "b_.:/=+-@", false),
            ("a unicode letter in the value", "Owner", "Café", false),
            ("a unicode letter in the key", "Café", "owner", false),
            ("an emoji in the value", "Env", "test \u{1F680}", true),
            ("an emoji in the key", "env \u{1F680}", "test", true),
            ("a control character in the value", "Env", "te\u{7}st", true),
        ];
        for (case, key, value, expected) in rows {
            let refused = tag_charset_refusal(&[(key, value)]).is_some();
            assert_eq!(
                refused, *expected,
                "case {case:?}: expected refused == {expected}, got {refused}"
            );
        }
    }
}
