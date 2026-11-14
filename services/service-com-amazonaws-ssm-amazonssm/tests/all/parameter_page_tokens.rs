// SPDX-License-Identifier: CC0-1.0

//! Which `NextToken` a paginated read of Parameter Store resumes from, end to end over the
//! front door.
//!
//! RULE under test: **a `NextToken` is opaque and carries its own position, so a read
//! resumes only from a token it ISSUED: any other value — however exactly it is shaped like
//! a position, and including a token another read or another endpoint issued — is refused
//! with HTTP 400 `InvalidNextToken` naming the rejected token, never served as a page.** A
//! read that honoured a value of the right shape would hand a client that corrupted or
//! reused a token the first page of a scan it was paging past, which is how a paginator
//! loops or double-processes instead of failing fast.
//!
//! The table's rows differ along what the rule generalises over: the READ that is paged
//! (the hierarchy read, the metadata describe, the version history), the FORM of the value
//! offered as a token (prose, the position in the clear, an ARN, hex too short to carry an
//! authenticator, a token of another read, a token of another endpoint, a token of this
//! read with one character changed) and the page size paging walks at. Every read is also
//! paged through with the tokens it DID issue in the same pass, so no row passes against a
//! read that refuses every token.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message, router};

/// The parameter whose versions the history read pages through.
const VERSIONED: &str = "/Tok/versioned";

/// The hierarchy the two name-ordered reads page through.
const HIERARCHY: &str = "/Tok";

/// One paginated read: its operation name, and the request that asks for its first page.
struct Read {
    case: &'static str,
    operation: &'static str,
    request: fn(i64) -> Value,
    /// How many records the read walks, so paging can be checked to visit each once.
    records: usize,
}

/// The paginated reads of Parameter Store. Source: each operation's `paginated` trait
/// (`smithy_query(query="operation",
/// operation="GetParametersByPath,DescribeParameters,GetParameterHistory")`), all three of
/// which page with `NextToken` and `MaxResults`.
const READS: &[Read] = &[
    Read {
        case: "the hierarchy read",
        operation: "GetParametersByPath",
        request: |page_size| json!({ "Path": HIERARCHY, "Recursive": true, "MaxResults": page_size }),
        records: 4,
    },
    Read {
        case: "the metadata describe",
        operation: "DescribeParameters",
        request: |page_size| json!({ "MaxResults": page_size }),
        records: 4,
    },
    Read {
        case: "the version history",
        operation: "GetParameterHistory",
        request: |page_size| json!({ "Name": VERSIONED, "MaxResults": page_size }),
        records: 3,
    },
];

/// Paging with the tokens a read hands out visits every record exactly once, at every page
/// size, and the token never carries the position in the clear.
#[tokio::test]
async fn paging_with_the_tokens_a_read_issued_walks_every_record_once() {
    let router = router();
    seed(&router).await;

    for read in READS {
        for page_size in [1, 2, 3] {
            let case = format!("{} at page size {page_size}", read.case);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = (read.request)(page_size);
                if let Some(token) = &token {
                    body["NextToken"] = json!(token);
                }
                let (status, page) = call(&router, read.operation, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a page the read itself asked for is served, got \
                     {status} {page}"
                );
                let reported = identities(&page);
                assert!(
                    reported.len() as i64 <= page_size,
                    "case {case:?}: a page carries at most MaxResults records, got {page}"
                );
                seen.extend(reported);
                pages += 1;
                assert!(
                    pages <= read.records + 1,
                    "case {case:?}: paging must terminate, got {pages} pages"
                );
                match page["NextToken"].as_str() {
                    Some(next) => {
                        assert!(
                            !next.contains(HIERARCHY) && next.parse::<i64>().is_err(),
                            "case {case:?}: the token is opaque and carries no position in \
                             the clear, got {next}"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen.len(),
                read.records,
                "case {case:?}: paging visits every record exactly once, got {seen:?}"
            );
            let mut unique = seen.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                read.records,
                "case {case:?}: paging reports no record twice, got {seen:?}"
            );
        }
    }
}

/// A value this read did not issue names no page of it, whatever its form.
#[tokio::test]
async fn a_token_this_read_did_not_issue_is_refused() {
    let router = router();
    seed(&router).await;
    // A second endpoint on its own store: its tokens are authenticated under its own
    // secret, so they are no more this endpoint's than a value a client made up.
    let other_endpoint = crate::parameter_store::router();
    seed(&other_endpoint).await;

    for read in READS {
        let issued = first_token(&router, read).await;
        let elsewhere = first_token(&other_endpoint, read).await;
        let foreign_read = match read.operation {
            "GetParametersByPath" => first_token(&router, &READS[1]).await,
            _ => first_token(&router, &READS[0]).await,
        };
        let mut tampered = issued.clone();
        let flipped = if issued.starts_with('0') { '1' } else { '0' };
        tampered.replace_range(0..1, &flipped.to_string());

        let cases: Vec<(String, String)> = vec![
            ("prose".to_string(), "sim2real-probe-not-a-real-token".to_string()),
            ("a parameter name in the clear".to_string(), VERSIONED.to_string()),
            ("a hierarchy path in the clear".to_string(), HIERARCHY.to_string()),
            ("a version number in the clear".to_string(), "2".to_string()),
            (
                "an ARN of a parameter".to_string(),
                "arn:aws:ssm:us-west-2:000000000000:parameter/Tok/versioned".to_string(),
            ),
            (
                "hex too short to carry an authenticator".to_string(),
                "abcdef".to_string(),
            ),
            ("a token of this read with a character changed".to_string(), tampered),
            ("a token another read of this endpoint issued".to_string(), foreign_read),
            ("a token another endpoint issued".to_string(), elsewhere),
        ];

        for (case, token) in cases {
            let mut body = (read.request)(1);
            body["NextToken"] = json!(token);
            let (status, refused) = call(&router, read.operation, &body).await;
            assert_eq!(
                (status, error_code(&refused), error_message(&refused)),
                (
                    400,
                    "InvalidNextToken".to_string(),
                    format!("The following token is invalid: {token}.")
                ),
                "case {:?} / {case:?}: expected {} to refuse {token}, got {status} {refused}",
                read.case,
                read.operation
            );
        }

        // The token this read issued is still served, so the refusals above are about the
        // values and not about the read.
        let mut body = (read.request)(1);
        body["NextToken"] = json!(issued);
        let (status, served) = call(&router, read.operation, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: the token this read issued resumes it, got {status} {served}",
            read.case
        );
    }
}

/// An absent and an empty `NextToken` name no position, so each is the first page rather
/// than a refusal.
#[tokio::test]
async fn no_token_at_all_is_the_first_page() {
    let router = router();
    seed(&router).await;

    for read in READS {
        for (case, body) in [
            ("an absent token", (read.request)(1)),
            ("an empty token", {
                let mut body = (read.request)(1);
                body["NextToken"] = json!("");
                body
            }),
        ] {
            let (status, page) = call(&router, read.operation, &body).await;
            assert_eq!(
                status, 200,
                "case {:?} / {case:?}: expected the first page, got {status} {page}",
                read.case
            );
            assert_eq!(
                identities(&page).len(),
                1,
                "case {:?} / {case:?}: expected one record at MaxResults 1, got {page}",
                read.case
            );
        }
    }
}

/// The first token a read hands out at a page size of one.
async fn first_token(router: &MultiServiceRouter, read: &Read) -> String {
    let (status, page) = call(router, read.operation, &(read.request)(1)).await;
    assert_eq!(
        status, 200,
        "expected {} to serve a first page, got {status} {page}",
        read.operation
    );
    page["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "expected {} to leave records over and hand out a token, got {page}",
                read.operation
            )
        })
        .to_string()
}

/// What one page reports, as the identity of each record in it: the parameter's name and
/// the version reported for it, which is unique in every one of these reads (distinct names
/// for the name-ordered reads, distinct versions of one name for the history).
fn identities(page: &Value) -> Vec<String> {
    page["Parameters"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    format!(
                        "{}:{}",
                        record["Name"].as_str().unwrap_or_default(),
                        record["Version"].as_i64().unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The records the reads page through: four parameters of one hierarchy, one of which
/// carries three versions.
async fn seed(router: &MultiServiceRouter) {
    for leaf in ["a", "b", "c"] {
        put(router, &format!("{HIERARCHY}/{leaf}"), "seeded", false).await;
    }
    put(router, VERSIONED, "v1", false).await;
    put(router, VERSIONED, "v2", true).await;
    put(router, VERSIONED, "v3", true).await;
}

/// One parameter written through the front door.
async fn put(router: &MultiServiceRouter, name: &str, value: &str, overwrite: bool) {
    let (status, written) = call(
        router,
        "PutParameter",
        &json!({ "Name": name, "Type": "String", "Value": value, "Overwrite": overwrite }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected {name} to be written, got {status} {written}"
    );
}
