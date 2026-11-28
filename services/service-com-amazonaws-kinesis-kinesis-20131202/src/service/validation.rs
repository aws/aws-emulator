// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint violations the
//! framework's own boundary walk found become the `message` a 400 `ValidationException`
//! carries.
//!
//! [`crate::service`] owns the SEAM — which requests are read for violations, and how those
//! violations are collected with the values the caller submitted — and this module owns the
//! sentence, so the clause rules live in ONE place instead of in each operation that trips
//! them. What is ENFORCED is always the model's: every violation handed to [`refusal`] comes
//! from the framework's own walk, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is AWS's `<N> validation error(s) detected: ` over one clause per
//!    violation, and a clause states the value, the member's dotted path and the requirement
//!    it failed in the framework's own words — both come from the shared renderer
//!    (`service_common::validation_message::aws_validation_refusal_withholding_with`), so no
//!    requirement and no member path is spelled in this crate;
//! 2. the value a clause quotes is the one the caller SUBMITTED, in full: the message is the
//!    only place the service reports the offending input back, and a caller comparing the
//!    echo against what it sent has to see its own bytes. The framework's own walk stores a
//!    shortened rendering of a long value, so [`crate::service`] collects the violations
//!    with the submitted values restored before handing them here;
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes to
//!    demand a whole-string match (`service_common::validation::unanchored`), and for the
//!    resource-ARN expression it is the one the service's own front door publishes
//!    ([`STATED_EXPRESSIONS`]);
//! 4. an enumerated type's value set is listed in the order the front door's own HASH TABLE
//!    of those values is walked (`service_common::validation_message::in_hashed_enum_order`)
//!    — ascending bucket of each value's string hash, and the later-declared of two values
//!    that share a bucket first — which is neither the order the model declares its members
//!    in nor lexicographic;
//! 5. one member's clause states NO value at all ([`WITHHELD_VALUE_MEMBERS`]);
//! 6. a constraint broken by an ELEMENT of a list member is stated AT THAT LIST MEMBER — no
//!    element position and no `member` step in the path — quoting the whole list the request
//!    carried and nesting the ELEMENT SHAPE'S WHOLE declared constraint set inside
//!    `Member must satisfy constraint: [ … ]` ([`element_set`]). The set is the element
//!    shape's, not just the constraint that happened to fail, so a request breaking two of an
//!    element's constraints — or carrying two offending elements — is ONE validation error
//!    about one member; it is read off the MODEL ([`input_schemas`]) because a violation
//!    carries only what failed. A constraint broken by the list ITSELF (its `@length`, its
//!    `@uniqueItems`), and one broken by a member INSIDE a structure element, keep their own
//!    clause and their own path.
//!
//! The status (400) and the error code are what the framework already answers a constraint
//! failure with on this `awsJson1_1` service and are restated by that shared renderer, not
//! changed.

use ::std::collections::HashMap;

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::service_common::validation::list_of_element;
use ::smithy_server_runtime::{Refusal, Refused};

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) and for a refusal that is not a constraint failure (an
/// unauthenticated caller keeps the framework's 403 and its wording), which keeps the
/// framework's own.
///
/// `lists` yields every list of scalar elements the REQUEST carried, keyed by the member path
/// the walk reports that list at and already rendered
/// (`service_common::validation::submitted_lists`) — the value the clause stated at a list
/// member quotes, which no violation carries. It is a closure because it is only read when a
/// violation names a list ELEMENT, so a refusal that has none never reads the request again,
/// and the request-less seam passes one that yields nothing: a clause then keeps the
/// framework's own echo rather than a value the caller never sent.
pub(super) fn refusal(
    violations: &[Violation],
    lists: impl FnOnce() -> HashMap<String, String>,
) -> Option<Refused> {
    // The value set of an enumerated type is re-ordered BEFORE the clauses are rendered, so
    // the clause itself — the member path, the echoed value and the requirement's wording —
    // still comes from the one place that writes it.
    let stated: Vec<Violation> = violations
        .iter()
        .map(::service_common::validation_message::in_hashed_enum_order)
        .collect();
    let lists = if stated
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        lists()
    } else {
        HashMap::new()
    };
    ::service_common::validation_message::aws_validation_refusal_over_elements_stating_quoting_withholding(
        &Refusal::Invalid(stated),
        stated_expression,
        |collection| lists.get(collection).cloned(),
        withholds_value,
        element_set,
    )
}

/// The message this service states for violations a HANDLER collected itself, rather than for
/// a refusal the framework's own walk handed over, and `None` when there is nothing to word.
///
/// The same clauses under the same rules through the same [`refusal`], so one broken constraint
/// cannot be worded two ways depending on which layer found it. A handler states the member it
/// collected the violation on, so there is no request to read a list off here.
pub(crate) fn stated_message(violations: &[Violation]) -> Option<String> {
    refusal(violations, HashMap::new).map(|refused| refused.message)
}

/// The requirement the clause stated at the list member `collection` nests: every constraint
/// the ELEMENT shape declares, in the order a clause states them, inside
/// `Member must satisfy constraint: [ … ]` — and `None` for a member no input shape declares
/// element constraints for, whose violations keep the framework's own per-element clause.
///
/// The constraints come from the MODEL ([`input_schemas`]), because a violation carries only
/// the one that failed; each sentence is the framework's own
/// (`service_common::validation_message::requirement`) so a nested requirement reads exactly
/// like one a reported member gets, with this service's two renderings applied to it: an
/// expression as the front door publishes it ([`stated_expression`]) and an enumerated value
/// set in the order the front door lists one (rule 4 above).
fn element_set(collection: &str) -> Option<String> {
    use ::service_common::validation_message::{
        element_set_requirement, hashed_enum_order, requirement,
    };
    let declared =
        ::service_common::validation_message::element_constraint_set(&input_schemas(), collection)?;
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| match kind {
            ViolationKind::Pattern(pattern) => {
                requirement(ViolationKind::Pattern(stated_expression(&pattern)))
            }
            ViolationKind::EnumValue(values) => requirement(ViolationKind::EnumValue(
                hashed_enum_order(&values)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )),
            other => requirement(other),
        })
        .collect();
    Some(element_set_requirement(&requirements))
}

/// Every INPUT shape of this service, as the generated schema statics.
///
/// ALL of them, not the ones one report happened to cite: an element constraint set is a
/// property of an input shape, so leaving an operation out would silently word its list
/// clauses with less than the model declares. Regenerate with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`, which names exactly these statics; an input
/// shape missing from this list is not a wrong clause, only a clause that keeps the
/// framework's per-element wording.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::ADD_TAGS_TO_STREAM_INPUT_SCHEMA,
        t::CREATE_CHANNEL_INPUT_SCHEMA,
        t::CREATE_STREAM_INPUT_SCHEMA,
        t::DECREASE_STREAM_RETENTION_PERIOD_INPUT_SCHEMA,
        t::DELETE_CHANNEL_INPUT_SCHEMA,
        t::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        t::DELETE_STREAM_INPUT_SCHEMA,
        t::DEREGISTER_STREAM_CONSUMER_INPUT_SCHEMA,
        t::DESCRIBE_ACCOUNT_SETTINGS_INPUT_SCHEMA,
        t::DESCRIBE_CHANNEL_INPUT_SCHEMA,
        t::DESCRIBE_LIMITS_INPUT_SCHEMA,
        t::DESCRIBE_STREAM_CONSUMER_INPUT_SCHEMA,
        t::DESCRIBE_STREAM_INPUT_SCHEMA,
        t::DESCRIBE_STREAM_SUMMARY_INPUT_SCHEMA,
        t::DISABLE_ENHANCED_MONITORING_INPUT_SCHEMA,
        t::ENABLE_ENHANCED_MONITORING_INPUT_SCHEMA,
        t::GET_RECORDS_INPUT_SCHEMA,
        t::GET_RESOURCE_POLICY_INPUT_SCHEMA,
        t::GET_SHARD_ITERATOR_INPUT_SCHEMA,
        t::INCREASE_STREAM_RETENTION_PERIOD_INPUT_SCHEMA,
        t::LIST_CHANNELS_INPUT_SCHEMA,
        t::LIST_SHARDS_INPUT_SCHEMA,
        t::LIST_STREAMS_INPUT_SCHEMA,
        t::LIST_STREAM_CONSUMERS_INPUT_SCHEMA,
        t::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        t::LIST_TAGS_FOR_STREAM_INPUT_SCHEMA,
        t::MERGE_SHARDS_INPUT_SCHEMA,
        t::PUT_RECORDS_INPUT_SCHEMA,
        t::PUT_RECORD_INPUT_SCHEMA,
        t::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        t::REGISTER_STREAM_CONSUMER_INPUT_SCHEMA,
        t::REMOVE_TAGS_FROM_STREAM_INPUT_SCHEMA,
        t::SPLIT_SHARD_INPUT_SCHEMA,
        t::START_STREAM_ENCRYPTION_INPUT_SCHEMA,
        t::STOP_STREAM_ENCRYPTION_INPUT_SCHEMA,
        t::TAG_RESOURCE_INPUT_SCHEMA,
        t::UNTAG_RESOURCE_INPUT_SCHEMA,
        t::UPDATE_ACCOUNT_SETTINGS_INPUT_SCHEMA,
        t::UPDATE_CHANNEL_INPUT_SCHEMA,
        t::UPDATE_MAX_RECORD_SIZE_INPUT_SCHEMA,
        t::UPDATE_SHARD_COUNT_INPUT_SCHEMA,
        t::UPDATE_STREAM_MODE_INPUT_SCHEMA,
        t::UPDATE_STREAM_RECORD_DISTRIBUTION_STRATEGY_INPUT_SCHEMA,
        t::UPDATE_STREAM_WARM_THROUGHPUT_INPUT_SCHEMA,
    ]
}

/// The members whose clause states NO value: the member path is named, the requirement is
/// named, and neither the submitted bytes nor the `null` an absent member is stated with
/// appears.
///
/// A channel's name is the one such member of this service: a refusal of a `ChannelName`
/// outside its expression names the member and the expression alone. Every other member —
/// the resource ARN a tagging call carries, a partition key, a stream mode — is quoted back
/// with the value that was submitted.
///
/// The usual source for this set is the model, which marks a member whose value is not to be
/// disclosed `@sensitive` — `service_common::sensitive_members::WithheldMembers::from_model`
/// answers that for a service, and is what to reach for the moment this model marks one. It
/// cannot answer this row: this model marks no member `@sensitive`, so the set is this
/// service's recorded behaviour and lives here. The path rule below is the shared one's, so
/// the two cannot disagree about which clause a member's name decides.
const WITHHELD_VALUE_MEMBERS: &[&str] = &["channelName"];

/// Whether the clause for the member at `path` states no value ([`WITHHELD_VALUE_MEMBERS`]).
///
/// ANY segment decides, not only the last: what sits under such a member is part of that
/// member's value. The validating walk spells a path in lowerCamel, with a `<1-based
/// index>.member` step from a collection into one of its elements, so splitting on `.`
/// reaches every segment of it.
fn withholds_value(path: &str) -> bool {
    path.split('.')
        .any(|segment| WITHHELD_VALUE_MEMBERS.contains(&segment))
}

/// The expressions this service's front door publishes for a value that has to match one,
/// keyed by the expression the model declares (anchors already dropped, see
/// [`stated_expression`]).
///
/// A row exists only where the two differ, and this model declares none that do: its
/// resource-ARN expression lists every class of Kinesis resource a tagging call may name
/// (stream, channel and queue), so a clause quotes the model's own expression. The table stays
/// because the rule it serves is the front door's — a client told an expression must be able to
/// read a valid value as valid — and the next model update is where a divergence would
/// reappear. Only the expression REPORTED comes from here; which values are ACCEPTED is the
/// model's `@pattern`, which is what the framework's walk checks.
const STATED_EXPRESSIONS: &[(&str, &str)] = &[];

/// The expression a clause quotes for the modelled `pattern`: without the `^`/`$` pair a
/// model writes around a whole-value match, and as the front door publishes it where the two
/// differ ([`STATED_EXPRESSIONS`]).
fn stated_expression(pattern: &str) -> String {
    let unanchored = ::service_common::validation::unanchored(pattern);
    STATED_EXPRESSIONS
        .iter()
        .find(|(modelled, _)| *modelled == unanchored)
        .map_or(unanchored, |(_, stated)| (*stated).to_string())
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of inputs rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/constraint_refusal_wording.rs`.

    use super::*;
    use ::aws_smithy_schema_server::constraints::ViolationKind;
    use ::smithy_server_runtime::Service;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json"
    );

    /// One violation of `kind` at `path`, carrying `value` as its echo.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// The message [`refusal`] states for one violation, with no request to read a list off.
    fn stated(violation: Violation) -> String {
        refusal(&[violation], HashMap::new)
            .expect("a constraint refusal is stated")
            .message
    }

    /// A `@pattern` is quoted without the whole-value anchors a model writes the total match
    /// with — over this service's own expressions, including one the issue did not cite.
    #[test]
    fn a_pattern_is_quoted_without_its_whole_value_anchors() {
        // (case, modelled pattern, quoted expression). Source: `smithy_query(query="shape",
        // shape="com.amazonaws.kinesis#<Shape>")` for each.
        let rows: [(&str, &str, &str); 4] = [
            ("ChannelName", r"^[a-zA-Z0-9_.-]+$", r"[a-zA-Z0-9_.-]+"),
            (
                "StreamId",
                r"^[a-z0-9]{20}-[a-z0-9]{3}$",
                r"[a-z0-9]{20}-[a-z0-9]{3}",
            ),
            (
                "StreamARN",
                r"^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$",
                r"arn:aws.*:kinesis:.*:\d{12}:stream/\S+",
            ),
            // An expression with no anchors at all is quoted as written.
            ("an unanchored expression", r"[a-z]+", r"[a-z]+"),
        ];

        for (case, modelled, quoted) in rows {
            assert_eq!(
                stated_expression(modelled),
                quoted,
                "case {case:?}: expected the expression quoted as {quoted:?}"
            );
        }
    }

    /// The resource-ARN expression is reported with every resource class the front door
    /// admits, and every row of the override table is keyed by an expression this model really
    /// declares.
    #[test]
    fn the_stated_resource_arn_expression_names_every_resource_class() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        for (modelled, stated) in STATED_EXPRESSIONS {
            assert!(
                model.contains(&modelled.replace('\\', "\\\\")),
                "case {modelled:?}: a stated expression is keyed by one the model declares"
            );
            assert_eq!(
                stated_expression(&format!("^{modelled}$")),
                *stated,
                "case {modelled:?}: expected it reported as {stated:?}"
            );
        }
        // The expression itself, whatever the table holds: the classes of Kinesis resource a
        // tagging call may name are a stream, a channel and a queue, which is what the model
        // declares (`smithy_query(query="shape",shape="ResourceARN")`) and so what a clause
        // quotes.
        let modelled = r"^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel|queue)/\S+$";
        assert!(
            model.contains(&modelled.replace('\\', "\\\\")),
            "case the resource-ARN expression: expected the model to declare {modelled:?}"
        );
        assert_eq!(
            stated_expression(modelled),
            r"arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel|queue)/\S+",
            "the resource-ARN expression names the stream, channel and queue classes"
        );
    }

    /// An enumerated type's value set is listed in the order the front door walks its own
    /// hash table of those values — neither the order the model declares them in nor
    /// lexicographic — whatever the type and whatever member carries it.
    #[test]
    fn an_enum_value_set_is_listed_in_the_front_doors_table_order() {
        // (case, the set as the model declares it, the set as a clause lists it). The declared
        // halves are the model's own: `smithy_query(query="shape",
        // shape="com.amazonaws.kinesis#<Enum>")`. The stated halves follow the one order
        // `service_common::validation_message::in_hashed_enum_order` derives from the values
        // themselves, so no row is a case of its own.
        let rows: [(&str, &[&str], &str); 7] = [
            ("StreamMode", &["PROVISIONED", "ON_DEMAND"], "[ON_DEMAND, PROVISIONED]"),
            (
                "EncryptionType",
                &["NONE", "KMS"],
                "[KMS, NONE]",
            ),
            (
                // Lexicographic and the table's order agree on neither of these two.
                "MinimumThroughputBillingCommitmentInputStatus",
                &["ENABLED", "DISABLED"],
                "[ENABLED, DISABLED]",
            ),
            (
                "MinimumThroughputBillingCommitmentOutputStatus",
                &["ENABLED", "DISABLED", "ENABLED_UNTIL_EARLIEST_ALLOWED_END"],
                "[ENABLED, ENABLED_UNTIL_EARLIEST_ALLOWED_END, DISABLED]",
            ),
            (
                "ShardIteratorType",
                &[
                    "AT_SEQUENCE_NUMBER",
                    "AFTER_SEQUENCE_NUMBER",
                    "TRIM_HORIZON",
                    "LATEST",
                    "AT_TIMESTAMP",
                ],
                "[AFTER_SEQUENCE_NUMBER, LATEST, AT_TIMESTAMP, AT_SEQUENCE_NUMBER, TRIM_HORIZON]",
            ),
            (
                // A set holding two PAIRS of values that share a bucket, so the within-bucket
                // half of the order is exercised too: the later-declared of the two is stated
                // first.
                "ShardFilterType",
                &[
                    "AFTER_SHARD_ID",
                    "AT_TRIM_HORIZON",
                    "FROM_TRIM_HORIZON",
                    "AT_LATEST",
                    "AT_TIMESTAMP",
                    "FROM_TIMESTAMP",
                ],
                "[FROM_TIMESTAMP, AT_TIMESTAMP, AT_TRIM_HORIZON, AT_LATEST, FROM_TRIM_HORIZON, \
                 AFTER_SHARD_ID]",
            ),
            (
                "MetricsName",
                &[
                    "IncomingBytes",
                    "IncomingRecords",
                    "OutgoingBytes",
                    "OutgoingRecords",
                    "WriteProvisionedThroughputExceeded",
                    "ReadProvisionedThroughputExceeded",
                    "IteratorAgeMilliseconds",
                    "ALL",
                ],
                "[ALL, IteratorAgeMilliseconds, IncomingBytes, \
                 WriteProvisionedThroughputExceeded, OutgoingRecords, \
                 ReadProvisionedThroughputExceeded, IncomingRecords, OutgoingBytes]",
            ),
        ];

        for (case, declared, listed) in rows {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let message = stated(violation(
                "member",
                ViolationKind::EnumValue(declared),
                Some("NOPE"),
            ));
            let expected = format!(
                "1 validation error detected: Value 'NOPE' at 'member' failed to satisfy \
                 constraint: Member must satisfy enum value set: {listed}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the value set listed as {listed:?}"
            );
        }
    }

    /// The value half of a clause: quoted for every member but the channel's name, which
    /// states the member and the requirement alone — whatever the constraint class, and
    /// whatever the request carried.
    #[test]
    fn the_value_half_is_quoted_except_for_the_withheld_member() {
        // (case, member path, the value the walk recorded, the clause's subject half)
        let rows: [(&str, &str, Option<&str>, &str); 5] = [
            ("a stream name", "streamName", Some("bad name"), "Value 'bad name' at 'streamName'"),
            (
                "a partition key",
                "records.1.member.partitionKey",
                Some("kkk"),
                "Value 'kkk' at 'records.1.member.partitionKey'",
            ),
            (
                "an absent member",
                "resourceARN",
                None,
                "Value null at 'resourceARN'",
            ),
            (
                "the channel name, present",
                "channelName",
                Some("bad channel name!!"),
                "Value at 'channelName'",
            ),
            (
                "the channel name, absent",
                "channelName",
                None,
                "Value at 'channelName'",
            ),
        ];

        for (case, path, value, subject) in rows {
            let message = stated(violation(path, ViolationKind::MaxLength(8), value));
            let expected = format!(
                "1 validation error detected: {subject} failed to satisfy constraint: Member \
                 must have length less than or equal to 8"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the clause to open with {subject:?}"
            );
        }
    }

    /// A refusal with nothing to word, and one that is not a constraint failure at all, keep
    /// the framework's own wording.
    #[test]
    fn a_refusal_this_service_does_not_word_keeps_the_frameworks_wording() {
        assert!(
            refusal(&[], HashMap::new).is_none(),
            "case no violations: expected the framework's own wording"
        );
        assert!(
            super::super::EmulatedService::new(std::sync::Arc::new(
                ::engine::InMemoryEngine::new()
            ))
            .override_refusal(&Refusal::Unauthenticated)
            .is_none(),
            "case an unauthenticated caller: expected the framework's own 403 wording"
        );
    }
}
