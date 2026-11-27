// SPDX-License-Identifier: CC0-1.0

//! An Amazon EC2 transit gateway route export WRITES the routes of the named route table into
//! the caller's Amazon S3 bucket, where a client reads the file back.
//!
//! RULE: `ExportTransitGatewayRoutes` "exports routes from the specified transit gateway route
//! table to the specified S3 bucket. By default, all routes are exported. Alternatively, you
//! can filter by CIDR range. The routes are saved to the specified bucket in a JSON file"
//! (`smithy_query(query="operation",operation="ExportTransitGatewayRoutes")`), so the export IS
//! the write: the routes the table holds — narrowed by the filter names that operation's own
//! `Filters` doc lists and by nothing else — are serialized as one JSON object stored in that
//! bucket, and the `S3Location` the call answers is the `s3://` address of exactly that object
//! ("The URL of the exported file in Amazon S3. For example,
//! s3://bucket_name/VPCTransitGateway/TransitGatewayRouteTables/file_name"). An export pointed
//! at a bucket the object store does not hold saves no file, so it is refused rather than
//! answered with a location nothing can be read at, and a dry run writes nothing at all.
//!
//! The write goes through the in-process callback into this emulator's own object store, so a
//! single-service router has no Amazon S3 to write into: that is why this suite lives here
//! rather than in the EC2 crate, whose own suite
//! (`services/service-com-amazonaws-ec2-amazonec2/tests/all/export_transit_gateway_routes.rs`)
//! pins the half of the rule that needs no object store — every refusal of the request itself.
//!
//! The rows span what the rule generalises over: which documented filter narrows the export
//! (none at all, each attribute name, each of the four CIDR relations, two names joined with an
//! AND), what each filter's values relate to, whether the selected routes forward to an
//! attachment or drop the traffic they match, and the address family of the destination.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every call here is signed for, which is where the bucket is created and the
/// Region the export addresses the object store in.
const REGION: &str = "us-west-2";

/// The bucket the exports are written into, and one no create ever makes.
const BUCKET: &str = "tgw-route-exports";
const ABSENT_BUCKET: &str = "tgw-route-exports-missing";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The key space an exported route table's file sits in, as this operation's own output
/// documents it.
const KEY_PREFIX: &str = "VPCTransitGateway/TransitGatewayRouteTables";

/// The media type the exported file is stored under: it is a JSON document.
const JSON_MEDIA_TYPE: &str = "application/json";

/// Amazon S3 and Amazon EC2 over ONE shared engine, as `router/src/lib.rs` mounts them — so the
/// export's in-process callback reaches this very object store — plus the engine, so the suite
/// can seed the transit gateway attachment a forwarding route names (that attachment's own
/// create is still a scaffold stub).
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A SigV4 `Authorization` header scoped to `service` in [`REGION`], which is what routes the
/// request and what a service reads the caller's Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One raw request to the emulator's Amazon S3 front door: path-style, so the bucket and key
/// live in the URI. Answers the status, the declared media type and the body.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, Option<String>, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (
        status,
        media_type,
        String::from_utf8_lossy(response.into_body().as_ref()).to_string(),
    )
}

/// One ec2Query action: an `application/x-www-form-urlencoded` POST answered with XML.
async fn ec2_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.into_body().as_ref()).to_string(),
    )
}

/// The same call, which must be served, answered with its XML body.
async fn ec2_ok(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = ec2_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {body}"
    );
    body
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// `PUT /<bucket>` with the location constraint a bucket outside `us-east-1` carries.
async fn given_bucket(router: &MultiServiceRouter) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, _media_type, body) =
        s3(router, "PUT", &format!("/{BUCKET}"), configuration.as_bytes()).await;
    assert_eq!(
        status, 200,
        "setup: the export bucket must be created, got {status} with {body:?}"
    );
}

/// Every key the export bucket holds, as `ListObjectsV2` reports them.
async fn listed_keys(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, _media_type, body) =
        s3(router, "GET", &format!("/{BUCKET}?list-type=2"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the export bucket must be listable, got {status} with {body:?}"
    );
    body.split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split("</Key>").next())
        .map(str::to_string)
        .collect()
}

/// The VPC a seeded attachment attaches, which is the `resourceId` a route forwarding to that
/// attachment reports.
const ATTACHED_VPC: &str = "vpc-0123456789abcdef0";

/// The routes one route table is seeded with, each as (destination, forwards to the attachment).
const SEEDED_ROUTES: [(&str, bool); 4] = [
    ("10.0.0.0/16", true),
    ("10.0.1.0/24", false),
    ("192.168.0.0/16", false),
    ("2001:db8::/32", false),
];

/// The transit gateway route table the cases export, with the attachment one of its routes
/// forwards to.
struct Fixture {
    route_table_id: String,
    attachment_id: String,
}

/// A transit gateway with one static route per entry of [`SEEDED_ROUTES`] in its default
/// association route table, created through EC2's own operations.
async fn given_routes(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Fixture {
    let created = ec2_ok(router, "CreateTransitGateway", &[]).await;
    let route_table_id = element(&created, "associationDefaultRouteTableId");
    let transit_gateway_id = element(&created, "transitGatewayId");

    // `CreateTransitGatewayVpcAttachment` is still a scaffold stub, so the attachment a
    // forwarding route names is seeded through the store's own writer — the same way the EC2
    // crate's own transit gateway suites seed one.
    let attachment_id = engine
        .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
        .lock()
        .insert_transit_gateway_attachment(ec2::state::TransitGatewayAttachmentView::Vpc(
            ec2::ops::types::TransitGatewayVpcAttachment {
                transit_gateway_id: Some(transit_gateway_id),
                vpc_id: Some(ATTACHED_VPC.to_string()),
                vpc_owner_id: Some("210987654321".to_string()),
                state: Some(
                    ec2::state::TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string(),
                ),
                subnet_ids: Some(vec!["subnet-0123456789abcdef0".to_string()]),
                ..Default::default()
            },
        ))
        .expect("the store holds the seeded attachment");

    for (destination, forwards) in SEEDED_ROUTES {
        let mut params = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("DestinationCidrBlock", destination),
        ];
        if forwards {
            params.push(param("TransitGatewayAttachmentId", &attachment_id));
        } else {
            params.push(param("Blackhole", "true"));
        }
        ec2_ok(router, "CreateTransitGatewayRoute", &params).await;
    }

    Fixture {
        route_table_id,
        attachment_id,
    }
}

/// `ExportTransitGatewayRoutes` for `route_table_id` into `bucket`, narrowed by `narrow_by`.
fn export_request(
    route_table_id: &str,
    bucket: &str,
    narrow_by: &[(&str, &[&str])],
) -> Vec<(String, String)> {
    let mut params = vec![
        param("TransitGatewayRouteTableId", route_table_id),
        param("S3Bucket", bucket),
    ];
    for (index, (name, values)) in narrow_by.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Filter.{position}.Name"), name));
        for (value_index, value) in values.iter().enumerate() {
            params.push(param(
                &format!("Filter.{position}.Value.{}", value_index + 1),
                value,
            ));
        }
    }
    params
}

/// One export, as a row of the table: the filters it narrows by and the destinations the file
/// it writes must carry, in order.
struct Case {
    name: &'static str,
    narrow_by: &'static [(&'static str, &'static [&'static str])],
    /// `None` names the attachment-id filter, whose value is minted per run and so is filled
    /// in from the fixture rather than written here.
    attachment_filter: Option<&'static str>,
    expected: &'static [&'static str],
}

/// An export writes the routes of the named table into the bucket, as the JSON file the
/// location it answers addresses — narrowed by each documented filter and by nothing else.
#[tokio::test]
async fn an_export_writes_the_tables_routes_into_the_bucket() {
    // Rows differ along which documented filter narrows the export, what that filter's values
    // relate to (text, an attachment, a CIDR relation), and how many filters are joined.
    let cases: &[Case] = &[
        Case {
            name: "no filter at all, which exports every route",
            narrow_by: &[],
            attachment_filter: None,
            expected: &[
                "10.0.0.0/16",
                "10.0.1.0/24",
                "192.168.0.0/16",
                "2001:db8::/32",
            ],
        },
        Case {
            name: "state blackhole",
            narrow_by: &[("state", &["blackhole"])],
            attachment_filter: None,
            expected: &["10.0.1.0/24", "192.168.0.0/16", "2001:db8::/32"],
        },
        Case {
            name: "state active, which is the route that forwards",
            narrow_by: &[("state", &["active"])],
            attachment_filter: None,
            expected: &["10.0.0.0/16"],
        },
        Case {
            name: "type static, which every created route is",
            narrow_by: &[("type", &["static"])],
            attachment_filter: None,
            expected: &[
                "10.0.0.0/16",
                "10.0.1.0/24",
                "192.168.0.0/16",
                "2001:db8::/32",
            ],
        },
        Case {
            name: "type propagated, which none of them is",
            narrow_by: &[("type", &["propagated"])],
            attachment_filter: None,
            expected: &[],
        },
        Case {
            name: "the destination CIDR range of one route",
            narrow_by: &[(
                "transit-gateway-route-destination-cidr-block",
                &["192.168.0.0/16"],
            )],
            attachment_filter: None,
            expected: &["192.168.0.0/16"],
        },
        Case {
            name: "the destination CIDR range as a wildcard",
            narrow_by: &[("transit-gateway-route-destination-cidr-block", &["10.*"])],
            attachment_filter: None,
            expected: &["10.0.0.0/16", "10.0.1.0/24"],
        },
        Case {
            name: "two values of one filter, joined with an OR",
            narrow_by: &[(
                "transit-gateway-route-destination-cidr-block",
                &["10.0.1.0/24", "2001:db8::/32"],
            )],
            attachment_filter: None,
            expected: &["10.0.1.0/24", "2001:db8::/32"],
        },
        Case {
            name: "the resource the attachment attaches",
            narrow_by: &[("attachment.resource-id", &[ATTACHED_VPC])],
            attachment_filter: None,
            expected: &["10.0.0.0/16"],
        },
        Case {
            name: "the attachment the route forwards to",
            narrow_by: &[],
            attachment_filter: Some("attachment.transit-gateway-attachment-id"),
            expected: &["10.0.0.0/16"],
        },
        Case {
            name: "an exact CIDR match",
            narrow_by: &[("route-search.exact-match", &["10.0.1.0/24"])],
            attachment_filter: None,
            expected: &["10.0.1.0/24"],
        },
        Case {
            name: "the routes whose destination is a subnet of the value",
            narrow_by: &[("route-search.subnet-of-match", &["10.0.0.0/8"])],
            attachment_filter: None,
            expected: &["10.0.0.0/16", "10.0.1.0/24"],
        },
        Case {
            name: "the routes whose destination encompasses the value",
            narrow_by: &[("route-search.supernet-of-match", &["10.0.1.5/32"])],
            attachment_filter: None,
            expected: &["10.0.0.0/16", "10.0.1.0/24"],
        },
        Case {
            name: "the longest prefix matching the value",
            narrow_by: &[("route-search.longest-prefix-match", &["10.0.1.5/32"])],
            attachment_filter: None,
            expected: &["10.0.1.0/24"],
        },
        Case {
            name: "two filters, joined with an AND",
            narrow_by: &[
                ("state", &["blackhole"]),
                ("route-search.subnet-of-match", &["10.0.0.0/8"]),
            ],
            attachment_filter: None,
            expected: &["10.0.1.0/24"],
        },
    ];

    let (router, engine) = emulator();
    given_bucket(&router).await;
    let fixture = given_routes(&router, &engine).await;

    for case in cases {
        let name = case.name;
        let owned_values: Vec<String> = vec![fixture.attachment_id.clone()];
        let borrowed: Vec<&str> = owned_values.iter().map(String::as_str).collect();
        let mut narrow_by: Vec<(&str, &[&str])> = case
            .narrow_by
            .iter()
            .map(|(filter, values)| (*filter, *values))
            .collect();
        if let Some(filter) = case.attachment_filter {
            narrow_by.push((filter, borrowed.as_slice()));
        }

        let (status, body) = ec2_call(
            &router,
            "ExportTransitGatewayRoutes",
            &export_request(&fixture.route_table_id, BUCKET, &narrow_by),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the export must be served, got {status} with {body}"
        );

        // The location the call answers is the `s3://` address of the file it wrote, inside the
        // key space this operation's output documents.
        let location = element(&body, "s3Location");
        let expected_prefix = format!(
            "s3://{BUCKET}/{KEY_PREFIX}/{route_table}_",
            route_table = fixture.route_table_id
        );
        assert!(
            location.starts_with(&expected_prefix) && location.ends_with(".json"),
            "case {name:?}: expected a location under {expected_prefix:?} ending in .json, got \
             {location:?}"
        );
        let key = location
            .strip_prefix(&format!("s3://{BUCKET}/"))
            .unwrap_or_default()
            .to_string();

        // What the client does next: read the file back out of the bucket it named.
        let (status, media_type, file) = s3(&router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the exported file at {key:?} must be readable, got {status} with \
             {file:?}"
        );
        assert_eq!(
            media_type.as_deref(),
            Some(JSON_MEDIA_TYPE),
            "case {name:?}: the exported file is a JSON document, got {media_type:?}"
        );
        let document: Value = serde_json::from_str(&file).unwrap_or_else(|error| {
            panic!("case {name:?}: the exported file is not JSON: {error}: {file}")
        });

        let exported: Vec<String> = document["routes"]
            .as_array()
            .map(|routes| {
                routes
                    .iter()
                    .map(|route| {
                        route["destinationCidrBlock"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_else(|| panic!("case {name:?}: the file carries its routes: {document}"));
        let expected: Vec<String> = case.expected.iter().map(|one| one.to_string()).collect();
        assert_eq!(
            exported, expected,
            "case {name:?}: expected the file to carry {expected:?} in destination order, got \
             {exported:?} in {document}"
        );

        // The filters the export was narrowed by are stated in the file, so a reader can tell
        // which routes of the table it is a report of — and an export narrowed by nothing
        // states none.
        let stated = document.get("filters");
        if narrow_by.is_empty() {
            assert!(
                stated.is_none(),
                "case {name:?}: an export narrowed by nothing states no filters, got {document}"
            );
        } else {
            let stated = stated
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {name:?}: the file states its filters: {document}"));
            let names: Vec<&str> = stated
                .iter()
                .map(|filter| filter["name"].as_str().unwrap_or_default())
                .collect();
            let asked: Vec<&str> = narrow_by.iter().map(|(filter, _)| *filter).collect();
            assert_eq!(
                names, asked,
                "case {name:?}: expected the file to state the filters {asked:?}, got {names:?} \
                 in {document}"
            );
            for (stated, (_, values)) in stated.iter().zip(narrow_by.iter()) {
                let stated_values: Vec<&str> = stated["values"]
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().unwrap_or_default())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    stated_values, *values,
                    "case {name:?}: expected the file to state the values {values:?}, got \
                     {stated_values:?} in {document}"
                );
            }
        }

        // A route that forwards carries the attachment it forwards to, with that attachment's
        // own resource — the members `SearchTransitGatewayRoutes` reports for the same route.
        if case.expected.contains(&"10.0.0.0/16") {
            let forwarding = document["routes"]
                .as_array()
                .and_then(|routes| {
                    routes
                        .iter()
                        .find(|route| route["destinationCidrBlock"] == "10.0.0.0/16")
                })
                .unwrap_or_else(|| panic!("case {name:?}: the forwarding route is exported"));
            assert_eq!(
                forwarding["state"], "active",
                "case {name:?}: a route that forwards is active, got {forwarding}"
            );
            assert_eq!(
                forwarding["transitGatewayAttachments"][0]["transitGatewayAttachmentId"],
                Value::String(fixture.attachment_id.clone()),
                "case {name:?}: the route names the attachment it forwards to, got {forwarding}"
            );
            assert_eq!(
                forwarding["transitGatewayAttachments"][0]["resourceId"], ATTACHED_VPC,
                "case {name:?}: the route names the attachment's own resource, got {forwarding}"
            );
            assert_eq!(
                forwarding["transitGatewayAttachments"][0]["resourceType"], "vpc",
                "case {name:?}: the route names that resource's type, got {forwarding}"
            );
        }

        // A route that drops the traffic it matches forwards to nothing, so it names no
        // attachment at all.
        if case.expected.contains(&"10.0.1.0/24") {
            let blackhole = document["routes"]
                .as_array()
                .and_then(|routes| {
                    routes
                        .iter()
                        .find(|route| route["destinationCidrBlock"] == "10.0.1.0/24")
                })
                .unwrap_or_else(|| panic!("case {name:?}: the blackhole route is exported"));
            assert_eq!(
                blackhole["state"], "blackhole",
                "case {name:?}: a route that drops its traffic is a blackhole, got {blackhole}"
            );
            assert_eq!(
                blackhole["transitGatewayAttachments"],
                Value::Null,
                "case {name:?}: a blackhole route names no attachment, got {blackhole}"
            );
        }
    }
}

/// An export the object store will not take saves no file, so it is refused rather than
/// answered with a location nothing can be read at — and a dry run writes nothing either.
#[tokio::test]
async fn an_export_that_saves_no_file_answers_no_location() {
    let (router, engine) = emulator();
    given_bucket(&router).await;
    let fixture = given_routes(&router, &engine).await;

    let case = "a bucket the object store does not hold";
    let before = listed_keys(&router, case).await;
    let (status, body) = ec2_call(
        &router,
        "ExportTransitGatewayRoutes",
        &export_request(&fixture.route_table_id, ABSENT_BUCKET, &[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: an export that cannot save its file is refused, got {status} with {body}"
    );
    assert_eq!(
        element(&body, "Code"),
        "InvalidRequest",
        "case {case:?}: the refusal names the request it could not carry out, got {body}"
    );
    assert!(
        body.contains(ABSENT_BUCKET),
        "case {case:?}: the refusal names the bucket it is about, got {body}"
    );
    assert!(
        !body.contains("s3Location"),
        "case {case:?}: a refused export answers no location, got {body}"
    );
    assert_eq!(
        listed_keys(&router, case).await,
        before,
        "case {case:?}: a refused export writes nothing into any bucket"
    );

    let case = "a dry run";
    let mut params = export_request(&fixture.route_table_id, BUCKET, &[]);
    params.push(param("DryRun", "true"));
    let (status, body) = ec2_call(&router, "ExportTransitGatewayRoutes", &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: a dry run that would have succeeded is refused, got {status} with {body}"
    );
    assert_eq!(
        element(&body, "Code"),
        "DryRunOperation",
        "case {case:?}: the refusal is the dry-run verdict, got {body}"
    );
    assert_eq!(
        listed_keys(&router, case).await,
        before,
        "case {case:?}: a dry run writes no file"
    );
}
