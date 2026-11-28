// SPDX-License-Identifier: CC0-1.0

//! `GetPlan`: the ETL code that performs a mapping from one Data Catalog table onto the
//! target tables a request names.
//!
//! RULE: the plan is the Glue codegen pipeline computed from the STORED table definitions
//! and the requested mapping — a `DataSource` node reading the source table out of the
//! catalog, then, per target table, an `ApplyMapping` of the entries that belong to that
//! target, a `SelectFields` of the fields it mapped, a `ResolveChoice` against the target's
//! stored schema and a `DataSink` writing to it; a request naming no target table maps,
//! selects and writes to the location it named. Every node is annotated with its
//! type, arguments, the name it binds and the frame it reads, and named `<kind><position>`
//! in one counter across the script. Each table is named by the spelling the CATALOG holds,
//! and a mapping entry that leaves a type unstated takes the type the stored column
//! declares. `Language` picks the member that carries the code: `SCALA` answers
//! `ScalaCode`, `PYTHON` or no language at all answers `PythonScript`, and the other member
//! is absent. Nothing is persisted.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many target tables the request
//! names (none — with a `Location` as its destination, since a request stating neither is
//! refused, which [`crate::mapping_destination`] pins — one, two), which members a mapping
//! entry states (both types, neither, a renamed target path, no target path, no source path,
//! a target table of its own), whether
//! the field is a storage-descriptor column, a partition key or no column of the table at
//! all, which language is asked for, whether the request carries additional plan options,
//! which member of `Location` it carries, and whether it spells the catalog's names in
//! another case. A last table walks the two size bounds this operation enforces for itself
//! at each ceiling and one past it.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::get_mapping::{
    MAX_MAPPING_ENTRIES, MAX_MAPPING_SINKS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{emulator, ok};

/// The database every row stores its tables in. Each row runs on its own store, so one
/// name serves them all; the catalog folds it, which is the spelling the plan names.
const DATABASE: &str = "plan_db";

/// One column of a table under test: its `Name` and the `Type` it declares. A column need
/// not declare one — `Column$Type` is not required (`smithy_query shape=Column`) — which
/// `None` stands for.
type Col = (&'static str, Option<&'static str>);

/// A table one row stores: the name it is created under, its storage-descriptor columns
/// and its partition keys.
struct Stored {
    name: &'static str,
    columns: &'static [Col],
    partition_keys: &'static [Col],
}

/// A table with no partition keys, which is what most rows store.
fn flat(name: &'static str, columns: &'static [Col]) -> Stored {
    Stored {
        name,
        columns,
        partition_keys: &[],
    }
}

/// One row of the plan table: the case, the tables it stores, the request it sends and the
/// statements the generated script must carry, in order.
struct Row {
    case: &'static str,
    /// `Language`, or `None` for a request that asks for none.
    language: Option<&'static str>,
    source: Stored,
    sinks: Vec<Stored>,
    /// The `Mapping` list the request carries.
    mapping: Vec<Value>,
    location: Option<Value>,
    additional_options: Option<Value>,
    expected: Vec<&'static str>,
}

/// A mapping entry naming a source field and the target field it maps onto, with both
/// types stated.
fn typed(
    source_table: &str,
    source_path: &str,
    source_type: &str,
    target_table: &str,
    target_path: &str,
    target_type: &str,
) -> Value {
    json!({
        "SourceTable": source_table,
        "SourcePath": source_path,
        "SourceType": source_type,
        "TargetTable": target_table,
        "TargetPath": target_path,
        "TargetType": target_type,
    })
}

/// The rows: one per shape of request, stored table and mapping entry the rule covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one target: the pipeline reads, maps, selects, resolves and writes",
            language: None,
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![flat(
                "orders_copy",
                &[("id", Some("bigint")), ("name", Some("varchar(64)"))],
            )],
            mapping: vec![
                typed("orders", "id", "int", "orders_copy", "id", "bigint"),
                typed(
                    "orders",
                    "name",
                    "string",
                    "orders_copy",
                    "name",
                    "varchar(64)",
                ),
            ],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"bigint\"), (\"name\", \"string\", \"name\", \
                 \"varchar(64)\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\", \
                 \"name\"], transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "an entry stating no type takes the type each stored table declares",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![flat("orders_copy", &[("id", Some("bigint"))])],
            mapping: vec![json!({ "SourcePath": "id", "TargetPath": "id" })],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"bigint\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "a partition key is a column of the data, so its declared type fills in",
            language: None,
            source: Stored {
                name: "events",
                columns: &[("id", Some("int"))],
                partition_keys: &[("dt", Some("string"))],
            },
            sinks: vec![flat(
                "events_copy",
                &[("id", Some("int")), ("dt", Some("date"))],
            )],
            mapping: vec![
                json!({ "SourcePath": "id", "TargetPath": "id" }),
                json!({ "SourcePath": "dt", "TargetPath": "dt" }),
            ],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"events\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\"), (\"dt\", \"string\", \"dt\", \"date\")], \
                 transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\", \
                 \"dt\"], transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"events_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"events_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "a field no stored column declares a type for maps as a string",
            language: None,
            source: flat("payloads", &[("payload", None)]),
            sinks: vec![flat("payloads_copy", &[("payload", None)])],
            mapping: vec![
                json!({ "SourcePath": "payload" }),
                json!({ "SourcePath": "payload.id" }),
            ],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"payloads\", transformation_ctx = \
                 \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = \
                 [(\"payload\", \"string\", \"payload\", \"string\"), (\"payload.id\", \
                 \"string\", \"payload.id\", \"string\")], transformation_ctx = \
                 \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = \
                 [\"payload\", \"payload.id\"], transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"payloads_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"payloads_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "a renamed target path is what the mapped frame selects",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![flat("orders_copy", &[("order_id", Some("bigint"))])],
            mapping: vec![json!({ "SourcePath": "id", "TargetPath": "order_id" })],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"order_id\", \"bigint\")], transformation_ctx = \
                 \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = \
                 [\"order_id\"], transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "an entry naming no source path maps nothing; one naming no target path \
                   keeps the source name",
            language: None,
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![flat("orders_copy", &[("id", Some("int"))])],
            mapping: vec![
                json!({ "SourcePath": "id" }),
                json!({ "TargetPath": "orphan", "TargetType": "string" }),
            ],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"orders_copy\", \
                 transformation_ctx = \"datasink4\")",
            ],
        },
        Row {
            case: "two targets get one pipeline each, in the order sent, and an entry \
                   naming a target belongs to that target alone",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![
                flat("orders_first", &[("id", Some("bigint"))]),
                flat("orders_second", &[("id", Some("smallint"))]),
            ],
            mapping: vec![
                typed("orders", "id", "int", "orders_first", "id", "bigint"),
                typed("orders", "id", "int", "orders_second", "order_id", "smallint"),
            ],
            location: None,
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"bigint\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_first\", \
                 transformation_ctx = \"resolvechoice3\")",
                "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice3, database = \"plan_db\", table_name = \"orders_first\", \
                 transformation_ctx = \"datasink4\")",
                "applymapping5 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"order_id\", \"smallint\")], transformation_ctx = \
                 \"applymapping5\")",
                "selectfields6 = SelectFields.apply(frame = applymapping5, paths = \
                 [\"order_id\"], transformation_ctx = \"selectfields6\")",
                "resolvechoice7 = ResolveChoice.apply(frame = selectfields6, choice = \
                 \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_second\", \
                 transformation_ctx = \"resolvechoice7\")",
                "datasink8 = glueContext.write_dynamic_frame.from_catalog(frame = \
                 resolvechoice7, database = \"plan_db\", table_name = \"orders_second\", \
                 transformation_ctx = \"datasink8\")",
            ],
        },
        Row {
            case: "a location and no target table: the plan maps, selects and writes there",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            // A request stating NEITHER a target table nor a location states nowhere for
            // the mapped data to go and is refused, which [`crate::mapping_destination`]
            // pins; what varies here is which destination the request states.
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders_only/" }],
            })),
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"s3\", connection_options = {\"path\": \
                 \"s3://bucket/orders_only/\"}, transformation_ctx = \"datasink3\")",
            ],
        },
        Row {
            case: "an S3 location is where a plan with no target table writes",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "S3": [
                    { "Name": "path", "Value": "s3://bucket/orders_out/" },
                    { "Name": "format", "Value": "parquet" },
                ],
            })),
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"s3\", connection_options = {\"path\": \
                 \"s3://bucket/orders_out/\", \"format\": \"parquet\"}, transformation_ctx = \
                 \"datasink3\")",
            ],
        },
        Row {
            case: "a JDBC location writes through its own connection type",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "Jdbc": [{ "Name": "connectionName", "Value": "warehouse" }],
            })),
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"jdbc\", connection_options = \
                 {\"connectionName\": \"warehouse\"}, transformation_ctx = \"datasink3\")",
            ],
        },
        Row {
            case: "a DynamoDB location writes through its own connection type",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "DynamoDB": [{ "Name": "dynamodb.output.tableName", "Value": "orders_out" }],
            })),
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"dynamodb\", connection_options = \
                 {\"dynamodb.output.tableName\": \"orders_out\"}, transformation_ctx = \
                 \"datasink3\")",
            ],
        },
        Row {
            case: "the additional plan options are carried into the source read",
            language: None,
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders_out/" }],
            })),
            // The documented pair, plus a second one, so the ordering is visible.
            additional_options: Some(json!({ "inferSchema": "true", "another": "value" })),
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\", \
                 additional_options = {\"another\": \"value\", \"inferSchema\": \"true\"})",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"s3\", connection_options = {\"path\": \
                 \"s3://bucket/orders_out/\"}, transformation_ctx = \"datasink3\")",
            ],
        },
        Row {
            case: "PYTHON asked for explicitly answers the same Python script",
            language: Some("PYTHON"),
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders_out/" }],
            })),
            additional_options: None,
            expected: vec![
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \
                 \"plan_db\", table_name = \"orders\", transformation_ctx = \"datasource0\")",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \
                 \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")",
                "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
                 transformation_ctx = \"selectfields2\")",
                "datasink3 = glueContext.write_dynamic_frame.from_options(frame = \
                 selectfields2, connection_type = \"s3\", connection_options = {\"path\": \
                 \"s3://bucket/orders_out/\"}, transformation_ctx = \"datasink3\")",
            ],
        },
        Row {
            case: "SCALA answers the same pipeline as Scala code",
            language: Some("SCALA"),
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![flat("orders_copy", &[("id", Some("bigint"))])],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: None,
            additional_options: None,
            expected: vec![
                "val datasource0 = glueContext.getCatalogSource(database = \"plan_db\", \
                 tableName = \"orders\", redshiftTmpDir = \"\", transformationContext = \
                 \"datasource0\").getDynamicFrame()",
                "val applymapping1 = datasource0.applyMapping(mappings = Seq((\"id\", \
                 \"int\", \"id\", \"bigint\")), caseSensitive = false, transformationContext \
                 = \"applymapping1\")",
                "val selectfields2 = applymapping1.selectFields(paths = Seq(\"id\"), \
                 transformationContext = \"selectfields2\")",
                "val resolvechoice3 = selectfields2.resolveChoice(choiceOption = \
                 Some(ChoiceOption(\"MATCH_CATALOG\")), database = Some(\"plan_db\"), \
                 tableName = Some(\"orders_copy\"), transformationContext = \
                 \"resolvechoice3\")",
                "val datasink4 = glueContext.getCatalogSink(database = \"plan_db\", tableName \
                 = \"orders_copy\", redshiftTmpDir = \"\", transformationContext = \
                 \"datasink4\").writeDynamicFrame(resolvechoice3)",
            ],
        },
        Row {
            case: "a SCALA plan with a location writes through the formatted sink",
            language: Some("SCALA"),
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![],
            mapping: vec![json!({ "SourcePath": "id" })],
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders_out/" }],
            })),
            additional_options: Some(json!({ "inferSchema": "true" })),
            expected: vec![
                "val datasource0 = glueContext.getCatalogSource(database = \"plan_db\", \
                 tableName = \"orders\", redshiftTmpDir = \"\", transformationContext = \
                 \"datasource0\", additionalOptions = \
                 JsonOptions(\"\"\"{\"inferSchema\": \"true\"}\"\"\")).getDynamicFrame()",
                "val applymapping1 = datasource0.applyMapping(mappings = Seq((\"id\", \
                 \"int\", \"id\", \"int\")), caseSensitive = false, transformationContext = \
                 \"applymapping1\")",
                "val selectfields2 = applymapping1.selectFields(paths = Seq(\"id\"), \
                 transformationContext = \"selectfields2\")",
                "val datasink3 = glueContext.getSinkWithFormat(connectionType = \"s3\", \
                 options = JsonOptions(\"\"\"{\"path\": \
                 \"s3://bucket/orders_out/\"}\"\"\"), transformationContext = \
                 \"datasink3\").writeDynamicFrame(selectfields2)",
            ],
        },
    ]
}

/// The `Columns` list a table's columns are created with.
fn columns_json(columns: &[Col]) -> Vec<Value> {
    columns
        .iter()
        .map(|(name, data_type)| {
            let mut column = json!({ "Name": name });
            if let Some(data_type) = data_type {
                column["Type"] = json!(data_type);
            }
            column
        })
        .collect()
}

/// Store one table of a row in `database`.
async fn given_table(router: &MultiServiceRouter, database: &str, table: &Stored) {
    let mut table_input = json!({
        "Name": table.name,
        "StorageDescriptor": {
            "Location": format!("s3://bucket/{}/", table.name),
            "Columns": columns_json(table.columns),
        },
    });
    if !table.partition_keys.is_empty() {
        table_input["PartitionKeys"] = json!(columns_json(table.partition_keys));
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
}

/// Store a row's database, its source table and its target tables.
async fn given_row(router: &MultiServiceRouter, row: &Row) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(router, DATABASE, &row.source).await;
    for sink in &row.sinks {
        given_table(router, DATABASE, sink).await;
    }
}

/// One `GetPlan` body for a row.
fn get_plan_request(row: &Row) -> Value {
    let mut request = json!({
        "Source": { "DatabaseName": DATABASE, "TableName": row.source.name },
        "Mapping": row.mapping.clone(),
    });
    if !row.sinks.is_empty() {
        request["Sinks"] = Value::Array(
            row.sinks
                .iter()
                .map(|sink| json!({ "DatabaseName": DATABASE, "TableName": sink.name }))
                .collect(),
        );
    }
    if let Some(location) = &row.location {
        request["Location"] = location.clone();
    }
    if let Some(options) = &row.additional_options {
        request["AdditionalPlanOptionsMap"] = options.clone();
    }
    if let Some(language) = row.language {
        request["Language"] = json!(language);
    }
    request
}

/// The node statements of a generated script: the lines between the job's init and its
/// commit that are not `@`-annotations, with the Scala body's indentation removed.
fn statements(script: &str, scala: bool) -> Vec<String> {
    let (init, commit, comment) = if scala {
        ("    Job.init(", "    Job.commit()", "//")
    } else {
        ("job.init(", "job.commit()", "##")
    };
    script
        .lines()
        .skip_while(|line| !line.starts_with(init))
        .skip(1)
        .take_while(|line| *line != commit)
        .map(str::trim_start)
        .filter(|line| !line.starts_with(comment))
        .map(str::to_string)
        .collect()
}

/// The generated code of a row's answer: the member its `Language` asks for, with the other
/// member absent.
fn generated(case: &str, answered: &Value, scala: bool) -> String {
    let (asked, other) = if scala {
        ("ScalaCode", "PythonScript")
    } else {
        ("PythonScript", "ScalaCode")
    };
    assert!(
        answered.get(other).is_none(),
        "case {case:?}: the answer must carry no {other}, got {answered}"
    );
    answered[asked]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry {asked}, got {answered}"))
        .to_string()
}

/// Every row's plan is the pipeline the stored definitions and the requested mapping imply.
#[tokio::test]
async fn the_plan_is_the_pipeline_the_stored_tables_and_the_mapping_imply() {
    for row in &rows() {
        // One store per row, so a row's tables cannot be seen by another's.
        let router = emulator();
        given_row(&router, row).await;

        let answered = ok(&router, "GetPlan", &get_plan_request(row)).await;
        let scala = row.language == Some("SCALA");
        let script = generated(row.case, &answered, scala);
        let expected: Vec<String> = row.expected.iter().map(|line| line.to_string()).collect();
        assert_eq!(
            statements(&script, scala),
            expected,
            "case {case:?}: expected the statements {expected:?}, got the script {script:?}",
            case = row.case,
        );
    }
}

/// The whole Python script of one plan, annotations and all: the Glue codegen template,
/// with every node carrying its type, its arguments, the name it binds and the frame it
/// reads.
#[tokio::test]
async fn the_script_carries_the_annotation_of_every_node() {
    let router = emulator();
    let row = Row {
        case: "the canonical one-target plan",
        language: None,
        source: flat("orders", &[("id", Some("int"))]),
        sinks: vec![flat("orders_copy", &[("id", Some("bigint"))])],
        mapping: vec![json!({ "SourcePath": "id" })],
        location: None,
        additional_options: None,
        expected: vec![],
    };
    given_row(&router, &row).await;

    let answered = ok(&router, "GetPlan", &get_plan_request(&row)).await;
    let script = generated(row.case, &answered, false);
    let expected = concat!(
        "import sys\n",
        "from awsglue.transforms import *\n",
        "from awsglue.utils import getResolvedOptions\n",
        "from pyspark.context import SparkContext\n",
        "from awsglue.context import GlueContext\n",
        "from awsglue.job import Job\n",
        "\n",
        "## @params: [JOB_NAME]\n",
        "args = getResolvedOptions(sys.argv, ['JOB_NAME'])\n",
        "\n",
        "sc = SparkContext()\n",
        "glueContext = GlueContext(sc)\n",
        "spark = glueContext.spark_session\n",
        "job = Job(glueContext)\n",
        "job.init(args['JOB_NAME'], args)\n",
        "## @type: DataSource\n",
        "## @args: [database = \"plan_db\", table_name = \"orders\", transformation_ctx = \
         \"datasource0\"]\n",
        "## @return: datasource0\n",
        "## @inputs: []\n",
        "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \"plan_db\", \
         table_name = \"orders\", transformation_ctx = \"datasource0\")\n",
        "## @type: ApplyMapping\n",
        "## @args: [mapping = [(\"id\", \"int\", \"id\", \"bigint\")], transformation_ctx = \
         \"applymapping1\"]\n",
        "## @return: applymapping1\n",
        "## @inputs: [frame = datasource0]\n",
        "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \"int\", \
         \"id\", \"bigint\")], transformation_ctx = \"applymapping1\")\n",
        "## @type: SelectFields\n",
        "## @args: [paths = [\"id\"], transformation_ctx = \"selectfields2\"]\n",
        "## @return: selectfields2\n",
        "## @inputs: [frame = applymapping1]\n",
        "selectfields2 = SelectFields.apply(frame = applymapping1, paths = [\"id\"], \
         transformation_ctx = \"selectfields2\")\n",
        "## @type: ResolveChoice\n",
        "## @args: [choice = \"MATCH_CATALOG\", database = \"plan_db\", table_name = \
         \"orders_copy\", transformation_ctx = \"resolvechoice3\"]\n",
        "## @return: resolvechoice3\n",
        "## @inputs: [frame = selectfields2]\n",
        "resolvechoice3 = ResolveChoice.apply(frame = selectfields2, choice = \
         \"MATCH_CATALOG\", database = \"plan_db\", table_name = \"orders_copy\", \
         transformation_ctx = \"resolvechoice3\")\n",
        "## @type: DataSink\n",
        "## @args: [database = \"plan_db\", table_name = \"orders_copy\", transformation_ctx \
         = \"datasink4\"]\n",
        "## @return: datasink4\n",
        "## @inputs: [frame = resolvechoice3]\n",
        "datasink4 = glueContext.write_dynamic_frame.from_catalog(frame = resolvechoice3, \
         database = \"plan_db\", table_name = \"orders_copy\", transformation_ctx = \
         \"datasink4\")\n",
        "job.commit()\n",
    );
    assert_eq!(
        script, expected,
        "case {case:?}: expected the codegen template {expected:?}, got {script:?}",
        case = row.case,
    );
}

/// The plan names the spelling the CATALOG holds, not the one the request spelled: a
/// request in another case is served the same plan as one in the stored case.
#[tokio::test]
async fn the_plan_names_the_catalogs_own_spelling() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "Plan_DB" } }),
    )
    .await;
    for table in ["Orders", "Orders_Copy"] {
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": "Plan_DB",
                "TableInput": {
                    "Name": table,
                    "StorageDescriptor": { "Columns": [{ "Name": "Id", "Type": "int" }] },
                },
            }),
        )
        .await;
    }

    let answered = ok(
        &router,
        "GetPlan",
        &json!({
            "Source": { "DatabaseName": "PLAN_db", "TableName": "ORDERS" },
            "Sinks": [{ "DatabaseName": "PLAN_db", "TableName": "ORDERS_copy" }],
            "Mapping": [{ "SourcePath": "Id" }],
        }),
    )
    .await;
    let script = generated("a request in another case", &answered, false);
    for named in [
        "database = \"plan_db\", table_name = \"orders\",",
        "table_name = \"orders_copy\",",
    ] {
        assert!(
            script.contains(named),
            "the plan must name {named:?}, the catalog's own spelling; got {script:?}"
        );
    }
    // The stored column's declared type still fills the mapping, matched case-insensitively
    // like every other catalog name.
    assert!(
        script.contains("mappings = [(\"Id\", \"int\", \"Id\", \"int\")]"),
        "the mapped field must take the stored column's type; got {script:?}"
    );
}

/// The plan is a REPORT: asking for it twice answers the same script, and the tables it
/// read are unchanged afterwards.
#[tokio::test]
async fn generating_the_plan_changes_nothing() {
    for row in &rows() {
        let router = emulator();
        given_row(&router, row).await;
        let request = get_plan_request(row);

        let before = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": DATABASE, "Name": row.source.name }),
        )
        .await;
        let first = ok(&router, "GetPlan", &request).await;
        let again = ok(&router, "GetPlan", &request).await;
        let after = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": DATABASE, "Name": row.source.name }),
        )
        .await;

        assert_eq!(
            first, again,
            "case {case:?}: two identical calls must answer the same plan; got {first} then \
             {again}",
            case = row.case,
        );
        assert_eq!(
            before, after,
            "case {case:?}: the source table must be unchanged by the plan; it read {before} \
             before and {after} after",
            case = row.case,
        );
    }
}

/// One row of the miss table: the case, the database and tables the request names, and the
/// `Message` the refusal has to carry.
struct MissRow {
    case: &'static str,
    database: &'static str,
    source: &'static str,
    sinks: Vec<&'static str>,
    message: String,
}

/// A plan over a database or table the catalog does not hold is refused with HTTP 400 and
/// this operation's modelled `InvalidInputException` — there is no code to generate for a
/// table nothing can read — whether the absent entity is the source's or a target's.
#[tokio::test]
async fn an_absent_source_or_target_is_refused() {
    const PRESENT: &str = "present_tbl";

    let misses = vec![
        MissRow {
            case: "the source names no stored database",
            database: "no_such_db",
            source: PRESENT,
            sinks: vec![],
            // The refusal for an absent database, as this service words it.
            message: "Database no_such_db not found.".to_string(),
        },
        MissRow {
            case: "the source names no stored table",
            database: DATABASE,
            source: "no_such_tbl",
            sinks: vec![],
            // The refusal for an absent table read through its database.
            message: "Table no_such_tbl not found.".to_string(),
        },
        MissRow {
            case: "a target names no stored table",
            database: DATABASE,
            source: PRESENT,
            sinks: vec!["no_such_sink"],
            message: "Table no_such_sink not found.".to_string(),
        },
        MissRow {
            case: "the second of two targets names no stored table",
            database: DATABASE,
            source: PRESENT,
            sinks: vec![PRESENT, "no_such_sink"],
            message: "Table no_such_sink not found.".to_string(),
        },
    ];

    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(&router, DATABASE, &flat(PRESENT, &[("id", Some("int"))])).await;

    for miss in &misses {
        let mut request = json!({
            "Source": { "DatabaseName": miss.database, "TableName": miss.source },
            "Mapping": [{ "SourcePath": "id" }],
            // Every miss request states a destination, so what it is refused for is the
            // entity it names and not the either/or rule `mapping_destination` pins.
            "Location": { "S3": [{ "Name": "path", "Value": "s3://bucket/out/" }] },
        });
        if !miss.sinks.is_empty() {
            request["Sinks"] = Value::Array(
                miss.sinks
                    .iter()
                    .map(|sink| json!({ "DatabaseName": miss.database, "TableName": sink }))
                    .collect(),
            );
        }
        // The status, code and wording assertions are this crate's shared ones
        // (`error_messages::assert_refusal`).
        assert_refusal(
            &router,
            miss.case,
            "GetPlan",
            &request,
            "InvalidInputException",
            &miss.message,
        )
        .await;
    }

    // A refused plan is still a read: the table that IS stored is served afterwards, so the
    // misses above cannot be a handler that refuses everything.
    let served = ok(
        &router,
        "GetPlan",
        &json!({
            "Source": { "DatabaseName": DATABASE, "TableName": PRESENT },
            "Mapping": [{ "SourcePath": "id" }],
            "Location": { "S3": [{ "Name": "path", "Value": "s3://bucket/out/" }] },
        }),
    )
    .await;
    assert!(
        generated("the stored table", &served, false).contains("table_name = \"present_tbl\""),
        "the stored table must still be planned, got {served}"
    );
}

/// One `GetPlan` body naming `sinks` copies of `table` as the targets of `source`, with
/// `entries` mapping entries.
fn request_with(source: &str, table: &str, sinks: usize, entries: usize) -> Value {
    json!({
        "Source": { "DatabaseName": DATABASE, "TableName": source },
        "Sinks": (0..sinks)
            .map(|_| json!({ "DatabaseName": DATABASE, "TableName": table }))
            .collect::<Vec<Value>>(),
        "Mapping": (0..entries)
            .map(|at| json!({ "SourcePath": format!("col_{at}"), "SourceType": "string" }))
            .collect::<Vec<Value>>(),
    })
}

/// The model bounds neither the `Sinks` list nor the `Mapping` list, so this operation
/// bounds the request itself: a request AT either ceiling is served in full, and one past it
/// is refused with the modelled `InvalidInputException` naming the ceiling — rather than
/// served as a script whose size is the one list's length times the other's.
#[tokio::test]
async fn the_size_bounds_admit_the_ceiling_and_refuse_past_it() {
    const TABLE: &str = "bounded_tbl";
    // A mapping long enough that a plan onto half the admitted targets sits exactly on the
    // field ceiling, so both the ceiling and one past it are reachable cheaply.
    let targets_at_the_field_ceiling = MAX_MAPPING_SINKS / 2;
    let entries_at_the_field_ceiling = MAX_MAPPING_ENTRIES / targets_at_the_field_ceiling;

    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(&router, DATABASE, &flat(TABLE, &[("id", Some("int"))])).await;

    // AT the target ceiling: one pipeline per target, all of them generated. Each pipeline
    // is four nodes beside the one source node.
    let served = ok(
        &router,
        "GetPlan",
        &request_with(TABLE, TABLE, MAX_MAPPING_SINKS, 1),
    )
    .await;
    let script = generated("at the target ceiling", &served, false);
    assert_eq!(
        statements(&script, false).len(),
        1 + 4 * MAX_MAPPING_SINKS,
        "a request naming the admitted maximum of targets must be planned in full, got \
         {count} statements",
        count = statements(&script, false).len(),
    );

    // One target past it: refused as a request-size failure, before any lookup.
    assert_refusal(
        &router,
        "one target past the ceiling",
        "GetPlan",
        &request_with(TABLE, TABLE, MAX_MAPPING_SINKS + 1, 1),
        "InvalidInputException",
        &format!("Sinks may name at most {MAX_MAPPING_SINKS} tables"),
    )
    .await;

    // AT the mapped-field ceiling: the product of the entries and the targets named.
    let served = ok(
        &router,
        "GetPlan",
        &request_with(
            TABLE,
            TABLE,
            targets_at_the_field_ceiling,
            entries_at_the_field_ceiling,
        ),
    )
    .await;
    let script = generated("at the field ceiling", &served, false);
    assert_eq!(
        statements(&script, false).len(),
        1 + 4 * targets_at_the_field_ceiling,
        "a plan of exactly the admitted maximum of mapped fields must be generated in full, \
         got {count} statements",
        count = statements(&script, false).len(),
    );

    // One entry past it: the same request-size refusal, on the product this time, naming
    // both factors so a caller can see which to cut.
    let past = entries_at_the_field_ceiling + 1;
    assert_refusal(
        &router,
        "one mapping entry past the field ceiling",
        "GetPlan",
        &request_with(TABLE, TABLE, targets_at_the_field_ceiling, past),
        "InvalidInputException",
        &format!(
            "a plan of {past} mapping entries onto {targets_at_the_field_ceiling} target \
             tables carries more than the {MAX_MAPPING_ENTRIES} mapped fields one plan may \
             carry"
        ),
    )
    .await;

    // The bounds refuse a request, never the store: the small plan is still generated.
    let served = ok(&router, "GetPlan", &request_with(TABLE, TABLE, 1, 1)).await;
    assert_eq!(
        statements(&generated("inside both bounds", &served, false), false).len(),
        5,
        "a request inside both bounds must still be planned, got {served}"
    );
}
