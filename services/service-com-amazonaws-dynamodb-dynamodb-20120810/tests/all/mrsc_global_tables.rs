// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTable`'s multi-Region strong consistency (MRSC)
//! configuration: `MultiRegionConsistency` and the `GlobalTableWitnessUpdates` beside it.
//!
//! The rule these tests pin:
//!
//! > an MRSC replication group spans exactly three Regions — three full replicas, or two
//! > replicas and one witness — and the table it is formed from carries no LOCAL secondary
//! > index. A request that asks for any other configuration is refused outright with HTTP
//! > 400 `ValidationException` carrying the service's own FIXED sentence for that refusal,
//! > with nothing provisioned in any Region; a request that asks for one of the two
//! > configurations the service does serve is accepted, and every regional table of the
//! > group then reports the mode under `MultiRegionConsistency` and the witness Regions
//! > under `GlobalTableWitnesses`.
//!
//! The rows of each table below differ along what the rule generalises over: how many
//! Regions the group would span, whether the third Region is a replica or a witness, and
//! whether the table the group is formed from carries a local secondary index.
//!
//! Nothing here waits for a span: what the clock owes a replica or a witness is observed by
//! POLLING `DescribeTable` for the condition under the harness's generous deadline
//! ([`crate::emulator::table_when`]).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator, error_code, message, ok_in, table_when, REGION};

/// How a configuration whose Regions are not one of the two shapes an MRSC group takes is
/// refused — AWS's own sentence, which carries nothing of the request.
const UNSUPPORTED_COUNT: &str =
    "Unsupported replica count for global tables with MultiRegionConsistency set to STRONG.";

/// How a table carrying a local secondary index is refused MRSC — the sibling sentence of
/// the same refusal family, also fixed text.
const LOCAL_SECONDARY_INDEX: &str =
    "Local secondary indexes are not supported for global tables with MultiRegionConsistency \
     set to STRONG.";

/// How the mode asked for by a request that creates no replica is refused: that member "is
/// only valid when you create a global table by specifying one or more Create actions in the
/// ReplicaUpdates action list" (its own doc), which is the rule this service states inside
/// the clause it opens every refusal of an invalid parameter value with — a refusal of this
/// family no recording shows AWS's own wording for.
const CONSISTENCY_WITHOUT_CREATE: &str =
    "One or more parameter values were invalid: MultiRegionConsistency is only valid when the \
     request creates a global table with one or more Create actions in ReplicaUpdates";

/// The Regions an MRSC group may span in the test's own Region set: "US Region set: US East
/// (N. Virginia), US East (Ohio), US West (Oregon)"
/// (`CreateGlobalTableWitnessGroupMemberAction$RegionName`), of which [`REGION`] is the
/// first — so the other two are the Regions a replica or a witness is asked for here.
const SECOND_REGION: &str = "us-east-2";
const THIRD_REGION: &str = "us-west-2";

/// One table with a single partition key and NO local secondary index, created in
/// [`REGION`] and polled until it is `ACTIVE`.
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str) {
    ok_in(
        router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    table_when(router, case, REGION, name, |table| {
        table["TableStatus"] == json!("ACTIVE")
    })
    .await;
}

/// The same, for a table that DOES carry a local secondary index: the index needs a range
/// key beside the partition key it shares with the table.
async fn create_table_with_local_index(router: &MultiServiceRouter, case: &str, name: &str) {
    ok_in(
        router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "rank", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "LocalSecondaryIndexes": [{
                "IndexName": "ByRank",
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "rank", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    table_when(router, case, REGION, name, |table| {
        table["TableStatus"] == json!("ACTIVE")
    })
    .await;
}

/// The `ReplicaUpdates` list that creates one replica per Region named.
fn creates(regions: &[&str]) -> Value {
    Value::Array(
        regions
            .iter()
            .map(|region| json!({"Create": {"RegionName": region}}))
            .collect(),
    )
}

/// The Regions a description's `Replicas` names, sorted so a row can state them.
fn replica_regions(table: &Value) -> Vec<String> {
    let mut regions: Vec<String> = table["Replicas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|replica| replica["RegionName"].as_str().map(ToString::to_string))
        .collect();
    regions.sort();
    regions
}

/// The witness Regions a description names, each with the status it reports, sorted.
fn witnesses(table: &Value) -> Vec<(String, String)> {
    let mut held: Vec<(String, String)> = table["GlobalTableWitnesses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|witness| {
            (
                witness["RegionName"].as_str().unwrap_or_default().to_string(),
                witness["WitnessStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    held.sort();
    held
}

/// Whether every replica and every witness a description names has settled.
fn group_settled(table: &Value) -> bool {
    let replicas_active = table["Replicas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .all(|replica| replica["ReplicaStatus"] == json!("ACTIVE"));
    let witnesses_active = witnesses(table)
        .iter()
        .all(|(_, status)| status == "ACTIVE");
    replicas_active && witnesses_active
}

#[tokio::test]
async fn a_group_that_would_not_span_three_regions_is_refused_the_strong_mode() {
    // RULE: an MRSC global table spans exactly three Regions, so a `STRONG` request whose
    // replicas and witness add up to any other count is refused with the service's fixed
    // sentence for that refusal, before anything is provisioned.
    let router = emulator();

    // The rows differ in the COUNT the configuration would leave behind and in how it is
    // reached: too few Regions, too many replicas, and too many Regions with a witness
    // among them. Each row carries the sentence it must be refused with, so a row states
    // its own expectation.
    let rows: [(&str, Value, Value, &str); 4] = [
        (
            "two Regions: the local table and one replica",
            creates(&[SECOND_REGION]),
            Value::Null,
            UNSUPPORTED_COUNT,
        ),
        (
            "two Regions: the local table and one witness, with no replica create",
            Value::Array(Vec::new()),
            json!([{"Create": {"RegionName": THIRD_REGION}}]),
            // A request that creates no replica asks for the mode outside a create, which
            // is the refusal `MultiRegionConsistency`'s own doc states.
            CONSISTENCY_WITHOUT_CREATE,
        ),
        (
            "four Regions: the local table and three replicas",
            creates(&[SECOND_REGION, THIRD_REGION, "eu-west-1"]),
            Value::Null,
            UNSUPPORTED_COUNT,
        ),
        (
            "four Regions: the local table, two replicas and one witness",
            creates(&[SECOND_REGION, "eu-west-1"]),
            json!([{"Create": {"RegionName": THIRD_REGION}}]),
            UNSUPPORTED_COUNT,
        ),
    ];

    for (index, (case, replica_updates, witness_updates, expected)) in
        rows.into_iter().enumerate()
    {
        let name = format!("Mrsc.Count{index}");
        create_table(&router, case, &name).await;
        let mut request = json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
        });
        if let Some(entries) = replica_updates.as_array() {
            if !entries.is_empty() {
                request["ReplicaUpdates"] = replica_updates.clone();
            }
        }
        if !witness_updates.is_null() {
            request["GlobalTableWitnessUpdates"] = witness_updates.clone();
        }
        let (status, body) = call_in(&router, REGION, "UpdateTable", request.clone()).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (400, "ValidationException".to_string(), expected.to_string()),
            "case {case}: expected 400 ValidationException {expected:?} for {request}, got \
             {status} with {body}"
        );

        // Nothing was provisioned: the table is still a regional table of its own, with no
        // consistency mode and no witness, and no Region the request named holds a table of
        // its name.
        let described = ok_in(
            &router,
            REGION,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            (
                described["Table"]["Replicas"].clone(),
                described["Table"]["MultiRegionConsistency"].clone(),
                described["Table"]["GlobalTableWitnesses"].clone(),
            ),
            (Value::Null, Value::Null, Value::Null),
            "case {case}: the refused request configured nothing, got {described}"
        );
        for region in [SECOND_REGION, THIRD_REGION] {
            let (status, body) =
                call_in(&router, region, "DescribeTable", json!({"TableName": name})).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ResourceNotFoundException".to_string()),
                "case {case}: no table was created in {region}, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_table_carrying_a_local_secondary_index_is_refused_the_strong_mode() {
    // RULE: a local secondary index is declared with the table and indexes its own partition
    // key, and such a table is not one an MRSC group admits — refused with the family's
    // other fixed sentence whichever of the two admitted Region counts the request asks for.
    let router = emulator();

    // The rows differ in the shape of the group the request asks for: three replicas, and
    // two replicas with a witness. Both are counts the service serves, so the refusal is
    // about the TABLE.
    let rows: [(&str, Value, Value); 2] = [
        (
            "three replicas on a table carrying a local secondary index",
            creates(&[SECOND_REGION, THIRD_REGION]),
            Value::Null,
        ),
        (
            "two replicas and a witness on a table carrying a local secondary index",
            creates(&[SECOND_REGION]),
            json!([{"Create": {"RegionName": THIRD_REGION}}]),
        ),
    ];

    for (index, (case, replica_updates, witness_updates)) in rows.into_iter().enumerate() {
        let name = format!("Mrsc.LocalIndex{index}");
        create_table_with_local_index(&router, case, &name).await;
        let mut request = json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
            "ReplicaUpdates": replica_updates,
        });
        if !witness_updates.is_null() {
            request["GlobalTableWitnessUpdates"] = witness_updates.clone();
        }
        let (status, body) = call_in(&router, REGION, "UpdateTable", request.clone()).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                LOCAL_SECONDARY_INDEX.to_string()
            ),
            "case {case}: expected 400 ValidationException {LOCAL_SECONDARY_INDEX:?} for \
             {request}, got {status} with {body}"
        );
        let described = ok_in(
            &router,
            REGION,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            (
                described["Table"]["Replicas"].clone(),
                described["Table"]["MultiRegionConsistency"].clone(),
                described["Table"]["GlobalTableWitnesses"].clone(),
            ),
            (Value::Null, Value::Null, Value::Null),
            "case {case}: the refused request provisioned no replica and no witness, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn each_configuration_an_mrsc_group_takes_is_served_and_reported_by_every_member() {
    // RULE: the two configurations an MRSC group takes — three replicas, or two replicas and
    // one witness — are accepted, and the mode and the witness Regions are the GROUP's: every
    // regional table of it reports the same `MultiRegionConsistency` and the same
    // `GlobalTableWitnesses`, while a witness Region holds no table of the group's name.
    let router = emulator();

    // Row one: three full replicas, so the group spans three Regions with no witness.
    let case = "three replicas";
    let name = "Mrsc.ThreeReplicas";
    create_table(&router, case, name).await;
    let accepted = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
            "ReplicaUpdates": creates(&[SECOND_REGION, THIRD_REGION]),
        }),
    )
    .await;
    assert_eq!(
        (
            accepted["TableDescription"]["MultiRegionConsistency"].clone(),
            replica_regions(&accepted["TableDescription"]),
            witnesses(&accepted["TableDescription"]),
        ),
        (
            json!("STRONG"),
            vec![SECOND_REGION.to_string(), THIRD_REGION.to_string()],
            Vec::new(),
        ),
        "case {case}: the accepted update reports the mode and both replicas, got {accepted}"
    );
    let settled = table_when(&router, case, REGION, name, group_settled).await;
    assert_eq!(
        (
            settled["MultiRegionConsistency"].clone(),
            replica_regions(&settled),
            settled["GlobalTableWitnesses"].clone(),
        ),
        (
            json!("STRONG"),
            vec![SECOND_REGION.to_string(), THIRD_REGION.to_string()],
            Value::Null,
        ),
        "case {case}: the settled group spans three Regions with no witness, got {settled}"
    );
    for region in [SECOND_REGION, THIRD_REGION] {
        let described = ok_in(&router, region, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            described["Table"]["MultiRegionConsistency"],
            json!("STRONG"),
            "case {case}: the replica in {region} reports the group's mode, got {described}"
        );
    }

    // Row two: two replicas and one witness, the other configuration the service serves.
    let case = "two replicas and one witness";
    let name = "Mrsc.ReplicaAndWitness";
    create_table(&router, case, name).await;
    let accepted = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
            "ReplicaUpdates": creates(&[SECOND_REGION]),
            "GlobalTableWitnessUpdates": [{"Create": {"RegionName": THIRD_REGION}}],
        }),
    )
    .await;
    assert_eq!(
        (
            accepted["TableDescription"]["MultiRegionConsistency"].clone(),
            replica_regions(&accepted["TableDescription"]),
            witnesses(&accepted["TableDescription"]),
        ),
        (
            json!("STRONG"),
            vec![SECOND_REGION.to_string()],
            vec![(THIRD_REGION.to_string(), "CREATING".to_string())],
        ),
        "case {case}: the accepted update reports the witness being created, got {accepted}"
    );
    let settled = table_when(&router, case, REGION, name, group_settled).await;
    assert_eq!(
        (
            settled["MultiRegionConsistency"].clone(),
            replica_regions(&settled),
            witnesses(&settled),
        ),
        (
            json!("STRONG"),
            vec![SECOND_REGION.to_string()],
            vec![(THIRD_REGION.to_string(), "ACTIVE".to_string())],
        ),
        "case {case}: the settled group holds one replica and one ACTIVE witness, got {settled}"
    );
    let replica = ok_in(
        &router,
        SECOND_REGION,
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (
            replica["Table"]["MultiRegionConsistency"].clone(),
            witnesses(&replica["Table"]),
        ),
        (
            json!("STRONG"),
            vec![(THIRD_REGION.to_string(), "ACTIVE".to_string())],
        ),
        "case {case}: the replica's own regional table reports the group's configuration, got \
         {replica}"
    );
    // "You cannot perform read or write operations on a witness": the witness Region holds
    // no table of the group's name at all.
    let (status, body) = call_in(
        &router,
        THIRD_REGION,
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ResourceNotFoundException".to_string()),
        "case {case}: the witness Region holds no table, got {status} with {body}"
    );
}

#[tokio::test]
async fn more_witness_updates_than_one_call_carries_are_refused() {
    // RULE: `GlobalTableWitnessUpdates` is declared for ONE entry
    // (`GlobalTableWitnessGroupUpdateList`, @length min 1 max 1) — "you can create or delete
    // only one witness per UpdateTable operation" — so a request carrying more is refused by
    // the constraint the model states, with nothing configured.
    let router = emulator();
    let case = "two witness updates in one request";
    let name = "Mrsc.TwoWitnessUpdates";
    create_table(&router, case, name).await;
    let (status, body) = call_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
            "ReplicaUpdates": creates(&[SECOND_REGION]),
            "GlobalTableWitnessUpdates": [
                {"Create": {"RegionName": THIRD_REGION}},
                {"Create": {"RegionName": "eu-west-1"}},
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            "1 validation error detected: Value '2 items' at 'globalTableWitnessUpdates' failed \
             to satisfy constraint: Member must have length less than or equal to 1"
                .to_string()
        ),
        "case {case}: the request is refused, got {status} with {body}"
    );
    let described = ok_in(
        &router,
        REGION,
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (
            described["Table"]["Replicas"].clone(),
            described["Table"]["MultiRegionConsistency"].clone(),
            described["Table"]["GlobalTableWitnesses"].clone(),
        ),
        (Value::Null, Value::Null, Value::Null),
        "case {case}: the refused request configured nothing, got {described}"
    );
}

#[tokio::test]
async fn removing_the_witness_and_the_replica_leaves_a_single_region_table() {
    // RULE: "the deletion of both a witness and replica converts the remaining replica to a
    // single-Region DynamoDB table" (`GlobalTableWitnessGroupUpdate$Delete`), so the table
    // that is left reports no replica, no witness and no consistency mode — while a witness
    // Region the group never held cannot be removed at all.
    let router = emulator();
    let case = "an MRSC group losing its witness and its replica";
    let name = "Mrsc.TornDown";
    create_table(&router, case, name).await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "MultiRegionConsistency": "STRONG",
            "ReplicaUpdates": creates(&[SECOND_REGION]),
            "GlobalTableWitnessUpdates": [{"Create": {"RegionName": THIRD_REGION}}],
        }),
    )
    .await;
    table_when(&router, case, REGION, name, group_settled).await;

    // A witness the group does not hold addresses nothing, whichever Region names it.
    let (status, body) = call_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "GlobalTableWitnessUpdates": [{"Delete": {"RegionName": "eu-west-1"}}],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            "One or more parameter values were invalid: The witness in eu-west-1 is not part \
             of this table's global table"
                .to_string()
        ),
        "case {case}: removing a witness the group never held is refused, got {status} with \
         {body}"
    );

    let torn_down = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Delete": {"RegionName": SECOND_REGION}}],
            "GlobalTableWitnessUpdates": [{"Delete": {"RegionName": THIRD_REGION}}],
        }),
    )
    .await;
    assert_eq!(
        (
            replica_regions(&torn_down["TableDescription"]),
            witnesses(&torn_down["TableDescription"]),
        ),
        (
            vec![SECOND_REGION.to_string()],
            vec![(THIRD_REGION.to_string(), "DELETING".to_string())],
        ),
        "case {case}: both removals are reported in flight, got {torn_down}"
    );
    let single = table_when(&router, case, REGION, name, |table| {
        table["Replicas"] == Value::Null
    })
    .await;
    assert_eq!(
        (
            single["MultiRegionConsistency"].clone(),
            single["GlobalTableWitnesses"].clone(),
            single["TableStatus"].clone(),
        ),
        (Value::Null, Value::Null, json!("ACTIVE")),
        "case {case}: the remaining table is a single-Region table again, got {single}"
    );
}
