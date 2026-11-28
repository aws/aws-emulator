// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DisassociateTableReplication`.
//!
//! The rule under test: the operation acts only on a REPLICA — the table
//! replication created at a destination — and turns it into a table of the
//! account's own: `GetTable` and `ListTables` stop reporting the service that
//! managed it, and the source's replication status reports the destination as one
//! replication can no longer write to. A table that is not a replica, one an
//! earlier call already disassociated included, is refused and left exactly as it
//! is.
//!
//! The table spans what the rule generalises over: the number of destinations a
//! table is replicated to (so a disassociated replica is told apart from the
//! replicas left alone), the Region, and the kind of table the call addresses.

use serde_json::{json, Value};

use super::harness::{
    assert_served, await_replication_completed, served, Emulator,
};

/// The fixed sentences this service states the classes below with.
const NOT_A_REPLICA: &str = "The specified table is not a replica table.";
const NO_TABLE: &str = "The specified table does not exist.";
const REGION_MISMATCH: &str = "The provided Region does not match the Region of the resource.";
const NAME_TAKEN: &str =
    "The destination holds a table of this name that replication does not manage.";

/// The service that manages a replica (`GetTable$managedByService`): S3 Tables'
/// own replication principal.
const REPLICATION_PRINCIPAL: &str = "replication.s3tables.amazonaws.com";

/// The role a replication configuration names.
const ROLE: &str = "arn:aws:iam::000000000000:role/s3tables-disassociate";

/// The namespace and the table every row of this suite replicates.
const NAMESPACE: &str = "ns_replica";
const TABLE: &str = "replicated";

/// Create a table bucket and answer its ARN.
async fn create_bucket(emulator: &Emulator, region: &str, name: &str) -> String {
    served(
        emulator
            .create_table_bucket(region, &json!({ "name": name }))
            .await,
        "CreateTableBucket",
        name,
    )
    .text("arn")
}

/// Create a source table bucket holding one namespace and one table, and answer
/// the bucket's ARN beside the table's.
async fn create_source(emulator: &Emulator, region: &str, bucket: &str) -> (String, String) {
    let bucket_arn = create_bucket(emulator, region, bucket).await;
    served(
        emulator
            .create_namespace(region, &bucket_arn, &json!({ "namespace": [NAMESPACE] }))
            .await,
        "CreateNamespace",
        bucket,
    );
    let created = served(
        emulator
            .create_table(
                region,
                &bucket_arn,
                NAMESPACE,
                &json!({ "name": TABLE, "format": "ICEBERG" }),
            )
            .await,
        "CreateTable",
        bucket,
    );
    (bucket_arn, created.text("tableARN"))
}

/// Replicate `table_arn` to `destinations`.
async fn replicate(emulator: &Emulator, region: &str, table_arn: &str, destinations: &[String]) {
    let configuration = json!({
        "role": ROLE,
        "rules": [{
            "destinations": destinations
                .iter()
                .map(|arn| json!({ "destinationTableBucketARN": arn }))
                .collect::<Vec<Value>>(),
        }],
    });
    served(
        emulator
            .put_table_replication(
                region,
                &[("tableArn", table_arn.to_string())],
                &configuration,
            )
            .await,
        "PutTableReplication",
        "replicate",
    );
}

/// The service a read of `table_arn` reports as managing it, and `None` when no
/// service does.
async fn managed_by(emulator: &Emulator, region: &str, table_arn: &str, case: &str) -> Option<String> {
    let read = served(
        emulator.get_table_by_arn(region, table_arn).await,
        "GetTable",
        case,
    );
    read.optional_text("managedByService")
}

/// The replication status entry for `destination` of `table_arn`.
async fn destination_entry(
    emulator: &Emulator,
    region: &str,
    table_arn: &str,
    destination: &str,
    case: &str,
) -> Value {
    let read = served(
        emulator.get_table_replication_status(region, table_arn).await,
        "GetTableReplicationStatus",
        case,
    );
    read.list("destinations")
        .into_iter()
        .find(|entry| {
            entry
                .get("destinationTableBucketArn")
                .and_then(Value::as_str)
                == Some(destination)
        })
        .unwrap_or_else(|| panic!("case {case:?}: the status reports every destination: {read:?}"))
}

/// RULE: disassociating a replica makes it a table of the account's own, and
/// leaves every other replica of the same table alone.
#[tokio::test]
async fn disassociating_a_replica_makes_it_a_table_of_its_own() {
    // The rows differ along the number of destinations the table is replicated
    // to, so the replica that is disassociated is told apart from the ones left
    // alone, and along the Region.
    let cases: [(&str, &str, usize); 2] = [
        ("one destination", "us-east-1", 1),
        ("two destinations", "eu-west-1", 2),
    ];
    for (case, region, destinations) in cases {
        let emulator = Emulator::new();
        let (_, table_arn) = create_source(&emulator, region, "disassociate-source").await;
        let mut buckets = Vec::with_capacity(destinations);
        for index in 0..destinations {
            buckets
                .push(create_bucket(&emulator, region, &format!("disassociate-dest-{index}")).await);
        }
        replicate(&emulator, region, &table_arn, &buckets).await;
        // Wait for the CONDITION — the replication having been carried out — so a
        // replica really stands at every destination.
        let reported =
            await_replication_completed(&emulator, region, &table_arn, destinations, case).await;
        let replicas: Vec<String> = reported
            .iter()
            .map(|entry| {
                entry
                    .get("destinationTableArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a completed destination names its replica: {entry}")
                    })
                    .to_string()
            })
            .collect();

        // Every replica is a table replication manages until it is disassociated.
        for replica in &replicas {
            assert_eq!(
                managed_by(&emulator, region, replica, case).await.as_deref(),
                Some(REPLICATION_PRINCIPAL),
                "case {case:?}: a replica is managed by replication until it is disassociated"
            );
        }

        let disassociated = emulator
            .disassociate_table_replication(region, &replicas[0])
            .await;
        assert_eq!(
            disassociated.status, 200,
            "case {case:?}: DisassociateTableReplication answers its modelled 200, got {status} \
             {body}",
            status = disassociated.status,
            body = disassociated.body
        );

        // The disassociated table is the account's own now: no service manages it,
        // and the listing of its bucket says the same.
        assert_eq!(
            managed_by(&emulator, region, &replicas[0], case).await,
            None,
            "case {case:?}: a disassociated table is managed by no service"
        );
        let listed = served(
            emulator.list_tables(region, &buckets[0], &[]).await,
            "ListTables",
            case,
        );
        for row in listed.list("tables") {
            assert_eq!(
                row.get("managedByService"),
                None,
                "case {case:?}: the listing reports no managing service for a disassociated \
                 table, got {row}"
            );
        }
        // Every OTHER replica of the same table is left exactly as it was.
        for replica in replicas.iter().skip(1) {
            assert_eq!(
                managed_by(&emulator, region, replica, case).await.as_deref(),
                Some(REPLICATION_PRINCIPAL),
                "case {case:?}: disassociating one replica must leave the others alone"
            );
        }

        // On the source, the destination the replica was disassociated at is one
        // replication can no longer write to, because a table that is not
        // replication's own now stands under the name — while the others still
        // report their completed replication.
        let entry = destination_entry(&emulator, region, &table_arn, &buckets[0], case).await;
        assert_eq!(
            (
                entry.get("replicationStatus").and_then(Value::as_str),
                entry.get("failureMessage").and_then(Value::as_str),
            ),
            (Some("failed"), Some(NAME_TAKEN)),
            "case {case:?}: the source reports the disassociated destination as failed, got \
             {entry}"
        );
        for destination in buckets.iter().skip(1) {
            let entry = destination_entry(&emulator, region, &table_arn, destination, case).await;
            assert_eq!(
                entry.get("replicationStatus").and_then(Value::as_str),
                Some("completed"),
                "case {case:?}: a destination still holding its replica stays completed, got \
                 {entry}"
            );
        }

        // "Disassociating a table from replication is permanent and can't be
        // reversed": a second call on the same table is refused.
        let repeated = emulator
            .disassociate_table_replication(region, &replicas[0])
            .await;
        assert_eq!(
            (repeated.status, repeated.message()),
            (409, NOT_A_REPLICA.to_string()),
            "case {case:?}: a table already disassociated is no replica, got {status} {body}",
            status = repeated.status,
            body = repeated.body
        );
    }
}

/// RULE: a table that is not a replica is refused, and the refusal changes
/// nothing about it.
#[tokio::test]
async fn disassociating_a_table_that_is_not_a_replica_is_refused() {
    let region = "ap-southeast-2";
    let case = "not a replica";
    let emulator = Emulator::new();
    let (bucket_arn, table_arn) = create_source(&emulator, region, "disassociate-plain").await;

    // The rows differ along the kind of ARN the call addresses.
    let cases: Vec<(&str, String, u16, String)> = vec![
        (
            "a table the caller created",
            table_arn.clone(),
            409,
            NOT_A_REPLICA.to_string(),
        ),
        (
            "an ARN the store holds no table for",
            format!("{bucket_arn}/table/00000000-0000-4000-8000-000000000000"),
            404,
            NO_TABLE.to_string(),
        ),
        (
            "a table of another Region",
            "arn:aws:s3tables:us-east-2:000000000000:bucket/disassociate-plain/table/\
             00000000-0000-4000-8000-000000000001"
                .to_string(),
            400,
            REGION_MISMATCH.to_string(),
        ),
    ];
    for (row, arn, status, message) in cases {
        let refused = emulator.disassociate_table_replication(region, &arn).await;
        assert_eq!(
            (refused.status, refused.message()),
            (status, message),
            "case {case:?} ({row}): the call is refused, got {actual} {body}",
            actual = refused.status,
            body = refused.body
        );
    }

    // The table the first row addressed is exactly as it was: still there, and
    // still managed by no service.
    let read = emulator.get_table_by_arn(region, &table_arn).await;
    assert_served(&read, "GetTable", case);
    assert_eq!(
        read.optional_text("managedByService"),
        None,
        "case {case:?}: a refused call must change nothing about the table, got {body}",
        body = read.body
    );
}
