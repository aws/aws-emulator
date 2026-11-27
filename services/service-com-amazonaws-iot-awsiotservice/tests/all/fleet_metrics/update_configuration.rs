// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateFleetMetric` — "Updates the data for a fleet
//! metric" (`smithy_query(query="operation",operation="UpdateFleetMetric")`) — driven
//! against the `CreateFleetMetric` / `DescribeFleetMetric` / `DeleteFleetMetric`
//! helpers of the parent suite, so what an update records is read back through the
//! same reader a client reads it with.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PATCH /fleet-metric/{metricName}` request signed for the `iot` signing
//! name — so what is asserted is what an AWS SDK client would see, not an internal
//! function's return value.
//!
//! The rules under test:
//!
//! * an update is a PARTIAL write of one record: every member the request carries
//!   replaces the stored one and every member it leaves out keeps the value the last
//!   write recorded, so the identity the create minted (`metricName`, `metricArn`,
//!   `creationDate`) survives every update;
//! * a served update moves the record ONE version on — which is what the next
//!   `expectedVersion` has to name — and stamps `lastModifiedDate`, while a refused
//!   one moves nothing;
//! * `expectedVersion` is optimistic concurrency: a version other than the one the
//!   record is at is `VersionConflictException` (409) and nothing is written, so the
//!   same `expectedVersion` cannot be used twice;
//! * the configuration an update records is read under the SAME rules the create reads
//!   its own under — the index name, the aggregation and its values, the period, the
//!   unit and the field over the account's indexing configuration — and a refused
//!   update leaves the configuration the last write recorded.
//!
//! The tables span what those rules generalise over rather than one recorded request:
//! each optional member of the input updated on its own, each `AggregationTypeName`
//! the model declares, both ends of the modelled `period` range, the empty description
//! the modelled length bound admits, and each class of refusal the operation models.

use super::*;

/// The `indexName` of the THING GROUP index — one of the two names this service
/// recognises (`crate::state::FLEET_INDEX_NAMES`), which this suite's account has NOT
/// enabled, so it is the name that is an index and still does not exist.
const THING_GROUP_INDEX_NAME: &str = "AWS_ThingGroups";

/// A managed numeric field of the thing index, declared by no caller: `registry.version`
/// of the managed-field table fleet indexing publishes (`state::MANAGED_THING_FIELDS`).
const MANAGED_NUMBER_FIELD: &str = "registry.version";

/// The configuration every case below starts from: valid, and carrying EVERY optional
/// member, so a row that silently dropped one would be caught.
fn baseline_body() -> Value {
    json!({
        "queryString": "attributes.zone:north AND registry.version:[1 TO 10]",
        "aggregationType": { "name": "Statistics", "values": ["average", "sum"] },
        "aggregationField": NUMBER_CUSTOM_FIELD,
        "period": 300,
        "description": "the baseline configuration",
        "queryVersion": "2017-09-30",
        "unit": "Count",
        "indexName": THING_INDEX_NAME,
    })
}

/// An update request: `indexName` is `@required` on this operation, so every request
/// carries it and `patch` is what the case is about.
fn update_body(patch: &[(&str, Value)]) -> Value {
    let mut body = json!({ "indexName": THING_INDEX_NAME });
    let object = body
        .as_object_mut()
        .expect("the update body is a JSON object");
    for (member, value) in patch {
        object.insert((*member).to_string(), value.clone());
    }
    body
}

/// `AggregationType$values` as reported, with an absent list read as the empty one —
/// an aggregation with nothing to parameterise (`Cardinality`) carries no values.
fn aggregation_values(aggregation: &Value) -> Vec<String> {
    aggregation
        .get("values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every member of a fleet metric's record EXCEPT the three the version and the clock
/// move, keyed so one assertion compares the whole configuration at once.
///
/// `aggregationType` is flattened into its two members so an absent and an empty
/// `values` list compare equal.
fn configuration_of(described: &Value) -> Value {
    json!({
        "metricName": described["metricName"],
        "metricArn": described["metricArn"],
        "queryString": described["queryString"],
        "aggregationName": described["aggregationType"]["name"],
        "aggregationValues": aggregation_values(&described["aggregationType"]),
        "period": described["period"],
        "aggregationField": described["aggregationField"],
        "description": described["description"],
        "queryVersion": described["queryVersion"],
        "indexName": described["indexName"],
        "unit": described["unit"],
    })
}

/// The same configuration with the members a case changed replaced — what the read
/// after that case must report.
fn configuration_with(base: &Value, overrides: &[(&str, Value)]) -> Value {
    let mut expected = base.clone();
    let object = expected
        .as_object_mut()
        .expect("a configuration is a JSON object");
    for (member, value) in overrides {
        object.insert((*member).to_string(), value.clone());
    }
    expected
}

/// `DescribeFleetMetric$version`, as the read reports it.
fn version_of(described: &Value) -> i64 {
    described["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("no version in {described}"))
}

/// An epoch timestamp the read reports, as seconds.
fn timestamp_of(described: &Value, member: &str) -> f64 {
    described[member]
        .as_f64()
        .unwrap_or_else(|| panic!("no {member} in {described}"))
}

/// One accepted update: what it asks for, and the members a later read must report
/// differently from the baseline.
struct Served {
    case: &'static str,
    patch: Vec<(&'static str, Value)>,
    expected: Vec<(&'static str, Value)>,
}

/// The updates the partial-write rule must hold for. They span what it generalises
/// over: each optional member of the input on its own, each `AggregationTypeName` the
/// model declares (`smithy_query(query="shape",shape="AggregationTypeName")`:
/// `Statistics`, `Percentiles`, `Cardinality`), both ends of the modelled `period`
/// range 60..86400, a managed field as well as a declared custom one, and a request
/// carrying nothing but the required `indexName`.
fn served_updates() -> Vec<Served> {
    vec![
        Served {
            case: "the query string alone",
            patch: vec![("queryString", json!("thingName:sensor-*"))],
            expected: vec![("queryString", json!("thingName:sensor-*"))],
        },
        Served {
            case: "the aggregation, to one that takes no values",
            patch: vec![("aggregationType", json!({ "name": "Cardinality" }))],
            expected: vec![
                ("aggregationName", json!("Cardinality")),
                ("aggregationValues", json!([])),
            ],
        },
        Served {
            case: "the statistics the aggregation reports",
            patch: vec![(
                "aggregationType",
                json!({ "name": "Statistics", "values": ["count", "minimum", "stdDeviation"] }),
            )],
            expected: vec![
                ("aggregationName", json!("Statistics")),
                (
                    "aggregationValues",
                    json!(["count", "minimum", "stdDeviation"]),
                ),
            ],
        },
        Served {
            case: "the aggregation, to percentiles of the same field",
            patch: vec![(
                "aggregationType",
                json!({ "name": "Percentiles", "values": ["50", "99"] }),
            )],
            expected: vec![
                ("aggregationName", json!("Percentiles")),
                ("aggregationValues", json!(["50", "99"])),
            ],
        },
        Served {
            case: "the period, to the top of the modelled range",
            patch: vec![("period", json!(86400))],
            expected: vec![("period", json!(86400))],
        },
        Served {
            case: "the period, to the bottom of the modelled range",
            patch: vec![("period", json!(60))],
            expected: vec![("period", json!(60))],
        },
        Served {
            case: "the field, to another numeric field the index manages",
            patch: vec![("aggregationField", json!(MANAGED_NUMBER_FIELD))],
            expected: vec![("aggregationField", json!(MANAGED_NUMBER_FIELD))],
        },
        Served {
            case: "the description",
            patch: vec![("description", json!("the fleet's warm things"))],
            expected: vec![("description", json!("the fleet's warm things"))],
        },
        Served {
            case: "the description, to the empty one the length bound admits",
            patch: vec![("description", json!(""))],
            expected: vec![("description", json!(""))],
        },
        Served {
            case: "the query version",
            patch: vec![("queryVersion", json!("2016-03-23"))],
            expected: vec![("queryVersion", json!("2016-03-23"))],
        },
        Served {
            case: "the unit",
            patch: vec![("unit", json!("Bytes"))],
            expected: vec![("unit", json!("Bytes"))],
        },
        Served {
            case: "nothing but the required index name",
            patch: vec![],
            expected: vec![],
        },
    ]
}

/// RULE: an update is a PARTIAL write — every member the request carries replaces the
/// stored one, every member it leaves out keeps the value the last write recorded, the
/// identity the create minted survives it, and the record moves one version on.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::with_active_index().await;

    for (index, served) in served_updates().iter().enumerate() {
        let metric_name = format!("partial-update-{index}");
        let created = emulator.create_metric(&metric_name, &baseline_body()).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the metric an update case moves must be creatable, got {} body {}",
            served.case,
            created.status,
            created.text()
        );
        let before = emulator.describe_metric(&metric_name).await.json();

        let reply = emulator
            .update_metric(&metric_name, &update_body(&served.patch))
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected the update to be served, got {} body {}",
            served.case,
            reply.status,
            reply.text()
        );

        let after = emulator.describe_metric(&metric_name).await.json();
        let expected = configuration_with(&configuration_of(&before), &served.expected);
        assert_eq!(
            configuration_of(&after),
            expected,
            "case {:?}: expected {expected}, got {}",
            served.case,
            after
        );
        assert_eq!(
            version_of(&after),
            version_of(&before) + 1,
            "case {:?}: a served update moves the record one version on, got {}",
            served.case,
            after
        );
        assert_eq!(
            timestamp_of(&after, "creationDate"),
            timestamp_of(&before, "creationDate"),
            "case {:?}: an update must not restamp creationDate, got {}",
            served.case,
            after
        );
        assert!(
            timestamp_of(&after, "lastModifiedDate") >= timestamp_of(&after, "creationDate"),
            "case {:?}: lastModifiedDate must be at or after creationDate, got {}",
            served.case,
            after
        );

        // A second read reports the same record: the update wrote it, and reading it
        // changes nothing a later caller sees.
        let again = emulator.describe_metric(&metric_name).await.json();
        assert_eq!(
            again, after,
            "case {:?}: two reads of one record must agree, got {again} then {after}",
            served.case
        );
    }
}

/// RULE: `expectedVersion` is the version the caller believes the record is at, so a
/// request naming another one is `VersionConflictException` (409) and writes nothing —
/// which is why the same `expectedVersion` cannot be used twice.
#[tokio::test]
async fn an_update_is_checked_against_the_expected_version() {
    let emulator = Emulator::with_active_index().await;
    let metric_name = "version-checked";
    assert_eq!(
        emulator
            .create_metric(metric_name, &baseline_body())
            .await
            .status,
        200,
        "the metric the version cases name must be creatable"
    );

    // Each served update moves the record on by one, so the version the next request
    // must name is the one the read reports — never a figure fixed in this test.
    for round in 0..3_i64 {
        let described = emulator.describe_metric(metric_name).await.json();
        let current = version_of(&described);
        let configuration = configuration_of(&described);

        // Every version other than the one the record is at is a conflict, stated in
        // the wording the class asks for: the version asked about, the metric, and the
        // version the record is actually at.
        for stale in [current - 1, current + 1, current + 96] {
            let conflict = emulator
                .update_metric(
                    metric_name,
                    &update_body(&[("expectedVersion", json!(stale))]),
                )
                .await;
            assert_eq!(
                conflict.status,
                409,
                "case round {round} expectedVersion {stale}: expected 409, got {} body {}",
                conflict.status,
                conflict.text()
            );
            assert_eq!(
                conflict.error_code(),
                "VersionConflictException",
                "case round {round} expectedVersion {stale}: got {}",
                conflict.text()
            );
            assert_eq!(
                conflict.error_message(),
                format!(
                    "Version {stale} of fleet metric {metric_name} does not match the current \
                     version {current}"
                ),
                "case round {round} expectedVersion {stale}: got {}",
                conflict.text()
            );
            let refused = emulator.describe_metric(metric_name).await.json();
            assert_eq!(
                version_of(&refused),
                current,
                "case round {round} expectedVersion {stale}: a conflict must write nothing, got {refused}"
            );
            assert_eq!(
                configuration_of(&refused),
                configuration,
                "case round {round} expectedVersion {stale}: a conflict must write nothing, got {refused}"
            );
        }

        // The version the record IS at is served, and moves the record past it — so
        // replaying this very request is the conflict the round above pinned.
        let served = emulator
            .update_metric(
                metric_name,
                &update_body(&[
                    ("expectedVersion", json!(current)),
                    ("description", json!(format!("round {round}"))),
                ]),
            )
            .await;
        assert_eq!(
            served.status,
            200,
            "case round {round}: the version the record is at must be served, got {} body {}",
            served.status,
            served.text()
        );
        let moved = emulator.describe_metric(metric_name).await.json();
        assert_eq!(
            version_of(&moved),
            current + 1,
            "case round {round}: a served update moves the record one version on, got {moved}"
        );
        assert_eq!(
            moved["description"],
            json!(format!("round {round}")),
            "case round {round}: the served update must be the one recorded, got {moved}"
        );
    }

    // An update that names no expectedVersion at all is served whatever the record is
    // at: the member is optional, so an absent one asserts nothing to check.
    let before = version_of(&emulator.describe_metric(metric_name).await.json());
    assert_eq!(
        emulator
            .update_metric(metric_name, &update_body(&[]))
            .await
            .status,
        200,
        "an update naming no expectedVersion must be served"
    );
    assert_eq!(
        version_of(&emulator.describe_metric(metric_name).await.json()),
        before + 1,
        "an update naming no expectedVersion still moves the record on"
    );
}

/// One refused update: what it asks for, and how the service refuses it.
struct Refused {
    case: &'static str,
    patch: Vec<(&'static str, Value)>,
    status: u16,
    code: &'static str,
    /// The message, where the class's wording is this service's own sentence rather
    /// than a list of modelled values.
    message: Option<String>,
}

/// The updates the configuration rules must refuse. One row per class of refusal the
/// operation models, each naming a value outside what the rule admits rather than one
/// recorded request.
fn refused_updates() -> Vec<Refused> {
    vec![
        Refused {
            // `FLEET_INDEX_NAMES` is the pair of names this service recognises
            // (`AWS_Things`, `AWS_ThingGroups`); anything else is no index at all.
            case: "an indexName that is no fleet index of this service",
            patch: vec![("indexName", json!("AWS_Things_v2"))],
            status: 400,
            code: "InvalidRequestException",
            message: Some("Unrecognized indexName AWS_Things_v2".to_string()),
        },
        Refused {
            case: "an indexName that is no fleet index, in another form",
            patch: vec![("indexName", json!("my-own-index"))],
            status: 400,
            code: "InvalidRequestException",
            message: Some("Unrecognized indexName my-own-index".to_string()),
        },
        Refused {
            case: "a recognised index the account has not enabled",
            patch: vec![("indexName", json!(THING_GROUP_INDEX_NAME))],
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(format!(
                "Index {THING_GROUP_INDEX_NAME} does not exist. Please enable index by calling \
                 UpdateIndexingConfiguration"
            )),
        },
        Refused {
            // "must be multiple of 60" (`UpdateFleetMetric$period`).
            case: "a period that is no whole number of minutes",
            patch: vec![("period", json!(90))],
            status: 400,
            code: "InvalidRequestException",
            message: Some("period must be a multiple of 60; this one is 90".to_string()),
        },
        Refused {
            // The modelled `FleetMetricUnit` enum is what admits a unit, so a value
            // outside it is refused on the request walk ahead of the handler; the
            // message is the constraint's own, which is why this row pins the class
            // and not a sentence of this service's.
            case: "a unit that is no FleetMetricUnit",
            patch: vec![("unit", json!("Furlongs"))],
            status: 400,
            code: "InvalidRequestException",
            message: None,
        },
        Refused {
            case: "an aggregation type the service does not have",
            patch: vec![("aggregationType", json!({ "name": "Mean" }))],
            status: 400,
            code: "InvalidRequestException",
            message: None,
        },
        Refused {
            case: "a statistic the Statistics aggregation does not report",
            patch: vec![(
                "aggregationType",
                json!({ "name": "Statistics", "values": ["median"] }),
            )],
            status: 400,
            code: "InvalidRequestException",
            // The statistics the service reports are the modelled `Statistics` structure's
            // own members (`smithy_query(query="shape",shape="Statistics")`), which is the
            // catalog the refusal lists.
            message: Some(
                "aggregationType.values median is not a supported statistic; the supported \
                 statistics are count, average, sum, minimum, maximum, sumOfSquares, variance, \
                 stdDeviation"
                    .to_string(),
            ),
        },
        Refused {
            case: "values given to the aggregation that takes none",
            patch: vec![(
                "aggregationType",
                json!({ "name": "Cardinality", "values": ["count"] }),
            )],
            status: 400,
            code: "InvalidRequestException",
            message: Some("the Cardinality aggregation type takes no aggregationType.values".to_string()),
        },
        Refused {
            case: "a field the index does not carry",
            patch: vec![("aggregationField", json!("attributes.undeclared"))],
            status: 400,
            code: "InvalidRequestException",
            message: Some(format!(
                "Aggregation field attributes.undeclared is not indexed in index \
                 {THING_INDEX_NAME}; add it as a custom field by calling \
                 UpdateIndexingConfiguration"
            )),
        },
        Refused {
            // The baseline aggregation asks for the arithmetic statistics, which summarise
            // NUMBERS; over a field holding text the only statistic there is is the count.
            case: "an arithmetic statistic over a field that holds no numbers",
            patch: vec![("aggregationField", json!(STRING_CUSTOM_FIELD))],
            status: 400,
            code: "InvalidRequestException",
            message: Some(
                "Invalid values [average, sum] for Statistics aggregation on String field. \
                 Supported values are [count]"
                    .to_string(),
            ),
        },
        Refused {
            case: "a percentile of a field that holds no numbers",
            patch: vec![
                ("aggregationField", json!(STRING_CUSTOM_FIELD)),
                (
                    "aggregationType",
                    json!({ "name": "Percentiles", "values": ["50"] }),
                ),
            ],
            status: 400,
            code: "InvalidRequestException",
            message: Some(format!(
                "Invalid dataType String for field {STRING_CUSTOM_FIELD}. Only Number dataType \
                 is allowed for Percentiles aggregation"
            )),
        },
        Refused {
            // `Cardinality` counts the DISTINCT values of a field, which a flag is not asked
            // about at all.
            case: "the distinct values of a managed flag field",
            patch: vec![
                ("aggregationField", json!("connectivity.connected")),
                ("aggregationType", json!({ "name": "Cardinality" })),
            ],
            status: 400,
            code: "InvalidRequestException",
            message: Some(
                "Invalid dataType Boolean for field connectivity.connected. Only Number, String \
                 dataType is allowed for Cardinality aggregation"
                    .to_string(),
            ),
        },
    ]
}

/// RULE: the configuration an update records is read under the same rules the create
/// reads its own under, and a refused update leaves the configuration — and the
/// version — the last write recorded.
#[tokio::test]
async fn a_refused_update_records_nothing() {
    let emulator = Emulator::with_active_index().await;

    for (index, refused) in refused_updates().iter().enumerate() {
        let metric_name = format!("refused-update-{index}");
        assert_eq!(
            emulator
                .create_metric(&metric_name, &baseline_body())
                .await
                .status,
            200,
            "case {:?}: the metric a refusal case names must be creatable",
            refused.case
        );
        let before = emulator.describe_metric(&metric_name).await.json();

        let reply = emulator
            .update_metric(&metric_name, &update_body(&refused.patch))
            .await;
        assert_eq!(
            reply.status,
            refused.status,
            "case {:?}: expected {}, got {} body {}",
            refused.case,
            refused.status,
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            refused.code,
            "case {:?}: expected {}, got {}",
            refused.case,
            refused.code,
            reply.text()
        );
        if let Some(message) = &refused.message {
            assert_eq!(
                &reply.error_message(),
                message,
                "case {:?}: expected the refusal to state the rule, got {}",
                refused.case,
                reply.text()
            );
        }

        let after = emulator.describe_metric(&metric_name).await.json();
        assert_eq!(
            after, before,
            "case {:?}: a refused update must record nothing, got {after} was {before}",
            refused.case
        );
    }
}
