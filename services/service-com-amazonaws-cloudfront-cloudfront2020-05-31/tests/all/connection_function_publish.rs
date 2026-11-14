// SPDX-License-Identifier: CC0-1.0

//! `PublishConnectionFunction` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are:
//!
//! * a publish COPIES the `DEVELOPMENT` stage of the connection function its `Id` names
//!   into the `LIVE` stage: the stage it writes carries the configuration and the identity
//!   the create stored, keeps the connection function's created time, is last modified at
//!   the publish, and gets a FRESH version token of its own — so a later describe of the
//!   `LIVE` stage answers from that record, while the stage the publish READ keeps its own
//!   status and version token;
//! * the copy only happens at the version its caller read: an `If-Match` that carries no
//!   version token at all is "missing or not valid" (`InvalidIfMatchVersion`, HTTP 400), one
//!   that carries a version token that is not the current one is a failed precondition
//!   (`PreconditionFailed`, HTTP 412), and a refused publish leaves the connection function
//!   with no `LIVE` stage at all;
//! * the operation addresses the resource by the `Id` its create minted, so an id no
//!   connection function answers to — a name among them — is an entity that was not found
//!   (HTTP 404) and publishes nothing;
//! * a publish is ASYNCHRONOUS: the summary it answers with reports the in-progress status,
//!   and the CLOCK — not a call — owes the written stage the status it settles to, computed
//!   from the stored distributions: a connection function a distribution names settles as
//!   associated, and one nothing names as unassociated.
//!
//! The table rows differ along everything the copy generalises over — the name's length
//! (both published bounds of `FunctionName`) and character classes, the runtime (every value
//! the modelled `FunctionRuntime` enum admits), the comment's length and whether it is
//! ASCII, the code's size (the modelled minimum, a real script and the published quota
//! bound) and whether it is text at all, and whether the configuration associates a key
//! value store — because every one of them is part of what a publish has to carry across.

use std::collections::BTreeSet;
use std::time::Instant;

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::{
    create_connection_function, create_table, describe_connection_function, key_value_store_arn,
    member, Create,
};
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, is_version_token, Answer, API_VERSION,
    POLL_INTERVAL, READY_DEADLINE,
};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The status a stage that has never been published reports.
const STATUS_UNPUBLISHED: &str = "UNPUBLISHED";

/// The status a published stage reports while no distribution names the connection
/// function, and the one it reports while a distribution does.
pub(crate) const STATUS_UNASSOCIATED: &str = "UNASSOCIATED";
const STATUS_ASSOCIATED: &str = "ASSOCIATED";

/// The status the stage a publish has just written reports: publishing a connection
/// function is asynchronous, so the call that starts it answers the status the copy is IN.
pub(crate) const STATUS_PUBLISHING: &str = "PUBLISHING";

/// Look again at the `LIVE` stage of the connection function `id` until it reports
/// `settled` — the status the CLOCK owes a publish — or [`READY_DEADLINE`] passes, which
/// fails the case.
///
/// The CONDITION is what is waited for, never a span of the host's clock.
pub(crate) async fn awaited_publish(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    settled: &str,
) -> Answer {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let live = describe_connection_function(router, id, Some(STAGE_LIVE)).await;
        if live.root().text_at("Status") == Some(settled) {
            return live;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the publish never settled to {settled}, last answer {}",
            live.raw
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// Calling the publish
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/connection-function/{Id}/publish`, carrying `if_match` as the
/// `If-Match` header the operation binds its required `IfMatch` member to.
///
/// `pub(crate)` so the suite that reads a published stage's CODE drives the publish through
/// this one call path rather than a second copy of it.
pub(crate) async fn publish_connection_function(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/connection-function/{id}/publish"),
        None,
        Some(if_match),
    )
    .await
}

/// What one create handed back: the identity it minted and the version token of the
/// `DEVELOPMENT` stage it stored.
struct Created {
    id: String,
    arn: String,
    etag: String,
    created_time: String,
}

/// Create the connection function `create` asks for and answer what the create minted.
async fn create_one(router: &MultiServiceRouter, create: &Create) -> Created {
    let case = create.case;
    let answer = create_connection_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        answer.status, answer.raw
    );
    let root = answer.root();
    Created {
        id: member(root, "Id", case, &answer.raw),
        arn: member(root, "ConnectionFunctionArn", case, &answer.raw),
        created_time: member(root, "CreatedTime", case, &answer.raw),
        etag: answer.header("etag").to_string(),
    }
}

/// The same create under another name, so a row can be run against a connection function of
/// its own without the name of the previous one standing in the way.
fn renamed(create: &Create, name: &str) -> Create {
    let mut renamed = create.clone();
    renamed.name = name.to_string();
    renamed
}

/// Assert the `ConnectionFunctionSummary` document `answer` carries is the connection
/// function `create` asked for, in `stage` and with `status`, reporting the identity
/// `created` minted.
fn assert_summary(
    answer: &Answer,
    create: &Create,
    created: &Created,
    stage: &str,
    status: &str,
    context: &str,
) {
    let case = create.case;
    let summary = answer.root();
    let raw = answer.raw.as_str();

    let name = member(summary, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let id = member(summary, "Id", case, raw);
    assert_eq!(
        id, created.id,
        "case {case} ({context}): expected the Id the create minted, got {id:?}"
    );

    let arn = member(summary, "ConnectionFunctionArn", case, raw);
    assert_eq!(
        arn, created.arn,
        "case {case} ({context}): expected the ARN the create minted, got {arn:?}"
    );

    let reported_stage = member(summary, "Stage", case, raw);
    assert_eq!(
        reported_stage, stage,
        "case {case} ({context}): expected Stage {stage}, got {reported_stage:?}"
    );

    let reported_status = member(summary, "Status", case, raw);
    assert_eq!(
        reported_status, status,
        "case {case} ({context}): expected Status {status}, got {reported_status:?}"
    );

    let comment = summary
        .text_at("ConnectionFunctionConfig/Comment")
        .unwrap_or("")
        .to_string();
    assert_eq!(
        comment, create.comment,
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );

    let runtime = member(summary, "ConnectionFunctionConfig/Runtime", case, raw);
    assert_eq!(
        runtime, create.runtime,
        "case {case} ({context}): expected the submitted Runtime, got {runtime:?}"
    );

    let associated: Vec<String> = summary
        .at("ConnectionFunctionConfig/KeyValueStoreAssociations/Items")
        .map(|items| {
            items
                .children_named("KeyValueStoreAssociation")
                .iter()
                .map(|association| {
                    association
                        .text_at("KeyValueStoreARN")
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        associated, create.associations,
        "case {case} ({context}): expected the submitted key value store associations, got \
         {associated:?}"
    );

    // A copy keeps the created time of the connection function it was taken from, and is
    // last modified when it was written.
    let reported_created = member(summary, "CreatedTime", case, raw);
    assert_eq!(
        reported_created, created.created_time,
        "case {case} ({context}): expected the CreatedTime the create reported, got \
         {reported_created:?}"
    );
    let modified = member(summary, "LastModifiedTime", case, raw);
    assert!(
        !modified.is_empty(),
        "case {case} ({context}): expected a LastModifiedTime, got {modified:?}"
    );
}

/// The `If-Match` header one precondition row sends.
enum Token {
    /// Built from the current version token of the stage the publish would copy.
    Current(fn(&str) -> String),
    /// A value that stands on its own, whatever version the store holds.
    Literal(String),
}

/// One `If-Match` header, and what the service is expected to answer it with: `None` to
/// publish, or the status and error code the refusal carries.
struct Precondition {
    case: &'static str,
    token: Token,
    refusal: Option<(u16, &'static str)>,
}

/// The `DistributionConfig` document for `reference` with `ConnectionFunctionAssociation`
/// naming the connection function `id` — the member a distribution carries a connection
/// function by ("The association's ID"), spliced in where `DistributionConfig` carries it:
/// after every member the shared builder emits.
fn distribution_xml(reference: &str, id: &str) -> String {
    let base = config("a distribution that names a connection function", reference).xml();
    let association =
        format!("<ConnectionFunctionAssociation><Id>{id}</Id></ConnectionFunctionAssociation>");
    base.replace(
        "</DistributionConfig>",
        &format!("{association}</DistributionConfig>"),
    )
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A publish COPIES the `DEVELOPMENT` stage into `LIVE`: the summary it answers with, and
/// every later read of the `LIVE` stage, report the stored configuration and the identity
/// the create minted, under a version token of their own — while the stage the publish read
/// keeps its own status and token. Across the table, no two published stages share a token.
#[tokio::test]
async fn a_publish_copies_development_into_live_and_the_describe_reports_it() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_publish_store").await;
    let mut live_etags = BTreeSet::new();

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created = create_one(&router, &create).await;

        let published = publish_connection_function(&router, &created.id, &created.etag).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 from the publish, got {} with {}",
            published.status, published.raw
        );
        assert_summary(
            &published,
            &create,
            &created,
            STAGE_LIVE,
            STATUS_PUBLISHING,
            "the publish",
        );

        // The LIVE stage answers reads from the record the publish wrote, under a version
        // token of its own — once the clock has settled the status that publish is owed.
        let live = awaited_publish(&router, case, &created.id, STATUS_UNASSOCIATED).await;
        assert_eq!(
            live.status, 200,
            "case {case}: expected HTTP 200 describing the LIVE stage, got {} with {}",
            live.status, live.raw
        );
        assert_summary(
            &live,
            &create,
            &created,
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
            "the describe of LIVE",
        );
        let live_etag = live.header("etag").to_string();
        assert!(
            is_version_token(&live_etag),
            "case {case}: expected a version token for the LIVE stage, got {live_etag:?}"
        );
        assert_ne!(
            live_etag, created.etag,
            "case {case}: expected the published stage to carry a version token of its own"
        );
        assert!(
            live_etags.insert(live_etag.clone()),
            "case {case}: the version token {live_etag} repeated"
        );

        // The stage the publish READ is untouched: it is still the unpublished stage, at the
        // version the create handed back.
        let development =
            describe_connection_function(&router, &created.id, Some(STAGE_DEVELOPMENT)).await;
        assert_eq!(
            development.status, 200,
            "case {case}: expected HTTP 200 describing DEVELOPMENT, got {} with {}",
            development.status, development.raw
        );
        assert_summary(
            &development,
            &create,
            &created,
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
            "the describe of DEVELOPMENT",
        );
        let development_etag = development.header("etag");
        assert_eq!(
            development_etag, created.etag,
            "case {case}: expected the create's version token from DEVELOPMENT, got \
             {development_etag:?}"
        );
    }
}

/// The copy only happens at the version its caller read. A header carrying the current
/// version token — bare, quoted as an HTTP entity tag, or quoted as a weak one — publishes;
/// anything that is no version token at all is refused as missing or not valid; a
/// well-formed token that belongs to another version is a failed precondition. A refused
/// publish leaves the connection function with no `LIVE` stage.
#[tokio::test]
async fn a_publish_needs_the_current_version_of_the_stage_it_copies() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_precondition_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    // A version token this service really minted that belongs to another connection
    // function, so the stale case is a well-formed token rather than a made-up string.
    let foreign = create_one(&router, &renamed(&row, "foreign_version_source")).await;

    let cases = [
        Precondition {
            case: "the current version token, bare",
            token: Token::Current(|current| current.to_string()),
            refusal: None,
        },
        Precondition {
            case: "the current version token, quoted as an entity tag",
            token: Token::Current(|current| format!("\"{current}\"")),
            refusal: None,
        },
        Precondition {
            case: "the current version token, quoted as a weak entity tag",
            token: Token::Current(|current| format!("W/\"{current}\"")),
            refusal: None,
        },
        Precondition {
            case: "a value that is no version token at all",
            token: Token::Literal("not-a-version".to_string()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "a token of the right shape in the wrong case",
            token: Token::Current(|current| current.to_lowercase()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "a token whose body is too short",
            token: Token::Literal("E123".to_string()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "the version token of another connection function",
            token: Token::Literal(foreign.etag.clone()),
            refusal: Some((412, "PreconditionFailed")),
        },
    ];

    for (index, precondition) in cases.iter().enumerate() {
        let case = precondition.case;
        let refusal = &precondition.refusal;
        // One connection function per row, so what a row publishes cannot change what the
        // next one reads.
        let create = renamed(&row, &format!("precondition_probe_{index}"));
        let created = create_one(&router, &create).await;
        let sent = match &precondition.token {
            Token::Current(render) => render(&created.etag),
            Token::Literal(literal) => literal.clone(),
        };

        let answer = publish_connection_function(&router, &created.id, &sent).await;
        let live = describe_connection_function(&router, &created.id, Some(STAGE_LIVE)).await;

        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected HTTP 200 publishing with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                let stage = member(answer.root(), "Stage", case, &answer.raw);
                assert_eq!(
                    stage, STAGE_LIVE,
                    "case {case}: expected the published stage, got {stage:?}"
                );
                assert_eq!(
                    live.status, 200,
                    "case {case}: expected the LIVE stage to be readable, got {} with {}",
                    live.status, live.raw
                );
            }
            Some((status, code)) => {
                assert_eq!(
                    answer.status, *status,
                    "case {case}: expected HTTP {status} publishing with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    *code,
                    "case {case}: expected {code}, got {}",
                    answer.raw
                );
                assert_eq!(
                    live.status, 404,
                    "case {case}: a refused publish must leave no LIVE stage, got {} with {}",
                    live.status, live.raw
                );
                // The stage the publish would have copied is untouched.
                let development =
                    describe_connection_function(&router, &created.id, Some(STAGE_DEVELOPMENT))
                        .await;
                assert_eq!(
                    development.header("etag"),
                    created.etag,
                    "case {case}: a refused publish must leave the DEVELOPMENT version alone, got \
                     {}",
                    development.raw
                );
            }
        }
    }
}

/// The operation addresses the resource by "The connection function ID" the create minted,
/// so every other identifier — the name it was created under among them — names no
/// connection function to publish: HTTP 404 `EntityNotFound`, and nothing is published.
#[tokio::test]
async fn an_id_no_connection_function_answers_to_is_not_found() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_unknown_id_store").await;
    let table = create_table(&kvs_arn);
    let create = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "present_connection_function",
    );
    let created = create_one(&router, &create).await;

    let unknown = [
        ("the name the create was given", create.name.clone()),
        ("an id of the minted shape nothing holds", {
            let mut other = created.id.clone();
            // Flip the last character of a real id, so the value has the shape this service
            // mints and still names nothing.
            other.pop();
            format!("{other}0")
        }),
        ("a value of no particular shape", "missing".to_string()),
    ];

    for (case, identifier) in unknown {
        let answer = publish_connection_function(&router, &identifier, &created.etag).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 publishing {identifier:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            answer.raw
        );
    }

    // Nothing was published: the connection function that is there still has no LIVE stage.
    let live = describe_connection_function(&router, &created.id, Some(STAGE_LIVE)).await;
    assert_eq!(
        live.status, 404,
        "a refused publish must leave no LIVE stage, got {} with {}",
        live.status, live.raw
    );
}

/// A publish is ASYNCHRONOUS: the summary it answers with reports the in-progress status,
/// and the status the written stage SETTLES to is computed from the stored distributions: a
/// connection function a distribution names settles as associated, one nothing names as
/// unassociated — and each publish mints a fresh version token for the stage it writes.
#[tokio::test]
async fn the_published_status_reports_whether_a_distribution_names_the_function() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_association_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    let named = create_one(&router, &renamed(&row, "function_a_distribution_names")).await;
    let unnamed = create_one(&router, &renamed(&row, "function_nothing_names")).await;

    // The publish itself reports the in-progress status, and while no distribution names the
    // connection function the clock settles it to published-but-unassociated.
    let first = publish_connection_function(&router, &named.id, &named.etag).await;
    assert_eq!(
        first.status, 200,
        "expected HTTP 200 from the first publish, got {} with {}",
        first.status, first.raw
    );
    let first_status = member(first.root(), "Status", "before any association", &first.raw);
    assert_eq!(
        first_status, STATUS_PUBLISHING,
        "case before any association: expected Status {STATUS_PUBLISHING}, got {first_status:?}"
    );
    let first_live = awaited_publish(
        &router,
        "before any association",
        &named.id,
        STATUS_UNASSOCIATED,
    )
    .await;
    let first_live_etag = first_live.header("etag").to_string();

    // A distribution that names the connection function, stored with the association it
    // submitted.
    let distribution =
        create_distribution(&router, &distribution_xml("publish-association", &named.id)).await;
    assert_eq!(
        distribution.status, 201,
        "expected HTTP 201 creating the distribution, got {} with {}",
        distribution.status, distribution.raw
    );
    let stored_association = distribution
        .root()
        .text_at("DistributionConfig/ConnectionFunctionAssociation/Id")
        .unwrap_or_default();
    assert_eq!(
        stored_association, named.id,
        "expected the distribution to carry the submitted association, got \
         {stored_association:?}"
    );

    // So the next publish of that connection function settles on the association, under a
    // fresh version token.
    let again = publish_connection_function(&router, &named.id, &named.etag).await;
    assert_eq!(
        again.status, 200,
        "expected HTTP 200 from the second publish, got {} with {}",
        again.status, again.raw
    );
    let again_status = member(again.root(), "Status", "a distribution names it", &again.raw);
    assert_eq!(
        again_status, STATUS_PUBLISHING,
        "case a distribution names it: expected Status {STATUS_PUBLISHING}, got {again_status:?}"
    );
    let live = awaited_publish(
        &router,
        "a distribution names it",
        &named.id,
        STATUS_ASSOCIATED,
    )
    .await;
    let live_status = member(live.root(), "Status", "a distribution names it", &live.raw);
    assert_eq!(
        live_status, STATUS_ASSOCIATED,
        "case a distribution names it: expected the describe to report Status \
         {STATUS_ASSOCIATED}, got {live_status:?}"
    );
    let live_etag = live.header("etag").to_string();
    assert_ne!(
        live_etag, first_live_etag,
        "expected the second publish to mint a version token of its own"
    );

    // The connection function the distribution does not name is unaffected by it.
    let control = publish_connection_function(&router, &unnamed.id, &unnamed.etag).await;
    assert_eq!(
        control.status, 200,
        "expected HTTP 200 publishing the connection function nothing names, got {} with {}",
        control.status, control.raw
    );
    let control_status = member(
        control.root(),
        "Status",
        "nothing names it",
        &control.raw,
    );
    assert_eq!(
        control_status, STATUS_PUBLISHING,
        "case nothing names it: expected Status {STATUS_PUBLISHING}, got {control_status:?}"
    );
    let control_live =
        awaited_publish(&router, "nothing names it", &unnamed.id, STATUS_UNASSOCIATED).await;
    let control_live_status =
        member(control_live.root(), "Status", "nothing names it", &control_live.raw);
    assert_eq!(
        control_live_status, STATUS_UNASSOCIATED,
        "case nothing names it: expected the describe to report Status {STATUS_UNASSOCIATED}, \
         got {control_live_status:?}"
    );
}
