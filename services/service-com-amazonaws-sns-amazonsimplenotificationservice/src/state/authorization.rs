// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a signed request perform the action it asks for on the RESOURCE it
//! addresses — the decision an operation takes before it reads or writes anything.
//!
//! Every operation of this service is signed, so every call is made by a principal, and a
//! principal may perform an action only when the documents in force allow it. Which
//! documents those are follows from what the call addresses, and this module answers both
//! questions the same way:
//!
//! * a call that addresses a TOPIC ([`topic_refusal`]) is decided by two documents, which
//!   AWS evaluates together: the calling principal's own identity-based policies, and the
//!   topic's access policy (its `Policy` attribute, the resource-based policy SNS generates
//!   for a topic whose owner has set none);
//! * a call that addresses NO resource of its own ([`account_refusal`]) — the account's own
//!   listings, its SMS settings and its SMS-sandbox membership, whose IAM actions support no
//!   resource-level permissions — is decided by the identity-based documents alone, over the
//!   resource that names every resource of this service in the account and Region the
//!   request reached ([`account_wide_resource`]); there is no resource-based policy in force
//!   on such a call.
//!
//! An explicit `Deny` in either document refuses, an `Allow` from either grants, and with
//! neither the request falls to IAM's default deny — answered with the error this service
//! declares for an unauthorized call, `AuthorizationErrorException` (HTTP 403, wire code
//! `AuthorizationError`).
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the IAM
//!    store of the same shared engine, which holds both halves of that question: the
//!    role-session record a temporary credential's mint left there, and the registry
//!    `iam:CreateAccessKey` wrote for a long-term key. So the principal a refusal blames is
//!    the one the account really holds for the credential the request signed with.
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through IAM's own
//!    accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes effect on
//!    the next request and deleting it lifts it again — beside the topic's own `Policy`,
//!    read from this service's store, where `SetTopicAttributes` and `AddPermission` write
//!    it.
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of policy documents against `(action, resource, principal)`, so this service and
//!    every other one handed the same documents answer alike; and the SENTENCES a refusal
//!    carries, which are that module's, so nothing of IAM's wording is spelled here beyond
//!    the one reason clause it does not publish.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle.
//!
//! ## Which half grants
//!
//! A resource-based statement that names the ACCOUNT rather than the principal — a root
//! ARN or a bare account id, which is the form `AddPermission` writes a delegated account
//! as — delegates the decision to that account's own identity-based policies rather than
//! granting the call outright, so the account spellings
//! ([`::service_common::iam_authorization::account_principals`]) are read for a `Deny` and
//! not for an `Allow`. That is also why the policy SNS generates for a topic whose owner
//! set none grants nothing by itself: it allows every principal of the topic's own account
//! only where `aws:SourceOwner` equals that account, a key a cross-service request carries
//! and a direct call does not, and a statement whose condition the request cannot decide
//! never grants (the shared evaluator's fail-closed reading).
//!
//! A grant the topic's policy gives the principal BY NAME stands on its own: "resource-based
//! policies that specify the IAM entity as the principal are not limited by the permissions
//! boundary" (IAM, evaluating effective permissions with boundaries), so only a grant that
//! came from the identity-based documents is capped by it.
//!
//! ## What a statement's `Condition` is decided against
//!
//! A statement applies to a request only where its `Condition` holds for that request, so the
//! decision is made with the condition keys the request ANSWERS FOR ([`request_conditions`]):
//! the principal that signed it, and the subscription the request itself names. A conditioned
//! statement then grants exactly the requests it covers and denies exactly those — where
//! without a value for its key the clause cannot be decided at all and the shared evaluator
//! reads it fail-closed, withholding every grant the statement carries.
//!
//! Each key is also DECLARED as one this service models, which is what lets a clause over it
//! be decided for a request that carries NO value for it: this service's own subscription
//! keys have a value only where the call names a subscription, so a statement conditioned on
//! one of them does not apply to a call that names none rather than leaving it undecided.
//!
//! A key this gate answers for nothing — `aws:SourceArn` and `aws:SourceOwner`, which only a
//! request another service makes for one of its resources carries — is left undeclared, so a
//! clause over it stays undecided and is read fail-closed.
//!
//! ## The callers this gate does not bound
//!
//! * **A call another emulated SERVICE makes in-process.** Such a call presents the account's
//!   own published credential and arrives through the framework's in-process callback, which
//!   stamps a chain-depth extension on it that a request from outside this process never
//!   carries ([`is_in_process_call`]) — the principal behind it is a SERVICE, not an identity
//!   of this account, so no document of the account is in force on it and nothing here refuses
//!   it. (A cross-service caller that has a resource policy of its own to satisfy reads it
//!   where that question belongs — the alarm action that publishes to a topic asks the topic's
//!   policy about the CloudWatch service principal, which is not this account's credential.)
//! * **The account's own published credential, presented from outside.** It speaks for the
//!   account itself — the position AWS account root credentials are in — which holds no
//!   identity-based policies, so it needs no grant. What DOES refuse it is an explicit `Deny`
//!   in the resource's own policy that names EVERY principal, which is a statement about every
//!   caller there is, so a topic whose owner has locked it with such a statement takes no call
//!   from anybody. A `Deny` naming an identity of this account, or the account itself, is about
//!   an identity the account holds no record for here, and so does not reach it.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the account's store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no IAM store**, where no user and no policy can exist.
//!
//! Nothing here writes: both stores are only read, each under its own lock and neither held
//! while the other is taken, and the decision is reached before the operation touches its
//! own records.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision, PreparedDocument};

/// This service's IAM action prefix: an SNS action is `SNS:<OperationName>` ("Actions
/// defined by Amazon Simple Notification Service", IAM Service Authorization Reference),
/// which is how every constant below spells one and how a refusal names the action it
/// withheld.
const ACTION_PREFIX: &str = "SNS";

/// The ARN namespace of this service's resources, which is also the name its regional
/// endpoint is built from.
pub(crate) const ARN_NAMESPACE: &str = "sns";

/// The resource part of the ARN that names EVERY resource of this service in one Region of
/// one account.
const EVERY_RESOURCE: &str = "*";

/// The IAM action the operation named `operation` performs.
///
/// Built from the operation's own name rather than listed per operation, because that IS the
/// rule the action namespace follows.
fn action_of(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The IAM action `Publish` performs, and the one `PublishBatch` performs too: the batch
/// call publishes messages, and the SNS action namespace publishes no action of its own for
/// it ("Actions defined by Amazon Simple Notification Service", IAM Service Authorization
/// Reference).
pub const PUBLISH: &str = "SNS:Publish";

/// The IAM action `GetTopicAttributes` performs.
pub const GET_TOPIC_ATTRIBUTES: &str = "SNS:GetTopicAttributes";

/// The IAM action `SetTopicAttributes` performs.
pub const SET_TOPIC_ATTRIBUTES: &str = "SNS:SetTopicAttributes";

/// The IAM action `AddPermission` performs.
pub const ADD_PERMISSION: &str = "SNS:AddPermission";

/// The IAM action `RemovePermission` performs.
pub const REMOVE_PERMISSION: &str = "SNS:RemovePermission";

/// The IAM action `DeleteTopic` performs.
pub const DELETE_TOPIC: &str = "SNS:DeleteTopic";

/// The IAM action `Subscribe` performs.
pub const SUBSCRIBE: &str = "SNS:Subscribe";

/// The IAM action `ListSubscriptionsByTopic` performs.
pub const LIST_SUBSCRIPTIONS_BY_TOPIC: &str = "SNS:ListSubscriptionsByTopic";

/// The IAM action `Unsubscribe` performs.
pub const UNSUBSCRIBE: &str = "SNS:Unsubscribe";

/// The IAM action `SetSubscriptionAttributes` performs.
pub const SET_SUBSCRIPTION_ATTRIBUTES: &str = "SNS:SetSubscriptionAttributes";

/// The IAM action `GetSubscriptionAttributes` performs.
pub const GET_SUBSCRIPTION_ATTRIBUTES: &str = "SNS:GetSubscriptionAttributes";

/// The IAM action `ConfirmSubscription` performs.
pub const CONFIRM_SUBSCRIPTION: &str = "SNS:ConfirmSubscription";

/// The IAM action `TagResource` performs.
pub const TAG_RESOURCE: &str = "SNS:TagResource";

/// The IAM action `UntagResource` performs.
pub const UNTAG_RESOURCE: &str = "SNS:UntagResource";

/// The IAM action `ListTagsForResource` performs.
pub const LIST_TAGS_FOR_RESOURCE: &str = "SNS:ListTagsForResource";

/// The IAM action `GetDataProtectionPolicy` performs.
pub const GET_DATA_PROTECTION_POLICY: &str = "SNS:GetDataProtectionPolicy";

/// The ARN namespace of the service IAM holds its own entities under, which is the namespace
/// of the ARN a principal answers [`PRINCIPAL_ARN_KEY`] with (see [`principal_arn`]).
const IAM_ARN_NAMESPACE: &str = "iam";

/// The global condition keys that name the PRINCIPAL the request was signed by, which this
/// gate answers for every call: "aws:PrincipalArn — use this key to compare the Amazon
/// Resource Name (ARN) of the principal that made the request with the ARN that you specify
/// in the policy" and "aws:PrincipalAccount — the account the principal belongs to" (IAM's
/// global condition keys).
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// This service's OWN condition keys, which name the subscription a request is about:
/// "sns:Endpoint — the URL, email address, or ARN from a `Subscribe` request or a previously
/// confirmed subscription" and "sns:Protocol — the protocol value from a `Subscribe` request
/// or a previously confirmed subscription" (SNS's own condition keys).
const ENDPOINT_KEY: &str = "sns:Endpoint";
const PROTOCOL_KEY: &str = "sns:Protocol";

/// The reason clause for a request an explicit `Deny` in the RESOURCE's own policy refuses:
/// a fact about that document, so the sentence names it rather than the action.
///
/// The shared module publishes the identity-side clauses and this one for the resource side
/// is spelled the way every other service in this tree that refuses on a resource policy
/// spells it (`grep -rl 'with an explicit deny in a resource-based policy' services/*/src`).
const RESOURCE_POLICY_EXPLICIT_DENY: &str = "with an explicit deny in a resource-based policy";

/// The refusal this request earns from the documents in force on it, or `None` when IAM
/// permits it (and for every caller this gate does not bound, see the module docs).
///
/// `action` is the IAM action the operation performs (one of the constants above) and
/// `topic_arn` the ARN of the topic the REQUEST addresses, as the request named it rather
/// than as a stored record resolves it — so the decision does not depend on whether that
/// topic exists, which is what keeps a refusal from telling an unauthorized caller whether
/// it does.
///
/// Call it with this service's own store NOT locked: both stores are reached here, each
/// under its own lock.
#[must_use]
pub fn topic_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    topic_arn: &str,
) -> Option<String> {
    topic_refusal_for(engine, ctx, action, topic_arn, Asked::default())
}

/// [`topic_refusal`] for a call that names a SUBSCRIPTION of the topic, which answers for
/// this service's own condition keys with the values `asked` carries.
///
/// The same decision over the same two documents: all that the subscription adds is the
/// values a statement conditioned on [`ENDPOINT_KEY`] or [`PROTOCOL_KEY`] is decided
/// against, so a grant scoped to one protocol or one endpoint covers exactly the
/// subscriptions it names.
///
/// Call it with this service's own store NOT locked: both stores are reached here, each
/// under its own lock.
#[must_use]
pub fn topic_refusal_for<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    topic_arn: &str,
    asked: Asked<'_>,
) -> Option<String> {
    let policy = topic_policy(engine, topic_arn);
    refusal(
        &caller(engine, ctx),
        action,
        topic_arn,
        policy.as_deref(),
        asked,
    )
}

/// What a call names of the SUBSCRIPTION it is about, which is what this service's own
/// condition keys take their values from.
///
/// Default for a call that names no subscription at all — a publish, a read or a write of the
/// topic itself — which carries no value for either key: "the protocol value from a
/// `Subscribe` request or a previously confirmed subscription" is a value only a call about a
/// subscription has.
#[derive(Debug, Clone, Copy, Default)]
pub struct Asked<'a> {
    /// The subscription's delivery protocol — the value of [`PROTOCOL_KEY`].
    pub protocol: Option<&'a str>,
    /// The subscription's endpoint — the value of [`ENDPOINT_KEY`].
    pub endpoint: Option<&'a str>,
}

/// The refusal a call that addresses a SUBSCRIPTION earns from the documents in force on it,
/// or `None` when IAM permits it (and for every caller this gate does not bound, see the
/// module docs).
///
/// A subscription is addressed through the TOPIC it extends — the topic is the resource type
/// this service's actions are granted on ("Resource types defined by Amazon Simple
/// Notification Service", IAM Service Authorization Reference) — and a subscription's ARN
/// carries its topic's, so the resource the decision is made on is read out of the ARN the
/// REQUEST named: no record has to exist for it, which is what keeps a refusal from telling an
/// unauthorized caller whether the subscription does. `None` for a value that names no
/// subscription at all, which addresses no resource of this service: the operation's own
/// argument validation is what refuses that.
///
/// The subscription's protocol and endpoint, where this endpoint holds the record, are the
/// values this service's own condition keys take for such a call: "the protocol value from a
/// `Subscribe` request or a previously confirmed subscription".
///
/// Call it with this service's own store NOT locked: every store it reads is reached here,
/// each under its own lock.
#[must_use]
pub fn subscription_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    subscription_arn: &str,
) -> Option<String> {
    let topic_arn = subscription_topic_arn(subscription_arn)?;
    let held = subscription_binding(engine, subscription_arn);
    let asked = match &held {
        Some((protocol, endpoint)) => Asked {
            protocol: Some(protocol.as_str()),
            endpoint: endpoint.as_deref(),
        },
        None => Asked::default(),
    };
    topic_refusal_for(engine, ctx, action, &topic_arn, asked)
}

/// The ARN of the TOPIC the subscription `subscription_arn` extends, or `None` when it names no
/// subscription of this service.
///
/// A subscription's ARN is its topic's followed by the subscription's own id segment (as
/// `Subscribe` mints it), so the topic is the head of the value the REQUEST named — read
/// without parsing anything else out of it, and with the head held to the same question every
/// topic ARN of this service is held to.
fn subscription_topic_arn(subscription_arn: &str) -> Option<String> {
    let (topic_arn, id) = subscription_arn.rsplit_once(':')?;
    let names_a_subscription = !id.is_empty() && super::topic_name_from_arn(topic_arn).is_some();
    names_a_subscription.then(|| topic_arn.to_string())
}

/// The delivery protocol and endpoint of the subscription `subscription_arn` names, as this
/// region's store holds them, and `None` when it holds no such subscription.
fn subscription_binding<Eng: Send + Sync + 'static>(
    engine: &Eng,
    subscription_arn: &str,
) -> Option<(String, Option<String>)> {
    let store = super::service_state(engine)?;
    let store = store.lock();
    store
        .subscriptions
        .get(subscription_arn)
        .map(|held| (held.protocol.clone(), held.endpoint.clone()))
}

/// The refusal an ACCOUNT-WIDE call earns from the documents in force on its caller, or
/// `None` when IAM permits it (and for every caller this gate does not bound, see the module
/// docs).
///
/// `operation` is the operation's own name, from which the action it performs is built
/// ([`action_of`]). The resource is the one IAM judges such a call on
/// ([`account_wide_resource`]): the call addresses no resource of its own, so there is
/// nothing of the request to resolve and no resource-based policy in force on it — the
/// decision rests on the caller's identity-based documents and the boundary that caps them.
///
/// Call it with this service's own store NOT locked: the IAM store is read here.
#[must_use]
pub fn account_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Option<String> {
    let resource = account_wide_resource(&super::request_region(ctx.parts()));
    refusal(
        &caller(engine, ctx),
        &action_of(operation),
        &resource,
        None,
        Asked::default(),
    )
}

/// The resource an SNS call that addresses no resource of its own is judged on: every
/// resource of this service in the account and Region the request reached,
/// `arn:<partition>:sns:<region>:<account>:*`.
///
/// A statement grants such a call only by covering that whole set, which is why a policy
/// scoped to one topic's ARN grants nothing account-wide; and it is the resource AWS names in
/// the refusal of such a call. The Region is the one the REQUEST addressed, because that is
/// the endpoint whose resources the call is about.
fn account_wide_resource(region: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, EVERY_RESOURCE)
}

/// The access policy of the topic `topic_arn`, as it reports it — the document its owner set
/// or the one SNS generates for a topic with none — and `None` when this region's store
/// holds no such topic, which is a call with no resource-based policy in force.
fn topic_policy<Eng: Send + Sync + 'static>(engine: &Eng, topic_arn: &str) -> Option<String> {
    let store = super::service_state(engine)?;
    let store = store.lock();
    store
        .topics
        .get(topic_arn)
        .map(super::TopicRecord::policy_attribute)
}

/// The refusal `action` on `resource` earns from the documents in force — `caller`'s own,
/// and the resource policy `resource_policy` carries — or `None` when they permit it.
///
/// IAM's order, which is the shared evaluation's: an explicit `Deny` in either document
/// refuses whatever else is written; otherwise a grant from either suffices; and a grant the
/// identity-based documents gave is capped by the principal's permissions boundary.
///
/// Every statement is weighed under the condition keys this request answers for
/// ([`request_conditions`]), `asked` included, so a conditioned statement applies to exactly
/// the requests its `Condition` holds for.
fn refusal(
    caller: &Caller,
    action: &str,
    resource: &str,
    resource_policy: Option<&str>,
    asked: Asked<'_>,
) -> Option<String> {
    // The call one emulated SERVICE makes into this one in-process: the principal behind it is
    // a service rather than an identity of this account, and no document of the account is in
    // force on it, so there is nothing here to refuse it with (see the module docs).
    if caller.is_a_service_call() {
        return None;
    }
    let conditions = request_conditions(caller, asked);
    // An identity-based document states no `Principal` element, so the principal spellings
    // travel with the request for the RESOURCE policy's sake: that is what its `Principal`
    // element is matched against.
    let asked = iam::Request::new(action, resource, &caller.principals)
        .with_context(&conditions.carried)
        .with_published_keys(&conditions.published);
    let identity = iam::identity_decision(&caller.documents, &asked);
    if identity == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let prepared = resource_policy.map(PreparedDocument::read);
    // A statement that DENIES the account refuses each of the account's principals, so the
    // deny is read with the account's own spellings beside the principal's — except for the
    // account's own published credential, which the account holds no identity record for at
    // all: a statement naming an identity of this account, or the account itself, is about an
    // identity that credential is not, so only a statement denying EVERY principal is about it
    // (see the module docs).
    let denying = if caller.account_credential {
        Vec::new()
    } else {
        let mut delegated = caller.principals.clone();
        delegated.extend(iam::account_principals(&caller.arn));
        delegated
    };
    let denying = iam::Request::new(action, resource, &denying)
        .with_context(&conditions.carried)
        .with_published_keys(&conditions.published);
    let stated_deny = prepared
        .as_ref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_outcome_prepared(document, &denying).decision
        });
    if stated_deny == Decision::Denied {
        // The same sentence the shared module builds for an identity-based refusal: AWS
        // words every authorization failure alike — who, what action, which resource, and
        // what refused it — so only the reason clause differs.
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            RESOURCE_POLICY_EXPLICIT_DENY,
        ));
    }
    // Past the explicit deny, the account's own published credential needs no grant at all: it
    // holds no identity-based documents, so there is nothing left here to refuse it with (see
    // the module docs).
    if caller.account_credential {
        return None;
    }
    // The grant is read with the principal's OWN spellings alone: a statement naming the
    // account delegates to that account's identity-based policies rather than granting.
    let stated_allow = prepared
        .as_ref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_outcome_prepared(document, &asked).decision
        });
    if stated_allow == Decision::Allowed {
        // A grant the resource policy gives the principal by name is not capped by the
        // principal's permissions boundary (see the module docs).
        return None;
    }
    if !(identity == Decision::Allowed || caller.identity_is_unbounded()) {
        // Neither document allows the call, which within one account is IAM's default deny:
        // what the principal is missing is a permission of its OWN, so the sentence names the
        // identity-based policies that did not grant it, the clause AWS states for a
        // same-account caller no policy allows.
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            &iam::no_identity_policy_allows(action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not
    // a permission the principal has. The account's own credential carries none.
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &asked) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(action),
    };
    Some(iam::not_authorized_by_identity_policy(
        &caller.arn,
        action,
        resource,
        &reason,
    ))
}

/// The condition keys THIS request answers for: the values it carries, and the names it
/// DECLARES it models (see the module docs).
#[derive(Debug, Default)]
struct RequestConditions {
    /// The `(key, value)` pairs the request carries, one pair per value.
    carried: Vec<(String, String)>,
    /// The names the request models, whether or not it carries a value for each.
    published: Vec<&'static str>,
}

/// The condition keys this request answers for.
///
/// Both of this service's own keys are declared whatever the call is, because every call
/// either names a subscription or names none; the principal keys are declared for a caller
/// that has a principal spelling at all, and the account is read out of the principal's own
/// ARN through the tree's one ARN parse, so an ARN carrying none declares no account key
/// rather than a guess.
fn request_conditions(caller: &Caller, asked: Asked<'_>) -> RequestConditions {
    let mut carried = Vec::new();
    let mut published = vec![ENDPOINT_KEY, PROTOCOL_KEY];
    if let Some(principal) = principal_arn(caller) {
        carried.push((PRINCIPAL_ARN_KEY.to_string(), principal.to_string()));
        published.push(PRINCIPAL_ARN_KEY);
        let account = ::service_common::arn::parse(principal)
            .map(|parsed| parsed.account)
            .filter(|account| !account.is_empty());
        if let Some(account) = account {
            carried.push((PRINCIPAL_ACCOUNT_KEY.to_string(), account.to_string()));
            published.push(PRINCIPAL_ACCOUNT_KEY);
        }
    }
    // An empty value is no value: a call that names no protocol or no endpoint carries none
    // for that key, which is what the key's declaration above lets a clause decide.
    if let Some(protocol) = asked.protocol.filter(|value| !value.is_empty()) {
        carried.push((PROTOCOL_KEY.to_string(), protocol.to_string()));
    }
    if let Some(endpoint) = asked.endpoint.filter(|value| !value.is_empty()) {
        carried.push((ENDPOINT_KEY.to_string(), endpoint.to_string()));
    }
    RequestConditions { carried, published }
}

/// The ARN this caller answers [`PRINCIPAL_ARN_KEY`] with, or `None` for a caller that has no
/// principal spelling at all.
///
/// The ARN of the IAM ENTITY behind the credential: "for IAM roles, the request context
/// returns the ARN of the role, not the ARN of the user that assumed the role" (IAM's global
/// condition keys) — which among the spellings the caller is known by is the one in IAM's own
/// namespace, the session ARN a temporary credential speaks under being STS's. A caller known
/// by no IAM spelling answers with the first spelling it has, which is the principal
/// `sts:GetCallerIdentity` reports.
fn principal_arn(caller: &Caller) -> Option<&str> {
    caller
        .principals
        .iter()
        .map(String::as_str)
        .find(|arn| {
            ::service_common::arn::parse(arn)
                .is_some_and(|parsed| parsed.vendor == IAM_ARN_NAMESPACE)
        })
        .or_else(|| caller.principals.first().map(String::as_str))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal a policy's `Principal` element can name it by: the
    /// session ARN a temporary credential speaks under, and the ARN of the IAM role behind
    /// it.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when this request arrived through the IN-PROCESS callback one emulated service
    /// calls another with, rather than from a client outside the emulator (see
    /// [`is_in_process_call`]).
    in_process_call: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Whether this is the call one emulated SERVICE makes into this one in-process: the
    /// account's own published credential, presented by a request that came from inside this
    /// process. No document of this account is in force on such a call (see the module docs).
    fn is_a_service_call(&self) -> bool {
        self.account_credential && self.in_process_call
    }
}

/// Whether the request arrived through the IN-PROCESS callback one emulated service calls
/// another with, rather than from a client outside the emulator.
///
/// Such a call carries the chain-depth extension the framework stamps on every callback
/// ([`::smithy_server_runtime::CallbackRecursionDepth`]), and "an external request carries
/// none" — so it is the one thing about a request a caller outside this process cannot state
/// for itself. The stamp is always the NEXT depth of the chain, so it is never zero.
///
/// The crate's ONE reading of where a request came from: this gate asks it to tell the call one
/// emulated service makes apart from a client's own ([`Caller::is_a_service_call`]), and the tag
/// rules ask it to tell the tags Amazon Web Services applies from inside itself apart from a
/// caller's ([`super::tags::tag_key_origin`]), so the two cannot answer differently.
pub(crate) fn is_in_process_call(ctx: &::smithy_server_runtime::OperationContext<'_>) -> bool {
    ctx.parts()
        .extensions
        .get::<::smithy_server_runtime::CallbackRecursionDepth>()
        .is_some_and(|depth| depth.0 > 0)
}

/// Who this request speaks for, resolved from the access key id it signed with.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let in_process = is_in_process_call(ctx);
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(iam) = iam_store::service_state(engine) else {
        return account_caller(in_process);
    };
    let store = iam.lock();
    let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());
    // A TEMPORARY credential of a role session: the principal is the SESSION, and the
    // policies in force are the ROLE's ("the temporary security credentials … have the
    // permissions of the role", `AssumeRole`). The role's own ARN is a spelling a policy
    // names the caller by too.
    if let Some(session) = store.role_session(&access_key_id, now) {
        let mut caller = Caller {
            arn: session.session_arn.clone(),
            principals: vec![session.session_arn.clone(), session.role_arn.clone()],
            ..Caller::default()
        };
        let role_arn = session.role_arn.clone();
        read_policies_of(&store, &role_arn, &mut caller);
        return caller;
    }
    // A LONG-TERM access key speaks for the IAM user that holds it, which is the identity
    // `sts:GetCallerIdentity` reports for it; a key the account holds no record for is the
    // account's own published credential.
    let user_arn = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
        .map(|user| user.arn.clone());
    match user_arn {
        Some(user_arn) => {
            let mut caller = Caller {
                arn: user_arn.clone(),
                principals: vec![user_arn.clone()],
                ..Caller::default()
            };
            read_policies_of(&store, &user_arn, &mut caller);
            caller
        }
        None => account_caller(in_process),
    }
}

/// The caller a request presenting the account's own published credential speaks for: the
/// principal `sts:GetCallerIdentity` reports for it, which the IAM store holds no policies
/// for (see the module docs). `in_process` says whether that request came from inside this
/// process ([`is_in_process_call`]).
fn account_caller(in_process: bool) -> Caller {
    let arn = ::service_common::caller_principal::ambient_user_arn();
    Caller {
        arn: arn.clone(),
        principals: vec![arn],
        account_credential: true,
        in_process_call: in_process,
        ..Caller::default()
    }
}

/// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
/// accessors: the documents in force on it, whether it carries an attachment whose document
/// this emulator cannot read, and the permissions boundary that caps it.
fn read_policies_of(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    identity_arn: &str,
    caller: &mut Caller,
) {
    if let Some(policies) = store.principal_policies(identity_arn) {
        caller.documents = policies.documents;
        // An attachment the collection resolved no document for is an entry of AWS's own
        // catalog this emulator does not carry: a grant it cannot read.
        caller.unreadable_attachment = !policies.uncarried.is_empty();
    }
    caller.boundary = store.permissions_boundary_document(identity_arn);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A topic of this account, as every ARN this service mints spells one.
    fn topic_arn() -> String {
        super::super::topic_arn("us-east-1", "gate-topic")
    }

    /// An IAM user of this account, with the identity-based documents `documents`.
    fn user(documents: &[&str]) -> Caller {
        let arn = ::emulator_server::emulator_arn("iam", "", "user/gate-user");
        Caller {
            arn: arn.clone(),
            principals: vec![arn],
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            ..Caller::default()
        }
    }

    /// RULE: a principal whose identity-based documents say nothing is refused a topic whose
    /// policy grants it nothing, and the same call is allowed once either document allows
    /// the action on that topic.
    #[test]
    fn a_grant_from_either_document_allows_and_neither_refuses() {
        let topic = topic_arn();
        let principal = user(&[]).arn;
        let identity_allow =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:Publish","Resource":"{topic}"}}]}}"#);
        let resource_allow = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{principal}"}},
            "Action":"SNS:Publish","Resource":"{topic}"}}]}}"#
        );
        let default_policy =
            super::super::default_topic_policy(&topic, &super::super::owner_account_id()).to_json();
        // (case, the identity documents, the topic policy, whether the call is allowed)
        let rows: &[(&str, &[&str], Option<&str>, bool)] = &[
            ("no document at all", &[], None, false),
            (
                "the policy SNS generates for a topic with none",
                &[],
                Some(&default_policy),
                false,
            ),
            (
                "an identity-based grant",
                &[&identity_allow],
                Some(&default_policy),
                true,
            ),
            (
                "a grant in the topic's own policy",
                &[],
                Some(&resource_allow),
                true,
            ),
        ];
        for (case, documents, policy, expected) in rows {
            let refused = refusal(&user(documents), PUBLISH, &topic, *policy, Asked::default());
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: an explicit `Deny` wins, from either document, whatever the other allows.
    #[test]
    fn an_explicit_deny_in_either_document_refuses() {
        let topic = topic_arn();
        let principal = user(&[]).arn;
        let identity_allow =
            r#"{"Statement":[{"Effect":"Allow","Action":"sns:*","Resource":"*"}]}"#.to_string();
        let identity_deny = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Action":"sns:Publish","Resource":"{topic}"}}]}}"#
        );
        let resource_deny = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{principal}"}},
            "Action":"SNS:Publish","Resource":"{topic}"}}]}}"#
        );
        let refused = refusal(
            &user(&[&identity_allow, &identity_deny]),
            PUBLISH,
            &topic,
            None,
            Asked::default(),
        );
        assert_eq!(
            refused,
            Some(iam::not_authorized_by_identity_policy(
                &principal,
                PUBLISH,
                &topic,
                iam::IDENTITY_POLICY_EXPLICIT_DENY,
            )),
            "case \"an identity-based deny beside a wildcard allow\": unexpected answer"
        );
        let refused = refusal(
            &user(&[&identity_allow]),
            PUBLISH,
            &topic,
            Some(&resource_deny),
            Asked::default(),
        );
        assert_eq!(
            refused,
            Some(iam::not_authorized_by_identity_policy(
                &principal,
                PUBLISH,
                &topic,
                RESOURCE_POLICY_EXPLICIT_DENY,
            )),
            "case \"a topic-policy deny beside an identity-based allow\": unexpected answer"
        );
    }

    /// RULE: the action asked about is the one the operation performs, so a document scoped
    /// to one action decides that action alone — and SNS's own spelling of an action and
    /// IAM's differ only in case, which IAM compares case-insensitively.
    #[test]
    fn a_grant_is_read_for_the_action_it_names() {
        let topic = topic_arn();
        let document =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:publish","Resource":"{topic}"}}]}}"#);
        // (case, the action asked about, whether the grant covers it)
        let rows: &[(&str, &str, bool)] = &[
            ("the granted action", PUBLISH, true),
            ("another action on the same topic", SUBSCRIBE, false),
            ("a read of the same topic", GET_TOPIC_ATTRIBUTES, false),
            ("a write of the same topic", SET_TOPIC_ATTRIBUTES, false),
        ];
        for (case, action, expected) in rows {
            let refused = refusal(&user(&[&document]), action, &topic, None, Asked::default());
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: the grant has to name the resource the request addresses, so a grant on one
    /// topic allows nothing on another.
    #[test]
    fn a_grant_is_read_for_the_resource_it_names() {
        let granted = super::super::topic_arn("us-east-1", "granted-topic");
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:Publish","Resource":"{granted}"}}]}}"#
        );
        // (case, the topic addressed, whether the grant covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the granted topic", granted.clone(), true),
            (
                "another topic of the same account",
                super::super::topic_arn("us-east-1", "other-topic"),
                false,
            ),
            (
                "the same name in another region",
                super::super::topic_arn("eu-west-1", "granted-topic"),
                false,
            ),
        ];
        for (case, topic, expected) in rows {
            let refused = refusal(&user(&[&document]), PUBLISH, topic, None, Asked::default());
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: the account's own published credential — the one an emulated service signs its
    /// in-process calls with — holds no identity-based document and needs no grant, so a topic
    /// that merely grants it nothing does not refuse it and a statement about an identity of
    /// this account does not reach it; an explicit `Deny` naming EVERY principal refuses it
    /// where the request came from OUTSIDE the emulator, while the in-process call one emulated
    /// service makes into this one has no document of this account in force on it at all.
    #[test]
    fn the_accounts_own_credential_is_refused_only_by_a_deny_of_every_principal() {
        let topic = topic_arn();
        let denied_everyone = r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"*",
            "Resource":"*"}]}"#
            .to_string();
        let account_root =
            format!("arn:aws:iam::{}:root", super::super::owner_account_id());
        let denied_account = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{account_root}"}},
            "Action":"*","Resource":"*"}}]}}"#
        );
        let granted_nobody = r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":
            "arn:aws:iam::000000000000:role/someone-else"},"Action":"*","Resource":"*"}]}"#
            .to_string();
        // (case, the topic's policy, whether the call is refused)
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("a topic with no policy at all", None, false),
            (
                "a topic policy granting this caller nothing",
                Some(&granted_nobody),
                false,
            ),
            (
                "a topic policy denying the account's own identities",
                Some(&denied_account),
                false,
            ),
            (
                "a topic policy denying every principal every action",
                Some(&denied_everyone),
                true,
            ),
        ];
        for (case, policy, expected) in rows {
            for action in [PUBLISH, GET_TOPIC_ATTRIBUTES, DELETE_TOPIC] {
                // From OUTSIDE the emulator, where the credential speaks for the account.
                let refused = refusal(
                    &account_caller(false),
                    action,
                    &topic,
                    *policy,
                    Asked::default(),
                );
                assert_eq!(
                    refused.is_some(),
                    *expected,
                    "case {case:?} asking {action}: expected refused == {expected}, got \
                     {refused:?}"
                );
                // The same credential presented by the IN-PROCESS call one emulated service
                // makes into this one: no document of this account is in force on it, so not
                // even a deny of every principal refuses it.
                let refused = refusal(
                    &account_caller(true),
                    action,
                    &topic,
                    *policy,
                    Asked::default(),
                );
                assert_eq!(
                    refused, None,
                    "case {case:?} asking {action} in-process: expected the service call to be \
                     allowed, got {refused:?}"
                );
            }
        }
    }

    /// The operations whose IAM action supports no resource-level permissions, so that each
    /// is judged over the account's whole SNS resource: the account's own listings, its SMS
    /// settings and its SMS-sandbox membership. Source: these are the operations of
    /// `smithy_query(query="digest")` whose input carries no ARN member, so there is no
    /// resource of the request to judge the call on.
    const ACCOUNT_WIDE_OPERATIONS: &[&str] = &[
        "GetSMSSandboxAccountStatus",
        "ListTopics",
        "ListSubscriptions",
        "GetSMSAttributes",
        "SetSMSAttributes",
        "ListPhoneNumbersOptedOut",
        "VerifySMSSandboxPhoneNumber",
    ];

    /// RULE: a call that addresses no resource of its own is judged on the ARN naming every
    /// SNS resource of the account and Region it reached, and an explicit `Deny` for its
    /// action overrides any `Allow` beside it.
    #[test]
    fn an_account_wide_call_is_judged_on_the_accounts_own_sns_resource() {
        for region in ["us-east-1", "us-west-2", "eu-west-1"] {
            let resource = account_wide_resource(region);
            assert_eq!(
                resource,
                format!(
                    "arn:aws:sns:{region}:{}:*",
                    super::super::owner_account_id()
                ),
                "region {region:?}: expected the account's whole SNS resource, got {resource:?}"
            );
        }
        let region = "us-west-2";
        let resource = account_wide_resource(region);
        for operation in ACCOUNT_WIDE_OPERATIONS {
            let action = action_of(operation);
            let allow =
                format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:{operation}","Resource":"*"}}]}}"#);
            let deny =
                format!(r#"{{"Statement":[{{"Effect":"Deny","Action":"sns:{operation}","Resource":"*"}}]}}"#);
            let other = r#"{"Statement":[{"Effect":"Allow","Action":"sns:Publish",
                "Resource":"*"}]}"#
                .to_string();
            let topic_scoped = format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:{operation}",
                "Resource":"{}"}}]}}"#,
                super::super::topic_arn(region, "one-topic")
            );
            // (case, the caller's identity documents, the refusal expected of them)
            let rows: &[(&str, &[&str], Option<String>)] = &[
                (
                    "no document at all",
                    &[],
                    Some(iam::no_identity_policy_allows(&action)),
                ),
                ("a grant for this action", &[&allow], None),
                (
                    "an explicit deny beside the grant",
                    &[&allow, &deny],
                    Some(iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()),
                ),
                (
                    "a grant for another action only",
                    &[&other],
                    Some(iam::no_identity_policy_allows(&action)),
                ),
                (
                    "a grant scoped to one topic's ARN",
                    &[&topic_scoped],
                    Some(iam::no_identity_policy_allows(&action)),
                ),
            ];
            for (case, documents, reason) in rows {
                let caller = user(documents);
                let refused = refusal(&caller, &action, &resource, None, Asked::default());
                let expected = reason.as_deref().map(|reason| {
                    iam::not_authorized_by_identity_policy(
                        &caller.arn,
                        &action,
                        &resource,
                        reason,
                    )
                });
                assert_eq!(
                    refused, expected,
                    "case {case:?} asking {action}: unexpected answer"
                );
            }
        }
    }
}
