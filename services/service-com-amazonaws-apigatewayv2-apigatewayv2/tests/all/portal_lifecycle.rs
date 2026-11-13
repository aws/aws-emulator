// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Portal` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreatePortal` persists a portal and answers 201 with that STORED record** —
//!   the `PortalId` and `PortalArn` it minted, the endpoint configuration it resolved,
//!   and every member the request supplied — and `GetPortal` reports the same record,
//!   member for member. A portal is created UNPUBLISHED (`DISABLED`, the one terminal
//!   not-serving value of `PublishStatus`), so it reports no `LastPublished` until
//!   something publishes it.
//! * **`Authorization` and `EndpointConfiguration` are tagged unions**: a request that
//!   names none of a union's alternatives, or more than one, is refused 400
//!   `BadRequestException` and persists nothing.
//! * **A value in an ARN member that is not an ARN is refused 400**.
//! * **A value the SERVICE computes is not taken from the request**:
//!   `PortalTheme$LogoLastUploaded` reports the instant this call uploaded a logo, so it
//!   is set when the request carries a `LogoUri` and cleared when it does not, whatever
//!   the request itself put there.
//! * **An account holds at most `MAX_PORTALS` portals**, after which a create is refused
//!   429 `TooManyRequestsException`.
//! * **`GetPortal` on an id nothing minted is 404 `NotFoundException`**.
//!
//! The rows vary what each rule generalises over: which union alternative the request
//! names (and how many), which optional members it carries (none, some, all), the number
//! of included portal-product ARNs (absent, empty, one, several), and the region the
//! request is signed for — the region being what the generated domain name and hosted
//! zone id are derived from.

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, emulator, error_code, instant,
};

/// The region [`call`] signs for, mirrored here because the generated endpoint values
/// are derived from the request's region.
const REGION: &str = "eu-west-1";

/// A second region, so "derived from the region" is asserted over more than one value.
const OTHER_REGION: &str = "ap-southeast-2";

/// The emulator's canonical account id, the account segment of every ARN it emits.
const ACCOUNT_ID: &str = "000000000000";

/// A `CustomColors` value: every member is required
/// (`smithy_query(query="shape", shape="CustomColors")`).
pub(crate) fn custom_colors() -> Value {
    json!({
        "accentColor": "#ff9900",
        "backgroundColor": "#ffffff",
        "errorValidationColor": "#d13212",
        "headerColor": "#232f3e",
        "navigationColor": "#16191f",
        "textColor": "#111111"
    })
}

/// A minimal `PortalContent`: `DisplayName` and `Theme` are required, `Description` is
/// not.
pub(crate) fn portal_content(display_name: &str) -> Value {
    json!({ "displayName": display_name, "theme": { "customColors": custom_colors() } })
}

/// A portal-product ARN of the form `CreatePortalProduct` mints: this service's ARN
/// namespace and the `/portalproducts/{id}` resource path.
///
/// Used where a row needs an ARN whose FORM is right and whose product the account does not
/// hold; a row whose portal is accepted names products created through
/// [`hold_included_products`], since a portal may only include products the account has.
fn product_arn(id: &str) -> String {
    format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portalproducts/{id}")
}

/// Create one portal product per `includedPortalProductArns` entry `request` carries, and
/// rewrite that member to name the products those calls MINTED; the ARNs now in the request
/// are answered, for a caller that asserts on what the portal reports back.
///
/// A portal includes the products a published portal renders, so a write may only name
/// products the account HOLDS: the ARNs a row is written with are placeholders for the ones
/// `CreatePortalProduct` mints here. A request carrying no such member, or an empty list, is
/// left exactly as it was.
pub(crate) async fn hold_included_products(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    label: &str,
    request: &mut Value,
) -> Vec<Value> {
    let placeholders = request
        .get("includedPortalProductArns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut held: Vec<Value> = Vec::with_capacity(placeholders.len());
    for index in 0..placeholders.len() {
        let (status, created) = call(
            emulator,
            "POST",
            "/v2/portalproducts",
            Some(json!({ "displayName": format!("{label} product {index}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreatePortalProduct for the product the \
             portal includes, got {status} {created}"
        );
        held.push(Value::String(as_str(&created, "portalProductArn")));
    }
    if request.get("includedPortalProductArns").is_some() {
        request["includedPortalProductArns"] = Value::Array(held.clone());
    }
    held
}

/// One accepted-create row: the request, and what the stored portal must report.
struct Row {
    case: &'static str,
    request: Value,
    /// Members the response must report with exactly these values.
    expected: Vec<(&'static str, Value)>,
    /// Members no reader may report for this row.
    absent: Vec<&'static str>,
    /// The domain the account brought, when the row names one.
    own_domain: Option<(&'static str, &'static str)>,
    /// Whether the row's theme must carry a `LogoLastUploaded`.
    expect_logo_uploaded: bool,
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "no authorization and API Gateway's default domain: only the required members",
            request: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Public Portal")
            }),
            expected: vec![
                ("publishStatus", json!("DISABLED")),
                // Nothing was included, so the list is reported empty rather than
                // invented.
                ("includedPortalProductArns", json!([])),
            ],
            absent: vec![
                "lastPublished",
                "lastPublishedDescription",
                "statusException",
                "rumAppMonitorName",
            ],
            own_domain: Option::None,
            expect_logo_uploaded: false,
        },
        Row {
            case: "a Cognito-authorized portal with every optional member set",
            request: json!({
                "authorization": {
                    "cognitoConfig": {
                        "appClientId": "1h57kf5cpq17m0eml12eqemjq1",
                        "userPoolArn": format!("arn:aws:cognito-idp:{REGION}:{ACCOUNT_ID}:userpool/{REGION}_pooL1"),
                        "userPoolDomain": "https://portal-users.auth.eu-west-1.amazoncognito.com"
                    }
                },
                "endpointConfiguration": {
                    "acmManaged": {
                        "certificateArn": format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"),
                        "domainName": "developer.example.com"
                    }
                },
                "portalContent": {
                    "displayName": "Example Developer Portal",
                    "description": "every optional member set",
                    "theme": { "customColors": custom_colors() }
                },
                "includedPortalProductArns": [product_arn("prodaaaaaa"), product_arn("prodbbbbbb")],
                "logoUri": "s3://example-portal-assets/logo.png",
                "rumAppMonitorName": "example-portal-monitor",
                "tags": { "team": "developer-experience", "env": "test" }
            }),
            expected: vec![
                ("publishStatus", json!("DISABLED")),
                ("rumAppMonitorName", json!("example-portal-monitor")),
                (
                    "includedPortalProductArns",
                    json!([product_arn("prodaaaaaa"), product_arn("prodbbbbbb")]),
                ),
                (
                    "tags",
                    json!({ "team": "developer-experience", "env": "test" }),
                ),
            ],
            absent: vec!["lastPublished", "lastPublishedDescription", "statusException"],
            own_domain: Some((
                "developer.example.com",
                "arn:aws:acm:eu-west-1:000000000000:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f",
            )),
            expect_logo_uploaded: true,
        },
        Row {
            case: "an empty included-products list, which includes nothing",
            request: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Empty Inclusions"),
                "includedPortalProductArns": [],
                "tags": {}
            }),
            expected: vec![
                ("includedPortalProductArns", json!([])),
                ("tags", json!({})),
            ],
            absent: vec!["lastPublished"],
            own_domain: Option::None,
            expect_logo_uploaded: false,
        },
        Row {
            case: "one included product, and a logo but no RUM monitor",
            request: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("One Product"),
                "includedPortalProductArns": [product_arn("prodcccccc")],
                "logoUri": "https://assets.example.com/logo.svg"
            }),
            expected: vec![
                ("includedPortalProductArns", json!([product_arn("prodcccccc")])),
            ],
            absent: vec!["rumAppMonitorName", "lastPublished"],
            own_domain: Option::None,
            expect_logo_uploaded: true,
        },
        Row {
            case: "a Cognito-authorized portal on the default domain",
            request: json!({
                "authorization": {
                    "cognitoConfig": {
                        "appClientId": "7bd0s2k9rr1cv9m0lq3ucaf9bk",
                        "userPoolArn": format!("arn:aws:cognito-idp:{REGION}:{ACCOUNT_ID}:userpool/{REGION}_pooL2"),
                        "userPoolDomain": "https://internal-portal.auth.eu-west-1.amazoncognito.com"
                    }
                },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Internal Portal")
            }),
            expected: vec![("publishStatus", json!("DISABLED"))],
            absent: vec!["lastPublished"],
            own_domain: Option::None,
            expect_logo_uploaded: false,
        },
        Row {
            case: "a theme claiming a logo upload the request never made",
            request: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                // No `logoUri`, so no logo was uploaded by this call: the instant the
                // request filled in is the SERVICE's to report, and is cleared.
                "portalContent": {
                    "displayName": "Claimed Upload",
                    "theme": {
                        "customColors": custom_colors(),
                        // Epoch seconds: the vendored schema generator emits no
                        // `@timestampFormat`, so restJson1's default numeric form is what
                        // this member deserializes from (see `instant` in
                        // `api_lifecycle`).
                        "logoLastUploaded": 1_577_934_245_i64
                    }
                }
            }),
            expected: vec![("publishStatus", json!("DISABLED"))],
            absent: vec!["lastPublished"],
            own_domain: Option::None,
            expect_logo_uploaded: false,
        },
    ]
}

/// The endpoint configuration a response reports, as an object.
fn endpoint_configuration(case: &str, body: &Value) -> Value {
    body.get("endpointConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the response must report an endpointConfiguration, got {body}"))
}

#[tokio::test]
async fn create_portal_persists_the_record_and_get_portal_reports_it_back() {
    let emulator = emulator();
    let mut minted: Vec<String> = Vec::new();

    for mut row in accepted_rows() {
        let case = row.case;
        // A portal may only include products the account HOLDS, so the products this row
        // includes are created first and the row is pointed at them.
        let held = hold_included_products(&emulator, case, &mut row.request).await;
        for (member, value) in &mut row.expected {
            if *member == "includedPortalProductArns" && !held.is_empty() {
                *value = Value::Array(held.clone());
            }
        }
        let (status, created) =
            call(&emulator, "POST", "/v2/portals", Some(row.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );

        let portal_id = as_str(&created, "portalId");
        assert_minted_id(case, "PortalId", &portal_id);
        minted.push(portal_id.clone());

        // The ARN names this portal, in this request's region, under the emulator's
        // canonical account.
        assert_eq!(
            as_str(&created, "portalArn"),
            format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portals/{portal_id}"),
            "case {case:?}: expected the portal's own ARN, got {created}"
        );

        // `LastModified` is the instant this call stored the record.
        let last_modified = instant(case, &created, "lastModified");

        // The resolved endpoint configuration: the generated default domain name always,
        // the account's own domain and certificate only when the request brought one.
        let endpoint = endpoint_configuration(case, &created);
        let default_domain = as_str(&endpoint, "portalDefaultDomainName");
        assert!(
            default_domain.starts_with(&format!("{portal_id}."))
                && default_domain.contains(REGION),
            "case {case:?}: expected a default domain name derived from the portal id and \
             region, got {default_domain:?}"
        );
        let hosted_zone = as_str(&endpoint, "portalDomainHostedZoneId");
        assert!(
            hosted_zone.starts_with('Z')
                && hosted_zone.len() == 14
                && hosted_zone[1..]
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "case {case:?}: expected a hosted zone id of the published form (Z + 13 \
             uppercase alphanumerics), got {hosted_zone:?}"
        );
        match row.own_domain {
            Some((domain, certificate)) => {
                assert_eq!(
                    endpoint.get("domainName"),
                    Some(&json!(domain)),
                    "case {case:?}: expected the account's own domain name, got {endpoint}"
                );
                assert_eq!(
                    endpoint.get("certificateArn"),
                    Some(&json!(certificate)),
                    "case {case:?}: expected the account's own certificate ARN, got {endpoint}"
                );
            }
            Option::None => {
                for member in ["domainName", "certificateArn"] {
                    assert!(
                        endpoint.get(member).is_none_or(Value::is_null),
                        "case {case:?}: a portal on the default domain reports no {member}, \
                         got {endpoint}"
                    );
                }
            }
        }

        // The authorization and content the request named, reported back verbatim.
        assert_eq!(
            created.get("authorization"),
            row.request.get("authorization"),
            "case {case:?}: expected the authorization the request named, got {created}"
        );
        assert_eq!(
            created.get("portalContent").and_then(|c| c.get("displayName")),
            row.request
                .get("portalContent")
                .and_then(|c| c.get("displayName")),
            "case {case:?}: expected the display name the request named, got {created}"
        );
        assert_eq!(
            created.get("portalContent").and_then(|c| c.get("description")),
            row.request
                .get("portalContent")
                .and_then(|c| c.get("description")),
            "case {case:?}: expected the description the request named, got {created}"
        );

        // A logo the request supplied was uploaded by this call, which is what
        // `PortalTheme$LogoLastUploaded` reports; a request carrying none uploaded
        // nothing.
        let theme = created
            .get("portalContent")
            .and_then(|content| content.get("theme"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            theme.get("customColors"),
            Some(&custom_colors()),
            "case {case:?}: expected the custom colors the request named, got {theme}"
        );
        if row.expect_logo_uploaded {
            let uploaded = instant(case, &theme, "logoLastUploaded");
            assert!(
                (uploaded - last_modified).abs() <= 2.0,
                "case {case:?}: expected the logo upload instant to be this call's, got \
                 {uploaded} against {last_modified}"
            );
        } else {
            assert!(
                theme.get("logoLastUploaded").is_none_or(Value::is_null),
                "case {case:?}: a request with no LogoUri uploaded no logo, got {theme}"
            );
        }

        for (member, value) in &row.expected {
            assert_eq!(
                created.get(member),
                Some(value),
                "case {case:?}: expected CreatePortal to report {member}={value}, got {created}"
            );
        }
        for member in &row.absent {
            assert!(
                created.get(member).is_none_or(Value::is_null),
                "case {case:?}: expected CreatePortal to report no {member}, got {created}"
            );
        }

        // The same record, read back.
        let (status, fetched) =
            call(&emulator, "GET", &format!("/v2/portals/{portal_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {fetched}"
        );
        for member in [
            "portalId",
            "portalArn",
            "authorization",
            "endpointConfiguration",
            "includedPortalProductArns",
            "portalContent",
            "publishStatus",
            "rumAppMonitorName",
            "tags",
            "lastModified",
        ] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {case:?}: expected GetPortal to report the same {member} CreatePortal \
                 did, got {fetched} after {created}"
            );
        }
        for member in &row.absent {
            assert!(
                fetched.get(member).is_none_or(Value::is_null),
                "case {case:?}: expected GetPortal to report no {member}, got {fetched}"
            );
        }
        // Nothing has previewed this portal, so the preview it reports carries nothing —
        // the member itself is always there, as an empty structure, so a client reading
        // `portal.preview` to decide whether a preview exists is never handed a null.
        assert_eq!(
            fetched.get("preview"),
            Some(&json!({})),
            "case {case:?}: expected the empty preview on a portal nothing previewed, got \
             {fetched}"
        );

        // Reading the portal changed nothing a later caller sees.
        let (status, again) =
            call(&emulator, "GET", &format!("/v2/portals/{portal_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the second GetPortal, got {status} {again}"
        );
        assert_eq!(
            again, fetched,
            "case {case:?}: expected a read to leave the portal as it was, got {again} after \
             {fetched}"
        );
    }

    // Every create minted a distinct id.
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "portal {index}: expected distinct ids, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn the_generated_endpoint_values_follow_the_requests_region() {
    let emulator = emulator();
    let request = json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content("Regional Portal")
    });

    let mut by_region: Vec<(&str, String, String)> = Vec::new();
    for region in [REGION, OTHER_REGION, REGION] {
        let (status, created) = call_in_region(
            &emulator,
            region,
            "POST",
            "/v2/portals",
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "region {region:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        assert_eq!(
            as_str(&created, "portalArn"),
            format!("arn:aws:apigateway:{region}:{ACCOUNT_ID}:/portals/{portal_id}"),
            "region {region:?}: expected the ARN to name the request's region, got {created}"
        );
        let endpoint = endpoint_configuration(region, &created);
        let domain = as_str(&endpoint, "portalDefaultDomainName");
        assert!(
            domain.contains(region),
            "region {region:?}: expected the default domain name to name the request's \
             region, got {domain:?}"
        );
        by_region.push((
            region,
            domain,
            as_str(&endpoint, "portalDomainHostedZoneId"),
        ));
    }

    // One hosted zone id per region: the same on every read of the same region, and a
    // different one for a different region.
    assert_eq!(
        by_region[0].2, by_region[2].2,
        "expected one hosted zone id per region, got {:?} and {:?}",
        by_region[0], by_region[2]
    );
    assert_ne!(
        by_region[0].2, by_region[1].2,
        "expected different regions to report different hosted zone ids, got {:?} and {:?}",
        by_region[0], by_region[1]
    );
    // The default domain name is the PORTAL's: two portals never share one.
    assert_ne!(
        by_region[0].1, by_region[2].1,
        "expected a default domain name unique to each portal, got {:?} and {:?}",
        by_region[0], by_region[2]
    );
}

/// The rows of the refusal table: a request, and why it names no single alternative.
fn refused_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an authorization naming none of its alternatives",
            json!({
                "authorization": {},
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("No Authorization")
            }),
        ),
        (
            "an authorization naming both of its alternatives",
            json!({
                "authorization": {
                    "none": {},
                    "cognitoConfig": {
                        "appClientId": "1h57kf5cpq17m0eml12eqemjq1",
                        "userPoolArn": format!("arn:aws:cognito-idp:{REGION}:{ACCOUNT_ID}:userpool/{REGION}_pooL3"),
                        "userPoolDomain": "https://both.auth.eu-west-1.amazoncognito.com"
                    }
                },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Both Authorizations")
            }),
        ),
        (
            "an endpoint configuration naming none of its alternatives",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": {},
                "portalContent": portal_content("No Endpoint")
            }),
        ),
        (
            "an endpoint configuration naming both of its alternatives",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": {
                    "none": {},
                    "acmManaged": {
                        "certificateArn": format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/8f4c1b1e"),
                        "domainName": "both.example.com"
                    }
                },
                "portalContent": portal_content("Both Endpoints")
            }),
        ),
        (
            "an included-products value that is not an ARN at all",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Not An Arn"),
                "includedPortalProductArns": ["this-value-is-no-arn-but-is-long-enough"]
            }),
        ),
        (
            "an included-products ARN missing its trailing fields",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Truncated Arn"),
                "includedPortalProductArns": ["arn:aws:apigateway:eu-west-1"]
            }),
        ),
        (
            "one good ARN and one that is not an ARN",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Mixed Arns"),
                "includedPortalProductArns": [
                    product_arn("proddddddd"),
                    "arn-without-the-colon-separators-at-all"
                ]
            }),
        ),
        (
            // `service_common::arn::parse_of` reads the vendor field: an ARN of another
            // service names nothing this service could include.
            "a well-formed ARN of another service's namespace",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Other Vendor Arn"),
                "includedPortalProductArns": [
                    format!("arn:aws:s3:::portal-products-bucket/{}", "product")
                ]
            }),
        ),
        (
            "an ARN with an empty vendor field",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Empty Vendor Arn"),
                "includedPortalProductArns": [
                    format!("arn:aws::{REGION}:{ACCOUNT_ID}:/portalproducts/prodeeeeee")
                ]
            }),
        ),
        (
            // Every resource of this service shares one ARN namespace, so the RESOURCE
            // path is what says which resource type an ARN names: an api is not a
            // portal product.
            "an ARN of this service naming another resource type",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Api Arn"),
                "includedPortalProductArns": [
                    format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/apis/abcdefghij")
                ]
            }),
        ),
        (
            // The resource TYPE with no id after it names no resource.
            "an ARN carrying the resource type but no product id",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("No Product Id"),
                "includedPortalProductArns": [
                    format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portalproducts/")
                ]
            }),
        ),
    ]
}

#[tokio::test]
async fn a_portal_request_that_names_no_single_alternative_or_no_arn_is_refused() {
    let emulator = emulator();

    for (case, request) in refused_rows() {
        let (status, body) = call(&emulator, "POST", "/v2/portals", Some(request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreatePortal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // Every create above was refused, so the account persisted nothing AND minted
    // nothing: the first portal this emulator accepts carries the FIRST id of the id
    // bijection, which is the id a fresh emulator's first portal carries too.
    let accepted = json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content("After The Refusals")
    });
    let (status, created) =
        call(&emulator, "POST", "/v2/portals", Some(accepted.clone())).await;
    assert_eq!(
        status, 201,
        "expected 201 for a well-formed portal after the refusals, got {status} {created}"
    );
    let after_refusals = as_str(&created, "portalId");

    let fresh = crate::api_lifecycle::emulator();
    let (status, first) = call(&fresh, "POST", "/v2/portals", Some(accepted)).await;
    assert_eq!(
        status, 201,
        "expected 201 for the fresh emulator's first portal, got {status} {first}"
    );
    assert_eq!(
        after_refusals,
        as_str(&first, "portalId"),
        "expected the refused creates to have minted nothing, so the first accepted portal \
         carries the same id a fresh emulator's first portal does, got {after_refusals:?} and \
         {first}"
    );
}

#[tokio::test]
async fn a_portal_id_nothing_minted_is_not_found() {
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("The Only Portal")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortal, got {status} {created}"
    );
    let portal_id = as_str(&created, "portalId");

    for (case, id) in [
        ("an id nothing minted", "nosuchprtl".to_string()),
        ("the portal's id with a character changed", {
            let mut other = portal_id.clone();
            other.pop();
            format!("{other}z0")
        }),
    ] {
        let (status, body) = call(&emulator, "GET", &format!("/v2/portals/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetPortal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_account_holds_at_most_the_portals_its_quota_allows() {
    // The bound the service enforces, read from the service itself rather than copied:
    // the create past it is refused with the modelled `TooManyRequestsException` ("a
    // limit has been exceeded").
    const LIMIT: usize = service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_PORTALS;

    let emulator = emulator();
    for index in 0..LIMIT {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content(&format!("Portal {index}"))
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "portal {index} of {LIMIT}: expected 201 from CreatePortal, got {status} {created}"
        );
    }

    let (status, body) = call(
        &emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("One Too Many")
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 from the portal past the limit of {LIMIT}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );
}
