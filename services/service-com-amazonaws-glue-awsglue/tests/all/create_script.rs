// SPDX-License-Identifier: CC0-1.0

//! `CreateScript`: the ETL code a directed acyclic graph is transformed into.
//!
//! RULE: the graph a request states is the script's own record of itself — every node carries
//! what it does (`NodeType`), the name it binds (`Id`) and its properties as name-value pairs
//! (`Args`), and every edge says whose frame a node reads and under which parameter — so this
//! operation writes that graph out as a Glue ETL script: the codegen annotations above each
//! node (`@type`, `@args`, `@return`, `@inputs`) and then the statement binding the node's
//! name to the call its type names over the frames its edges name and the properties it
//! carries, inside the Glue job skeleton every generated script is written in. Every node
//! stands after the nodes binding the frames it reads, whatever order the request lists them
//! in. `Language` picks the member the code comes back in (`SCALA` → `ScalaCode`, `PYTHON` or
//! nothing → `PythonScript`), nothing is persisted, and a graph the model's own shapes cannot
//! describe — a name two nodes bind, an edge naming an end no node binds, nodes reading one
//! another's frames — is refused with `InvalidInputException`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many nodes the graph carries and
//! how they are chained, which node types it carries (a catalog-addressed source and sink, a
//! connection-addressed one, a transform), how many frames a node reads and whether each is
//! named by parameter or positionally, how a property's value is spelled (quoted, a list of
//! tuples, a key-value object, a number, a reference), the order the request lists the nodes
//! in, and which language the code is asked in. Two invariants are checked over whole classes
//! of input: the graph read back out of a generated script is the graph it was generated from,
//! and every order a chain can be listed in generates a script binding each frame before it is
//! read.

use serde_json::{json, Value};

use crate::error_messages::assert_refusal;
use crate::wire::{emulator, ok};

/// The language a row asks its code in, and the member that code comes back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    /// `PYTHON`, asked for by name.
    Python,
    /// No `Language` at all, which is the Python one too.
    Default,
    Scala,
}

impl Language {
    /// The `Language` member a request states for this row, if any.
    fn asked(self) -> Option<&'static str> {
        match self {
            Language::Python => Some("PYTHON"),
            Language::Default => None,
            Language::Scala => Some("SCALA"),
        }
    }

    /// The member of the answer the code comes back in.
    fn member(self) -> &'static str {
        match self {
            Language::Scala => "ScalaCode",
            _ => "PythonScript",
        }
    }

    /// The member of the answer that must carry nothing.
    fn other_member(self) -> &'static str {
        match self {
            Language::Scala => "PythonScript",
            _ => "ScalaCode",
        }
    }

    /// The last line of the Glue job skeleton's preamble, and the close after the nodes: the
    /// script's nodes stand between the two.
    fn skeleton(self) -> (&'static str, &'static str) {
        match self {
            Language::Scala => (
                "    Job.init(args(\"JOB_NAME\"), glueContext, args.asJava)\n",
                "    Job.commit()\n  }\n}\n",
            ),
            _ => ("job.init(args['JOB_NAME'], args)\n", "job.commit()\n"),
        }
    }
}

/// One node as a comparable row: its `Id`, its `NodeType`, and each property's name beside
/// the text its value is carried as.
type NodeRow = (String, String, Vec<(String, String)>);

/// One edge as a comparable row: its `Source`, its `Target` and the parameter it names.
type EdgeRow = (String, String, Option<String>);

/// The row one `CodeGenNode` document makes — read the same way off a node a request states
/// and off a node the graph of a generated script reports, which is what makes the two
/// comparable.
fn node_row(node: &Value) -> NodeRow {
    (
        node["Id"].as_str().unwrap_or_default().to_string(),
        node["NodeType"].as_str().unwrap_or_default().to_string(),
        node["Args"]
            .as_array()
            .map(|args| {
                args.iter()
                    .map(|arg| {
                        (
                            arg["Name"].as_str().unwrap_or_default().to_string(),
                            arg["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// The row one `CodeGenEdge` document makes, read the same way on both sides.
fn edge_row(edge: &Value) -> EdgeRow {
    (
        edge["Source"].as_str().unwrap_or_default().to_string(),
        edge["Target"].as_str().unwrap_or_default().to_string(),
        edge
            .get("TargetParameter")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// The rows of a list member of an answer, which must be reported even when it is empty.
fn rows_of(case: &str, answered: &Value, member: &str) -> Vec<Value> {
    answered
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the answer must report {member}, got {answered}"))
        .clone()
}

/// One node of a graph a request states.
fn node(id: &str, node_type: &str, args: &[(&str, &str)]) -> Value {
    json!({
        "Id": id,
        "NodeType": node_type,
        "Args": args
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect::<Vec<Value>>(),
    })
}

/// One edge: the node whose frame is read, the node reading it, and the parameter it is read
/// as — absent where the frame is read positionally.
fn edge(source: &str, target: &str, parameter: Option<&str>) -> Value {
    match parameter {
        Some(parameter) => json!({
            "Source": source,
            "Target": target,
            "TargetParameter": parameter,
        }),
        None => json!({ "Source": source, "Target": target }),
    }
}

/// One row: a graph, the language the code is asked in, and the nodes the script written from
/// it stands on — the text between the Glue job skeleton's preamble and its close.
struct Row {
    case: &'static str,
    nodes: Vec<Value>,
    edges: Vec<Value>,
    language: Language,
    body: String,
}

impl Row {
    /// The request this row makes.
    fn request(&self) -> Value {
        let mut body = json!({ "DagNodes": self.nodes, "DagEdges": self.edges });
        if let Some(language) = self.language.asked() {
            body["Language"] = json!(language);
        }
        body
    }
}

/// The nodes of a generated script: the text between the Glue job skeleton's preamble and its
/// close, which is also an assertion that the script carries that skeleton at all.
fn body(case: &str, language: Language, script: &str) -> String {
    let (preamble_ends, epilogue) = language.skeleton();
    let after = script
        .split_once(preamble_ends)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the script must open with the Glue job skeleton ending \
                 {preamble_ends:?}, got {script:?}"
            )
        })
        .1;
    after
        .strip_suffix(epilogue)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the script must close with {epilogue:?}, got {script:?}"
            )
        })
        .to_string()
}

/// The code one answer carries, with the member the request did not ask for asserted empty.
fn code(case: &str, language: Language, answered: &Value) -> String {
    assert!(
        answered.get(language.other_member()).is_none(),
        "case {case:?}: the answer must carry only {member}, got {answered}",
        member = language.member(),
    );
    answered
        .get(language.member())
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the answer must carry the code in {member}, got {answered}",
                member = language.member(),
            )
        })
        .to_string()
}

/// A graph reading one catalog table, mapping its fields and writing them to a location, as
/// Glue codegen names and chains such a pipeline.
fn pipeline_nodes() -> Vec<Value> {
    vec![
        node(
            "datasource0",
            "DataSource",
            &[
                ("database", "\"sales\""),
                ("table_name", "\"orders\""),
                ("transformation_ctx", "\"datasource0\""),
            ],
        ),
        node(
            "applymapping1",
            "ApplyMapping",
            &[
                ("mapping", "[(\"id\", \"int\", \"id\", \"int\")]"),
                ("transformation_ctx", "\"applymapping1\""),
            ],
        ),
        node(
            "datasink2",
            "DataSink",
            &[
                ("connection_type", "\"s3\""),
                (
                    "connection_options",
                    "{\"path\": \"s3://out-bucket/orders\"}",
                ),
                ("transformation_ctx", "\"datasink2\""),
            ],
        ),
    ]
}

/// The edges of that pipeline: each node reads the frame the one before it binds.
fn pipeline_edges() -> Vec<Value> {
    vec![
        edge("datasource0", "applymapping1", Some("frame")),
        edge("applymapping1", "datasink2", Some("frame")),
    ]
}

/// The rows: each graph and the script nodes it is written out as.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source, a mapping and a connection sink, in Python",
            nodes: pipeline_nodes(),
            edges: pipeline_edges(),
            language: Language::Python,
            body: concat!(
                "## @type: DataSource\n",
                "## @args: [database = \"sales\", table_name = \"orders\", transformation_ctx = \"datasource0\"]\n",
                "## @return: datasource0\n",
                "## @inputs: []\n",
                "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \"sales\", table_name = \"orders\", transformation_ctx = \"datasource0\")\n",
                "## @type: ApplyMapping\n",
                "## @args: [mapping = [(\"id\", \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\"]\n",
                "## @return: applymapping1\n",
                "## @inputs: [frame = datasource0]\n",
                "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")\n",
                "## @type: DataSink\n",
                "## @args: [connection_type = \"s3\", connection_options = {\"path\": \"s3://out-bucket/orders\"}, transformation_ctx = \"datasink2\"]\n",
                "## @return: datasink2\n",
                "## @inputs: [frame = applymapping1]\n",
                "datasink2 = glueContext.write_dynamic_frame.from_options(frame = applymapping1, connection_type = \"s3\", connection_options = {\"path\": \"s3://out-bucket/orders\"}, transformation_ctx = \"datasink2\")\n",
            )
            .to_string(),
        },
        Row {
            case: "the same graph with its nodes listed sink first",
            nodes: {
                let mut listed = pipeline_nodes();
                listed.reverse();
                listed
            },
            edges: pipeline_edges(),
            language: Language::Default,
            // The script is the SAME one: the order the request lists the nodes in does not
            // change which node reads which frame, and every frame is bound before it is read.
            body: rows_body_of_the_pipeline(),
        },
        Row {
            case: "a catalog sink and a transform reading two named frames, in Python",
            nodes: vec![
                node("left", "DataSource", &[("database", "\"sales\""), ("table_name", "\"orders\"")]),
                node("right", "DataSource", &[("database", "\"sales\""), ("table_name", "\"customers\"")]),
                node(
                    "joined",
                    "Join",
                    &[("keys1", "[\"customer_id\"]"), ("keys2", "[\"id\"]")],
                ),
                node(
                    "written",
                    "DataSink",
                    &[("database", "\"sales\""), ("table_name", "\"joined\"")],
                ),
            ],
            edges: vec![
                edge("left", "joined", Some("frame1")),
                edge("right", "joined", Some("frame2")),
                edge("joined", "written", Some("frame")),
            ],
            language: Language::Python,
            body: concat!(
                "## @type: DataSource\n",
                "## @args: [database = \"sales\", table_name = \"orders\"]\n",
                "## @return: left\n",
                "## @inputs: []\n",
                "left = glueContext.create_dynamic_frame.from_catalog(database = \"sales\", table_name = \"orders\")\n",
                "## @type: DataSource\n",
                "## @args: [database = \"sales\", table_name = \"customers\"]\n",
                "## @return: right\n",
                "## @inputs: []\n",
                "right = glueContext.create_dynamic_frame.from_catalog(database = \"sales\", table_name = \"customers\")\n",
                "## @type: Join\n",
                "## @args: [keys1 = [\"customer_id\"], keys2 = [\"id\"]]\n",
                "## @return: joined\n",
                "## @inputs: [frame1 = left, frame2 = right]\n",
                "joined = Join.apply(frame1 = left, frame2 = right, keys1 = [\"customer_id\"], keys2 = [\"id\"])\n",
                "## @type: DataSink\n",
                "## @args: [database = \"sales\", table_name = \"joined\"]\n",
                "## @return: written\n",
                "## @inputs: [frame = joined]\n",
                "written = glueContext.write_dynamic_frame.from_catalog(frame = joined, database = \"sales\", table_name = \"joined\")\n",
            )
            .to_string(),
        },
        Row {
            case: "a frame read positionally, and a property that is a reference",
            nodes: vec![
                node(
                    "source",
                    "DataSource",
                    &[("database", "args[\"DATABASE\"]"), ("table_name", "\"orders\"")],
                ),
                node("dropped", "DropFields", &[("paths", "[\"secret\"]")]),
            ],
            edges: vec![edge("source", "dropped", None)],
            language: Language::Python,
            body: concat!(
                "## @type: DataSource\n",
                "## @args: [database = args[\"DATABASE\"], table_name = \"orders\"]\n",
                "## @return: source\n",
                "## @inputs: []\n",
                "source = glueContext.create_dynamic_frame.from_catalog(database = args[\"DATABASE\"], table_name = \"orders\")\n",
                "## @type: DropFields\n",
                "## @args: [paths = [\"secret\"]]\n",
                "## @return: dropped\n",
                "## @inputs: [source]\n",
                "dropped = DropFields.apply(source, paths = [\"secret\"])\n",
            )
            .to_string(),
        },
        Row {
            case: "a numeric property and a node that reads no frame",
            nodes: vec![node("sampled", "Spigot", &[("num_records", "10")])],
            edges: vec![],
            language: Language::Python,
            body: concat!(
                "## @type: Spigot\n",
                "## @args: [num_records = 10]\n",
                "## @return: sampled\n",
                "## @inputs: []\n",
                "sampled = Spigot.apply(num_records = 10)\n",
            )
            .to_string(),
        },
        Row {
            case: "the pipeline in Scala",
            nodes: pipeline_nodes(),
            edges: pipeline_edges(),
            language: Language::Scala,
            body: concat!(
                "    // @type: DataSource\n",
                "    // @args: [database = \"sales\", table_name = \"orders\", transformation_ctx = \"datasource0\"]\n",
                "    // @return: datasource0\n",
                "    // @inputs: []\n",
                "    val datasource0 = glueContext.getCatalogSource(database = \"sales\", tableName = \"orders\", redshiftTmpDir = \"\", transformationContext = \"datasource0\").getDynamicFrame()\n",
                "    // @type: ApplyMapping\n",
                "    // @args: [mapping = [(\"id\", \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\"]\n",
                "    // @return: applymapping1\n",
                "    // @inputs: [frame = datasource0]\n",
                "    val applymapping1 = datasource0.applyMapping(mappings = Seq((\"id\", \"int\", \"id\", \"int\")), transformationContext = \"applymapping1\")\n",
                "    // @type: DataSink\n",
                "    // @args: [connection_type = \"s3\", connection_options = {\"path\": \"s3://out-bucket/orders\"}, transformation_ctx = \"datasink2\"]\n",
                "    // @return: datasink2\n",
                "    // @inputs: [frame = applymapping1]\n",
                "    val datasink2 = glueContext.getSinkWithFormat(connectionType = \"s3\", options = JsonOptions(\"\"\"{\"path\": \"s3://out-bucket/orders\"}\"\"\"), transformationContext = \"datasink2\").writeDynamicFrame(applymapping1)\n",
            )
            .to_string(),
        },
        Row {
            case: "a catalog sink in Scala",
            nodes: vec![
                node("source", "DataSource", &[("database", "\"sales\""), ("table_name", "\"orders\"")]),
                node(
                    "sink",
                    "DataSink",
                    &[("database", "\"sales\""), ("table_name", "\"orders_copy\"")],
                ),
            ],
            edges: vec![edge("source", "sink", Some("frame"))],
            language: Language::Scala,
            body: concat!(
                "    // @type: DataSource\n",
                "    // @args: [database = \"sales\", table_name = \"orders\"]\n",
                "    // @return: source\n",
                "    // @inputs: []\n",
                "    val source = glueContext.getCatalogSource(database = \"sales\", tableName = \"orders\", redshiftTmpDir = \"\").getDynamicFrame()\n",
                "    // @type: DataSink\n",
                "    // @args: [database = \"sales\", table_name = \"orders_copy\"]\n",
                "    // @return: sink\n",
                "    // @inputs: [frame = source]\n",
                "    val sink = glueContext.getCatalogSink(database = \"sales\", tableName = \"orders_copy\", redshiftTmpDir = \"\").writeDynamicFrame(source)\n",
            )
            .to_string(),
        },
        Row {
            case: "a graph with no node at all",
            nodes: vec![],
            edges: vec![],
            language: Language::Python,
            body: String::new(),
        },
        Row {
            case: "a graph with no node at all, in Scala",
            nodes: vec![],
            edges: vec![],
            language: Language::Scala,
            body: String::new(),
        },
    ]
}

/// The Python script nodes of the pipeline, as the first row pins them: the row that lists the
/// same graph's nodes in another order expects the SAME script, so the text is named once.
fn rows_body_of_the_pipeline() -> String {
    concat!(
        "## @type: DataSource\n",
        "## @args: [database = \"sales\", table_name = \"orders\", transformation_ctx = \"datasource0\"]\n",
        "## @return: datasource0\n",
        "## @inputs: []\n",
        "datasource0 = glueContext.create_dynamic_frame.from_catalog(database = \"sales\", table_name = \"orders\", transformation_ctx = \"datasource0\")\n",
        "## @type: ApplyMapping\n",
        "## @args: [mapping = [(\"id\", \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\"]\n",
        "## @return: applymapping1\n",
        "## @inputs: [frame = datasource0]\n",
        "applymapping1 = ApplyMapping.apply(frame = datasource0, mappings = [(\"id\", \"int\", \"id\", \"int\")], transformation_ctx = \"applymapping1\")\n",
        "## @type: DataSink\n",
        "## @args: [connection_type = \"s3\", connection_options = {\"path\": \"s3://out-bucket/orders\"}, transformation_ctx = \"datasink2\"]\n",
        "## @return: datasink2\n",
        "## @inputs: [frame = applymapping1]\n",
        "datasink2 = glueContext.write_dynamic_frame.from_options(frame = applymapping1, connection_type = \"s3\", connection_options = {\"path\": \"s3://out-bucket/orders\"}, transformation_ctx = \"datasink2\")\n",
    )
    .to_string()
}

/// Every row's script is the code its graph describes, written into the Glue job skeleton.
#[tokio::test]
async fn the_script_is_the_code_the_graph_describes() {
    let router = emulator();
    for row in &rows() {
        let answered = ok(&router, "CreateScript", &row.request()).await;
        let written = code(row.case, row.language, &answered);
        assert_eq!(
            body(row.case, row.language, &written),
            row.body,
            "case {case:?}: expected the script nodes {expected:?}, got {written:?}",
            case = row.case,
            expected = row.body,
        );
    }
}

/// The graph read back out of a script this operation generated is the graph that script was
/// generated from: every node with its type and its properties as the graph spells them, and
/// every edge from the node whose frame is read to the node reading it, under the same
/// parameter. A property whose value is a reference the script resolves rather than a literal
/// comes back with `Param` set, which is a reading of the value's text.
#[tokio::test]
async fn the_graph_of_a_generated_script_is_the_graph_it_describes() {
    let router = emulator();
    for row in rows()
        .iter()
        .filter(|row| row.language != Language::Scala)
    {
        let generated = ok(&router, "CreateScript", &row.request()).await;
        let script = code(row.case, row.language, &generated);
        let read = ok(
            &router,
            "GetDataflowGraph",
            &json!({ "PythonScript": script }),
        )
        .await;

        // The nodes come back in the order the SCRIPT stands them in, which the row's own
        // expected text pins.
        let order: Vec<String> = row
            .body
            .lines()
            .filter_map(|line| line.trim_start().split_once("@return: "))
            .map(|(_, id)| id.trim().to_string())
            .collect();
        let stated: Vec<NodeRow> = order
            .iter()
            .map(|id| {
                let node = row
                    .nodes
                    .iter()
                    .find(|node| node["Id"].as_str() == Some(id.as_str()))
                    .unwrap_or_else(|| panic!("case {case:?}: the row states {id}", case = row.case));
                node_row(node)
            })
            .collect();
        let answered: Vec<NodeRow> = rows_of(row.case, &read, "DagNodes")
            .iter()
            .map(node_row)
            .collect();
        assert_eq!(
            answered, stated,
            "case {case:?}: expected the graph {stated:?} back, got {answered:?}",
            case = row.case,
        );

        let mut expected: Vec<EdgeRow> = row.edges.iter().map(edge_row).collect();
        let mut sorted: Vec<EdgeRow> = rows_of(row.case, &read, "DagEdges")
            .iter()
            .map(edge_row)
            .collect();
        sorted.sort();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "case {case:?}: expected the edges {expected:?} back, got {sorted:?}",
            case = row.case,
        );
    }

    // A property stating a reference comes back as a parameter, and one stating a literal does
    // not: the reading of the generated script's own text, not a restatement of the request.
    let generated = ok(
        &router,
        "CreateScript",
        &json!({
            "DagNodes": [node(
                "source",
                "DataSource",
                &[("database", "args[\"DATABASE\"]"), ("table_name", "\"orders\"")],
            )],
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetDataflowGraph",
        &json!({ "PythonScript": generated["PythonScript"].as_str().unwrap_or_default() }),
    )
    .await;
    let params: Vec<(String, bool)> = read["DagNodes"][0]["Args"]
        .as_array()
        .unwrap_or_else(|| panic!("the graph must report the node's Args, got {read}"))
        .iter()
        .map(|arg| {
            (
                arg["Name"].as_str().unwrap_or_default().to_string(),
                arg.get("Param").and_then(Value::as_bool).unwrap_or(false),
            )
        })
        .collect();
    assert_eq!(
        params,
        vec![
            ("database".to_string(), true),
            ("table_name".to_string(), false),
        ],
        "a property spelled as a reference must come back as a parameter, got {read}"
    );
}

/// A script this emulator GENERATED is written back, byte for byte, from the graph read out of
/// it: transforming a graph into code is the inverse of transforming that code into a graph, so
/// a plan's script survives the round trip through `GetDataflowGraph` and back.
#[tokio::test]
async fn a_generated_plans_script_survives_the_round_trip() {
    let router = emulator();
    let database = "script_db";
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    for table in ["orders", "orders_copy"] {
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": database,
                "TableInput": {
                    "Name": table,
                    "StorageDescriptor": { "Columns": [{ "Name": "id", "Type": "int" }] },
                },
            }),
        )
        .await;
    }

    for (case, sinks, location) in [
        (
            "a plan writing to a catalog table",
            json!([{ "DatabaseName": database, "TableName": "orders_copy" }]),
            Value::Null,
        ),
        (
            "a plan with no target table, writing to a location",
            json!([]),
            json!({ "S3": [{ "Name": "path", "Value": "s3://bucket/orders_out/" }] }),
        ),
    ] {
        let mut request = json!({
            "Source": { "DatabaseName": database, "TableName": "orders" },
            "Sinks": sinks,
            "Mapping": [{ "SourcePath": "id" }],
        });
        // A plan states where the mapped data goes: a target table, or the location a
        // request naming no target table names instead.
        if !location.is_null() {
            request["Location"] = location;
        }
        let planned = ok(&router, "GetPlan", &request).await;
        let script = planned["PythonScript"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: GetPlan must answer a PythonScript, got {planned}"))
            .to_string();

        let graph = ok(
            &router,
            "GetDataflowGraph",
            &json!({ "PythonScript": script }),
        )
        .await;
        let written = ok(
            &router,
            "CreateScript",
            &json!({
                "DagNodes": graph["DagNodes"],
                "DagEdges": graph["DagEdges"],
                "Language": "PYTHON",
            }),
        )
        .await;
        assert_eq!(
            written["PythonScript"].as_str(),
            Some(script.as_str()),
            "case {case:?}: the graph of a generated script must write that script back; \
             expected {script:?}, got {:?}",
            written["PythonScript"].as_str(),
        );
    }
}

/// Every order the nodes of one chain can be listed in generates a script binding each frame
/// BEFORE the node reading it: the script a graph generates is a script that runs, whatever
/// order the request happened to list its nodes in.
#[tokio::test]
async fn every_frame_is_bound_before_the_node_that_reads_it() {
    /// Every ordering of `count` positions, grown by inserting each position into every place
    /// of the orderings already built.
    fn orderings(count: usize) -> Vec<Vec<usize>> {
        let mut orders: Vec<Vec<usize>> = vec![Vec::new()];
        for element in 0..count {
            let mut grown: Vec<Vec<usize>> = Vec::new();
            for order in &orders {
                for at in 0..=order.len() {
                    let mut candidate = order.clone();
                    candidate.insert(at, element);
                    grown.push(candidate);
                }
            }
            orders = grown;
        }
        orders
    }

    let chain: Vec<String> = (0..4).map(|at| format!("node{at}")).collect();
    let edges: Vec<Value> = chain
        .windows(2)
        .map(|pair| edge(&pair[0], &pair[1], Some("frame")))
        .collect();

    let router = emulator();
    for order in orderings(chain.len()) {
        let case = format!("{order:?}");
        let nodes: Vec<Value> = order
            .iter()
            .map(|&at| node(&chain[at], "SelectFields", &[("paths", "[\"id\"]")]))
            .collect();
        let answered = ok(
            &router,
            "CreateScript",
            &json!({ "DagNodes": nodes, "DagEdges": edges }),
        )
        .await;
        let script = answered["PythonScript"].as_str().unwrap_or_default();
        let bound: Vec<&str> = script
            .lines()
            .filter_map(|line| line.split_once("@return: "))
            .map(|(_, id)| id.trim())
            .collect();
        let expected: Vec<&str> = chain.iter().map(String::as_str).collect();
        assert_eq!(
            bound, expected,
            "case {case}: every node must be written after the frame it reads; expected \
             {expected:?}, got {bound:?} in {script:?}"
        );
    }
}

/// A graph the model's own shapes cannot describe is refused with HTTP 400 and this
/// operation's modelled `InvalidInputException`, naming what in the graph no script can be
/// written from.
#[tokio::test]
async fn a_graph_that_describes_no_script_is_refused() {
    struct Miss {
        case: &'static str,
        request: Value,
        message: &'static str,
    }

    let misses = vec![
        Miss {
            case: "a name two nodes bind",
            request: json!({
                "DagNodes": [
                    node("twice", "DataSource", &[]),
                    node("twice", "SelectFields", &[]),
                ],
            }),
            message: "twice names a node the graph already names",
        },
        Miss {
            case: "an edge whose source is no node of the graph",
            request: json!({
                "DagNodes": [node("target", "SelectFields", &[])],
                "DagEdges": [edge("absent", "target", Some("frame"))],
            }),
            message: "the edge from absent to target names absent, which is no node of the graph",
        },
        Miss {
            case: "an edge whose target is no node of the graph",
            request: json!({
                "DagNodes": [node("source", "DataSource", &[])],
                "DagEdges": [edge("source", "absent", Some("frame"))],
            }),
            message: "the edge from source to absent names absent, which is no node of the graph",
        },
        Miss {
            case: "two nodes reading one another's frames",
            request: json!({
                "DagNodes": [
                    node("first", "SelectFields", &[]),
                    node("second", "SelectFields", &[]),
                ],
                "DagEdges": [
                    edge("first", "second", Some("frame")),
                    edge("second", "first", Some("frame")),
                ],
            }),
            message: "the nodes first, second read one another's frames, so the graph is not \
                      the directed acyclic graph a script is generated from",
        },
        Miss {
            case: "a node reading its own frame",
            request: json!({
                "DagNodes": [node("itself", "SelectFields", &[])],
                "DagEdges": [edge("itself", "itself", Some("frame"))],
            }),
            message: "the nodes itself read one another's frames, so the graph is not the \
                      directed acyclic graph a script is generated from",
        },
    ];

    let router = emulator();
    for miss in &misses {
        assert_refusal(
            &router,
            miss.case,
            "CreateScript",
            &miss.request,
            "InvalidInputException",
            miss.message,
        )
        .await;
    }

    // A refused graph refuses nothing else: a graph that does describe a script is still
    // transformed afterwards, so the refusals above cannot be a handler that refuses
    // everything.
    let served = ok(
        &router,
        "CreateScript",
        &json!({ "DagNodes": [node("source", "DataSource", &[])] }),
    )
    .await;
    assert!(
        served["PythonScript"]
            .as_str()
            .unwrap_or_default()
            .contains("@return: source"),
        "a graph describing one node must still be transformed, got {served}"
    );
}

/// A graph AT each size bound this operation enforces is written out, and one past it is
/// refused: the bounds are this emulator's own, because the model bounds neither list.
#[tokio::test]
async fn the_graph_size_bounds_admit_their_ceiling_and_refuse_past_it() {
    use service_com_amazonaws_glue_awsglue::ops::create_script::{MAX_DAG_EDGES, MAX_DAG_NODES};

    /// A graph of `count` independent nodes.
    fn nodes_of(count: usize) -> Vec<Value> {
        (0..count)
            .map(|at| node(&format!("node{at}"), "SelectFields", &[]))
            .collect()
    }

    let router = emulator();
    for (case, count, admitted) in [
        ("at the node ceiling", MAX_DAG_NODES, true),
        ("one node past the ceiling", MAX_DAG_NODES + 1, false),
    ] {
        let request = json!({ "DagNodes": nodes_of(count) });
        if admitted {
            let answered = ok(&router, "CreateScript", &request).await;
            let written = answered["PythonScript"].as_str().unwrap_or_default();
            assert_eq!(
                written.matches("@return: ").count(),
                count,
                "case {case:?}: a graph of {count} nodes must write {count} nodes, got {written:?}"
            );
        } else {
            assert_refusal(
                &router,
                case,
                "CreateScript",
                &request,
                "InvalidInputException",
                &format!(
                    "the graph carries {count} nodes, more than the {MAX_DAG_NODES} nodes one \
                     graph may carry"
                ),
            )
            .await;
        }
    }

    for (case, count, admitted) in [
        ("at the edge ceiling", MAX_DAG_EDGES, true),
        ("one edge past the ceiling", MAX_DAG_EDGES + 1, false),
    ] {
        let edges: Vec<Value> = (0..count)
            .map(|_| edge("node0", "node1", Some("frame")))
            .collect();
        let request = json!({ "DagNodes": nodes_of(2), "DagEdges": edges });
        if admitted {
            let answered = ok(&router, "CreateScript", &request).await;
            let written = answered["PythonScript"].as_str().unwrap_or_default();
            assert_eq!(
                written.matches("frame = node0").count(),
                // Every edge names one frame the reading node is annotated with, and the
                // statement passes each of them too.
                count * 2,
                "case {case:?}: a graph of {count} edges must read {count} frames, got {written:?}"
            );
        } else {
            assert_refusal(
                &router,
                case,
                "CreateScript",
                &request,
                "InvalidInputException",
                &format!(
                    "the graph carries {count} edges, more than the {MAX_DAG_EDGES} edges one \
                     graph may carry"
                ),
            )
            .await;
        }
    }
}

/// Transforming a graph is a REPORT: two identical calls answer the same script, and a
/// transform never touches what the catalog holds.
#[tokio::test]
async fn generating_a_script_changes_nothing() {
    let router = emulator();
    let database = "unchanged_db";
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": "orders" } }),
    )
    .await;
    let read = json!({ "DatabaseName": database, "Name": "orders" });
    let before = ok(&router, "GetTable", &read).await;

    for row in &rows() {
        let request = row.request();
        let first = ok(&router, "CreateScript", &request).await;
        let again = ok(&router, "CreateScript", &request).await;
        assert_eq!(
            first, again,
            "case {case:?}: two identical calls must answer the same script; got {first} then \
             {again}",
            case = row.case,
        );
    }

    let after = ok(&router, "GetTable", &read).await;
    assert_eq!(
        before, after,
        "generating a script must leave the catalog alone; expected {before}, got {after}"
    );
}
