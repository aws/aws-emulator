// SPDX-License-Identifier: CC0-1.0

//! The RESULT SCHEMA of the operation an integration calls: the types a MOCKED result
//! has to conform to.
//!
//! `TestState` takes "a mocked result or error for the state under test", and its
//! `mock.fieldValidationMode` "determines the level of strictness when validating
//! mocked results against their respective API models" — `STRICT` (the default) and
//! `PRESENT` both requiring that "all present fields must conform to the API's
//! schema", `NONE` performing no validation. The schema a mocked result is judged
//! against is therefore the OUTPUT of the operation the mocked integration calls, and
//! a member of it that arrives as another JSON type than that output declares is a
//! defect in the request rather than a result to test the state against.
//!
//! What a member's type IS is a fact of the INVOKED service's model, which this crate
//! cannot read — the same reason [`super::APIS`] is a list rather than a rule — so it
//! is stated per operation here, each entry read from that model with the
//! `smithy_query` call its comment names. An operation with no entry is one whose
//! result schema is not stated, and a mocked result for it conforms to nothing, which
//! is what `NONE` means for every operation: no claim is made about a schema this
//! emulator does not carry.
//!
//! Which operation's output that is comes from what the task's result IS. A
//! REQUEST-RESPONSE call answers with its own operation's output. A `.sync` task's
//! result is the job's final REPORT, so the model it conforms to is the output of the
//! operation that REPORTS the job ([`super::RunAJob::describe`]) rather than of the one
//! that started it. A `.waitForTaskToken` task reports what the task reply carried, a
//! `Resource` that is a function's ARN reports that function's own payload and an HTTP
//! Task reports the endpoint's response — none of those is an operation's output
//! document, so none is judged against one ([`schema_of`]).

use super::*;

/// The JSON a member of an invoked operation's output arrives as, as that operation's
/// own model declares the member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaType {
    /// A `string`, an `enum`, or a `blob` — which a JSON document carries as a string.
    String,
    /// An `integer`, `long`, `short` or `byte`.
    Integer,
    /// A `float` or `double`.
    Double,
    /// A `boolean`.
    Boolean,
    /// A `map`, which a JSON document carries as an object.
    Map,
    /// A `list`, which a JSON document carries as an array.
    List,
    /// A `structure`, which a JSON document carries as an object.
    Structure,
}

impl SchemaType {
    /// The name a refusal states this type by, with the article it reads with: the
    /// type the invoked model declares, which is the schema the mocked result is
    /// validated against.
    ///
    /// RECORDED: a member declared as a MAP is stated as "an object", which is the JSON
    /// a map arrives as — and a `structure` arrives as that same JSON, so the two are
    /// stated alike.
    pub fn stated(self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::Integer => "an integer",
            Self::Double => "a double",
            Self::Boolean => "a boolean",
            Self::Map | Self::Structure => "an object",
            Self::List => "a list",
        }
    }

    /// Whether `value` is of this type.
    ///
    /// An integral member admits a whole number alone, because a JSON number with a
    /// fraction is no value of one; a floating-point member admits any JSON number,
    /// since a whole number is a value of it. A JSON `null` is no value of any type —
    /// it is the member being absent, which is what a result that does not carry it
    /// looks like — so it is admitted everywhere and judged by nothing here.
    fn holds(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Double => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Map | Self::Structure => value.is_object(),
            Self::List => value.is_array(),
        }
    }
}

/// One invoked operation's result schema: the service its model belongs to (the ARN
/// namespace [`super::Api::namespace`] names it by), the operation, and the type each
/// of its output members is declared as.
///
/// The members are named as the RESULT carries them, which is PascalCase: a `Task`
/// state's integration data is written in that spelling whatever the invoked API spells
/// its own members in ([`super::MemberCase`]), so a mocked result names them that way
/// too.
#[derive(Debug)]
pub struct ResultSchema {
    /// The invoked service, by its ARN namespace.
    pub service: &'static str,
    /// The operation, as its own model spells it.
    pub operation: &'static str,
    /// Its output members, each with the type that model declares.
    ///
    /// A member whose type a JSON document's own types do not decide — a `timestamp`,
    /// whose spelling is the invoked protocol's rather than the member's — is left out
    /// rather than stated as a type it might not arrive as.
    pub members: &'static [(&'static str, SchemaType)],
}

/// The result schemas of the integration operations this emulator calls.
///
/// One entry per operation, each read from the invoked service's own model.
pub const RESULT_SCHEMAS: &[ResultSchema] = &[
    // `smithy_query(query="operation",operation="Invoke",model_path="services/
    // service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
    // com-amazonaws-lambda-awsgirapiservice.json")`: `InvocationResponse` is
    // `StatusCode` (Integer), `FunctionError` (String), `LogResult` (String),
    // `Payload` (Blob), `ExecutedVersion` (Version, a string) and
    // `DurableExecutionArn` (a string).
    ResultSchema {
        service: "lambda",
        operation: "Invoke",
        members: &[
            ("StatusCode", SchemaType::Integer),
            ("FunctionError", SchemaType::String),
            ("LogResult", SchemaType::String),
            ("Payload", SchemaType::String),
            ("ExecutedVersion", SchemaType::String),
            ("DurableExecutionArn", SchemaType::String),
        ],
    },
    // `smithy_query(query="operation",operation="StartExecution")` on this service's
    // own model: `StartExecutionOutput` is `executionArn` (Arn, a string) and
    // `startDate` (Timestamp, left out above).
    ResultSchema {
        service: "states",
        operation: "StartExecution",
        members: &[("ExecutionArn", SchemaType::String)],
    },
    // The REPORT of a nested execution, which is what a `states:startExecution.sync`
    // task's result is. `smithy_query(query="operation",operation="DescribeExecution")`
    // on this service's own model: `executionArn`, `stateMachineArn`, `name`, `status`
    // (an enum), `input`, `output`, `traceHeader`, `mapRunArn`, `error`, `cause`,
    // `stateMachineVersionArn`, `stateMachineAliasArn` and `redriveStatusReason` are
    // strings, `inputDetails` and `outputDetails` structures
    // (`CloudWatchEventsExecutionDataDetails`), `redriveStatus` an enum, `redriveCount`
    // an integer, and `startDate`, `stopDate` and `redriveDate` timestamps (left out
    // above).
    ResultSchema {
        service: "states",
        operation: "DescribeExecution",
        members: &[
            ("ExecutionArn", SchemaType::String),
            ("StateMachineArn", SchemaType::String),
            ("Name", SchemaType::String),
            ("Status", SchemaType::String),
            ("Input", SchemaType::String),
            ("InputDetails", SchemaType::Structure),
            ("Output", SchemaType::String),
            ("OutputDetails", SchemaType::Structure),
            ("TraceHeader", SchemaType::String),
            ("MapRunArn", SchemaType::String),
            ("Error", SchemaType::String),
            ("Cause", SchemaType::String),
            ("StateMachineVersionArn", SchemaType::String),
            ("StateMachineAliasArn", SchemaType::String),
            ("RedriveCount", SchemaType::Integer),
            ("RedriveStatus", SchemaType::String),
            ("RedriveStatusReason", SchemaType::String),
        ],
    },
    // `smithy_query(query="operation",operation="SendMessage",model_path="services/
    // service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`:
    // `SendMessageResult` is `MD5OfMessageBody`, `MD5OfMessageAttributes`,
    // `MD5OfMessageSystemAttributes`, `MessageId` and `SequenceNumber`, all String.
    ResultSchema {
        service: "sqs",
        operation: "SendMessage",
        members: &[
            ("MD5OfMessageBody", SchemaType::String),
            ("MD5OfMessageAttributes", SchemaType::String),
            ("MD5OfMessageSystemAttributes", SchemaType::String),
            ("MessageId", SchemaType::String),
            ("SequenceNumber", SchemaType::String),
        ],
    },
    // `smithy_query(query="operation",operation="Publish",model_path="services/
    // service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
    // com-amazonaws-sns-amazonsimplenotificationservice.json")`: `PublishResponse` is
    // `MessageId` (messageId, a string) and `SequenceNumber` (String).
    ResultSchema {
        service: "sns",
        operation: "Publish",
        members: &[
            ("MessageId", SchemaType::String),
            ("SequenceNumber", SchemaType::String),
        ],
    },
    // `smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,DeleteItem",
    // model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/
    // smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")`: the write
    // operations answer `Attributes` (AttributeMap, a map), `ConsumedCapacity` and
    // `ItemCollectionMetrics` (structures), and `GetItem` answers `Item` (AttributeMap)
    // beside `ConsumedCapacity`.
    ResultSchema {
        service: "dynamodb",
        operation: "PutItem",
        members: DYNAMODB_WRITE_RESULT,
    },
    ResultSchema {
        service: "dynamodb",
        operation: "UpdateItem",
        members: DYNAMODB_WRITE_RESULT,
    },
    ResultSchema {
        service: "dynamodb",
        operation: "DeleteItem",
        members: DYNAMODB_WRITE_RESULT,
    },
    ResultSchema {
        service: "dynamodb",
        operation: "GetItem",
        members: &[
            ("Item", SchemaType::Map),
            ("ConsumedCapacity", SchemaType::Structure),
        ],
    },
    // `smithy_query(query="operation",operation="StartQueryExecution",
    // model_path="services/service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json")`: `StartQueryExecutionOutput` is
    // `QueryExecutionId` (QueryExecutionId, a string).
    ResultSchema {
        service: "athena",
        operation: "StartQueryExecution",
        members: &[("QueryExecutionId", SchemaType::String)],
    },
    // The REPORT of a query, which is what an `athena:startQueryExecution.sync` task's
    // result is. `smithy_query(query="operation",operation="GetQueryExecution",
    // model_path="services/service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json")`: `GetQueryExecutionOutput` is
    // `QueryExecution` (a structure).
    ResultSchema {
        service: "athena",
        operation: "GetQueryExecution",
        members: &[("QueryExecution", SchemaType::Structure)],
    },
    // `smithy_query(query="operation",operation="StartJobRun",model_path="services/
    // service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`:
    // `StartJobRunResponse` is `JobRunId` (IdString, a string).
    ResultSchema {
        service: "glue",
        operation: "StartJobRun",
        members: &[("JobRunId", SchemaType::String)],
    },
    // The REPORT of a job run, which is what a `glue:startJobRun.sync` task's result
    // is. `smithy_query(query="operation",operation="GetJobRun",model_path="services/
    // service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`:
    // `GetJobRunResponse` is `JobRun` (a structure).
    ResultSchema {
        service: "glue",
        operation: "GetJobRun",
        members: &[("JobRun", SchemaType::Structure)],
    },
    // `smithy_query(query="operation",operation="StartBackupJob",model_path="services/
    // service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/
    // com-amazonaws-backup-cryocontrollerusermanager.json")`: `StartBackupJobOutput` is
    // `BackupJobId` (string), `RecoveryPointArn` (ARN, a string), `CreationDate`
    // (timestamp, left out above) and `IsParent` (Boolean2, a boolean).
    ResultSchema {
        service: "backup",
        operation: "StartBackupJob",
        members: &[
            ("BackupJobId", SchemaType::String),
            ("RecoveryPointArn", SchemaType::String),
            ("IsParent", SchemaType::Boolean),
        ],
    },
];

/// The output members DynamoDB's three item-WRITE operations share, which their one
/// `smithy_query` above reads alike for all three.
const DYNAMODB_WRITE_RESULT: &[(&str, SchemaType)] = &[
    ("Attributes", SchemaType::Map),
    ("ConsumedCapacity", SchemaType::Structure),
    ("ItemCollectionMetrics", SchemaType::Structure),
];

/// The schema the RESULT of `call` is judged against, or `None` when the task's result
/// is not the invoked operation's own output, or no schema is stated for that
/// operation.
pub fn schema_of(call: &Call) -> Option<&'static ResultSchema> {
    // A task reply's payload, a function's own response and an endpoint's answer are
    // each the result of something no operation of the invoked model describes, so
    // none of them is an operation's output document.
    if call.callback || call.invokes.is_some() || call.endpoint.is_some() {
        return None;
    }
    // A run-a-job task's result is the JOB's final report, which is the output of the
    // operation that reports the job rather than of the one that started it — so that
    // operation's schema is what such a result conforms to.
    let operation = match call.job {
        Some(job) => job.describe,
        None => call.operation.as_str(),
    };
    RESULT_SCHEMAS
        .iter()
        .find(|schema| schema.service == call.api.namespace && schema.operation == operation)
}

/// The first member of `result` that is not of the type the schema of `call`'s
/// operation declares for it, as `(member, declared type)`.
///
/// Reported in the SCHEMA's own member order, so one mocked result earns one refusal
/// whatever order its members were written in. A member the schema does not name is
/// judged by nothing: the result of an optimized integration carries the SDK's own
/// response envelope beside the operation's members, which is no part of that
/// operation's output.
pub fn schema_violation(call: &Call, result: &Value) -> Option<(&'static str, SchemaType)> {
    let schema = schema_of(call)?;
    let members = result.as_object()?;
    schema.members.iter().find_map(|(member, declared)| {
        members
            .get(*member)
            .filter(|value| !value.is_null() && !declared.holds(value))
            .map(|_| (*member, *declared))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: the case, the `Resource` the state names, the mocked
    /// result, and the member and type a refusal names (none where it conforms).
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        Option<(&'static str, SchemaType)>,
    );

    /// The schema is read off the operation the `Resource` names, and a member of a
    /// mocked result conforms only when its JSON type is the one that operation's
    /// output declares.
    ///
    /// The rows span the types the table states — a blob member (reported as a
    /// string), an integral one, a map and a boolean — and both families' spellings of
    /// one `Resource`, each with a conforming and a non-conforming value.
    #[test]
    fn a_mocked_result_is_judged_by_the_invoked_operations_output() {
        // (case, the `Resource`, the mocked result, the member and type a refusal
        // names, if any)
        let rows: [Row; 14] = [
            (
                "a blob member sent as an object",
                "arn:aws:states:::lambda:invoke",
                r#"{"StatusCode":200,"Payload":{"ok":true}}"#,
                Some(("Payload", SchemaType::String)),
            ),
            (
                "the same member sent as the string it is declared as",
                "arn:aws:states:::lambda:invoke",
                r#"{"StatusCode":200,"Payload":"{\"ok\":true}"}"#,
                None,
            ),
            (
                "an integral member sent as a string",
                "arn:aws:states:::lambda:invoke",
                r#"{"StatusCode":"200"}"#,
                Some(("StatusCode", SchemaType::Integer)),
            ),
            (
                "an integral member sent with a fraction",
                "arn:aws:states:::lambda:invoke",
                r#"{"StatusCode":200.5}"#,
                Some(("StatusCode", SchemaType::Integer)),
            ),
            (
                "the AWS SDK family's spelling of the same operation",
                "arn:aws:states:::aws-sdk:lambda:invoke",
                r#"{"Payload":42}"#,
                Some(("Payload", SchemaType::String)),
            ),
            (
                "a map member sent as an array",
                "arn:aws:states:::dynamodb:getItem",
                r#"{"Item":[]}"#,
                Some(("Item", SchemaType::Map)),
            ),
            (
                "a boolean member sent as a string",
                "arn:aws:states:::aws-sdk:backup:startBackupJob",
                r#"{"BackupJobId":"j-1","IsParent":"false"}"#,
                Some(("IsParent", SchemaType::Boolean)),
            ),
            (
                "members the schema does not name",
                "arn:aws:states:::sqs:sendMessage",
                r#"{"MessageId":"m-1","SdkHttpMetadata":{"HttpStatusCode":200}}"#,
                None,
            ),
            (
                // The result of a run-a-job task is the job's own REPORT, so the
                // START call's output schema is not what it conforms to: the id that
                // call answered is no member of the report.
                "a run-a-job task's mocked result, by the start call's member",
                "arn:aws:states:::glue:startJobRun.sync",
                r#"{"JobRunId":7}"#,
                None,
            ),
            (
                // What it IS judged by: the output of the operation that reports the
                // job, whose one member carries the run's own document.
                "a run-a-job report member sent as a string",
                "arn:aws:states:::glue:startJobRun.sync",
                r#"{"JobRun":"SUCCEEDED"}"#,
                Some(("JobRun", SchemaType::Structure)),
            ),
            (
                "the same member sent as the object it is declared as",
                "arn:aws:states:::glue:startJobRun.sync",
                r#"{"JobRun":{"JobRunState":"SUCCEEDED"}}"#,
                None,
            ),
            (
                "a query report's member sent as a list",
                "arn:aws:states:::athena:startQueryExecution.sync",
                r#"{"QueryExecution":[]}"#,
                Some(("QueryExecution", SchemaType::Structure)),
            ),
            (
                "a nested execution's report member sent as an object",
                "arn:aws:states:::states:startExecution.sync",
                r#"{"ExecutionArn":"arn:aws:states:::execution:m:e","Status":{"of":"it"}}"#,
                Some(("Status", SchemaType::String)),
            ),
            (
                // The pattern's second revision reports the same operation's output,
                // so the same schema judges its mocked result.
                "the second revision of the same integration",
                "arn:aws:states:::states:startExecution.sync:2",
                r#"{"RedriveCount":"none"}"#,
                Some(("RedriveCount", SchemaType::Integer)),
            ),
        ];

        for (case, resource, result, expected) in rows {
            let call = read(resource).expect("the row names an integration this emulator calls");
            let mocked: Value = serde_json::from_str(result).expect("the row is JSON");
            let actual = schema_violation(&call, &mocked);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every entry names a service [`super::APIS`] carries and an operation of it that
    /// a `Resource` can reach, so no row states a schema for a call this emulator
    /// never makes.
    #[test]
    fn every_stated_schema_names_an_integration_this_emulator_calls() {
        for schema in RESULT_SCHEMAS {
            let served = APIS.iter().any(|api| api.namespace == schema.service);
            assert!(
                served,
                "schema {}:{}: expected a service this emulator integrates with",
                schema.service, schema.operation
            );
            assert!(
                !schema.members.is_empty(),
                "schema {}:{}: expected at least one stated member",
                schema.service,
                schema.operation
            );
        }
    }
}
