// SPDX-License-Identifier: CC0-1.0

//! What `ExportTransitGatewayRoutes` refuses, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an export is a READ of the transit gateway route table the request
//! names, written into the Amazon S3 bucket it names — so every fact of the request is
//! settled before anything is written. A parameter the request did not really carry is
//! `MissingParameter`; a bucket name outside Amazon S3's naming rules names no bucket at all;
//! an id that is not a transit gateway route table id is malformed before any table is looked
//! up, and a well-formed id the account holds no table for is not found; a filter name THIS
//! operation's own `Filters` doc does not list is refused whether or not a route would have
//! matched it, and a `route-search.*` value that is no CIDR block relates to nothing; and a
//! dry run of a request that would have succeeded writes nothing.
//!
//! Each row is a distinct input along what that rule generalises over: which member is at
//! fault and whether it was left out altogether or carried empty, which class of bad bucket
//! name it is, whether the route table id is unreadable or merely unheld, and whether the
//! refused filter name is one no operation documents or one only the SIBLING operation
//! (`SearchTransitGatewayRoutes`) documents.
//!
//! The export's own write, and the file a caller reads back out of the bucket, need an object
//! store to write into, which a single-service router has none of: that half of the rule is
//! `core/router/tests/all/ec2_transit_gateway_route_export.rs`.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The ec2Query `Action` under test.
const ACTION: &str = "ExportTransitGatewayRoutes";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// A bucket name inside Amazon S3's general-purpose naming rules, so a case that is about
/// another member is never refused for this one.
const BUCKET: &str = "transit-gateway-route-exports";

/// One call of [`ACTION`] signed for the Region above.
async fn invoke(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// A transit gateway created through the wire, as the id of the default ASSOCIATION route
/// table the create owes it ("we create a default transit gateway route table and use it as
/// the default association route table" — `CreateTransitGateway`).
async fn route_table(router: &MultiServiceRouter) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "setup: CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has an association route table")
}

/// Which route table id a row names.
#[derive(Debug, Clone, Copy)]
enum Table {
    /// The one the account really holds.
    Held,
    /// The literal value given, which the account holds no table for.
    Named(&'static str),
    /// No value at all — the empty string a `clientOptional` `@required` member arrives as.
    None,
}

/// One refusal, as a row of the table.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The route table the request names.
    table: Table,
    /// The `S3Bucket` the request names; `None` names none at all.
    bucket: Option<&'static str>,
    /// The `Filters` the request carries.
    narrow_by: &'static [(&'static str, &'static [&'static str])],
    /// Whether the request is a dry run.
    dry_run: bool,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The EC2 error code the refusal carries.
    code: &'static str,
    /// Text the refusal's message must carry, so a row pins the sentence and not only the code.
    message: &'static str,
}

/// Every member of the request is settled before anything is written: a refused export
/// answers the fault of the member at issue, and never a 501 or a success.
#[tokio::test]
async fn an_export_refuses_every_fault_of_its_own_request() {
    let cases: &[Case] = &[
        Case {
            // A `@required` member the request carries no field of at all arrives null, which
            // the request boundary refuses in its own wording before the handler runs.
            name: "no route table id at all",
            table: Table::None,
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "MissingParameter",
            message: "The request must include the TransitGatewayRouteTableId parameter.",
        },
        Case {
            // The same member carried EMPTY reaches the handler as the empty string: a
            // parameter the request did not really carry, which the handler names.
            name: "an empty route table id",
            table: Table::Named(""),
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter transitGatewayRouteTableId",
        },
        Case {
            name: "no bucket at all",
            table: Table::Held,
            bucket: None,
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "MissingParameter",
            message: "The request must include the S3Bucket parameter.",
        },
        Case {
            name: "an empty bucket",
            table: Table::Held,
            bucket: Some(""),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter s3Bucket",
        },
        Case {
            name: "a bucket named with characters no bucket name carries",
            table: Table::Held,
            bucket: Some("Not_A_Bucket"),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "InvalidRequest",
            message: "The S3 bucket name contains invalid characters.",
        },
        Case {
            name: "a bucket name under the published length floor",
            table: Table::Held,
            bucket: Some("ab"),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "InvalidRequest",
            message: "The S3 bucket name contains invalid characters.",
        },
        Case {
            name: "a bucket name written as an IP address, which S3 holds none under",
            table: Table::Held,
            bucket: Some("192.168.5.4"),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "InvalidRequest",
            message: "The S3 bucket name contains invalid characters.",
        },
        Case {
            name: "something that is not a route table id at all",
            table: Table::Named("not-a-route-table"),
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "InvalidRouteTableId.Malformed",
            message: "Invalid Transit Gateway Route Table id not-a-route-table.",
        },
        Case {
            name: "a well-formed route table id the account holds no table for",
            table: Table::Named("tgw-rtb-0123456789abcdef0"),
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: false,
            status: 400,
            code: "InvalidRouteTableID.NotFound",
            message: "Transit Gateway Route Table tgw-rtb-0123456789abcdef0 was deleted or does \
                      not exist.",
        },
        Case {
            name: "a filter name no operation documents",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[("route-colour", &["blue"])],
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
            message: "Value (route-colour) for parameter Filter is invalid.",
        },
        Case {
            // The sibling search documents this name; this operation's own `Filters` doc does
            // not, so the set is per OPERATION rather than per resource.
            name: "a filter name only the sibling search documents",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[("prefix-list-id", &["pl-0123456789abcdef0"])],
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
            message: "Value (prefix-list-id) for parameter Filter is invalid.",
        },
        Case {
            name: "the other filter name only the sibling search documents",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[("attachment.resource-type", &["vpc"])],
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
            message: "Value (attachment.resource-type) for parameter Filter is invalid.",
        },
        Case {
            name: "a CIDR search whose value is no address prefix",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[("route-search.exact-match", &["ten-dot-oh"])],
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
            message: "This is not a valid CIDR block.",
        },
        Case {
            name: "a subnet-of search whose value is an address with no prefix length",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[("route-search.subnet-of-match", &["10.0.0.0"])],
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
            message: "This is not a valid CIDR block.",
        },
        Case {
            name: "a dry run of a request that would have succeeded",
            table: Table::Held,
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: true,
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set",
        },
        Case {
            // The flag asks whether the request WOULD have succeeded, so one that would have
            // been refused is refused with that reason instead.
            name: "a dry run of a request naming a route table the account does not hold",
            table: Table::Named("tgw-rtb-00000000000000aaa"),
            bucket: Some(BUCKET),
            narrow_by: &[],
            dry_run: true,
            status: 400,
            code: "InvalidRouteTableID.NotFound",
            message: "Transit Gateway Route Table tgw-rtb-00000000000000aaa was deleted or does \
                      not exist.",
        },
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let held = route_table(&router).await;

    for case in cases {
        let name = case.name;
        let mut params: Vec<(String, String)> = Vec::new();
        match case.table {
            Table::Held => params.push(param("TransitGatewayRouteTableId", &held)),
            Table::Named(id) => params.push(param("TransitGatewayRouteTableId", id)),
            Table::None => {}
        }
        if let Some(bucket) = case.bucket {
            params.push(param("S3Bucket", bucket));
        }
        params.extend(filters(case.narrow_by));
        if case.dry_run {
            params.push(param("DryRun", "true"));
        }

        let (status, body) = invoke(&router, &params).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected HTTP {}, got {status} with body {body}",
            case.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, case.code,
            "case {name:?}: expected the code {:?}, got {code:?} with body {body}",
            case.code
        );
        let message = error_message(&body);
        assert!(
            message.contains(case.message),
            "case {name:?}: expected a message carrying {:?}, got {message:?}",
            case.message
        );
    }
}
