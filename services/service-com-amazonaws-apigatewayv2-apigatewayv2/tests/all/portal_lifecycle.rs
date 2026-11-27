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
//! * **The published default domain is a DOMAIN NAME this service generates**:
//!   `EndpointConfigurationResponse$PortalDefaultDomainName` is the host a consumer fetches
//!   the portal at, so it carries the portal's own labels and nothing else — no scheme, no
//!   `:port` and no path — minted in the zone this emulator answers in rather than one under
//!   API Gateway's own, which nothing on the machine this emulator runs on resolves. It is
//!   settled once, when the portal is created, so every portal-reading operation reports the
//!   same value whichever endpoint the caller reached this service at, and the portal's
//!   published page is served there.
//!
//! The rows vary what each rule generalises over: which union alternative the request
//! names (and how many), which optional members it carries (none, some, all), the number
//! of included portal-product ARNs (absent, empty, one, several), the region the
//! request is signed for — the region being what the hosted zone id is derived from — and,
//! for the domain rule, the endpoint the calls are addressed at (this service's own AWS
//! host, an IP literal with a port and without one, a loopback name with a port).

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_at_host, call_in_region, emulator, error_code, instant,
};

/// The region [`call`] signs for, mirrored here because the hosted zone the endpoint
/// configuration reports is derived from the request's region.
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

        // The resolved endpoint configuration: the name the portal's site is published
        // under always, the account's own domain and certificate only when the request
        // brought one.
        let endpoint = endpoint_configuration(case, &created);
        let default_domain = as_str(&endpoint, "portalDefaultDomainName");
        assert_published_domain_name(case, &default_domain, &portal_id);
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
        // The generated default domain name carries the portal's own id and the Region the
        // portal lives in, so a portal of another Region is named in that Region; the hosted
        // zone the Region decides is asserted below.
        assert_published_domain_name(region, &domain, &portal_id);
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

/// The modelled bound of the member under test: `PortalDefaultDomainName` targets
/// `__stringMin3Max256` (`smithy_query(query="shape", shape="EndpointConfigurationResponse")`).
const DOMAIN_NAME_LENGTH: (usize, usize) = (3, 256);

/// The reserved name of the loopback interface this emulator's listeners bind, which every
/// resolver answers with that interface
/// (`service_common::data_plane_addressing::LOOPBACK_ZONE`).
pub(crate) const ANNOUNCED_LOOPBACK_NAME: &str = "localhost";

/// The labels AWS generates a portal's default domain name from, around the portal's own id
/// and the Region it lives in: `p-{portalId}.apigw-portal.{region}.on.aws` — the form this
/// emulator can announce for no portal, because nothing on the machine it runs on resolves a
/// name of that shape.
pub(crate) const PORTAL_DOMAIN_ID_PREFIX: &str = "p-";
pub(crate) const PORTAL_DOMAIN_LABEL: &str = "apigw-portal";

/// The reserved path segments a portal's site is served behind on an address of this
/// emulator's own: `/_aws/portal/{portal-id}`.
const PORTAL_SITE_PATH_PREFIX: &str = "/_aws/portal/";

/// Assert `value` is the address this emulator SERVES the portal `portal_id`'s site at, which
/// is what the portal publishes as its default domain.
///
/// `PortalDefaultDomainName` is "the portal default domain name. This domain name is
/// generated and managed by API Gateway", typed `__stringMin3Max256`, and what a client does
/// with the value is FETCH it — so it has to name an address this emulator is reached at.
/// That is one of two forms: the `host:port` of the socket the portal was given, whose host is
/// a loopback address the core bound (the address every client of this machine dials), or this
/// emulator's own endpoint followed by the portal's own reserved site path, for a portal that
/// got no socket of its own. A name in AWS's own zone, or one this emulator mints in a zone of
/// its own, is neither: no resolver a client of this machine has answers for it, so announcing
/// one announces an address nothing reaches.
///
/// The Region the portal lives in is no part of any address of this process, so this takes
/// none: a caller that wants it in its own failure message states it there.
pub(crate) fn assert_published_domain_name(case: &str, value: &str, portal_id: &str) {
    let (min, max) = DOMAIN_NAME_LENGTH;
    let length = value.chars().count();
    assert!(
        (min..=max).contains(&length),
        "case {case:?}: expected a value of {min}..{max} characters, got {length} in {value:?}"
    );
    assert!(
        !value.contains("://") && !value.contains(' ') && !value.contains('@'),
        "case {case:?}: expected an address of this emulator with no scheme or userinfo, got \
         {value:?} (portal {portal_id})"
    );
    let (authority, path) = match value.find('/') {
        Some(at) => (&value[..at], &value[at..]),
        None => (value, ""),
    };
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host)
        .trim_matches(|character| character == '[' || character == ']');
    // The defect this pins: the GENERATED name AWS answers with — the portal's own id and the
    // endpoint label, in whatever zone — is a name nothing on this machine resolves, so it is
    // never the address this emulator announces.
    assert!(
        !host.contains(&format!(
            "{PORTAL_DOMAIN_ID_PREFIX}{portal_id}.{PORTAL_DOMAIN_LABEL}"
        )),
        "case {case:?}: expected an address this emulator is reached at, got the generated \
         domain name {value:?} (portal {portal_id})"
    );
    assert!(
        host.parse::<std::net::IpAddr>().is_ok()
            || host.eq_ignore_ascii_case(ANNOUNCED_LOOPBACK_NAME),
        "case {case:?}: expected the published site to be announced on an address a client of \
         this machine dials, got the host {host:?} in {value:?} (portal {portal_id})"
    );
    // Either the portal's own socket, which names a port and carries no path, or this
    // emulator's own endpoint with the portal's reserved site path behind it.
    if path.is_empty() {
        assert!(
            authority
                .rsplit_once(':')
                .is_some_and(|(_host, port)| port.parse::<u16>().is_ok_and(|port| port != 0)),
            "case {case:?}: a portal published on a socket of its own must name the port it \
             landed on, got {value:?} (portal {portal_id})"
        );
    } else {
        assert_eq!(
            path,
            format!("{PORTAL_SITE_PATH_PREFIX}{portal_id}"),
            "case {case:?}: a portal published on this emulator's own endpoint must name its \
             own reserved site path, got {value:?}"
        );
    }
}

/// The default domain name a portal body reports.
fn default_domain_name(case: &str, body: &Value) -> String {
    as_str(&endpoint_configuration(case, body), "portalDefaultDomainName")
}

/// A domain the account owns with the certificate it is served under: the other
/// alternative of `EndpointConfigurationRequest`, whose `AcmManaged` members are both
/// required.
fn brought_domain() -> Value {
    json!({
        "acmManaged": {
            "domainName": "developers.example.com",
            "certificateArn": format!(
                "arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/\
                 11111111-2222-3333-4444-555555555555"
            )
        }
    })
}

/// One row of the domain-name table: where the calls are addressed, the Region they are
/// signed for, and the domain configuration the portal is created with.
struct DomainRow {
    case: &'static str,
    /// The `Host` the calls carry — the endpoint this client reached the emulator at.
    host: &'static str,
    region: &'static str,
    /// The request's `endpointConfiguration`.
    endpoint_configuration: fn() -> Value,
    /// The domain this row's request brings, when it brings one.
    own_domain: Option<&'static str>,
}

fn domain_rows() -> Vec<DomainRow> {
    vec![
        DomainRow {
            case: "this service's own AWS host, API Gateway's default domain",
            host: "apigateway.eu-west-1.amazonaws.com",
            region: REGION,
            endpoint_configuration: || json!({ "none": {} }),
            own_domain: None,
        },
        DomainRow {
            case: "the emulator's own endpoint as an IP literal with a port",
            host: "127.0.0.1:44539",
            region: REGION,
            endpoint_configuration: || json!({ "none": {} }),
            own_domain: None,
        },
        DomainRow {
            case: "the emulator's own endpoint as an IP literal with no port",
            host: "127.0.0.1",
            region: OTHER_REGION,
            endpoint_configuration: || json!({ "none": {} }),
            own_domain: None,
        },
        DomainRow {
            case: "a loopback name with a port, and a domain the account brought",
            host: "localhost:9297",
            region: "us-west-2",
            endpoint_configuration: brought_domain,
            own_domain: Some("developers.example.com"),
        },
        DomainRow {
            case: "this service's AWS host in a third Region, and a brought domain",
            host: "apigateway.eu-central-1.amazonaws.com",
            region: "eu-central-1",
            endpoint_configuration: brought_domain,
            own_domain: Some("developers.example.com"),
        },
    ]
}

/// Create one portal for `row` and answer `(portal id, the created body)`.
async fn portal_of_row(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    row: &DomainRow,
) -> (String, Value) {
    let (status, created) = call_at_host(
        emulator,
        row.host,
        row.region,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": (row.endpoint_configuration)(),
            "portalContent": portal_content("Domain Name Portal")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreatePortal, got {status} {created}",
        row.case
    );
    (as_str(&created, "portalId"), created)
}

/// `EndpointConfigurationResponse$PortalDefaultDomainName` is the domain name this emulator
/// GENERATES for the portal — the portal's own labels in the zone this emulator answers in,
/// which its portal data plane claims host-style — whichever endpoint the caller reached this
/// service at, and the same value on every operation that reports the portal.
#[tokio::test]
async fn every_portal_reader_reports_the_domain_name_generated_for_it() {
    for row in domain_rows() {
        let case = row.case;
        let emulator = emulator();
        let (portal_id, created) = portal_of_row(&emulator, &row).await;

        // Settled once, when the portal was created: the endpoint a later caller reaches
        // this emulator at never moves it.
        let expected = default_domain_name(case, &created);
        assert_published_domain_name(
            &format!("{case}: CreatePortal"),
            &expected,
            &portal_id,
        );

        // A domain the account brought is reported beside it and displaces nothing.
        assert_eq!(
            endpoint_configuration(case, &created)
                .get("domainName")
                .and_then(Value::as_str)
                .map(str::to_string),
            row.own_domain.map(str::to_string),
            "case {case:?}: expected the domain the request brought, got {created}"
        );

        let (status, read) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {read}"
        );
        let read_domain = default_domain_name(case, &read);
        assert_eq!(
            read_domain, expected,
            "case {case:?}: expected the same published address from GetPortal, got {read}"
        );
        assert_published_domain_name(
            &format!("{case}: GetPortal"),
            &read_domain,
            &portal_id,
        );

        let (status, patched) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "PATCH",
            &format!("/v2/portals/{portal_id}"),
            Some(json!({
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Renamed Domain Name Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdatePortal, got {status} {patched}"
        );
        let patched_domain = default_domain_name(case, &patched);
        assert_eq!(
            patched_domain, expected,
            "case {case:?}: expected the same published address from UpdatePortal, got \
             {patched}"
        );
        assert_published_domain_name(
            &format!("{case}: UpdatePortal"),
            &patched_domain,
            &portal_id,
        );

        let (status, listed) =
            call_at_host(&emulator, row.host, row.region, "GET", "/v2/portals", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListPortals, got {status} {listed}"
        );
        let summary = listed
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| as_str(item, "portalId") == portal_id)
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListPortals must report the portal, got {listed}")
            });
        let listed_domain = default_domain_name(case, &summary);
        assert_eq!(
            listed_domain, expected,
            "case {case:?}: expected the same published address in the portal's summary, \
             got {listed}"
        );
        assert_published_domain_name(
            &format!("{case}: ListPortals"),
            &listed_domain,
            &portal_id,
        );
    }
}

/// One portal, one name: portals of the same account are published under different domain
/// names, however the client that created them addressed this emulator.
#[tokio::test]
async fn each_portal_is_published_at_an_address_of_its_own() {
    let emulator = emulator();
    let mut held: Vec<(String, String)> = Vec::new();
    for row in domain_rows() {
        let (portal_id, created) = portal_of_row(&emulator, &row).await;
        let domain = default_domain_name(row.case, &created);
        assert_published_domain_name(row.case, &domain, &portal_id);
        assert!(
            !held.iter().any(|(_, seen)| seen == &domain),
            "case {:?}: the published address {domain:?} is already held, by {held:?}",
            row.case
        );
        held.push((portal_id, domain));
    }
    assert_eq!(
        held.len(),
        domain_rows().len(),
        "expected one published address per portal, got {held:?}"
    );
}

/// The published default domain is an address this emulator ANSWERS on, not decoration: a
/// portal created through the emulator's own endpoint — a client that resolves no AWS host —
/// is served its published page at the address it was told about, and on that same endpoint
/// with the portal behind the reserved path marker.
#[tokio::test]
async fn the_published_default_domain_serves_the_portals_published_page() {
    let case = "a portal created through the emulator's own endpoint";
    let local_endpoint = "127.0.0.1:44539";
    let emulator = emulator();
    let (portal_id, created) = portal_of_row(
        &emulator,
        &DomainRow {
            case,
            host: local_endpoint,
            region: REGION,
            endpoint_configuration: || json!({ "none": {} }),
            own_domain: None,
        },
    )
    .await;
    let domain = default_domain_name(case, &created);
    assert_published_domain_name(case, &domain, &portal_id);

    let (status, published) = call_at_host(
        &emulator,
        local_endpoint,
        REGION,
        "POST",
        &format!("/v2/portals/{portal_id}/publish"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from PublishPortal, got {status} {published}"
    );

    // Both addressings of the one portal: the published address as the host, and the
    // emulator's own endpoint with the portal behind the reserved path marker.
    let addressings = [
        ("the published default domain", domain, "/".to_string()),
        (
            "the emulator's own endpoint with the portal in the path",
            local_endpoint.to_string(),
            format!("/_aws/portal/{portal_id}/"),
        ),
    ];
    for (addressing, host, path) in addressings {
        // The wait is for the CONDITION — the publish having settled and the page being
        // served — and never for a fixed time; the poll is the data-plane suite's own.
        let page = crate::portal_data_plane::fetch_path_until_served(
            &emulator,
            &format!("{case}: {addressing}"),
            &host,
            &path,
        )
        .await;
        crate::portal_data_plane::served(&format!("{case}: {addressing}"), &page, &portal_id);
    }
}
