// SPDX-License-Identifier: CC0-1.0

//! Amazon DynamoDB's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon DynamoDB publishes its resource counts under.
const DYNAMODB_USAGE_SERVICE: &str = "DynamoDB";

/// The page of the Amazon DynamoDB Developer Guide this service points a reader at for the
/// quota behind a row, with the anchor of the section that states it.
fn developer_guide_url(page: &str, anchor: &str) -> String {
    let anchor = if anchor.is_empty() {
        String::new()
    } else {
        format!("#{anchor}")
    };
    format!(
        "https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/{page}.html{anchor}"
    )
}

/// The clause that closes a row of this service: the page of the Developer Guide that
/// states the limit.
///
/// SOURCE: the recorded default-quota listing of `dynamodb`, whose rows close with
/// `For more information, see <this page>#<anchor>` rather than describing the limit in the
/// catalog's own words — the throughput rows point at the capacity-mode section of the
/// quotas page, the control-plane row at its API section, and the throughput-decrease row
/// at the Service Quotas page. The sentence is published WITHOUT a closing full stop after
/// the link.
fn see_developer_guide(page: &str, anchor: &str) -> String {
    format!(
        "For more information, see {}",
        developer_guide_url(page, anchor)
    )
}

/// The sentence DynamoDB publishes for one of its THROUGHPUT planes: what the plane
/// allocates, the plane it is allocated over, and the capacity-mode section of the quotas
/// page.
///
/// SOURCE: the recorded default-quota listing of `dynamodb`, whose table-level write
/// throughput row reads `The maximum number of write throughput allocated for a table or
/// global secondary index.` followed by the link this builds. The whole class is that one
/// sentence around the capacity the plane allocates and the plane it is allocated over, so
/// it is built once here.
fn throughput_description(capacity: &str, allocated_over: &str) -> String {
    format!(
        "The maximum number of {capacity} throughput allocated {allocated_over}. {}",
        see_developer_guide("Limits", "default-limits-throughput-capacity-modes")
    )
}

/// The sentence DynamoDB publishes for a plane of CAPACITY UNITS: the units the plane
/// allows, whose they are, and the clause naming which tables they are counted over.
///
/// SOURCE: the recorded default-quota listing of `dynamodb`, whose account-level
/// provisioned-write row reads `The maximum number of write capacity units allocated for
/// the account; applicable only for tables (including all associated global secondary
/// indexes) in provisioned read/write capacity mode.` and whose streams row reads the same
/// sentence around `allowed for a table with streams enabled` and `tables`. The class is
/// that one sentence around the capacity, the allowance clause as the service words it and
/// the tables the clause applies to, so it is built once here.
fn capacity_units_description(capacity: &str, allowance: &str, applicable_to: &str) -> String {
    format!(
        "The maximum number of {capacity} capacity units {allowance}; applicable only for \
         {applicable_to} in provisioned read/write capacity mode."
    )
}

/// Amazon DynamoDB's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `dynamodb` service code (the
/// "Service, account, and table quotas in Amazon DynamoDB" page of the Amazon
/// DynamoDB Developer Guide), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which quota and for the published wording of each row.
/// DynamoDB publishes its throughput planes TWICE over — once for the account and once
/// for a single table, and once for each of the two capacity modes — which is why the rows
/// are named for the plane and the mode rather than for throughput as a whole. The two
/// account-level PROVISIONED planes are the ones CloudWatch meters, each under its own
/// `AccountProvisioned*CapacityUnits` series.
///
/// The service describes a row by naming the limit and then pointing the reader at the page
/// of its Developer Guide that states it ([`see_developer_guide`]) rather than restating the
/// rule in the catalog's own words, and its throughput-decrease row is published as that
/// RULE — four decreases a day, plus one an hour where none was taken — rather than as a
/// count of something.
///
/// A quota this catalog can cite no code for is left to the default-quota-set gap rather
/// than published under a neighbouring code (see [`super::descriptive_quota_code`]), which
/// is why the import-job plane of the account is absent: `L-F3CA5463`, which this catalog
/// had published it under, is the code the throughput-decrease rule is published under.
///
/// The recorded listing states `L-F7858A77` for the SECONDARY-INDEX plane of a table and
/// publishes the decrease rule's name as the provisioned capacity decreases of a day, so
/// those are the bindings and the name carried here; the export-job concurrency that
/// binding displaced takes the code it vacated (`L-6F30DCE1`) with that code's published
/// adjustability, which is the one member of it no source names for that plane.
///
/// It states `L-923BEB7A` for the write-throughput plane of a table with STREAMS enabled,
/// so that plane is what the code publishes here; the on-demand read plane of the account
/// this catalog had served under it names no other code any source states, so it is left to
/// the default-quota-set gap.
pub(in crate::state) fn dynamodb_quotas() -> Vec<QuotaRow> {
    // The account-level provisioned-throughput planes, the two DynamoDB publishes an
    // `AWS/Usage` resource count for: the capacity units provisioned across the
    // account's tables, read and write. CloudWatch publishes each under its own
    // `Resource` series, and the rows carry no window of their own.
    let account_provisioned = |code: &str, capacity: &str, resource: &str| {
        quota(
            format!("Account-level {capacity} throughput limit (Provisioned mode)"),
            80_000.0,
        )
        .code(code)
        .metric_without_period(usage_metric(
            DYNAMODB_USAGE_SERVICE,
            &[
                ("Class", "None"),
                ("Resource", resource),
                ("Type", "Resource"),
            ],
        ))
        .describe(&format!(
            "{} {}",
            capacity_units_description(
                capacity,
                "allocated for the account",
                "tables (including all associated global secondary indexes)",
            ),
            see_developer_guide("Limits", "default-limits-throughput-capacity-modes")
        ))
    };
    vec![
        quota("Concurrent control plane operations", 500.0)
            .code("L-1BB77E89")
            .describe(&format!(
                "The maximum number of allowed concurrent control plane operations. {}",
                see_developer_guide("Limits", "limits-api")
            )),
        quota("Table-level read throughput limit", 40_000.0)
            .code("L-2A593B99")
            .describe(&throughput_description(
                "read",
                "for a table or global secondary index",
            )),
        account_provisioned("L-34F6A552", "read", "AccountProvisionedReadCapacityUnits"),
        account_provisioned("L-34F8CCC8", "write", "AccountProvisionedWriteCapacityUnits"),
        // The export-job concurrency of the account, the plane the secondary-index binding
        // below displaced: it takes the code that binding vacated, and keeps the
        // adjustability the recorded listing reports for THAT code, which is the one member
        // of it no source names for this plane.
        quota("Concurrent export jobs", 300.0)
            .code("L-6F30DCE1")
            .fixed()
            .describe(
                "The maximum number of concurrent export jobs in this account in the current \
                 Region.",
            ),
        // The write-throughput plane of a table WITH STREAMS ENABLED, published under the
        // code the recorded listing states for it: the capacity units a streaming table may
        // be allocated, closing on the clause that other quotas may apply as well and on the
        // streams section of the Service Quotas page. The on-demand account read plane this
        // catalog had published under this code names no other code any source here states,
        // so it is left to the default-quota-set gap rather than served under a neighbouring
        // quota's code.
        quota(
            "Write throughput limit for DynamoDB Streams (Provisioned mode)",
            40_000.0,
        )
        .code("L-923BEB7A")
        .describe(&format!(
            "{} Other quotas might also apply. {}",
            capacity_units_description(
                "write",
                "allowed for a table with streams enabled",
                "tables"
            ),
            see_developer_guide("ServiceQuotas", "limits-dynamodb-streams")
        )),
        quota("Table-level write throughput limit", 40_000.0)
            .code("L-AB614373")
            .describe(&throughput_description(
                "write",
                "for a table or global secondary index",
            )),
        quota(
            "Account-level write throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-C2098644")
        .describe(
            "The maximum number of write request units that all on-demand tables in this \
             account in the current Region can serve per second.",
        ),
        quota(
            "Table-level read throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-CF0CBE56")
        .describe(
            "The maximum number of read request units that a single on-demand table can \
             serve per second.",
        ),
        quota(
            "Table-level write throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-D98E8184")
        .describe(
            "The maximum number of write request units that a single on-demand table can \
             serve per second.",
        ),
        // The throughput-DECREASE rule of a table, which DynamoDB publishes as the rule
        // itself rather than as a count: four decreases are allowed at any time of the day
        // in the GMT zone, and an hour without one earns a further decrease, which is what
        // brings the day's maximum to the value this row carries.
        quota("Provisioned capacity decreases per day", 27.0)
            .code("L-F3CA5463")
            .describe(&format!(
                "A decrease is allowed up to four times any time per day (GMT time zone). \
                 Also, if there was no decrease in the past hour, an additional decrease is \
                 allowed, effectively bringing the maximum number of decreases in a day to \
                 27 times. {}",
                see_developer_guide("ServiceQuotas", "")
            )),
        // The secondary-index plane of a table, published under the code the recorded
        // listing states for it: the plane's name, limit and sentence are its own, and the
        // code it is bound to is the listing's.
        quota("Global Secondary Indexes per table", 20.0)
            .code("L-F7858A77")
            .describe(&format!(
                "The maximum number of global secondary indexes that can be created for a \
                 table. For more information, see, {}",
                developer_guide_url("ServiceQuotas", "limits-secondary-indexes")
            )),
        // The table plane of the account, the one DynamoDB publishes a CloudWatch
        // usage metric for (`AWS/Usage`, `ResourceCount`, `Resource=TableCount`).
        quota("Maximum number of tables", 2500.0)
            .code("L-F98FE922")
            .metric(usage_metric(
                DYNAMODB_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "TableCount"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(&format!(
                "The maximum number of tables that can be created per account. {}",
                see_developer_guide("Limits", "limits-tables")
            )),
    ]
}
