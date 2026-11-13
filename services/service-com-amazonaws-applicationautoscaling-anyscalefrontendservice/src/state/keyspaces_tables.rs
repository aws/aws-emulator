// SPDX-License-Identifier: CC0-1.0

//! The Amazon Keyspaces table a `cassandra` scalable target names, resolved in the service
//! that owns it.
//!
//! A scalable target is a capacity dimension OF A RESOURCE, so registering one has to resolve
//! that resource where it lives: `cassandra:table:ReadCapacityUnits` and
//! `cassandra:table:WriteCapacityUnits` scale the provisioned throughput of ONE Amazon
//! Keyspaces table, so a table that does not exist has no throughput to scale, and neither
//! has one whose capacity mode provisions none — "throughputMode:PAY_PER_REQUEST" is the
//! on-demand mode, where there is no provisioned value for a policy to move. That is the
//! same rule the DynamoDB and ECS resolvers apply one namespace over
//! ([`super::resolve_dynamodb_capacity`], [`super::resolve_ecs_capacity`]); only WHERE the
//! answer comes from differs.
//!
//! ## Why this one is asked over the wire
//!
//! Every other resolver reads the owning service's store through its state TYPE. Amazon
//! Keyspaces cannot be read that way from here: that service's own auto scaling is
//! implemented ON TOP of this one, so its crate already depends on this crate, and naming its
//! state type here would be a dependency cycle. So the table is resolved by CALLING the
//! owning service — `GetTable`, the one operation that reports a table's capacity
//! specification — in-process over the callback transport, signed the way every other
//! in-process caller signs ([`::service_common::in_process_sigv4`]). The answer is that
//! service's own, so the two can no more disagree than two readers of one store can.
//!
//! An engine on which Amazon Keyspaces is not mounted answers the call with the router's
//! `UnknownService`, which resolves NOTHING rather than refusing: the same answer this
//! service gives for a namespace whose owning store it cannot reach.

use ::serde_json::Value;

/// Amazon Keyspaces' signing name, which is what routes the in-process call to its own
/// handlers, and the service shape an awsJson `X-Amz-Target` names an operation under (that
/// model's `sigv4Name: cassandra` and service shape `KeyspacesService`,
/// `smithy_query(query="service_info")`). It speaks `awsJson1_0`.
const KEYSPACES_SIGNING_NAME: &str = "cassandra";
/// See [`KEYSPACES_SIGNING_NAME`].
const KEYSPACES_TARGET_PREFIX: &str = "KeyspacesService";
/// The media type an `awsJson1_0` request and its answer carry.
const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The error code Amazon Keyspaces answers for a table it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of `GetTable`).
const TABLE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The capacity mode that HAS provisioned throughput: "throughputMode:PROVISIONED ... you
/// specify the number of reads and writes per second that you need for your application"
/// (`CapacitySpecification$throughputMode`). Any other mode provisions none.
const PROVISIONED_THROUGHPUT_MODE: &str = "PROVISIONED";

/// The keyspace and table a `cassandra` `ResourceId` names:
/// `keyspace/<keyspaceName>/table/<tableName>`, the form Application Auto Scaling documents
/// for that namespace's table resource type.
pub fn split_cassandra_resource_id(resource_id: &str) -> Option<(&str, &str)> {
    let rest = resource_id.strip_prefix("keyspace/")?;
    let (keyspace, table) = rest.split_once("/table/")?;
    (!keyspace.is_empty() && !table.is_empty() && !table.contains('/'))
        .then_some((keyspace, table))
}

/// Resolve the table a `cassandra` scalable target names, in Amazon Keyspaces itself.
///
/// * `Ok(Some(units))` — the table exists with provisioned throughput, and this is the
///   capacity it provisions for the half of its specification the dimension scales;
/// * `Ok(None)` — this emulator could not reach the owning service, or that service reported
///   no capacity for that half, which resolves nothing rather than refusing;
/// * `Err(message)` — the table cannot carry this target, as the `ValidationException`
///   message naming why.
///
/// ONE call answers all three: the capacity specification `GetTable` reports carries the mode
/// that decides the registration and the units the registered window is enforced on.
pub async fn resolve_cassandra_table(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<Option<i32>, String> {
    let Some((keyspace, table)) = split_cassandra_resource_id(resource_id) else {
        return Err(format!(
            "ResourceId not valid for the cassandra service namespace: {resource_id} must name \
             an Amazon Keyspaces table as keyspace/<keyspace>/table/<table>"
        ));
    };
    let request = ::serde_json::json!({"keyspaceName": keyspace, "tableName": table});
    match get_table(callback, region, &request).await {
        // The table resolved: what decides the registration is the capacity mode it is in.
        // A mode the answer does not report is one this reader cannot judge, so it resolves
        // nothing rather than refusing. The refusal names the TABLE and the mode its
        // throughput must be in for a target to scale it.
        Ok(answered) => match reported_throughput_mode(&answered) {
            None | Some(PROVISIONED_THROUGHPUT_MODE) => Ok(provisioned_units(
                &answered,
                super::remote_resources::reads_the_capacity(scalable_dimension),
            )),
            Some(_) => Err(super::scalable_target_validation_failed(&format!(
                "Table {table} must be in {PROVISIONED_THROUGHPUT_MODE} throughput mode"
            ))),
        },
        Err(refusal) if refusal.code == TABLE_NOT_FOUND => {
            // The refusal names the resource and the dimension the registration asked for,
            // and states as its reason which table was looked for in which keyspace.
            Err(super::resource_validation_failed(
                resource_id,
                scalable_dimension,
                &format!("Table {table} does not exist in keyspace {keyspace}"),
            ))
        }
        // Anything else is a fact about this process rather than about the request — the
        // owning service is not mounted on this engine, or its answer could not be read — so
        // the resource is UNRESOLVED and the registration is answered on its own terms.
        Err(_) => Ok(None),
    }
}

/// One refusal another emulated service answered: the code a client branches on.
struct Refused {
    code: String,
}

/// The capacity mode `GetTable` reports for the table it answered
/// (`capacitySpecification.throughputMode`), when it reports one.
fn reported_throughput_mode(answered: &Value) -> Option<&str> {
    answered
        .get("capacitySpecification")
        .and_then(|capacity| capacity.get("throughputMode"))
        .and_then(Value::as_str)
        .filter(|mode| !mode.is_empty())
}

/// The capacity the table a `cassandra` scalable target names provisions right now, for the
/// half of its capacity specification the dimension scales, or `None` when this emulator could
/// not read it.
pub async fn read_table_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    reads: bool,
) -> Option<i32> {
    let (keyspace, table) = split_cassandra_resource_id(resource_id)?;
    let identity = ::serde_json::json!({"keyspaceName": keyspace, "tableName": table});
    let answered = get_table(callback, region, &identity).await.ok()?;
    provisioned_units(&answered, reads)
}

/// The units the capacity specification of an answered table provisions for one half of it.
fn provisioned_units(answered: &Value, reads: bool) -> Option<i32> {
    let member = if reads {
        READ_CAPACITY_UNITS
    } else {
        WRITE_CAPACITY_UNITS
    };
    let units = answered
        .get("capacitySpecification")
        .and_then(|capacity| capacity.get(member))
        .and_then(Value::as_i64)?;
    i32::try_from(units).ok()
}

/// Move the provisioned throughput of the table a `cassandra` scalable target names, in Amazon
/// Keyspaces itself, answering whether that table took the change.
///
/// This is what `cassandra:table:ReadCapacityUnits` and its write sibling scale: the table's own
/// `capacitySpecification`, which is where a client reads the capacity back. The specification
/// carries BOTH units, so the one this movement does not name is the one the table already has
/// — read from the table itself, the same way the registration resolves it — and the mode is
/// the provisioned one, which is the only mode a scalable target can be registered against.
pub async fn set_table_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    reads: bool,
    units: i32,
) -> bool {
    let Some((keyspace, table)) = split_cassandra_resource_id(resource_id) else {
        return false;
    };
    let identity = ::serde_json::json!({"keyspaceName": keyspace, "tableName": table});
    let Ok(answered) = get_table(callback, region, &identity).await else {
        return false;
    };
    let held = |member: &str| {
        answered
            .get("capacitySpecification")
            .and_then(|capacity| capacity.get(member))
            .and_then(Value::as_i64)
    };
    let (reads_now, writes_now) = (
        held(READ_CAPACITY_UNITS).unwrap_or_else(|| i64::from(units)),
        held(WRITE_CAPACITY_UNITS).unwrap_or_else(|| i64::from(units)),
    );
    let wanted = i64::from(units);
    let request = ::serde_json::json!({
        "keyspaceName": keyspace,
        "tableName": table,
        "capacitySpecification": {
            "throughputMode": PROVISIONED_THROUGHPUT_MODE,
            READ_CAPACITY_UNITS: if reads { wanted } else { reads_now },
            WRITE_CAPACITY_UNITS: if reads { writes_now } else { wanted },
        },
    });
    call(callback, region, "UpdateTable", &request).await.is_ok()
}

/// The members of a table's `capacitySpecification` that carry its provisioned units.
const READ_CAPACITY_UNITS: &str = "readCapacityUnits";
/// See [`READ_CAPACITY_UNITS`].
const WRITE_CAPACITY_UNITS: &str = "writeCapacityUnits";

/// Call Amazon Keyspaces' `GetTable` in-process and read the table it reports.
async fn get_table(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    request: &Value,
) -> Result<Value, Refused> {
    call(callback, region, "GetTable", request).await
}

/// Call one Amazon Keyspaces operation in-process and read the document it answers.
///
/// The request is the one that api takes — its operation in the `X-Amz-Target` header and the
/// input document as the body — and everything a call of another emulated service takes
/// beyond that (the signature, the dispatch, the reading of a refusal's own code) is the one
/// in-process caller this service has ([`super::in_process_calls`]).
async fn call(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refused> {
    let unreachable = || Refused {
        code: "InternalFailure".to_string(),
    };
    let body = ::serde_json::to_vec(request).map_err(|_| unreachable())?;
    let call = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{KEYSPACES_SIGNING_NAME}.{region}.amazonaws.com"),
        )
        .header("content-type", AWS_JSON_1_0)
        .header(
            "x-amz-target",
            format!("{KEYSPACES_TARGET_PREFIX}.{operation}"),
        )
        .body(body)
        .map_err(|_| unreachable())?;
    let answered = super::in_process_calls::call(callback, KEYSPACES_SIGNING_NAME, region, call)
        .await
        .ok_or_else(unreachable)?;
    if answered.is_success() {
        return Ok(answered.document().unwrap_or(Value::Null));
    }
    Err(Refused {
        code: answered.code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `cassandra` resource id names ONE table inside ONE keyspace, as
    /// `keyspace/<keyspace>/table/<table>`; anything else names no table at all.
    #[test]
    fn a_cassandra_resource_id_names_one_table_of_one_keyspace() {
        /// (case, the resource id, the keyspace and table it names)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "a table of a keyspace",
                "keyspace/orders/table/items",
                Some(("orders", "items")),
            ),
            (
                "names underscored the way Amazon Keyspaces allows",
                "keyspace/my_ks/table/my_table",
                Some(("my_ks", "my_table")),
            ),
            ("no table segment", "keyspace/orders", None),
            ("an empty table name", "keyspace/orders/table/", None),
            ("an empty keyspace name", "keyspace//table/items", None),
            ("a further segment", "keyspace/orders/table/items/extra", None),
            ("a DynamoDB resource id", "table/orders", None),
            ("nothing at all", "", None),
        ];
        for (case, resource_id, expected) in rows {
            let actual = split_cassandra_resource_id(resource_id);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
