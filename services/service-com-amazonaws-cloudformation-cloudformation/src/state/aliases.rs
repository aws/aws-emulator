// SPDX-License-Identifier: CC0-1.0

//! Stack ALIASES: the alternative name a stack can be addressed by.
//!
//! ## The rule
//!
//! An alias is "an alternative name for a stack that can be used in place of the stack name
//! in any CloudFormation API call that accepts the `StackName` parameter", and "after setting
//! an alias, both the original stack name and the alias resolve to the same stack". So an
//! alias is not a second record: it is a second NAME on the one stack
//! ([`super::StoredStack::alias`]), resolved by the one lookup every operation that addresses
//! a stack goes through
//! ([`super::ComAmazonawsCloudformationCloudformationState::resolve`]). Nothing else about
//! the stack changes — "setting an alias does not change the stack's ARN, physical resource
//! IDs, or any deployed resources".
//!
//! ## One namespace
//!
//! `SetAliasInput$AliasName` is documented as a name that "must be unique within the same
//! account and Region. It cannot match an existing stack name or another active alias", so a
//! Region's stack names and its active aliases are ONE namespace: an alias a live stack's
//! name already holds is refused, and so is a stack name an alias already resolves
//! ([`Self::stack_name_taken`] is what both ask). Otherwise one name would address two
//! stacks, and which of them it reached would be an accident of lookup order.
//!
//! An alias addresses a LIVE stack, exactly as a stack name does ("deleted stacks don't show
//! up in `DescribeStacks`"): the name of a stack whose deletion completed resolves nothing,
//! and neither does its alias, which is also what frees both for reuse.
//!
//! ## What is bounded
//!
//! "Each stack can have at most one alias" — [`MAX_ALIASES_PER_STACK`] — so the aliases a
//! Region holds are bounded by the live stacks it holds ([`super::MAX_STACKS`]) and this
//! surface adds no growth site of its own. A second alias for a stack that has one is the
//! modelled `LimitExceededException` rather than a replacement: "to change a stack's alias,
//! first remove the existing alias using `RemoveAlias`, then set a new one".

use super::{ComAmazonawsCloudformationCloudformationState as Store, StoredStack};

/// How many aliases one stack can carry: "each stack can have at most one alias"
/// (`smithy_query(query="doc", shape="SetAlias")`).
pub const MAX_ALIASES_PER_STACK: usize = 1;

/// The member path a refusal states the alias name at — the lowerCamel spelling every
/// violation path takes, as [`super::STACK_NAME_PATH`] is for a stack identifier.
pub const ALIAS_NAME_PATH: &str = "aliasName";

/// `Err(message)` for an alias name outside the rule `SetAliasInput$AliasName` documents: "an
/// alias name can contain only alphanumeric characters (case-sensitive) and hyphens. It must
/// start with an alphabetic character and can't be longer than 128 characters" — the very
/// rule a stack NAME is held to, which is what makes the two one namespace, reported against
/// this member.
///
/// The model carries the length as `@length(1..128)`, so the boundary refuses a value outside
/// it before a handler runs; what is left for the handler is the character rule, worded by the
/// one implementation of it ([`super::validate_name`]).
pub fn validate_alias_name(alias_name: &str) -> Result<(), String> {
    super::validate_name(ALIAS_NAME_PATH, alias_name)
}

/// The sentence of the modelled `AlreadyExistsException` ("the resource with the name
/// requested already exists") for an alias name the Region's one namespace already holds —
/// a live stack's name, or an alias already resolving one.
///
/// Fixed text apart from the name the request asked for, in the form this service words a
/// taken stack name in (`CreateStack`: "Stack [name] already exists").
pub fn alias_already_exists(alias_name: &str) -> String {
    format!("Alias [{alias_name}] already exists")
}

/// The sentence of the modelled `LimitExceededException` ("the quota for the resource has
/// already been reached") for a stack that already carries an alias, in the form this service
/// words a reached quota in (`CreateStack`: "Limit for stacks exceeded: …").
///
/// It states the quota that was reached, which is the documented one: "each stack can have at
/// most one alias. To change a stack's alias, first remove the existing alias using
/// RemoveAlias, then set a new one".
pub fn alias_limit_exceeded() -> String {
    format!(
        "Limit for aliases exceeded: no more than {MAX_ALIASES_PER_STACK} alias can be set on \
         a stack"
    )
}

/// The sentence answered when a request names an alias this Region holds none of: fixed text
/// apart from the alias the request supplied, in the form this service words an address that
/// resolves nothing in.
pub fn alias_does_not_exist(alias_name: &str) -> String {
    format!("Alias [{alias_name}] does not exist")
}

impl Store {
    /// The live stack of `region` that carries `alias_name`, or `None` for an alias this
    /// Region holds none of.
    ///
    /// A deleted stack's alias resolves nothing, for the same reason its NAME does not: both
    /// address a stack that is gone.
    pub fn stack_with_alias(&self, region: &str, alias_name: &str) -> Option<&StoredStack> {
        // Reads `stacks` directly rather than through `live_stacks`, whose Region parameter
        // shares the store's borrow: this answers a stack that outlives the Region string the
        // caller passed in.
        self.stacks.iter().find(|stack| {
            stack.region == region
                && stack.alias.as_deref() == Some(alias_name)
                && stack.is_live()
        })
    }

    /// Whether `name` is already taken in `region`'s ONE stack-name namespace: a live stack's
    /// own name, or an active alias.
    ///
    /// What `SetAlias` asks of the alias it is given ("it cannot match an existing stack name
    /// or another active alias") and what a call that MINTS a stack name asks of that name,
    /// so neither can take a name the other resolves.
    pub fn stack_name_taken(&self, region: &str, name: &str) -> bool {
        self.live_stacks(region)
            .any(|stack| stack.stack_name == name || stack.alias.as_deref() == Some(name))
    }
}
