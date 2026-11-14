// SPDX-License-Identifier: CC0-1.0

//! The `Scope` this service namespaces every resource by: the two values the model
//! declares, and the refusal for a value WAF will not accept.
//!
//! Whether a request may NAME a scope is a question about the request rather than
//! about the store, so it lives with this service's other request-level policy
//! (`crate::service::requests::validate_scope`); what a scope IS lives here, where
//! the resources it namespaces are.

use super::*;

/// The `Scope` values the model declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`).
pub const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";
/// See [`SCOPE_CLOUDFRONT`].
pub const SCOPE_REGIONAL: &str = "REGIONAL";

/// The `Scope` value set, in the order the model declares it — which is the order the
/// request validator lists it in when it refuses a value outside it.
pub const SCOPE_VALUES: &[&str] = &[SCOPE_CLOUDFRONT, SCOPE_REGIONAL];

/// The scope token WAF reads as a scope WITHOUT the `Scope` enum declaring it: the name
/// of the global namespace itself, which every CLOUDFRONT-scoped resource's ARN carries
/// as its first resource segment (`global/…`, [`resource_arn`]), spelled as a scope.
///
/// Recorded behaviour separates it from every other undeclared value: WAF answers this
/// one with its own bad-parameter refusal naming the scope ([`invalid_scope_exception`]),
/// and a token its scope vocabulary does not carry at all with the request validator's
/// value-set clause ([`scope_value_set_envelope`]) — so the two are two different
/// refusals, not one.
pub const SCOPE_GLOBAL_NAMESPACE: &str = "GLOBAL";

/// The region an ARN carries when the request names none: the emulator's default
/// endpoint region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `true` when `scope` is one of the two values the `Scope` enum declares.
pub fn is_known_scope(scope: &str) -> bool {
    matches!(scope, SCOPE_CLOUDFRONT | SCOPE_REGIONAL)
}

/// `true` when WAF reads `scope` as a scope at all — one of the two values the enum
/// declares, or [`SCOPE_GLOBAL_NAMESPACE`].
///
/// A value this answers `false` for is not a scope WAF can even name, so it is the
/// request validator's value-set refusal rather than an operation's.
pub fn is_scope_token(scope: &str) -> bool {
    is_known_scope(scope) || scope == SCOPE_GLOBAL_NAMESPACE
}

/// `Scope`: the `@enum` value set and nothing else.
static SCOPE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(EnumTrait::new(SCOPE_VALUES)));
    traits
});

/// `Scope`, as every operation that takes one declares it: the member schema the
/// validating walk judges a scope value against.
///
/// The walk reads only a member schema's NAME — which is what the clause's `'scope'`
/// path comes from — and its trait map, so one declaration serves every such operation.
static M_SCOPE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$Scope",
        NAMESPACE,
        "Scope",
    ),
    ShapeType::String,
    "Scope",
    0,
)
.with_traits(&SCOPE_TRAITS);

/// The request validator's envelope for a scope value WAF's vocabulary does not carry:
/// the value-set clause naming the two values the enum declares.
///
/// Built by the framework's own walk over [`M_SCOPE`] ([`validate_members`]), so a scope
/// refused by an operation that judges the value itself reads exactly like one refused
/// at the request boundary.
pub fn scope_value_set_envelope(scope: &str) -> String {
    constraint_envelope(&validate_members(&[(
        &M_SCOPE,
        ConstrainedValue::Text(scope),
    )]))
}

/// The `ErrorReason` WAF carries for a `Scope` it will not accept, as the recorded
/// WAF responses show it.
///
/// One sentence for the whole class — a value the enum does not declare, and a
/// value it does declare that this endpoint does not serve — because the
/// `Parameter` member carries the rejected scope, so the reason does not have to
/// distinguish them.
pub const SCOPE_NOT_VALID_REASON: &str = "The scope is not valid.";

/// [`invalid_parameter_exception`] for a scope WAF will not accept: `Field`
/// [`FIELD_SCOPE_VALUE`], `Parameter` the rejected scope as the request wrote it
/// and `Reason` [`SCOPE_NOT_VALID_REASON`].
pub fn invalid_scope_exception(scope: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_SCOPE_VALUE, scope, SCOPE_NOT_VALID_REASON)
}

/// The `Scope` whose namespace the resource part of a wafv2 resource ARN opens with,
/// or `None` where `value` is no wafv2 resource ARN at all.
///
/// The layout is the one [`is_wafv2_resource_arn`] pins —
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>` —
/// and that leading segment is exactly what a `Scope` names: `global` is the
/// namespace [`SCOPE_CLOUDFRONT`] names and `regional` the one [`SCOPE_REGIONAL`]
/// does ([`resource_arn`] mints them from the scope the create named). So a caller
/// holding an ARN alone can tell WHICH namespace the resource it names lives in,
/// without resolving it against this store — which is what another service deciding
/// whether it may use a web ACL needs.
pub fn arn_scope(value: &str) -> Option<&'static str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    match value.split(':').nth(5)?.split('/').next()? {
        "global" => Some(SCOPE_CLOUDFRONT),
        "regional" => Some(SCOPE_REGIONAL),
        // `is_wafv2_resource_arn` admits no third namespace, so this arm states that
        // invariant rather than hiding a case.
        _ => None,
    }
}
