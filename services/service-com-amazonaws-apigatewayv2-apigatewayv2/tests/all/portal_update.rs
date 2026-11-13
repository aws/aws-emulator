// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdatePortal`.
//!
//! The rules these cases assert:
//!
//! * **`UpdatePortal` is a PATCH** (`PATCH /v2/portals/{PortalId}`): a member the request
//!   carries replaces the stored one, a member it omits leaves the stored value exactly as
//!   it was, and the members this operation does not take (the portal's id, its ARN, its
//!   publication state and its tags) are carried over untouched. The patched record is
//!   what the response reports AND what a later `GetPortal` reports, so the patch was
//!   persisted rather than echoed.
//! * **`LastModified` advances when the patch carries a member to apply**, and a patch
//!   that carries none leaves the record — `LastModified` included — exactly as it was.
//! * **A value the SERVICE computes is not taken from the request**:
//!   `PortalTheme$LogoLastUploaded` reports the instant a logo was last uploaded, so a
//!   patch carrying a `LogoUri` reports its own instant and a patch carrying none keeps
//!   the instant of the upload there WAS — whatever the request itself put there, and
//!   even when the patch rewrites the theme around it. The generated
//!   `PortalDefaultDomainName` and `PortalDomainHostedZoneId` are the same kind of value:
//!   derived from the portal's id and the Region it was created in, they survive a patch
//!   signed for another Region.
//! * **A patch is refused 409 `ConflictException` while a change to the portal's
//!   publication is still in flight** — the service is publishing or removing the
//!   configuration the patch would replace — and the refusal lifts of its own accord once
//!   that build comes due. A preview in flight changes nothing about what the portal
//!   publishes, so it does not block a patch.
//! * **A portal is patched only through its own minted id**: an id nothing minted is 404
//!   `NotFoundException` and writes nothing.
//! * **A patch whose values are invalid is refused 400 and persists nothing**:
//!   `Authorization` and `EndpointConfiguration` are tagged unions, so one naming none of
//!   its alternatives or naming several is refused, as is a value in
//!   `IncludedPortalProductArns` that is not a portal-product ARN, or a member past its
//!   modelled length bound.
//!
//! The rows vary what the rules generalise over: WHICH members the patch carries (each
//! one alone, all of them, none), which alternative of each union it names, the number of
//! included portal-product ARNs (none, one, several), value length at and past each
//! published bound, the script the values are written in, whether the member was set
//! before the patch, which build is in flight when the patch arrives, and the Region the
//! patch is signed for. The recorded portal lifecycle that filed the issue — create,
//! patch, read back — is ONE row of the first table.

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    PORTAL_BUILD, PUBLISH_STATUS_DISABLED, PUBLISH_STATUS_DISABLE_IN_PROGRESS,
    PUBLISH_STATUS_PUBLISHED, PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
};

use crate::api_lifecycle::{
    as_str, call, call_in_region, emulator, error_code, instant, past_the_whole_second_of,
    strip_nulls,
};
use crate::portal_lifecycle::{custom_colors, hold_included_products, portal_content};

/// The portals collection URI (`CreatePortal`'s modelled `POST /v2/portals`); one portal
/// hangs off it as `{COLLECTION}/{id}`, the URI both `GetPortal` and `UpdatePortal` are
/// bound to.
const COLLECTION: &str = "/v2/portals";

/// The region [`call`] signs for, mirrored here because the generated endpoint values are
/// derived from the Region the portal was created in.
const REGION: &str = "eu-west-1";

/// A second region, so "generated from the Region the portal was created in" is asserted
/// against a patch signed for another one.
const OTHER_REGION: &str = "ap-southeast-2";

/// The members `UpdatePortal` takes that a reader reports back verbatim.
/// `EndpointConfiguration` is NOT one of them: its request and response shapes differ
/// (`EndpointConfigurationRequest` / `EndpointConfigurationResponse`), so it is asserted
/// on its own.
const PATCHABLE: [&str; 4] = [
    "authorization",
    "includedPortalProductArns",
    "portalContent",
    "rumAppMonitorName",
];

/// How long to wait so a patch lands in a LATER second than what came before it. The
/// service reports its instants at whole-second granularity, so only a wait past a second
/// boundary makes a move observable on the wire. A BLOCKING wait on purpose, for the
/// reason given in `portal_product_update`: this workspace's `tokio` carries no `time`
/// feature.
const SECOND_BOUNDARY_WAIT: Duration = Duration::from_millis(1_100);

/// How far a reported instant may sit from this test's own clock reading, in seconds.
const INSTANT_SLACK_SECONDS: f64 = 60.0;

/// The longest `RumAppMonitorName` the modelled `__stringMin0Max255` takes (len 0..255).
const MAX_RUM_APP_MONITOR_NAME_LEN: usize = 255;

/// The longest `LogoUri` the modelled `__stringMin0Max1092` takes (len 0..1092).
const MAX_LOGO_URI_LEN: usize = 1092;

/// A portal-product ARN of the form `CreatePortalProduct` mints: this service's ARN
/// namespace and the `/portalproducts/{id}` resource path.
fn product_arn(id: &str) -> String {
    format!(
        "arn:aws:apigateway:{REGION}:{}:/portalproducts/{id}",
        ::engine::emulator_account_id()
    )
}

/// A Cognito authorization: every member of `CognitoConfig` is required
/// (`smithy_query(query="shape", shape="CognitoConfig")`).
fn cognito_authorization(app_client_id: &str) -> Value {
    json!({
        "cognitoConfig": {
            "appClientId": app_client_id,
            "userPoolArn": format!(
                "arn:aws:cognito-idp:{REGION}:{}:userpool/{REGION}_pooL1",
                ::engine::emulator_account_id()
            ),
            "userPoolDomain": "https://portal-users.auth.eu-west-1.amazoncognito.com"
        }
    })
}

/// An `AcmManaged` endpoint configuration: the account's own domain and the ACM
/// certificate for it, both required members of `ACMManaged`.
fn own_domain(domain: &str) -> Value {
    json!({
        "acmManaged": {
            "domainName": domain,
            "certificateArn": format!(
                "arn:aws:acm:{REGION}:{}:certificate/11111111-2222-3333-4444-555555555555",
                ::engine::emulator_account_id()
            )
        }
    })
}

/// A `PortalContent` carrying a description as well as the required display name and
/// theme.
fn described_content(display_name: &str, description: &str) -> Value {
    json!({
        "displayName": display_name,
        "description": description,
        "theme": { "customColors": custom_colors() }
    })
}

/// One patch row: the portal to create, an optional patch applied BEFORE the one under
/// test (so a member that was already set is covered too), and the patch under test.
struct Row {
    case: &'static str,
    create: Value,
    earlier_patch: Option<Value>,
    patch: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the portal content alone, leaving the authorization and tags as they were",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Developer Portal"),
                "tags": { "team": "platform", "env": "test" }
            }),
            earlier_patch: None,
            patch: json!({
                "portalContent": described_content("Developer Portal v2", "now with a description")
            }),
        },
        Row {
            case: "the authorization alone, from no authentication to Cognito",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Opening Up")
            }),
            earlier_patch: None,
            patch: json!({ "authorization": cognito_authorization("1h57kf5cpq17m0eml12eqemjq1") }),
        },
        Row {
            case: "the authorization alone, from Cognito back to no authentication",
            create: json!({
                "authorization": cognito_authorization("2abcdef3ghijk4lmnop5qrstu6"),
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Closing Down")
            }),
            earlier_patch: None,
            patch: json!({ "authorization": { "none": {} } }),
        },
        Row {
            case: "the endpoint configuration alone, bringing the account's own domain",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Own Domain")
            }),
            earlier_patch: None,
            patch: json!({ "endpointConfiguration": own_domain("portal.example.com") }),
        },
        Row {
            case: "the endpoint configuration alone, back to API Gateway's default domain",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": own_domain("portal.example.net"),
                "portalContent": portal_content("Default Domain")
            }),
            earlier_patch: None,
            patch: json!({ "endpointConfiguration": { "none": {} } }),
        },
        Row {
            case: "one included portal-product ARN onto a portal that included none",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("One Product")
            }),
            earlier_patch: None,
            patch: json!({ "includedPortalProductArns": [product_arn("prod000001")] }),
        },
        Row {
            case: "several included portal-product ARNs replacing one",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "includedPortalProductArns": [product_arn("prod000002")],
                "portalContent": portal_content("Many Products")
            }),
            earlier_patch: None,
            patch: json!({
                "includedPortalProductArns": [
                    product_arn("prod000003"),
                    product_arn("prod000004"),
                    product_arn("prod000005")
                ]
            }),
        },
        Row {
            case: "the included portal-product ARNs emptied",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "includedPortalProductArns": [product_arn("prod000006")],
                "portalContent": portal_content("No Products")
            }),
            earlier_patch: None,
            patch: json!({ "includedPortalProductArns": [] }),
        },
        Row {
            case: "the RUM app monitor name alone onto a portal that had none",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Monitored")
            }),
            earlier_patch: None,
            patch: json!({ "rumAppMonitorName": "portal-rum" }),
        },
        Row {
            case: "the RUM app monitor name at the modelled maximum length",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "rumAppMonitorName": "short",
                "portalContent": portal_content("Maximal Monitor")
            }),
            earlier_patch: None,
            patch: json!({ "rumAppMonitorName": "m".repeat(MAX_RUM_APP_MONITOR_NAME_LEN) }),
        },
        Row {
            case: "the RUM app monitor name emptied, which the modelled bound allows (len 0..255)",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "rumAppMonitorName": "portal-rum",
                "portalContent": portal_content("Emptied Monitor")
            }),
            earlier_patch: None,
            patch: json!({ "rumAppMonitorName": "" }),
        },
        Row {
            case: "every patchable member at once",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("All At Once"),
                "tags": { "owner": "api-team" }
            }),
            earlier_patch: None,
            patch: json!({
                "authorization": cognito_authorization("7vwxyz8abcde9fghij0klmnop"),
                "endpointConfiguration": own_domain("all.example.com"),
                "includedPortalProductArns": [product_arn("prod000007"), product_arn("prod000008")],
                "logoUri": "s3://example-portal-assets/logo.png",
                "portalContent": described_content("All At Once, Patched", "everything moved"),
                "rumAppMonitorName": "all-at-once-rum"
            }),
        },
        Row {
            case: "values outside ASCII, which round-trip unchanged",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Unicode")
            }),
            earlier_patch: None,
            patch: json!({
                "portalContent": described_content(
                    "Zahlungs-Portal 決済",
                    "決済 API — ünïcödé, emoji 🎉, and a combining é"
                )
            }),
        },
        Row {
            case: "a second patch over the members an earlier one set",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Twice"),
                "tags": { "owner": "api-team" }
            }),
            earlier_patch: Some(json!({
                "rumAppMonitorName": "set-by-the-first-patch",
                "includedPortalProductArns": [product_arn("prod000009")]
            })),
            patch: json!({ "portalContent": described_content("Twice, Patched", "second patch") }),
        },
        Row {
            case: "the values the portal already carries, repeated",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "rumAppMonitorName": "idempotent-rum",
                "portalContent": portal_content("Idempotent")
            }),
            earlier_patch: None,
            patch: json!({
                "authorization": { "none": {} },
                "rumAppMonitorName": "idempotent-rum",
                "portalContent": portal_content("Idempotent")
            }),
        },
        Row {
            case: "a logo URI at the modelled maximum length",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Long Logo")
            }),
            earlier_patch: None,
            patch: json!({ "logoUri": long_logo_uri() }),
        },
    ]
}

/// A `LogoUri` exactly at the modelled maximum: a URI whose path is padded out to
/// [`MAX_LOGO_URI_LEN`] characters in all.
fn long_logo_uri() -> String {
    let prefix = "s3://example-portal-assets/";
    format!(
        "{prefix}{}",
        "l".repeat(MAX_LOGO_URI_LEN - prefix.chars().count())
    )
}

/// One member of a document, with an absent member read as `null` — so "absent" and
/// "reported as null" are not asserted as two different things. The normalizer is the
/// crate-wide one in `api_lifecycle`, shared with every other suite.
fn member(document: &Value, name: &str) -> Value {
    strip_nulls(document.get(name).unwrap_or(&Value::Null))
}

/// `document`'s `portalContent` without the theme's `LogoLastUploaded`: that member is
/// the SERVICE's to report (it is asserted on its own), so comparing the content a
/// request carried with the content a reader reports has to leave it out.
fn content_without_logo_instant(document: &Value) -> Value {
    let mut content = member(document, "portalContent");
    if let Some(theme) = content.get_mut("theme").and_then(Value::as_object_mut) {
        theme.remove("logoLastUploaded");
    }
    content
}

/// The theme a reader reports for a portal.
fn theme_of(case: &str, body: &Value) -> Value {
    body.get("portalContent")
        .and_then(|content| content.get("theme"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the response must report a theme, got {body}"))
}

/// The endpoint configuration a reader reports for a portal.
fn endpoint_of(case: &str, body: &Value) -> Value {
    body.get("endpointConfiguration").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the response must report an endpointConfiguration, got {body}")
    })
}

/// The account's own domain and certificate a document reports, or `None` when it reports
/// a portal on API Gateway's default domain.
fn brought_domain(endpoint: &Value) -> Option<(String, String)> {
    let domain = endpoint.get("domainName").and_then(Value::as_str)?;
    Some((
        domain.to_string(),
        as_str(endpoint, "certificateArn"),
    ))
}

/// This test's clock reading, in epoch seconds.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// Create one portal and return its id.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", COLLECTION, Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    as_str(&created, "portalId")
}

/// Read one portal back, asserting the read succeeded.
async fn get_portal(emulator: &MultiServiceRouter, case: &str, portal_id: &str) -> Value {
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("{COLLECTION}/{portal_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetPortal, got {status} {fetched}"
    );
    fetched
}

/// Patch one portal, asserting the patch was accepted.
async fn patch_portal(
    emulator: &MultiServiceRouter,
    case: &str,
    portal_id: &str,
    patch: Value,
) -> Value {
    let (status, patched) = call(
        emulator,
        "PATCH",
        &format!("{COLLECTION}/{portal_id}"),
        Some(patch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdatePortal, got {status} {patched}"
    );
    patched
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_leaves_the_others_as_they_were() {
    let emulator = emulator();

    for mut row in rows() {
        let case = row.case;
        // A portal may only include products the account HOLDS, so every product the row's
        // create and its patches name is created first and the row is pointed at them.
        let _held = hold_included_products(&emulator, case, &mut row.create).await;
        if let Some(earlier) = row.earlier_patch.as_mut() {
            let _held = hold_included_products(&emulator, case, earlier).await;
        }
        let _held = hold_included_products(&emulator, case, &mut row.patch).await;
        let portal_id = create_portal(&emulator, case, &row.create).await;

        if let Some(earlier) = &row.earlier_patch {
            patch_portal(&emulator, case, &portal_id, earlier.clone()).await;
        }

        // The record as it stands before the patch under test.
        let before = get_portal(&emulator, case, &portal_id).await;
        let patched = patch_portal(&emulator, case, &portal_id, row.patch.clone()).await;

        // Every member the request reports back verbatim: the value the patch carried,
        // else the stored one.
        for name in PATCHABLE {
            let (expected, actual) = if name == "portalContent" {
                let source = if row.patch.get(name).is_some() {
                    &row.patch
                } else {
                    &before
                };
                (
                    content_without_logo_instant(source),
                    content_without_logo_instant(&patched),
                )
            } else {
                let expected = if row.patch.get(name).is_some() {
                    member(&row.patch, name)
                } else {
                    member(&before, name)
                };
                (expected, member(&patched, name))
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected UpdatePortal to report {name}={expected}, got {patched}"
            );
        }

        // The endpoint configuration: which domain the ACCOUNT brought is the patch's to
        // move, while the generated default domain name and hosted zone id are the
        // portal's own and survive the patch.
        let before_endpoint = endpoint_of(case, &before);
        let patched_endpoint = endpoint_of(case, &patched);
        let expected_domain = match row.patch.get("endpointConfiguration") {
            Some(requested) => requested.get("acmManaged").map(|managed| {
                (
                    as_str(managed, "domainName"),
                    as_str(managed, "certificateArn"),
                )
            }),
            None => brought_domain(&before_endpoint),
        };
        assert_eq!(
            brought_domain(&patched_endpoint),
            expected_domain,
            "case {case:?}: expected the domain the account brought to be {expected_domain:?}, \
             got {patched_endpoint}"
        );
        for generated in ["portalDefaultDomainName", "portalDomainHostedZoneId"] {
            assert_eq!(
                as_str(&patched_endpoint, generated),
                as_str(&before_endpoint, generated),
                "case {case:?}: expected the generated {generated} to survive the patch, got \
                 {patched_endpoint} against {before_endpoint}"
            );
        }

        // The members this operation does not take are carried over untouched.
        for carried in ["portalId", "portalArn", "publishStatus"] {
            assert_eq!(
                as_str(&patched, carried),
                as_str(&before, carried),
                "case {case:?}: expected the stored {carried} to survive the patch, got {patched}"
            );
        }
        assert_eq!(
            member(&patched, "tags"),
            member(&before, "tags"),
            "case {case:?}: expected the stored tags to survive the patch, got {patched}"
        );

        // What the response reports is what a later reader reports: the patch was
        // persisted, not echoed.
        let fetched = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            strip_nulls(&fetched),
            strip_nulls(&patched),
            "case {case:?}: expected GetPortal to report the patched record, got {fetched} \
             against {patched}"
        );

        // The patch carried a member, so the record's modification instant reads this
        // call's own clock rather than a fixed literal.
        let now = now_seconds();
        let reported = instant(case, &fetched, "lastModified");
        assert!(
            (now - reported).abs() <= INSTANT_SLACK_SECONDS,
            "case {case:?}: expected LastModified to read the patch instant; got {reported} \
             against a clock reading {now}"
        );
    }
}

#[tokio::test]
async fn the_logo_upload_instant_is_the_services_not_the_requests() {
    let emulator = emulator();

    /// A theme claiming an upload that never happened. Epoch seconds: the vendored schema
    /// generator emits no `@timestampFormat`, so restJson1's default numeric form is what
    /// this member deserializes from (see `instant` in `api_lifecycle`).
    const CLAIMED_UPLOAD: i64 = 1_577_934_245;

    fn claiming_content(display_name: &str) -> Value {
        json!({
            "displayName": display_name,
            "theme": { "customColors": custom_colors(), "logoLastUploaded": CLAIMED_UPLOAD }
        })
    }

    // (case, the create, the patch, whether this patch uploaded a logo)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "a patch carrying a logo onto a portal that never had one",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("First Logo")
            }),
            json!({ "logoUri": "s3://example-portal-assets/first.png" }),
            true,
        ),
        (
            "a patch replacing the logo a portal already had",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "logoUri": "s3://example-portal-assets/old.png",
                "portalContent": portal_content("Replaced Logo")
            }),
            json!({ "logoUri": "https://assets.example.com/new.svg" }),
            true,
        ),
        (
            "a patch rewriting the theme and claiming an upload it never made",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "logoUri": "s3://example-portal-assets/kept.png",
                "portalContent": portal_content("Kept Logo")
            }),
            json!({ "portalContent": claiming_content("Kept Logo, Patched") }),
            false,
        ),
        (
            "a patch claiming an upload onto a portal that never had a logo",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("No Logo")
            }),
            json!({ "portalContent": claiming_content("No Logo, Patched") }),
            false,
        ),
    ];

    for (case, create, patch, uploads_a_logo) in rows {
        let portal_id = create_portal(&emulator, case, &create).await;
        let before = get_portal(&emulator, case, &portal_id).await;
        let stored_upload = theme_of(case, &before)
            .get("logoLastUploaded")
            .filter(|value| !value.is_null())
            .map(|_| instant(case, &theme_of(case, &before), "logoLastUploaded"));

        // Past a second boundary, so an instant that moved is visible at the whole-second
        // granularity the member is reported at.
        sleep(SECOND_BOUNDARY_WAIT);

        let patched = patch_portal(&emulator, case, &portal_id, patch).await;
        let theme = theme_of(case, &patched);
        assert_eq!(
            theme.get("customColors"),
            Some(&custom_colors()),
            "case {case:?}: expected the custom colors to survive the patch, got {theme}"
        );

        if uploads_a_logo {
            let uploaded = instant(case, &theme, "logoLastUploaded");
            let now = now_seconds();
            assert!(
                (now - uploaded).abs() <= INSTANT_SLACK_SECONDS,
                "case {case:?}: expected the logo upload instant to be this call's; got \
                 {uploaded} against a clock reading {now}"
            );
            if let Some(stored) = stored_upload {
                assert!(
                    uploaded > stored,
                    "case {case:?}: expected the logo upload instant to advance past the \
                     stored {stored}, got {uploaded}"
                );
            }
        } else {
            match stored_upload {
                Some(stored) => {
                    let reported = instant(case, &theme, "logoLastUploaded");
                    assert_eq!(
                        reported, stored,
                        "case {case:?}: a patch that uploaded nothing keeps the instant of the \
                         upload there was ({stored}), got {reported}"
                    );
                }
                None => assert!(
                    theme.get("logoLastUploaded").is_none_or(Value::is_null),
                    "case {case:?}: a portal no call uploaded a logo for reports none, got \
                     {theme}"
                ),
            }
        }

        // What the patch reported is what a later reader reports.
        let fetched = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            strip_nulls(&theme_of(case, &fetched)),
            strip_nulls(&theme),
            "case {case:?}: expected GetPortal to report the patched theme, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_carries_no_member_leaves_the_record_exactly_as_it_was() {
    let emulator = emulator();
    let case = "a patch carrying none of the portal's members";

    let mut create = json!({
        "authorization": { "none": {} },
        "endpointConfiguration": own_domain("untouched.example.com"),
        "includedPortalProductArns": [product_arn("prod000010")],
        "logoUri": "s3://example-portal-assets/untouched.png",
        "rumAppMonitorName": "untouched-rum",
        "portalContent": described_content("Untouched", "nothing in the patch applies"),
        "tags": { "team": "platform" }
    });
    let _held = hold_included_products(&emulator, case, &mut create).await;
    let portal_id = create_portal(&emulator, case, &create).await;
    let before = get_portal(&emulator, case, &portal_id).await;

    // The member is reported at whole-second granularity, so the patch has to land in a
    // LATER second for a move to be visible at all: the wait is for the clock to read one,
    // never for a fixed time.
    past_the_whole_second_of(case, instant(case, &before, "lastModified")).await;

    let patched = patch_portal(&emulator, case, &portal_id, json!({})).await;
    assert_eq!(
        strip_nulls(&patched),
        strip_nulls(&before),
        "case {case:?}: expected the record to be reported unchanged, got {patched} against \
         {before}"
    );

    let after = get_portal(&emulator, case, &portal_id).await;
    assert_eq!(
        strip_nulls(&after),
        strip_nulls(&before),
        "case {case:?}: expected the stored record to be unchanged, got {after} against {before}"
    );
}

#[tokio::test]
async fn a_patch_that_carries_a_member_advances_last_modified() {
    let emulator = emulator();

    // One row per member the patch can carry, so none of them is the only one that
    // records a modification.
    let patches = [
        ("the authorization", json!({ "authorization": { "none": {} } })),
        (
            "the endpoint configuration",
            json!({ "endpointConfiguration": own_domain("moved.example.com") }),
        ),
        (
            "the included portal-product ARNs",
            json!({ "includedPortalProductArns": [product_arn("prod000014")] }),
        ),
        (
            "the logo URI",
            json!({ "logoUri": "s3://example-portal-assets/moved.png" }),
        ),
        (
            "the portal content",
            json!({ "portalContent": described_content("Moved", "by the patch") }),
        ),
        ("the RUM app monitor name", json!({ "rumAppMonitorName": "moved-rum" })),
    ];

    for (case, mut patch) in patches {
        let _held = hold_included_products(&emulator, case, &mut patch).await;
        let portal_id = create_portal(
            &emulator,
            case,
            &json!({
                "authorization": cognito_authorization("3clockabcde4fghij5klmnop6"),
                "endpointConfiguration": { "none": {} },
                "rumAppMonitorName": "created-rum",
                "portalContent": portal_content("Clock")
            }),
        )
        .await;
        let created_at = instant(
            case,
            &get_portal(&emulator, case, &portal_id).await,
            "lastModified",
        );

        // Past a second boundary, so a move is visible at the whole-second granularity
        // the member is reported at.
        sleep(SECOND_BOUNDARY_WAIT);

        let patched = patch_portal(&emulator, case, &portal_id, patch).await;
        let patched_at = instant(case, &patched, "lastModified");
        assert!(
            patched_at > created_at,
            "case {case:?}: expected LastModified to advance past the creation instant \
             {created_at}, got {patched_at}"
        );

        let fetched = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            instant(case, &fetched, "lastModified"),
            patched_at,
            "case {case:?}: expected GetPortal to report the patched LastModified, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_patch_signed_for_another_region_keeps_the_generated_default_domain() {
    let emulator = emulator();
    let case = "a patch signed for a region the portal was not created in";

    let portal_id = create_portal(
        &emulator,
        case,
        &json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Cross Region")
        }),
    )
    .await;
    let before = endpoint_of(case, &get_portal(&emulator, case, &portal_id).await);

    let (status, patched) = call_in_region(
        &emulator,
        OTHER_REGION,
        "PATCH",
        &format!("{COLLECTION}/{portal_id}"),
        Some(json!({ "endpointConfiguration": own_domain("cross.example.com") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdatePortal, got {status} {patched}"
    );

    let endpoint = endpoint_of(case, &patched);
    assert_eq!(
        brought_domain(&endpoint),
        Some((
            "cross.example.com".to_string(),
            as_str(&own_domain("cross.example.com")["acmManaged"], "certificateArn")
        )),
        "case {case:?}: expected the domain the patch brought, got {endpoint}"
    );
    // Generated from the portal's id and the Region it was created in, so the Region the
    // patch was signed for does not move them.
    for generated in ["portalDefaultDomainName", "portalDomainHostedZoneId"] {
        assert_eq!(
            as_str(&endpoint, generated),
            as_str(&before, generated),
            "case {case:?}: expected the generated {generated} of the creating Region, got \
             {endpoint} against {before}"
        );
    }
    assert!(
        as_str(&endpoint, "portalDefaultDomainName").contains(REGION),
        "case {case:?}: expected the default domain name to name {REGION}, got {endpoint}"
    );
}

#[tokio::test]
async fn a_patch_is_refused_while_a_publication_change_is_in_flight() {
    // (case, the call that starts the build, its URI suffix and method, the status the
    // portal reports while it runs, whether that build blocks a patch)
    let builds: Vec<(&str, &str, &str, &str, bool)> = vec![
        (
            "a publish the service is still carrying out",
            "POST",
            "publish",
            PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
            true,
        ),
        (
            "a disable the service is still carrying out",
            "DELETE",
            "publish",
            PUBLISH_STATUS_DISABLE_IN_PROGRESS,
            true,
        ),
        (
            "a preview, which changes nothing about what the portal publishes",
            "POST",
            "preview",
            PUBLISH_STATUS_DISABLED,
            false,
        ),
    ];

    for (case, method, segment, in_flight_status, blocks) in builds {
        let emulator = emulator();
        let portal_id = create_portal(
            &emulator,
            case,
            &json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("In Flight")
            }),
        )
        .await;
        let uri = format!("{COLLECTION}/{portal_id}");

        // A disable only has a publication to take down once there IS one, so that row
        // publishes first and waits for the publish to come due.
        if method == "DELETE" {
            let (status, body) = call(&emulator, "POST", &format!("{uri}/publish"), None).await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
            );
            sleep(past_the_build());
        }

        let before = get_portal(&emulator, case, &portal_id).await;
        let (status, body) = call(&emulator, method, &format!("{uri}/{segment}"), None).await;
        assert!(
            status == 202 || status == 204,
            "case {case:?}: expected the build to be started, got {status} {body}"
        );
        assert_eq!(
            as_str(&get_portal(&emulator, case, &portal_id).await, "publishStatus"),
            in_flight_status,
            "case {case:?}: expected the portal to report {in_flight_status} while the build runs"
        );

        let patch = json!({ "portalContent": described_content("Patched", "while a build ran") });
        let (patch_status, patched) =
            call(&emulator, "PATCH", &uri, Some(patch.clone())).await;
        if blocks {
            assert_eq!(
                patch_status, 409,
                "case {case:?}: expected 409 from UpdatePortal, got {patch_status} {patched}"
            );
            assert_eq!(
                error_code(&patched),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {patched}"
            );
            // The refusal wrote nothing.
            let after = get_portal(&emulator, case, &portal_id).await;
            assert_eq!(
                content_without_logo_instant(&after),
                content_without_logo_instant(&before),
                "case {case:?}: expected a refused patch to persist nothing, got {after} \
                 against {before}"
            );

            // The refusal lifts of its own accord once the build the service already owed
            // comes due — nothing is called in between.
            sleep(past_the_build());
            let patched = patch_portal(&emulator, case, &portal_id, patch.clone()).await;
            assert_eq!(
                content_without_logo_instant(&patched),
                strip_nulls(&patch["portalContent"]),
                "case {case:?}: expected the patch to land once the build came due, got \
                 {patched}"
            );
            assert!(
                [PUBLISH_STATUS_PUBLISHED, PUBLISH_STATUS_DISABLED]
                    .contains(&as_str(&patched, "publishStatus").as_str()),
                "case {case:?}: expected the portal to have settled in a terminal status, got \
                 {patched}"
            );
        } else {
            assert_eq!(
                patch_status, 200,
                "case {case:?}: expected 200 from UpdatePortal, got {patch_status} {patched}"
            );
            assert_eq!(
                content_without_logo_instant(&patched),
                strip_nulls(&patch["portalContent"]),
                "case {case:?}: expected the patch to land, got {patched}"
            );
        }
    }
}

#[tokio::test]
async fn a_portal_is_patched_only_through_its_own_minted_id() {
    let emulator = emulator();
    let case = "a patch addressed at an id nothing minted";

    let portal_id = create_portal(
        &emulator,
        case,
        &json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Addressed")
        }),
    )
    .await;
    let before = get_portal(&emulator, case, &portal_id).await;

    for (label, id) in [
        ("an id nothing minted", "nosuchportal".to_string()),
        ("the id's uppercase form", portal_id.to_uppercase()),
        (
            "the id with its first character dropped",
            portal_id[1..].to_string(),
        ),
        ("a long id nothing minted", "z".repeat(30)),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("{COLLECTION}/{id}"),
            Some(json!({ "portalContent": portal_content("should not land") })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from UpdatePortal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals wrote anything: the portal that does exist is unchanged.
    let after = get_portal(&emulator, case, &portal_id).await;
    assert_eq!(
        strip_nulls(&after),
        strip_nulls(&before),
        "case {case:?}: expected the existing portal to be untouched by the refused patches, \
         got {after} against {before}"
    );
}

#[tokio::test]
async fn a_patch_whose_values_are_invalid_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let case = "an invalid patch";

    let mut create = json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "includedPortalProductArns": [product_arn("prod000011")],
        "rumAppMonitorName": "valid-rum",
        "portalContent": described_content("Valid", "a valid record")
    });
    let _held = hold_included_products(&emulator, case, &mut create).await;
    let portal_id = create_portal(&emulator, case, &create).await;
    let uri = format!("{COLLECTION}/{portal_id}");
    let before = get_portal(&emulator, case, &portal_id).await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "an authorization naming none of its alternatives",
            json!({ "authorization": {} }),
        ),
        (
            "an authorization naming both of its alternatives",
            json!({
                "authorization": {
                    "none": {},
                    "cognitoConfig": cognito_authorization("1h57kf5cpq17m0eml12eqemjq1")["cognitoConfig"]
                }
            }),
        ),
        (
            "an endpoint configuration naming none of its alternatives",
            json!({ "endpointConfiguration": {} }),
        ),
        (
            "an endpoint configuration naming both of its alternatives",
            json!({
                "endpointConfiguration": {
                    "none": {},
                    "acmManaged": own_domain("both.example.com")["acmManaged"]
                }
            }),
        ),
        (
            "an included product that is not an ARN at all",
            json!({ "includedPortalProductArns": ["not-an-arn-at-all"] }),
        ),
        (
            "an included product naming another vendor",
            json!({
                "includedPortalProductArns": [
                    "arn:aws:s3:::example-bucket/portalproducts/prod000012"
                ]
            }),
        ),
        (
            "an included product naming another resource type of this service",
            json!({
                "includedPortalProductArns": [
                    format!(
                        "arn:aws:apigateway:{REGION}:{}:/apis/abc1234567",
                        ::engine::emulator_account_id()
                    )
                ]
            }),
        ),
        (
            "an included product ARN with no product id",
            json!({
                "includedPortalProductArns": [
                    format!(
                        "arn:aws:apigateway:{REGION}:{}:/portalproducts/",
                        ::engine::emulator_account_id()
                    )
                ]
            }),
        ),
        (
            "one valid included product ARN beside one that is not",
            json!({ "includedPortalProductArns": [product_arn("prod000013"), "still-not-an-arn"] }),
        ),
        (
            "a RUM app monitor name one character past the modelled maximum",
            json!({ "rumAppMonitorName": "m".repeat(MAX_RUM_APP_MONITOR_NAME_LEN + 1) }),
        ),
        (
            "a logo URI one character past the modelled maximum",
            json!({ "logoUri": format!("{}l", long_logo_uri()) }),
        ),
    ];

    for (label, patch) in rows {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from UpdatePortal, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {label:?}: expected the patch to be refused as invalid, got {code:?} {body}"
        );

        let after = get_portal(&emulator, label, &portal_id).await;
        assert_eq!(
            strip_nulls(&after),
            strip_nulls(&before),
            "case {label:?}: expected a refused patch to persist nothing, got {after} against \
             {before}"
        );
    }
}
