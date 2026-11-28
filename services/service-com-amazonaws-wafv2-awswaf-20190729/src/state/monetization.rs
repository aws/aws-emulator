// SPDX-License-Identifier: CC0-1.0

//! What a SAVE that configures monetization has to satisfy: the preconditions the
//! `Monetize` rule action and the `MonetizationConfig` resource setting put on the
//! document being written.
//!
//! `MonetizeAction` states all three of them in one place
//! (`smithy_query(query="shape",shape="MonetizeAction")`): "this action is available
//! only for web ACLs associated with Amazon CloudFront distributions", "you must
//! configure a `MonetizationConfig` on the web ACL or rule group before adding rules
//! that use this action", and "you cannot use the Monetize action for rate-based
//! rules". None of them is a judgement one member can make on its own — each reads the
//! action against the scope the resource is namespaced by, the setting beside it, or
//! the statement the rule runs — so each lives here, over the whole saved document,
//! and is run by every operation that SAVES one (a web ACL or a rule group, created or
//! updated).
//!
//! Two of them are the modelled `WAFInvalidParameterException` (client, HTTP 400),
//! which describes the parameter at fault; the scope precondition is the modelled
//! `WAFInvalidOperationException` (client, HTTP 400), the refusal for an operation that
//! does not exist in the namespace it was asked for — the same answer every other
//! monetization call gets there ([`monetization_not_available_exception`]).
//!
//! One more precondition is about something the configuration NAMES rather than about
//! the document: the connected (Payments) account its payments settle into has to
//! RESOLVE, and a reference that resolves to nothing is the modelled
//! `WAFNonexistentItemException` — while one that names a partition connected accounts
//! are not available in is the modelled `WAFInvalidParameterException`
//! ([`connected_account_refusal`]).

use super::*;
use crate::ops::types::{MonetizationConfig, MonetizeAction};
use std::collections::BTreeSet;

/// How the config-required refusal names the resource being saved when that resource
/// is a web ACL: the shape's own name, as the model spells it.
pub const MONETIZED_RESOURCE_WEB_ACL: &str = "WebACL";

/// How the config-required refusal names the resource being saved when that resource
/// is a rule group. See [`MONETIZED_RESOURCE_WEB_ACL`].
pub const MONETIZED_RESOURCE_RULE_GROUP: &str = "RuleGroup";

/// The `ErrorReason` WAF carries when a parameter's value is one the setting it was
/// written in does not support — the value is well formed and the setting is real, and
/// that setting may not carry that value — as the recorded WAF responses word it.
///
/// One sentence for the whole class: the `Field` member names the setting and the
/// `Parameter` member carries the value, so the reason does not have to.
pub const PARAMETER_VALUE_NOT_SUPPORTED_REASON: &str = "The parameter value isn't supported.";

/// `WAFInvalidParameterException` as WAF returns it when the document being saved uses
/// the `Monetize` action and carries no `MonetizationConfig`: the sentence naming the
/// kind of resource the setting belongs on, and what to put in it.
///
/// `resource_kind` is [`MONETIZED_RESOURCE_WEB_ACL`] or
/// [`MONETIZED_RESOURCE_RULE_GROUP`] — the only part of the sentence that varies,
/// because the setting lives on both ("you must provide this configuration when any
/// rule in the web ACL or rule group uses the `Monetize` action",
/// `MonetizationConfig`'s own doc).
///
/// The body carries this sentence alone: unlike the parameter refusals built by
/// [`invalid_parameter_exception`], which name a `Field` of the
/// `ParameterExceptionField` vocabulary and the value at fault, what is wrong here is
/// a setting the request left OUT, so there is no value to report and WAF states the
/// requirement instead.
pub fn monetization_config_required_exception(
    resource_kind: &str,
) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "MonetizeAction requires MonetizationConfig to be configured on the \
             {resource_kind}. Provide a MonetizationConfig with payment network details."
        )),
        field: None,
        parameter: None,
        reason: None,
    }
}

/// The sentence `WAFNonexistentItemException` carries when what failed to resolve is
/// the CONNECTED ACCOUNT a monetization configuration names, as the recorded WAF
/// responses word it.
///
/// Fixed text in full, with no part taken from the request: it names the member a caller
/// corrects (`ConnectedAccountArn`) generically rather than echoing the reference that
/// resolved to nothing, and tells the caller where the current one is read from — a
/// connected account that was deleted and recreated is a different account under a
/// different ARN, so a stale reference is the common way the resolution fails.
pub const CONNECTED_ACCOUNT_NOT_FOUND_MESSAGE: &str = "The specified connected account \
     does not exist. Verify the ConnectedAccountArn. If the account was deleted and \
     recreated, it has a new ARN - use the current one from the Payments console.";

/// `WAFNonexistentItemException` as WAF returns it when the connected account a
/// monetization configuration names resolves to nothing: nothing but
/// [`CONNECTED_ACCOUNT_NOT_FOUND_MESSAGE`].
pub fn connected_account_not_found_exception() -> WAFNonexistentItemException {
    WAFNonexistentItemException {
        message: Some(CONNECTED_ACCOUNT_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The `ParameterExceptionField` value naming the settings a connected-account
/// reference is written in (`smithy_query(query="shape",shape="ParameterExceptionField")`
/// declares `MONETIZATION_CONFIG`).
pub const FIELD_MONETIZATION_CONFIG: &str = "MONETIZATION_CONFIG";

/// The one partition a connected account can be addressed in: "connected accounts are
/// available only in the commercial Amazon Web Services partition"
/// (`ConnectedAccountConfig`'s own doc).
pub const CONNECTED_ACCOUNT_PARTITION: &str = "aws";

/// The refusal a save earns from a CONNECTED ACCOUNT its monetization configuration
/// names, or `None` when it names none or names only accounts that resolve.
///
/// `MonetizationConfig$ConnectedAccountConfigs` configures the accounts the
/// configuration's payments settle into, each naming one by `ConnectedAccountArn`. A
/// connected account is not a WAFv2 resource: it is a Payments one, created in that
/// console and only ever RESOLVED here — which is why the refusal sends a caller there
/// for the current ARN — so the save resolves every account it names against the accounts
/// this service knows of
/// ([`ComAmazonawsWafv2Awswaf20190729State::connected_accounts`]) before it writes
/// anything.
///
/// TWO refusals, which the shape's own doc tells apart: a reference WAF cannot address at
/// all because "connected accounts are available only in the commercial Amazon Web
/// Services partition" is the modelled `WAFInvalidParameterException` ("…or
/// `WAFInvalidParameterException` if the ARN is invalid or the partition is
/// unsupported"), judged first because such an ARN names no account anywhere; a
/// well-addressed reference that resolves to nothing — an account never created, or
/// deleted and recreated so the reference is stale — is
/// [`connected_account_not_found_exception`] ("`WAFNonexistentItemException` if the ARN
/// no longer exists").
///
/// Run by every operation that SAVES a monetization configuration, after the
/// preconditions on the document itself ([`monetization_save_refusal`]): monetization is
/// a CloudFront-delivery feature, so a save asking for it in a namespace that does not
/// serve it asks for something that does not exist there whatever account it names, and
/// that is the refusal WAF answers.
pub fn connected_account_refusal(
    monetization_config: Option<&MonetizationConfig>,
    connected_accounts: &BTreeSet<String>,
) -> Option<MonetizationRefusal> {
    let configured = monetization_config?.connected_account_configs.as_deref()?;
    for named in configured
        .iter()
        .map(|config| config.connected_account_arn.as_str())
    {
        // The partition is read by the ONE parse this tree shares; the member's
        // `@pattern` has already settled the ARN's form at the request boundary, so a
        // value reaching here that does not parse is no ARN of any partition.
        let partition = ::service_common::arn::parse(named).map(|arn| arn.partition);
        if partition != Some(CONNECTED_ACCOUNT_PARTITION) {
            return Some(MonetizationRefusal::Parameter(invalid_parameter_exception(
                FIELD_MONETIZATION_CONFIG,
                named,
                PARAMETER_VALUE_NOT_SUPPORTED_REASON,
            )));
        }
        if !connected_accounts.contains(named) {
            return Some(MonetizationRefusal::NonexistentItem(
                connected_account_not_found_exception(),
            ));
        }
    }
    None
}

/// A `MonetizeAction` as a refusal's `Parameter` renders it: the shape's name and the
/// one member it carries, whose value is the request's own `PriceMultiplier` and the
/// literal `null` where the request sent none.
///
/// `MonetizeAction$PriceMultiplier` is the action's only member and is optional, so
/// this is the whole action as the caller wrote it — the value a caller corrects the
/// request by.
pub fn monetize_action_parameter(action: &MonetizeAction) -> String {
    let price_multiplier = action.price_multiplier.as_deref().unwrap_or("null");
    format!("MonetizeAction(priceMultiplier={price_multiplier})")
}

/// Every `Monetize` action the saved document configures, in the order the request
/// wrote them.
///
/// A `RuleAction` appears in two places in a saved document: as a rule's own `Action`,
/// and as the `ActionToUse` an override puts in the place of a rule-group rule's own
/// action. Both are actions rules of this document take, so both are collected —
/// collected over the whole rule tree with an explicit worklist, like every other walk
/// over a caller-supplied rule tree ([`validate_statement_nesting`]).
pub fn monetize_actions(rules: &[Rule]) -> Vec<&MonetizeAction> {
    let mut actions: Vec<&MonetizeAction> = Vec::new();
    for rule in rules {
        if let Some(monetize) = rule
            .action
            .as_ref()
            .and_then(|action| action.monetize.as_ref())
        {
            actions.push(monetize);
        }
        let mut pending: Vec<&Statement> = vec![&rule.statement];
        while let Some(statement) = pending.pop() {
            for carried in [
                statement
                    .rule_group_reference_statement
                    .as_ref()
                    .and_then(|reference| reference.rule_action_overrides.as_deref()),
                statement
                    .managed_rule_group_statement
                    .as_ref()
                    .and_then(|managed| managed.rule_action_overrides.as_deref()),
            ] {
                actions.extend(
                    carried
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|carried| carried.action_to_use.monetize.as_ref()),
                );
            }
            pending.extend(nested_statements(statement));
        }
    }
    actions
}

/// The `Monetize` action of a RATE-BASED rule of `rules`, if one of them carries one.
///
/// "You cannot use the Monetize action for rate-based rules" (`MonetizeAction`'s own
/// doc): a rate-based rule counts the requests that match it over a window, and the
/// Monetize action answers a request it takes with an HTTP 402 payment flow the client
/// then completes, so WAF has no evaluation for a rule that asks for both and refuses
/// the save rather than storing one.
///
/// A rule is rate-based when its OWN top-level statement is the rate-based one, which
/// is the only place that statement kind may be written
/// ([`statement_settings_refusal`] refuses it anywhere else).
pub fn rate_based_monetize_action(rules: &[Rule]) -> Option<&MonetizeAction> {
    rules
        .iter()
        .filter(|rule| rule.statement.rate_based_statement.is_some())
        .find_map(|rule| {
            rule.action
                .as_ref()
                .and_then(|action| action.monetize.as_ref())
        })
}

/// What a save carrying monetization settings is refused with: one of the three
/// modelled exceptions, each on the error channel of the operation that made the
/// judgement ([`MonetizationRefusal::refuse`]).
pub enum MonetizationRefusal {
    /// The monetization settings were asked for in a namespace that does not serve
    /// them: the modelled `WAFInvalidOperationException`.
    NotAvailableInScope(WAFInvalidOperationException),
    /// The settings themselves do not go together, or one of them is missing: the
    /// modelled `WAFInvalidParameterException`.
    Parameter(WAFInvalidParameterException),
    /// The connected account the configuration names resolves to nothing: the modelled
    /// `WAFNonexistentItemException` ([`connected_account_refusal`]).
    NonexistentItem(WAFNonexistentItemException),
}

impl MonetizationRefusal {
    /// This refusal on the error channel of the operation that made the judgement:
    /// `invalid_operation`, `invalid_parameter` and `nonexistent_item` are that
    /// operation's variants for the three modelled exceptions.
    pub fn refuse<E>(
        self,
        invalid_operation: impl FnOnce(WAFInvalidOperationException) -> E,
        invalid_parameter: impl FnOnce(WAFInvalidParameterException) -> E,
        nonexistent_item: impl FnOnce(WAFNonexistentItemException) -> E,
    ) -> E {
        match self {
            Self::NotAvailableInScope(refusal) => invalid_operation(refusal),
            Self::Parameter(refusal) => invalid_parameter(refusal),
            Self::NonexistentItem(refusal) => nonexistent_item(refusal),
        }
    }
}

/// The refusal a saved document earns from its monetization settings, or `None` when
/// it earns none.
///
/// Run by every operation that SAVES a web ACL or a rule group, before anything is
/// written, so a refused save stores nothing and rotates no lock token. `scope` is the
/// namespace the saved resource lives in, `rules` its rule list, `monetization_config`
/// the setting the same request carries, `resource_kind` which of the two kinds of
/// resource is being saved ([`MONETIZED_RESOURCE_WEB_ACL`],
/// [`MONETIZED_RESOURCE_RULE_GROUP`]) and `connected_accounts` the accounts a
/// configuration's settlement account is resolved against
/// ([`ComAmazonawsWafv2Awswaf20190729State::connected_accounts`]).
///
/// The four preconditions, judged in this order:
///
/// 1. monetization is a CloudFront-delivery feature — "this action is available only
///    for web ACLs associated with Amazon CloudFront distributions" — so asking for it
///    at all under the regional namespace asks for something that does not exist
///    there, whichever setting asked ([`monetization_not_available_exception`]);
/// 2. a rate-based rule may not take the action, and the refusal names the rule's
///    action setting and renders the action the request wrote
///    ([`rate_based_monetize_action`]);
/// 3. a document whose rules take the action must carry the configuration those
///    payments settle against ([`monetization_config_required_exception`]);
/// 4. the connected account that configuration names has to resolve
///    ([`connected_account_refusal`]) — last, because it is the one precondition about
///    something OUTSIDE the document, and a configuration asked for where the feature
///    is not served, or missing where it is required, is answered for that first.
pub fn monetization_save_refusal(
    scope: &str,
    rules: &[Rule],
    monetization_config: Option<&MonetizationConfig>,
    resource_kind: &str,
    connected_accounts: &BTreeSet<String>,
) -> Option<MonetizationRefusal> {
    // What the request carries IS the configuration the saved document will have,
    // whatever the stored one was: a save is a whole-document write, and an update
    // replaces this setting as well as the rules.
    let configured = monetization_config.is_some();
    let monetize_actions = monetize_actions(rules);
    if (!monetize_actions.is_empty() || configured) && scope != SCOPE_CLOUDFRONT {
        return Some(MonetizationRefusal::NotAvailableInScope(
            monetization_not_available_exception(),
        ));
    }
    if let Some(action) = rate_based_monetize_action(rules) {
        return Some(MonetizationRefusal::Parameter(invalid_parameter_exception(
            FIELD_RULE_ACTION,
            &monetize_action_parameter(action),
            PARAMETER_VALUE_NOT_SUPPORTED_REASON,
        )));
    }
    if !monetize_actions.is_empty() && !configured {
        return Some(MonetizationRefusal::Parameter(
            monetization_config_required_exception(resource_kind),
        ));
    }
    connected_account_refusal(monetization_config, connected_accounts)
}
