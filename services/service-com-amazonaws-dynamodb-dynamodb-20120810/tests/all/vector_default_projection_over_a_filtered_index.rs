// SPDX-License-Identifier: CC0-1.0

//! What a `SearchVectors` carrying NO `ProjectionExpression` retrieves through an index that
//! projects the whole table and filters inline.
//!
//! RULE under test: **a default retrieval through a vector index answers the attributes that
//! index projects WITHOUT the one its `VectorAttribute` names — "if not specified, the
//! operation returns all attributes projected into the vector index"
//! (`SearchVectors$ProjectionExpression`), while a result carries "the projected attributes
//! of the item returned by the vector search" (`SearchResultItem$Item`), which the vector the
//! index found the item BY is not one of. A `ProjectionExpression` "identifies one or more
//! attributes to retrieve from the index", so that attribute is handed back exactly where the
//! request names it, in the form the index holds it in: "each element is a 32-bit IEEE-754
//! floating point number" (`SearchVectors$SearchVector`, the shape
//! `VectorIndex$VectorAttribute` names on an item).**
//!
//! The rule is about the index SEARCHED and not about what that index stores, so it is asked
//! here of indexes whose `Projection` is `ALL` — "all of the table attributes are projected
//! into the index" (`Projection$ProjectionType`), so no `NonKeyAttributes` filtering applies
//! and the vector is withheld by this rule alone — and that declare an `INLINE_FILTER` search
//! schema, "an attribute projected into the vector index for filtering at the storage layer
//! during search" (`SearchSchemaElement$SearchSchemaElementType`), which a result therefore
//! does carry.
//!
//! The rows differ along what the rule generalises over: the index's `Dimensions`, spanning
//! the length `SearchVectorList` admits (one component, the `4096` that list's `@length` caps
//! it at, and counts between), its `DistanceFunction` (each of the three
//! `VectorDistanceFunction` declares), the name of the vector attribute (so one index's
//! vector is another's ordinary projected attribute), how many `INLINE_FILTER` elements the
//! search schema carries (one and two), whether the search pins them in a
//! `SearchConditionExpression` at all, and how many attributes the item carries outside the
//! vector.
//!
//! Nothing here waits a fixed span: each table is polled until its indexes report `ACTIVE`
//! through the crate's shared bounded waiter, whose deadline is a backstop against a
//! transition that never happens rather than a measurement.
//!
//! Every operation is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same `MultiServiceRouter`
//! the emulator's front doors use.

use serde_json::{json, Value};

use crate::emulator::{emulator, ok, search_result_attributes, table_when, REGION};

/// The table's partition-key attribute, which every projection carries: "the primary key
/// attributes … are automatically projected" (`Projection`).
const KEY: &str = "pk";

/// The largest `SearchVector` the model admits — `SearchVectorList` carries a `@length` of 1
/// to 4096 (`smithy_query(query="shape", shape="SearchVectorList")`) — which is the dimension
/// count the widest row below declares, and the size at which withholding the vector from a
/// default retrieval matters most.
const WIDEST_DIMENSIONS: usize = 4096;

/// A vector of `dimensions` components pointing along its first axis.
///
/// Written with the decimals a 32-bit float reads back as, so the coordinates the index HOLDS
/// ([`crate::emulator`]'s service holds them as floats) are the coordinates the request wrote
/// and a retrieved vector can be compared against what was put.
fn embedding(dimensions: usize) -> Value {
    Value::Array(
        (0..dimensions)
            .map(|axis| json!({"N": if axis == 0 { "1.0" } else { "0.0" }}))
            .collect(),
    )
}

/// One row: a vector index over the table below, and the search schema it filters by.
struct Row {
    case: &'static str,
    index: &'static str,
    /// The attribute the index finds an item by — the one a default retrieval withholds.
    vector: &'static str,
    dimensions: usize,
    distance: &'static str,
    /// The `INLINE_FILTER` elements of the index's search schema, which a search may pin.
    filters: &'static [&'static str],
}

/// The rows, every one of them an `ALL` projection with an `INLINE_FILTER` search schema.
fn rows() -> &'static [Row] {
    &[
        Row {
            case: "one dimension, COSINE, one inline filter",
            index: "one-dim-cosine",
            vector: "single",
            dimensions: 1,
            distance: "COSINE",
            filters: &["tier"],
        },
        Row {
            case: "four dimensions, EUCLIDEAN, one inline filter",
            index: "four-dim-euclidean",
            vector: "emb",
            dimensions: 4,
            distance: "EUCLIDEAN",
            filters: &["tier"],
        },
        Row {
            case: "four dimensions, DOT_PRODUCT, two inline filters",
            index: "four-dim-dot-product",
            vector: "beta",
            dimensions: 4,
            distance: "DOT_PRODUCT",
            filters: &["tier", "lang"],
        },
        Row {
            case: "the widest vector the model admits, COSINE, two inline filters",
            index: "widest-dim-cosine",
            vector: "wide",
            dimensions: WIDEST_DIMENSIONS,
            distance: "COSINE",
            filters: &["tier", "lang"],
        },
    ]
}

/// The item every row searches for: the key, the two attributes the schemas filter on, one
/// attribute no schema names (projected by `ALL` alone), and one vector per row.
fn item() -> Value {
    let mut written = json!({
        KEY: {"S": "item-1"},
        "tier": {"S": "gold"},
        "lang": {"S": "en"},
        "note": {"S": "projected by ALL"},
    });
    for row in rows() {
        written[row.vector] = json!({"L": embedding(row.dimensions)});
    }
    written
}

/// The attribute names the stored item carries, sorted — the set a default retrieval through
/// an `ALL` index answers, minus the vector that index found the item by.
fn stored_attributes() -> Vec<String> {
    let mut names: Vec<String> = item()
        .as_object()
        .expect("the written item is a document")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// The vector index declaration one row stands for.
fn index(row: &Row) -> Value {
    let schema: Vec<Value> = row
        .filters
        .iter()
        .map(|attribute| {
            json!({"AttributeName": attribute, "SearchSchemaElementType": "INLINE_FILTER"})
        })
        .collect();
    json!({
        "IndexName": row.index,
        "VectorAttribute": {"AttributeName": row.vector},
        "Dimensions": row.dimensions,
        "DistanceFunction": row.distance,
        // "ALL - All of the table attributes are projected into the index"
        // (`Projection$ProjectionType`), so nothing here is withheld for being unprojected.
        "Projection": {"ProjectionType": "ALL"},
        "SearchSchema": schema,
    })
}

/// Poll `DescribeTable` until every vector index of `table` reports `ACTIVE` — the waiter a
/// client runs before searching one, since "the index must be in the ACTIVE state"
/// (`SearchVectors$IndexName`). The crate's shared bounded waiter does the polling, so this
/// suite carries no deadline of its own.
async fn indexes_ready(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
) {
    table_when(router, case, REGION, table, |described| {
        let declared = described["VectorIndexes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        !declared.is_empty()
            && declared
                .iter()
                .all(|index| index["IndexStatus"] == json!("ACTIVE"))
    })
    .await;
}

/// One `SearchVectors` through `row`'s index: narrowed by `projection_expression` when the
/// caller names one, and pinning every `INLINE_FILTER` element of the index's schema when
/// `pin_filters` is set — "inline filters are optional in the SearchConditionExpression"
/// (`SearchSchemaElement$SearchSchemaElementType`), so both readings are asked of each row.
async fn search(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    row: &Row,
    projection_expression: Option<&str>,
    pin_filters: bool,
) -> Value {
    let mut body = json!({
        "TableName": table,
        "IndexName": row.index,
        "SearchVector": embedding(row.dimensions),
        "TopK": 5,
    });
    if pin_filters {
        let condition = row
            .filters
            .iter()
            .map(|attribute| format!("{attribute} = :{attribute}"))
            .collect::<Vec<String>>()
            .join(" AND ");
        let mut values = json!({});
        for attribute in row.filters {
            values[format!(":{attribute}")] = item()[*attribute].clone();
        }
        body["SearchConditionExpression"] = json!(condition);
        body["ExpressionAttributeValues"] = values;
    }
    if let Some(expression) = projection_expression {
        body["ProjectionExpression"] = json!(expression);
    }
    ok(router, "SearchVectors", body).await
}

/// The table every row searches, with one vector index per row, polled until each reports
/// `ACTIVE` and holding [`item`].
async fn filtered_table(router: &smithy_server_runtime::MultiServiceRouter) -> &'static str {
    let table = "vector-default-projection-over-a-filtered-index";
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": KEY, "KeyType": "HASH"}],
            // A search-schema attribute must be declared ("one element in SearchSchema is not
            // defined in attribute definitions"), and a `VectorAttribute` must not be: no
            // schema is built over the vector itself.
            "AttributeDefinitions": [
                {"AttributeName": KEY, "AttributeType": "S"},
                {"AttributeName": "tier", "AttributeType": "S"},
                {"AttributeName": "lang", "AttributeType": "S"},
            ],
            "VectorIndexes": rows().iter().map(index).collect::<Vec<Value>>(),
        }),
    )
    .await;
    indexes_ready(router, "the table carrying the filtered vector indexes", table).await;
    ok(
        router,
        "PutItem",
        json!({"TableName": table, "Item": item()}),
    )
    .await;
    table
}

#[tokio::test]
async fn a_default_retrieval_withholds_the_searched_index_vector() {
    let router = emulator();
    let table = filtered_table(&router).await;
    let stored = stored_attributes();

    for row in rows() {
        for pinned in [false, true] {
            let answered = search(&router, table, row, None, pinned).await;
            let reported = search_result_attributes(&answered, 0);
            let expected: Vec<String> = stored
                .iter()
                .filter(|name| name.as_str() != row.vector)
                .cloned()
                .collect();
            assert_eq!(
                reported,
                expected,
                "case {:?} (inline filters pinned: {pinned}): a default retrieval answers the \
                 attributes the index projects without the {:?} it found the item by, expected \
                 {expected:?}, got {reported:?} from {answered}",
                row.case,
                row.vector
            );
            assert!(
                answered["SearchResults"][0]["Item"][row.vector].is_null(),
                "case {:?} (inline filters pinned: {pinned}): the vector attribute {:?} is not \
                 one of the attributes a default retrieval reports, got {answered}",
                row.case,
                row.vector
            );
            // The attributes the index filters at the storage layer ARE retrieved, as are the
            // key and an attribute only `ALL` projects.
            for carried in row.filters.iter().chain([&KEY, &"note"]) {
                assert_eq!(
                    answered["SearchResults"][0]["Item"][carried],
                    item()[carried],
                    "case {:?} (inline filters pinned: {pinned}): {carried:?} is projected into \
                     the index and comes back as written, got {answered}",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn naming_the_vector_attribute_retrieves_it_whatever_its_width() {
    let router = emulator();
    let table = filtered_table(&router).await;

    for row in rows() {
        // "A string that identifies one or more attributes to retrieve from the index.
        // Separate attribute names with commas" (`SearchVectors$ProjectionExpression`).
        let expression = format!("{}, {KEY}", row.vector);
        let answered = search(&router, table, row, Some(&expression), false).await;
        let reported = search_result_attributes(&answered, 0);
        let mut expected = vec![KEY.to_string(), row.vector.to_string()];
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {:?} narrowed to {expression:?}: expected {expected:?}, got {reported:?} from \
             {answered}",
            row.case
        );
        let carried = answered["SearchResults"][0]["Item"][row.vector]["L"].clone();
        assert_eq!(
            carried,
            embedding(row.dimensions),
            "case {:?} narrowed to {expression:?}: the retrieved vector round-trips the {} \
             coordinates that were written, got {carried} from {answered}",
            row.case,
            row.dimensions
        );
    }

    // The TABLE's item is untouched by what a search of its indexes hands back: every vector
    // it was written with is still stored, and still read back through `GetItem` — at its own
    // width, whatever decimals the item store canonicalises each coordinate to.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": table, "Key": {KEY: item()[KEY].clone()}}),
    )
    .await;
    for row in rows() {
        let held = read["Item"][row.vector]["L"].as_array().cloned();
        let components = held.map(|held| held.len());
        assert_eq!(
            components,
            Some(row.dimensions),
            "case {:?}: the stored item keeps the {} coordinates of the vector a default search \
             leaves out of its results, got {components:?} from {read}",
            row.case,
            row.dimensions
        );
    }
}
