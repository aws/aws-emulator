// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of the identifier refusal of
//! `CreateDBClusterEndpoint`.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on. The assertions read the XML the emulator answers with, so a handler
//! that computed nothing could not pass them.
//!
//! The rule under test: a `DBClusterEndpointIdentifier` outside the identifier rule this
//! front door holds the member to is refused `InvalidParameterValue` (HTTP 400) with the
//! SUBJECT sentence this front door words that member by — `Invalid cluster endpoint
//! identifier: ` followed by the value exactly as the request spelled it (the two spaces
//! after the colon are part of the sentence) — rather than with the sentence pair that
//! states the identifier rule and names the member, which is what the members that carry no
//! subject of their own answer. The value is the only variable part, and it is echoed as
//! SENT: the refusal is about what the caller wrote, not about the folded spelling the
//! endpoint would have been stored under.
//!
//! The table spans what the rule generalises over: a character the identifier alphabet
//! excludes (an underscore, a punctuation mark, a space, a multi-byte letter), a value whose
//! first character is not a letter, each of the two hyphen clauses, a value one character past
//! the published bound, and a value of no characters at all — and the refusal is the same
//! sentence whichever control plane's cluster the create names, whether that cluster exists
//! at all (the member is checked before anything is resolved) and whichever `EndpointType`
//! is asked for. The rows that are WELL formed are served, so the gate refuses the values the
//! rule excludes rather than everything: a one-character name, a mixed-case spelling, digits
//! and hyphens, and a value at the published bound.
//!
//! Nothing here waits out a span of wall-clock: the created endpoints are read back through
//! the shared poll loop ([`crate::lifecycle_poll_loops::poll_until`]).
//!
//! The last test reads the two NOT-FOUND sentences of this same API — the source-cluster one
//! the report and the create answer, and the endpoint one the modification and the delete
//! answer — through the GRAPH control plane's view of this one front door, which is where the
//! wording of those refusals is read by a client of that view: each is formatted from the
//! request rather than from the error shape's documentation, and the member gate above runs
//! first, so a create carrying a well-formed identifier for a cluster that is not there reads
//! the cluster's sentence and not the member's.

use service_com_amazonaws_rds_amazonrdsv19::state::MAX_DB_CLUSTER_ENDPOINT_IDENTIFIER_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, subtrees, text};
use crate::lifecycle_poll_loops::poll_until;

/// The Aurora DB cluster the served rows hang their endpoints off.
const AURORA_CLUSTER: &str = "emu-ceirw-aurora";

/// A cluster of another control plane behind this one front door — the graph engine — so a
/// row proves the refusal is the member's and not one control plane's.
const GRAPH_CLUSTER: &str = "emu-ceirw-graph";

/// A cluster identifier nothing creates: a create naming it still answers the IDENTIFIER
/// refusal, because the member is checked before any cluster is resolved.
const ABSENT_CLUSTER: &str = "emu-ceirw-absent";

/// The error code a value outside its member's published constraints is reported with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The sentence this front door words `DBClusterEndpointIdentifier` by: its subject, two
/// spaces, and the value the request sent.
fn invalid_endpoint_identifier(named: &str) -> String {
    format!("Invalid cluster endpoint identifier:  {named}")
}

/// The sentence pair that states the identifier rule and names the member instead of echoing
/// the value — the wording the members with no subject of their own answer, which this member
/// must NOT answer.
fn the_rule_sentence(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with a \
         hyphen or contain two consecutive hyphens."
    )
}

/// `prefix` grown to `length` characters with the letter `x`, so a row sends the published
/// bound (or one character past it) without a hand-counted literal.
fn padded_to(prefix: &str, length: usize) -> String {
    let mut named = prefix.to_string();
    while named.chars().count() < length {
        named.push('x');
    }
    assert_eq!(
        named.chars().count(),
        length,
        "the padded identifier must sit at {length} characters, got {named:?}"
    );
    named
}

/// `CreateDBCluster` for one of the clusters above.
///
/// An Aurora cluster is created with a master user, as a client that means to connect to it
/// does; a graph cluster takes neither member — Amazon Neptune assigns the master user itself
/// — so the create names only what that engine accepts.
async fn a_cluster(router: &MultiServiceRouter, identifier: &str, engine: &str) {
    let mut params: Vec<(&str, &str)> = vec![("DBClusterIdentifier", identifier), ("Engine", engine)];
    if engine.starts_with("aurora") {
        params.extend([
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "emu-Secret-99"),
        ]);
    }
    expect_ok(identifier, call(router, "CreateDBCluster", &params).await);
}

/// `CreateDBClusterEndpoint` with its three required members.
async fn create_endpoint(
    router: &MultiServiceRouter,
    identifier: &str,
    cluster: &str,
    endpoint_type: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBClusterEndpoint",
        &[
            ("DBClusterEndpointIdentifier", identifier),
            ("DBClusterIdentifier", cluster),
            ("EndpointType", endpoint_type),
        ],
    )
    .await
}

/// Assert the identifier refusal: its status, its code and its sentence, with the value the
/// request sent echoed in it and the rule sentence nowhere in it.
fn expect_identifier_refusal(case: &str, response: (u16, String), named: &str) {
    let body = expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
    let message = text(&body, "Message");
    assert_eq!(
        message,
        invalid_endpoint_identifier(named),
        "case {case:?}: expected the subject sentence echoing {named:?}, got {message:?} in \
         {body}"
    );
    assert_ne!(
        message,
        the_rule_sentence("DBClusterEndpointIdentifier"),
        "case {case:?}: the refusal must carry the member's subject sentence, not the one that \
         states the rule, got {body}"
    );
}

/// The fixed half of the sentence a `DBClusterNotFoundFault` of the cluster-endpoint API
/// carries: the cluster is named as the SOURCE the call's own resource stands on, and the
/// request's `DBClusterIdentifier` is the only variable part.
const SOURCE_CLUSTER_PREFIX: &str = "The source cluster could not be found or cannot be accessed: ";

/// The sentence a `DBClusterEndpointNotFoundFault` carries: the resource type, the identifier
/// the request sent, and `not found` with no terminal period.
fn endpoint_not_found(named: &str) -> String {
    format!("DBClusterEndpoint {named} not found")
}

/// One malformed identifier, and which clause of the rule it breaks.
struct Malformed {
    case: &'static str,
    named: String,
}

/// The sentence this front door refuses an `EndpointType` outside its published domain with:
/// that the type is invalid, then the valid types, comma-separated and written writer-first.
///
/// Entirely FIXED text — it echoes the rejected value nowhere — so it is spelled out here
/// rather than formatted from the request.
const INVALID_ENDPOINT_TYPE_MESSAGE: &str =
    "Invalid endpoint type. The valid endpoint types are: WRITER,READER,ANY";

/// The malformed identifiers every row of the table below sends: one per clause the rule
/// states, and one per kind of character the identifier alphabet excludes.
fn malformed_identifiers() -> Vec<Malformed> {
    let row = |case: &'static str, named: String| Malformed { case, named };
    vec![
        row("underscores and a punctuation mark", "Bad_Endpoint_Name!".to_string()),
        row("an underscore alone", "emu_ceirw_underscored".to_string()),
        row("a space", "emu ceirw spaced".to_string()),
        row("a dot", "emu.ceirw.dotted".to_string()),
        row("a multi-byte letter", "emu-ceirw-caf\u{e9}".to_string()),
        row("a leading digit", "9emu-ceirw".to_string()),
        row("a leading hyphen", "-emu-ceirw".to_string()),
        row("a trailing hyphen", "emu-ceirw-".to_string()),
        row("two consecutive hyphens", "emu--ceirw".to_string()),
        row("no characters at all", String::new()),
        row(
            "one character past the published bound",
            padded_to("emu-ceirw-long-", MAX_DB_CLUSTER_ENDPOINT_IDENTIFIER_LEN + 1),
        ),
    ]
}

/// The report of the endpoint `identifier`, polled until that endpoint is `available` — the
/// status the transition an accepted create or modification owes it settles to.
///
/// The read is `DescribeDBClusterEndpoints` and nothing else — a REPORTING call — so only a
/// transition this service's clock owes can end the loop.
async fn poll_endpoint_available(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let params = [("DBClusterEndpointIdentifier", identifier.to_string())];
    let (body, _) = poll_until(
        case,
        &format!("{identifier} reaching available"),
        || call(router, "DescribeDBClusterEndpoints", &params),
        |status, body| {
            status == 200
                && subtrees(body, "DBClusterEndpointList")
                    .first()
                    .is_some_and(|endpoint| {
                        optional_text(endpoint, "Status").as_deref() == Some("available")
                    })
        },
    )
    .await;
    subtrees(&body, "DBClusterEndpointList")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the report must carry {identifier}"))
}

#[tokio::test]
async fn an_identifier_outside_the_rule_is_refused_by_its_subject_and_echoed_back() {
    let router = emulator();
    a_cluster(&router, AURORA_CLUSTER, "aurora-postgresql").await;
    a_cluster(&router, GRAPH_CLUSTER, "neptune").await;

    // Each malformed value, on a cluster that exists, on a cluster of another control plane
    // behind this front door, and on a cluster nothing created — the member is checked before
    // anything is resolved, so all three answer the one sentence.
    for Malformed { case, named } in malformed_identifiers() {
        for (where_, cluster, endpoint_type) in [
            ("on an Aurora cluster", AURORA_CLUSTER, "ANY"),
            ("on a graph cluster", GRAPH_CLUSTER, "READER"),
            ("on a cluster that does not exist", ABSENT_CLUSTER, "WRITER"),
        ] {
            expect_identifier_refusal(
                &format!("{case} {where_}"),
                create_endpoint(&router, &named, cluster, endpoint_type).await,
                &named,
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_inside_the_rule_is_served_and_read_back() {
    let router = emulator();
    a_cluster(&router, AURORA_CLUSTER, "aurora-postgresql").await;

    let longest = padded_to("emu-ceirw-ok-", MAX_DB_CLUSTER_ENDPOINT_IDENTIFIER_LEN);
    // The well-formed shapes the rule admits: one character, a plain lower-case name, a
    // mixed-case spelling the store folds, digits and hyphens, and the published bound itself.
    let rows: Vec<(&str, String)> = vec![
        ("one character", "e".to_string()),
        ("a plain lower-case name", "emu-ceirw-ok".to_string()),
        ("a mixed-case spelling", "EMU-CEIRW-Mixed".to_string()),
        ("digits and hyphens", "emu-ceirw-9-7".to_string()),
        ("the longest identifier this service admits", longest),
    ];
    for (case, named) in rows {
        expect_ok(
            case,
            create_endpoint(&router, &named, AURORA_CLUSTER, "ANY").await,
        );
        // The endpoint is read back through the report, so the create stored a record rather
        // than answering a shape: the identifier it is reported under is the folded spelling
        // of the value the request sent.
        let folded = named.to_ascii_lowercase();
        let endpoint = poll_endpoint_available(&router, case, &folded).await;
        assert_eq!(
            text(&endpoint, "DBClusterEndpointIdentifier"),
            folded,
            "case {case:?}: the created endpoint is reported under the folded identifier, got \
             {endpoint}"
        );
    }
}

#[tokio::test]
async fn the_graph_control_planes_cluster_endpoint_calls_carry_the_front_doors_own_sentences() {
    let router = emulator();
    // A graph cluster EXISTS, so every refusal below is about the resource the call named
    // rather than about an empty store.
    a_cluster(&router, GRAPH_CLUSTER, "neptune").await;

    // The identifier the create sends here is WELL formed, so the call gets past the member
    // gate and the refusal is the cluster's: the two gates are in that order, and each has its
    // own sentence.
    for (case, cluster) in [
        ("a cluster nothing created", ABSENT_CLUSTER),
        ("a cluster named by a spelling nothing holds", "emu-ceirw-other"),
    ] {
        for (which, response) in [
            (
                "DescribeDBClusterEndpoints",
                call(
                    &router,
                    "DescribeDBClusterEndpoints",
                    &[("DBClusterIdentifier", cluster)],
                )
                .await,
            ),
            (
                "CreateDBClusterEndpoint",
                create_endpoint(&router, "emu-ceirw-graph-ep", cluster, "READER").await,
            ),
        ] {
            let case = format!("{which} for {case}");
            let body = expect_error(&case, response, 404, "DBClusterNotFoundFault");
            let message = text(&body, "Message");
            assert_eq!(
                message,
                format!("{SOURCE_CLUSTER_PREFIX}{cluster}"),
                "case {case:?}: expected the source-cluster sentence naming {cluster:?}, got \
                 {message:?} in {body}"
            );
        }
    }

    // A custom endpoint nothing created is named by the identifier the request sent, in the
    // spelling it sent it in, on both calls that ADDRESS an endpoint.
    for named in ["emu-ceirw-absent-ep", "EMU-CEIRW-Absent-EP"] {
        for (which, response) in [
            (
                "ModifyDBClusterEndpoint",
                call(
                    &router,
                    "ModifyDBClusterEndpoint",
                    &[
                        ("DBClusterEndpointIdentifier", named),
                        ("EndpointType", "ANY"),
                    ],
                )
                .await,
            ),
            (
                "DeleteDBClusterEndpoint",
                call(
                    &router,
                    "DeleteDBClusterEndpoint",
                    &[("DBClusterEndpointIdentifier", named)],
                )
                .await,
            ),
        ] {
            let case = format!("{which} for {named:?}");
            let body = expect_error(&case, response, 400, "DBClusterEndpointNotFoundFault");
            let message = text(&body, "Message");
            assert_eq!(
                message,
                endpoint_not_found(named),
                "case {case:?}: expected the endpoint-not-found sentence naming {named:?}, got \
                 {message:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_endpoint_type_outside_the_published_domain_is_refused_with_the_types_that_are_valid() {
    let router = emulator();
    a_cluster(&router, AURORA_CLUSTER, "aurora-postgresql").await;
    // An endpoint that EXISTS, so the modification rows reach the member gate on a call that
    // would otherwise be served.
    expect_ok(
        "the endpoint the modification rows address",
        create_endpoint(&router, "emu-ceirw-typed", AURORA_CLUSTER, "ANY").await,
    );

    // The rows span the kinds of value outside the domain: a word that is no type at all, the
    // type a custom endpoint REPORTS (`CUSTOM`, which no create may ask for), a published
    // value carrying trailing text, a value of no characters, and a multi-byte one.
    for (case, requested) in [
        ("a word that is no type", "BOGUS"),
        ("the type an endpoint reports", "CUSTOM"),
        ("a published value with trailing text", "READER-ONLY"),
        ("no characters at all", ""),
        ("a multi-byte value", "RE\u{c4}DER"),
    ] {
        for (which, response) in [
            (
                "CreateDBClusterEndpoint",
                create_endpoint(&router, "emu-ceirw-type", AURORA_CLUSTER, requested).await,
            ),
            (
                "ModifyDBClusterEndpoint",
                call(
                    &router,
                    "ModifyDBClusterEndpoint",
                    &[
                        ("DBClusterEndpointIdentifier", "emu-ceirw-typed"),
                        ("EndpointType", requested),
                    ],
                )
                .await,
            ),
        ] {
            let case = format!("{which} with {case} ({requested:?})");
            let body = expect_error(&case, response, 400, INVALID_PARAMETER_VALUE);
            let message = text(&body, "Message");
            assert_eq!(
                message, INVALID_ENDPOINT_TYPE_MESSAGE,
                "case {case:?}: expected the invalid-endpoint-type sentence, got {message:?} in \
                 {body}"
            );
        }
    }

    // Every published value is accepted, in any case, so the gate refuses what the domain
    // excludes rather than everything — and the type is reported back in the published case.
    // A modification is taken only on an endpoint that is available, so each row waits for
    // the transition the previous one owed through the shared poll loop.
    for (case, requested, reported) in [
        ("the reader type", "READER", "READER"),
        ("the writer type in lower case", "writer", "WRITER"),
        ("the either type in mixed case", "Any", "ANY"),
    ] {
        poll_endpoint_available(&router, case, "emu-ceirw-typed").await;
        let modified = expect_ok(
            case,
            call(
                &router,
                "ModifyDBClusterEndpoint",
                &[
                    ("DBClusterEndpointIdentifier", "emu-ceirw-typed"),
                    ("EndpointType", requested),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&modified, "CustomEndpointType"),
            reported,
            "case {case:?}: the accepted type is reported in its published case, got {modified}"
        );
    }
}
