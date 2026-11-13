// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `NamespaceProperties` every read of a Cloud Map namespace
//! reports.
//!
//! The rule under test: `Properties.DnsProperties` describes the Route 53 hosted zone
//! Cloud Map keeps for a namespace and that zone's Start of Authority record, and it is
//! part of what a namespace IS — so every namespace read reports the structure, whatever
//! the namespace's type, carrying the properties that namespace actually has:
//!
//! * `HostedZoneId` names the hosted zone of a namespace that has one, and is absent for
//!   a namespace that has none;
//! * `SOA` is always reported, and because `SOA$TTL` is modelled `@required` it always
//!   carries a TTL for purposes of negative caching: the one the namespace was created or
//!   updated with, and zero for a namespace that was never given one;
//! * the same projection answers `GetNamespace` and every `ListNamespaces` summary, so a
//!   listing and a per-namespace read can never disagree.
//!
//! The rows span all three `NamespaceType` values, both references a read accepts (id and
//! ARN), a namespace created with no `Properties` at all, SOA TTLs at both published
//! `RecordTTL` bounds, and a namespace whose properties an update rewrote.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), reusing
//! the one harness in `tests/all/cloud_map_core.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{emulator, ok};

/// A VPC id of the shape `CreatePrivateDnsNamespace$Vpc` takes (`ResourceId`, 0..64).
const VPC: &str = "vpc-0123456789abcdef0";

/// The published `RecordTTL` range, from `smithy_query(query="shape", shape="RecordTTL")`:
/// `range {min: 0, max: 2147483647}`.
const TTL_MIN: i64 = 0;
const TTL_MAX: i64 = 2147483647;

/// What a namespace read must report for the namespace a row created.
struct Expected {
    /// Whether the namespace holds a Route 53 hosted zone, which only a DNS namespace
    /// does.
    hosted_zone: bool,
    /// The `SOA.TTL` the namespace reports.
    soa_ttl: i64,
}

/// Create a namespace with `create` and return its id, resolved through the
/// `CREATE_NAMESPACE` operation the call files.
async fn create_namespace(router: &MultiServiceRouter, create: &str, body: Value) -> String {
    let created = ok(router, create, body).await;
    let operation_id = created["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{create} returns an OperationId, got {created}"))
        .to_string();
    let filed = ok(
        router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .unwrap_or_else(|| panic!("the CREATE_NAMESPACE operation targets a namespace, got {filed}"))
        .to_string()
}

/// `GetNamespace`'s view of one namespace, named by its id or its ARN.
async fn read(router: &MultiServiceRouter, reference: &str) -> Value {
    ok(router, "GetNamespace", json!({ "Id": reference })).await["Namespace"].clone()
}

/// The `ListNamespaces` summary of one namespace, out of a page that holds every stored
/// namespace — so the assertions are made against a MULTI-entry page.
async fn summary(router: &MultiServiceRouter, case: &str, namespace_id: &str) -> Value {
    let mut request = json!({});
    let mut pages = 0usize;
    loop {
        let listed = ok(router, "ListNamespaces", request.clone()).await;
        let namespaces = listed["Namespaces"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListNamespaces answers a list, got {listed}"))
            .clone();
        if let Some(found) = namespaces
            .iter()
            .find(|summary| summary["Id"] == json!(namespace_id))
        {
            return found.clone();
        }
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: paging must terminate while looking for {namespace_id}"
        );
        match listed["NextToken"].as_str() {
            Some(token) => request["NextToken"] = json!(token),
            None => panic!("case {case:?}: {namespace_id} must be listed, got {listed}"),
        }
    }
}

/// Assert the rule on one namespace, through `GetNamespace` by id and by ARN and through
/// the namespace's `ListNamespaces` summary.
async fn assert_properties(
    router: &MultiServiceRouter,
    case: &str,
    namespace_id: &str,
    expected: &Expected,
) {
    let by_id = read(router, namespace_id).await;
    let arn = by_id["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a namespace reports its ARN, got {by_id}"))
        .to_string();
    let by_arn = read(router, &arn).await;
    let listed = summary(router, case, namespace_id).await;

    for (path, read) in [
        ("GetNamespace by id", &by_id),
        ("GetNamespace by ARN", &by_arn),
        ("the ListNamespaces summary", &listed),
    ] {
        let case = format!("{case}, through {path}");
        let properties = &read["Properties"];
        let dns = &properties["DnsProperties"];
        assert!(
            dns.is_object(),
            "case {case:?}: a namespace reports a DnsProperties structure, got {read}"
        );

        let hosted_zone = &dns["HostedZoneId"];
        assert_eq!(
            hosted_zone.is_string(),
            expected.hosted_zone,
            "case {case:?}: expected a hosted zone reported: {}, got {hosted_zone:?}",
            expected.hosted_zone
        );

        assert!(
            dns["SOA"].is_object(),
            "case {case:?}: DnsProperties reports the SOA of the namespace, got {dns:?}"
        );
        assert_eq!(
            dns["SOA"]["TTL"],
            json!(expected.soa_ttl),
            "case {case:?}: expected SOA TTL {}, got {:?}",
            expected.soa_ttl,
            dns["SOA"]["TTL"]
        );

        // Every namespace this service stores is also discoverable by the API, so the
        // HTTP side of the same structure is reported beside the DNS side.
        assert!(
            properties["HttpProperties"]["HttpName"].is_string(),
            "case {case:?}: a namespace reports the name it is discovered under, got {read}"
        );
    }
}

/// Every namespace reports its DNS properties, whatever its type and whatever SOA TTL it
/// was given — including none at all.
#[tokio::test]
async fn every_namespace_read_reports_its_dns_properties() {
    let router = emulator();

    // (case, the create to call, the request members beside Name, what a read must
    // report)
    let cases: Vec<(&str, &str, Value, Expected)> = vec![
        (
            "an HTTP namespace, which holds no hosted zone",
            "CreateHttpNamespace",
            json!({}),
            Expected {
                hosted_zone: false,
                soa_ttl: TTL_MIN,
            },
        ),
        (
            "an HTTP namespace with a description",
            "CreateHttpNamespace",
            json!({ "Description": "the API-only namespace" }),
            Expected {
                hosted_zone: false,
                soa_ttl: TTL_MIN,
            },
        ),
        (
            "a public DNS namespace created with no Properties",
            "CreatePublicDnsNamespace",
            json!({}),
            Expected {
                hosted_zone: true,
                soa_ttl: TTL_MIN,
            },
        ),
        (
            "a public DNS namespace at the published low TTL bound",
            "CreatePublicDnsNamespace",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": TTL_MIN } } } }),
            Expected {
                hosted_zone: true,
                soa_ttl: TTL_MIN,
            },
        ),
        (
            "a public DNS namespace at the published high TTL bound",
            "CreatePublicDnsNamespace",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": TTL_MAX } } } }),
            Expected {
                hosted_zone: true,
                soa_ttl: TTL_MAX,
            },
        ),
        (
            "a private DNS namespace created with no Properties",
            "CreatePrivateDnsNamespace",
            json!({ "Vpc": VPC }),
            Expected {
                hosted_zone: true,
                soa_ttl: TTL_MIN,
            },
        ),
        (
            "a private DNS namespace with a minute of negative caching",
            "CreatePrivateDnsNamespace",
            json!({ "Vpc": VPC, "Properties": { "DnsProperties": { "SOA": { "TTL": 60 } } } }),
            Expected {
                hosted_zone: true,
                soa_ttl: 60,
            },
        ),
    ];

    // Every namespace is created first, so the listing each row is checked against holds
    // all of them rather than one.
    let mut created = Vec::new();
    for (index, (case, create, extra, expected)) in cases.into_iter().enumerate() {
        let mut body = json!({ "Name": format!("props-row{index}.example.com") });
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }
        created.push((case, create_namespace(&router, create, body).await, expected));
    }

    for (case, namespace_id, expected) in &created {
        assert_properties(&router, case, namespace_id, expected).await;
    }
}

/// An update rewrites the properties its change carries and nothing else, and the
/// namespace still reports the whole structure afterwards: a DNS namespace reports the
/// TTL the update gave it, and an HTTP namespace — which an update can only re-describe —
/// keeps reporting a DNS structure with no hosted zone.
#[tokio::test]
async fn an_updated_namespace_still_reports_its_dns_properties() {
    let router = emulator();

    let http = create_namespace(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "props-updated-http" }),
    )
    .await;
    ok(
        &router,
        "UpdateHttpNamespace",
        json!({ "Id": http, "Namespace": { "Description": "re-described" } }),
    )
    .await;
    assert_properties(
        &router,
        "an HTTP namespace an update re-described",
        &http,
        &Expected {
            hosted_zone: false,
            soa_ttl: TTL_MIN,
        },
    )
    .await;

    // (case, the create, its extra request members, the update, the TTL the change
    // carries)
    let dns_cases: Vec<(&str, &str, Value, &str, i64)> = vec![
        (
            "a public DNS namespace first given a TTL by an update",
            "CreatePublicDnsNamespace",
            json!({}),
            "UpdatePublicDnsNamespace",
            300,
        ),
        (
            "a public DNS namespace whose TTL an update replaced",
            "CreatePublicDnsNamespace",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": 30 } } } }),
            "UpdatePublicDnsNamespace",
            TTL_MAX,
        ),
        (
            "a private DNS namespace first given a TTL by an update",
            "CreatePrivateDnsNamespace",
            json!({ "Vpc": VPC }),
            "UpdatePrivateDnsNamespace",
            TTL_MIN,
        ),
        (
            "a private DNS namespace whose TTL an update replaced",
            "CreatePrivateDnsNamespace",
            json!({ "Vpc": VPC, "Properties": { "DnsProperties": { "SOA": { "TTL": 45 } } } }),
            "UpdatePrivateDnsNamespace",
            120,
        ),
    ];

    for (index, (case, create, extra, update, ttl)) in dns_cases.into_iter().enumerate() {
        let mut body = json!({ "Name": format!("props-updated{index}.example.com") });
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }
        let namespace_id = create_namespace(&router, create, body).await;
        ok(
            &router,
            update,
            json!({
                "Id": namespace_id,
                "Namespace": { "Properties": { "DnsProperties": { "SOA": { "TTL": ttl } } } },
            }),
        )
        .await;
        assert_properties(
            &router,
            case,
            &namespace_id,
            &Expected {
                hosted_zone: true,
                soa_ttl: ttl,
            },
        )
        .await;
    }
}
