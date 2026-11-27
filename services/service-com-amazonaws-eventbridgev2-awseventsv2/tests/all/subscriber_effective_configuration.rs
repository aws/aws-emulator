// SPDX-License-Identifier: CC0-1.0

//! The configuration a subscriber DELIVERS under, as every read of it reports.
//!
//! RULE: a subscriber always batches, always retries, always hands its target
//! some shape of the event and always reads its filters in some language, so the
//! service settles a value for each of those members when the subscriber is
//! written — the one the request named, or the service's default where it named
//! none — and every later read reports that effective configuration rather than
//! only the members the caller spelled out. The same holds for an update: a
//! configuration carried in part leaves the members it did not name at their
//! defaults. The members are `BatchConfiguration`, `RetryPolicy`, `Transformer`
//! (absent only for a universal target, which takes its input transformation
//! from `UniversalTargetParameters.Input`) and `FilterConfiguration$Language`.
//!
//! RULE: `Description` is free text, so a value holding no non-whitespace
//! character describes nothing — whichever resource it describes and whether a
//! create or an update carries it, the resource keeps no description and the
//! read reports no member.
//!
//! The rows span what those rules generalise over: which members the request
//! carries (none, one, part of a shape, all of it), the published bounds of the
//! ranges they take, whether the transformer is named or defaulted, whether the
//! target takes a transformer at all, whether a filter configuration names its
//! language, and — for the description — the empty string, whitespace of
//! several shapes, and text with whitespace around it, on all three resource
//! types this service describes.
//!
//! Everything is read off the `rpcv2Cbor` wire, through the harness
//! `modelled_input_refusal.rs` already owns. Nothing here waits on the wall
//! clock: the one wait is the bus's own provisioning transition, polled for the
//! CONDITION.

use engine::InMemoryEngine;

use service_com_amazonaws_eventbridgev2_awseventsv2 as eventsv2;

use eventsv2::ops::create_event_source::CreateEventSource;
use eventsv2::ops::create_subscriber::CreateSubscriber;
use eventsv2::ops::describe_event_source::DescribeEventSource;
use eventsv2::ops::describe_subscriber::DescribeSubscriber;
use eventsv2::ops::types::{
    BatchConfiguration, CreateSubscriberInput, DescribeEventSourceInput, DescribeSubscriberInput,
    DescribeSubscriberOutput, Filter, FilterConfiguration, RetryPolicy, Transformer,
    UniversalTargetParameters, UpdateInvokeConfiguration, UpdateSubscriberInput,
};
use eventsv2::ops::update_subscriber::UpdateSubscriber;

use crate::modelled_input_refusal::{
    create_source, create_subscriber, refusal, rendered, with_client_token, Wire,
    INVALID_INPUT_EXCEPTION,
};

/// The `MaxBatchSize` a subscriber that named none batches at — the service's
/// own applied default, inside `BatchConfiguration$MaxBatchSize`'s
/// `@range 1..500`.
const DEFAULT_MAX_BATCH_SIZE: i32 = 10;

/// The target ARN of a UNIVERSAL (aws-sdk) target, the form
/// `UniversalTargetParameters` documents: `arn:aws:events:::aws-sdk:{service}:{apiAction}`.
const UNIVERSAL_TARGET_ARN: &str = "arn:aws:events:::aws-sdk:s3:putObject";

/// The `MaxBatchWindowInSeconds` a subscriber that named none batches within:
/// the bottom of `BatchConfiguration$MaxBatchWindowInSeconds`'s
/// `@range 0..300`, so nothing is held back waiting for a batch to fill.
const DEFAULT_MAX_BATCH_WINDOW: i32 = 0;

/// `RetryPolicy$MaxRetryAttempts`: "Maximum number of retry attempts (0-185,
/// default: 5)".
/// Source: `smithy_query(query="shape", shape="RetryPolicy")`.
const DEFAULT_MAX_RETRY_ATTEMPTS: i32 = 5;

/// `RetryPolicy$MaxEventAgeInSeconds`: "Maximum age of an event in seconds
/// before it is discarded (60-86400, default: 300)".
/// Source: `smithy_query(query="shape", shape="RetryPolicy")`.
const DEFAULT_MAX_EVENT_AGE: i32 = 300;

/// `RetryPolicy$RetryStrategy`: "Strategy for determining which exceptions are
/// retried. Default: ALL" — and the one entry `RetryStrategy` declares.
const DEFAULT_RETRY_STRATEGY: &str = "ALL";

/// `FilterConfiguration$Language`: "Defaults to EVENT_BRIDGE_PATTERN when not
/// specified" — and the one entry `FilterLanguage` declares.
const DEFAULT_FILTER_LANGUAGE: &str = "EVENT_BRIDGE_PATTERN";

/// `TransformerType::RAW`, the transformation a subscriber that configured none
/// hands its target.
const DEFAULT_TRANSFORMER_TYPE: &str = "RAW";

/// The published bounds of the ranges the batching and retry members take.
/// Source: `smithy_query(query="shape", shape="BatchConfiguration,RetryPolicy")`.
const MAX_BATCH_SIZE_CEILING: i32 = 500;
const MAX_BATCH_SIZE_FLOOR: i32 = 1;
const MAX_BATCH_WINDOW_CEILING: i32 = 300;
const MAX_RETRY_ATTEMPTS_FLOOR: i32 = 0;
const MAX_RETRY_ATTEMPTS_CEILING: i32 = 185;
const MAX_EVENT_AGE_FLOOR: i32 = 60;
const MAX_EVENT_AGE_CEILING: i32 = 86_400;

/// An event pattern a filter can carry, of the one language this service reads.
const PATTERN: &str = r#"{"source":["effective.probe"]}"#;

/// A second pattern, so an update can replace the filters with other ones.
const OTHER_PATTERN: &str = r#"{"source":["effective.other"]}"#;

/// The configuration a read reports a subscriber delivers under, in one
/// comparable value — so a row states the WHOLE effective configuration rather
/// than only the member it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Effective {
    /// `BatchConfiguration`, as `(MaxBatchSize, MaxBatchWindowInSeconds)`.
    batch: Option<(Option<i32>, Option<i32>)>,
    /// `RetryPolicy`, as `(MaxRetryAttempts, MaxEventAgeInSeconds,
    /// RetryStrategy)`.
    retry: Option<(Option<i32>, Option<i32>, Option<String>)>,
    /// `Transformer$Type`; the outer `None` means the member is absent.
    transformer: Option<Option<String>>,
    /// `FilterConfiguration$Language`; the outer `None` means the subscriber
    /// holds no filter configuration at all.
    language: Option<Option<String>>,
    /// How many filters that configuration holds.
    filters: Option<usize>,
}

/// The effective configuration of a subscriber that named none of these
/// members: every one of them at the service's default, and no filters.
fn defaults() -> Effective {
    Effective {
        batch: Some((
            Some(DEFAULT_MAX_BATCH_SIZE),
            Some(DEFAULT_MAX_BATCH_WINDOW),
        )),
        retry: Some((
            Some(DEFAULT_MAX_RETRY_ATTEMPTS),
            Some(DEFAULT_MAX_EVENT_AGE),
            Some(DEFAULT_RETRY_STRATEGY.to_string()),
        )),
        transformer: Some(Some(DEFAULT_TRANSFORMER_TYPE.to_string())),
        language: None,
        filters: None,
    }
}

/// What `described` reports about the configuration the subscriber delivers
/// under.
fn effective_of(described: &DescribeSubscriberOutput) -> Effective {
    Effective {
        batch: described
            .batch_configuration
            .as_ref()
            .map(|batch| (batch.max_batch_size, batch.max_batch_window_in_seconds)),
        retry: described.retry_policy.as_ref().map(|policy| {
            (
                policy.max_retry_attempts,
                policy.max_event_age_in_seconds,
                policy.retry_strategy.clone(),
            )
        }),
        transformer: described
            .transformer
            .as_ref()
            .map(|transformer| transformer.r#type.clone()),
        language: described
            .filter_configuration
            .as_ref()
            .map(|configuration| configuration.language.clone()),
        filters: described.filter_configuration.as_ref().map(|configuration| {
            configuration
                .filters
                .as_ref()
                .map(Vec::len)
                .unwrap_or_default()
        }),
    }
}

/// One `FilterConfiguration` holding one filter of `pattern`, in `language`.
fn filter_configuration(pattern: &str, language: Option<&str>) -> FilterConfiguration {
    FilterConfiguration {
        language: language.map(str::to_string),
        filters: Some(vec![Filter {
            pattern: pattern.to_string(),
            scope: "METADATA".to_string(),
        }]),
    }
}

/// Create the subscriber `input` describes and read it back.
async fn created_and_read(
    wire: &Wire,
    case: &str,
    input: CreateSubscriberInput,
) -> (String, DescribeSubscriberOutput) {
    let created = wire
        .ok::<CreateSubscriber<InMemoryEngine>>(case, &with_client_token(input))
        .await;
    let arn = created
        .subscriber_arn
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted SubscriberArn, got {created:?}"));
    let described = read(wire, case, &arn).await;
    (arn, described)
}

/// Read the subscriber at `arn`.
async fn read(wire: &Wire, case: &str, arn: &str) -> DescribeSubscriberOutput {
    wire.ok::<DescribeSubscriber<InMemoryEngine>>(
        case,
        &DescribeSubscriberInput {
            subscriber_arn: arn.to_string(),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: a create settles the configuration the subscriber delivers under
// ---------------------------------------------------------------------------

/// Every created subscriber reads back an effective value for its batching,
/// retry policy, transformation and filter language, whichever of those members
/// the create named.
#[tokio::test]
async fn a_created_subscriber_reads_back_the_configuration_it_delivers_under() {
    let wire = Wire::new();
    let bus = wire.active_bus("the bus every row is created on", "effective-bus").await;

    // (case, what the create carries beyond the required members, the effective
    // configuration it must read back)
    type Row = (
        &'static str,
        fn(&mut CreateSubscriberInput),
        fn() -> Effective,
    );
    let rows: [Row; 12] = [
        (
            "a create naming none of these members",
            |_| {},
            defaults,
        ),
        (
            "a batch size alone",
            |input| {
                input.batch_configuration = Some(BatchConfiguration {
                    max_batch_size: Some(7),
                    max_batch_window_in_seconds: None,
                });
            },
            || Effective {
                batch: Some((Some(7), Some(DEFAULT_MAX_BATCH_WINDOW))),
                ..defaults()
            },
        ),
        (
            "a batch window alone",
            |input| {
                input.batch_configuration = Some(BatchConfiguration {
                    max_batch_size: None,
                    max_batch_window_in_seconds: Some(42),
                });
            },
            || Effective {
                batch: Some((Some(DEFAULT_MAX_BATCH_SIZE), Some(42))),
                ..defaults()
            },
        ),
        (
            "a batching configuration at the top of both published ranges",
            |input| {
                input.batch_configuration = Some(BatchConfiguration {
                    max_batch_size: Some(MAX_BATCH_SIZE_CEILING),
                    max_batch_window_in_seconds: Some(MAX_BATCH_WINDOW_CEILING),
                });
            },
            || Effective {
                batch: Some((Some(MAX_BATCH_SIZE_CEILING), Some(MAX_BATCH_WINDOW_CEILING))),
                ..defaults()
            },
        ),
        (
            "a batching configuration at the bottom of both published ranges",
            |input| {
                input.batch_configuration = Some(BatchConfiguration {
                    max_batch_size: Some(MAX_BATCH_SIZE_FLOOR),
                    max_batch_window_in_seconds: Some(DEFAULT_MAX_BATCH_WINDOW),
                });
            },
            || Effective {
                batch: Some((Some(MAX_BATCH_SIZE_FLOOR), Some(DEFAULT_MAX_BATCH_WINDOW))),
                ..defaults()
            },
        ),
        (
            "a retry attempt count alone, at the bottom of its published range",
            |input| {
                input.retry_policy = Some(RetryPolicy {
                    max_retry_attempts: Some(MAX_RETRY_ATTEMPTS_FLOOR),
                    ..Default::default()
                });
            },
            || Effective {
                retry: Some((
                    Some(MAX_RETRY_ATTEMPTS_FLOOR),
                    Some(DEFAULT_MAX_EVENT_AGE),
                    Some(DEFAULT_RETRY_STRATEGY.to_string()),
                )),
                ..defaults()
            },
        ),
        (
            "an event age alone, at the bottom of its published range",
            |input| {
                input.retry_policy = Some(RetryPolicy {
                    max_event_age_in_seconds: Some(MAX_EVENT_AGE_FLOOR),
                    ..Default::default()
                });
            },
            || Effective {
                retry: Some((
                    Some(DEFAULT_MAX_RETRY_ATTEMPTS),
                    Some(MAX_EVENT_AGE_FLOOR),
                    Some(DEFAULT_RETRY_STRATEGY.to_string()),
                )),
                ..defaults()
            },
        ),
        (
            "a whole retry policy at the top of both published ranges",
            |input| {
                input.retry_policy = Some(RetryPolicy {
                    max_retry_attempts: Some(MAX_RETRY_ATTEMPTS_CEILING),
                    max_event_age_in_seconds: Some(MAX_EVENT_AGE_CEILING),
                    retry_strategy: Some(DEFAULT_RETRY_STRATEGY.to_string()),
                });
            },
            || Effective {
                retry: Some((
                    Some(MAX_RETRY_ATTEMPTS_CEILING),
                    Some(MAX_EVENT_AGE_CEILING),
                    Some(DEFAULT_RETRY_STRATEGY.to_string()),
                )),
                ..defaults()
            },
        ),
        (
            "a transformer the create names",
            |input| {
                input.transformer = Some(Transformer {
                    r#type: Some("WITH_METADATA".to_string()),
                    jsonata_configuration: None,
                });
            },
            || Effective {
                transformer: Some(Some("WITH_METADATA".to_string())),
                ..defaults()
            },
        ),
        (
            "a transformer naming no type at all",
            |input| {
                input.transformer = Some(Transformer::default());
            },
            defaults,
        ),
        (
            "filters naming no language",
            |input| {
                input.filter_configuration = Some(filter_configuration(PATTERN, None));
            },
            || Effective {
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "filters naming the language themselves",
            |input| {
                input.filter_configuration =
                    Some(filter_configuration(PATTERN, Some(DEFAULT_FILTER_LANGUAGE)));
            },
            || Effective {
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
    ];

    for (index, (case, carry, expected)) in rows.into_iter().enumerate() {
        let mut input = create_subscriber(&format!("effective-created-{index}"), &bus, None);
        carry(&mut input);
        let (_, described) = created_and_read(&wire, case, input).await;
        let reported = effective_of(&described);
        let wanted = expected();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the effective configuration {wanted:?}, got {reported:?}"
        );
    }
}

/// A universal target takes its input transformation from
/// `UniversalTargetParameters.Input`, so a subscriber invoking one holds no
/// `Transformer` at all — while its batching and retry policy are settled the
/// same way as every other subscriber's.
#[tokio::test]
async fn a_subscriber_on_a_universal_target_reads_back_no_transformer() {
    let wire = Wire::new();
    let bus = wire.active_bus("the universal target's bus", "universal-bus").await;

    let case = "a create naming a universal target";
    let mut input = create_subscriber("effective-universal", &bus, None);
    // A universal target is the one whose ARN names an SDK call rather than a
    // resource — `UniversalTargetParameters` is "Used when TargetArn is in the
    // format arn:aws:events:::aws-sdk:{service}:{apiAction}" — so the target and
    // the parameters member that goes with it are named together.
    input.invoke_configuration.target_arn = UNIVERSAL_TARGET_ARN.to_string();
    input.invoke_configuration.universal_target_parameters = Some(UniversalTargetParameters {
        input: "{}".to_string(),
        invocation_timeout_seconds: None,
    });
    // A universal target is invoked for a batch of events, so its batching is
    // the caller's to state; these are the values the service would have
    // applied anyway, so the effective configuration below is the defaulted
    // one.
    input.batch_configuration = Some(BatchConfiguration {
        max_batch_size: Some(DEFAULT_MAX_BATCH_SIZE),
        max_batch_window_in_seconds: Some(DEFAULT_MAX_BATCH_WINDOW),
    });
    let (arn, described) = created_and_read(&wire, case, input).await;
    let reported = effective_of(&described);
    let wanted = Effective {
        transformer: None,
        ..defaults()
    };
    assert_eq!(
        reported, wanted,
        "case {case:?}: expected {wanted:?}, got {reported:?}"
    );

    // And it still holds none after an update that changes something else.
    let case = "an update of that subscriber";
    wire.ok::<UpdateSubscriber<InMemoryEngine>>(
        case,
        &UpdateSubscriberInput {
            subscriber_arn: arn.clone(),
            batch_configuration: Some(BatchConfiguration {
                max_batch_size: Some(3),
                max_batch_window_in_seconds: None,
            }),
            ..Default::default()
        },
    )
    .await;
    let reported = effective_of(&read(&wire, case, &arn).await);
    let wanted = Effective {
        transformer: None,
        batch: Some((Some(3), Some(DEFAULT_MAX_BATCH_WINDOW))),
        ..defaults()
    };
    assert_eq!(
        reported, wanted,
        "case {case:?}: expected {wanted:?}, got {reported:?}"
    );
}

/// What a subscriber invokes is fixed at creation — `UpdateInvokeConfiguration`
/// carries no `TargetArn` — so a parameters member for ANOTHER service than the
/// stored target's is a member that target cannot be invoked through: the update
/// is refused `InvalidInputException` (400) whatever transformation the
/// subscriber holds, and writes nothing, so the effective configuration a read
/// reports is the one the create left.
#[tokio::test]
async fn an_update_naming_parameters_the_stored_target_does_not_take_is_refused() {
    let wire = Wire::new();
    let bus = wire
        .active_bus("the universal switch's bus", "universal-switch-bus")
        .await;

    // (case, the Transformer the create names, the whole Message of the refusal)
    let rows: [(&str, Option<&str>, Refusal); 3] = [
        (
            "a subscriber that configured no transformer",
            None,
            Refusal::ParametersMismatch,
        ),
        (
            "a subscriber that named the transformation the service applies anyway",
            Some(DEFAULT_TRANSFORMER_TYPE),
            Refusal::ParametersMismatch,
        ),
        (
            "a subscriber that named a transformation of its own",
            Some("WITH_METADATA"),
            Refusal::TransformerOnUniversalTarget,
        ),
    ];

    for (index, (case, named, expected)) in rows.into_iter().enumerate() {
        let mut input = create_subscriber(&format!("universal-switch-{index}"), &bus, None);
        input.transformer = named.map(|r#type| Transformer {
            r#type: Some(r#type.to_string()),
            jsonata_configuration: None,
        });
        let (arn, before) = created_and_read(&wire, case, input).await;
        let invoked = before
            .invoke_configuration
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: the create must report what it invokes"));

        let update = UpdateSubscriberInput {
            subscriber_arn: arn.clone(),
            invoke_configuration: Some(UpdateInvokeConfiguration {
                role_arn: invoked.role_arn.clone(),
                universal_target_parameters: Some(UniversalTargetParameters {
                    input: "{}".to_string(),
                    invocation_timeout_seconds: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (status, body) = wire
            .call::<UpdateSubscriber<InMemoryEngine>>(&update)
            .await;
        assert_eq!(
            status,
            400,
            "case {case:?}: the update must be refused 400, got {status} with {:?}",
            rendered(&body)
        );
        let (code, message) = refusal(case, &body);
        let wanted_message = match expected {
            // The member the request named, the target it named it for, and the
            // member that target takes instead — the target ARN read back off
            // the created subscriber rather than written out here.
            Refusal::ParametersMismatch => format!(
                "UniversalTargetParameters is not supported for target {}. Expected SqsParameters",
                invoked.target_arn
            ),
            Refusal::TransformerOnUniversalTarget => TRANSFORMER_ON_UNIVERSAL_TARGET.to_string(),
        };
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_INPUT_EXCEPTION, wanted_message.as_str()),
            "case {case:?}: expected {INVALID_INPUT_EXCEPTION} saying {wanted_message:?}, got \
             {code:?} saying {message:?}"
        );
        let after = effective_of(&read(&wire, case, &arn).await);
        // The transformation the create left the subscriber: the one it named,
        // or the one the service applies where it named none.
        let wanted = Effective {
            transformer: Some(Some(
                named.unwrap_or(DEFAULT_TRANSFORMER_TYPE).to_string(),
            )),
            ..defaults()
        };
        assert_eq!(
            after, wanted,
            "case {case:?}: a refused update must write nothing, expected {wanted:?}, got \
             {after:?}"
        );
    }
}

/// Which reading of the chain refuses a row of the table above: the parameters
/// member the stored target cannot be invoked through, or — where the subscriber
/// holds a transformation its caller asked for — that transformation against the
/// universal parameters the request carries, which is read first.
enum Refusal {
    ParametersMismatch,
    TransformerOnUniversalTarget,
}

/// The whole `Message` of the refusal a transformation on a universal target is
/// answered with: `Transformer` is "Not applicable to universal (aws-sdk)
/// targets, whose input transformation is UniversalTargetParameters.Input; a
/// Transformer on such a target is rejected".
const TRANSFORMER_ON_UNIVERSAL_TARGET: &str = "Transformer: a universal target takes its input \
     transformation from UniversalTargetParameters.Input and accepts no Transformer";

// ---------------------------------------------------------------------------
// RULE: an update settles it again over what it leaves behind
// ---------------------------------------------------------------------------

/// An update leaves the subscriber an effective value for every one of these
/// members: a configuration it carries in part takes the service's default for
/// each member it did not name, and a member it does not carry at all stands.
#[tokio::test]
async fn an_update_leaves_the_subscriber_an_effective_configuration() {
    let wire = Wire::new();
    let bus = wire.active_bus("the updated subscribers' bus", "effective-update-bus").await;

    // (case, the update, the effective configuration it must leave behind)
    type Row = (
        &'static str,
        fn(String) -> UpdateSubscriberInput,
        fn() -> Effective,
    );
    let rows: [Row; 6] = [
        (
            "an update carrying nothing at all",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                ..Default::default()
            },
            || Effective {
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "a batch size alone, which defaults the window beside it",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                batch_configuration: Some(BatchConfiguration {
                    max_batch_size: Some(5),
                    max_batch_window_in_seconds: None,
                }),
                ..Default::default()
            },
            || Effective {
                batch: Some((Some(5), Some(DEFAULT_MAX_BATCH_WINDOW))),
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "a batch window alone, which defaults the size beside it",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                batch_configuration: Some(BatchConfiguration {
                    max_batch_size: None,
                    max_batch_window_in_seconds: Some(MAX_BATCH_WINDOW_CEILING),
                }),
                ..Default::default()
            },
            || Effective {
                batch: Some((
                    Some(DEFAULT_MAX_BATCH_SIZE),
                    Some(MAX_BATCH_WINDOW_CEILING),
                )),
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "a retry policy naming only its attempt count",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                retry_policy: Some(RetryPolicy {
                    max_retry_attempts: Some(1),
                    ..Default::default()
                }),
                ..Default::default()
            },
            || Effective {
                retry: Some((
                    Some(1),
                    Some(DEFAULT_MAX_EVENT_AGE),
                    Some(DEFAULT_RETRY_STRATEGY.to_string()),
                )),
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "filters naming no language, which replace the ones it held",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                filter_configuration: Some(filter_configuration(OTHER_PATTERN, None)),
                ..Default::default()
            },
            || Effective {
                language: Some(Some(DEFAULT_FILTER_LANGUAGE.to_string())),
                filters: Some(1),
                ..defaults()
            },
        ),
        (
            "the empty filter configuration, which clears the filter and its language with it",
            |arn| UpdateSubscriberInput {
                subscriber_arn: arn,
                filter_configuration: Some(FilterConfiguration::default()),
                ..Default::default()
            },
            defaults,
        ),
    ];

    for (index, (case, update, expected)) in rows.into_iter().enumerate() {
        // Each row starts from a subscriber created with one filter and nothing
        // else of these members, so the effective configuration it starts from
        // is the defaults plus that filter.
        let mut input = create_subscriber(&format!("effective-updated-{index}"), &bus, None);
        input.filter_configuration = Some(filter_configuration(PATTERN, None));
        let (arn, _) = created_and_read(&wire, case, input).await;

        wire.ok::<UpdateSubscriber<InMemoryEngine>>(case, &update(arn.clone()))
            .await;
        let reported = effective_of(&read(&wire, case, &arn).await);
        let wanted = expected();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the effective configuration {wanted:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: free text that holds no non-whitespace character describes nothing
// ---------------------------------------------------------------------------

/// A `Description` holding no non-whitespace character leaves the resource with
/// none, on every resource type of this service that has one and whether the
/// create or the update carries it; text with whitespace AROUND it is a
/// description and is kept exactly as sent.
#[tokio::test]
async fn a_description_that_holds_no_text_leaves_the_resource_with_none() {
    let wire = Wire::new();
    let bus = wire.active_bus("the described resources' bus", "description-bus").await;

    // (case, the Description the request carries, the description the resource
    // keeps)
    let rows: [(&str, &str, Option<&str>); 6] = [
        ("the empty string", "", None),
        ("one space", " ", None),
        ("several spaces", "    ", None),
        // The member's `@pattern` excludes the control characters, so tabs and
        // newlines never reach a handler; this is the other whitespace it
        // admits.
        ("a non-breaking space", "\u{00a0}", None),
        ("text", "paused for maintenance", Some("paused for maintenance")),
        (
            "text with whitespace around it",
            "  padded description  ",
            Some("  padded description  "),
        ),
    ];

    for (index, (case, description, kept)) in rows.into_iter().enumerate() {
        // The subscriber, on the create...
        let mut input = create_subscriber(&format!("described-subscriber-{index}"), &bus, None);
        input.description = Some(description.to_string());
        let (arn, described) = created_and_read(&wire, case, input).await;
        assert_eq!(
            described.description.as_deref(),
            kept,
            "case {case:?}: a created subscriber must keep {kept:?}, got {:?}",
            described.description
        );

        // ...and on the update, over a subscriber that holds a description.
        let case_update = format!("{case}, carried by an update");
        wire.ok::<UpdateSubscriber<InMemoryEngine>>(
            &case_update,
            &UpdateSubscriberInput {
                subscriber_arn: arn.clone(),
                description: Some("the description it held before".to_string()),
                ..Default::default()
            },
        )
        .await;
        wire.ok::<UpdateSubscriber<InMemoryEngine>>(
            &case_update,
            &UpdateSubscriberInput {
                subscriber_arn: arn.clone(),
                description: Some(description.to_string()),
                ..Default::default()
            },
        )
        .await;
        let described = read(&wire, &case_update, &arn).await;
        assert_eq!(
            described.description.as_deref(),
            kept,
            "case {case_update:?}: an updated subscriber must keep {kept:?}, got {:?}",
            described.description
        );

        // The event source, which carries the same member.
        let mut source = create_source(&format!("described-source-{index}"), &bus, None);
        source.description = Some(description.to_string());
        let created = wire
            .ok::<CreateEventSource<InMemoryEngine>>(case, &with_client_token(source))
            .await;
        let source_arn = created.event_source_arn.clone().unwrap_or_else(|| {
            panic!("case {case:?}: expected a minted EventSourceArn, got {created:?}")
        });
        let read_source = wire
            .ok::<DescribeEventSource<InMemoryEngine>>(
                case,
                &DescribeEventSourceInput {
                    event_source_arn: source_arn,
                },
            )
            .await;
        assert_eq!(
            read_source.description.as_deref(),
            kept,
            "case {case:?}: a created event source must keep {kept:?}, got {:?}",
            read_source.description
        );
    }
}
