// SPDX-License-Identifier: CC0-1.0

//! WHICH values an `EventSourceArn` may name a DynamoDB stream with: the value has to name ONE
//! stream of ONE table.
//!
//! RULE: an Amazon DynamoDB event source is "the ARN of the stream" (`EventSourceArn`), and
//! such a stream is a stream OF A TABLE — its resource field is that table's own continued with
//! `/stream/<label>`, where the label is what tells one stream of a table from another ("the
//! combination of … table name, StreamLabel is guaranteed to be unique",
//! `StreamDescription$StreamLabel`). So a resource field whose table half or label half is
//! empty or carries a further `/` addresses something other than one stream of one table, and
//! `CreateEventSourceMapping` refuses it with the modelled `InvalidParameterValueException`
//! (HTTP 400) stating the ARN format that source's pollable resource is addressed by, exactly
//! as it refuses a value that stops at the table — persisting nothing. The label's own TEXT is
//! not what the rule is about: a value naming one table and one label is admitted whatever form
//! that label takes.
//!
//! The rows differ along what the rule generalises over: WHICH half of the resource field is
//! not one thing and HOW (a label carrying a further segment — the form the reported run sent —
//! no label at all, no table at all, a table half carrying a further segment), beside the
//! served control. That control is a stream this emulator MINTED: a table created through
//! DynamoDB's own `CreateTable` with a `StreamSpecification`, mounted beside Lambda and IAM on
//! one engine, so the admitted value is the `LatestStreamArn` that table reports rather than a
//! constant written here, and the mapping is read back through `GetEventSourceMapping`. Every
//! refused row is checked to have left no mapping on the value it named. The complement — the
//! same refusal for every other vendor's wrong resource shape — is the table in
//! `tests/all/event_source_mapping_source_shape_and_window.rs`.

use serde_json::json;

use crate::event_source_mapping_role_access::{
    allow, assert_refused, create_body, create_function_as, given_role, logs_only,
};
use crate::event_source_mapping_shard_refusal_wording::{
    given_streamed_table, router_with_iam_and_dynamodb,
};
use crate::event_source_mapping_source_shape_and_window::expected_format_refusal;
use crate::event_source_mappings::{create_mapping, get_mapping, list_mappings};

/// The segment that makes a DynamoDB `table/…` ARN name the table's STREAM rather than the
/// table itself, from `EventSourceArn`'s "Amazon DynamoDB Streams – The ARN of the stream".
const STREAM_SEGMENT: &str = "/stream/";

/// The segment the TABLE half of such an ARN opens with.
const TABLE_SEGMENT: &str = ":table/";

/// The table whose stream this emulator mints for the served control below.
const STREAMED_TABLE: &str = "arn-shape-orders";

/// The function every row's mapping would invoke, and the role it runs as.
const FUNCTION: &str = "arn-shape-probe";
/// See [`FUNCTION`].
const ROLE: &str = "arn-shape-reader";

/// The resource half of the format a DynamoDB stream event source is addressed by, as the
/// refusal states it. Source: `EventSourceArn`'s own "Amazon DynamoDB Streams – The ARN of the
/// stream", written with the placeholder names such a format uses.
const DYNAMODB_FORMAT: &str = "table/TableName/stream/StreamId";

/// A stream ARN naming `table` and `label`, written in the account and Region the real stream
/// ARN `like` carries — so each row varies only in the halves the rule is about.
fn stream_arn_like(like: &str, table: &str, label: &str) -> String {
    let prefix = like
        .split_once(TABLE_SEGMENT)
        .unwrap_or_else(|| panic!("{like} must carry {TABLE_SEGMENT}"))
        .0;
    format!("{prefix}{TABLE_SEGMENT}{table}{STREAM_SEGMENT}{label}")
}

#[tokio::test]
async fn a_mapping_is_created_only_on_a_value_naming_one_stream_of_one_table() {
    let case = "the stream a table of this emulator reports";
    let router = router_with_iam_and_dynamodb();
    let streamed = given_streamed_table(&router, case, STREAMED_TABLE).await;
    let label = streamed
        .rsplit_once(STREAM_SEGMENT)
        .unwrap_or_else(|| panic!("{streamed} must carry {STREAM_SEGMENT}"))
        .1
        .to_string();

    // One role granted every DynamoDB read on every resource, so no row is decided by the
    // grant: what each row is about is the value naming the source.
    let role = given_role(
        &router,
        case,
        ROLE,
        Some(json!([logs_only(), allow("dynamodb", &["*"], "*")])),
    )
    .await;
    create_function_as(&router, case, FUNCTION, &role).await;

    // (case, the event source the create names, whether it names one stream of one table)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the stream the table reports", streamed.clone(), true),
        (
            "a label carrying a further segment",
            stream_arn_like(&streamed, STREAMED_TABLE, "label/nonexistent-stream-label"),
            false,
        ),
        (
            "no label at all",
            stream_arn_like(&streamed, STREAMED_TABLE, ""),
            false,
        ),
        (
            "no table at all",
            stream_arn_like(&streamed, "", &label),
            false,
        ),
        (
            "a table half carrying a further segment",
            stream_arn_like(&streamed, &format!("{STREAMED_TABLE}/replica"), &label),
            false,
        ),
    ];

    for (case, event_source, names_one_stream) in rows {
        let answer = create_mapping(&router, &create_body(FUNCTION, &event_source)).await;

        if !names_one_stream {
            assert_refused(
                case,
                &answer,
                &expected_format_refusal("dynamodb", DYNAMODB_FORMAT),
            );
            // The refusal is INSTEAD of the mapping, not beside it.
            let listed = list_mappings(&router, &format!("FunctionName={FUNCTION}")).await;
            let wired: Vec<String> = listed
                .mappings()
                .iter()
                .filter_map(|mapping| mapping["EventSourceArn"].as_str().map(str::to_string))
                .collect();
            assert!(
                !wired.contains(&event_source),
                "case {case:?}: a refused create must leave no mapping on {event_source}, got \
                 {wired:?}"
            );
            continue;
        }

        assert_eq!(
            answer.status,
            202,
            "case {case:?}: expected HTTP 202 for {event_source}, got {} {}",
            answer.status,
            answer.text()
        );
        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("EventSourceArn")),
            (200, Some(event_source.clone())),
            "case {case:?}: the mapping must read back on {event_source}, got {} {}",
            reading.status,
            reading.text()
        );
    }
}
