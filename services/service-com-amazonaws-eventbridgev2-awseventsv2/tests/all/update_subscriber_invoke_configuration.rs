// SPDX-License-Identifier: CC0-1.0

//! The contract an UPDATE of a subscriber's `InvokeConfiguration` is held to,
//! read where a client reads it: off the `rpcv2Cbor` wire.
//!
//! RULE 1 — what a subscriber invokes is fixed at creation
//! (`UpdateInvokeConfiguration` carries no `TargetArn`), and each target service
//! is invoked through its OWN parameters member, so a parameters member the
//! service of the stored `TargetArn` does not take names a call this service
//! could never make: the update is refused `InvalidInputException` (400) saying
//! `<Supplied> is not supported for target <TargetArn>. Expected <Expected>`,
//! while the member that target DOES take, and a configuration naming no
//! parameters member at all, are written.
//!
//! RULE 2 — `UpdateInvokeConfiguration$RoleArn` is the "IAM role the service
//! assumes to invoke the target. Must belong to the calling account", so a role
//! of ANOTHER account is one no delivery could be made under and the caller may
//! not configure it: the update is refused `AccessDeniedException` (403) with
//! `Access denied.` as its whole message, while a role of the calling account —
//! at the account root or under a path — is written.
//!
//! RULE 3 — a refused update writes NOTHING, so a `DescribeSubscriber` that
//! follows one reports the configuration the subscriber had before it, down to
//! the role, the parameters member, the filters and the transformer.
//!
//! The rows span what those rules generalise over: RULE 1 over every target
//! service this crate's targets table names and over which other member the
//! request supplies for it, and RULE 2 over the account the role belongs to and
//! the path it sits under.
//!
//! Nothing here waits on the wall clock: the one wait is the bus's own
//! provisioning transition, polled for the CONDITION by the shared harness.

use engine::InMemoryEngine;

use service_com_amazonaws_eventbridgev2_awseventsv2 as eventsv2;

use eventsv2::ops::create_subscriber::CreateSubscriber;
use eventsv2::ops::describe_subscriber::DescribeSubscriber;
use eventsv2::ops::types::{
    DescribeSubscriberInput, DescribeSubscriberOutput, EventBusV2Parameters, Filter,
    FilterConfiguration, HttpParameters, InvokeConfiguration, KinesisParameters, LambdaParameters,
    SnsParameters, SqsParameters, StepFunctionsParameters, Transformer, UpdateInvokeConfiguration,
    UpdateSubscriberInput,
};
use eventsv2::ops::update_subscriber::UpdateSubscriber;

use crate::modelled_input_refusal::{
    create_subscriber, refusal, rendered, with_client_token, Wire, INVALID_INPUT_EXCEPTION, REGION,
};

/// The error shape a caller that may not act under a value it supplied is
/// refused with, and the `__type` that refusal carries.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The whole `Message` of that refusal: `AccessDeniedException` is "The caller
/// does not have the permissions required to perform the operation", and the
/// wording names neither the role nor the target.
const ACCESS_DENIED: &str = "Access denied.";

/// An account that is not the one a request reaching this emulator arrives as,
/// written as the twelve digits `RoleArn`'s modelled `@pattern` requires.
const FOREIGN_ACCOUNT: &str = "999988887777";

/// An event pattern a filter can carry, of the one language this service reads.
const PATTERN: &str = r#"{"source":["invoke.contract"]}"#;

/// The transformation the subscriber of every row below is created with, so a
/// refused update is visible as having left it alone.
const TRANSFORMER_TYPE: &str = "WITH_METADATA";

/// The target-parameter members `UpdateInvokeConfiguration` declares, each named
/// as the model names it.
///
/// Source: `smithy_query(query="shape", shape="UpdateInvokeConfiguration")` —
/// `LambdaParameters | SqsParameters | SnsParameters | KinesisParameters |
/// StepFunctionsParameters | HttpParameters | UniversalTargetParameters |
/// EventBusV2Parameters`. `UniversalTargetParameters` is not among the rows
/// here: its own target form carries a required `Input` and a stated batching,
/// which `crate::subscriber_effective_configuration` drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parameters {
    Lambda,
    Sqs,
    Sns,
    Kinesis,
    StepFunctions,
    Http,
    EventBusV2,
}

impl Parameters {
    /// The member name a refusal and a read both name this block by.
    fn member(self) -> &'static str {
        match self {
            Self::Lambda => "LambdaParameters",
            Self::Sqs => "SqsParameters",
            Self::Sns => "SnsParameters",
            Self::Kinesis => "KinesisParameters",
            Self::StepFunctions => "StepFunctionsParameters",
            Self::Http => "HttpParameters",
            Self::EventBusV2 => "EventBusV2Parameters",
        }
    }

    /// `configuration` carrying this block, as a request states it.
    fn written_onto(self, configuration: &mut UpdateInvokeConfiguration) {
        match self {
            Self::Lambda => {
                configuration.lambda_parameters = Some(LambdaParameters::default());
            }
            Self::Sqs => {
                configuration.sqs_parameters = Some(SqsParameters {
                    message_group_id: Some("invoke-contract-group".to_string()),
                    ..Default::default()
                });
            }
            Self::Sns => {
                configuration.sns_parameters = Some(SnsParameters::default());
            }
            Self::Kinesis => {
                configuration.kinesis_parameters = Some(KinesisParameters {
                    partition_key: Some("invoke-contract-key".to_string()),
                    ..Default::default()
                });
            }
            Self::StepFunctions => {
                configuration.step_functions_parameters = Some(StepFunctionsParameters::default());
            }
            Self::Http => {
                configuration.http_parameters = Some(HttpParameters::default());
            }
            Self::EventBusV2 => {
                configuration.event_bus_v2_parameters = Some(EventBusV2Parameters::default());
            }
        }
    }
}

/// The parameters member `configuration` carries, read back the way a client
/// reads it: whichever of the declared blocks the response holds.
fn carried(configuration: &InvokeConfiguration) -> Option<&'static str> {
    [
        (
            Parameters::Lambda,
            configuration.lambda_parameters.is_some(),
        ),
        (Parameters::Sqs, configuration.sqs_parameters.is_some()),
        (Parameters::Sns, configuration.sns_parameters.is_some()),
        (
            Parameters::Kinesis,
            configuration.kinesis_parameters.is_some(),
        ),
        (
            Parameters::StepFunctions,
            configuration.step_functions_parameters.is_some(),
        ),
        (Parameters::Http, configuration.http_parameters.is_some()),
        (
            Parameters::EventBusV2,
            configuration.event_bus_v2_parameters.is_some(),
        ),
    ]
    .into_iter()
    .find_map(|(block, present)| present.then_some(block.member()))
}

/// An `UpdateInvokeConfiguration` naming `role_arn` and carrying `parameters`,
/// which is the whole of what an update can rewrite about how a subscriber
/// invokes its target.
fn update_invoke(role_arn: &str, parameters: Option<Parameters>) -> UpdateInvokeConfiguration {
    let mut configuration = UpdateInvokeConfiguration {
        role_arn: role_arn.to_string(),
        ..Default::default()
    };
    if let Some(parameters) = parameters {
        parameters.written_onto(&mut configuration);
    }
    configuration
}

/// The same configuration as a create states it: the target a subscriber is
/// minted on beside the members an update can rewrite.
fn invoke(
    target_arn: &str,
    role_arn: &str,
    parameters: Option<Parameters>,
) -> InvokeConfiguration {
    let configuration = update_invoke(role_arn, parameters);
    InvokeConfiguration {
        target_arn: target_arn.to_string(),
        role_arn: configuration.role_arn,
        lambda_parameters: configuration.lambda_parameters,
        sqs_parameters: configuration.sqs_parameters,
        sns_parameters: configuration.sns_parameters,
        kinesis_parameters: configuration.kinesis_parameters,
        step_functions_parameters: configuration.step_functions_parameters,
        http_parameters: configuration.http_parameters,
        universal_target_parameters: configuration.universal_target_parameters,
        event_bus_v2_parameters: configuration.event_bus_v2_parameters,
    }
}

/// An IAM role ARN of `account` under `path`, of the form `RoleArn`'s modelled
/// `@pattern` admits (`^arn:aws(-[a-z0-9]+)*:iam::\d{12}:role\/[\w+=,.@/-]+$`).
fn role(account: &str, path: &str, name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{path}{name}")
}

/// The role of the CALLING account every subscriber below is created with.
fn own_role() -> String {
    role(engine::emulator_account_id(), "", "invoke-contract")
}

/// The facts about a subscriber's configuration a refused update must leave
/// alone and a served one must change, as `DescribeSubscriber` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Configured {
    target_arn: Option<String>,
    role_arn: Option<String>,
    parameters: Option<&'static str>,
    filters: Option<Vec<String>>,
    transformer: Option<String>,
}

/// Those facts, read off a `DescribeSubscriber` response.
fn configured_of(read: &DescribeSubscriberOutput) -> Configured {
    Configured {
        target_arn: read
            .invoke_configuration
            .as_ref()
            .map(|invoked| invoked.target_arn.clone()),
        role_arn: read
            .invoke_configuration
            .as_ref()
            .map(|invoked| invoked.role_arn.clone()),
        parameters: read.invoke_configuration.as_ref().and_then(carried),
        filters: read.filter_configuration.as_ref().and_then(|configuration| {
            configuration.filters.as_ref().map(|filters| {
                filters
                    .iter()
                    .map(|filter| filter.pattern.clone())
                    .collect()
            })
        }),
        transformer: read
            .transformer
            .as_ref()
            .and_then(|transformer| transformer.r#type.clone()),
    }
}

/// Create a subscriber on `bus` invoking `target_arn` through `parameters`, and
/// hand back its ARN beside what a read reports about its configuration.
async fn created(
    wire: &Wire,
    case: &str,
    name: &str,
    bus: &str,
    target_arn: &str,
    parameters: Option<Parameters>,
) -> (String, Configured) {
    let mut input = create_subscriber(name, bus, None);
    input.invoke_configuration = invoke(target_arn, &own_role(), parameters);
    input.filter_configuration = Some(FilterConfiguration {
        language: None,
        filters: Some(vec![Filter {
            pattern: PATTERN.to_string(),
            scope: "METADATA".to_string(),
        }]),
    });
    input.transformer = Some(Transformer {
        r#type: Some(TRANSFORMER_TYPE.to_string()),
        jsonata_configuration: None,
    });
    let arn = wire
        .ok::<CreateSubscriber<InMemoryEngine>>(case, &with_client_token(input))
        .await
        .subscriber_arn
        .unwrap_or_else(|| panic!("case {case:?}: CreateSubscriber must report a SubscriberArn"));
    let reported = configured_of(&read(wire, case, &arn).await);
    (arn, reported)
}

/// The subscriber at `arn` as `DescribeSubscriber` reports it.
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
// RULE 1: the parameters member the stored target takes
// ---------------------------------------------------------------------------

/// Every target service the crate invokes, each with the ARN form that names one
/// of its resources and the parameters member it is invoked through.
///
/// Source: `smithy_query(query="shape", shape="UpdateInvokeConfiguration")` for
/// the member names, and each service's own ARN grammar for the target form.
/// `{bus}` stands for an event bus of this service, substituted per run because
/// an `event-busv2` ARN carries an identifier only the emulator mints.
const TARGET_SERVICES: [(&str, &str, Parameters); 7] = [
    ("an SQS queue", "arn:aws:sqs:{region}:{account}:invoke-contract-queue", Parameters::Sqs),
    ("an SNS topic", "arn:aws:sns:{region}:{account}:invoke-contract-topic", Parameters::Sns),
    (
        "a Lambda function",
        "arn:aws:lambda:{region}:{account}:function:invoke-contract",
        Parameters::Lambda,
    ),
    (
        "a Kinesis stream",
        "arn:aws:kinesis:{region}:{account}:stream/invoke-contract",
        Parameters::Kinesis,
    ),
    (
        "a Step Functions state machine",
        "arn:aws:states:{region}:{account}:stateMachine:invoke-contract",
        Parameters::StepFunctions,
    ),
    (
        "an API Gateway stage",
        "arn:aws:execute-api:{region}:{account}:abcdef1234/live/POST/events",
        Parameters::Http,
    ),
    ("another event bus", "{bus}", Parameters::EventBusV2),
];

/// `form` with the per-run values a target ARN carries filled in.
fn target(form: &str, bus: &str) -> String {
    form.replace("{region}", REGION)
        .replace("{account}", engine::emulator_account_id())
        .replace("{bus}", bus)
}

/// RULE: an update's parameters member must be the one the service of the
/// subscriber's stored `TargetArn` is invoked through — every other member is
/// refused `InvalidInputException` (400) naming the member supplied, the target
/// it was supplied for and the member that target takes — and the refusal writes
/// nothing.
#[tokio::test]
async fn an_update_names_the_parameters_member_its_stored_target_takes() {
    let wire = Wire::new();
    let bus = wire
        .active_bus("the invoke contract's bus", "invoke-contract-bus")
        .await;
    let other_bus = wire
        .active_bus("the forwarded-to bus", "invoke-contract-downstream")
        .await;

    for (index, (case, form, expected)) in TARGET_SERVICES.into_iter().enumerate() {
        let target_arn = target(form, &other_bus);
        // The member supplied in error is the NEXT service's one, so each row
        // is a different (target, supplied) pair rather than the recorded one.
        let supplied = TARGET_SERVICES[(index + 1) % TARGET_SERVICES.len()].2;
        assert_ne!(
            supplied,
            expected,
            "case {case:?}: the row must supply a member other than the one its target takes"
        );

        let (arn, before) = created(
            &wire,
            case,
            &format!("invoke-contract-{index}"),
            &bus,
            &target_arn,
            Some(expected),
        )
        .await;

        let (status, body) = wire
            .call::<UpdateSubscriber<InMemoryEngine>>(&UpdateSubscriberInput {
                subscriber_arn: arn.clone(),
                invoke_configuration: Some(update_invoke(&own_role(), Some(supplied))),
                ..Default::default()
            })
            .await;
        assert_eq!(
            status,
            400,
            "case {case:?}: {} for target {target_arn} must be refused 400, got {status} with {:?}",
            supplied.member(),
            rendered(&body)
        );
        let (code, message) = refusal(case, &body);
        let wanted = format!(
            "{} is not supported for target {target_arn}. Expected {}",
            supplied.member(),
            expected.member()
        );
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_INPUT_EXCEPTION, wanted.as_str()),
            "case {case:?}: expected {INVALID_INPUT_EXCEPTION} saying {wanted:?}, got {code:?} \
             saying {message:?}"
        );

        let after = configured_of(&read(&wire, case, &arn).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must write nothing, expected {before:?}, got \
             {after:?}"
        );
    }
}

/// RULE: the member the stored target DOES take is written, and so is a
/// configuration that names no parameters member at all — the configuration
/// replaces the stored one wholesale, so the member it omits is gone from the
/// read.
#[tokio::test]
async fn an_update_writes_the_parameters_member_its_stored_target_takes() {
    let wire = Wire::new();
    let bus = wire
        .active_bus("the served contract's bus", "invoke-served-bus")
        .await;
    let other_bus = wire
        .active_bus("the served forwarded-to bus", "invoke-served-downstream")
        .await;

    // (case, the member the update carries, the member the read must report)
    for (index, (service, form, expected)) in TARGET_SERVICES.into_iter().enumerate() {
        let target_arn = target(form, &other_bus);
        let rows: [(&str, Option<Parameters>, Option<&'static str>); 2] = [
            ("the member its target takes", Some(expected), Some(expected.member())),
            ("no parameters member at all", None, None),
        ];
        for (row, (case, supplied, reported)) in rows.into_iter().enumerate() {
            let case = &format!("{service}, {case}");
            let (arn, before) = created(
                &wire,
                case,
                &format!("invoke-served-{index}-{row}"),
                &bus,
                &target_arn,
                Some(expected),
            )
            .await;

            // A role of the calling account other than the created one, so the
            // write is visible as a CHANGE rather than as the created value.
            let moved_role = role(engine::emulator_account_id(), "", "invoke-contract-two");
            wire.ok::<UpdateSubscriber<InMemoryEngine>>(
                case,
                &UpdateSubscriberInput {
                    subscriber_arn: arn.clone(),
                    invoke_configuration: Some(update_invoke(&moved_role, supplied)),
                    ..Default::default()
                },
            )
            .await;

            let after = configured_of(&read(&wire, case, &arn).await);
            let wanted = Configured {
                role_arn: Some(moved_role),
                parameters: reported,
                ..before.clone()
            };
            assert_eq!(
                after, wanted,
                "case {case:?}: expected {wanted:?} after the update, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the account the role belongs to
// ---------------------------------------------------------------------------

/// RULE: `UpdateInvokeConfiguration$RoleArn` "Must belong to the calling
/// account", so a role of another account is refused `AccessDeniedException`
/// (403) with `Access denied.` as its whole message and writes nothing, while a
/// role of the calling account — at the account root or under a path — is
/// written.
#[tokio::test]
async fn an_update_assumes_only_a_role_of_the_calling_account() {
    let wire = Wire::new();
    let bus = wire
        .active_bus("the role contract's bus", "invoke-role-bus")
        .await;
    let target_arn = target(TARGET_SERVICES[0].1, &bus);

    // (case, the role the update names, whether the caller may act under it)
    let own = engine::emulator_account_id();
    let rows: [(&str, String, bool); 5] = [
        (
            "a role of the calling account",
            role(own, "", "invoke-role-direct"),
            true,
        ),
        (
            "a role of the calling account under a path",
            role(own, "service-role/", "invoke-role-pathed"),
            true,
        ),
        (
            "a role of another account",
            role(FOREIGN_ACCOUNT, "", "invoke-role-foreign"),
            false,
        ),
        (
            "a role of another account under a path",
            role(FOREIGN_ACCOUNT, "service-role/", "invoke-role-foreign"),
            false,
        ),
        (
            "a role of another account under the same name as the caller's",
            role(FOREIGN_ACCOUNT, "", "invoke-contract"),
            false,
        ),
    ];

    for (index, (case, named, permitted)) in rows.into_iter().enumerate() {
        let (arn, before) = created(
            &wire,
            case,
            &format!("invoke-role-{index}"),
            &bus,
            &target_arn,
            Some(Parameters::Sqs),
        )
        .await;

        let (status, body) = wire
            .call::<UpdateSubscriber<InMemoryEngine>>(&UpdateSubscriberInput {
                subscriber_arn: arn.clone(),
                invoke_configuration: Some(update_invoke(&named, Some(Parameters::Sqs))),
                ..Default::default()
            })
            .await;
        let after = configured_of(&read(&wire, case, &arn).await);

        if permitted {
            assert_eq!(
                status,
                200,
                "case {case:?}: the role {named} is the caller's own and must be written, got \
                 {status} with {:?}",
                rendered(&body)
            );
            let wanted = Configured {
                role_arn: Some(named.clone()),
                ..before.clone()
            };
            assert_eq!(
                after, wanted,
                "case {case:?}: expected {wanted:?} after the update, got {after:?}"
            );
        } else {
            assert_eq!(
                status,
                403,
                "case {case:?}: the role {named} belongs to another account and must be refused \
                 403, got {status} with {:?}",
                rendered(&body)
            );
            let (code, message) = refusal(case, &body);
            assert_eq!(
                (code.as_str(), message.as_str()),
                (ACCESS_DENIED_EXCEPTION, ACCESS_DENIED),
                "case {case:?}: expected {ACCESS_DENIED_EXCEPTION} saying {ACCESS_DENIED:?}, got \
                 {code:?} saying {message:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must write nothing, expected {before:?}, got \
                 {after:?}"
            );
        }
    }
}
