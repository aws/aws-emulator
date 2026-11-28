// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 bot lifecycle slice: `CreateBot`,
//! `DescribeBot`, `ListBots`, `UpdateBot`, `DeleteBot`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create mints an id matching the modelled `Id` shape and stores the record, so a
//!   read of that id reports back what was stored;
//! * an update replaces the stored definition and a later read reports the new values;
//! * a delete removes the record, so a later read cannot find it;
//! * a listing is built from the stored records, filtered, ordered and paged;
//! * an id no record carries is refused with the error each operation's model declares.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`lex`), which is how
/// the router picks the service that claims the request. The signature is never
/// checked: authentication is off for this router.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/lex/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// An IAM role ARN satisfying the modelled `RoleArn` shape
/// (len 32..2048, pattern `^arn:aws:iam::[0-9]{12}:role/.*$`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/lex-demo";

/// The members `CreateBot` accepts that `DescribeBot` reports back.
///
/// `botTags` / `testBotAliasTags` are deliberately absent: the modelled
/// `DescribeBotResponse` carries neither (`smithy_query(query="operation",
/// operation="DescribeBot")`), so they are checked on the create response instead.
const ECHOED_BY_DESCRIBE: [&str; 8] = [
    "botName",
    "description",
    "roleArn",
    "dataPrivacy",
    "idleSessionTTLInSeconds",
    "botType",
    "botMembers",
    "errorLogSettings",
];

/// One emulator holding one Lex Models V2 service on the single shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the shared engine handed back, for a test that has to reach
/// the store directly — to age a stored record the way the clock ages it, say.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request and return its status and parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _, parsed) = call_announcing(router, method, uri, body).await;
    (status, parsed)
}

/// Dispatch one request and return its status, the media type it announces and its
/// parsed JSON body — the three things a client reads off a response.
pub(crate) async fn call_announcing(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
    let (status, announced, raw) = call_raw(router, method, uri, body).await;
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&raw).unwrap_or(Value::Null)
    };
    (status, announced, parsed)
}

/// The same dispatch, with the response document handed back as the TEXT the client
/// receives rather than as a parsed tree — for an assertion about how a value is
/// serialized, which parsing into a `Value` throws away.
pub(crate) async fn call_raw(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Option<String>, String) {
    let payload = body.map_or_else(Bytes::new, |body| {
        Bytes::from(serde_json::to_vec(&body).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "models-v2-lex.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let announced = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.body();
    let raw = String::from_utf8_lossy(bytes).into_owned();
    (status, announced, raw)
}

/// The wire error code restJson1 puts in the error envelope's `__type`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A member of a JSON object, or `Null` when it is absent — an absent member and an
/// explicit null compare equal, which is what "the response omits what was not set"
/// means on the wire.
pub(crate) fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// A response member as a string.
pub(crate) fn text(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// A timestamp member, which restJson1 renders as epoch seconds.
pub(crate) fn seconds(body: &Value, name: &str) -> f64 {
    member(body, name).as_f64().unwrap_or(f64::NAN)
}

/// A `CreateBot` request body.
pub(crate) fn create_body(name: &str, ttl: i64, extra: Value) -> Value {
    let mut body = json!({
        "botName": name,
        "roleArn": ROLE_ARN,
        "dataPrivacy": { "childDirected": false },
        "idleSessionTTLInSeconds": ttl,
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Create a bot and return its minted id, failing the test on any other answer.
pub(crate) async fn create_bot(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, response) = call(router, "PUT", "/bots", Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "CreateBot {body:?}: expected 202, got {status} {response:?}"
    );
    text(&response, "botId")
}

/// A bot name of exactly `len` characters that satisfies the modelled `Name` pattern
/// `^([0-9a-zA-Z][_-]?){1,100}$`.
fn name_of_length(len: usize) -> String {
    "Bot".chars().cycle().take(len).collect()
}

/// The table every round-trip assertion runs over: rows differ in which optional
/// members are set, in the bot type, in the session TTL (including both published
/// bounds) and in the shape of the name.
fn bot_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            create_body("MinimalBot", 300, json!({})),
        ),
        (
            "session TTL at the published minimum",
            create_body("TtlFloorBot", 60, json!({})),
        ),
        (
            "session TTL at the published maximum",
            create_body("TtlCeilingBot", 86_400, json!({})),
        ),
        (
            "every optional member set",
            create_body(
                "FullyLoadedBot",
                1_800,
                json!({
                    "description": "a bot with every optional member set",
                    "botType": "Bot",
                    "botTags": { "team": "voice", "env": "test" },
                    "testBotAliasTags": { "env": "test" },
                    "errorLogSettings": { "enabled": true },
                    "dataPrivacy": { "childDirected": true },
                }),
            ),
        ),
        (
            "a bot network with two members",
            create_body(
                "NetworkBot",
                600,
                json!({
                    "botType": "BotNetwork",
                    "botMembers": [
                        {
                            "botMemberId": "ABCDEFGHIJ",
                            "botMemberName": "MemberOne",
                            "botMemberAliasId": "TSTALIASID",
                            "botMemberAliasName": "AmazonLexTestAlias",
                            "botMemberVersion": "DRAFT",
                        },
                        {
                            "botMemberId": "KLMNOPQRST",
                            "botMemberName": "MemberTwo",
                            "botMemberAliasId": "0123456789",
                            "botMemberAliasName": "SecondAlias",
                            "botMemberVersion": "1",
                        },
                    ],
                }),
            ),
        ),
        (
            "a name at the published maximum length",
            create_body(&name_of_length(100), 900, json!({})),
        ),
        (
            "a name using the separators the pattern allows",
            create_body("My_Bot-Name_2", 120, json!({})),
        ),
    ]
}

#[tokio::test]
async fn a_created_bot_is_stored_updated_and_deleted_through_its_own_id() {
    let router = emulator();

    for (case, request) in bot_cases() {
        // CREATE — the minted id must match the modelled `Id` shape and back a record.
        let (status, created) = call(&router, "PUT", "/bots", Some(request.clone())).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected CreateBot to answer 202, got {status} {created:?}"
        );
        let bot_id = text(&created, "botId");
        assert_eq!(
            bot_id.chars().count(),
            10,
            "case {case:?}: botId must be 10 characters, got {bot_id:?}"
        );
        assert!(
            bot_id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: botId must match ^[0-9a-zA-Z]+$, got {bot_id:?}"
        );
        assert_eq!(
            text(&created, "botStatus"),
            "Creating",
            "case {case:?}: a bot AWS reports as Creating must not start Available: {created:?}"
        );
        for tag_member in ["botTags", "testBotAliasTags"] {
            assert_eq!(
                member(&created, tag_member),
                member(&request, tag_member),
                "case {case:?}: CreateBot must report the {tag_member} it stored"
            );
        }
        let created_at = seconds(&created, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {created:?}"
        );

        // DESCRIBE — the stored record, member for member.
        let (status, described) =
            call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBot to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "botId"),
            bot_id,
            "case {case:?}: DescribeBot must report the id it was asked about"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeBot must report the stored {name}"
            );
        }
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );

        // UPDATE — the stored definition is replaced and read back.
        let update = json!({
            "botName": member(&request, "botName"),
            "roleArn": ROLE_ARN,
            "dataPrivacy": { "childDirected": true },
            "idleSessionTTLInSeconds": 1_200,
            "description": "updated by the lifecycle test",
        });
        let (status, updated) =
            call(&router, "PUT", &format!("/bots/{bot_id}"), Some(update)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected UpdateBot to answer 202, got {status} {updated:?}"
        );
        assert_eq!(
            text(&updated, "description"),
            "updated by the lifecycle test",
            "case {case:?}: UpdateBot must report the new description, got {updated:?}"
        );
        let updated_at = seconds(&updated, "lastUpdatedDateTime");
        assert!(
            updated_at >= created_at,
            "case {case:?}: lastUpdatedDateTime {updated_at} must not precede creationDateTime {created_at}"
        );

        let (status, described) =
            call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBot after UpdateBot to answer 200, got {status}"
        );
        assert_eq!(
            text(&described, "description"),
            "updated by the lifecycle test",
            "case {case:?}: the update must be visible to a later read, got {described:?}"
        );
        assert_eq!(
            member(&described, "idleSessionTTLInSeconds"),
            json!(1_200),
            "case {case:?}: the updated TTL must be visible to a later read, got {described:?}"
        );
        assert_eq!(
            member(&described, "dataPrivacy"),
            json!({ "childDirected": true }),
            "case {case:?}: the updated dataPrivacy must be visible, got {described:?}"
        );
        assert_eq!(
            member(&described, "botMembers"),
            Value::Null,
            "case {case:?}: an update that omits botMembers must clear them, got {described:?}"
        );

        // DELETE — the record goes away.
        let (status, deleted) =
            call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected DeleteBot to answer 202, got {status} {deleted:?}"
        );
        assert_eq!(
            text(&deleted, "botId"),
            bot_id,
            "case {case:?}: DeleteBot must report the id it removed, got {deleted:?}"
        );
        assert_eq!(
            text(&deleted, "botStatus"),
            "Deleting",
            "case {case:?}: DeleteBot must report the Deleting status, got {deleted:?}"
        );

        let (status, gone) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeBot after DeleteBot must answer 404, got {status} {gone:?}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {gone:?}"
        );

        let (status, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
        assert_eq!(status, 200, "case {case:?}: ListBots must answer 200");
        let ids = listed_ids(&listed);
        assert!(
            !ids.contains(&bot_id),
            "case {case:?}: a deleted bot must not be listed, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn the_published_bot_quota_bounds_what_one_account_can_store() {
    let router = emulator();

    // Fill the account to the published quota.
    let mut ids = Vec::new();
    for index in 0..state::BOTS_PER_ACCOUNT_QUOTA {
        ids.push(create_bot(&router, &create_body(&format!("QuotaBot{index}"), 300, json!({}))).await);
    }

    // The whole store is still listable in one bounded page: the quota is below the
    // modelled maxResults maximum, so nothing is truncated and no token is handed out.
    let (status, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    assert_eq!(status, 200, "a full account must still be listable");
    assert_eq!(
        listed_ids(&listed).len(),
        state::BOTS_PER_ACCOUNT_QUOTA,
        "the listing must carry every stored bot, got {listed:?}"
    );
    assert_eq!(
        member(&listed, "nextToken"),
        Value::Null,
        "a listing below the page cap must not hand out a nextToken"
    );

    // One past the quota is refused with the modelled quota error, and nothing is stored.
    let (status, response) = call(
        &router,
        "PUT",
        "/bots",
        Some(create_body("OneTooManyBot", 300, json!({}))),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the quota must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException past the quota, got {response:?}"
    );
    let (_, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    assert_eq!(
        listed_ids(&listed).len(),
        state::BOTS_PER_ACCOUNT_QUOTA,
        "the refused create must not have stored a bot"
    );

    // The bound is on what is stored now, not on how many were ever created: freeing a
    // slot lets the next create through.
    let freed = ids.remove(0);
    let (status, _) = call(&router, "DELETE", &format!("/bots/{freed}"), None).await;
    assert_eq!(status, 202, "deleting a bot must free its quota slot");
    let (status, response) = call(
        &router,
        "PUT",
        "/bots",
        Some(create_body("OneTooManyBot", 300, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "a create into a freed slot must succeed, got {status} {response:?}"
    );
}

/// The `botId`s a `ListBots` response reports, in the order they were returned.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("botSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| text(summary, "botId"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_minted_bot_id_is_distinct_and_backed_by_a_record() {
    let router = emulator();

    let mut ids = BTreeSet::new();
    for index in 0..25 {
        let name = format!("UniqueBot{index}");
        let bot_id = create_bot(&router, &create_body(&name, 60 + index * 10, json!({}))).await;
        assert!(
            ids.insert(bot_id.clone()),
            "bot {index}: botId {bot_id} was minted twice"
        );
        let (status, described) =
            call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 200,
            "bot {index}: a minted id must back a record, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "botName"),
            name,
            "bot {index}: the record behind {bot_id} must be the bot that was created"
        );
    }
    assert_eq!(ids.len(), 25, "every created bot must have its own id");
}

#[tokio::test]
async fn a_created_bot_becomes_available_once_the_clock_owes_the_transition() {
    let router = emulator();

    let bot_id = create_bot(&router, &create_body("SettlingBot", 300, json!({}))).await;

    // Straight after the create the caller still observes the transient status.
    let (_, described) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(
        text(&described, "botStatus"),
        "Creating",
        "a bot read back immediately must still be Creating, got {described:?}"
    );

    // Past the due instant the transition the clock owed is materialised, by a read and
    // by a listing alike.
    tokio::time::sleep(state::CREATION_SETTLES_AFTER + Duration::from_millis(150)).await;
    let (_, described) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(
        text(&described, "botStatus"),
        "Available",
        "a bot past its due instant must be Available, got {described:?}"
    );
    let (_, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    let summary = listed
        .get("botSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first().cloned())
        .expect("the created bot is listed");
    assert_eq!(
        text(&summary, "botStatus"),
        "Available",
        "ListBots must report the same status as DescribeBot, got {summary:?}"
    );

    // And the status is stable from there: a read never walks it back.
    let (_, described) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(
        text(&described, "botStatus"),
        "Available",
        "a settled status must stay settled, got {described:?}"
    );
}

#[tokio::test]
async fn list_bots_pages_the_stored_bots_for_every_page_size() {
    let router = emulator();

    let names = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"];
    let mut expected = Vec::new();
    for name in names {
        expected.push(create_bot(&router, &create_body(name, 300, json!({}))).await);
    }

    // The listing is ordered by botName ascending, so the expected id order is the
    // creation order of the alphabetically-sorted names above.
    for page_size in [1_usize, 2, 3, 5, 6, 7] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, listed) = call(&router, "POST", "/bots", Some(request.clone())).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListBots {request:?} must answer 200, got {listed:?}"
            );
            let page = listed_ids(&listed);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must hold at most maxResults entries, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "page size {page_size}: a page of a non-empty listing must not be empty"
            );
            seen.extend(page);
            pages += 1;
            next_token = listed
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= names.len(),
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: paging must walk every stored bot exactly once in order"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: paging must not repeat an entry, got {seen:?}"
        );
    }

    // No maxResults: one page carrying everything.
    let (status, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    assert_eq!(status, 200, "ListBots without maxResults must answer 200");
    assert_eq!(
        listed_ids(&listed),
        expected,
        "a listing without maxResults must carry every stored bot"
    );
    assert_eq!(
        member(&listed, "nextToken"),
        Value::Null,
        "a complete listing must not hand out a nextToken, got {listed:?}"
    );

    // A bot deleted mid-pagination does not strand the listing: the token resumes at the
    // first surviving entry at or after it, with no repeat and no refusal.
    let (_, first_page) = call(&router, "POST", "/bots", Some(json!({ "maxResults": 2 }))).await;
    let token = text(&first_page, "nextToken");
    assert!(!token.is_empty(), "the first page must hand out a token");
    let dropped = expected[2].clone();
    let (status, _) = call(&router, "DELETE", &format!("/bots/{dropped}"), None).await;
    assert_eq!(status, 202, "deleting the bot the token names must succeed");
    let (status, resumed) = call(
        &router,
        "POST",
        "/bots",
        Some(json!({ "maxResults": 2, "nextToken": token })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token whose bot was deleted must still resume the listing, got {resumed:?}"
    );
    assert_eq!(
        listed_ids(&resumed),
        vec![expected[3].clone(), expected[4].clone()],
        "paging must resume at the first surviving entry, got {resumed:?}"
    );
}

#[tokio::test]
async fn list_bots_filters_and_sorts_from_the_stored_records() {
    let router = emulator();

    // name, botType
    let fixtures = [
        ("OrderCoffeeBot", "Bot"),
        ("OrderPizzaBot", "Bot"),
        ("SupportBot", "Bot"),
        ("RetailNetwork", "BotNetwork"),
    ];
    for (name, bot_type) in fixtures {
        create_bot(
            &router,
            &create_body(name, 300, json!({ "botType": bot_type })),
        )
        .await;
    }

    // Each row: the request, and the bot names it must select, in order.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter, default order",
            json!({}),
            vec![
                "OrderCoffeeBot",
                "OrderPizzaBot",
                "RetailNetwork",
                "SupportBot",
            ],
        ),
        (
            "sorted by name descending",
            json!({ "sortBy": { "attribute": "BotName", "order": "Descending" } }),
            vec![
                "SupportBot",
                "RetailNetwork",
                "OrderPizzaBot",
                "OrderCoffeeBot",
            ],
        ),
        (
            "sorted by name ascending",
            json!({ "sortBy": { "attribute": "BotName", "order": "Ascending" } }),
            vec![
                "OrderCoffeeBot",
                "OrderPizzaBot",
                "RetailNetwork",
                "SupportBot",
            ],
        ),
        (
            "name equals",
            json!({ "filters": [{ "name": "BotName", "operator": "EQ", "values": ["SupportBot"] }] }),
            vec!["SupportBot"],
        ),
        (
            "name contains",
            json!({ "filters": [{ "name": "BotName", "operator": "CO", "values": ["Order"] }] }),
            vec!["OrderCoffeeBot", "OrderPizzaBot"],
        ),
        (
            "name not equal",
            json!({ "filters": [{ "name": "BotName", "operator": "NE", "values": ["SupportBot"] }] }),
            vec!["OrderCoffeeBot", "OrderPizzaBot", "RetailNetwork"],
        ),
        (
            "type equals BotNetwork",
            json!({ "filters": [{ "name": "BotType", "operator": "EQ", "values": ["BotNetwork"] }] }),
            vec!["RetailNetwork"],
        ),
        (
            "type equals Bot",
            json!({ "filters": [{ "name": "BotType", "operator": "EQ", "values": ["Bot"] }] }),
            vec!["OrderCoffeeBot", "OrderPizzaBot", "SupportBot"],
        ),
        (
            "two filters together",
            json!({ "filters": [
                { "name": "BotName", "operator": "CO", "values": ["Bot"] },
                { "name": "BotType", "operator": "EQ", "values": ["Bot"] },
            ] }),
            vec!["OrderCoffeeBot", "OrderPizzaBot", "SupportBot"],
        ),
        (
            "a filter nothing matches",
            json!({ "filters": [{ "name": "BotName", "operator": "EQ", "values": ["NoSuchBot"] }] }),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, listed) = call(&router, "POST", "/bots", Some(request.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListBots {request:?} must answer 200, got {listed:?}"
        );
        let names: Vec<String> = listed
            .get("botSummaries")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| text(summary, "botName"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names, expected,
            "case {case:?}: ListBots {request:?} selected the wrong bots"
        );
    }
}

#[tokio::test]
async fn an_unknown_bot_id_is_refused_with_the_error_each_operation_models() {
    let router = emulator();

    // A bot that does exist, so the refusals below are about the id and nothing else.
    create_bot(&router, &create_body("PresentBot", 300, json!({}))).await;

    let update = json!({
        "botName": "PresentBot",
        "roleArn": ROLE_ARN,
        "dataPrivacy": { "childDirected": false },
        "idleSessionTTLInSeconds": 300,
    });

    // DescribeBot models ResourceNotFoundException; UpdateBot and DeleteBot do not and
    // answer PreconditionFailedException (HTTP 412) instead
    // (`smithy_query(query="operation",operation="DescribeBot,UpdateBot,DeleteBot")`).
    let cases: Vec<(&str, &str, Option<Value>, u16, &str)> = vec![
        (
            "DescribeBot",
            "GET",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateBot",
            "PUT",
            Some(update),
            412,
            "PreconditionFailedException",
        ),
        (
            "DeleteBot",
            "DELETE",
            None,
            412,
            "PreconditionFailedException",
        ),
    ];

    // Ids that satisfy the modelled `Id` shape but name no stored bot.
    for unknown in ["0000000000", "zzzzzzzzzz", "A1B2C3D4E5"] {
        for (operation, method, body, expected_status, expected_code) in &cases {
            let (status, response) = call(
                &router,
                method,
                &format!("/bots/{unknown}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                status, *expected_status,
                "{operation} on unknown id {unknown}: expected {expected_status}, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                *expected_code,
                "{operation} on unknown id {unknown}: expected {expected_code}, got {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_bot_name_stays_unique_in_the_account() {
    let router = emulator();

    let first = create_bot(&router, &create_body("TakenName", 300, json!({}))).await;
    let second = create_bot(&router, &create_body("OtherName", 300, json!({}))).await;

    // Creating a second bot with a name already in use is refused.
    let (status, response) = call(
        &router,
        "PUT",
        "/bots",
        Some(create_body("TakenName", 600, json!({}))),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate botName must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "expected ConflictException for a duplicate botName, got {response:?}"
    );

    // Renaming a bot onto a name already in use is refused the same way.
    let rename = json!({
        "botName": "TakenName",
        "roleArn": ROLE_ARN,
        "dataPrivacy": { "childDirected": false },
        "idleSessionTTLInSeconds": 300,
    });
    let (status, response) =
        call(&router, "PUT", &format!("/bots/{second}"), Some(rename)).await;
    assert_eq!(
        status, 409,
        "a rename onto a taken botName must be refused, got {status} {response:?}"
    );

    // And a bot may be updated under its OWN name.
    let same_name = json!({
        "botName": "TakenName",
        "roleArn": ROLE_ARN,
        "dataPrivacy": { "childDirected": false },
        "idleSessionTTLInSeconds": 7_200,
    });
    let (status, response) =
        call(&router, "PUT", &format!("/bots/{first}"), Some(same_name)).await;
    assert_eq!(
        status, 202,
        "updating a bot under its own name must succeed, got {status} {response:?}"
    );
}

#[tokio::test]
async fn a_bot_a_network_still_lists_is_only_deleted_when_the_check_is_skipped() {
    let router = emulator();

    let member_id = create_bot(&router, &create_body("MemberBot", 300, json!({}))).await;
    create_bot(
        &router,
        &create_body(
            "OwningNetwork",
            300,
            json!({
                "botType": "BotNetwork",
                "botMembers": [{
                    "botMemberId": member_id,
                    "botMemberName": "MemberBot",
                    "botMemberAliasId": "TSTALIASID",
                    "botMemberAliasName": "AmazonLexTestAlias",
                    "botMemberVersion": "DRAFT",
                }],
            }),
        ),
    )
    .await;

    let (status, response) =
        call(&router, "DELETE", &format!("/bots/{member_id}"), None).await;
    assert_eq!(
        status, 409,
        "a bot in use by a network must not be deleted by default, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "expected ConflictException for a bot still in use, got {response:?}"
    );

    // It is still there.
    let (status, _) = call(&router, "GET", &format!("/bots/{member_id}"), None).await;
    assert_eq!(status, 200, "the refused delete must not have removed the bot");

    let (status, response) = call(
        &router,
        "DELETE",
        &format!("/bots/{member_id}?skipResourceInUseCheck=true"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "skipResourceInUseCheck must delete the bot anyway, got {status} {response:?}"
    );
    let (status, _) = call(&router, "GET", &format!("/bots/{member_id}"), None).await;
    assert_eq!(status, 404, "the skipped-check delete must have removed the bot");
}

#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let router = emulator();

    let bot_id = create_bot(&router, &create_body("ConstrainedBot", 300, json!({}))).await;

    // Rows outside a published bound, or outside a modelled enum's value set. Each must
    // be refused as a 400 ValidationException rather than stored.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "session TTL one below the published minimum",
            "PUT",
            "/bots".to_string(),
            Some(create_body("TooShortTtlBot", 59, json!({}))),
        ),
        (
            "session TTL one above the published maximum",
            "PUT",
            "/bots".to_string(),
            Some(create_body("TooLongTtlBot", 86_401, json!({}))),
        ),
        (
            "a botType outside the modelled enum",
            "PUT",
            "/bots".to_string(),
            Some(create_body(
                "BadTypeBot",
                300,
                json!({ "botType": "Robot" }),
            )),
        ),
        (
            "a name longer than the published maximum",
            "PUT",
            "/bots".to_string(),
            Some(create_body(&name_of_length(101), 300, json!({}))),
        ),
        (
            "a roleArn that is not an IAM role ARN",
            "PUT",
            "/bots".to_string(),
            Some(json!({
                "botName": "BadRoleBot",
                "roleArn": "arn:aws:iam::123456789012:user/not-a-role-but-long-enough",
                "dataPrivacy": { "childDirected": false },
                "idleSessionTTLInSeconds": 300,
            })),
        ),
        (
            "a sort attribute outside the modelled enum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "sortBy": { "attribute": "BotId", "order": "Ascending" } })),
        ),
        (
            "a sort order outside the modelled enum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "sortBy": { "attribute": "BotName", "order": "Sideways" } })),
        ),
        (
            "a filter operator outside the modelled enum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "filters": [{ "name": "BotName", "operator": "LT", "values": ["x"] }] })),
        ),
        (
            "a filter name outside the modelled enum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "filters": [{ "name": "BotColour", "operator": "EQ", "values": ["x"] }] })),
        ),
        (
            "a nextToken that is not a token this operation issued",
            "POST",
            "/bots".to_string(),
            Some(json!({ "maxResults": 1, "nextToken": "9999999999" })),
        ),
        (
            "maxResults one below the published minimum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "maxResults": 0 })),
        ),
        (
            "a negative maxResults",
            "POST",
            "/bots".to_string(),
            Some(json!({ "maxResults": -1 })),
        ),
        (
            "maxResults one above the published maximum",
            "POST",
            "/bots".to_string(),
            Some(json!({ "maxResults": 1_001 })),
        ),
        (
            "a botId shorter than the modelled length",
            "GET",
            "/bots/short".to_string(),
            None,
        ),
        (
            "an update that drops a required member",
            "PUT",
            format!("/bots/{bot_id}"),
            Some(json!({ "botName": "ConstrainedBot" })),
        ),
    ];

    for (case, method, uri, body) in cases {
        let (status, response) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // None of the refusals stored anything: only the one bot created above is listed.
    let (_, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    assert_eq!(
        listed_ids(&listed),
        vec![bot_id],
        "a refused request must not store a bot, got {listed:?}"
    );
}
