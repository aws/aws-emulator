// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a stack's custom branding theme over the wire.
//!
//! Every case drives the real awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `CreateThemeForStack` persists the branding and reports it back from the stored
//!   record, so a later `DescribeThemeForStack` answers identically;
//! * the logo and favicon URLs a theme reports are DERIVED from the S3 location the
//!   request named and the Region the request was signed for;
//! * a theme belongs to a stack that exists, and a stack has exactly one theme;
//! * every member an `UpdateThemeForStack` request carries replaces the stored field, a
//!   member it omits leaves that field alone, and a `ThemeAttribute` in
//!   `AttributesToDelete` clears the field behind it;
//! * `DeleteThemeForStack` removes the branding and leaves the stack, and deleting the
//!   stack removes the branding it owns.

use serde_json::{json, Value};

use crate::refusal_messages::{already_exists_sentence, not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The account every refusal names: the single account this emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The resource word AWS's refusal sentences use for a stack, and the words they use for
/// the branding theme a stack owns — a theme is identified by the stack it brands, so its
/// sentence names that stack.
const STACK_KIND: &str = "stack";
const THEME_KIND: &str = "theme for stack";

/// AWS's `ResourceNotFoundException` sentence for a stack that has no custom branding.
///
/// The doubled space after the account id is AWS's own: the sentence's slot between the
/// account and `does not exist` is empty, as it is in the duplicate-name sentence.
fn theme_not_found_sentence(stack: &str) -> String {
    format!("The {THEME_KIND} {stack} for account {ACCOUNT}  does not exist.")
}

/// The URL a theme reports for one of its branding assets: the virtual-hosted-style URL
/// of the S3 object the request named.
///
/// `location` is an `S3Location` as the request carries it (`S3Bucket`, optional
/// `S3Key` — `smithy_query(query="shape", shape="S3Location")`).
fn asset_url(region: &str, location: &Value) -> String {
    let bucket = location["S3Bucket"].as_str().expect("a bucket");
    let key = location["S3Key"].as_str().unwrap_or_default();
    format!("https://{bucket}.s3.{region}.amazonaws.com/{key}")
}

/// `CreateThemeForStack` requests spanning the input class, without their `StackName`.
///
/// The rows differ along the colour theme (every value `ThemeStyling` models), the number
/// of footer links (none, one, two), the length and character classes of the title, and
/// the form of the S3 locations (a flat key, a nested key, a dotted bucket, an absent
/// key).
///
/// Member names, their constraints and the enum values come from
/// `smithy_query(query="operation", operation="CreateThemeForStack")` and the
/// `THEME_STYLING_VALUES` constant in `src/ops/types/shared_types.rs`
/// (BLUE | LIGHT_BLUE | PINK | RED).
fn theme_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            // `ThemeTitleText` is @length 1..300, so a single character is the shortest
            // title a create admits.
            "required members only, shortest title, no footer links",
            json!({
                "TitleText": "A",
                "ThemeStyling": "BLUE",
                "OrganizationLogoS3Location": { "S3Bucket": "brand-assets", "S3Key": "logo.png" },
                "FaviconS3Location": { "S3Bucket": "brand-assets", "S3Key": "favicon.ico" },
            }),
        ),
        (
            "one footer link, light blue",
            json!({
                "TitleText": "Acme Apps",
                "ThemeStyling": "LIGHT_BLUE",
                "FooterLinks": [
                    { "DisplayName": "Support", "FooterLinkURL": "https://support.example.invalid" },
                ],
                "OrganizationLogoS3Location": { "S3Bucket": "brand-assets", "S3Key": "logo.png" },
                "FaviconS3Location": { "S3Bucket": "brand-assets", "S3Key": "favicon.ico" },
            }),
        ),
        (
            // `ThemeTitleText`'s pattern is ^[-@./#&+\w\s]*$, so the punctuation below is
            // admitted and the title is not just letters.
            "two footer links, pink, punctuated title",
            json!({
                "TitleText": "Acme - Apps & More / Portal #1",
                "ThemeStyling": "PINK",
                "FooterLinks": [
                    { "DisplayName": "IT Support", "FooterLinkURL": "https://it.example.invalid/help" },
                    { "DisplayName": "Products", "FooterLinkURL": "https://www.example.invalid/products" },
                ],
                "OrganizationLogoS3Location": {
                    "S3Bucket": "brand.assets-01",
                    "S3Key": "themes/2026/logo-wide.png",
                },
                "FaviconS3Location": {
                    "S3Bucket": "brand.assets-01",
                    "S3Key": "themes/2026/icons/favicon.ico",
                },
            }),
        ),
        (
            // 300 characters is the longest title `ThemeTitleText` admits.
            "longest title, red",
            json!({
                "TitleText": "t".repeat(300),
                "ThemeStyling": "RED",
                "FooterLinks": [
                    { "DisplayName": "Only a name" },
                ],
                "OrganizationLogoS3Location": { "S3Bucket": "logos", "S3Key": "a/b/c/logo.svg" },
                "FaviconS3Location": { "S3Bucket": "logos", "S3Key": "a/b/c/icon.png" },
            }),
        ),
        (
            // `S3Key` is optional on `S3Location`: a location that carries only a bucket
            // addresses that bucket, and the theme still reports a URL for it.
            "locations without a key",
            json!({
                "TitleText": "Bucket only",
                "ThemeStyling": "BLUE",
                "FooterLinks": [],
                "OrganizationLogoS3Location": { "S3Bucket": "keyless-assets" },
                "FaviconS3Location": { "S3Bucket": "keyless-assets" },
            }),
        ),
    ]
}

/// The theme the request must report back: every member derived from that request, with
/// the branding state a create leaves the theme in.
///
/// `CreatedTime` is server-generated, so it is taken from `actual` — its presence and
/// stability are asserted separately.
fn expected_theme(stack: &str, region: &str, request: &Value, actual: &Value) -> Value {
    let mut theme = json!({
        "StackName": stack,
        // Creating the branding is what turns it on for the stack.
        "State": "ENABLED",
        "ThemeTitleText": request["TitleText"],
        "ThemeStyling": request["ThemeStyling"],
        "ThemeOrganizationLogoURL": asset_url(region, &request["OrganizationLogoS3Location"]),
        "ThemeFaviconURL": asset_url(region, &request["FaviconS3Location"]),
        "CreatedTime": actual["CreatedTime"],
    });
    // A theme with no footer links displays none, so the member is absent rather than
    // present-and-empty.
    let links = request["FooterLinks"].as_array();
    if links.is_some_and(|links| !links.is_empty()) {
        theme["ThemeFooterLinks"] = request["FooterLinks"].clone();
    }
    theme
}

/// Create `stack` and return the emulator holding it.
async fn emulator_with_stack(region: &str, stack: &str) -> Emulator {
    let emulator = Emulator::in_region(region);
    emulator
        .ok("setup", "CreateStack", json!({ "Name": stack }))
        .await;
    emulator
}

/// The request that brands `stack`, as the update tests leave it before updating.
fn base_theme_request(stack: &str) -> Value {
    json!({
        "StackName": stack,
        "TitleText": "Acme Apps",
        "ThemeStyling": "BLUE",
        "FooterLinks": [
            { "DisplayName": "Support", "FooterLinkURL": "https://support.example.invalid" },
        ],
        "OrganizationLogoS3Location": { "S3Bucket": "brand-assets", "S3Key": "logo.png" },
        "FaviconS3Location": { "S3Bucket": "brand-assets", "S3Key": "favicon.ico" },
    })
}

#[tokio::test]
async fn create_theme_persists_the_branding_a_later_describe_reports() {
    let region = "us-east-1";

    for (case, body) in theme_rows() {
        let stack = "branded";
        let emulator = emulator_with_stack(region, stack).await;
        let mut request = body.clone();
        request["StackName"] = json!(stack);

        let created = emulator.ok(case, "CreateThemeForStack", request.clone()).await;
        let theme = created["Theme"].clone();

        assert!(
            theme["CreatedTime"].is_number(),
            "case {case:?}: the theme must report when it was created, got {theme}"
        );
        let expected = expected_theme(stack, region, &request, &theme);
        assert_eq!(
            theme, expected,
            "case {case:?}: expected theme {expected}, got {theme}"
        );

        // The reader answers from the stored record, so it reports the same theme —
        // `CreatedTime` included.
        let described = emulator
            .ok(case, "DescribeThemeForStack", json!({ "StackName": stack }))
            .await;
        assert_eq!(
            described["Theme"], theme,
            "case {case:?}: DescribeThemeForStack must report the created theme {theme}, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn theme_asset_urls_address_the_named_object_in_the_requests_region() {
    // Rows: the Region the request is signed for. The URL a theme reports is built from
    // the bucket, the key and that Region, so it changes with each of the three.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        for (case, body) in theme_rows() {
            let stack = "regional";
            let case = format!("{region} / {case}");
            let emulator = emulator_with_stack(region, stack).await;
            let mut request = body.clone();
            request["StackName"] = json!(stack);

            let created = emulator
                .ok(&case, "CreateThemeForStack", request.clone())
                .await;
            let theme = &created["Theme"];

            for (member, location) in [
                ("ThemeOrganizationLogoURL", "OrganizationLogoS3Location"),
                ("ThemeFaviconURL", "FaviconS3Location"),
            ] {
                let expected = asset_url(region, &request[location]);
                assert_eq!(
                    theme[member].as_str().unwrap_or_default(),
                    expected,
                    "case {case:?}: {member} must address the object {location} names, \
                     expected {expected:?}, got {theme}"
                );
            }
        }
    }
}

/// One `UpdateThemeForStack` case: the update to send, and the theme members it must
/// change. A `Value::Null` expectation means the member must be ABSENT afterwards.
struct UpdateRow {
    case: String,
    update: Value,
    expect: Vec<(&'static str, Value)>,
}

/// Updates spanning the input class: the rows differ along the KIND of member being
/// written (a scalar string, an enum, a list of structures, a nested S3 location), along
/// how many entries a list carries, and along whether the request carries anything to
/// change at all. The styling rows walk every value `ThemeStyling` models except the one
/// the base theme already has.
fn update_rows(region: &str) -> Vec<UpdateRow> {
    let mut rows = vec![
        UpdateRow {
            case: "title replaced".to_string(),
            update: json!({ "TitleText": "Acme Portal #2" }),
            expect: vec![("ThemeTitleText", json!("Acme Portal #2"))],
        },
        UpdateRow {
            case: "footer links replaced and grown".to_string(),
            update: json!({
                "FooterLinks": [
                    { "DisplayName": "IT Support", "FooterLinkURL": "https://it.example.invalid" },
                    { "DisplayName": "Products", "FooterLinkURL": "https://www.example.invalid" },
                    { "DisplayName": "Status" },
                ],
            }),
            expect: vec![(
                "ThemeFooterLinks",
                json!([
                    { "DisplayName": "IT Support", "FooterLinkURL": "https://it.example.invalid" },
                    { "DisplayName": "Products", "FooterLinkURL": "https://www.example.invalid" },
                    { "DisplayName": "Status" },
                ]),
            )],
        },
        UpdateRow {
            case: "footer links replaced by none".to_string(),
            update: json!({ "FooterLinks": [] }),
            expect: vec![("ThemeFooterLinks", Value::Null)],
        },
        UpdateRow {
            case: "branding disabled".to_string(),
            update: json!({ "State": "DISABLED" }),
            expect: vec![("State", json!("DISABLED"))],
        },
        UpdateRow {
            case: "branding re-enabled".to_string(),
            update: json!({ "State": "ENABLED" }),
            expect: vec![("State", json!("ENABLED"))],
        },
        UpdateRow {
            case: "assets re-pointed at other objects".to_string(),
            update: json!({
                "OrganizationLogoS3Location": {
                    "S3Bucket": "brand.assets-02",
                    "S3Key": "v2/logo.svg",
                },
                "FaviconS3Location": { "S3Bucket": "brand.assets-02" },
            }),
            expect: vec![
                (
                    "ThemeOrganizationLogoURL",
                    json!(asset_url(
                        region,
                        &json!({ "S3Bucket": "brand.assets-02", "S3Key": "v2/logo.svg" })
                    )),
                ),
                (
                    "ThemeFaviconURL",
                    json!(asset_url(region, &json!({ "S3Bucket": "brand.assets-02" }))),
                ),
            ],
        },
        UpdateRow {
            case: "every member at once".to_string(),
            update: json!({
                "TitleText": "Everything",
                "ThemeStyling": "RED",
                "State": "DISABLED",
                "FooterLinks": [{ "DisplayName": "One", "FooterLinkURL": "https://one.example.invalid" }],
                "OrganizationLogoS3Location": { "S3Bucket": "all-assets", "S3Key": "logo.png" },
                "FaviconS3Location": { "S3Bucket": "all-assets", "S3Key": "icon.png" },
            }),
            expect: vec![
                ("ThemeTitleText", json!("Everything")),
                ("ThemeStyling", json!("RED")),
                ("State", json!("DISABLED")),
                (
                    "ThemeFooterLinks",
                    json!([{ "DisplayName": "One", "FooterLinkURL": "https://one.example.invalid" }]),
                ),
                (
                    "ThemeOrganizationLogoURL",
                    json!(asset_url(
                        region,
                        &json!({ "S3Bucket": "all-assets", "S3Key": "logo.png" })
                    )),
                ),
                (
                    "ThemeFaviconURL",
                    json!(asset_url(
                        region,
                        &json!({ "S3Bucket": "all-assets", "S3Key": "icon.png" })
                    )),
                ),
            ],
        },
        UpdateRow {
            case: "nothing but the stack name".to_string(),
            update: json!({}),
            expect: Vec::new(),
        },
    ];
    // The base theme is BLUE, so every other colour `ThemeStyling` models is a change.
    for styling in ["LIGHT_BLUE", "PINK", "RED"] {
        rows.push(UpdateRow {
            case: format!("styling replaced with {styling}"),
            update: json!({ "ThemeStyling": styling }),
            expect: vec![("ThemeStyling", json!(styling))],
        });
    }
    rows
}

#[tokio::test]
async fn update_theme_writes_the_members_it_carries_and_leaves_the_rest() {
    let region = "us-east-1";
    let stack = "updated";

    for row in update_rows(region) {
        let case = &row.case;
        let emulator = emulator_with_stack(region, stack).await;
        let created = emulator
            .ok(case, "CreateThemeForStack", base_theme_request(stack))
            .await;

        // What the theme must look like afterwards: the created theme with exactly the
        // members this update writes replaced, and every other member — `CreatedTime`
        // included — untouched.
        let mut expected = created["Theme"].clone();
        for (member, value) in &row.expect {
            if value.is_null() {
                expected
                    .as_object_mut()
                    .expect("a theme object")
                    .remove(*member);
            } else {
                expected[*member] = value.clone();
            }
        }

        let mut update = row.update.clone();
        update["StackName"] = json!(stack);
        let updated = emulator.ok(case, "UpdateThemeForStack", update).await;
        assert_eq!(
            updated["Theme"], expected,
            "case {case:?}: expected theme {expected}, got {updated}"
        );

        // The update was persisted, not just reported.
        let described = emulator
            .ok(case, "DescribeThemeForStack", json!({ "StackName": stack }))
            .await;
        assert_eq!(
            described["Theme"], expected,
            "case {case:?}: DescribeThemeForStack must report the updated theme \
             {expected}, got {described}"
        );
    }
}

#[tokio::test]
async fn update_theme_clears_the_attributes_it_deletes() {
    let region = "us-east-1";
    let stack = "cleared";

    // Rows: how many footer links the theme carries before FOOTER_LINKS is deleted. The
    // attribute values are `ThemeAttribute`'s (`THEME_ATTRIBUTE_VALUES` in
    // `src/ops/types/shared_types.rs`), whose single value is FOOTER_LINKS.
    for link_count in 0..=3_usize {
        let case = format!("{link_count} footer link(s) cleared");
        let emulator = emulator_with_stack(region, stack).await;
        let links: Vec<Value> = (0..link_count)
            .map(|index| {
                json!({
                    "DisplayName": format!("Link {index}"),
                    "FooterLinkURL": format!("https://{index}.example.invalid"),
                })
            })
            .collect();
        let mut create = base_theme_request(stack);
        create["FooterLinks"] = json!(links);
        let created = emulator.ok(&case, "CreateThemeForStack", create).await;

        let updated = emulator
            .ok(
                &case,
                "UpdateThemeForStack",
                json!({ "StackName": stack, "AttributesToDelete": ["FOOTER_LINKS"] }),
            )
            .await;

        // The cleared member is ABSENT, not present-and-empty, and nothing else changed.
        let mut expected = created["Theme"].clone();
        expected
            .as_object_mut()
            .expect("a theme object")
            .remove("ThemeFooterLinks");
        assert_eq!(
            updated["Theme"], expected,
            "case {case:?}: expected theme {expected}, got {updated}"
        );

        // The same links can be attached again: clearing detached them, it did not
        // forbid them.
        if link_count > 0 {
            let reattached = emulator
                .ok(
                    &case,
                    "UpdateThemeForStack",
                    json!({ "StackName": stack, "FooterLinks": links }),
                )
                .await;
            assert_eq!(
                reattached["Theme"]["ThemeFooterLinks"],
                json!(links),
                "case {case:?}: the links must be attachable again, got {reattached}"
            );
        }
    }
}

#[tokio::test]
async fn update_theme_refuses_setting_and_deleting_the_same_attribute() {
    let region = "us-east-1";
    let stack = "conflicting";
    let emulator = emulator_with_stack(region, stack).await;
    let created = emulator
        .ok("setup", "CreateThemeForStack", base_theme_request(stack))
        .await;

    // Rows: the other members the request may also carry. Whatever else it asks for,
    // asking to both set and delete one attribute is refused.
    let rows = [
        ("footer links only", json!({})),
        ("alongside a title", json!({ "TitleText": "Rejected" })),
        (
            "alongside a styling and a state",
            json!({ "ThemeStyling": "PINK", "State": "DISABLED" }),
        ),
    ];

    for (case, extra) in rows {
        let mut request = extra.clone();
        request["StackName"] = json!(stack);
        request["AttributesToDelete"] = json!(["FOOTER_LINKS"]);
        request["FooterLinks"] =
            json!([{ "DisplayName": "Set", "FooterLinkURL": "https://set.example.invalid" }]);

        let message = refusal(
            &emulator,
            case,
            "UpdateThemeForStack",
            request,
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        let expected = "FooterLinks cannot be specified while AttributesToDelete \
                        contains FOOTER_LINKS";
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // A refused request changes nothing.
        let described = emulator
            .ok(case, "DescribeThemeForStack", json!({ "StackName": stack }))
            .await;
        assert_eq!(
            described["Theme"], created["Theme"],
            "case {case:?}: the refused update must leave the theme {} alone, got {described}",
            created["Theme"]
        );
    }
}

#[tokio::test]
async fn a_theme_needs_its_stack_and_a_stack_has_one_theme() {
    let region = "us-east-1";

    for (case, body) in theme_rows() {
        let stack = "branded";
        let emulator = Emulator::in_region(region);
        let mut request = body.clone();
        request["StackName"] = json!(stack);

        // Nothing to brand yet: the refusal names the STACK that does not exist.
        let message = refusal(
            &emulator,
            case,
            "CreateThemeForStack",
            request.clone(),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(STACK_KIND, stack);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        emulator
            .ok(case, "CreateStack", json!({ "Name": stack }))
            .await;
        emulator
            .ok(case, "CreateThemeForStack", request.clone())
            .await;

        // A stack has exactly one theme, so branding it again is a duplicate.
        let message = refusal(
            &emulator,
            case,
            "CreateThemeForStack",
            request,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        let expected = already_exists_sentence(THEME_KIND, stack);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_unbranded_stack_has_no_theme_to_read_update_or_delete() {
    let region = "us-east-1";
    let stack = "unbranded";
    let emulator = emulator_with_stack(region, stack).await;

    // Rows: every operation that reads or changes branding that is not there. The stack
    // exists, so what each refusal reports missing is the THEME.
    let rows: Vec<(&str, Value)> = vec![
        ("DescribeThemeForStack", json!({ "StackName": stack })),
        (
            "UpdateThemeForStack",
            json!({ "StackName": stack, "TitleText": "nobody applies this" }),
        ),
        ("DeleteThemeForStack", json!({ "StackName": stack })),
    ];

    for (operation, request) in &rows {
        let message = refusal(
            &emulator,
            operation,
            operation,
            request.clone(),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = theme_not_found_sentence(stack);
        assert_eq!(
            message, expected,
            "case {operation:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // A reader asked about a stack that does not exist reports the STACK missing, not
    // its branding.
    for absent in ["absent", "Mixed.Case_name-9", "0123456789"] {
        let message = refusal(
            &emulator,
            absent,
            "DescribeThemeForStack",
            json!({ "StackName": absent }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(STACK_KIND, absent);
        assert_eq!(
            message, expected,
            "case {absent:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn deleting_the_theme_leaves_the_stack_and_frees_the_branding() {
    let region = "us-east-1";

    for (case, body) in theme_rows() {
        let stack = "rebranded";
        let emulator = emulator_with_stack(region, stack).await;
        let mut request = body.clone();
        request["StackName"] = json!(stack);
        emulator
            .ok(case, "CreateThemeForStack", request.clone())
            .await;

        emulator
            .ok(case, "DeleteThemeForStack", json!({ "StackName": stack }))
            .await;

        // The branding is gone …
        let message = refusal(
            &emulator,
            case,
            "DescribeThemeForStack",
            json!({ "StackName": stack }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = theme_not_found_sentence(stack);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // … the stack it branded is not …
        let described = emulator
            .ok(case, "DescribeStacks", json!({ "Names": [stack] }))
            .await;
        assert_eq!(
            described["Stacks"][0]["Name"],
            json!(stack),
            "case {case:?}: deleting the theme must leave the stack, got {described}"
        );

        // … and the stack can be branded again.
        let recreated = emulator.ok(case, "CreateThemeForStack", request).await;
        assert_eq!(
            recreated["Theme"]["StackName"],
            json!(stack),
            "case {case:?}: the stack must be brandable again, got {recreated}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_theme_it_owns() {
    let region = "us-east-1";

    for (case, body) in theme_rows() {
        let stack = "short-lived";
        let emulator = emulator_with_stack(region, stack).await;
        let mut request = body.clone();
        request["StackName"] = json!(stack);
        emulator
            .ok(case, "CreateThemeForStack", request.clone())
            .await;

        emulator
            .ok(case, "DeleteStack", json!({ "Name": stack }))
            .await;
        // A stack created again under the same name starts unbranded: the theme went
        // with the stack that owned it.
        emulator
            .ok(case, "CreateStack", json!({ "Name": stack }))
            .await;

        let message = refusal(
            &emulator,
            case,
            "DescribeThemeForStack",
            json!({ "StackName": stack }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = theme_not_found_sentence(stack);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}
