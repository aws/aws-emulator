// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTable`'s `MultiRegionConsistency` and
//! `GlobalTableWitnessUpdates` — the multi-Region strong consistency (MRSC) surface.
//!
//! The rule these tests pin:
//!
//! > `MultiRegionConsistency` and `GlobalTableWitnessUpdates` are modelled `UpdateTable`
//! > members stating a configuration of the replication GROUP, so a request naming them is
//! > applied against stored state and answered a `TableDescription` — never an unmodelled
//! > error: the mode and the witness Region are recorded on every regional table of the
//! > group, each witness reports `CREATING` until the clock makes its join due and `ACTIVE`
//! > from then on (`DELETING` until a removal is due), every full-replica Region's
//! > `DescribeTable` reports the same `MultiRegionConsistency` and `GlobalTableWitnesses`,
//! > and a configuration AWS does not serve is refused HTTP 400 `ValidationException` with
//! > the stored group left exactly as it was.
//!
//! The rows differ along what the rule generalises over: the Region a witness serves from
//! and the Region the group's second replica lives in — one row per shape of Region code —
//! so a projection keyed to one Region fails here.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same `MultiServiceRouter`
//! the emulator's front doors use, and waits for the CONDITION the clock owes through the
//! shared bounded poll loop rather than for a fixed time.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call_in, emulator, error_code, group_member_in, message, ok_in, table_when, REGION,
};

/// One row of the MRSC table: the case, the Region the group's second replica lives in, and
/// the Region that serves as its witness.
type WitnessCase = (&'static str, &'static str, &'static str);

/// The groups each test forms, one row per shape of Region code: one Region in the same
/// continent group as [`REGION`], one on another continent, and one whose code carries three
/// words.
const WITNESS_CASES: [WitnessCase; 3] = [
    ("a replica and a witness on this continent", "us-west-2", "us-east-2"),
    ("a replica and a witness on another", "eu-west-1", "eu-central-1"),
    ("a witness whose code has three words", "ap-south-1", "us-gov-east-1"),
];

/// The table name one row addresses, named for the Region its witness serves from so the
/// rows cannot collide.
fn table_of(witness: &str) -> String {
    format!("Mrsc.{witness}")
}

/// One PAY_PER_REQUEST table with a single HASH key, created in [`REGION`].
async fn create_table(router: &MultiServiceRouter, name: &str) {
    ok_in(
        router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// The `GlobalTableWitnesses` entry a description carries for `region`, or `Null` when it
/// carries none.
fn witness_in(description: &Value, region: &str) -> Value {
    group_member_in(description, "GlobalTableWitnesses", region)
}

/// The `Replicas` entry a description carries for `region`, or `Null` when it carries none.
fn replica_in(description: &Value, region: &str) -> Value {
    group_member_in(description, "Replicas", region)
}

/// Whether a description reports the whole MRSC group settled: the table, the replica in
/// `replica` and the witness in `witness` all `ACTIVE`.
fn mrsc_settled(table: &Value, replica: &str, witness: &str) -> bool {
    table["TableStatus"] == json!("ACTIVE")
        && replica_in(table, replica)["ReplicaStatus"] == json!("ACTIVE")
        && witness_in(table, witness)["WitnessStatus"] == json!("ACTIVE")
}

#[tokio::test]
async fn an_mrsc_global_table_is_formed_with_a_witness_and_reported_from_every_replica_region() {
    let router = emulator();
    for (_, _, witness) in WITNESS_CASES {
        create_table(&router, &table_of(witness)).await;
    }
    // Every table was created before anything waits, so the whole table shares ONE
    // transition span instead of one each.
    for (case, _, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            table["TableStatus"] == json!("ACTIVE")
        })
        .await;
        // A table on its own is no global table, so it reports neither member.
        let standalone = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name}))
            .await;
        assert_eq!(
            (
                standalone["Table"]["MultiRegionConsistency"].clone(),
                standalone["Table"]["GlobalTableWitnesses"].clone(),
            ),
            (Value::Null, Value::Null),
            "case {case}: a regional table of its own reports no consistency mode and no \
             witness, got {standalone}"
        );
    }

    // The MRSC create: the modelled members are honoured and answered a `TableDescription`.
    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "MultiRegionConsistency": "STRONG",
                "ReplicaUpdates": [{"Create": {"RegionName": replica}}],
                "GlobalTableWitnessUpdates": [{"Create": {"RegionName": witness}}],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a request naming modelled members is served, got {status} with {body}"
        );
        let description = &body["TableDescription"];
        assert_eq!(
            (
                description["MultiRegionConsistency"].clone(),
                witness_in(description, witness)["WitnessStatus"].clone(),
                replica_in(description, replica)["ReplicaStatus"].clone(),
            ),
            (
                json!("STRONG"),
                json!("CREATING"),
                json!("CREATING"),
            ),
            "case {case}: the accepted update describes the group it is forming, got {body}"
        );
    }

    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        // The group settles on the clock, polled for rather than slept out.
        let primary = table_when(&router, case, REGION, &name, |table| {
            mrsc_settled(table, replica, witness)
        })
        .await;
        assert_eq!(
            (
                primary["MultiRegionConsistency"].clone(),
                witness_in(&primary, witness),
            ),
            (
                json!("STRONG"),
                json!({"RegionName": witness, "WitnessStatus": "ACTIVE"}),
            ),
            "case {case}: the primary Region reports the settled MRSC group, got {primary}"
        );
        // ...and so does the replica's own Region: the configuration is the GROUP's, so
        // every full-replica Region answers the same mode and the same witness.
        let replicated = table_when(&router, case, replica, &name, |table| {
            mrsc_settled(table, REGION, witness)
        })
        .await;
        assert_eq!(
            (
                replicated["MultiRegionConsistency"].clone(),
                witness_in(&replicated, witness),
            ),
            (
                json!("STRONG"),
                json!({"RegionName": witness, "WitnessStatus": "ACTIVE"}),
            ),
            "case {case}: the replica Region reports the same group, got {replicated}"
        );
        // The witness holds no replica of the data, so the group has no regional table
        // there at all.
        let (status, body) = call_in(
            &router,
            witness,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ResourceNotFoundException".to_string()),
            "case {case}: a witness Region holds no replica table, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_mrsc_group_serves_a_cross_region_read_of_a_just_written_item() {
    let router = emulator();
    for (_, _, witness) in WITNESS_CASES {
        create_table(&router, &table_of(witness)).await;
    }
    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            table["TableStatus"] == json!("ACTIVE")
        })
        .await;
        ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "MultiRegionConsistency": "STRONG",
                "ReplicaUpdates": [{"Create": {"RegionName": replica}}],
                "GlobalTableWitnessUpdates": [{"Create": {"RegionName": witness}}],
            }),
        )
        .await;
    }

    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            mrsc_settled(table, replica, witness)
        })
        .await;
        ok_in(
            &router,
            REGION,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"Id": {"S": "k1"}, "Version": {"N": "2"}},
            }),
        )
        .await;
        // A strongly consistent read from the OTHER Region of the group sees the write with
        // no propagation wait.
        let read = ok_in(
            &router,
            replica,
            "GetItem",
            json!({"TableName": name, "Key": {"Id": {"S": "k1"}}, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            read["Item"]["Version"],
            json!({"N": "2"}),
            "case {case}: the cross-Region read sees the write, got {read}"
        );
        // ...so a conditional write from that Region stating a STALE version is refused
        // against the cross-Region-latest item.
        let (status, body) = call_in(
            &router,
            replica,
            "UpdateItem",
            json!({
                "TableName": name,
                "Key": {"Id": {"S": "k1"}},
                "UpdateExpression": "SET #v = :next",
                "ConditionExpression": "#v = :stale",
                "ExpressionAttributeNames": {"#v": "Version"},
                "ExpressionAttributeValues": {":next": {"N": "3"}, ":stale": {"N": "1"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ConditionalCheckFailedException".to_string(),
                "The conditional request failed".to_string(),
            ),
            "case {case}: the stale conditional write is refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn time_to_live_is_refused_on_a_table_of_an_mrsc_group_and_served_on_every_other() {
    let router = emulator();
    // Two tables per row: the one that joins an MRSC group, and the control that stays a
    // regional table of its own — the refusal follows the GROUP's mode, not the operation.
    for (_, _, witness) in WITNESS_CASES {
        create_table(&router, &table_of(witness)).await;
        create_table(&router, &format!("{}.control", table_of(witness))).await;
    }
    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            table["TableStatus"] == json!("ACTIVE")
        })
        .await;
        ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "MultiRegionConsistency": "STRONG",
                "ReplicaUpdates": [{"Create": {"RegionName": replica}}],
                "GlobalTableWitnessUpdates": [{"Create": {"RegionName": witness}}],
            }),
        )
        .await;
    }

    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            mrsc_settled(table, replica, witness)
        })
        .await;
        let specification = json!({"Enabled": true, "AttributeName": "ExpiresAt"});
        for region in [REGION, replica] {
            let (status, body) = call_in(
                &router,
                region,
                "UpdateTimeToLive",
                json!({"TableName": name, "TimeToLiveSpecification": specification}),
            )
            .await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "ValidationException".to_string(),
                    "Time To Live is not supported for global tables with \
                     MultiRegionConsistency set to STRONG."
                        .to_string(),
                ),
                "case {case} in {region}: TTL is refused on a table of an MRSC group, got \
                 {status} with {body}"
            );
        }
        // The control table is in no group at all, so the same call is served there.
        let control = format!("{name}.control");
        let served = ok_in(
            &router,
            REGION,
            "UpdateTimeToLive",
            json!({"TableName": control, "TimeToLiveSpecification": specification}),
        )
        .await;
        assert_eq!(
            served["TimeToLiveSpecification"]["AttributeName"],
            json!("ExpiresAt"),
            "case {case}: a table outside an MRSC group still takes the setting, got {served}"
        );
    }
}

#[tokio::test]
async fn a_witness_and_a_replica_leave_an_mrsc_group_together() {
    let router = emulator();
    for (_, _, witness) in WITNESS_CASES {
        create_table(&router, &table_of(witness)).await;
    }
    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            table["TableStatus"] == json!("ACTIVE")
        })
        .await;
        ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "MultiRegionConsistency": "STRONG",
                "ReplicaUpdates": [{"Create": {"RegionName": replica}}],
                "GlobalTableWitnessUpdates": [{"Create": {"RegionName": witness}}],
            }),
        )
        .await;
    }

    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        table_when(&router, case, REGION, &name, |table| {
            mrsc_settled(table, replica, witness)
        })
        .await;
        // "You cannot delete a single witness from a MRSC global table - you must delete
        // both a replica and the witness together": the witness alone is refused, and the
        // group is left exactly as it was.
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "GlobalTableWitnessUpdates": [{"Delete": {"RegionName": witness}}],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                "One or more parameter values were invalid: You cannot delete a single \
                 witness from a MRSC global table - you must delete both a replica and the \
                 witness together"
                    .to_string(),
            ),
            "case {case}: removing the witness alone is refused, got {status} with {body}"
        );
        let unchanged = ok_in(
            &router,
            REGION,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            witness_in(&unchanged["Table"], witness)["WitnessStatus"],
            json!("ACTIVE"),
            "case {case}: the refused removal left the witness in the group, got {unchanged}"
        );
    }

    for (case, replica, witness) in WITNESS_CASES {
        let name = table_of(witness);
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "ReplicaUpdates": [{"Delete": {"RegionName": replica}}],
                "GlobalTableWitnessUpdates": [{"Delete": {"RegionName": witness}}],
            }),
        )
        .await;
        assert_eq!(
            (
                status,
                witness_in(&body["TableDescription"], witness)["WitnessStatus"].clone(),
            ),
            (200, json!("DELETING")),
            "case {case}: the teardown is served and reports the witness leaving, got \
             {status} with {body}"
        );
    }

    for (case, _, witness) in WITNESS_CASES {
        let name = table_of(witness);
        // Both members gone, the remaining replica is a single-Region table again: no
        // witness, no replica and no consistency mode.
        let standalone = table_when(&router, case, REGION, &name, |table| {
            table["GlobalTableWitnesses"] == Value::Null && table["Replicas"] == Value::Null
        })
        .await;
        assert_eq!(
            standalone["MultiRegionConsistency"],
            Value::Null,
            "case {case}: a table left on its own reports no consistency mode, got \
             {standalone}"
        );
        // ...and the setting the MRSC group refused is served again.
        let served = ok_in(
            &router,
            REGION,
            "UpdateTimeToLive",
            json!({
                "TableName": name,
                "TimeToLiveSpecification": {"Enabled": true, "AttributeName": "ExpiresAt"},
            }),
        )
        .await;
        assert_eq!(
            served["TimeToLiveSpecification"]["Enabled"],
            json!(true),
            "case {case}: the torn-down table takes the setting again, got {served}"
        );
    }
}
