// SPDX-License-Identifier: CC0-1.0

//! How a capacity provider is ADDRESSED: the ARN this service mints for one, the ARN
//! resource type that ARN carries, the reading of a client-sent `CapacityProviderName` in
//! either documented form, and the refusals a value that names no provider — or an operator
//! role this service cannot manage its compute with, or may not be handed at all — is
//! answered with.
//!
//! It lives in its own module rather than in the already long [`crate::state`], which
//! re-exports every item here, so a handler still names them as `crate::state::…` and the
//! module split is invisible to it. The record itself, its clock-owed `Pending` ->
//! `Active` transition and the store that holds it stay in [`crate::state`] beside the
//! function slice, because the tagging surface and `materialize_all` reach them there.
//!
//! Minting and reading sit together ON PURPOSE: [`capacity_provider_arn`] writes the ARN
//! and [`parse_capacity_provider_name`] reads one back, both over
//! [`CAPACITY_PROVIDER_RESOURCE_TYPE`], so the two cannot drift.
//!
//! Nothing here can panic: the value a caller sends is UNTRUSTED text, and it is only ever
//! split on `:` and compared — nothing indexes, slices or does arithmetic on it.

use super::ARN_NAMESPACE;

/// The ARN vendor field an IAM role ARN carries, as the modelled pattern of the operator
/// role member writes it
/// (`smithy_query(query="shape",shape="CapacityProviderPermissionsConfig")`:
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`) — the field the
/// cross-account reading below parses the operator role over.
const IAM_ARN_NAMESPACE: &str = "iam";

/// The ARN resource type of a capacity provider, as the model's `CapacityProviderArn`
/// pattern writes it (`…:lambda:<region>:<account>:capacity-provider:<name>`) — stated
/// once, for the ARN this module mints and the ARN it reads back.
pub const CAPACITY_PROVIDER_RESOURCE_TYPE: &str = "capacity-provider";

/// The ARN of a capacity provider:
/// `arn:aws:lambda:<region>:<account>:capacity-provider:<name>`, as the model's
/// `CapacityProviderArn` pattern requires.
#[must_use]
pub fn capacity_provider_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CAPACITY_PROVIDER_RESOURCE_TYPE}:{name}"),
    )
}

/// The bare provider name a `CapacityProviderName` names.
///
/// The model's `CapacityProviderName` pattern admits two forms — a capacity-provider ARN
/// and a bare name — and both name the same record, so an ARN resolves here to the name
/// it carries. `None` when the value is an `arn:`-prefixed string that is not a
/// capacity-provider ARN of this service, which names no provider at all.
///
/// The value is UNTRUSTED request text, so the ARN form goes through the ONE shared parser
/// (`service_common::arn`, whose `parse` walks the six fields with `splitn(6, ':')` and `?`
/// on each `next()`) rather than a second ad-hoc reader, and the bare form is taken as it
/// stands: nothing here indexes, slices or does arithmetic on the value, so no input can
/// make it panic.
#[must_use]
pub fn parse_capacity_provider_name(value: &str) -> Option<String> {
    if !value.starts_with("arn:") {
        return (!value.is_empty()).then(|| value.to_string());
    }
    // arn:<partition>:lambda:<region>:<account>:capacity-provider:<name>. The resource
    // type is required, so an ARN of another Lambda resource is not read as a provider
    // named after its resource type, and an ARN of another vendor names no provider here.
    ::service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    ::service_common::arn::resource_id_of_type(value, CAPACITY_PROVIDER_RESOURCE_TYPE)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The ARN an operation addressed at the provider `capacity_provider_name` names, which is
/// the resource IAM's decision about that call is made on
/// ([`crate::state::authorize_caller`]).
///
/// Built from the REQUEST alone — no store is read — and spelled exactly as the ARN this
/// module mints for a provider of that name in the Region the caller signed for, so a policy
/// written about one provider covers exactly the calls that address it, whichever documented
/// `CapacityProviderName` form the request used. A CREATE is judged on the ARN of the
/// provider it is about to create, which is the resource such a call names.
///
/// [`crate::state::invoke_authorization::ANY_RESOURCE`] for a value that names no provider at
/// all: there is no ARN of this account to decide on, and IAM writes the resource of such a
/// request that way. The decision is taken ahead of the service's own reading of the value, so
/// the refusal of a caller no policy allows does not depend on the value being well formed.
#[must_use]
pub fn judged_resource(region: &str, capacity_provider_name: &str) -> String {
    match parse_capacity_provider_name(capacity_provider_name) {
        Some(name) => capacity_provider_arn(region, &name),
        None => super::invoke_authorization::ANY_RESOURCE.to_string(),
    }
}

/// The message a `CapacityProviderName` that names no provider is refused with: a
/// validation message built from the request, naming the member and the constraint it
/// failed.
#[must_use]
pub fn capacity_provider_name_message(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at 'capacityProviderName' failed to \
         satisfy constraint: Member must satisfy one of the documented name formats (name \
         or capacity provider ARN)"
    )
}

/// The sentence a capacity-provider operation refuses a well-formed `CapacityProviderName`
/// that names no provider with, where that sentence's variable part is the value the REQUEST
/// named — either documented `CapacityProviderName` form, echoed exactly as the caller
/// supplied it.
///
/// WHICH form the variable part takes is RECORDED PER OPERATION, because AWS states it per
/// operation and no model records it: the two sentences are otherwise identical, and both
/// are the same two fixed clauses.
///
/// * `DeleteCapacityProvider` and `ListFunctionVersionsByCapacityProvider` echo the
///   identifier, which is this function (corpus `simparity-wire-aws-v1`, fixture
///   `lambda-capacity-provider-negative-paths` steps `task#4` and `task#5`: a request
///   naming a bare provider name is answered with that bare name);
/// * `GetCapacityProvider` and `UpdateCapacityProvider` state the capacity-provider ARN the
///   identifier names in the Region the caller signed for, which is what
///   [`super::resolve_capacity_provider`] builds for every caller of it (same corpus,
///   fixture `lambda-capacity-provider-lifecycle` steps `task#3` and `task#4`: a request
///   naming a bare provider name is answered with the full ARN).
///
/// Nothing in either form is read out of the store: the provider the request named does not
/// exist, so there is no record to report anything from.
#[must_use]
pub fn capacity_provider_not_found_message(capacity_provider_name: &str) -> String {
    format!(
        "Capacity provider not found: {capacity_provider_name}. Check that the ARN is \
         correct and the capacity provider exists in this Region."
    )
}

/// The sentence a create is refused with when this account already holds a capacity
/// provider under the name the request carried.
///
/// Two fixed sentences — the conflict, and the rule that makes it one — whose only variable
/// part is the ARN of the provider that already exists, which is the one
/// [`capacity_provider_arn`] mints for that name in the Region the caller signed for.
/// Nothing of the request is echoed: the name travels in the ARN the store holds for the
/// existing provider.
#[must_use]
pub fn capacity_provider_already_exists_message(capacity_provider_arn: &str) -> String {
    format!(
        "A capacity provider with ARN {capacity_provider_arn} already exists. Capacity \
         provider names must be unique within your account."
    )
}

/// Why a request naming an operator role this service cannot manage compute with is
/// refused.
///
/// Wholly fixed text: it names neither the role the request carried nor which half of the
/// question failed, because a caller that may not use the role is not told whether it
/// exists.
///
/// The question it answers is [`operator_role_is_unassumable`], which is the reading this
/// surface takes; only this sentence is the capacity-provider surface's own.
pub const OPERATOR_ROLE_INVALID_MESSAGE: &str =
    "The operator role is invalid or doesn't have sufficient permissions. Verify the role \
     and permissions and try again.";

/// Whether IAM ANSWERED that `operator_role_arn` is a role this service cannot manage a
/// provider's compute as: this account's IAM holds NO role under the name the ARN carries,
/// or the role it does hold carries a trust policy that does not let this service assume it
/// — the create is then refused with [`OPERATOR_ROLE_INVALID_MESSAGE`] and nothing is
/// stored.
///
/// It is the reading of the ONE shared question (`service_common::iam_role`) that the
/// cross-service suite `core/router/tests/all/lambda_capacity_provider_operator_role.rs`
/// pins over its table of operator roles: the role a provider manages compute as is part of
/// what the client provisions in this account, so "this account's IAM holds no role under
/// that name" IS an answer about the role the request named. That separates it from a
/// function's EXECUTION role ([`super::execution_role_is_unassumable`]), which is routinely
/// provisioned out of band and so serves such a create. Neither reading restates which role
/// of this account an ARN names, how IAM is asked, or how a trust document is read.
///
/// `false` — do not refuse — for every other case, wherever nothing is KNOWN: a value that
/// names no role of THIS account (another account's IAM is not this emulator's to ask, and
/// such a value is already refused ahead of this question by
/// [`operator_role_is_of_another_account`]), an emulator with no IAM mounted to ask at all,
/// and an answer this emulator could not read. The principal that has to be admitted is the
/// one a function's execution role admits ([`super::EXECUTION_ROLE_PRINCIPAL`]): this
/// service is what assumes both.
///
/// It therefore takes the STRONGER of the two shared readings
/// (`service_common::iam_role::role_is_unassumable`, which refuses a role this account's IAM
/// reports ABSENT) rather than the one a function's EXECUTION role takes
/// (`held_role_is_unassumable`, which serves it): an execution role is routinely provisioned
/// out of band, while the role a provider manages compute as is part of what the client
/// provisions in this account, and THIS operation is recorded refusing an operator role that
/// does not exist. The difference between the two surfaces is deliberate; the cross-service
/// suite `core/router/tests/all/lambda_capacity_provider_operator_role.rs` pins it, so a
/// change that makes that suite fail is the change to fix.
pub async fn operator_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operator_role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        operator_role_arn,
        super::EXECUTION_ROLE_PRINCIPAL,
    )
    .await
}

/// Why a request that hands this service an operator role of ANOTHER account is refused.
///
/// Wholly fixed text: the denial is about the hand-off itself rather than about any role, so
/// it names neither the role the request carried nor the account that holds it — the same
/// sentence the rest of this tree records AWS refusing a pass of another account's role with.
///
/// The question it answers is [`operator_role_is_of_another_account`]; it is answered with
/// the 403 `AccessDeniedException` channel ([`crate::scaffolded::AccessDeniedException`]),
/// not with the modelled 400 of [`OPERATOR_ROLE_INVALID_MESSAGE`], because the caller is
/// refused on the authorization to pass the role rather than on the role's own fitness.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// Whether `operator_role_arn` names an IAM role of an account OTHER than the one whose
/// resources this emulator holds — the create is then refused with
/// [`CROSS_ACCOUNT_PASS_ROLE_MESSAGE`] and nothing is stored.
///
/// Creating a provider HANDS this service the operator role, which is an `iam:PassRole` the
/// caller has to hold, and a pass of a role is granted by the account that HOLDS that role:
/// a principal of this account can pass a role of its own account and no role of another
/// one, whatever that other role trusts. So this is decided on the REQUEST alone — the
/// account field of the ARN against [`::engine::emulator_account_id`] — and asks IAM
/// nothing: the IAM that could answer about another account's role is not this emulator's,
/// which is exactly why the pass cannot be authorized. Being grounded in the request and not
/// in an IAM answer, it stands whether or not an IAM is mounted to ask, unlike
/// [`operator_role_is_unassumable`], and it is asked FIRST, so a role of another account is
/// never the subject of a question about this account's records.
///
/// `false` for a role ARN of THIS account, which the member's modelled pattern admits only
/// with a 12-digit account, and for an EMPTY account field, which names no OTHER account —
/// both are then judged by [`operator_role_is_unassumable`]. Also `false` for a value that
/// is no IAM ARN at all: such a value violates the member's modelled pattern and is refused
/// by the constraint walk ahead of any handler, so nothing here has to invent an answer for
/// it. The ARN's fields come from the crate's ONE shared parse
/// (`service_common::arn::parse_of`), so nothing indexes or slices the untrusted value.
///
/// These answers are AWS's RECORDED behaviour for this operation, and they differ from the
/// reading a function's EXECUTION role takes — which serves a role of another account — on
/// purpose: an execution role is stored for later invocations, while the operator role is
/// handed over by this very call. The cross-service suite
/// `core/router/tests/all/lambda_capacity_provider_operator_role.rs` and this crate's
/// `tests/all/capacity_provider_operator_role.rs` pin both answers, so a fix for another
/// issue must not flip them: if those suites fail in a gate, the change under review is what
/// to fix.
#[must_use]
pub fn operator_role_is_of_another_account(operator_role_arn: &str) -> bool {
    ::service_common::arn::parse_of(operator_role_arn, IAM_ARN_NAMESPACE).is_some_and(|arn| {
        !arn.account.is_empty() && arn.account != ::engine::emulator_account_id()
    })
}
