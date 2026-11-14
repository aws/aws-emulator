// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence EC2 refuses a value that is no local-gateway-route-table id with.
//!
//! The error CLASS is one — `InvalidLocalGatewayRouteTableID.Malformed` at HTTP 400, before
//! anything is looked up — and the sentence that carries it is chosen by WHO refuses it:
//!
//! * a value that is no identifier at all never reaches the service that owns the local
//!   gateway, so every operation answers it at the request boundary, in the boundary's own
//!   words for this resource kind — `The local-gateway-route-table ID '<id>' is malformed`;
//! * a value SHAPED like an EC2 resource id — a prefix, a `-`, and a suffix of lowercase
//!   hexadecimal digits — but none EC2 could have issued for this resource is handed on, so a
//!   create of a resource BELONGING to the table answers it in the owning service's words,
//!   `Invalid LocalGateway Route Table id <id>`, with the id unquoted and last.
//!
//! The operations ON the table itself keep the boundary's sentence for either form, so the
//! rows below drive both forms through both groups: the two creates that take this id as the
//! table their new resource belongs to, and three readers and writers of the table itself.
//!
//! The rows differ along what the rule generalises over — the FORM of the value (this
//! resource's prefix with a suffix of neither published length, another local gateway
//! resource's prefix, the prefix without its separator, an upper-case suffix, a
//! non-hexadecimal suffix, the prefix alone, a value with no separator at all, and text that
//! is no id at all), the OPERATION that answers it, and the member's form field (a scalar and
//! a list entry) — so no single row is the specification. Two control rows pin that the shape
//! rule decides only the MALFORMED class: a well-formed id the account holds no table for is
//! still the resource's single `.NotFound` sentence, through both creates.

use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED, INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
};

use super::ec2_query::{call, emulator, error_code, error_message};

/// The Region every call here is signed for. No row depends on it.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers every refusal below with.
const REFUSAL_STATUS: u16 = 400;

/// One operation under test: its `Action`, the form field it reads this id from, whether it
/// CREATES a resource belonging to the table (which is what hands a value shaped like an id to
/// the service that owns the local gateway), and the other members its request must carry for
/// the route table id to be the only thing left to decide.
struct Caller {
    action: &'static str,
    field: &'static str,
    creates_in_table: bool,
    /// The form fields beside the id: a route is named by the destination it matches and the
    /// target it sends traffic to, both read before the table is resolved, so a row that must
    /// reach the lookup carries them.
    beside: &'static [(&'static str, &'static str)],
}

/// The operations that read a `LocalGatewayRouteTableId`: both creates of a resource belonging
/// to the table, and three operations about the table itself — the search and the describe
/// that read one, and the delete that takes one down — so each sentence is driven through more
/// than one caller and through both a scalar member and a list entry.
const CALLERS: &[Caller] = &[
    Caller {
        action: "CreateCoipPool",
        field: "LocalGatewayRouteTableId",
        creates_in_table: true,
        beside: &[],
    },
    Caller {
        action: "CreateLocalGatewayRoute",
        field: "LocalGatewayRouteTableId",
        creates_in_table: true,
        beside: &[
            ("DestinationCidrBlock", "192.0.2.0/24"),
            (
                "LocalGatewayVirtualInterfaceGroupId",
                "lgw-vif-grp-0123456789abcdef0",
            ),
        ],
    },
    Caller {
        action: "SearchLocalGatewayRoutes",
        field: "LocalGatewayRouteTableId",
        creates_in_table: false,
        beside: &[],
    },
    Caller {
        action: "DeleteLocalGatewayRouteTable",
        field: "LocalGatewayRouteTableId",
        creates_in_table: false,
        beside: &[],
    },
    Caller {
        action: "DescribeLocalGatewayRouteTables",
        field: "LocalGatewayRouteTableId.1",
        creates_in_table: false,
        beside: &[],
    },
];

/// The request `caller` sends for the route table id `id`: the id in that caller's own form
/// field, with whatever the caller needs beside it.
fn params(caller: &Caller, id: &str) -> Vec<(String, String)> {
    let mut params = vec![(caller.field.to_string(), id.to_string())];
    params.extend(
        caller
            .beside
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    params
}

/// The values no local gateway route table id could be, and whether each is SHAPED like an EC2
/// resource id at all.
///
/// `lgw-rtb-<8 or 17 lowercase hex digits>` is the form EC2 issues
/// (`LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX`), so every value here is outside it; the second
/// column is the shape question the sentence turns on, written out per row rather than
/// recomputed, so the table says which branch it drives.
const VALUES: &[(&str, bool)] = &[
    // This resource's own prefix with a suffix of neither published length.
    ("lgw-rtb-0000000000000000", true),
    ("lgw-rtb-0a1b2c3", true),
    // Another local gateway resource's prefix, and this prefix without its separator.
    ("lgw-00000000000000001", true),
    ("lgwrtb-00000000000000000", true),
    ("lgw-vif-grp-0123456789abcdef0", true),
    // An upper-case suffix and a non-hexadecimal one: neither is a suffix EC2 mints, and
    // neither reads as an identifier.
    ("lgw-rtb-0A1B2C3D", false),
    ("lgw-rtb-zzzzzzzzzzzzzzzzz", false),
    // The prefix alone, a value with no separator at all, and text that is no id at all.
    ("lgw-rtb-", false),
    ("notanidatall", false),
    ("totally-not-an-id", false),
    ("not-a-route-table-id", false),
];

/// Both published id lengths for a route table the account holds none of: well formed, so the
/// control rows below are the resource's `.NotFound` rather than this suite's class.
const ABSENT_IDS: &[&str] = &["lgw-rtb-0123456789abcdef0", "lgw-rtb-0a1b2c3d"];

/// The request boundary's sentence for this resource kind, which every operation answers a
/// value that is no identifier at all with.
fn boundary_sentence(id: &str) -> String {
    format!("The local-gateway-route-table ID '{id}' is malformed")
}

/// The owning service's sentence, which a create of a resource belonging to the table answers a
/// value shaped like a resource id with: a fixed prefix then the id, unquoted and last.
fn owning_service_sentence(id: &str) -> String {
    format!("Invalid LocalGateway Route Table id {id}")
}

/// The sentence this resource answers a well-formed id the account holds no table for with.
fn not_found_sentence(id: &str) -> String {
    format!("The LocalGateway Route Table ID '{id}' does not exist")
}

/// RULE: a value outside the form EC2 issues for a local gateway route table is refused 400
/// `InvalidLocalGatewayRouteTableID.Malformed` by every operation that reads one, with the
/// owning service's sentence where a create of a resource belonging to the table was handed a
/// value shaped like a resource id, and the request boundary's sentence everywhere else.
#[tokio::test]
async fn a_malformed_route_table_id_carries_the_sentence_of_whoever_refused_it() {
    let (router, _engine) = emulator();

    for caller in CALLERS {
        for (value, shaped) in VALUES {
            let case = format!("{} given {value:?}", caller.action);
            let (status, body) = call(&router, REGION, caller.action, &params(caller, value)).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
            );
            let answered_code = error_code(&body);
            assert_eq!(
                answered_code, INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
                "case {case:?}: expected the code \
                 {INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED:?}, got {answered_code:?} in \
                 {body}"
            );
            let expected = if caller.creates_in_table && *shaped {
                owning_service_sentence(value)
            } else {
                boundary_sentence(value)
            };
            let answered = error_message(&body);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the message {expected:?}, got {answered:?}"
            );
        }
    }
}

/// RULE: the form of the value decides only the MALFORMED class — an id EC2 could have issued
/// that names no route table of the account is still refused `.NotFound`, with this resource's
/// single not-found sentence, through the creates that word a malformed id their own way.
#[tokio::test]
async fn a_well_formed_but_unheld_route_table_id_keeps_the_not_found_sentence() {
    let (router, _engine) = emulator();

    for caller in CALLERS.iter().filter(|caller| caller.creates_in_table) {
        for absent in ABSENT_IDS {
            let case = format!("{} given the unheld {absent:?}", caller.action);
            let (status, body) =
                call(&router, REGION, caller.action, &params(caller, absent)).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
            );
            let answered_code = error_code(&body);
            assert_eq!(
                answered_code, INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
                "case {case:?}: expected the code \
                 {INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND:?}, got {answered_code:?} in \
                 {body}"
            );
            let expected = not_found_sentence(absent);
            let answered = error_message(&body);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the message {expected:?}, got {answered:?}"
            );
        }
    }
}
