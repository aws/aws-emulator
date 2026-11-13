// SPDX-License-Identifier: CC0-1.0

//! What `ListSigningPlatforms` answers for the signing-platform catalog this
//! service publishes, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpQuery` bindings, deserialization, the handler and serialization — the
//! same path a client takes. The rules under test, each over a TABLE rather than
//! over one request:
//!
//! * a listing enumerates the platforms this service publishes, each reported
//!   with the members the model declares for a `SigningPlatform` and with
//!   exactly what the single-platform reader reports for the same id;
//! * each of `category`, `partner` and `target` narrows the listing to the
//!   published platforms whose value for that member EQUALS the filter, and a
//!   filter value no published platform carries narrows it to nothing — an empty
//!   page, not a refusal;
//! * a pagination token is an opaque value the service ISSUES for one listing —
//!   the filters that call carried — so a page resumes only a call that
//!   enumerates that same listing, and a `nextToken` the service never handed
//!   out for it is refused 400 `ValidationException` (`code` naming the
//!   invalid-token condition) instead of being served as a page out of some
//!   other ordering;
//! * walking a listing with the tokens it hands out therefore reports every
//!   platform of that listing exactly once, in one stable order, whatever the
//!   page size — and a platform is a published fact, so the walk changes nothing
//!   a later caller sees.
//!
//! The rows differ along what the rules generalise over: which member is
//! filtered on and on how many at once, whether the filter matches every, some
//! or no published platform, how a non-matching value differs from a matching
//! one (case, prefix, a member's value carried on another member), the page size
//! (both published bounds of `MaxResults` and each size in between, plus one
//! past each bound), and the FORM of a refused token and the listing it is
//! replayed against.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    PlatformListing, PLATFORM_CATEGORY_AWS, PLATFORM_CATEGORY_EXTERNAL, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite calls through. Platforms are a published catalog rather
/// than a regional resource, but a request still carries a credential scope.
const REGION: &str = "us-west-2";

/// `MaxResults`'s modelled bounds (`@range(min: 1, max: 25)`, see
/// `smithy_query(query="operation",operation="ListSigningPlatforms")`).
const MAX_RESULTS_FLOOR: i32 = 1;
const MAX_RESULTS_CEILING: i32 = 25;

/// The error `code` a `nextToken` this service never handed out carries, spelled
/// out here rather than imported so a wrong code in the handler fails this suite.
const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// The modelled error shape a refused request is answered with (HTTP 400).
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// The one value this model's `Category` enum names
/// (`smithy_query(query="shape",shape="Category")`, mirrored in
/// `crate::ops::types::CATEGORY_VALUES`). The published catalog publishes no
/// platform in it, so a listing filtered on it selects nothing: a filter value is
/// matched against what the catalog publishes, not against the modelled enum.
const MODELLED_CATEGORY: &str = "AWSIoT";

/// The members the model declares for a `SigningPlatform`, as it spells them on
/// the wire (`smithy_query(query="shape",shape="SigningPlatform")`). Every one of
/// them is a published fact about the platform, so a listing and a get must
/// agree on all of them.
const SIGNING_PLATFORM_MEMBERS: &[&str] = &[
    "platformId",
    "displayName",
    "partner",
    "target",
    "category",
    "signingConfiguration",
    "signingImageFormat",
    "maxSizeInMB",
    "revocationSupported",
];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or the empty string when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The reported platforms, in the order they were reported.
    fn listed(&self) -> Vec<Value> {
        self.body["platforms"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "ListSigningPlatforms must report a platforms list: {:?}",
                    self.body
                )
            })
            .clone()
    }

    /// The reported platform ids, in the order they were reported.
    fn listed_ids(&self) -> Vec<String> {
        self.listed()
            .iter()
            .map(|platform| {
                platform["platformId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `nextToken` this answer carries, or `None` on a final page.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `ListSigningPlatforms` with the raw query string `query`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/signing-platforms{query}"), None).await
}

/// `GetSigningPlatform` for one id — the single-platform reader a listing has to
/// agree with.
async fn get_platform(router: &MultiServiceRouter, platform_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-platforms/{platform_id}"),
        None,
    )
    .await
}

/// Which platforms one listing enumerates, as the request spells its filters.
///
/// `None` is a member the call filters on at all; every value is already in the
/// form it travels in a query string.
#[derive(Debug, Default)]
struct Filter {
    category: Option<&'static str>,
    partner: Option<&'static str>,
    target: Option<&'static str>,
}

impl Filter {
    /// The filters as query parameters, each prefixed with `&` so a row can be
    /// appended to a `?maxResults=…` query.
    fn query(&self) -> String {
        let mut query = String::new();
        for (name, value) in [
            ("category", self.category),
            ("partner", self.partner),
            ("target", self.target),
        ] {
            if let Some(value) = value {
                query.push_str(&format!("&{name}={value}"));
            }
        }
        query
    }

    /// The published platforms this filter set selects, in the order the catalog
    /// publishes them: each filter the row names has to EQUAL the platform's
    /// published value for that member.
    ///
    /// Derived from the catalog the service publishes
    /// (`SIGNING_PLATFORM_CATALOG`), so a platform added to it is covered here
    /// without the table naming it.
    fn expected_ids(&self) -> Vec<String> {
        SIGNING_PLATFORM_CATALOG
            .iter()
            .filter(|entry| {
                self.category.is_none_or(|wanted| wanted == entry.category)
                    && self.partner.is_none_or(|wanted| wanted == entry.partner)
                    && self.target.is_none_or(|wanted| wanted == entry.target)
            })
            .map(|entry| entry.platform_id.to_string())
            .collect()
    }
}

/// One filter row: what it selects, and whether the suite expects it to select
/// any platform at all (so a handler answering nothing everywhere fails).
struct FilterCase {
    case: &'static str,
    filter: Filter,
    expect_platforms: bool,
}

/// The rows differ along which member is filtered on, how many at once, and
/// whether the value matches every, some or no published platform — including a
/// matching value in the wrong case, a prefix of one, and one member's value
/// carried on another member.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no filter at all",
            filter: Filter::default(),
            expect_platforms: true,
        },
        FilterCase {
            case: "the only category the model's enum names, which no platform is published in",
            filter: Filter {
                category: Some(MODELLED_CATEGORY),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "the category of a platform AWS does not own",
            filter: Filter {
                category: Some(PLATFORM_CATEGORY_EXTERNAL),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a partner that is not a segment of its platform's id",
            filter: Filter {
                partner: Some("Notary"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a target that is not the whole suffix of its platform's id",
            filter: Filter {
                target: Some("OCI"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "the AWS category",
            filter: Filter {
                category: Some(PLATFORM_CATEGORY_AWS),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a partner that owns one platform",
            filter: Filter {
                partner: Some("AWSLambda"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a partner that owns several platforms",
            filter: Filter {
                partner: Some("AmazonFreeRTOS"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a target naming an algorithm pair",
            filter: Filter {
                target: Some("SHA256-ECDSA"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a target naming a device signing template",
            filter: Filter {
                target: Some("SHA1-RSA-TISHA1"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a category and the partner inside it",
            filter: Filter {
                category: Some(PLATFORM_CATEGORY_AWS),
                partner: Some("AmazonFreeRTOS"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a partner and one of its targets",
            filter: Filter {
                partner: Some("AmazonFreeRTOS"),
                target: Some("SHA256-ECDSA"),
                ..Filter::default()
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "all three members of one published platform",
            filter: Filter {
                category: Some(PLATFORM_CATEGORY_AWS),
                partner: Some("AWSLambda"),
                target: Some("SHA384-ECDSA"),
            },
            expect_platforms: true,
        },
        FilterCase {
            case: "a category and a partner published in the other one",
            filter: Filter {
                category: Some(PLATFORM_CATEGORY_EXTERNAL),
                partner: Some("AWSLambda"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "a published category in the wrong case",
            filter: Filter {
                category: Some("aws"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "a prefix of a published partner",
            filter: Filter {
                partner: Some("Amazon"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "a published platform id as the partner",
            filter: Filter {
                partner: Some("AWSLambda-SHA384-ECDSA"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "a published partner carried on the target",
            filter: Filter {
                target: Some("AWSLambda"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
        FilterCase {
            case: "a category no platform is published in",
            filter: Filter {
                category: Some("AWSPrivate"),
                ..Filter::default()
            },
            expect_platforms: false,
        },
    ]
}

/// Assert `token` is an opaque server-issued value and not the resume key itself:
/// it carries only characters an `@httpQuery` value travels in unescaped, and it
/// carries no published platform id in the clear.
fn assert_opaque(case: &str, token: &str) {
    assert!(
        token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case {case:?}: expected an opaque token of hex digits, got {token:?}"
    );
    for entry in SIGNING_PLATFORM_CATALOG {
        assert!(
            !token.contains(entry.platform_id),
            "case {case:?}: expected a token that does not carry the platform id {:?} in the \
             clear, got {token:?}",
            entry.platform_id
        );
    }
}

/// Walk a listing with the tokens it hands out, asserting every page is a 200 of
/// at most `page_size` platforms and that each token is opaque, and answering
/// every platform id reported, in order.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    filters: &str,
    page_size: i32,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=SIGNING_PLATFORM_CATALOG.len() {
        let query = match token.as_deref() {
            Some(token) => format!("?maxResults={page_size}{filters}&nextToken={token}"),
            None => format!("?maxResults={page_size}{filters}"),
        };
        let page = list(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {query} expected 200, got {} {:?}",
            page.status, page.body
        );
        let reported = page.listed_ids();
        assert!(
            reported.len() <= page_size as usize,
            "case {case:?}: page {query} expected at most {page_size} platforms, got {reported:?}"
        );
        ids.extend(reported);
        match page.next_token() {
            Some(next) => {
                assert_opaque(case, &next);
                token = Some(next);
            }
            None => return ids,
        }
    }
    panic!(
        "case {case:?}: the walk did not end within {} pages",
        SIGNING_PLATFORM_CATALOG.len() + 1
    );
}

/// Every published platform is listed, with every member the model declares for
/// it and with exactly what the single-platform reader answers for the same id.
#[tokio::test]
async fn a_listing_reports_the_published_catalog_as_the_get_reader_does() {
    let router = emulator();

    let answer = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    assert_eq!(
        answer.status, 200,
        "expected ListSigningPlatforms 200, got {} {:?}",
        answer.status, answer.body
    );
    let expected: Vec<String> = Filter::default().expected_ids();
    assert_eq!(
        answer.listed_ids(),
        expected,
        "expected the whole published catalog {expected:?}, got {:?}",
        answer.listed_ids()
    );
    assert_eq!(
        answer.next_token(),
        None,
        "expected no nextToken when the whole catalog fits one page, got {:?}",
        answer.next_token()
    );

    for platform in answer.listed() {
        let case = platform["platformId"].as_str().unwrap_or_default().to_string();
        let got = get_platform(&router, &case).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: a listed platform must be readable, got {} {:?}",
            got.status, got.body
        );
        for member in SIGNING_PLATFORM_MEMBERS {
            assert_eq!(
                platform[member], got.body[*member],
                "case {case:?}: expected the listing and the get to agree on {member}, got {:?} \
                 and {:?}",
                platform[member], got.body[*member]
            );
        }
        // Every modelled member of a published platform is reported: the listing
        // is the full summary, not the id alone.
        for member in ["platformId", "partner", "target", "category"] {
            assert!(
                platform[member].as_str().is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected {member} to be reported, got {platform:?}"
            );
        }
        assert!(
            platform["maxSizeInMB"].as_i64().is_some_and(|max| max > 0),
            "case {case:?}: expected a positive payload bound, got {platform:?}"
        );
        assert!(
            platform["revocationSupported"].is_boolean(),
            "case {case:?}: expected revocationSupported to be reported, got {platform:?}"
        );
    }
}

/// Each filter narrows the listing to the published platforms whose value for
/// that member equals it, and a value no platform carries narrows it to nothing.
#[tokio::test]
async fn a_filter_selects_the_platforms_that_carry_its_value() {
    let router = emulator();

    for row in filter_cases() {
        let case = row.case;
        let query = format!("?maxResults={MAX_RESULTS_CEILING}{}", row.filter.query());
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {query}, got {} {:?}",
            answer.status, answer.body
        );
        let expected = row.filter.expected_ids();
        assert_eq!(
            !expected.is_empty(),
            row.expect_platforms,
            "case {case:?}: the table expects expect_platforms={}, but the catalog selects \
             {expected:?}",
            row.expect_platforms
        );
        assert_eq!(
            answer.listed_ids(),
            expected,
            "case {case:?}: expected {expected:?} for {query}, got {:?}",
            answer.listed_ids()
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: expected no nextToken on a final page, got {:?}",
            answer.next_token()
        );
        // Whatever the filter selected satisfies every filter the row named.
        for platform in answer.listed() {
            for (member, wanted) in [
                ("category", row.filter.category),
                ("partner", row.filter.partner),
                ("target", row.filter.target),
            ] {
                if let Some(wanted) = wanted {
                    assert_eq!(
                        platform[member].as_str(),
                        Some(wanted),
                        "case {case:?}: expected every listed platform to carry {member} \
                         {wanted:?}, got {platform:?}"
                    );
                }
            }
        }
    }
}

/// Walking a listing with the tokens it issues reports every platform of that
/// listing exactly once, in the one order the catalog publishes, whatever the
/// page size — and the walk leaves what a later caller sees untouched.
#[tokio::test]
async fn a_listing_walked_with_its_own_tokens_reports_every_platform_once() {
    let router = emulator();

    // Every page size from the modelled floor to one past the catalog's size,
    // plus the modelled ceiling: a page smaller than, equal to and larger than
    // the listing it cuts.
    let mut page_sizes: Vec<i32> = (MAX_RESULTS_FLOOR
        ..=i32::try_from(SIGNING_PLATFORM_CATALOG.len() + 1).expect("the catalog is small"))
        .collect();
    page_sizes.push(MAX_RESULTS_CEILING);

    for row in filter_cases() {
        for page_size in &page_sizes {
            let case = format!("{} at {page_size} per page", row.case);
            let expected = row.filter.expected_ids();
            let walked = walk(&router, &case, &row.filter.query(), *page_size).await;
            assert_eq!(
                walked, expected,
                "case {case:?}: expected {expected:?} once each in catalog order, got {walked:?}"
            );
            let unique: BTreeSet<&String> = walked.iter().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case:?}: expected no platform twice, got {walked:?}"
            );
            // A read reports a published fact, so the walk is repeatable.
            let again = walk(&router, &case, &row.filter.query(), *page_size).await;
            assert_eq!(
                again, walked,
                "case {case:?}: expected the walk to be repeatable, got {again:?} after {walked:?}"
            );
        }
    }
}

/// A token the service never handed out for the listing it is replayed against.
struct RefusalCase {
    case: &'static str,
    /// The rejected `nextToken` value.
    token: String,
    /// The filters the token is replayed under, appended to the query.
    filters: &'static str,
}

fn refusal_cases(issued: &str, issued_for_profiles: &str) -> Vec<RefusalCase> {
    // A token's payload is hex, so flipping one digit of a real token yields a
    // value of the right form that decodes to something the service never issued.
    let mut corrupted: Vec<char> = issued.chars().collect();
    let last = corrupted.len() - 1;
    corrupted[last] = if corrupted[last] == '0' { '1' } else { '0' };
    let corrupted: String = corrupted.into_iter().collect();

    vec![
        RefusalCase {
            case: "a value no page ever returned",
            token: "zzzz".to_string(),
            filters: "",
        },
        RefusalCase {
            case: "the id of a published platform",
            token: "AWSLambda-SHA384-ECDSA".to_string(),
            filters: "",
        },
        RefusalCase {
            case: "the id of the first platform of the listing",
            token: SIGNING_PLATFORM_CATALOG[0].platform_id.to_string(),
            filters: "",
        },
        RefusalCase {
            case: "a platform id hex-encoded by the client",
            token: ::hex::encode("AWSLambda-SHA384-ECDSA"),
            filters: "",
        },
        RefusalCase {
            case: "an issued token truncated in transit",
            token: issued[..issued.len() - 4].to_string(),
            filters: "",
        },
        RefusalCase {
            case: "an issued token with one digit corrupted",
            token: corrupted,
            filters: "",
        },
        RefusalCase {
            case: "an issued token with a trailing digit appended",
            token: format!("{issued}a"),
            filters: "",
        },
        RefusalCase {
            case: "an issued token replayed under a category filter",
            token: issued.to_string(),
            filters: "&category=External",
        },
        RefusalCase {
            case: "an issued token replayed under a partner filter",
            token: issued.to_string(),
            filters: "&partner=AmazonFreeRTOS",
        },
        RefusalCase {
            case: "an issued token replayed under a target filter",
            token: issued.to_string(),
            filters: "&target=SHA256-ECDSA",
        },
        // The filters are request values this service constrains to no alphabet,
        // so these rows probe the field boundary of the cursor with values shaped
        // like its own rendering and with the separator itself: neither may be
        // read as the listing the token was issued for.
        RefusalCase {
            case: "an issued token replayed under a partner shaped like a rendered field",
            token: issued.to_string(),
            filters: "&partner=6:AWSIoT",
        },
        RefusalCase {
            case: "an issued token replayed under a partner carrying the field separator",
            token: issued.to_string(),
            filters: "&partner=x%1Fy",
        },
        RefusalCase {
            case: "a token the profile listing handed out",
            token: issued_for_profiles.to_string(),
            filters: "",
        },
        // A cursor of the right form, bound to the listing it is carried into,
        // that resumes at a published platform the FILTERS exclude: it names no
        // position in this enumeration, so it is refused rather than answered
        // with the empty page a listing that matches nothing serves.
        RefusalCase {
            case: "a cursor of this listing resuming at a platform its filters exclude",
            token: PlatformListing {
                category: Some(PLATFORM_CATEGORY_EXTERNAL.to_string()),
                ..PlatformListing::default()
            }
            .page_token("AWSLambda-SHA384-ECDSA"),
            filters: "&category=External",
        },
    ]
}

/// A `nextToken` this service never issued for the listing it is carried into is
/// refused 400 with the modelled `code`, rather than served as a page.
#[tokio::test]
async fn a_next_token_this_listing_never_handed_out_is_refused() {
    let router = emulator();

    let first = list(&router, "?maxResults=2").await;
    assert_eq!(
        first.status, 200,
        "the first page must be served: {:?}",
        first.body
    );
    let issued = first
        .next_token()
        .unwrap_or_else(|| panic!("a truncated page must carry a nextToken: {:?}", first.body));
    assert_opaque("the issued first-page token", &issued);

    // A token issued by the service's OTHER paginated read: it decodes and its
    // digest matches, so only the listing it was issued for keeps it out.
    for name in ["Token_Profile_A", "Token_Profile_B"] {
        let put = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({ "platformId": "AWSLambda-SHA384-ECDSA" })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "seeding {name}: expected 200, got {} {:?}",
            put.status, put.body
        );
    }
    let profiles = list_profiles(&router, "?maxResults=1").await;
    let issued_for_profiles = profiles.next_token().unwrap_or_else(|| {
        panic!(
            "a truncated profile page must carry a nextToken: {:?}",
            profiles.body
        )
    });

    for row in refusal_cases(&issued, &issued_for_profiles) {
        let case = row.case;
        let query = format!("?maxResults=2{}&nextToken={}", row.filters, row.token);
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {query}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected the {ERROR_SHAPE_VALIDATION} shape, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            ERROR_CODE_INVALID_NEXT_TOKEN,
            "case {case:?}: expected the modelled code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: expected a message beside the code, got {:?}",
            answer.body
        );
        assert!(
            answer.body["platforms"].is_null(),
            "case {case:?}: a refusal must report no platforms, got {:?}",
            answer.body
        );
    }
}

/// `ListSigningProfiles` with the raw query string `query` — the service's other
/// paginated read, which issues tokens of the same form.
async fn list_profiles(router: &MultiServiceRouter, query: &str) -> Answer {
    let answer = call(router, "GET", &format!("/signing-profiles{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "a profile listing must be served: {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// `maxResults` is held to its modelled range: each published bound is served,
/// and one past either bound is refused 400.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let router = emulator();

    for page_size in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let answer = list(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            answer.status, 200,
            "case {page_size}: a page size the model allows must be served, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.listed_ids().len() <= page_size as usize,
            "case {page_size}: expected at most {page_size} platforms, got {:?}",
            answer.listed_ids()
        );
    }

    for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let answer = list(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            answer.status, 400,
            "case {page_size}: a page size outside the modelled range must be refused, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {page_size}: expected the {ERROR_SHAPE_VALIDATION} shape, got {:?}",
            answer.body
        );
    }
}

/// Listing platforms publishes nothing: an id outside the catalog is still not
/// found after a listing, and the listing answers the same thing afterwards.
#[tokio::test]
async fn listing_platforms_publishes_no_platform() {
    let router = emulator();

    let unpublished = "PrivatePlatform-SHA256-ECDSA";
    let before = get_platform(&router, unpublished).await;
    assert_eq!(
        before.status, 404,
        "case {unpublished:?}: expected 404 before the listing, got {} {:?}",
        before.status, before.body
    );

    let first = list(&router, "?maxResults=1").await;
    let walked = walk(&router, "the whole catalog", "", 1).await;
    let after = get_platform(&router, unpublished).await;
    assert_eq!(
        after.status, 404,
        "case {unpublished:?}: expected 404 after the listing, got {} {:?}",
        after.status, after.body
    );
    let again = list(&router, "?maxResults=1").await;
    assert_eq!(
        again.body, first.body,
        "expected the first page to be unchanged by a walk of {walked:?}, got {:?} after {:?}",
        again.body, first.body
    );
}
