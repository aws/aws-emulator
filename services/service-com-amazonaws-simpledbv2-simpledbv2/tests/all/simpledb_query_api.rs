// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Amazon SimpleDB 2009-04-15 AWS Query API served on the
//! `sdb` endpoint beside the modelled v2 export operations.
//!
//! The rule under test: a form-encoded request on `"/"` naming an `Action` of that API
//! is served by that action's handler — HTTP 200 and a `<<Action>Response
//! xmlns="http://sdb.amazonaws.com/doc/2009-04-15/">` document carrying the action's own
//! result and `ResponseMetadata` — and every answer is computed from the ONE store the
//! v2 export operations read, so a domain created here is a domain `StartDomainExport`
//! resolves and the items written here are the items an export of that domain counts.
//! A refusal is that API's own `<Response><Errors><Error>` envelope naming the code and
//! the sentence the API publishes for it.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, the
//! service's request-claiming seam, and the handlers — so what is asserted is what a
//! SimpleDB client observes.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_simpledbv2_simpledbv2::service::sdb_query_api::{
    API_VERSION, SERVED_ACTIONS, UNSERVED_ACTIONS,
};
use service_com_amazonaws_simpledbv2_simpledbv2::state::simpledb_domains::{
    MAX_DOMAIN_NAME_LEN, MAX_LIST_DOMAINS, MAX_TEXT_BYTES, MIN_DOMAIN_NAME_LEN,
};

use crate::domain_export_lifecycle::{call, emulator, error_code, member, ok_call, REGION};

/// The XML namespace every document of this API carries.
const SDB_NAMESPACE: &str = "http://sdb.amazonaws.com/doc/2009-04-15/";

/// The bucket every export below writes to — a name the modelled `S3BucketName` pattern
/// admits, so no case is refused on it.
const BUCKET: &str = "sdb-query-api-bucket";

/// How long a case waits for a status move the clock owes before giving up.
///
/// Generous on purpose: what is asserted is that the move HAPPENS, never how fast the
/// host is, and every wait below polls for the condition instead of sleeping a fixed
/// time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One call to the 2009-04-15 API: `(http status, response body)`.
///
/// The request is what a SimpleDB client sends — `POST "/"` with an
/// `application/x-www-form-urlencoded` body — and the body is encoded by the
/// `form_urlencoded` crate rather than by hand, so a value carrying a space, an `&` or
/// non-ASCII text reaches the service exactly as a client would send it.
async fn query(router: &MultiServiceRouter, parameters: &[(&str, &str)]) -> (u16, String) {
    let mut form = ::form_urlencoded::Serializer::new(String::new());
    for (name, value) in parameters {
        form.append_pair(name, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sdb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded; charset=utf-8")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDSDBTEST/20260101/{REGION}/sdb/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// One call that must be served, returning the document it answered with.
async fn served(
    router: &MultiServiceRouter,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut form: Vec<(&str, &str)> = vec![("Action", action), ("Version", API_VERSION)];
    form.extend_from_slice(parameters);
    let (status, body) = query(router, &form).await;
    assert_eq!(
        status, 200,
        "case {action} {parameters:?}: expected HTTP 200, got {status} with {body}"
    );
    assert!(
        body.contains(&format!("<{action}Response xmlns=\"{SDB_NAMESPACE}\">")),
        "case {action} {parameters:?}: expected the {action}Response envelope in the \
         2009-04-15 namespace, got {body}"
    );
    assert!(
        text_of(&body, "RequestId").is_some_and(|id| !id.is_empty()),
        "case {action} {parameters:?}: every answer names the request it answers, got {body}"
    );
    assert!(
        text_of(&body, "BoxUsage").is_some_and(|usage| usage.parse::<f64>().is_ok()),
        "case {action} {parameters:?}: every answer reports a BoxUsage, got {body}"
    );
    body
}

/// One call that must be refused, returning `(http status, error code, error message)`.
async fn refused(
    router: &MultiServiceRouter,
    parameters: &[(&str, &str)],
) -> (u16, String, String) {
    let (status, body) = query(router, parameters).await;
    assert!(
        body.contains("<Response><Errors><Error>"),
        "case {parameters:?}: a refusal is this API's own error envelope, got {body}"
    );
    let code = text_of(&body, "Code").unwrap_or_default();
    let message = text_of(&body, "Message").unwrap_or_default();
    (status, code, message)
}

/// The text of the FIRST element named `name`, wherever it sits in the document.
///
/// Read with a real XML parser, so what an assertion compares is the text a client's own
/// parser resolves — entities and all — and never the bytes the wire carried. The match
/// is on the element's LOCAL name, because this API's answers sit in its own namespace
/// and its error documents sit in none.
fn text_of(document: &str, name: &str) -> Option<String> {
    texts_of(document, name).into_iter().next()
}

/// The text of EVERY element named `name`, in document order.
fn texts_of(document: &str, name: &str) -> Vec<String> {
    let parsed = ::roxmltree::Document::parse(document)
        .unwrap_or_else(|error| panic!("the answer must be XML ({error}): {document}"));
    parsed
        .descendants()
        .filter(|node| node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// The attribute name-value pairs a `GetAttributesResult` reports, in document order.
fn attribute_pairs(document: &str) -> Vec<(String, String)> {
    let parsed = ::roxmltree::Document::parse(document)
        .unwrap_or_else(|error| panic!("the answer must be XML ({error}): {document}"));
    let child_text = |element: ::roxmltree::Node<'_, '_>, name: &str| {
        element
            .children()
            .find(|node| node.tag_name().name() == name)
            .map(|node| node.text().unwrap_or_default().to_string())
    };
    parsed
        .descendants()
        .filter(|node| node.tag_name().name() == "Attribute")
        .filter_map(|element| Some((child_text(element, "Name")?, child_text(element, "Value")?)))
        .collect()
}

/// Create `domain` on the 2009-04-15 wire.
async fn create_domain(router: &MultiServiceRouter, domain: &str) {
    served(router, "CreateDomain", &[("DomainName", domain)]).await;
}

/// Put one attribute pair on `item` of `domain`, replacing what that name held when
/// `replace` says so.
async fn put_attribute(
    router: &MultiServiceRouter,
    domain: &str,
    item: &str,
    name: &str,
    value: &str,
    replace: bool,
) {
    served(
        router,
        "PutAttributes",
        &[
            ("DomainName", domain),
            ("ItemName", item),
            ("Attribute.0.Name", name),
            ("Attribute.0.Value", value),
            ("Attribute.0.Replace", if replace { "true" } else { "false" }),
        ],
    )
    .await;
}

/// Write the three pairs the delete cases work from onto `item`: one multi-valued
/// attribute and one single-valued one.
async fn seed_item(router: &MultiServiceRouter, domain: &str, item: &str) {
    for (name, value) in [("colour", "blue"), ("colour", "green"), ("size", "large")] {
        put_attribute(router, domain, item, name, value, false).await;
    }
}

/// The pairs `GetAttributes` reports for `item` of `domain`.
async fn get_attributes(
    router: &MultiServiceRouter,
    domain: &str,
    item: &str,
) -> Vec<(String, String)> {
    let body = served(
        router,
        "GetAttributes",
        &[("DomainName", domain), ("ItemName", item)],
    )
    .await;
    attribute_pairs(&body)
}

/// One row of the served-action table: the action, the request that must be served, and
/// the result members its answer has to carry.
struct EnvelopeCase<'a> {
    action: &'a str,
    parameters: Vec<(&'a str, &'a str)>,
    result_members: Vec<&'a str>,
}

/// One row of the write-and-read-back table: the pairs to write in order (with each
/// pair's `Replace` flag), and the pairs `GetAttributes` must report afterwards.
struct WriteCase<'a> {
    case: &'a str,
    writes: Vec<(&'a str, &'a str, bool)>,
    expected: Vec<(&'a str, &'a str)>,
}

/// One row of a refusal table: the request, and the status, code and sentence it must be
/// refused with. `item` names the item a conditional case writes to, and is unused by the
/// rows that name no item.
struct RefusalCase<'a> {
    case: &'a str,
    item: &'a str,
    parameters: Vec<(&'a str, &'a str)>,
    status: u16,
    code: &'a str,
    message: String,
}

/// Every action this surface serves answers its OWN 2009-04-15 envelope, with the result
/// members that action reports and nothing else.
///
/// The rows are the whole served catalogue (`SERVED_ACTIONS`), in an order that leaves
/// each one something to work on, so no action of this API is served by accident and none
/// is left unexercised.
#[tokio::test]
async fn every_served_action_answers_its_own_envelope() {
    let router = emulator();
    let domain = "envelope-domain";

    let rows: Vec<EnvelopeCase<'_>> = vec![
        EnvelopeCase {
            action: "CreateDomain",
            parameters: vec![("DomainName", domain)],
            result_members: vec![],
        },
        EnvelopeCase {
            action: "ListDomains",
            parameters: vec![],
            result_members: vec!["DomainName"],
        },
        EnvelopeCase {
            action: "PutAttributes",
            parameters: vec![
                ("DomainName", domain),
                ("ItemName", "item-1"),
                ("Attribute.0.Name", "colour"),
                ("Attribute.0.Value", "blue"),
            ],
            result_members: vec![],
        },
        EnvelopeCase {
            action: "GetAttributes",
            parameters: vec![("DomainName", domain), ("ItemName", "item-1")],
            result_members: vec!["Attribute"],
        },
        EnvelopeCase {
            action: "DomainMetadata",
            parameters: vec![("DomainName", domain)],
            result_members: vec![
                "ItemCount",
                "ItemNamesSizeBytes",
                "AttributeNameCount",
                "AttributeNamesSizeBytes",
                "AttributeValueCount",
                "AttributeValuesSizeBytes",
                "Timestamp",
            ],
        },
        EnvelopeCase {
            action: "DeleteAttributes",
            parameters: vec![("DomainName", domain), ("ItemName", "item-1")],
            result_members: vec![],
        },
        EnvelopeCase {
            action: "DeleteDomain",
            parameters: vec![("DomainName", domain)],
            result_members: vec![],
        },
    ];

    let exercised: Vec<&str> = rows.iter().map(|row| row.action).collect();
    let mut sorted = exercised.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted, SERVED_ACTIONS,
        "case the served catalogue: every action this surface serves needs a row here, \
         got {exercised:?}"
    );

    for row in rows {
        let body = served(&router, row.action, &row.parameters).await;
        for member in row.result_members {
            assert!(
                body.contains(&format!("<{member}>")),
                "case {}: its result must report {member}, got {body}",
                row.action
            );
        }
        if row.action == "CreateDomain" || row.action == "DeleteDomain" {
            assert!(
                !body.contains(&format!("<{}Result>", row.action)),
                "case {}: this action reports nothing but ResponseMetadata, got {body}",
                row.action
            );
        }
    }
}

/// A domain created on the 2009-04-15 wire is the DOMAIN the v2 export operations
/// resolve, for every form of name the API admits — and deleting it on that wire takes
/// it away from them again.
#[tokio::test]
async fn a_domain_created_on_the_query_wire_is_the_domain_the_v2_export_resolves() {
    // One row per FORM of admitted domain name: the published minimum and maximum
    // lengths, and each punctuation character the published alphabet allows.
    let longest = "l".repeat(MAX_DOMAIN_NAME_LEN);
    let shortest = "d".repeat(MIN_DOMAIN_NAME_LEN);
    let rows: Vec<(&str, &str)> = vec![
        ("the shortest admitted name", shortest.as_str()),
        ("a plain lower-case name", "orders"),
        ("mixed case with every admitted punctuation", "Mixed-Case.Domain_1"),
        ("digits only", "2026"),
        ("the longest admitted name", longest.as_str()),
    ];

    for (case, domain) in rows {
        let router = emulator();

        // Before the create, the v2 export refuses the name: nothing holds it.
        let (status, body) = call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": BUCKET }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an uncreated domain must be refused by StartDomainExport, got \
             {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchDomainException",
            "case {case}: expected NoSuchDomainException, got {body:?}"
        );

        create_domain(&router, domain).await;

        // After it, the same v2 call is served, and the export names the domain.
        let started = ok_call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": BUCKET }),
        )
        .await;
        let export_arn = member(&started, "exportArn")
            .unwrap_or_else(|| panic!("case {case}: no exportArn in {started:?}"));
        let fetched = ok_call(
            &router,
            "GetExport",
            &json!({ "exportArn": export_arn.clone() }),
        )
        .await;
        assert_eq!(
            member(&fetched, "domainName").as_deref(),
            Some(domain),
            "case {case}: GetExport must report the created domain, got {fetched:?}"
        );

        // And the query wire enumerates the same domain.
        let listed = served(&router, "ListDomains", &[]).await;
        assert_eq!(
            texts_of(&listed, "DomainName"),
            vec![domain.to_string()],
            "case {case}: ListDomains must report the created domain, got {listed}"
        );

        // Deleting it on the query wire takes it away from the v2 export too.
        served(&router, "DeleteDomain", &[("DomainName", domain)]).await;
        let (status, body) = call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": BUCKET }),
        )
        .await;
        assert_eq!(
            error_code(&body),
            "NoSuchDomainException",
            "case {case}: a deleted domain must be refused again ({status}), got {body:?}"
        );
    }
}

/// `CreateDomain` is idempotent: a name the account already holds is answered the same
/// way, and the items that name already holds stay where they are.
#[tokio::test]
async fn creating_a_domain_twice_keeps_the_items_it_already_holds() {
    let router = emulator();
    create_domain(&router, "idempotent").await;
    put_attribute(&router, "idempotent", "item-1", "colour", "blue", false).await;
    create_domain(&router, "idempotent").await;

    assert_eq!(
        get_attributes(&router, "idempotent", "item-1").await,
        vec![("colour".to_string(), "blue".to_string())],
        "case a repeated create: the items the domain already held must survive it"
    );
    assert_eq!(
        texts_of(&served(&router, "ListDomains", &[]).await, "DomainName"),
        vec!["idempotent".to_string()],
        "case a repeated create: the domain must be listed exactly once"
    );
}

/// `DeleteDomain` on a name the account does not hold is served and changes nothing.
#[tokio::test]
async fn deleting_a_domain_that_does_not_exist_is_served_and_changes_nothing() {
    let router = emulator();
    create_domain(&router, "kept").await;
    served(&router, "DeleteDomain", &[("DomainName", "never-created")]).await;
    assert_eq!(
        texts_of(&served(&router, "ListDomains", &[]).await, "DomainName"),
        vec!["kept".to_string()],
        "case deleting an absent domain: the domains the account holds must be untouched"
    );
}

/// What `PutAttributes` writes is what `GetAttributes` reads, over the forms a pair can
/// take: a single value, several values under one name, a replacing write, text the API
/// admits at its published maximum, and non-ASCII text.
#[tokio::test]
async fn what_put_attributes_writes_is_what_get_attributes_reads() {
    let router = emulator();
    let domain = "attribute-domain";
    create_domain(&router, domain).await;

    let longest = "v".repeat(MAX_TEXT_BYTES);
    let rows: Vec<WriteCase<'_>> = vec![
        WriteCase {
            case: "one pair",
            writes: vec![("colour", "blue", false)],
            expected: vec![("colour", "blue")],
        },
        WriteCase {
            case: "a second value under one name, without Replace",
            writes: vec![("colour", "blue", false), ("colour", "green", false)],
            expected: vec![("colour", "blue"), ("colour", "green")],
        },
        WriteCase {
            case: "a replacing write over two values",
            writes: vec![
                ("colour", "blue", false),
                ("colour", "green", false),
                ("colour", "red", true),
            ],
            expected: vec![("colour", "red")],
        },
        WriteCase {
            case: "two names on one item",
            writes: vec![("colour", "blue", false), ("size", "large", false)],
            expected: vec![("colour", "blue"), ("size", "large")],
        },
        WriteCase {
            case: "a value at the published maximum length",
            writes: vec![("payload", longest.as_str(), false)],
            expected: vec![("payload", longest.as_str())],
        },
        WriteCase {
            case: "non-ASCII name and value",
            writes: vec![("\u{8272}", "\u{9752}\u{3044}", false)],
            expected: vec![("\u{8272}", "\u{9752}\u{3044}")],
        },
        WriteCase {
            case: "a value carrying the characters a form encodes",
            writes: vec![("expression", "a&b=c d+e%f", false)],
            expected: vec![("expression", "a&b=c d+e%f")],
        },
        WriteCase {
            case: "an empty value, which is a value",
            writes: vec![("empty", "", false)],
            expected: vec![("empty", "")],
        },
    ];

    for row in rows {
        // Each row is its own item, so no row can be served by what another row wrote.
        let case = row.case;
        let item = format!("item-{}", case.replace(' ', "-"));
        for (name, value, replace) in row.writes {
            put_attribute(&router, domain, &item, name, value, replace).await;
        }
        let read_back = get_attributes(&router, domain, &item).await;
        let expected: Vec<(String, String)> = row
            .expected
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            read_back, expected,
            "case {case}: expected {expected:?} back from GetAttributes, got {read_back:?}"
        );
    }
}

/// Several pairs in ONE `PutAttributes` call are all stored, and `AttributeName.N`
/// filters what `GetAttributes` reports to the names asked for.
#[tokio::test]
async fn a_single_call_writes_every_pair_and_a_read_may_filter_them() {
    let router = emulator();
    create_domain(&router, "multi").await;
    served(
        &router,
        "PutAttributes",
        &[
            ("DomainName", "multi"),
            ("ItemName", "item-1"),
            ("Attribute.0.Name", "colour"),
            ("Attribute.0.Value", "blue"),
            ("Attribute.1.Name", "size"),
            ("Attribute.1.Value", "large"),
            ("Attribute.2.Name", "colour"),
            ("Attribute.2.Value", "green"),
        ],
    )
    .await;

    assert_eq!(
        get_attributes(&router, "multi", "item-1").await,
        vec![
            ("colour".to_string(), "blue".to_string()),
            ("colour".to_string(), "green".to_string()),
            ("size".to_string(), "large".to_string()),
        ],
        "case three pairs in one call: every pair must be stored"
    );

    let filtered = served(
        &router,
        "GetAttributes",
        &[
            ("DomainName", "multi"),
            ("ItemName", "item-1"),
            ("AttributeName.0", "size"),
        ],
    )
    .await;
    assert_eq!(
        attribute_pairs(&filtered),
        vec![("size".to_string(), "large".to_string())],
        "case a filtered read: only the named attribute may be reported, got {filtered}"
    );

    // An item the domain does not hold is an empty result, not a refusal.
    let absent = served(
        &router,
        "GetAttributes",
        &[("DomainName", "multi"), ("ItemName", "never-written")],
    )
    .await;
    assert_eq!(
        attribute_pairs(&absent),
        Vec::new(),
        "case an item that was never written: expected an empty result, got {absent}"
    );
}

/// `DeleteAttributes` deletes exactly what the request names: one value, a whole
/// attribute, or the whole item.
#[tokio::test]
async fn delete_attributes_deletes_what_the_request_names() {
    let router = emulator();
    let domain = "delete-domain";
    create_domain(&router, domain).await;

    // One value of a multi-valued attribute.
    seed_item(&router, domain, "by-value").await;
    served(
        &router,
        "DeleteAttributes",
        &[
            ("DomainName", domain),
            ("ItemName", "by-value"),
            ("Attribute.0.Name", "colour"),
            ("Attribute.0.Value", "blue"),
        ],
    )
    .await;
    assert_eq!(
        get_attributes(&router, domain, "by-value").await,
        vec![
            ("colour".to_string(), "green".to_string()),
            ("size".to_string(), "large".to_string()),
        ],
        "case deleting one value: only that value may go"
    );

    // A whole attribute, named without a value.
    seed_item(&router, domain, "by-name").await;
    served(
        &router,
        "DeleteAttributes",
        &[
            ("DomainName", domain),
            ("ItemName", "by-name"),
            ("Attribute.0.Name", "colour"),
        ],
    )
    .await;
    assert_eq!(
        get_attributes(&router, domain, "by-name").await,
        vec![("size".to_string(), "large".to_string())],
        "case deleting a whole attribute: every value of that name must go"
    );

    // The whole item, named with no attribute at all.
    seed_item(&router, domain, "whole-item").await;
    served(
        &router,
        "DeleteAttributes",
        &[("DomainName", domain), ("ItemName", "whole-item")],
    )
    .await;
    assert_eq!(
        get_attributes(&router, domain, "whole-item").await,
        Vec::new(),
        "case deleting the whole item: nothing of it may remain"
    );

    // Deleting what the item does not hold removes nothing and is still served.
    seed_item(&router, domain, "untouched").await;
    served(
        &router,
        "DeleteAttributes",
        &[
            ("DomainName", domain),
            ("ItemName", "untouched"),
            ("Attribute.0.Name", "weight"),
        ],
    )
    .await;
    assert_eq!(
        get_attributes(&router, domain, "untouched").await.len(),
        3,
        "case deleting an attribute the item does not hold: nothing may go"
    );

    // An item whose every attribute is gone is gone itself, and the domain's metadata
    // counts it no more.
    let metadata = served(&router, "DomainMetadata", &[("DomainName", domain)]).await;
    assert_eq!(
        text_of(&metadata, "ItemCount").as_deref(),
        Some("3"),
        "case after the deletions: only the items that still hold attributes count, got \
         {metadata}"
    );
}

/// `DomainMetadata` counts what the domain holds: every figure is computed from its
/// items, and each grows only with what was actually stored.
#[tokio::test]
async fn domain_metadata_counts_what_the_domain_holds() {
    let router = emulator();
    create_domain(&router, "metadata").await;

    let empty = served(&router, "DomainMetadata", &[("DomainName", "metadata")]).await;
    for member in [
        "ItemCount",
        "ItemNamesSizeBytes",
        "AttributeNameCount",
        "AttributeNamesSizeBytes",
        "AttributeValueCount",
        "AttributeValuesSizeBytes",
    ] {
        assert_eq!(
            text_of(&empty, member).as_deref(),
            Some("0"),
            "case a domain nothing was written to: {member} must be 0, got {empty}"
        );
    }

    // Two items, one attribute name shared between them and three values in all.
    put_attribute(&router, "metadata", "aa", "colour", "blue", false).await;
    put_attribute(&router, "metadata", "aa", "colour", "green", false).await;
    put_attribute(&router, "metadata", "bbbb", "colour", "red", false).await;

    let filled = served(&router, "DomainMetadata", &[("DomainName", "metadata")]).await;
    // "aa" + "bbbb" is 6 bytes of item names; the one unique attribute name "colour" is
    // 6 bytes; the three values "blue", "green" and "red" are 12 bytes.
    for (member, expected) in [
        ("ItemCount", "2"),
        ("ItemNamesSizeBytes", "6"),
        ("AttributeNameCount", "1"),
        ("AttributeNamesSizeBytes", "6"),
        ("AttributeValueCount", "3"),
        ("AttributeValuesSizeBytes", "12"),
    ] {
        assert_eq!(
            text_of(&filled, member).as_deref(),
            Some(expected),
            "case two items with three values: {member} must be {expected}, got {filled}"
        );
    }
    assert!(
        text_of(&filled, "Timestamp").is_some_and(|stamp| stamp.parse::<u64>().is_ok()),
        "case a written domain: the metadata timestamp is Epoch seconds, got {filled}"
    );
}

/// `ListDomains` enumerates in name order, pages at the size the request asks for, and
/// continues from the token it issued.
#[tokio::test]
async fn list_domains_pages_in_name_order() {
    let router = emulator();
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    // Created out of order, so the enumeration's order is the store's and not the
    // order of creation.
    for domain in ["charlie", "alpha", "echo", "bravo", "delta"] {
        create_domain(&router, domain).await;
    }

    for page_size in [1_usize, 2, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut parameters: Vec<(&str, &str)> = vec![("MaxNumberOfDomains", size.as_str())];
            if let Some(cursor) = token.as_deref() {
                parameters.push(("NextToken", cursor));
            }
            let body = served(&router, "ListDomains", &parameters).await;
            let page = texts_of(&body, "DomainName");
            assert!(
                page.len() <= page_size,
                "case MaxNumberOfDomains={page_size}: a page may not exceed the size asked \
                 for, got {body}"
            );
            seen.extend(page);
            token = text_of(&body, "NextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case MaxNumberOfDomains={page_size}: the pages must enumerate every domain \
             once, in name order"
        );
    }

    // An unpaged call reports them all, and names no continuation.
    let whole = served(&router, "ListDomains", &[]).await;
    assert_eq!(
        texts_of(&whole, "DomainName"),
        names,
        "case no MaxNumberOfDomains: every domain must be reported, got {whole}"
    );
    assert_eq!(
        text_of(&whole, "NextToken"),
        None,
        "case a page that reached the end: no continuation token may be issued, got {whole}"
    );
}

/// The items written on the query wire are the items an export of that domain counts:
/// `GetExport` reports the domain's own item count once the export has completed.
#[tokio::test]
async fn an_export_counts_the_items_the_query_wire_wrote() {
    let router = emulator();
    create_domain(&router, "counted").await;
    for item in ["item-1", "item-2", "item-3"] {
        put_attribute(&router, "counted", item, "colour", "blue", false).await;
    }

    let started = ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "counted", "s3Bucket": BUCKET }),
    )
    .await;
    let export_arn = member(&started, "exportArn").expect("the start minted an ARN");

    // Poll for the status the clock owes rather than sleeping for it: what is asserted
    // is that the export completes, not how fast this host is.
    let request = json!({ "exportArn": export_arn });
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let succeeded = loop {
        let reported = ok_call(&router, "GetExport", &request).await;
        if member(&reported, "exportStatus").as_deref() == Some("SUCCEEDED") {
            break reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the export to complete: it never reached SUCCEEDED; last \
             report {reported:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    };
    assert_eq!(
        succeeded.get("itemsCount").and_then(serde_json::Value::as_i64),
        Some(3),
        "case an export of a domain holding three items: itemsCount must count them, got \
         {succeeded:?}"
    );
}

/// A read on this wire does not change what a later caller sees.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    create_domain(&router, "read-only").await;
    put_attribute(&router, "read-only", "item-1", "colour", "blue", false).await;

    let first_metadata = served(&router, "DomainMetadata", &[("DomainName", "read-only")]).await;
    let first_list = served(&router, "ListDomains", &[]).await;
    let first_item = get_attributes(&router, "read-only", "item-1").await;

    let second_metadata = served(&router, "DomainMetadata", &[("DomainName", "read-only")]).await;
    let second_list = served(&router, "ListDomains", &[]).await;
    let second_item = get_attributes(&router, "read-only", "item-1").await;

    for (case, first, second) in [
        ("DomainMetadata", first_metadata, second_metadata),
        ("ListDomains", first_list, second_list),
    ] {
        // The request id differs per response by design; everything the store answered
        // with must be identical.
        let without_id = |document: &str| {
            document
                .split("<RequestId>")
                .next()
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(
            without_id(&first),
            without_id(&second),
            "case a second {case}: a read must not change what a later caller sees"
        );
    }
    assert_eq!(
        first_item, second_item,
        "case a second GetAttributes: a read must not change what a later caller sees"
    );
}

/// A conditional write happens only when its `Expected` condition holds, and the refusal
/// names the published code for the way it failed. Nothing is written by a refused call.
#[tokio::test]
async fn a_conditional_write_is_judged_before_anything_is_written() {
    let router = emulator();
    let domain = "conditional";
    create_domain(&router, domain).await;
    put_attribute(&router, domain, "item-1", "colour", "blue", false).await;
    put_attribute(&router, domain, "multi", "colour", "blue", false).await;
    put_attribute(&router, domain, "multi", "colour", "green", false).await;

    // The condition that holds: the write lands.
    let (status, body) = query(
        &router,
        &[
            ("Action", "PutAttributes"),
            ("Version", API_VERSION),
            ("DomainName", domain),
            ("ItemName", "item-1"),
            ("Attribute.0.Name", "size"),
            ("Attribute.0.Value", "large"),
            ("Expected.0.Name", "colour"),
            ("Expected.0.Value", "blue"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case a condition that holds: expected HTTP 200, got {status} with {body}"
    );
    assert!(
        get_attributes(&router, domain, "item-1")
            .await
            .contains(&("size".to_string(), "large".to_string())),
        "case a condition that holds: the write must have landed"
    );

    // Every way a condition fails, with the code the API publishes for it.
    let rows: Vec<RefusalCase<'_>> = vec![
        RefusalCase {
            case: "the expected value is not the one held",
            item: "item-1",
            parameters: vec![("Expected.0.Name", "colour"), ("Expected.0.Value", "red")],
            status: 409,
            code: "ConditionalCheckFailed",
            message: "Conditional check failed. Attribute (colour) value is (blue) but was \
                      expected (red)"
                .to_string(),
        },
        RefusalCase {
            case: "the expected attribute does not exist",
            item: "item-1",
            parameters: vec![("Expected.0.Name", "weight"), ("Expected.0.Value", "heavy")],
            status: 404,
            code: "AttributeDoesNotExist",
            message: "Attribute (weight) does not exist".to_string(),
        },
        RefusalCase {
            case: "the attribute was expected not to exist",
            item: "item-1",
            parameters: vec![("Expected.0.Name", "colour"), ("Expected.0.Exists", "false")],
            status: 409,
            code: "ConditionalCheckFailed",
            message: "Conditional check failed. Attribute (colour) value exists.".to_string(),
        },
        RefusalCase {
            case: "a condition on a multi-valued attribute",
            item: "multi",
            parameters: vec![("Expected.0.Name", "colour"), ("Expected.0.Value", "blue")],
            status: 409,
            code: "MultiValuedAttribute",
            message: "Attribute (colour) is multi valued. Conditional check can only be \
                      performed on a single-valued attribute"
                .to_string(),
        },
        RefusalCase {
            case: "a value expected of an attribute expected not to exist",
            item: "item-1",
            parameters: vec![
                ("Expected.0.Name", "colour"),
                ("Expected.0.Value", "blue"),
                ("Expected.0.Exists", "false"),
            ],
            status: 400,
            code: "ExistsAndExpectedValue",
            message: "Expected.Exists=false and Expected.Value cannot be specified together"
                .to_string(),
        },
        RefusalCase {
            case: "an attribute expected to exist with no value named",
            item: "item-1",
            parameters: vec![("Expected.0.Name", "colour"), ("Expected.0.Exists", "true")],
            status: 400,
            code: "IncompleteExpectedExpression",
            message: "If Expected.Exists=true or unspecified, then Expected.Value has to be \
                      specified"
                .to_string(),
        },
    ];

    for row in rows {
        let mut parameters: Vec<(&str, &str)> = vec![
            ("Action", "PutAttributes"),
            ("Version", API_VERSION),
            ("DomainName", domain),
            ("ItemName", row.item),
            ("Attribute.0.Name", "weight"),
            ("Attribute.0.Value", "heavy"),
        ];
        parameters.extend(row.parameters);
        let (answered, answered_code, answered_message) = refused(&router, &parameters).await;
        assert_eq!(
            (answered, answered_code.as_str(), answered_message.as_str()),
            (row.status, row.code, row.message.as_str()),
            "case {}: expected ({}, {}, {}), got ({answered}, {answered_code}, \
             {answered_message})",
            row.case,
            row.status,
            row.code,
            row.message
        );
        assert!(
            !get_attributes(&router, domain, row.item)
                .await
                .contains(&("weight".to_string(), "heavy".to_string())),
            "case {}: a refused conditional write must write nothing",
            row.case
        );
    }
}

/// Every refusal of this API answers the status, the code and the sentence the API
/// publishes for it — one row per way a request can be wrong.
#[tokio::test]
async fn every_refusal_names_the_published_code_and_sentence() {
    let router = emulator();
    create_domain(&router, "held").await;

    let too_long_item = "i".repeat(MAX_TEXT_BYTES + 1);
    let too_short_domain = "d".repeat(MIN_DOMAIN_NAME_LEN - 1);
    let too_long_domain = "d".repeat(MAX_DOMAIN_NAME_LEN + 1);
    let domain_rule = format!(
        "DomainName must be between {MIN_DOMAIN_NAME_LEN} and {MAX_DOMAIN_NAME_LEN} \
         characters long and may contain only a-z, A-Z, 0-9, '_', '-' and '.'."
    );
    let page_rule = format!("MaxNumberOfDomains must be between 1 and {MAX_LIST_DOMAINS}.");

    let rows: Vec<RefusalCase<'_>> = vec![
        RefusalCase {
            case: "an action no Amazon SimpleDB API has",
            item: "",
            parameters: vec![("Action", "ConjureDomain"), ("Version", API_VERSION)],
            status: 400,
            code: "InvalidAction",
            message: "The action ConjureDomain is not valid for this web service.".to_string(),
        },
        RefusalCase {
            case: "an action of this API this emulator does not serve",
            item: "",
            parameters: vec![
                ("Action", "Select"),
                ("SelectExpression", "select * from held"),
            ],
            status: 501,
            code: "NotImplemented",
            message: "action Select is not implemented in this emulator".to_string(),
        },
        RefusalCase {
            case: "a version no Amazon SimpleDB API declares",
            item: "",
            parameters: vec![("Action", "ListDomains"), ("Version", "2007-11-07")],
            status: 400,
            code: "NoSuchVersion",
            message: "The requested version (2007-11-07) is not valid.".to_string(),
        },
        RefusalCase {
            case: "no DomainName at all",
            item: "",
            parameters: vec![("Action", "CreateDomain"), ("Version", API_VERSION)],
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter DomainName".to_string(),
        },
        RefusalCase {
            case: "a domain name below the published minimum length",
            item: "",
            parameters: vec![
                ("Action", "CreateDomain"),
                ("DomainName", too_short_domain.as_str()),
            ],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"{too_short_domain}\") for parameter DomainName is invalid. \
                 {domain_rule}"
            ),
        },
        RefusalCase {
            case: "a domain name past the published maximum length",
            item: "",
            parameters: vec![
                ("Action", "CreateDomain"),
                ("DomainName", too_long_domain.as_str()),
            ],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"{too_long_domain}\") for parameter DomainName is invalid. \
                 {domain_rule}"
            ),
        },
        RefusalCase {
            case: "a domain name carrying a character the published alphabet excludes",
            item: "",
            parameters: vec![("Action", "CreateDomain"), ("DomainName", "two words")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"two words\") for parameter DomainName is invalid. {domain_rule}"
            ),
        },
        RefusalCase {
            case: "PutAttributes naming a domain the account does not hold",
            item: "",
            parameters: vec![
                ("Action", "PutAttributes"),
                ("DomainName", "never-created"),
                ("ItemName", "item-1"),
                ("Attribute.0.Name", "colour"),
                ("Attribute.0.Value", "blue"),
            ],
            status: 400,
            code: "NoSuchDomain",
            message: "The specified domain does not exist.".to_string(),
        },
        RefusalCase {
            case: "GetAttributes naming a domain the account does not hold",
            item: "",
            parameters: vec![
                ("Action", "GetAttributes"),
                ("DomainName", "never-created"),
                ("ItemName", "item-1"),
            ],
            status: 400,
            code: "NoSuchDomain",
            message: "The specified domain does not exist.".to_string(),
        },
        RefusalCase {
            case: "DeleteAttributes naming a domain the account does not hold",
            item: "",
            parameters: vec![
                ("Action", "DeleteAttributes"),
                ("DomainName", "never-created"),
                ("ItemName", "item-1"),
            ],
            status: 400,
            code: "NoSuchDomain",
            message: "The specified domain does not exist.".to_string(),
        },
        RefusalCase {
            case: "DomainMetadata naming a domain the account does not hold",
            item: "",
            parameters: vec![("Action", "DomainMetadata"), ("DomainName", "never-created")],
            status: 400,
            code: "NoSuchDomain",
            message: "The specified domain does not exist.".to_string(),
        },
        RefusalCase {
            case: "PutAttributes with no ItemName",
            item: "",
            parameters: vec![
                ("Action", "PutAttributes"),
                ("DomainName", "held"),
                ("Attribute.0.Name", "colour"),
                ("Attribute.0.Value", "blue"),
            ],
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter ItemName".to_string(),
        },
        RefusalCase {
            case: "PutAttributes with no attribute to write",
            item: "",
            parameters: vec![
                ("Action", "PutAttributes"),
                ("DomainName", "held"),
                ("ItemName", "item-1"),
            ],
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter Attribute.0.Name".to_string(),
        },
        RefusalCase {
            case: "PutAttributes with an attribute that names no value",
            item: "",
            parameters: vec![
                ("Action", "PutAttributes"),
                ("DomainName", "held"),
                ("ItemName", "item-1"),
                ("Attribute.3.Name", "colour"),
            ],
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter Attribute.3.Value".to_string(),
        },
        RefusalCase {
            case: "an item name past the published maximum length",
            item: "",
            parameters: vec![
                ("Action", "GetAttributes"),
                ("DomainName", "held"),
                ("ItemName", too_long_item.as_str()),
            ],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"{too_long_item}\") for parameter ItemName is invalid. Value exceeds \
                 maximum length of {MAX_TEXT_BYTES}."
            ),
        },
        RefusalCase {
            case: "a page size below the published minimum",
            item: "",
            parameters: vec![("Action", "ListDomains"), ("MaxNumberOfDomains", "0")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"0\") for parameter MaxNumberOfDomains is invalid. {page_rule}"
            ),
        },
        RefusalCase {
            case: "a page size past the published maximum",
            item: "",
            parameters: vec![("Action", "ListDomains"), ("MaxNumberOfDomains", "101")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"101\") for parameter MaxNumberOfDomains is invalid. {page_rule}"
            ),
        },
        RefusalCase {
            case: "a page size that is not a number at all",
            item: "",
            parameters: vec![("Action", "ListDomains"), ("MaxNumberOfDomains", "many")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Value (\"many\") for parameter MaxNumberOfDomains is invalid. {page_rule}"
            ),
        },
        RefusalCase {
            case: "a continuation token this store did not issue",
            item: "",
            parameters: vec![
                ("Action", "ListDomains"),
                ("NextToken", "6e6f742d612d746f6b656e"),
            ],
            status: 400,
            code: "InvalidNextToken",
            message: "The specified next token is not valid.".to_string(),
        },
    ];

    for row in rows {
        let (answered, answered_code, answered_message) = refused(&router, &row.parameters).await;
        assert_eq!(
            (answered, answered_code.as_str(), answered_message.as_str()),
            (row.status, row.code, row.message.as_str()),
            "case {}: expected ({}, {}, {}), got ({answered}, {answered_code}, \
             {answered_message})",
            row.case,
            row.status,
            row.code,
            row.message
        );
    }

    // Nothing above created a domain: a refused call writes nothing.
    assert_eq!(
        texts_of(&served(&router, "ListDomains", &[]).await, "DomainName"),
        vec!["held".to_string()],
        "case after every refusal: no refused call may have created a domain"
    );
}

/// Every action of this API that this change does not serve is answered as not
/// implemented — never as a success, and never as an action this web service does not
/// have.
#[tokio::test]
async fn an_unserved_action_of_this_api_is_answered_as_not_implemented() {
    let router = emulator();
    for action in UNSERVED_ACTIONS {
        let (status, code, message) =
            refused(&router, &[("Action", action), ("Version", API_VERSION)]).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                501,
                "NotImplemented",
                format!("action {action} is not implemented in this emulator").as_str()
            ),
            "case {action}: an action of this API this emulator does not serve must be \
             answered as not implemented"
        );
    }
}

/// The two APIs on this endpoint stay apart: a request on the modelled `/v2/*` path is
/// served by the modelled operation even when its body would name an `Action`, and a
/// request on `"/"` naming no action is not claimed by this surface at all.
#[tokio::test]
async fn the_two_apis_on_this_endpoint_stay_apart() {
    let router = emulator();

    // The modelled path is still the modelled operation.
    let listed = ok_call(&router, "ListExports", &json!({})).await;
    assert!(
        listed.get("exportSummaries").is_some(),
        "case the modelled path: ListExports must still be served there, got {listed:?}"
    );

    // A root-path request naming no action is none of this API's business: it is left to
    // the framework, which answers this service's OWN protocol refusal — restJson1's
    // "not implemented" for a route the model does not bind — rather than a 2009-04-15
    // document.
    let (status, body) = query(&router, &[("Version", API_VERSION)]).await;
    assert!(
        !body.contains(SDB_NAMESPACE),
        "case a root-path request naming no action: this API must not claim it, got \
         {status} with {body}"
    );
    assert_eq!(
        (status, body.contains("NotImplemented")),
        (501, true),
        "case a root-path request naming no action: expected this service's own restJson1 \
         routing refusal, got {status} with {body}"
    );
}
