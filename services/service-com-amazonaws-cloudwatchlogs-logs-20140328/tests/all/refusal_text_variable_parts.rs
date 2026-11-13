// SPDX-License-Identifier: CC0-1.0

//! What a CloudWatch Logs refusal message is allowed to INTERPOLATE.
//!
//! RULE: the sentence a refusal carries is the service's fixed text for that error class, and
//! its only variable parts are the ones that class names — nothing else of the request is
//! appended to it. Two classes, both reached here over the wire:
//!
//! * a processor the model admits only as the first element of a transformer, used later, is
//!   refused by NAMING that processor and nothing else: the position it was found at, the
//!   length of the pipeline and the member that carried it are not part of the sentence;
//! * a value outside a `@pattern` is refused by stating the member path that failed and the
//!   expression AWS publishes for the TARGET SHAPE — which, for the shapes whose published
//!   form drops the trait's surrounding `^`…`$`, is that expression's body — and never the
//!   value submitted there.
//!
//! The rows span what each rule generalises over rather than one request: for the position
//! rule, which vended parser is misplaced, how far down it sits (every position a 20-element
//! pipeline admits) and how many of them are misplaced at once; for the pattern rule, which
//! shape the member targets, which operation and member carried it, how the member is reached
//! (directly, as a prefix, or as an element of a list) and which character put the value
//! outside the class. Each expectation is DERIVED from the model's own expression by the rule
//! under test, so a row states the rule rather than a transcribed sentence, and a shape AWS
//! publishes anchored is the boundary row.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, messages, Logs};
// The violation envelope and the pattern clause are stated once, in the suite that owns the
// refusal-wording rules, and reused here rather than transcribed.
use crate::refusal_message_fidelity::{one_violation, pattern_requirement};
use crate::transformers::LEADING_ONLY_KINDS;
// The pipeline element and the position rule's fixed sentence are stated once, in the suite
// that owns that rule, and reused here rather than transcribed.
use crate::vended_parser_position::{expected_message as position_message, processor};

/// The code answered when a parameter is specified incorrectly — what `PutTransformer` and
/// the log-group and log-stream operations declare for a refusal.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code the delivery family declares for a refusal, the only refusal shape in its
/// modelled error set.
const VALIDATION: &str = "ValidationException";

/// The elements one `transformerConfig` may hold. Source: the model's `Processors` list
/// (`@length` min 1 max 20), as `PutTransformer`'s own contract states it.
const MAX_PROCESSORS: usize = 20;

/// The log group every `PutTransformer` row below is configured on.
const GROUP: &str = "/refusal-text/variable-parts";

/// A mutation step to pad a pipeline out with, so a misplaced parser can be put at any
/// position without the padding itself being a parser.
fn padding() -> Value {
    json!({"trimString": {"withKeys": ["message"]}})
}

/// A `PutTransformer` body for `pipeline` on this suite's log group.
fn transformer_body(pipeline: &[Value]) -> Value {
    json!({"logGroupIdentifier": GROUP, "transformerConfig": pipeline})
}

/// A pipeline led by `parseJSON` that puts `misplaced` at position `at`, padding the
/// positions between with a mutation step.
fn pipeline_with(misplaced: &str, at: usize) -> Vec<Value> {
    let mut pipeline = vec![processor("parseJSON")];
    pipeline.extend(std::iter::repeat_with(padding).take(at - 1));
    pipeline.push(processor(misplaced));
    pipeline
}

/// A misplaced vended parser is refused by its NAME alone, at every position a pipeline
/// admits one: the sentence carries no index, no pipeline length and no member name, so it
/// reads the same whether the parser sits straight behind the leading one or at the end of a
/// full pipeline.
#[tokio::test]
async fn a_misplaced_vended_parser_is_named_without_its_position() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    // The pipeline the group holds, which every refused put must leave in place.
    let stored = vec![processor("parseJSON")];
    logs.ok("PutTransformer", &transformer_body(&stored)).await;

    // Every position a 20-element pipeline can put a later processor at, with the vended
    // parsers rotated through them so the deep positions are not all one kind's.
    for at in 1..MAX_PROCESSORS {
        let misplaced = LEADING_ONLY_KINDS[at % LEADING_ONLY_KINDS.len()];
        let pipeline = pipeline_with(misplaced, at);
        let case = format!("{misplaced} at position {at} of {}", pipeline.len());
        let expected = position_message(misplaced);

        let (code, message) = logs
            .refusal("PutTransformer", &transformer_body(&pipeline))
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The request-derived clause the sentence must never grow: the member that carried
        // the pipeline and the index the processor was found at. The equality above already
        // pins position-independence — the expectation is built from the kind alone — and
        // this names the clause outright, because a parser kind may itself end in digits.
        assert!(
            !message.contains("transformerConfig") && !message.contains('['),
            "case {case:?}: the sentence names no input member or index, got {message:?}"
        );

        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": GROUP}))
            .await;
        assert_eq!(
            found["transformerConfig"],
            json!(stored),
            "case {case:?}: a refused put leaves the stored pipeline as it was, got {found}"
        );
    }
}

/// With several vended parsers misplaced at once the refusal still names exactly ONE of them
/// — the earliest one, the first the pipeline reads — and still states no position, so the
/// count of offenders is not a variable part of the sentence either.
#[tokio::test]
async fn several_misplaced_vended_parsers_are_refused_by_the_earliest_one() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // (case, the pipeline, the parser the sentence must name)
    let rows: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "two vended parsers straight behind the leading one",
            vec![
                processor("parseJSON"),
                processor("parseWAF"),
                processor("parseVPC"),
            ],
            "parseWAF",
        ),
        (
            "three vended parsers, the earliest deep in the pipeline",
            vec![
                processor("parseJSON"),
                padding(),
                padding(),
                processor("parseRoute53"),
                processor("parsePostgres"),
                processor("parseCloudfront"),
            ],
            "parseRoute53",
        ),
        (
            "a vended parser leading and another later",
            vec![
                processor("parseVPC"),
                padding(),
                processor("parseCloudfront"),
            ],
            "parseCloudfront",
        ),
    ];

    for (case, pipeline, named) in rows {
        let expected = position_message(named);
        let (code, message) = logs
            .refusal("PutTransformer", &transformer_body(&pipeline))
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// One member the model puts a `@pattern` on, and how the refusal that names it must read.
struct PatternCase {
    /// What this row is about, named in every assertion.
    name: &'static str,
    operation: &'static str,
    body: Value,
    /// The error code the SERVED OPERATION declares for a refusal.
    code: &'static str,
    /// The member path the refusal states, or `None` for a member reached through a list,
    /// whose path the framework's validating walk composes — the rule under test is the
    /// EXPRESSION, so such a row asserts the requirement clause and leaves the path alone.
    member: Option<&'static str>,
    /// The `@pattern` the model declares on the member's target shape, verbatim.
    declared: &'static str,
    /// Whether AWS publishes that shape's expression WITHOUT the trait's `^`…`$` anchors.
    /// Source: the recorded refusals of this service — the log group, log stream and
    /// delivery-source names and the log group identifier are published unanchored, while a
    /// delivery id is published exactly as the model spells it.
    published_unanchored: bool,
}

/// The expression a row's refusal must state: the declared one with its anchors dropped where
/// AWS publishes it that way, and otherwise the declared one as the model spells it.
///
/// This is the rule itself rather than a transcription of its output, so a row can only pass
/// by rendering the model's own expression the way AWS renders it.
fn published(case: &PatternCase) -> &'static str {
    if !case.published_unanchored {
        return case.declared;
    }
    case.declared
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: an unanchored published form needs an anchored declaration, got {:?}",
                case.name, case.declared
            )
        })
}

/// An ARN-typed member this service declares no constraint on, so the pattern under test is
/// the only violation in the response.
const SOME_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:delivery-destination:probe";

/// The log group the reads below name, so a row's only violation is its own pattern.
const PATTERN_GROUP: &str = "refusal-text-group";

/// Every row of the pattern table: one refused value, and the shape whose published
/// expression the refusal must state.
fn pattern_cases() -> Vec<PatternCase> {
    vec![
        // `LogGroupIdentifier` (`@length` 1..2048, `@pattern ^[\w#+=/:,.@-]*$`), reached by
        // the member that names a log group by either name or ARN — on a read and on a write
        // of the same resource, so the rendering follows the SHAPE rather than the operation.
        PatternCase {
            name: "a log group identifier with a space and a bang on a read",
            operation: "GetTransformer",
            body: json!({"logGroupIdentifier": "has space!"}),
            code: INVALID_PARAMETER,
            member: Some("logGroupIdentifier"),
            declared: r"^[\w#+=/:,.@-]*$",
            published_unanchored: true,
        },
        PatternCase {
            name: "a log group identifier with a bracket on a write",
            operation: "PutTransformer",
            body: json!({
                "logGroupIdentifier": "group[0]",
                "transformerConfig": [{"parseJSON": {}}],
            }),
            code: INVALID_PARAMETER,
            member: Some("logGroupIdentifier"),
            declared: r"^[\w#+=/:,.@-]*$",
            published_unanchored: true,
        },
        // `LogGroupName` (`@length` 1..512, `@pattern ^[\.\-_/#A-Za-z0-9]+$`), reached as a
        // PREFIX as well as as a name.
        PatternCase {
            name: "a log group name prefix with a space and a bang",
            operation: "DescribeLogGroups",
            body: json!({"logGroupNamePrefix": "bad prefix!"}),
            code: INVALID_PARAMETER,
            member: Some("logGroupNamePrefix"),
            declared: r"^[\.\-_/#A-Za-z0-9]+$",
            published_unanchored: true,
        },
        // `LogStreamName` (`@length` 1..512, `@pattern ^[^:*]*$`), reached by the member that
        // names one, as a prefix of an export, and as an ELEMENT of a list.
        PatternCase {
            name: "a log stream name with a colon",
            operation: "CreateLogStream",
            body: json!({"logGroupName": PATTERN_GROUP, "logStreamName": "has:colon"}),
            code: INVALID_PARAMETER,
            member: Some("logStreamName"),
            declared: r"^[^:*]*$",
            published_unanchored: true,
        },
        PatternCase {
            name: "an export's log stream name prefix with a colon",
            operation: "CreateExportTask",
            body: json!({
                "logGroupName": PATTERN_GROUP,
                "destination": "a-bucket",
                "from": 0,
                "to": 1,
                "logStreamNamePrefix": "pre:fix",
            }),
            code: INVALID_PARAMETER,
            member: Some("logStreamNamePrefix"),
            declared: r"^[^:*]*$",
            published_unanchored: true,
        },
        PatternCase {
            name: "a log stream name with a star inside a list",
            operation: "FilterLogEvents",
            body: json!({
                "logGroupName": PATTERN_GROUP,
                "logStreamNames": ["fine", "has*star"],
            }),
            code: INVALID_PARAMETER,
            member: None,
            declared: r"^[^:*]*$",
            published_unanchored: true,
        },
        // `DeliverySourceName` (`@length` 1..60, `@pattern ^[\w-]*$`), on an operation that
        // declares `ValidationException` rather than `InvalidParameterException`: the
        // rendering is the shape's, not the error code's.
        PatternCase {
            name: "a delivery source name with a space",
            operation: "CreateDelivery",
            body: json!({
                "deliverySourceName": "bad name",
                "deliveryDestinationArn": SOME_ARN,
            }),
            code: VALIDATION,
            member: Some("deliverySourceName"),
            declared: r"^[\w-]*$",
            published_unanchored: true,
        },
        // The boundary: `DeliveryId` (`@length` 1..64, `@pattern ^[0-9A-Za-z]+$`) is published
        // exactly as the model spells it, anchors included — so the rule is "state what AWS
        // publishes for the shape" rather than "always strip the anchors".
        PatternCase {
            name: "a delivery id with a dash, whose shape is published anchored",
            operation: "GetDelivery",
            body: json!({"id": "delivery-1"}),
            code: VALIDATION,
            member: Some("id"),
            declared: r"^[0-9A-Za-z]+$",
            published_unanchored: false,
        },
    ]
}

/// A value outside a `@pattern` is refused by stating the published expression for its target
/// shape and the member path that failed — and nothing of the value.
#[tokio::test]
async fn a_value_outside_a_pattern_states_the_published_expression() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": PATTERN_GROUP}))
        .await;
    for case in pattern_cases() {
        let name = case.name;
        let requirement = pattern_requirement(published(&case));
        let (code, message) = logs.refusal(case.operation, &case.body).await;
        assert_eq!(
            code, case.code,
            "case {name:?}: expected {}, got {code} {message:?}",
            case.code
        );
        match case.member {
            Some(member) => {
                let expected = one_violation(member, &requirement);
                assert_eq!(
                    message, expected,
                    "case {name:?}: expected {expected:?}, got {message:?}"
                );
            }
            None => {
                let tail = format!("failed to satisfy constraint: {requirement}");
                assert!(
                    message.starts_with("1 validation error detected: Value at '")
                        && message.ends_with(&tail),
                    "case {name:?}: expected one clause stating {requirement:?}, got {message:?}"
                );
            }
        }
        // Whatever the shape, the sentence states no anchored expression and quotes no
        // submitted value.
        if case.published_unanchored {
            assert!(
                !message.contains(case.declared),
                "case {name:?}: the published expression drops the anchors, got {message:?}"
            );
        }
        assert!(
            !message.contains("Value '"),
            "case {name:?}: the sentence quotes no submitted value, got {message:?}"
        );
    }
}

/// A value INSIDE each of those classes is not refused for it: the rendering rule narrows what
/// a refusal SAYS and changes nothing about which values are admitted, so a name built only
/// from characters its class holds reaches its operation AND resolves to the resource it names.
///
/// Each row asserts something the store has to answer — the configured pipeline, the named
/// group, the events of the named streams — so it cannot pass against an operation that admits
/// the value and then reports nothing of it.
#[tokio::test]
async fn a_value_inside_a_pattern_resolves_to_the_resource_it_names() {
    let logs = Logs::new();
    // Every character class `LogGroupName` admits — a dot, a dash, an underscore, a slash, a
    // hash, letters and digits — in one name, which is also inside `LogGroupIdentifier`.
    const ADMITTED: &str = "refusal-text.admitted_0/name#1";
    logs.ok("CreateLogGroup", &json!({"logGroupName": ADMITTED}))
        .await;
    // Two stream names inside `LogStreamName`'s class — one plain, one holding a space, which
    // the class admits — each carrying one event, so the filtered read has something to report.
    let streams = ["one", "two three"];
    let base = base_ms();
    for (offset, stream) in streams.iter().enumerate() {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": ADMITTED, "logStreamName": stream}),
        )
        .await;
        let at = base + i64::try_from(offset).expect("an offset fits in an i64");
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": ADMITTED,
                "logStreamName": stream,
                "logEvents": batch(&[(at, &format!("event on {stream}"))]),
            }),
        )
        .await;
    }

    // `logGroupIdentifier` of the admitted characters: the pipeline it configured is the one
    // the read reports back, so the identifier resolved to this group.
    let pipeline = vec![processor("parseJSON")];
    logs.ok(
        "PutTransformer",
        &json!({"logGroupIdentifier": ADMITTED, "transformerConfig": pipeline}),
    )
    .await;
    let found = logs
        .ok("GetTransformer", &json!({"logGroupIdentifier": ADMITTED}))
        .await;
    assert_eq!(
        found["transformerConfig"],
        json!(pipeline),
        "case \"a log group identifier of the admitted characters\": the identifier resolves to \
         the configured group, got {found}"
    );

    // `logGroupNamePrefix` of the admitted characters: the listing names this group.
    let page = logs
        .ok("DescribeLogGroups", &json!({"logGroupNamePrefix": ADMITTED}))
        .await;
    let names: Vec<&str> = page["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group["logGroupName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![ADMITTED],
        "case \"a log group name prefix of the admitted characters\": the prefix selects this \
         group, got {page}"
    );

    // A `logStreamNames` list of admitted names: the filtered read reports the events of both
    // streams it named.
    let filtered = logs
        .ok(
            "FilterLogEvents",
            &json!({"logGroupName": ADMITTED, "logStreamNames": streams}),
        )
        .await;
    let reported = messages(&filtered);
    let expected: Vec<String> = streams
        .iter()
        .map(|stream| format!("event on {stream}"))
        .collect();
    assert_eq!(
        reported, expected,
        "case \"a log stream name list of names holding neither a colon nor a star\": the list \
         resolves to the named streams, expected {expected:?}, got {reported:?}"
    );
}
