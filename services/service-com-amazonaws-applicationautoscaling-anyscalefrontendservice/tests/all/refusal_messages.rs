// SPDX-License-Identifier: CC0-1.0

//! What a refusal this SERVICE reaches SAYS.
//!
//! The rule under test, one sentence: a refusal Application Auto Scaling itself reaches
//! carries the service's own statement of the rule that was broken — naming the request's
//! own values where the rule is about them — and never the protocol boundary validator's
//! `Value 'X' at 'member' failed to satisfy constraint: Member must …` template, never a
//! raw constraint pattern, and never a value the caller did not supply (the region the
//! request happens to be signed for).
//!
//! Each suite below asserts that rule over a TABLE spanning the class it covers, not over
//! the one request an issue cited:
//!
//! * a DynamoDB resource that cannot carry a scalable target is refused by the KIND of
//!   resource that is missing, naming the request's own `ResourceId`;
//! * an inverted capacity window carries ONE sentence wherever a window is given;
//! * a schedule expression that does not parse reports where the parse stopped and the
//!   token it stopped on, and one that is not a schedule at all is answered with the
//!   grammar;
//! * an illegal `ScheduledActionName` is refused (the modelled-constraint message it carries
//!   is pinned by the `recorded_refusal_wording` suite);
//! * a policy configuration that cannot be evaluated is refused with the model's own
//!   sentence for the rule;
//! * a tag operation addressed to an ARN that names no registered scalable target answers in
//!   its own terms: the READ reports that the resource is not there, while the two WRITES
//!   refuse the write they were asked to make (`refusal_texts` pins that sentence).

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, Emulator, REGION};

/// What no refusal this service reaches may contain: the boundary validator's template for
/// a MODELLED constraint, which is not what the service answers its own rules with.
const CONSTRAINT_TEMPLATE: &str = "failed to satisfy constraint";

/// Assert the shape every refusal in this file shares.
fn assert_service_text(case: &str, message: &str) {
    assert!(
        !message.contains(CONSTRAINT_TEMPLATE),
        "case {case:?}: expected the service's own refusal text, not the constraint \
         template, got {message:?}"
    );
    assert!(
        !message.contains(REGION),
        "case {case:?}: expected the refusal to name no value the caller did not supply, \
         got {message:?}"
    );
}

/// What the DynamoDB store holds before the registration under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing at all.
    Nothing,
    /// A PROVISIONED table `orders`.
    Provisioned,
    /// A PAY_PER_REQUEST table `orders`.
    OnDemand,
    /// A PROVISIONED table `orders` carrying the global secondary index `by-customer`.
    WithIndex,
}

/// One DynamoDB resource a registration names, and the sentence it is refused with.
struct DynamoRow {
    case: &'static str,
    setup: Setup,
    resource_id: &'static str,
    dimension: &'static str,
    expected: &'static str,
}

/// The sentence a (namespace, resource id, dimension) triple that does not describe one
/// resource is refused with: the combination is what is unsupported, so the text names none of
/// the three.
const UNSUPPORTED_TRIPLE: &str = "Unsupported service namespace, resource type or scalable \
                                  dimension";

/// The catalog: the four DynamoDB resolution families, each on more than the one resource id
/// the issue cited, so what is pinned is the sentence rather than one request's values.
const DYNAMO_ROWS: &[DynamoRow] = &[
    DynamoRow {
        case: "a table that does not exist",
        setup: Setup::Nothing,
        resource_id: "table/orders",
        dimension: "dynamodb:table:ReadCapacityUnits",
        expected: "DynamoDB table does not exist: table/orders",
    },
    DynamoRow {
        case: "another table that does not exist, on the write dimension",
        setup: Setup::Nothing,
        resource_id: "table/no-such-thing",
        dimension: "dynamodb:table:WriteCapacityUnits",
        expected: "DynamoDB table does not exist: table/no-such-thing",
    },
    DynamoRow {
        case: "an index whose table does not exist",
        setup: Setup::Nothing,
        resource_id: "table/ghost/index/by-mail",
        dimension: "dynamodb:index:ReadCapacityUnits",
        expected: "DynamoDB table does not exist: table/ghost/index/by-mail",
    },
    DynamoRow {
        case: "an index the table does not carry",
        setup: Setup::WithIndex,
        resource_id: "table/orders/index/by-mail",
        dimension: "dynamodb:index:ReadCapacityUnits",
        expected: "DynamoDB index does not exist: table/orders/index/by-mail",
    },
    DynamoRow {
        case: "another index the table does not carry",
        setup: Setup::WithIndex,
        resource_id: "table/orders/index/by-anything",
        dimension: "dynamodb:index:WriteCapacityUnits",
        expected: "DynamoDB index does not exist: table/orders/index/by-anything",
    },
    DynamoRow {
        case: "an on-demand table has no provisioned throughput to scale",
        setup: Setup::OnDemand,
        resource_id: "table/orders",
        dimension: "dynamodb:table:WriteCapacityUnits",
        expected: "Validation failed for scalable target. Reason: PAY_PER_REQUEST table mode is \
                   not scalable.",
    },
    DynamoRow {
        case: "a table resource id paired with an index dimension",
        setup: Setup::Provisioned,
        resource_id: "table/orders",
        dimension: "dynamodb:index:ReadCapacityUnits",
        expected: UNSUPPORTED_TRIPLE,
    },
    DynamoRow {
        case: "an index resource id paired with a table dimension",
        setup: Setup::WithIndex,
        resource_id: "table/orders/index/by-customer",
        dimension: "dynamodb:table:ReadCapacityUnits",
        expected: UNSUPPORTED_TRIPLE,
    },
];

/// Create what `setup` describes.
async fn prepare(emulator: &Emulator, setup: Setup) {
    match setup {
        Setup::Nothing => {}
        Setup::Provisioned => emulator.create_provisioned_table("orders", 1, 1).await,
        Setup::OnDemand => emulator.create_on_demand_table("orders").await,
        Setup::WithIndex => {
            emulator
                .create_table_with_index("orders", "by-customer", "customer", 2, 2)
                .await;
        }
    }
}

#[tokio::test]
async fn a_dynamodb_resource_that_cannot_carry_a_target_is_refused_by_its_kind() {
    for row in DYNAMO_ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        prepare(&emulator, row.setup).await;

        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": row.resource_id,
                       "ScalableDimension": row.dimension,
                       "MinCapacity": 1, "MaxCapacity": 10}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );
        assert_service_text(case, &message);
    }
}

#[tokio::test]
async fn a_resolvable_dynamodb_resource_still_registers_and_reads_back() {
    // The refusals above are a RESOLUTION, not a rejection of the whole family: the table
    // and the index that do exist carry their targets, and the registration round-trips.
    let emulator = Emulator::empty();
    emulator
        .create_table_with_index("orders", "by-customer", "customer", 2, 2)
        .await;
    for (case, resource_id, dimension) in [
        ("the table itself", "table/orders", "dynamodb:table:ReadCapacityUnits"),
        (
            "the index the table carries",
            "table/orders/index/by-customer",
            "dynamodb:index:WriteCapacityUnits",
        ),
    ] {
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": resource_id,
                       "ScalableDimension": dimension, "MinCapacity": 2, "MaxCapacity": 8}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registration to be accepted, got {status} {body:?}"
        );
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": "dynamodb", "ResourceIds": [resource_id],
                       "ScalableDimension": dimension}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {described:?}"
        );
        let targets = described["ScalableTargets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            targets.len(),
            1,
            "case {case:?}: expected the registered target to read back, got {described:?}"
        );
        assert_eq!(
            targets[0]["MaxCapacity"],
            json!(8),
            "case {case:?}: expected the window the registration declared, got {targets:?}"
        );
    }
}

/// The one sentence every inverted capacity window is refused with, wherever the window is
/// given.
const INVERTED_WINDOW: &str = "Maximum capacity cannot be less than minimum capacity";

#[tokio::test]
async fn an_inverted_capacity_window_carries_one_sentence_wherever_it_is_given() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    // A first registration whose pair is inverted, over bounds of different magnitudes and
    // distances — the rule is about the ORDER of the pair, not about its values.
    for (case, min, max) in [
        ("the issue's pair", 9, 2),
        ("one apart", 2, 1),
        ("zero below a negative-free window", 1, 0),
        ("far apart", 1_000_000, 7),
    ] {
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "ecs", "ResourceId": "service/prod-cluster/other",
                       "ScalableDimension": "ecs:service:DesiredCount",
                       "MinCapacity": min, "MaxCapacity": max}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, INVERTED_WINDOW,
            "case {case:?}: expected {INVERTED_WINDOW:?}, got {message:?}"
        );
        assert_service_text(case, &message);
    }

    // The same sentence for a MIN-ONLY update that lands above the stored maximum: the
    // window the update leaves behind is the inverted one.
    let mut update = target.clone();
    update["MinCapacity"] = json!(9);
    let (status, body) = emulator.call("RegisterScalableTarget", update).await;
    assert_eq!(
        status, 400,
        "case \"min-only update above the stored max\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_message(&body),
        INVERTED_WINDOW,
        "case \"min-only update above the stored max\": expected {INVERTED_WINDOW:?}, got {body:?}"
    );

    // And for the window a SCHEDULED action moves the target's bounds to.
    let mut action = target.clone();
    action["ScheduledActionName"] = json!("evening");
    action["Schedule"] = json!("rate(5 minutes)");
    action["ScalableTargetAction"] = json!({"MinCapacity": 8, "MaxCapacity": 3});
    let (status, body) = emulator.call("PutScheduledAction", action).await;
    assert_eq!(
        status, 400,
        "case \"a scheduled action's inverted window\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_message(&body),
        INVERTED_WINDOW,
        "case \"a scheduled action's inverted window\": expected {INVERTED_WINDOW:?}, got {body:?}"
    );

    // The bound itself is a window: a pair whose minimum EQUALS its maximum is accepted.
    let mut equal = target.clone();
    equal["MinCapacity"] = json!(4);
    equal["MaxCapacity"] = json!(4);
    let (status, body) = emulator.call("RegisterScalableTarget", equal).await;
    assert_eq!(
        status, 200,
        "case \"a pair that is equal\": expected 200, got {status} {body:?}"
    );
}

/// The prefix every unparseable schedule expression is refused with.
const SCHEDULE_PREFIX: &str = "Invalid schedule expression. Details: ";

/// The grammar the service answers an expression that is not a schedule at all with.
///
/// The three forms are the model's own `Schedule` documentation ("At expressions -
/// at(yyyy-mm-ddThh:mm:ss), Rate expressions - rate(value unit), Cron expressions -
/// cron(fields)"), stated as the syntax a schedule has to have.
const SCHEDULE_SYNTAX: &str = "Invalid schedule expression. Details: Schedule expressions must \
                               have the following syntax: \
                               rate(<number>\\s?(minutes?|hours?|days?)), \
                               cron(<cron_expression>) or at(yyyy-MM-dd'T'HH:mm:ss).";

/// One cron expression that does not parse, with the character POSITION in the cron BODY the
/// parse stopped at, counted from zero, and the TOKEN it stopped on as the parser reads it.
struct CronRow {
    case: &'static str,
    schedule: &'static str,
    position: usize,
    token: &'static str,
}

/// The catalog: a broken token in each of several fields, so what is pinned is "the position
/// and the token", not one expression. The `aws_worded_client_errors` suite pins the whole
/// sentence the two are stated in.
const CRON_ROWS: &[CronRow] = &[
    CronRow {
        case: "a minutes field that is not a minute",
        schedule: "cron(not a cron)",
        position: 0,
        token: "NOT",
    },
    CronRow {
        case: "a named minutes field",
        schedule: "cron(sixty 10 * * ? *)",
        position: 0,
        token: "SIXTY",
    },
    CronRow {
        case: "an hours field out of range",
        schedule: "cron(0 99 * * ? *)",
        position: 2,
        token: "99",
    },
    CronRow {
        case: "a month that is not a month",
        schedule: "cron(0 10 * SMARCH ? *)",
        position: 7,
        token: "SMARCH",
    },
    CronRow {
        case: "a day of week that is not a day",
        schedule: "cron(0 10 ? * FUNDAY *)",
        position: 9,
        token: "FUNDAY",
    },
    CronRow {
        case: "a year that is not a year",
        schedule: "cron(0 10 * * ? someday)",
        position: 11,
        token: "SOMEDAY",
    },
];

#[tokio::test]
async fn an_unparseable_cron_expression_reports_the_position_and_the_token() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    for row in CRON_ROWS {
        let case = row.case;
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        request["Schedule"] = json!(row.schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with(SCHEDULE_PREFIX),
            "case {case:?}: expected the message to open with {SCHEDULE_PREFIX:?}, got {message:?}"
        );
        let position = format!("position {}", row.position);
        assert!(
            message.contains(&position),
            "case {case:?}: expected the message to report {position:?} \u{2014} where \
             {:?} starts in {:?} \u{2014} got {message:?}",
            row.token,
            row.schedule
        );
        assert!(
            message.contains(&format!("'{}'", row.token)),
            "case {case:?}: expected the message to name the token {:?}, got {message:?}",
            row.token
        );
        assert_service_text(case, &message);
    }
}

/// A cron expression carrying the wrong NUMBER of fields: the parse stops where the field
/// that cannot be read is, counted from zero in the cron body.
const CRON_FIELD_COUNT_ROWS: &[(&str, &str, usize)] = &[
    ("five fields, so no year", "cron(0 10 * * ?)", 10),
    ("no fields at all", "cron()", 0),
    ("seven fields", "cron(0 0 10 * * ? *)", 11),
];

#[tokio::test]
async fn a_cron_expression_of_the_wrong_field_count_reports_where_it_stopped() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    for (case, schedule, position) in CRON_FIELD_COUNT_ROWS {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        request["Schedule"] = json!(schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with(SCHEDULE_PREFIX),
            "case {case:?}: expected the message to open with {SCHEDULE_PREFIX:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("position {position}")),
            "case {case:?}: expected the message to report position {position}, got {message:?}"
        );
        assert_service_text(case, &message);
    }
}

#[tokio::test]
async fn an_expression_that_is_not_a_schedule_is_answered_with_the_grammar() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    // Each row is a SYNTAX failure rather than a parse position: the keyword, the rate value
    // or the rate unit is not one the grammar has, so the grammar is what the caller is told.
    for (case, schedule) in [
        ("an unknown rate unit", "rate(5 fortnights)"),
        ("a rate with no unit", "rate(5)"),
        ("a rate of zero", "rate(0 minutes)"),
        ("an at expression that is not a date", "at(tomorrow)"),
        ("an at expression with an impossible date", "at(2030-13-45T99:99:99)"),
        ("no documented form at all", "every monday at 9"),
        // (`Schedule` carries a modelled `@length(min: 1)`, so an EMPTY expression is
        // refused by the boundary validator with the framework's own constraint message
        // before the parser sees it — a modelled constraint, not a rule this service
        // reaches.)
        ("a keyword that is not one of the three", "every(5 minutes)"),
    ] {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        request["Schedule"] = json!(schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, SCHEDULE_SYNTAX,
            "case {case:?}: expected {SCHEDULE_SYNTAX:?}, got {message:?}"
        );
        assert_service_text(case, &message);
    }

    // And the forms the grammar does have are still accepted, and read back.
    for (case, schedule) in [
        ("a rate", "rate(5 minutes)"),
        ("a cron", "cron(0 10 * * ? *)"),
        ("an at expression", "at(2030-01-15T13:30:00)"),
    ] {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!(format!("action-{case}"));
        request["Schedule"] = json!(schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the schedule to be accepted, got {status} {body:?}"
        );
        let (status, described) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "ecs",
                       "ScheduledActionNames": [format!("action-{case}")]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScheduledActions to answer 200, got {described:?}"
        );
        assert_eq!(
            described["ScheduledActions"][0]["Schedule"],
            json!(schedule),
            "case {case:?}: expected the stored action to carry the schedule, got {described:?}"
        );
    }
}

#[tokio::test]
async fn an_illegal_scheduled_action_name_is_refused_and_a_legal_one_is_stored() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    // One row per alternative of the modelled pattern. What the refusal SAYS is the
    // modelled-constraint message, which the `recorded_refusal_wording` suite pins (it is the
    // one refusal of this service that IS the constraint template, because the constraint is
    // the model's own); what is asserted here is that each alternative is refused at all.
    for (case, name) in [
        ("a slash", "scale-up/weekday"),
        ("a colon", "scale:up"),
        ("a pipe", "scale|up"),
        ("a leading space", " scale-up"),
        ("a trailing space", "scale-up "),
        ("a C0 control character", "scale\u{1}up"),
        ("a C1 control character", "scale\u{85}up"),
    ] {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!(name);
        request["Schedule"] = json!("rate(5 minutes)");
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(name),
            "case {case:?}: expected the refusal to echo the submitted name, got {message:?}"
        );
    }

    // A name the pattern allows is still accepted.
    let mut request = target.clone();
    request["ScheduledActionName"] = json!("scale up nightly.2_a");
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"a legal name\": expected 200, got {status} {body:?}"
    );
}

/// One policy configuration that cannot be evaluated, and the sentence it is refused with.
struct PolicyRow {
    case: &'static str,
    /// Which namespace's target the policy is put on, so the metric it names is one that
    /// dimension is scaled on.
    dimension: &'static str,
    policy_type: &'static str,
    configuration: fn() -> Value,
    expected: &'static str,
}

/// The catalog. Each `expected` is the service's own sentence for the rule the configuration
/// breaks — the rules themselves being the model's: "you can specify either a predefined
/// metric or a customized metric", "you can't specify a resource label unless the metric type
/// is ALBRequestCountPerTarget", "at least one step adjustment is required if you are adding a
/// new step scaling policy configuration", "the upper and lower bound can't be null in the
/// same step adjustment", "at most one step adjustment can have a null lower bound" and the
/// same for the upper bound.
const POLICY_ROWS: &[PolicyRow] = &[
    PolicyRow {
        case: "a request-count policy with no resource label",
        dimension: "ecs:service:DesiredCount",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 1000.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ALBRequestCountPerTarget"}})
        },
        expected: "Resource label should be specified for predefined metric type \
                   ALBRequestCountPerTarget",
    },
    PolicyRow {
        case: "a resource label on a metric that is not counted per target",
        dimension: "ecs:service:DesiredCount",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ECSServiceAverageCPUUtilization",
                       "ResourceLabel": "app/my-alb/0/targetgroup/my-tg/0"}})
        },
        expected: "You can't specify a resource label unless the metric type is \
                   ALBRequestCountPerTarget",
    },
    PolicyRow {
        case: "both metric specifications at once",
        dimension: "ecs:service:DesiredCount",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ECSServiceAverageCPUUtilization"},
                   "CustomizedMetricSpecification": {
                       "MetricName": "Backlog", "Namespace": "Mine", "Statistic": "Average"}})
        },
        expected: "Only one of PredefinedMetricSpecification or CustomizedMetricSpecification \
                   can be set for target tracking scaling.",
    },
    PolicyRow {
        case: "neither metric specification",
        dimension: "dynamodb:table:ReadCapacityUnits",
        policy_type: "TargetTrackingScaling",
        configuration: || json!({"TargetValue": 40.0}),
        expected: "You must specify either a predefined metric or a customized metric",
    },
    PolicyRow {
        case: "a step policy with no step adjustment",
        dimension: "dynamodb:table:ReadCapacityUnits",
        policy_type: "StepScaling",
        configuration: || json!({"AdjustmentType": "ChangeInCapacity", "StepAdjustments": []}),
        expected: "There must be at least one step adjustment",
    },
    PolicyRow {
        case: "a step adjustment with neither bound",
        dimension: "ecs:service:DesiredCount",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"ScalingAdjustment": 1}]})
        },
        expected: "Both lower and upper bounds of a step adjustment cannot be left unspecified",
    },
    PolicyRow {
        case: "two step adjustments with a null lower bound",
        dimension: "ecs:service:DesiredCount",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [
                       {"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                       {"MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}]})
        },
        expected: "At most one step adjustment can have a null lower bound",
    },
    PolicyRow {
        case: "two step adjustments with a null upper bound",
        dimension: "ecs:service:DesiredCount",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [
                       {"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1},
                       {"MetricIntervalLowerBound": 10.0, "ScalingAdjustment": 2}]})
        },
        expected: "At most one step adjustment can have a null upper bound",
    },
];

#[tokio::test]
async fn a_policy_configuration_that_cannot_be_evaluated_states_the_documented_rule() {
    let emulator = Emulator::empty();

    for row in POLICY_ROWS {
        let case = row.case;
        let target = emulator.given_target(row.dimension, 1, 20).await;
        let member = if row.policy_type == "StepScaling" {
            "StepScalingPolicyConfiguration"
        } else {
            "TargetTrackingScalingPolicyConfiguration"
        };
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!(row.policy_type);
        request[member] = (row.configuration)();

        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );
        assert_service_text(case, &message);
    }
}

/// The sentence the tag READ carries for an ARN that resolves to no registered scalable
/// target.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The sentence a tag MUTATION carries for the same ARN.
const TAGGING_DENIED: &str = "Access Denied";

#[tokio::test]
async fn a_tag_operation_on_an_unregistered_target_answers_in_its_own_terms() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;
    let mut tagged = target.clone();
    tagged["Tags"] = json!({"owner": "orders-team"});
    tagged["MinCapacity"] = json!(1);
    tagged["MaxCapacity"] = json!(5);

    // The ARN of a target that IS registered answers with its tags: the refusals below are
    // about the target being gone, not about the ARN being unreadable.
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": "ecs"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected 200, got {status} {body:?}"
    );
    let arn = body["ScalableTargets"][0]["ScalableTargetARN"]
        .as_str()
        .expect("the registered target's ARN")
        .to_string();
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a registered target\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );

    // Every ARN that names no registered target now, whether it was minted and retired or
    // never minted at all, is answered alike by each operation.
    let never_minted =
        format!("arn:aws:application-autoscaling:{REGION}:000000000000:scalable-target/0123456789abcdef");
    for (case, resource_arn) in [
        ("a deregistered target's ARN", arn.clone()),
        ("an ARN this emulator never minted", never_minted),
    ] {
        // `ResourceNotFoundException` ("the specified resource doesn't exist"), answered at
        // the client-error status rather than at the 404 the shape's `@httpError` declares.
        let (status, body) = emulator
            .call(
                "ListTagsForResource",
                json!({"ResourceARN": resource_arn.clone()}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on ListTagsForResource: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on ListTagsForResource: expected a ResourceNotFoundException, got \
             {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, TARGET_NOT_FOUND,
            "case {case:?} on ListTagsForResource: expected {TARGET_NOT_FOUND:?}, got {message:?}"
        );
        assert_eq!(
            body["ResourceName"],
            json!(resource_arn),
            "case {case:?}: expected the ARN asked about to be reported in ResourceName, got \
             {body:?}"
        );
        assert_service_text(case, &message);

        // The two writes answer the same condition the same way as each other: an ARN that
        // resolves to no registered target identifies no scalable target whose tags the
        // caller may write, whether this service minted it for a target that has since been
        // deregistered or never minted it at all — so the write is refused rather than the
        // resource reported absent. Neither is the boundary validator's template.
        for (operation, request) in [
            (
                "TagResource",
                json!({"ResourceARN": resource_arn.clone(), "Tags": {"owner": "nobody"}}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": resource_arn.clone(), "TagKeys": ["owner"]}),
            ),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {operation}: expected a ValidationException, got {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, TAGGING_DENIED,
                "case {case:?} on {operation}: expected {TAGGING_DENIED:?}, got {message:?}"
            );
            assert_service_text(case, &message);
        }
    }
}
