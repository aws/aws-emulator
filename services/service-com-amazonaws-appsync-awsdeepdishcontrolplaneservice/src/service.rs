// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appsync-awsdeepdishcontrolplaneservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{http, ErasedOperation, Operation, Refusal, Refused, Service};
use std::collections::HashMap;
use std::sync::Arc;

/// Whether IAM lets a request perform the action it asks for — the decision taken in front
/// of every operation this service publishes, and the layer ([`authorization::Gated`]) that
/// takes it there.
pub mod authorization;

/// The Event API's DATA plane: the two sockets each `Api` is served on — the HTTP endpoint
/// a publisher POSTs to and the real-time endpoint a subscriber opens a WebSocket against
/// — which are what the hostnames in `Api$dns` publish. They are SOCKETS rather than
/// routes of this service, because the seams this file carries take one buffered request
/// and return one buffered response: see that module for why, and for what each endpoint
/// answers.
pub mod event_api;

/// How the two GAUGES a real-time plane reports — the connections open on an API and the
/// subscriptions they hold — are observed on a cadence while that API is connected to.
/// Shared by both planes, because a gauge of either is a quantity with a value at every
/// instant rather than a count of events.
pub mod gauge_sampling;

/// The media type the SCHEMA an introspection read answers with is labelled: the one
/// response of this service whose body is not the restJson1 envelope its own serializer
/// labels.
pub mod schema_media_type;

/// A GraphQL API's REAL-TIME plane: the socket each `GraphqlApi`'s `REALTIME` endpoint is
/// served on — the address `GraphqlApi$uris` publishes for a subscriber to dial — which is
/// a SOCKET rather than a route of this service for the same reason an Event API's
/// endpoints are.
pub mod graphql_api;

/// AppSync's own modelled error for a request that is not well formed:
/// `BadRequestException`, `httpError` 400, with members `message`/`reason`/`detail`
/// (`smithy_query(query="shape", shape="BadRequestException")`). Every operation of
/// this service models it, and no operation models `ValidationException`.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The error code the framework names a boundary refusal with on this service's
/// restJson1 envelope (`json` in `vendor/smithy-server-runtime/src/pre_dispatch.rs`),
/// repeated here because [`Refused`] replaces the code and the message TOGETHER: the
/// refusals below that only adjust the WORDING must keep answering under the code the
/// caller would otherwise have seen.
///
/// It is kept for ONE class of refusal: a value outside an enumerated type's value set,
/// which the corpus records this service answering under this code
/// ([`without_rejected_enum_values`]) — except for the two enumerated members it records
/// under its own code, the `format` a report is asked in ([`format_refusal`]) and the
/// `runtime.name` a request names its code's runtime by ([`runtime_name_refusal`]). Every
/// other constraint class answers under the service's own [`BAD_REQUEST_CODE`].
const FRAMEWORK_REFUSAL_CODE: &str = "ValidationException";

/// Every member this service's model constrains with `@range`, with the bounds the
/// shape it targets declares and the sentence this service states a value outside them
/// in, keyed by the member name a violation path ends in.
///
/// Source of the bounds: `smithy_query(query="shape", shape="MaxResults,MaxBatchSize,
/// QueryDepthLimit,ResolverCountLimit")` — `MaxResults` 0..25 (every list operation's
/// `maxResults`), `MaxBatchSize` 0..2000 (`CreateResolver`/`UpdateResolver`/
/// `CreateFunction`/`UpdateFunction`), `QueryDepthLimit` 0..75 and
/// `ResolverCountLimit` 0..10000 (`CreateGraphqlApi`/`UpdateGraphqlApi`). The one other
/// `@range` shape of this model, `TTL` 0..3600 (a Lambda authorizer's cached-result
/// lifetime), is stated in the service's own words for that limit
/// ([`authorizer_ttl_refusal`]) rather than in one of the forms below; every remaining
/// numeric member targets an unconstrained `Long`, so no other member can produce a
/// range violation.
///
/// The SENTENCE is the service's vocabulary rather than the model's, the way
/// [`REFUSAL_LABELS`] is: AppSync does not state every limit alike — it states a batch
/// size as a value that must lie between the bounds, a page size as one that must be
/// greater than the minimum and at most the maximum, and the two API-level limits as a
/// range the value must fall within — so each member carries the form the service states
/// it in ([`RangeSentence`]) and the bounds in that sentence are filled from this table.
///
/// FOLLOW-UP: these bounds are copied out of the model rather than read from it,
/// because a refusal reaches this file as a [`Violation`] whose
/// `ViolationKind::MinValue`/`MaxValue` carries only the bound that was CROSSED
/// (`vendor/smithy-schema-server/src/constraints.rs`), and the message states both.
/// Carrying the whole declared range on those variants would let the message be
/// computed from the model at refusal time and make this table unnecessary; that is a
/// change to the vendored framework, which this fix cannot make. Until then a member
/// absent from the table is named alone rather than with invented bounds, so drift
/// drops the range clause instead of stating a wrong one.
const RANGED_MEMBERS: &[(&str, i64, i64, RangeSentence)] = &[
    ("maxBatchSize", 0, 2_000, RangeSentence::ValueBetween),
    ("maxResults", 0, 25, RangeSentence::GreaterThanAndAtMost),
    ("queryDepthLimit", 0, 75, RangeSentence::WithinRange),
    ("resolverCountLimit", 0, 10_000, RangeSentence::WithinRange),
];

/// The forms this service states a value outside a member's modelled `@range` in — one
/// per member ([`RANGED_MEMBERS`]), because the service words its limits differently and
/// each member's wording is a requirement of the refusal.
#[derive(Clone, Copy, Debug)]
enum RangeSentence {
    /// The value has to lie between the two bounds, with both written plainly: "Invalid
    /// value for `<member>`. Must be between `<min>` and `<max>` (inclusive)".
    ValueBetween,
    /// The value has to be above the minimum and at most the maximum: "`<member>` must be
    /// greater than `<min>` and less than or equal to `<max>`."
    GreaterThanAndAtMost,
    /// The value has to fall within the range, whose bounds are written with their
    /// thousands grouped ([`grouped`]): "Invalid `<member>` given. Must fall within range
    /// `<min>` to `<max>`".
    WithinRange,
}

/// The sentence this service states a `label` outside `min..=max` in.
fn range_message(label: &str, min: i64, max: i64, sentence: RangeSentence) -> String {
    match sentence {
        RangeSentence::ValueBetween => {
            format!("Invalid value for {label}. Must be between {min} and {max} (inclusive)")
        }
        RangeSentence::GreaterThanAndAtMost => {
            format!("{label} must be greater than {min} and less than or equal to {max}.")
        }
        RangeSentence::WithinRange => format!(
            "Invalid {label} given. Must fall within range {} to {}",
            grouped(min),
            grouped(max)
        ),
    }
}

/// The members AppSync's own refusal labels differently from the request member that
/// carries them.
///
/// This is service vocabulary, not model data: nothing in the model states it, and it
/// comes from the service's observed wording — `resolverCountLimit` is reported as
/// `resolverLimit` ("Invalid resolverLimit given. Must fall within range 0 to
/// 10,000") and `operationLevelMetricsConfig` as `operationLevelMetricConfig`
/// ("Invalid operationLevelMetricConfig given."), a label short of the member's
/// plural. A member absent from this table is named exactly as the model names it,
/// which is what every other refusal above shows.
const REFUSAL_LABELS: &[(&str, &str)] = &[
    ("operationLevelMetricsConfig", "operationLevelMetricConfig"),
    ("resolverCountLimit", "resolverLimit"),
];

/// The member a violation path names: its last segment, so a nested member
/// (`enhancedMetricsConfig.dataSourceLevelMetricsBehavior`) is named the way AppSync
/// names it in a refusal — by the member, not by the path it sits on.
fn violated_member(path: &str) -> &str {
    path.split('.').next_back().unwrap_or(path)
}

/// The label AppSync's refusal names `member` by ([`REFUSAL_LABELS`]).
fn refusal_label(member: &str) -> &str {
    REFUSAL_LABELS
        .iter()
        .find(|(name, _)| *name == member)
        .map_or(member, |(_, label)| *label)
}

/// `value` with its digits grouped in threes by commas, the way AppSync renders a
/// bound in a refusal ("Must fall within range 0 to 10,000").
fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (position, digit) in digits.char_indices() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// AppSync's own refusal for a member whose value the model rules out outright — an
/// absent required member, or a value outside the member's modelled `@range`: the
/// service's `BadRequestException`, naming the offending member, and the whole
/// modelled range rather than the single bound that was crossed, in the sentence the
/// service states THAT member's range in ([`RANGED_MEMBERS`]).
///
/// The framework's generic report would name `ValidationException`, a shape no
/// operation of this service models, so a client matching on `BadRequestException`
/// (and reading its `reason`/`detail`) would never fire. The message states ONE
/// member — the first the walk refused — where the framework's aggregates every
/// clause, because that is how the service states it.
///
/// `None` when no violation is of either kind; a length, a pattern or an enum value
/// set then keeps the framework's wording (see [`without_rejected_enum_values`]).
fn member_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        let member = violated_member(&violation.path);
        let label = refusal_label(member);
        let range = RANGED_MEMBERS.iter().find(|(name, ..)| *name == member);
        let message = match violation.kind {
            ViolationKind::MissingRequiredMember => format!("Invalid {label} given."),
            ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) => match range {
                Some((_, min, max, sentence)) => range_message(label, *min, *max, *sentence),
                // A range this model does not declare cannot be stated; name the
                // member alone rather than inventing bounds for it.
                None => format!("Invalid {label} given."),
            },
            _ => return None,
        };
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message,
        })
    })
}

/// The member a Lambda authorizer's cached-result lifetime arrives on:
/// `LambdaAuthorizerConfig$authorizerResultTtlInSeconds`, "the number of seconds a response
/// should be cached for", targeting `TTL` — `@range` 0..3600, "the maximum value is one hour
/// (3600 seconds)" (`smithy_query(query="shape", shape="LambdaAuthorizerConfig,TTL")`). It is
/// reached as that member of a `CreateGraphqlApi` / `UpdateGraphqlApi` request's own
/// `lambdaAuthorizerConfig` and of each of its `additionalAuthenticationProviders`, which is
/// why the member is matched by name rather than by the whole path.
const AUTHORIZER_RESULT_TTL_MEMBER: &str = "authorizerResultTtlInSeconds";

/// AppSync's own refusal for a cached-result lifetime outside the window `TTL` declares:
/// its `BadRequestException`, stating the LIFETIME and the bound the value crossed, with the
/// comparison that was broken — "Authorizer result TTL cannot be > 3600".
///
/// The service states this limit in its own words rather than in the form it states a page
/// size or a batch size in ([`RANGED_MEMBERS`]): the sentence names the lifetime as the
/// service names it, not the request member, and carries the ONE bound the value crossed
/// rather than the whole window — which is the value a caller has to move to, and is taken
/// from the violation, so the bound stated is always the bound the model declares.
///
/// `None` when no violation is that member's range, which leaves every other member to the
/// sentence the rest of this file states for it.
fn authorizer_ttl_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violated_member(&violation.path) != AUTHORIZER_RESULT_TTL_MEMBER {
            return None;
        }
        let crossed = match &violation.kind {
            ViolationKind::MaxValue(max) => format!("> {max}"),
            ViolationKind::MinValue(min) => format!("< {min}"),
            _ => return None,
        };
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: format!("Authorizer result TTL cannot be {crossed}"),
        })
    })
}

/// The member every operation of this service names a reporting format in: the
/// `OutputType` `GetIntrospectionSchema` renders a schema under and the
/// `TypeDefinitionFormat` every type operation reports a definition in
/// (`smithy_query(query="shape", shape="OutputType,TypeDefinitionFormat")` — both
/// enumerate `SDL` and `JSON`).
const FORMAT_MEMBER: &str = "format";

/// AppSync's own refusal for a `format` whose value is outside the enumerated set the
/// member's type declares: its `BadRequestException`, naming the member and the rejected
/// value — the one value the caller has to change, and the one the service does echo.
///
/// The framework's generic report would name `ValidationException`, a shape no operation
/// of this service models, so a client matching on `BadRequestException` would never
/// fire. Every OTHER member whose enumerated value set a request falls outside keeps the
/// framework's wording with the rejected value lifted out
/// ([`without_rejected_enum_values`]).
fn format_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violated_member(&violation.path) != FORMAT_MEMBER
            || !matches!(violation.kind, ViolationKind::EnumValue(_))
        {
            return None;
        }
        let value = violation.value.as_deref()?;
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: format!("Invalid {FORMAT_MEMBER} {value} given"),
        })
    })
}

/// The order this service recites the values of `AuthenticationType` in when a refusal
/// states that member's permitted set.
///
/// It is the SERVICE's order rather than the model's: the model declares `API_KEY`,
/// `AWS_IAM`, `AMAZON_COGNITO_USER_POOLS`, `OPENID_CONNECT`, `AWS_LAMBDA`
/// (`smithy_query(query="shape", shape="AuthenticationType")`), and a recorded refusal of
/// an authorization mode recites them in the order below — so the permitted set a clause
/// states is rendered from here, as the whole set, and nothing of the model's own order
/// reaches the wire.
const RECITED_AUTHENTICATION_TYPES: &[&str] = &[
    "AWS_LAMBDA",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_IAM",
    "API_KEY",
];

/// Every enumerated type whose permitted set this service recites in an order of its own,
/// each as the WHOLE set in that order.
///
/// A clause's set is matched to an entry by the values it holds rather than by the member
/// it is stated at, because the violation carries the value set and not the shape that
/// declared it; an enum absent from here keeps the order the model declares.
const RECITED_VALUE_SETS: &[&[&str]] = &[RECITED_AUTHENTICATION_TYPES];

/// The order this service traverses a structure's members in when it recites ONE clause
/// per offending member.
///
/// The framework's walk recites them in the order the model declares the members, where
/// the service recites them in the order it traverses its own: a recorded `CreateApi`
/// refusal of all three of an `EventConfig`'s authorization-mode members states the
/// publishing modes first, then the connection modes, then the subscription modes, where
/// the model declares `connectionAuthModes` before either; and the recorded
/// `StartDataSourceIntrospection` refusals of two of an `RdsDataApiConfig`'s members state
/// them in the REVERSE of the order the model declares them — the DATABASE the
/// introspection would read, then the SECRET that logs in to it, then the CLUSTER its ARN
/// reaches — where the model declares `resourceArn`, `secretArn`, `databaseName`. A member
/// absent from here keeps its place in the walk's order, after the members named below,
/// which is why only the members that MOVE are named.
const RECITED_MEMBERS: &[&str] = &[
    "defaultPublishAuthModes",
    "connectionAuthModes",
    "defaultSubscribeAuthModes",
    "databaseName",
    "secretArn",
];

/// Where `path`'s member sits in the order this service recites members in
/// ([`RECITED_MEMBERS`]), or past every named one when it names none of them.
///
/// Every segment of the path is read, so a member reached through a list or a map
/// (`eventConfig.defaultPublishAuthModes.1.member.authType`) is ranked by the member whose
/// value broke, not by the leaf the walk reports it at.
fn recited_rank(path: &str) -> usize {
    path.split('.')
        .filter_map(|segment| RECITED_MEMBERS.iter().position(|named| *named == segment))
        .min()
        .unwrap_or(RECITED_MEMBERS.len())
}

/// The CONFIGURATION whose own members this service recites EXPRESSION-FIRST: a value that
/// breaks both the FORM its member declares and its SIZE is stated as the form it has to
/// have and then the bound it crossed, where the framework's walk states the crossed bound
/// first.
///
/// Keyed by the configuration the members sit on, because the order is a property of the
/// surface that validates them rather than of a constraint class: a recorded
/// `StartDataSourceIntrospection` refusal of a secret ARN that is both shorter than its
/// minimum and no secret ARN at all states the expression clause ahead of the length one,
/// and the other surfaces of this service state the crossed bound first. It is the same
/// surface whose clauses quote the value they refused, where the others withhold it.
const EXPRESSION_FIRST_CONFIG: &str = "rdsDataApiConfig";

/// Where `violation` sits among the clauses of ONE member: `0` unless this service recites
/// that member's clauses expression-first ([`EXPRESSION_FIRST_CONFIG`]), in which case the
/// `@pattern` clause ranks ahead of every other class.
///
/// `0` for everything else, so the stable sort leaves a member's clauses in the order the
/// framework's walk found them.
fn recited_class_rank(violation: &Violation) -> usize {
    let recited_expression_first = violation
        .path
        .split('.')
        .next()
        .is_some_and(|member| member == EXPRESSION_FIRST_CONFIG);
    if !recited_expression_first || matches!(violation.kind, ViolationKind::Pattern(_)) {
        return 0;
    }
    1
}

/// `violations` as this service recites them: one clause per offending member in the order
/// the service traverses those members ([`RECITED_MEMBERS`]), the clauses of ONE member in
/// the order the surface carrying it states its constraint classes
/// ([`recited_class_rank`]), and each enumerated clause's permitted set in the order the
/// service recites that enum's values ([`RECITED_VALUE_SETS`]).
///
/// The clauses themselves, and the envelope around them, stay the framework's: only the
/// ORDER of the report is the service's. The sort is stable, so clauses of members this
/// service recites no order for keep the walk's own.
fn recited(violations: &[Violation]) -> Vec<Violation> {
    let mut stated: Vec<Violation> = violations
        .iter()
        .map(|violation| {
            let mut violation = violation.clone();
            if let ViolationKind::EnumValue(allowed) = &violation.kind {
                if let Some(order) = recited_value_set(allowed) {
                    violation.kind = ViolationKind::EnumValue(
                        order.iter().map(|value| (*value).to_string()).collect(),
                    );
                }
            }
            violation
        })
        .collect();
    stated.sort_by_key(|violation| {
        (
            recited_rank(&violation.path),
            recited_class_rank(violation),
        )
    });
    stated
}

/// The order this service recites `allowed` in, when `allowed` is the whole value set of
/// an enum it recites in an order of its own.
fn recited_value_set(allowed: &[String]) -> Option<&'static [&'static str]> {
    RECITED_VALUE_SETS.iter().copied().find(|recited| {
        recited.len() == allowed.len()
            && recited
                .iter()
                .all(|value| allowed.iter().any(|held| held == value))
    })
}

/// The member path a request names the RUNTIME of the code it carries by: `AppSyncRuntime`
/// is reached as the `runtime` member of every operation that takes code
/// (`smithy_query(query="shape", shape="AppSyncRuntime")`), and its `name` targets the
/// enumerated `RuntimeName`.
const RUNTIME_NAME_PATH: &str = "runtime.name";

/// AppSync's own refusal for a `runtime.name` outside the enumerated set `RuntimeName`
/// declares: its `BadRequestException`, carrying the same aggregated constraint report the
/// framework states — the member path and the permitted values, the rejected value withheld
/// ([`without_rejected_enum_values`]) — under the code the operations that take a runtime
/// model for a request that is not well formed.
///
/// The DECISION and the sentence are the framework's; only the CODE is restated, because
/// `ValidationException` is a shape no operation of this service models, so a client's
/// typed-error dispatch over the runtime it named would never fire.
///
/// `None` when no violation is that member's value set, which leaves every other
/// enumerated member to the framework's own code ([`without_rejected_enum_values`]).
fn runtime_name_refusal(violations: &[Violation]) -> Option<Refused> {
    let names_the_runtime = violations.iter().any(|violation| {
        violation.path == RUNTIME_NAME_PATH && matches!(violation.kind, ViolationKind::EnumValue(_))
    });
    if !names_the_runtime {
        return None;
    }
    let stated = without_rejected_enum_values(violations)?;
    Some(Refused {
        code: BAD_REQUEST_CODE.to_string(),
        ..stated
    })
}

/// The framework's aggregated constraint report with the rejected value lifted out of
/// every ENUM clause, or `None` when no clause echoes one.
///
/// A value outside an enumerated type's value set is reported by naming the member
/// path and the permitted values; the rejected value itself is withheld, because it
/// is precisely the value the set does not contain. Every other clause — a length, a
/// range, a pattern, an absent member — keeps the framework's wording, including its
/// echo, and the code stays the one the framework answers with
/// ([`FRAMEWORK_REFUSAL_CODE`]).
///
/// The clauses are recited in the order this service recites them, and each enumerated
/// clause's permitted set in the order it recites that enum's values ([`recited`]).
///
/// The text replaced is built from the SAME two fields the framework interpolated
/// (`Violation::message`), so it matches the echo exactly whatever the value holds.
fn without_rejected_enum_values(violations: &[Violation]) -> Option<Refused> {
    let echoes_enum_value = violations
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::EnumValue(_)) && v.value.is_some());
    if !echoes_enum_value {
        return None;
    }
    let stated = recited(violations);
    let mut message = aws_smithy_schema_server::envelope(&stated)?;
    for violation in &stated {
        if !matches!(violation.kind, ViolationKind::EnumValue(_)) {
            continue;
        }
        let Some(value) = violation.value.as_deref() else {
            continue;
        };
        message = message.replace(
            &format!("Value '{value}' at '{}'", violation.path),
            &format!("Value at '{}'", violation.path),
        );
    }
    Some(Refused {
        code: FRAMEWORK_REFUSAL_CODE.to_string(),
        message,
    })
}

/// The member every tagging operation addresses its resource by.
///
/// Source: `smithy_query(query="operation", operation="TagResource,UntagResource,
/// ListTagsForResource")` — each takes `resourceArn`, bound to the one `/v1/tags/…`
/// path label and targeting `ResourceArn`, whose `@length` and `@pattern` admit one ARN
/// shape.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// Whether this violation is the TAGGING surface's own `resourceArn`: the TOP-LEVEL
/// member of that name, the one bound to the `/v1/tags/{resourceArn}` label and targeting
/// `ResourceArn`, whose pattern admits only an ARN of this service's own vendor.
///
/// The path is compared whole, because another member of this model is also named
/// `resourceArn` and is an ARN of ANOTHER vendor: `RdsDataApiConfig$resourceArn` — "the
/// resource ARN of the RDS cluster", pattern `^arn:[a-z-]*:rds:…:cluster:…$`
/// (`smithy_query(query="shape", shape="RdsDataApiConfig")`) — reaches this file on the
/// nested path `rdsDataApiConfig.resourceArn`, and the vendor the tagging surface expects
/// is not the vendor that member names.
fn tagging_resource_arn(violation: &Violation) -> bool {
    violation.path == RESOURCE_ARN_MEMBER
}

/// AppSync's own refusal for a TAGGING `resourceArn` whose VALUE cannot be a resource ARN
/// at all — outside the shape's length bounds or its pattern.
///
/// The tagging operations model `BadRequestException` and no constraint-violation error,
/// so the framework's generic report (`ValidationException`, with every broken clause
/// recited) names a shape no operation of this service models and a client's error-type
/// dispatch cannot match it. The service states the malformed ARN as one sentence
/// instead, which is why the rejected value is not echoed — and when the value IS an ARN,
/// of a vendor this service does not serve, that sentence names the vendor it expected
/// (`crate::state::invalid_resource_arn_message`), the one diagnosis that says which
/// field of the ARN failed.
///
/// Only the TAGGING surface's own member ([`tagging_resource_arn`]): the model declares a
/// second member of that name, `RdsDataApiConfig$resourceArn`, which is an ARN of another
/// vendor for another surface, and that one is stated as the constraint report every other
/// member's declared form is stated in ([`declared_form_refusal`]) — the member PATH and
/// the expression the model declares, which is the only diagnosis that says what shape of
/// ARN that member expects.
///
/// `None` when no violation is of that member and kind, which leaves every other
/// refusal to the wordings the rest of this file states.
fn resource_arn_refusal(violations: &[Violation]) -> Option<Refused> {
    violations
        .iter()
        .find(|violation| {
            tagging_resource_arn(violation)
                && matches!(
                    violation.kind,
                    ViolationKind::MinLength(_)
                        | ViolationKind::MaxLength(_)
                        | ViolationKind::Pattern(_)
                )
        })
        .map(|violation| Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: crate::state::invalid_resource_arn_message(violation.value.as_deref()),
        })
}

/// The member every operation that carries tags holds them in: a `TagMap`, whose VALUES the
/// request boundary checks against `TagValue`'s declared expression (its KEYS are checked by
/// the handlers' own rule, `crate::state::tag_key_refusal`).
const TAGS_MEMBER: &str = "tags";

/// AppSync's own refusal for a tag whose VALUE carries a character tags may not hold: the
/// service's `BadRequestException` with the one sentence that states the permitted
/// characters (`crate::state::INVALID_TAG_VALUE_MESSAGE`).
///
/// A tag value is not a member with a form of its own to report: the service states the tag
/// GRAMMAR, the same way it states the grammar of a tag key — so the clause the framework
/// would build for the broken expression, and the value it would echo, are both replaced by
/// that one sentence. Only a `@pattern` violation: a value past the length bound is refused
/// for its size rather than for its characters, so it keeps the clause the model's own
/// constraint words.
///
/// `None` when no violation is a tag value's expression, which leaves every other refusal
/// to the wordings the rest of this file states.
fn tag_value_refusal(violations: &[Violation]) -> Option<Refused> {
    violations
        .iter()
        .find(|violation| {
            // The path of a map VALUE is the member followed by the entry's key, so the
            // first segment names the map and a second one is what makes it an entry.
            let mut segments = violation.path.split('.');
            segments.next() == Some(TAGS_MEMBER)
                && segments.next().is_some()
                && matches!(violation.kind, ViolationKind::Pattern(_))
        })
        .map(|_| Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: crate::state::INVALID_TAG_VALUE_MESSAGE.to_string(),
        })
}

/// The member every operation that writes a resolver or a function carries its APPSYNC_JS
/// definition in: `code`, "The `resolver` code that contains the request and response
/// functions" (`Resolver$code`, `FunctionConfiguration$code`), whose target `Code` declares
/// a `@length` maximum.
const CODE_MEMBER: &str = "code";

/// AppSync's own refusal for a `code` past the ceiling its target shape declares: its
/// `BadRequestException` with the service's own sentence naming that ceiling, echoing
/// nothing of the submitted definition.
///
/// A definition is refused for its SIZE by the service rather than by the framework's
/// generic constraint report, which would quote the whole submitted module back into the
/// message; AppSync names the member's ceiling and nothing else. The ceiling is the one the
/// broken constraint carries, so the sentence states the model's own bound, and it is the
/// same sentence for every operation that WRITES the member — one over-long definition is
/// refused one way whichever call stored it.
///
/// The operation that EVALUATES a definition rather than storing one states the framework's
/// report for the same shape instead, so its `code` never reaches this seam: it is checked
/// in that handler (`crate::state::evaluated_code_refusal`), which is the only place the
/// two can be told apart.
///
/// Only the MAXIMUM: a definition shorter than the modelled minimum keeps the framework's
/// own report, which no recording words differently.
fn code_length_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violation.path != CODE_MEMBER {
            return None;
        }
        let ViolationKind::MaxLength(max) = violation.kind else {
            return None;
        };
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: code_ceiling_message(max),
        })
    })
}

/// The text AppSync states a `code` past its modelled ceiling with: fixed text whose one
/// variable part is the limit.
fn code_ceiling_message(max: i64) -> String {
    format!("Code must be {max} bytes or less")
}

/// AppSync's own refusal for a member whose value breaks the FORM its target shape
/// declares — a `@length` bound or a `@pattern` — in the service's modelled
/// `BadRequestException` (HTTP 400), carrying the framework's own constraint report as the
/// message.
///
/// The framework answers such a refusal under [`FRAMEWORK_REFUSAL_CODE`], a shape no
/// operation of this service models, so a client's typed-error dispatch
/// (`except BadRequestException`) never fires and code branching on the error code takes
/// the wrong branch. The DECISION is the framework's and so is the sentence — it names the
/// offending member and the expression it broke, which is how the service words it — and
/// only the CODE is restated, as the one client error these operations declare for a
/// request that is not well formed.
///
/// The last of the wordings this file states, so a member with a sentence of its own
/// (a tagging `resourceArn`, a `format`) keeps it; `None` when no violation is of either
/// class, which leaves a class this service does not word to the framework.
///
/// Its clauses are recited in the order this service recites them ([`recited`]), the same
/// order the enum report above is recited in: the report's ORDER is the service's however
/// many clauses it carries.
fn declared_form_refusal(violations: &[Violation]) -> Option<Refused> {
    let breaks_declared_form = violations.iter().any(|violation| {
        matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_) | ViolationKind::Pattern(_)
        )
    });
    if !breaks_declared_form {
        return None;
    }
    Some(Refused {
        code: BAD_REQUEST_CODE.to_string(),
        message: aws_smithy_schema_server::envelope(&recited(violations))?,
    })
}

/// The `com-amazonaws-appsync-awsdeepdishcontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`authorization::Gated`] per operation this service publishes, keyed by the
    /// routing key the model names that operation by
    /// ([`Operation::NAME`], the same key the generated routing table uses). Built once
    /// here, never per request; each entry carries the operation's TYPE and a handle on the
    /// one shared engine, so this is a table of names rather than a second set of
    /// operations.
    gated: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gated: gated_table::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The [`authorization::Gated`] wrapper for every operation this service publishes
/// ([`authorization::GATED_OPERATIONS`]).
///
/// Authorization is a property of the request's identity, the action it names and the
/// resource it addresses rather than of what a handler goes on to do, so it is taken here
/// for all of them at once — implemented or still scaffolded, and whatever error set the
/// operation itself declares. A stub answers 501 for a request whose caller IAM permits and
/// the refusal otherwise, which is the order the real service evaluates them in; leaving an
/// operation out would serve an unauthorized caller, the very divergence this removes.
fn gated_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut wrappers: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                wrappers.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(authorization::Gated::<
                        crate::ops::$module::$operation<Eng>,
                        Eng,
                    >::new(Arc::clone(engine))) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        associate_api::AssociateApi,
        associate_merged_graphql_api::AssociateMergedGraphqlApi,
        associate_source_graphql_api::AssociateSourceGraphqlApi,
        create_api::CreateApi,
        create_api_cache::CreateApiCache,
        create_api_key::CreateApiKey,
        create_channel_namespace::CreateChannelNamespace,
        create_data_source::CreateDataSource,
        create_domain_name::CreateDomainName,
        create_function::CreateFunction,
        create_graphql_api::CreateGraphqlApi,
        create_resolver::CreateResolver,
        create_type::CreateType,
        delete_api::DeleteApi,
        delete_api_cache::DeleteApiCache,
        delete_api_key::DeleteApiKey,
        delete_channel_namespace::DeleteChannelNamespace,
        delete_data_source::DeleteDataSource,
        delete_domain_name::DeleteDomainName,
        delete_function::DeleteFunction,
        delete_graphql_api::DeleteGraphqlApi,
        delete_resolver::DeleteResolver,
        delete_type::DeleteType,
        disassociate_api::DisassociateApi,
        disassociate_merged_graphql_api::DisassociateMergedGraphqlApi,
        disassociate_source_graphql_api::DisassociateSourceGraphqlApi,
        evaluate_code::EvaluateCode,
        evaluate_mapping_template::EvaluateMappingTemplate,
        flush_api_cache::FlushApiCache,
        get_api::GetApi,
        get_api_association::GetApiAssociation,
        get_api_cache::GetApiCache,
        get_channel_namespace::GetChannelNamespace,
        get_data_source::GetDataSource,
        get_data_source_introspection::GetDataSourceIntrospection,
        get_domain_name::GetDomainName,
        get_function::GetFunction,
        get_graphql_api::GetGraphqlApi,
        get_graphql_api_environment_variables::GetGraphqlApiEnvironmentVariables,
        get_resolver::GetResolver,
        get_schema_creation_status::GetSchemaCreationStatus,
        get_source_api_association::GetSourceApiAssociation,
        get_type::GetType,
        list_api_keys::ListApiKeys,
        list_apis::ListApis,
        list_channel_namespaces::ListChannelNamespaces,
        list_data_sources::ListDataSources,
        list_domain_names::ListDomainNames,
        list_functions::ListFunctions,
        list_graphql_apis::ListGraphqlApis,
        list_resolvers::ListResolvers,
        list_resolvers_by_function::ListResolversByFunction,
        list_source_api_associations::ListSourceApiAssociations,
        list_tags_for_resource::ListTagsForResource,
        list_types::ListTypes,
        list_types_by_association::ListTypesByAssociation,
        put_graphql_api_environment_variables::PutGraphqlApiEnvironmentVariables,
        start_data_source_introspection::StartDataSourceIntrospection,
        start_schema_creation::StartSchemaCreation,
        start_schema_merge::StartSchemaMerge,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_api::UpdateApi,
        update_api_cache::UpdateApiCache,
        update_api_key::UpdateApiKey,
        update_channel_namespace::UpdateChannelNamespace,
        update_data_source::UpdateDataSource,
        update_domain_name::UpdateDomainName,
        update_function::UpdateFunction,
        update_graphql_api::UpdateGraphqlApi,
        update_resolver::UpdateResolver,
        update_source_api_association::UpdateSourceApiAssociation,
        update_type::UpdateType,
    );
    // The one operation whose answer is not this service's restJson1 envelope: the SCHEMA
    // an introspection read answers with is the response's whole payload, labelled with the
    // media type AppSync labels a schema with ([`schema_media_type`]). Served behind the
    // same decision as every other operation — the layer wraps that gate rather than
    // replacing it.
    wrappers.insert(
        <crate::ops::get_introspection_schema::GetIntrospectionSchema<Eng> as Operation>::NAME,
        Box::new(schema_media_type::AnnouncedAsJson::new(
            authorization::Gated::<
                crate::ops::get_introspection_schema::GetIntrospectionSchema<Eng>,
                Eng,
            >::new(Arc::clone(engine)),
        )) as Box<dyn ErasedOperation>,
    );
    wrappers
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in
        // [`authorization::Gated`] so IAM's decision about the action is taken BEFORE the
        // handler runs, as AWS takes it. Every operation this service publishes is in that
        // table ([`authorization::GATED_OPERATIONS`]); a key the model does not name
        // resolves to `None` here, so the router's routing-error path is untouched.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gated.get(key).map(::std::convert::AsRef::as_ref)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. AppSync states three of those classes itself:
        //
        // * a TAGGING `resourceArn` whose value cannot be a resource ARN at all is its
        //   own `BadRequestException` with the service's sentence for a malformed ARN —
        //   naming the vendor it expected when the value is an ARN of another one
        //   (`resource_arn_refusal`) — the only client error its operations model;
        // * a Lambda authorizer's cached-result lifetime outside its modelled window is
        //   its own `BadRequestException` naming the lifetime and the bound the value
        //   crossed (`authorizer_ttl_refusal`), which is the sentence the service states
        //   that limit in rather than the form it states a page or batch size in;
        // * a member the model rules out outright — absent though required, or
        //   outside its modelled `@range` — is its own `BadRequestException` naming
        //   that member, and its range when it has one (`member_refusal`); the
        //   framework's `ValidationException` is a shape no operation of this service
        //   models;
        // * a value outside an enumerated type's value set names the member path and
        //   the permitted values WITHOUT echoing the rejected value
        //   (`without_rejected_enum_values`), under the framework's own code — except
        //   for the `format` a report is asked in, which the service refuses by naming
        //   the member and the rejected value (`format_refusal`), and the `runtime.name`
        //   a request names its code's runtime by, which keeps that sentence under the
        //   service's own `BadRequestException` (`runtime_name_refusal`);
        // * a tag whose VALUE carries a character tags may not hold is its own
        //   `BadRequestException` stating the tag grammar (`tag_value_refusal`), the way the
        //   service states the grammar of a tag key;
        // * a `code` past the ceiling its target shape declares is its own
        //   `BadRequestException` naming that ceiling (`code_length_refusal`), so the
        //   submitted definition is not echoed back into the message;
        // * a value that breaks the FORM its target shape declares — a `@length` bound or
        //   a `@pattern` — is its own `BadRequestException` carrying the framework's own
        //   constraint report (`declared_form_refusal`), because the sentence is right and
        //   only the code was a shape no operation of this service models.
        //
        // Every other refusal keeps the framework's wording and code.
        match refusal {
            Refusal::Invalid(violations) => resource_arn_refusal(violations)
                .or_else(|| authorizer_ttl_refusal(violations))
                .or_else(|| member_refusal(violations))
                .or_else(|| format_refusal(violations))
                .or_else(|| runtime_name_refusal(violations))
                .or_else(|| without_rejected_enum_values(violations))
                .or_else(|| tag_value_refusal(violations))
                .or_else(|| code_length_refusal(violations))
                .or_else(|| declared_form_refusal(violations)),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the table `override_operation` resolves a request through carries EXACTLY the
    /// operations the gate is asked about ([`authorization::GATED_OPERATIONS`], which that
    /// module's own test holds to the model), so no published action is served without IAM's
    /// decision and no key the model does not name is routed here.
    #[test]
    fn every_published_action_is_served_behind_the_decision() {
        // The engine here is only what the wrappers are BUILT from: this asserts the
        // table's keys, so nothing is stored in or read from it.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let table = gated_table::<engine::InMemoryEngine>(&engine);
        let mut wrapped: Vec<&str> = table.keys().copied().collect();
        wrapped.sort_unstable();
        let mut asked: Vec<&str> = authorization::GATED_OPERATIONS.to_vec();
        asked.sort_unstable();
        assert_eq!(
            wrapped, asked,
            "case \"the whole surface\": expected one wrapper per published action, got \
             {wrapped:?} against {asked:?}"
        );
    }
}
