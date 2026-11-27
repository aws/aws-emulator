// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE configuration of a stored subscriber: the delivery members the
//! service itself fills in for a caller that states none.
//!
//! A subscriber always batches, always retries, always hands its target some
//! shape of the event and always reads its filters in some language — so each of
//! those members has a value whether or not the request named one. The service
//! settles that value when the subscriber is written, which is why it lives here
//! beside the record rather than in a read: every later read projects the stored
//! subscriber, so one subscriber cannot read back two different effective
//! configurations, and the delivery path decides from the same values a client
//! can see.
//!
//! The members this settles are `BatchConfiguration`, `RetryPolicy`,
//! `Transformer` and `FilterConfiguration$Language`. Where the model documents
//! the default it is cited on the constant; the rest are the defaults this
//! service applies.

use super::Subscriber;
use crate::ops::types::{
    BatchConfiguration, FilterConfiguration, InvokeConfiguration, RetryPolicy, Transformer,
};

/// `FilterLanguage::EVENT_BRIDGE_PATTERN`, the one entry of
/// [`crate::ops::types::FILTER_LANGUAGE_VALUES`] and the language a
/// configuration that names none is read in: `FilterConfiguration$Language`
/// "Defaults to EVENT_BRIDGE_PATTERN when not specified".
pub const FILTER_LANGUAGE_EVENT_BRIDGE_PATTERN: &str = "EVENT_BRIDGE_PATTERN";

/// `RetryStrategy::ALL`, the one entry of
/// [`crate::ops::types::RETRY_STRATEGY_VALUES`] and the strategy a policy that
/// names none retries by: `RetryPolicy$RetryStrategy` "Strategy for determining
/// which exceptions are retried. Default: ALL".
pub const RETRY_STRATEGY_ALL: &str = "ALL";

/// The `MaxRetryAttempts` a subscriber retries a failed delivery at when the
/// request names none: `RetryPolicy$MaxRetryAttempts` "Maximum number of retry
/// attempts (0-185, default: 5)".
pub const DEFAULT_MAX_RETRY_ATTEMPTS: i32 = 5;

/// The `MaxEventAgeInSeconds` a subscriber discards a failing event at when the
/// request names none: `RetryPolicy$MaxEventAgeInSeconds` "Maximum age of an
/// event in seconds before it is discarded (60-86400, default: 300)".
pub const DEFAULT_MAX_EVENT_AGE_IN_SECONDS: i32 = 300;

/// The `MaxBatchSize` a subscriber batches at when the request names none.
///
/// `BatchConfiguration$MaxBatchSize` documents no default, so this is the
/// service's own applied value, inside the member's `@range 1..500`: a batch
/// holds up to this many events.
pub const DEFAULT_MAX_BATCH_SIZE: i32 = 10;

/// The `MaxBatchWindowInSeconds` a subscriber batches within when the request
/// names none.
///
/// `BatchConfiguration$MaxBatchWindowInSeconds` documents no default and is
/// "The maximum time in seconds to wait for a batch to fill", so the applied
/// value is the bottom of its `@range 0..300`: nothing is held back waiting for
/// a batch to fill, which is what makes a subscriber that configures no
/// batching deliver as its events arrive.
pub const DEFAULT_MAX_BATCH_WINDOW_IN_SECONDS: i32 = 0;

/// The batching configuration a subscriber whose request carried `requested`
/// delivers under: every member it did not name takes the service's default, so
/// a read reports what the subscriber actually does rather than only what the
/// caller spelled out.
pub fn effective_batch_configuration(requested: Option<&BatchConfiguration>) -> BatchConfiguration {
    BatchConfiguration {
        max_batch_size: requested
            .and_then(|configuration| configuration.max_batch_size)
            .or(Some(DEFAULT_MAX_BATCH_SIZE)),
        max_batch_window_in_seconds: requested
            .and_then(|configuration| configuration.max_batch_window_in_seconds)
            .or(Some(DEFAULT_MAX_BATCH_WINDOW_IN_SECONDS)),
    }
}

/// The retry policy a subscriber whose request carried `requested` retries
/// under, each member it did not name taking the default its own documentation
/// states.
pub fn effective_retry_policy(requested: Option<&RetryPolicy>) -> RetryPolicy {
    RetryPolicy {
        max_retry_attempts: requested
            .and_then(|policy| policy.max_retry_attempts)
            .or(Some(DEFAULT_MAX_RETRY_ATTEMPTS)),
        max_event_age_in_seconds: requested
            .and_then(|policy| policy.max_event_age_in_seconds)
            .or(Some(DEFAULT_MAX_EVENT_AGE_IN_SECONDS)),
        retry_strategy: requested
            .and_then(|policy| policy.retry_strategy.clone())
            .or_else(|| Some(RETRY_STRATEGY_ALL.to_string())),
    }
}

/// The transformation a subscriber invoking `invoke_configuration` applies,
/// given the `Transformer` its request carried.
///
/// A universal target keeps none — `Transformer` is "Absent for universal
/// (aws-sdk) targets, whose input transformation is
/// UniversalTargetParameters.Input" — so such a subscriber stores no
/// transformer whatever it asked for, and nothing else can hold both. Every
/// other subscriber hands its target some shape of the event, and the one a
/// request that named no transformer gets is [`super::TRANSFORMER_TYPE_RAW`]:
/// "a subscriber that configures no transformer hands its target" the event
/// untouched.
pub fn effective_transformer(
    invoke_configuration: &InvokeConfiguration,
    requested: Option<&Transformer>,
) -> Option<Transformer> {
    if invoke_configuration.universal_target_parameters.is_some() {
        return None;
    }
    Some(Transformer {
        r#type: requested
            .and_then(|transformer| transformer.r#type.clone())
            .or_else(|| Some(super::TRANSFORMER_TYPE_RAW.to_string())),
        jsonata_configuration: requested
            .and_then(|transformer| transformer.jsonata_configuration.clone()),
    })
}

/// The filter configuration a subscriber whose request carried `requested`
/// selects events with.
///
/// A subscriber holding no filter configuration at all is delivered every event
/// of its bus and keeps none here — there is no language to read patterns in
/// when there are no patterns. A configuration that IS held is read in the
/// language it names, or in the default one when it names none.
pub fn effective_filter_configuration(
    requested: Option<&FilterConfiguration>,
) -> Option<FilterConfiguration> {
    let requested = requested?;
    Some(FilterConfiguration {
        language: requested
            .language
            .clone()
            .or_else(|| Some(FILTER_LANGUAGE_EVENT_BRIDGE_PATTERN.to_string())),
        filters: requested.filters.clone(),
    })
}

/// Why the TRANSFORMATION a subscriber would be left with is not one this
/// service could hand its target, or `None` when it is.
///
/// Every reading of a subscriber's `Transformer` that answers
/// `InvalidInputException` — "A request parameter is missing or not valid" — in
/// ONE place, so every call that WRITES a subscriber's transformer admits
/// exactly the same transformations: the create that mints one and the update
/// that rewrites one. That matters beyond the wording of a refusal, because the
/// delivery path hands a target what the transformer produces and nothing else,
/// so a subscriber left holding a transformation this service cannot perform
/// delivers NOTHING and reports nothing about why, while its publisher keeps
/// being told its events were accepted.
///
/// The arguments are the transformation and the target configuration the call
/// leaves behind rather than the members its request happened to carry, since
/// either of the two may be the one being rewritten.
///
/// The readings, in the order a refusal takes them: the transformation against a
/// universal target, which takes none ([`super::transformer_fault`]); the
/// transformation itself — a type this service declares, with the configuration
/// that type requires and an expression the language admits
/// ([`super::transformer_configuration_fault`]); and the transformation a
/// previous-generation target can be handed
/// ([`super::classic_target_transformer_fault`]).
pub fn transformation_fault(
    invoke_configuration: &InvokeConfiguration,
    transformer: Option<&Transformer>,
) -> Option<String> {
    super::transformer_fault(
        invoke_configuration.universal_target_parameters.as_ref(),
        transformer,
    )
    .or_else(|| super::transformer_configuration_fault(transformer))
    .or_else(|| super::classic_target_transformer_fault(invoke_configuration, transformer))
}

/// Why the TARGET CONFIGURATION a subscriber would be left with is not one this
/// service could invoke, or `None` when it is.
///
/// The ONE contract every call that WRITES a subscriber's
/// `InvokeConfiguration` is held to — the create that mints one and the update
/// that rewrites one — so neither can leave a subscriber a configuration the
/// other would have refused. That matters beyond the wording of a refusal: a
/// subscriber whose configuration names a call this service cannot make
/// delivers NOTHING and says nothing about why, while its publisher keeps being
/// told its events were accepted.
///
/// The arguments are the configuration and the transformation the call leaves
/// behind rather than the members its request happened to carry, since either of
/// them may be the one being rewritten and an update inherits the rest from the
/// stored subscriber.
///
/// The readings, in the order a refusal takes them: the transformation the
/// configuration is handed ([`transformation_fault`]), the parameters member the
/// target takes ([`super::target_parameters_fault`]), the values that member
/// carries ([`super::configured_expression_fault`],
/// [`super::invocation_timeout_fault`]), the account the target belongs to
/// ([`super::target_account_fault`]) and the call a universal target names with
/// the request it is made with ([`super::universal_target_api_fault`],
/// [`super::universal_target_input_fault`]).
pub fn invoke_configuration_fault(
    invoke_configuration: &InvokeConfiguration,
    transformer: Option<&Transformer>,
) -> Option<String> {
    transformation_fault(invoke_configuration, transformer)
        .or_else(|| super::target_parameters_fault(invoke_configuration))
        .or_else(|| super::configured_expression_fault(invoke_configuration))
        .or_else(|| super::invocation_timeout_fault(invoke_configuration))
        .or_else(|| super::target_account_fault(invoke_configuration))
        .or_else(|| super::universal_target_api_fault(invoke_configuration))
        .or_else(|| super::universal_target_input_fault(invoke_configuration))
}

/// Whether `transformer` is the one the SERVICE applies to a subscriber that
/// configured none ([`effective_transformer`]) rather than one a caller asked
/// for: the raw transformation and nothing beside it.
///
/// A default nobody asked for is not a configuration a later request can be
/// held to, which is what lets an update that makes a subscriber's target
/// universal be served while one carrying a transformer of its own is refused.
pub fn is_default_transformer(transformer: &Transformer) -> bool {
    transformer.jsonata_configuration.is_none()
        && transformer.r#type.as_deref() == Some(super::TRANSFORMER_TYPE_RAW)
}

impl Subscriber {
    /// Settle this subscriber's effective configuration, so the stored record
    /// carries a value for every member the service applies a default to.
    ///
    /// Called by the create that mints a subscriber and by every update that
    /// writes one, so the invariant holds for the whole life of the record: a
    /// stored subscriber always names its batching, its retry policy and the
    /// language of whatever filters it holds, and holds a transformer exactly
    /// when its target takes one.
    pub fn settle_effective_configuration(&mut self) {
        self.batch_configuration = Some(effective_batch_configuration(
            self.batch_configuration.as_ref(),
        ));
        self.retry_policy = Some(effective_retry_policy(self.retry_policy.as_ref()));
        self.transformer =
            effective_transformer(&self.invoke_configuration, self.transformer.as_ref());
        self.filter_configuration =
            effective_filter_configuration(self.filter_configuration.as_ref());
    }
}
