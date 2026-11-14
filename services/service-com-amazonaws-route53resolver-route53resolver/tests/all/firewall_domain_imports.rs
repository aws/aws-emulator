// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall domain-file import and for the
//! canonical form a domain list holds its domains in.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! Two rules are pinned here.
//!
//! * A domain written to a firewall domain list is held as the DNS-canonical,
//!   fully qualified name — rooted with exactly one trailing dot — whatever
//!   spelling the request wrote, and every reader of the list projects that
//!   form; the rooting is idempotent, and membership is decided on the
//!   canonical name, so a removal written in either spelling removes the entry.
//!   It holds for every write path: `UpdateFirewallDomains` and
//!   `ImportFirewallDomains` alike.
//! * `ImportFirewallDomains` reads the domain file out of the account's own
//!   Amazon S3 inventory ("the file stored in Amazon Simple Storage Service
//!   (Amazon S3) that contains the list of domains to import ... must be a text
//!   file and must contain a single domain per line"): a URL that addresses a
//!   readable file makes the list exactly match it, and a URL that addresses no
//!   file DNS Firewall can download leaves the list's domains untouched and
//!   terminates the list in `COMPLETE_IMPORT_FAILED`, stating the download
//!   failure, which is a SETTLED step — the list can be imported into again and
//!   deleted afterwards.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    firewall_domain_list_status_is_settled, MAX_DOMAINS_PER_LIST,
};
use service_com_amazonaws_s3_amazons3::state::{service_state as s3_state, Object};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_type, ok, string_member, REGION};

/// The bucket every case below stores its domain file in, held by the account
/// in [`REGION`] — "the file must be in an S3 bucket that's in the same Region
/// as your DNS Firewall".
pub(crate) const BUCKET: &str = "dns-firewall-domain-files";

/// The `FirewallDomainListStatus` values the model declares
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus")`), which any
/// status a response reports must be one of.
const DOMAIN_LIST_STATUSES: &[&str] = &[
    "COMPLETE",
    "COMPLETE_IMPORT_FAILED",
    "IMPORTING",
    "DELETING",
    "UPDATING",
];

/// The sentence DNS Firewall states on a domain list whose import could not
/// download the file the request named: fixed text whose only variable part is
/// the Region of the request.
fn download_failed(region: &str) -> String {
    format!(
        "Failed to download the file from provided url, please try with a smaller file or copy \
         the file into a {region} S3 bucket."
    )
}

/// One emulator: this service beside the emulator's own Amazon S3, over ONE
/// shared engine, so the domain file an import reads is an object of the
/// account's real inventory and not a value written out here.
pub(crate) fn emulator_with_object_storage() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Store `body` at `key` of [`BUCKET`], through Amazon S3's own store — the one
/// map its `CreateBucket` writes and its `GetObject` reads — so the import
/// finds exactly the object the account holds.
pub(crate) fn hold_object(engine: &Arc<engine::InMemoryEngine>, key: &str, body: Vec<u8>) {
    let state = s3_state(&**engine).expect("the engine holds every mounted service's store");
    let mut store = state.lock();
    store.create_bucket(BUCKET, REGION);
    let written = store.put_object(BUCKET, key, Object::stored(body, Some("text/plain".into())));
    assert!(
        written.is_some(),
        "the harness must be able to store the domain file at {key:?}, got {written:?}"
    );
}

/// Create a domain list and return its id.
pub(crate) async fn seed_list(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("req-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create response carries a FirewallDomainList"),
        "Id",
    )
}

/// Apply one `UpdateFirewallDomains` call that must succeed.
pub(crate) async fn update(
    router: &MultiServiceRouter,
    id: &str,
    operation: &str,
    domains: &[&str],
) -> Value {
    ok(
        router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": id,
            "Operation": operation,
            "Domains": domains,
        }),
    )
    .await
}

/// The `FirewallDomainList` of a `GetFirewallDomainList` response.
pub(crate) async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    let fetched = ok(
        router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": id }),
    )
    .await;
    fetched
        .get("FirewallDomainList")
        .expect("the read carries a FirewallDomainList")
        .clone()
}

/// Every domain the list holds, in the order it reports them.
async fn list_domains(router: &MultiServiceRouter, id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id, "MaxResults": 5_000 }),
    )
    .await;
    listed
        .get("Domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the response carries a Domains array, got {listed}"))
        .iter()
        .map(|domain| {
            domain
                .as_str()
                .unwrap_or_else(|| panic!("a domain is a string, got {domain}"))
                .to_string()
        })
        .collect()
}

/// How long a poll waits before it looks at the list again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long a poll keeps looking before it gives up: generous, so how fast the
/// host runs never decides the outcome.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// Look at the list again until the change in flight has settled, so the step
/// it settles in is the one the case reads — the CONDITION is waited for and
/// never a span of the clock.
async fn settle(router: &MultiServiceRouter, id: &str, case: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let status = string_member(&read_list(router, id).await, "Status");
        if firewall_domain_list_status_is_settled(&status) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the list must settle, it still reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Call one operation with a credential scope naming `region`, and return
/// `(http status, parsed body)` — the Region is what roots this service's ARNs
/// and what the download failure names, so a case that varies it signs for it.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{region}/route53resolver/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Start one import that must be accepted, and assert it reports the step an
/// import in flight stands in.
async fn import(router: &MultiServiceRouter, id: &str, url: &str, case: &str) {
    let answered = ok(
        router,
        "ImportFirewallDomains",
        json!({
            "FirewallDomainListId": id,
            "Operation": "REPLACE",
            "DomainFileUrl": url,
        }),
    )
    .await;
    assert_eq!(
        string_member(&answered, "Id"),
        id,
        "case {case:?}: the import must report the list it is importing into, got {answered}"
    );
    let status = string_member(&answered, "Status");
    assert!(
        DOMAIN_LIST_STATUSES.contains(&status.as_str()),
        "case {case:?}: Status must be a modelled FirewallDomainListStatus, got {status:?}"
    );
    assert_eq!(
        status, "IMPORTING",
        "case {case:?}: an accepted import is under way when it answers, got {answered}"
    );
}

/// One row of the import table: how the URL addresses the file, what the file
/// holds, and the domains the list must hold once the import has settled.
struct ImportCase<'a> {
    case: &'a str,
    /// The key the file is stored at, and the URL form the request names it by.
    key: &'a str,
    url: String,
    body: &'a str,
    expected: Vec<&'a str>,
}

/// A readable domain file makes the list exactly match it, in the canonical
/// rooted form, and the list settles in `COMPLETE` with that many domains.
///
/// The rows span what the rule generalises over: each addressing form of an S3
/// object (the `s3://` URI, the virtual-hosted HTTPS URL and the path-style
/// one), a file holding one domain and one holding many, keys at the bucket
/// root and under a prefix, and the spellings a line comes in — unrooted,
/// already rooted, upper case, a wildcard, an underscored label, a single
/// label, blank lines and surrounding whitespace, and the same domain twice.
#[tokio::test]
async fn an_imported_domain_file_replaces_the_lists_domains() {
    let (router, engine) = emulator_with_object_storage();
    let cases = vec![
        ImportCase {
            case: "the s3 URI, one domain",
            key: "one.txt",
            url: format!("s3://{BUCKET}/one.txt"),
            body: "ads.example.com\n",
            expected: vec!["ads.example.com."],
        },
        ImportCase {
            case: "the virtual-hosted URL, three domains",
            key: "lists/three.txt",
            url: format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/lists/three.txt"),
            body: "ads.example.com\nmalware.example.org\ntracker.example.net\n",
            expected: vec![
                "ads.example.com.",
                "malware.example.org.",
                "tracker.example.net.",
            ],
        },
        ImportCase {
            case: "the path-style URL, spellings and blank lines",
            key: "mixed.txt",
            url: format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/mixed.txt"),
            body: "\n  rooted.example.com.  \nUPPER.example.com\n*.wild.example.com\n\
                   _dmarc.example.com\nlocalhost\nrooted.example.com\n",
            expected: vec![
                "rooted.example.com.",
                "upper.example.com.",
                "*.wild.example.com.",
                "_dmarc.example.com.",
                "localhost.",
            ],
        },
        ImportCase {
            case: "a file with no domains at all",
            key: "empty.txt",
            url: format!("s3://{BUCKET}/empty.txt"),
            body: "\n\n",
            expected: vec![],
        },
    ];

    for ImportCase {
        case,
        key,
        url,
        body,
        expected,
    } in cases
    {
        hold_object(&engine, key, body.as_bytes().to_vec());
        let id = seed_list(&router, &format!("import-{}", key.replace(['/', '.'], "-"))).await;
        // The list holds a domain of its own first, so `REPLACE` is observed to
        // make it match the FILE rather than to add to it.
        update(&router, &id, "ADD", &["left-over.example.com"]).await;

        import(&router, &id, &url, case).await;
        settle(&router, &id, case).await;

        let read = read_list(&router, &id).await;
        assert_eq!(
            string_member(&read, "Status"),
            "COMPLETE",
            "case {case:?}: an import of a readable file settles complete, got {read}"
        );
        assert_eq!(
            read.get("DomainCount").and_then(Value::as_i64),
            Some(i64::try_from(expected.len()).expect("a small count")),
            "case {case:?}: DomainCount is the number of domains the file listed, got {read}"
        );
        let domains = list_domains(&router, &id).await;
        assert_eq!(
            domains, expected,
            "case {case:?}: expected the canonical domains {expected:?}, got {domains:?}"
        );
        for domain in &domains {
            assert!(
                domain.ends_with('.') && !domain.ends_with(".."),
                "case {case:?}: every stored domain is rooted with exactly one dot, got {domain:?}"
            );
        }
    }
}

/// One row of the failed-import table: the URL the request names and why it
/// addresses no file this emulator can download.
struct FailedImportCase {
    case: &'static str,
    url: String,
}

/// A URL that addresses no downloadable file leaves the list's domains exactly
/// as they were and terminates the list in `COMPLETE_IMPORT_FAILED`, stating
/// the download failure for the Region of the request.
///
/// The rows span the ways an S3 address resolves to no readable file: a bucket
/// the account does not hold, a key nothing was stored at, an object that is
/// not text, and a file listing more domains than a list can hold ("please try
/// with a smaller file"). A value that is not an S3 address at all starts no
/// import to fail — it is refused outright, which
/// [`super::firewall_domain_import_refusals`] pins.
#[tokio::test]
async fn an_import_of_a_file_that_cannot_be_downloaded_fails_with_the_download_message() {
    let (router, engine) = emulator_with_object_storage();

    hold_object(&engine, "not-text.bin", vec![0xff, 0xfe, 0x00, 0x01]);
    let too_many: String = (0..=MAX_DOMAINS_PER_LIST)
        .map(|index| format!("d{index}.example.com\n"))
        .collect();
    hold_object(&engine, "too-many.txt", too_many.into_bytes());

    let cases = vec![
        FailedImportCase {
            case: "a bucket the account does not hold",
            url: "s3://no-such-bucket-here/domains.txt".to_string(),
        },
        FailedImportCase {
            case: "a key nothing was stored at",
            url: format!("s3://{BUCKET}/never-written.txt"),
        },
        FailedImportCase {
            case: "an object that is not a text file",
            url: format!("s3://{BUCKET}/not-text.bin"),
        },
        FailedImportCase {
            case: "a file listing more domains than a list holds",
            url: format!("s3://{BUCKET}/too-many.txt"),
        },
    ];

    // The Region is the one variable part of the sentence, so the rows are
    // walked once per signing Region.
    for region in [REGION, "eu-central-1"] {
        for FailedImportCase { case, url } in &cases {
            let case = format!("{case} signed for {region}");
            let id = seed_list(
                &router,
                &format!("failed-{region}-{}", url.len() + case.len()),
            )
            .await;
            let held = ["kept.example.com", "also-kept.example.net"];
            update(&router, &id, "REPLACE", &held).await;

            let (status, answered) = call_in_region(
                &router,
                region,
                "ImportFirewallDomains",
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": "REPLACE",
                    "DomainFileUrl": url.clone(),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the import is accepted and fails on the download, got {status}: {answered}"
            );
            assert_eq!(
                string_member(&answered, "Status"),
                "IMPORTING",
                "case {case:?}: an accepted import is under way when it answers, got {answered}"
            );

            settle(&router, &id, &case).await;

            let read = read_list(&router, &id).await;
            assert_eq!(
                string_member(&read, "Status"),
                "COMPLETE_IMPORT_FAILED",
                "case {case:?}: a failed import must terminate in the failed step, got {read}"
            );
            let message = string_member(&read, "StatusMessage");
            assert_eq!(
                message,
                download_failed(region),
                "case {case:?}: the list must state the download failure as AWS words it, got {read}"
            );
            assert!(
                message.len() <= 255,
                "case {case:?}: StatusMessage must satisfy its modelled length 0..255, got {message:?}"
            );

            // Nothing was imported: the list still holds exactly what it held.
            let domains = list_domains(&router, &id).await;
            assert_eq!(
                domains,
                held.iter()
                    .map(|domain| format!("{domain}."))
                    .collect::<Vec<_>>(),
                "case {case:?}: a failed import must leave the domains alone, got {domains:?}"
            );
            assert_eq!(
                read.get("DomainCount").and_then(Value::as_i64),
                Some(2),
                "case {case:?}: DomainCount follows the domains the list still holds, got {read}"
            );

            // The failed step is SETTLED: the list takes another import, and
            // the delete that ends a teardown is served on it.
            hold_object(&engine, "retry.txt", b"retried.example.com\n".to_vec());
            import(
                &router,
                &id,
                &format!("s3://{BUCKET}/retry.txt"),
                &format!("{case} (retried)"),
            )
            .await;
            settle(&router, &id, &case).await;
            let retried = list_domains(&router, &id).await;
            assert_eq!(
                retried,
                vec!["retried.example.com.".to_string()],
                "case {case:?}: a list that failed an import takes the next one, got {retried:?}"
            );

            let deleted = ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            )
            .await;
            assert_eq!(
                string_member(
                    deleted
                        .get("FirewallDomainList")
                        .unwrap_or_else(|| panic!("the delete answers the list, got {deleted}")),
                    "Id"
                ),
                id,
                "case {case:?}: the delete must answer the list it took out, got {deleted}"
            );
        }
    }
}

/// An import naming a list nothing is stored under, or a list DNS Firewall
/// publishes itself, is refused with the `ResourceNotFoundException` the
/// operation models — a published list is not the account's to write, so it
/// answers exactly as a list that does not exist.
#[tokio::test]
async fn an_import_into_a_list_that_is_not_the_callers_is_refused() {
    let (router, engine) = emulator_with_object_storage();
    hold_object(&engine, "refused.txt", b"example.com\n".to_vec());
    let url = format!("s3://{BUCKET}/refused.txt");

    let published = ok(&router, "ListFirewallDomainLists", json!({ "MaxResults": 100 }))
        .await
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the published lists")
        .iter()
        .filter(|item| item.get("ManagedOwnerName").is_some())
        .map(|item| string_member(item, "Id"))
        .next()
        .expect("at least one published list");

    for (case, id) in [
        ("an id nothing minted", "rslvr-fdl-0000000000000000".to_string()),
        ("a published list", published),
    ] {
        let (status, body) = call(
            &router,
            "ImportFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "REPLACE",
                "DomainFileUrl": url.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the import must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must name ResourceNotFoundException, got {body}"
        );
        assert!(
            string_member(&body, "Message").contains(&id),
            "case {case:?}: the refusal must name the id that was not found, got {body}"
        );
    }
}

/// A file line that is not a domain specification is refused with the
/// `ValidationException` the operation models, and the list is left alone.
#[tokio::test]
async fn an_imported_file_that_is_not_a_domain_list_is_refused() {
    let (router, engine) = emulator_with_object_storage();
    let id = seed_list(&router, "not-domains").await;
    update(&router, &id, "ADD", &["kept.example.com"]).await;

    // The forms `Domains`' own documentation excludes: a space inside a name,
    // a character outside the documented set, an empty label and a label past
    // the 63 octets DNS allows.
    for (case, body) in [
        ("a line with a space in it", "exa mple.com\n"),
        ("a line with a character the doc excludes", "exa/mple.com\n"),
        ("a line with an empty label", "a..example.com\n"),
        (
            "a line with a label past 63 octets",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.example.com\n",
        ),
        ("a document that is not a domain list", "{\"domains\": []}\n"),
    ] {
        hold_object(&engine, "bad.txt", body.as_bytes().to_vec());
        let (status, answered) = call(
            &router,
            "ImportFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "REPLACE",
                "DomainFileUrl": format!("s3://{BUCKET}/bad.txt"),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a file that is not a domain list must be refused, got {status}: {answered}"
        );
        assert!(
            error_type(&answered).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {answered}"
        );
        let read = read_list(&router, &id).await;
        assert_eq!(
            string_member(&read, "Status"),
            "COMPLETE",
            "case {case:?}: the refused import must leave the list settled, got {read}"
        );
        let domains = list_domains(&router, &id).await;
        assert_eq!(
            domains,
            vec!["kept.example.com.".to_string()],
            "case {case:?}: the refused import must write nothing, got {domains:?}"
        );
    }
}

/// One row of the canonical-form table: the spelling written and the spelling
/// every reader of the list reports it in.
struct SpellingCase<'a> {
    case: &'a str,
    written: &'a str,
    stored: &'a str,
}

/// Whatever spelling a write uses, the list holds and reports the canonical
/// name — rooted and case folded — and a removal written in ANY of those
/// spellings removes that one entry, so the spellings are one domain rather
/// than several.
///
/// The rows span the spellings of a name: unrooted, already rooted, mixed case,
/// upper case, a wildcard, an asterisk without a separator, an underscored
/// label, a single label, a punycode/ACE name that case folding leaves alone,
/// and a long many-label name.
#[tokio::test]
async fn every_write_path_holds_the_canonical_rooted_name() {
    let (router, engine) = emulator_with_object_storage();
    let cases = vec![
        SpellingCase {
            case: "an unrooted name",
            written: "ads.example.com",
            stored: "ads.example.com.",
        },
        SpellingCase {
            case: "a name already rooted",
            written: "ads.example.com.",
            stored: "ads.example.com.",
        },
        SpellingCase {
            case: "a name in mixed case",
            written: "Ads.Example.COM",
            stored: "ads.example.com.",
        },
        SpellingCase {
            case: "a rooted name in upper case",
            written: "ADS.EXAMPLE.COM.",
            stored: "ads.example.com.",
        },
        SpellingCase {
            case: "a punycode name case folding leaves alone",
            written: "xn--e1afmkfd.xn--80akhbyknj4f",
            stored: "xn--e1afmkfd.xn--80akhbyknj4f.",
        },
        SpellingCase {
            case: "a wildcard",
            written: "*.example.com",
            stored: "*.example.com.",
        },
        SpellingCase {
            case: "an asterisk without a separator",
            written: "*example.com",
            stored: "*example.com.",
        },
        SpellingCase {
            case: "an underscored label",
            written: "_dmarc.example.com",
            stored: "_dmarc.example.com.",
        },
        SpellingCase {
            case: "a single label",
            written: "localhost",
            stored: "localhost.",
        },
        SpellingCase {
            case: "a many-label name",
            written: "a.b.c.d.e.deep.example.com",
            stored: "a.b.c.d.e.deep.example.com.",
        },
    ];

    for (index, SpellingCase {
        case,
        written,
        stored,
    }) in cases.into_iter().enumerate()
    {
        // Every write path that puts a domain in a list: the two update
        // operations that add one, and the import.
        let add = format!("ADD-{index}");
        let replace = format!("REPLACE-{index}");
        let imported = format!("IMPORT-{index}");

        let by_add = seed_list(&router, &add).await;
        update(&router, &by_add, "ADD", &[written]).await;

        let by_replace = seed_list(&router, &replace).await;
        update(&router, &by_replace, "REPLACE", &[written]).await;

        let key = format!("spelling-{index}.txt");
        hold_object(&engine, &key, format!("{written}\n").into_bytes());
        let by_import = seed_list(&router, &imported).await;
        import(
            &router,
            &by_import,
            &format!("s3://{BUCKET}/{key}"),
            &format!("{case} (import)"),
        )
        .await;
        settle(&router, &by_import, case).await;

        for (path, id) in [
            ("ADD", &by_add),
            ("REPLACE", &by_replace),
            ("IMPORT", &by_import),
        ] {
            let case = format!("{case} written by {path}");
            let domains = list_domains(&router, id).await;
            assert_eq!(
                domains,
                vec![stored.to_string()],
                "case {case:?}: expected the canonical name {stored:?}, got {domains:?}"
            );
            assert_eq!(
                read_list(&router, id)
                    .await
                    .get("DomainCount")
                    .and_then(Value::as_i64),
                Some(1),
                "case {case:?}: the two spellings of one name are one entry"
            );
        }

        // The write is idempotent over the spelling: adding the OTHER spelling
        // of the same name adds nothing, and removing it removes the entry.
        let other = if written.ends_with('.') {
            written.trim_end_matches('.').to_string()
        } else {
            format!("{written}.")
        };
        update(&router, &by_add, "ADD", &[other.as_str()]).await;
        let after_add = list_domains(&router, &by_add).await;
        assert_eq!(
            after_add,
            vec![stored.to_string()],
            "case {case:?}: the other spelling of a stored name adds nothing, got {after_add:?}"
        );
        update(&router, &by_add, "REMOVE", &[other.as_str()]).await;
        let after_remove = list_domains(&router, &by_add).await;
        assert!(
            after_remove.is_empty(),
            "case {case:?}: a removal written in the other spelling must remove the entry, got {after_remove:?}"
        );
    }
}

/// The teardown AWS documents for a rule group — delete its rules, then the
/// group, then the domain lists they referenced — is served end to end: each
/// delete answers HTTP 200 with the record it took out, and the group reports
/// no rules left.
///
/// The rows are the number of rules the group held, so the delete is exercised
/// on an empty group and on groups whose rules were removed one at a time.
#[tokio::test]
async fn a_rule_group_and_the_lists_its_rules_referenced_can_be_torn_down() {
    let (router, _engine) = emulator_with_object_storage();

    for rules in [0_usize, 1, 3] {
        let case = format!("a group that held {rules} rule(s)");
        let group = string_member(
            ok(
                &router,
                "CreateFirewallRuleGroup",
                json!({
                    "CreatorRequestId": format!("req-group-{rules}"),
                    "Name": format!("teardown-{rules}"),
                }),
            )
            .await
            .get("FirewallRuleGroup")
            .unwrap_or_else(|| panic!("case {case:?}: the create answers a FirewallRuleGroup")),
            "Id",
        );

        // One domain list per rule, each referenced by the rule that matches
        // it — the shape a rule group takes in use, and what makes every rule
        // addressable by the source it matches.
        let mut lists: Vec<String> = Vec::new();
        for index in 0..rules {
            let list = seed_list(&router, &format!("teardown-list-{rules}-{index}")).await;
            update(&router, &list, "ADD", &["blocked.example.com"]).await;
            ok(
                &router,
                "CreateFirewallRule",
                json!({
                    "CreatorRequestId": format!("req-rule-{rules}-{index}"),
                    "FirewallRuleGroupId": group.clone(),
                    "FirewallDomainListId": list.clone(),
                    "Name": format!("rule-{rules}-{index}"),
                    "Priority": 100 + index * 10,
                    "Action": "ALERT",
                }),
            )
            .await;
            lists.push(list);
        }
        let listed = ok(
            &router,
            "ListFirewallRules",
            json!({ "FirewallRuleGroupId": group.clone() }),
        )
        .await;
        assert_eq!(
            listed
                .get("FirewallRules")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            rules,
            "case {case:?}: the group must report the rules it was created with, got {listed}"
        );

        // AWS's documented order: the rules first, then the group, then the
        // domain lists they referenced. A group that still holds a rule is not
        // in a state it can be deleted from, and says so.
        if rules > 0 {
            let (status, refusal) = call(
                &router,
                "DeleteFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": group.clone() }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a group that still holds a rule cannot be deleted, got {status}: {refusal}"
            );
            assert!(
                error_type(&refusal).contains("ConflictException"),
                "case {case:?}: the refusal must name ConflictException, got {refusal}"
            );
            assert_eq!(
                string_member(&refusal, "Message"),
                format!(
                    "[RSLVR-02103] The DNS Firewall rule group has an associated rule: \
                     \"{group}\".  Delete the rule and try again later."
                ),
                "case {case:?}: the refusal must state the fault as AWS words it, got {refusal}"
            );
            // The refusal wrote nothing: the group is still there with its
            // rules.
            let kept = ok(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": group.clone() }),
            )
            .await;
            assert_eq!(
                kept.get("FirewallRuleGroup")
                    .and_then(|group| group.get("RuleCount"))
                    .and_then(Value::as_i64),
                Some(i64::try_from(rules).expect("a small count")),
                "case {case:?}: the refused delete must leave the group and its rules, got {kept}"
            );
        }

        for (index, list) in lists.iter().enumerate() {
            ok(
                &router,
                "DeleteFirewallRule",
                json!({
                    "FirewallRuleGroupId": group.clone(),
                    "FirewallDomainListId": list.clone(),
                }),
            )
            .await;
            let left = ok(
                &router,
                "ListFirewallRules",
                json!({ "FirewallRuleGroupId": group.clone() }),
            )
            .await;
            assert_eq!(
                left.get("FirewallRules")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
                rules - index - 1,
                "case {case:?}: each delete must take exactly one rule out, got {left}"
            );
        }

        let deleted = ok(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": group.clone() }),
        )
        .await;
        let projected = deleted
            .get("FirewallRuleGroup")
            .unwrap_or_else(|| panic!("case {case:?}: the delete answers the group, got {deleted}"));
        assert_eq!(
            string_member(projected, "Id"),
            group,
            "case {case:?}: the delete must answer the group it took out, got {deleted}"
        );
        assert_eq!(
            projected.get("RuleCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a group whose rules were deleted holds none, got {deleted}"
        );

        for list in &lists {
            let dropped = ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": list.clone() }),
            )
            .await;
            assert_eq!(
                string_member(
                    dropped
                        .get("FirewallDomainList")
                        .unwrap_or_else(|| panic!("case {case:?}: the delete answers the list")),
                    "Id"
                ),
                *list,
                "case {case:?}: a list its rules no longer reference must be deletable, got {dropped}"
            );
        }
    }
}
